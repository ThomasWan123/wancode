//! Embedded grok-build agent session for the WanCode GUI.
//!
//! Mirrors the lifecycle used by `xai-grok-pager`'s headless mode
//! (init → authenticate → new session → prompt), but pumps every ACP
//! notification to the frontend as Tauri events instead of stdout:
//!
//! - `agent://update`      — session updates (message/thought/tool chunks)
//! - `agent://permission`  — tool-call approval requests (answered via
//!   the `agent_permission_respond` command)
//! - `agent://turn-end`    — a prompt turn finished (with stop reason or error)

use std::collections::HashMap;
use std::path::PathBuf;

use crate::crash_recovery::write_session_marker;
use crate::git_ops::{git_stash, git_status_ext, session_git_root};
use crate::provider_ops::{inject_managed_keys};
use crate::config_core::{
    apply_provider_preset, wancode_env_key, seed_default_mcp_into, user_config_path, validate_startup_models,
    write_config_atomic, StartupModels,
};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{Mutex, oneshot};
use tokio_util::sync::CancellationToken;

use agent_client_protocol as acp;
use xai_acp_lib::{AcpAgentTx, AcpClientMessage, acp_send};
use xai_grok_pager::acp::spawn::spawn_grok_shell;
use xai_grok_shell::agent::auth_method::AuthMethodKind;
use xai_grok_shell::agent::config::Config as AgentConfig;

/// 计划审批回包：(outcome, feedback)。
type PlanReply = (String, Option<String>);
/// 提问回包：问题文本 → 选中项列表；None = 用户取消。
type QuestionReply = Option<HashMap<String, Vec<String>>>;

pub struct AgentHandle {
    pub(crate) acp_tx: AcpAgentTx,
    pub(crate) session_id: acp::SessionId,
    cancel: CancellationToken,
    /// 会话工作区。git 命令用它本地解析 gitRoot（见 session_git_root）。
    pub cwd: PathBuf,
}

#[derive(Default)]
pub struct AgentState {
    pub(crate) handle: Mutex<Option<AgentHandle>>,
    pending_permissions: Mutex<HashMap<u64, oneshot::Sender<Option<String>>>>,
    next_permission_id: AtomicU64,
    /// Pending `x.ai/exit_plan_mode` approvals → (outcome, feedback).
    pending_plans: Mutex<HashMap<u64, oneshot::Sender<PlanReply>>>,
    /// Pending `x.ai/ask_user_question` requests: answers keyed by question text.
    pending_questions: Mutex<HashMap<u64, oneshot::Sender<QuestionReply>>>,
    /// Pending `x.ai/folder_trust/request` prompts → true = trust.
    pending_trust: Mutex<HashMap<u64, oneshot::Sender<bool>>>,
    /// 后台工作会话（Review 等）：通知泵对这些会话不发 agent://update，
    /// 权限请求一律自动取消——它们绝不能污染主聊天或卡在前端审批上。
    background_sessions: Mutex<std::collections::HashSet<String>>,
}

#[derive(Serialize, Clone)]
pub struct StartResult {
    pub session_id: String,
    pub models: Vec<String>,
    /// 会话真实 cwd——前端必须用它当工作区标签（#83：标签来自
    /// localStorage 而会话另有其主时，面板显示的是别的仓库）。
    pub cwd: String,
}

#[derive(Serialize, Clone)]
pub struct SessionEntry {
    pub session_id: String,
    pub title: String,
    pub updated_at: String,
    pub num_messages: usize,
    pub model_id: Option<String>,
}

/// List locally stored sessions for a workspace (newest first).
#[tauri::command]
pub async fn agent_list_sessions(workspace: String) -> Result<Vec<SessionEntry>, String> {
    let sessions =
        xai_grok_shell::session::merge::fetch_merged(None, Some(&workspace), None, 30).await;
    Ok(sessions
        .into_iter()
        .map(|s| SessionEntry {
            title: if s.summary.is_empty() {
                s.first_prompt.clone().unwrap_or_else(|| "(未命名会话)".into())
            } else {
                s.summary.clone()
            },
            session_id: s.session_id,
            updated_at: s.updated_at,
            num_messages: s.num_messages,
            model_id: s.model_id,
        })
        .collect())
}

/// List MCP servers configured for a workspace (from config.toml / .mcp.json).
#[tauri::command]
pub async fn agent_list_mcp(workspace: String) -> Result<Vec<String>, String> {
    let cwd = PathBuf::from(&workspace);
    let servers = xai_grok_shell::util::config::load_mcp_servers(
        &cwd,
        &xai_grok_tools::types::compat::CompatConfig::default(),
    );
    Ok(servers
        .iter()
        .map(|s| {
            serde_json::to_value(s)
                .ok()
                .and_then(|v| v.get("name").and_then(|n| n.as_str()).map(String::from))
                .unwrap_or_else(|| "(unnamed)".into())
        })
        .collect())
}

/// Start (or restart) an embedded agent session rooted at `workspace`.
#[tauri::command]
pub async fn agent_start(
    app: AppHandle,
    state: State<'_, AgentState>,
    workspace: String,
    model: Option<String>,
    resume: Option<String>,
) -> Result<StartResult, String> {
    // smoke 模式：前端不许动会话。debug 构建的 webview 若碰到活着的 dev
    // server 会加载完整前端并自动启动会话，把 autotest 的 handle 换成
    // localStorage 工作区（宿主仓库！）——run3 的 stash 事故 + S2/S4 全部
    // 抖动皆源于此。autotest 走 start_inner 内部路径，不经过这里。
    if std::env::var("WANCODE_AUTOTEST").is_ok() {
        return Err("AUTOTEST 模式：前端会话启动被禁用".into());
    }
    // Tear down any previous session first.
    if let Some(old) = state.handle.lock().await.take() {
        old.cancel.cancel();
    }

    let result = start_inner(app, &state, workspace, model, resume)
        .await
        .map_err(|e| format!("{e:#}"))?;
    Ok(result)
}

async fn start_inner(
    app: AppHandle,
    state: &State<'_, AgentState>,
    workspace: String,
    model: Option<String>,
    resume: Option<String>,
) -> Result<StartResult> {
    // Make WanCode-managed API keys (stored in the OS keyring) visible to the
    // engine's `env_key` resolution for this process.
    // ── 启动不变量（v0.12.2）：零模型绝不进入引擎 ─────────────────
    // 引擎在零模型状态下启动即 panic（capacity overflow / RefCell 双崩，
    // 实测）。此前的门控只在前端——恢复会话/切工作区/删最后一个模型后
    // 继续操作都可能绕过它直达这里。校验必须住在所有入口的必经之路上。
    // 错误码是前端契约：MODEL_REQUIRED → 重开向导；MODEL_CONFIG_INVALID
    // → 提示修配置。改动前先跑 config 单测。
    match validate_startup_models() {
        StartupModels::Ok => {}
        StartupModels::NoModels => {
            return Err(anyhow!("MODEL_REQUIRED: 尚未配置任何模型"));
        }
        StartupModels::RepairedDefault(fixed) => {
            tracing::warn!("[models].default 悬空，已自动修复为 {fixed}");
        }
        StartupModels::Invalid(reason) => {
            return Err(anyhow!("MODEL_CONFIG_INVALID: {reason}"));
        }
    }

    inject_managed_keys();
    let cwd = PathBuf::from(&workspace);
    if !cwd.is_dir() {
        return Err(anyhow!("工作区目录不存在: {workspace}"));
    }

    // 先拆掉旧会话。此前旧 handle 一直留到函数末尾才被替换——本次启动
    // 半路失败时它就成了僵尸：前端以为没会话/换了工作区，ext 调用却仍
    // 注入旧 sessionId，git 面板显示的是**另一个仓库**的改动（#83，
    // 在那个状态下 stash/丢弃会打错目标）。失败宁可「会话未启动」。
    if let Some(old) = state.handle.lock().await.take() {
        old.cancel.cancel();
    }

    // ── Config (mirrors headless.rs) ────────────────────────────────
    let raw_config =
        xai_grok_shell::config::load_effective_config().map_err(|e| anyhow!("加载配置失败: {e}"))?;
    let mut agent_config =
        AgentConfig::new_from_toml_cfg(&raw_config).map_err(|e| anyhow!("解析配置失败: {e}"))?;
    if let Some(ref m) = model {
        agent_config.default_model_override = Some(m.clone());
    }
    agent_config.resolve_runtime_fields(&xai_grok_shell::agent::config::RuntimeResolutionContext {
        raw_config: &raw_config,
        remote_settings: None,
        cwd: Some(&cwd),
        is_headless: true,
        cli_subagents: None,
        cli_web_search_model: None,
        cli_session_summary_model: None,
        cli_experimental_memory: false,
        cli_no_memory: false,
        disable_web_search: false,
        todo_gate: false,
        laziness_debug_log: None,
        storage_mode: None,
    });
    agent_config.mode = xai_grok_shell::agent::config::AgentMode::Headless;
    // GUI answers permission requests explicitly — never yolo.
    agent_config.default_yolo_mode = false;
    agent_config.default_auto_mode =
        xai_grok_shell::util::config::effective_auto_for_launch(false, None, None);

    // NOTE: we deliberately do NOT grant_folder_trust() here.
    //
    // That blanket grant was written when opening a workspace always meant the
    // user had just picked it in the folder dialog. Since 0.8.2 WanCode
    // auto-opens the last-used folder (or the home directory on first run), so
    // the grant was trusting folders the user never approved — and folder trust
    // is what gates repo-local MCP servers and LSP, i.e. config a cloned repo
    // can ship to make the agent run things.
    //
    // Instead we advertise `x.ai/folderTrust.interactive` below and let the
    // engine prompt through `x.ai/folder_trust/request`. The engine keeps
    // project-scoped config gated until an explicit grant, and treats any
    // undecodable answer as reject.

    let cancel = CancellationToken::new();
    let memory_config = agent_config.memory_config.clone();
    let spawned = spawn_grok_shell(agent_config, &cancel, memory_config)
        .await
        .map_err(|e| anyhow!("启动 Agent 失败: {e}"))?;
    let acp_tx = spawned.channel.tx;
    let mut acp_rx = spawned.channel.rx;

    // ── Initialize ─────────────────────────────────────────────────
    // The trust capability is read from `client_capabilities.meta`, NOT the
    // request meta — putting it on the request silently does nothing.
    let mut caps = acp::ClientCapabilities::new()
        .fs(acp::FileSystemCapabilities::new())
        .terminal(false);
    caps.meta = serde_json::json!({ "x.ai/folderTrust": { "interactive": true } })
        .as_object()
        .cloned();

    let init_req = acp::InitializeRequest::new(acp::ProtocolVersion::V1)
        .client_capabilities(caps)
        .meta(
            serde_json::json!({
                "clientType": "wancode",
                "clientVersion": env!("CARGO_PKG_VERSION"),
                "startupHints": {
                    "nonInteractive": true,
                    "skipGitStatus": false,
                    "skipProjectLayout": false,
                },
            })
            .as_object()
            .cloned(),
        );
    let init_resp: acp::InitializeResponse = acp_send(init_req, &acp_tx)
        .await
        .map_err(|e| anyhow!("ACP initialize 失败: {e}"))?;

    // ── Authenticate (non-interactive methods only) ─────────────────
    let method_id = init_resp
        .auth_methods
        .iter()
        .find(|m| !AuthMethodKind::from_id(m.id()).needs_interactive_login())
        .map(|m| m.id().clone())
        .context("没有可用的非交互认证方式（请在 ~/.grok/config.toml 配置模型 API Key）")?;
    let _: acp::AuthenticateResponse = acp_send(
        acp::AuthenticateRequest::new(method_id)
            .meta(serde_json::json!({"headless": true}).as_object().cloned()),
        &acp_tx,
    )
    .await
    .map_err(|e| anyhow!("认证失败: {e}"))?;

    // ── Event pump: ACP notifications → Tauri events ───────────────
    // Must start BEFORE the session opens: resuming a session replays
    // history notifications during LoadSession, and each notification
    // waits for a response — with no consumer that deadlocks.
    {
        let app = app.clone();
        let pump_cancel = cancel.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::select! {
                    _ = pump_cancel.cancelled() => break,
                    msg = acp_rx.recv() => {
                        let Some(msg) = msg else { break };
                        handle_acp_message(&app, msg).await;
                    }
                }
            }
        });
    }

    // ── Open session (new or resume-with-replay) ───────────────────
    let mcp_servers = xai_grok_shell::util::config::load_mcp_servers(
        &cwd,
        &xai_grok_tools::types::compat::CompatConfig::default(),
    );
    let (session_id, session_models) = if let Some(sid) = resume {
        let resp: acp::LoadSessionResponse = acp_send(
            acp::LoadSessionRequest::new(acp::SessionId::new(sid.clone()), cwd.clone())
                .mcp_servers(mcp_servers),
            &acp_tx,
        )
        .await
        .map_err(|e| anyhow!("恢复会话失败: {e}"))?;
        (acp::SessionId::new(sid), resp.models)
    } else {
        let resp: acp::NewSessionResponse = acp_send(
            acp::NewSessionRequest::new(cwd.clone()).mcp_servers(mcp_servers),
            &acp_tx,
        )
        .await
        .map_err(|e| anyhow!("创建会话失败: {e}"))?;
        (resp.session_id, resp.models)
    };
    let model_ids: Vec<String> = session_models
        .map(|m| m.available_models.iter().map(|am| am.model_id.0.to_string()).collect())
        .unwrap_or_default();

    *state.handle.lock().await = Some(AgentHandle {
        acp_tx: acp_tx.clone(),
        session_id: session_id.clone(),
        cancel,
        cwd: cwd.clone(),
    });

    // 新会话的技能来自 agent 启动时的内存快照（self.cfg.skills），运行期改
    // 的 [skills].disabled 它看不见——引擎没有任何回灌路径。开一个会话就补
    // 发一次 refresh-baseline，让它立刻从磁盘配置重新同步。失败无所谓：
    // 最坏就是退回旧行为。
    {
        let raw = serde_json::value::to_raw_value(&serde_json::json!({})).expect("static json");
        let _ = acp_send(
            acp::ExtRequest::new("x.ai/skills/refresh-baseline".to_string(), raw.into()),
            &acp_tx,
        )
        .await as Result<acp::ExtResponse, _>;
    }

    write_session_marker(&session_id.0, &cwd.to_string_lossy(), false);

    Ok(StartResult {
        session_id: session_id.0.to_string(),
        models: model_ids,
        cwd: cwd.to_string_lossy().into_owned(),
    })
}

async fn handle_acp_message(app: &AppHandle, msg: AcpClientMessage) {
    match msg {
        AcpClientMessage::SessionNotification(boxed) => {
            // 后台会话（Review 等）的更新不进主聊天流
            {
                let state: State<'_, AgentState> = app.state();
                let bg = state.background_sessions.lock().await;
                if bg.contains(boxed.request.session_id.0.as_ref()) {
                    let _ = boxed.response_tx.send(Ok(()));
                    return;
                }
            }
            let payload =
                serde_json::to_value(&boxed.request.update).unwrap_or(serde_json::Value::Null);
            if std::env::var("WANCODE_AUTOTEST").is_ok() {
                use std::io::Write;
                let kind = payload
                    .get("sessionUpdate")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
                    .to_string();
                let log = std::env::temp_dir().join("wancode-autotest.log");
                if let Ok(mut f) =
                    std::fs::OpenOptions::new().create(true).append(true).open(&log)
                {
                    let _ = writeln!(f, "update: {kind}");
                }
            }
            let _ = app.emit("agent://update", payload);
            let _ = boxed.response_tx.send(Ok(()));
        }
        AcpClientMessage::RequestPermission(req) => {
            // 无头 smoke：自动选第一个选项（引擎约定首项为放行），否则
            // S3/S4 的命令权限会等前端 600 秒。仅 AUTOTEST 模式生效。
            if std::env::var("WANCODE_AUTOTEST").is_ok() {
                let first = req.request.options.first().map(|o| o.option_id.clone());
                let outcome = match first {
                    Some(id) => acp::RequestPermissionOutcome::Selected(
                        acp::SelectedPermissionOutcome::new(id),
                    ),
                    None => acp::RequestPermissionOutcome::Cancelled,
                };
                let _ = req
                    .response_tx
                    .send(Ok(acp::RequestPermissionResponse::new(outcome)));
                return;
            }
            let state: State<'_, AgentState> = app.state();
            // 后台会话理论上是只读（plan）模式；万一有工具越权申请，
            // 直接取消而不是等前端 600 秒——前端根本看不见这个会话。
            if state
                .background_sessions
                .lock()
                .await
                .contains(req.request.session_id.0.as_ref())
            {
                let _ = req.response_tx.send(Ok(acp::RequestPermissionResponse::new(
                    acp::RequestPermissionOutcome::Cancelled,
                )));
                return;
            }
            let id = state.next_permission_id.fetch_add(1, Ordering::Relaxed);
            let (tx, rx) = oneshot::channel::<Option<String>>();
            state.pending_permissions.lock().await.insert(id, tx);

            let payload = serde_json::json!({
                "id": id,
                "request": serde_json::to_value(&req.request).unwrap_or(serde_json::Value::Null),
            });
            let _ = app.emit("agent://permission", payload);

            // Wait for the frontend's decision (10 min timeout → cancel).
            tauri::async_runtime::spawn(async move {
                let decision =
                    tokio::time::timeout(std::time::Duration::from_secs(600), rx).await;
                let outcome = match decision {
                    Ok(Ok(Some(option_id))) => acp::RequestPermissionOutcome::Selected(
                        acp::SelectedPermissionOutcome::new(acp::PermissionOptionId::new(
                            option_id,
                        )),
                    ),
                    _ => acp::RequestPermissionOutcome::Cancelled,
                };
                let _ = req
                    .response_tx
                    .send(Ok(acp::RequestPermissionResponse::new(outcome)));
            });
        }
        AcpClientMessage::ExtNotification(notif) => {
            let payload = serde_json::json!({
                "method": notif.request.method.to_string(),
                "params": serde_json::to_value(&notif.request.params).unwrap_or(serde_json::Value::Null),
            });
            let _ = app.emit("agent://ext", payload);
            let _ = notif.response_tx.send(Ok(()));
        }
        AcpClientMessage::ExtMethod(args) => {
            // 后台会话的交互型 ext 请求（exit_plan_mode / ask_user_question /
            // folder_trust）绝不能弹到前端——用户根本看不见那个会话。
            // 统一自动应答：计划直接放行、提问回空、信任拒绝。
            // （实测教训：Review 子会话在 plan 模式收尾时，把审查 JSON 当
            // "计划"弹进了主 UI 的审批框。）
            {
                let params: serde_json::Value = serde_json::from_str(args.request.params.get())
                    .unwrap_or(serde_json::Value::Null);
                let sid = params
                    .get("sessionId")
                    .or_else(|| params.get("session_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let state: State<'_, AgentState> = app.state();
                if !sid.is_empty() && state.background_sessions.lock().await.contains(sid) {
                    let resp = match args.request.method.as_ref() {
                        "x.ai/exit_plan_mode" => {
                            serde_json::json!({ "outcome": "approved", "feedback": null })
                        }
                        _ => serde_json::json!({}),
                    };
                    let raw = serde_json::value::to_raw_value(&resp).unwrap();
                    let _ = args.response_tx.send(Ok(acp::ExtResponse::new(raw.into())));
                    return;
                }
            }
            if args.request.method.as_ref() == "x.ai/exit_plan_mode" {
                let params: serde_json::Value =
                    serde_json::from_str(args.request.params.get()).unwrap_or(serde_json::Value::Null);
                let plan = params
                    .get("planContent")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let state: State<'_, AgentState> = app.state();
                let id = state.next_permission_id.fetch_add(1, Ordering::Relaxed);
                let (tx, rx) = oneshot::channel::<(String, Option<String>)>();
                state.pending_plans.lock().await.insert(id, tx);
                let _ = app.emit(
                    "agent://plan-approval",
                    serde_json::json!({ "id": id, "planContent": plan }),
                );
                tauri::async_runtime::spawn(async move {
                    let (outcome, feedback) =
                        match tokio::time::timeout(std::time::Duration::from_secs(600), rx).await {
                            Ok(Ok(v)) => v,
                            _ => ("cancelled".to_string(), None),
                        };
                    let resp = serde_json::json!({ "outcome": outcome, "feedback": feedback });
                    let raw = serde_json::value::to_raw_value(&resp).unwrap();
                    let _ = args.response_tx.send(Ok(acp::ExtResponse::new(raw.into())));
                });
            } else if args.request.method.as_ref() == "x.ai/ask_user_question" {
                // The agent is asking the user something. Previously this fell
                // into the catch-all below and got answered with `{}` — the
                // question never reached the user and the model saw a blank.
                let params: serde_json::Value =
                    serde_json::from_str(args.request.params.get()).unwrap_or(serde_json::Value::Null);
                let questions = params
                    .get("questions")
                    .cloned()
                    .unwrap_or(serde_json::Value::Array(vec![]));
                let state: State<'_, AgentState> = app.state();
                let id = state.next_permission_id.fetch_add(1, Ordering::Relaxed);
                let (tx, rx) =
                    oneshot::channel::<Option<HashMap<String, Vec<String>>>>();
                state.pending_questions.lock().await.insert(id, tx);
                let _ = app.emit(
                    "agent://ask-question",
                    serde_json::json!({ "id": id, "questions": questions }),
                );
                tauri::async_runtime::spawn(async move {
                    let answered =
                        match tokio::time::timeout(std::time::Duration::from_secs(600), rx).await {
                            Ok(Ok(v)) => v,
                            _ => None,
                        };
                    // Tagged on "outcome" — see AskUserQuestionExtResponse.
                    let resp = match answered {
                        Some(answers) => {
                            serde_json::json!({ "outcome": "accepted", "answers": answers })
                        }
                        None => serde_json::json!({ "outcome": "cancelled" }),
                    };
                    let raw = serde_json::value::to_raw_value(&resp).unwrap();
                    let _ = args.response_tx.send(Ok(acp::ExtResponse::new(raw.into())));
                });
            } else if args.request.method.as_ref() == "x.ai/folder_trust/request" {
                // 引擎问：这个工作区里有 repo 自带的 MCP/hooks/LSP 配置，
                // 要不要信任？未信任前引擎已把这些配置挡住了。
                let params: serde_json::Value =
                    serde_json::from_str(args.request.params.get()).unwrap_or(serde_json::Value::Null);
                let state: State<'_, AgentState> = app.state();
                let id = state.next_permission_id.fetch_add(1, Ordering::Relaxed);
                let (tx, rx) = oneshot::channel::<bool>();
                state.pending_trust.lock().await.insert(id, tx);
                let _ = app.emit(
                    "agent://folder-trust",
                    serde_json::json!({
                        "id": id,
                        "workspace": params.get("workspace").and_then(|v| v.as_str()).unwrap_or(""),
                        "cwd": params.get("cwd").and_then(|v| v.as_str()).unwrap_or(""),
                        "configKinds": params.get("configKinds").cloned()
                            .unwrap_or(serde_json::Value::Array(vec![])),
                    }),
                );
                tauri::async_runtime::spawn(async move {
                    // 超时/关闭一律按拒绝——引擎也把任何无法解码的回复当拒绝。
                    let trusted =
                        matches!(tokio::time::timeout(std::time::Duration::from_secs(600), rx).await,
                            Ok(Ok(true)));
                    let resp = serde_json::json!({
                        "outcome": if trusted { "trust" } else { "reject" }
                    });
                    let raw = serde_json::value::to_raw_value(&resp).unwrap();
                    let _ = args.response_tx.send(Ok(acp::ExtResponse::new(raw.into())));
                });
            } else {
                // Unknown reverse ext-request: answer with empty ok so the
                // agent-side tool call doesn't hang/fail.
                let raw = serde_json::value::to_raw_value(&serde_json::json!({})).unwrap();
                let _ = args.response_tx.send(Ok(acp::ExtResponse::new(raw.into())));
            }
        }
        _ => {}
    }
}

/// Answer a pending plan-mode approval (`x.ai/exit_plan_mode`).
/// `outcome`: "approved" | "cancelled" | "abandoned".
#[tauri::command]
pub async fn agent_plan_respond(
    state: State<'_, AgentState>,
    id: u64,
    outcome: String,
    feedback: Option<String>,
) -> Result<(), String> {
    let sender = state.pending_plans.lock().await.remove(&id);
    match sender {
        Some(tx) => {
            let _ = tx.send((outcome, feedback));
            Ok(())
        }
        None => Err(format!("没有待处理的计划审批 #{id}")),
    }
}

/// Self-test driven by `WANCODE_AUTOTEST=<workspace-dir>`: exercises the full
/// backend glue (start → prompt → events) without the UI and logs the result
/// Headless smoke suite (v0.13 refactor safety net).
///
/// `WANCODE_AUTOTEST=<fixture-dir>` 启动即运行：6 个场景全部走真实引擎，
/// 断言全部落在磁盘/git2 层（无 UI 依赖，坐标点击的维护成本教训）。
/// 结果写 %TEMP%/wancode-autotest.log，结尾一行 `SMOKE DONE pass=N fail=M`，
/// 随后进程自杀（scripts/smoke.ps1 轮询日志取结果）。
pub async fn autotest(app: AppHandle, workspace: String) {
    let log = std::env::temp_dir().join("wancode-autotest.log");
    let _ = std::fs::remove_file(&log);
    let write = |s: &str| {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log) {
            let _ = writeln!(f, "{s}");
        }
    };
    let mut pass = 0u32;
    let mut fail = 0u32;
    macro_rules! check {
        ($name:expr, $ok:expr, $detail:expr) => {{
            let ok: bool = $ok;
            if ok { pass += 1 } else { fail += 1 }
            write(&format!("SMOKE {} {}: {}", $name, if ok { "PASS" } else { "FAIL" }, $detail));
        }};
    }

    let state: State<'_, AgentState> = app.state();

    // ── S1 会话启动（默认模型）──────────────────────────────────────
    let started = start_inner(app.clone(), &state, workspace.clone(), None, None).await;
    let (sid, cwd) = match &started {
        Ok(r) => {
            check!("S1-start", true, format!("session={}", r.session_id));
            (r.session_id.clone(), r.cwd.clone())
        }
        Err(e) => {
            check!("S1-start", false, format!("{e:#}"));
            write(&format!("SMOKE DONE pass={pass} fail={fail}"));
            std::process::exit(1);
        }
    };
    let sessions_base = xai_grok_shell::util::grok_home::grok_home().join("sessions");
    let chat_text = || -> String {
        walkdir_find(&sessions_base, &sid)
            .map(|d| d.join("chat_history.jsonl"))
            .and_then(|f| std::fs::read_to_string(f).ok())
            .unwrap_or_default()
    };
    let acp_tx = {
        let g = state.handle.lock().await;
        g.as_ref().unwrap().acp_tx.clone()
    };
    let send = |text: String| {
        let tx = acp_tx.clone();
        let sid = acp::SessionId::new(sid.clone());
        async move {
            let blocks = vec![acp::ContentBlock::Text(acp::TextContent::new(text))];
            {
                let r: Result<acp::PromptResponse, _> =
                    acp_send(acp::PromptRequest::new(sid, blocks), &tx).await;
                r
            }
        }
    };

    // ── S2 基本回复 ────────────────────────────────────────────────
    let r = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        send("reply with exactly: SMOKE-BASIC".into()),
    )
    .await;
    let detail = match &r {
        Err(_) => "timeout-120s".to_string(),
        Ok(Err(e)) => format!("err={e}"),
        Ok(Ok(resp)) => format!("stop={:?}", resp.stop_reason),
    };
    let ok = matches!(&r, Ok(Ok(_))) && chat_text().contains("SMOKE-BASIC");
    check!("S2-reply", ok, detail);

    // ── S3 忙时排队（长任务 + 两条排队，全部完成且顺序保留）────────
    let long = tauri::async_runtime::spawn(send("Run the command ping -n 8 127.0.0.1 once, then reply SMOKE-LONG".into()));
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let qa = tauri::async_runtime::spawn(send("reply with exactly: SMOKE-QA".into()));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let qb = tauri::async_runtime::spawn(send("reply with exactly: SMOKE-QB".into()));
    let _ = tokio::time::timeout(std::time::Duration::from_secs(180), long).await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(60), qa).await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(60), qb).await;
    let text = chat_text();
    let order_ok = match (text.find("SMOKE-QA"), text.find("SMOKE-QB")) {
        (Some(a), Some(b)) => a < b,
        _ => false,
    };
    check!(
        "S3-queue",
        text.contains("SMOKE-LONG") && order_ok,
        format!("long={} order={order_ok}", text.contains("SMOKE-LONG"))
    );

    // ── S4 回合中插话 ──────────────────────────────────────────────
    let long2 = tauri::async_runtime::spawn(send("Run the command ping -n 20 127.0.0.1 once, then reply SMOKE-D".into()));
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    let ij = ext_call(
        &state,
        "x.ai/interject",
        serde_json::json!({ "text": "Stop now. Reply with exactly: SMOKE-IJ" }),
    )
    .await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(180), long2).await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    let ok = ij.is_ok() && chat_text().contains("SMOKE-IJ");
    check!("S4-interject", ok, format!("call={}", ij.is_ok()));

    // ── S5 Git 状态 + 贮藏（git2 断言，不依赖 git CLI）────────────
    let fixture = (|| -> Result<(), String> {
        let repo = git2::Repository::init(&cwd).map_err(|e| e.to_string())?;
        let f = std::path::Path::new(&cwd).join("smoke.txt");
        std::fs::write(&f, "base").map_err(|e| e.to_string())?;
        let mut idx = repo.index().map_err(|e| e.to_string())?;
        idx.add_path(std::path::Path::new("smoke.txt")).map_err(|e| e.to_string())?;
        idx.write().map_err(|e| e.to_string())?;
        let tree_id = idx.write_tree().map_err(|e| e.to_string())?;
        let tree = repo.find_tree(tree_id).map_err(|e| e.to_string())?;
        let sig = git2::Signature::now("smoke", "smoke@t").map_err(|e| e.to_string())?;
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .map_err(|e| e.to_string())?;
        std::fs::write(&f, "changed").map_err(|e| e.to_string())?;
        Ok(())
    })();
    match fixture {
        Ok(()) => {
            // 事故防线（2026-07-21：一次 stash 打到了宿主仓库，未提交代码
            // 被回退）：先确认客户端解析的 gitRoot 就是 fixture，不是就
            // FAIL 并拒绝执行任何写操作。探针同时落日志供根因分析。
            let resolved = session_git_root(&state).await.ok().flatten();
            write(&format!("SMOKE S5 resolved gitRoot={resolved:?} fixture={cwd}"));
            let fixture_ok = resolved
                .as_deref()
                .map(|r| {
                    let norm = |x: &str| x.replace('/', "\\").trim_end_matches('\\').to_lowercase();
                    norm(r) == norm(&cwd)
                })
                .unwrap_or(false);
            if !fixture_ok {
                check!("S5-git-stash", false, format!("resolved root 不是 fixture：{resolved:?}——拒绝执行 stash"));
            } else {
            let st = git_status_ext(state.clone()).await;
            let has_change = st
                .as_ref()
                .ok()
                .and_then(|v| {
                    v.pointer("/result/unstaged")
                        .or_else(|| v.pointer("/result/data/unstaged"))
                })
                .and_then(|u| u.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(false);
            let stash = git_stash(state.clone(), None).await;
            let clean_after = git2::Repository::open(&cwd)
                .ok()
                .map(|mut r| {
                    let mut n = 0;
                    let _ = r.stash_foreach(|_, _, _| {
                        n += 1;
                        true
                    });
                    let dirty = r
                        .statuses(None)
                        .map(|s| {
                            s.iter().any(|e| {
                                let st = e.status();
                                st != git2::Status::CURRENT && st != git2::Status::WT_NEW
                            })
                        })
                        .unwrap_or(true);
                    n == 1 && !dirty
                })
                .unwrap_or(false);
            check!(
                "S5-git-stash",
                has_change && stash.is_ok() && clean_after,
                format!("change={has_change} stash={} clean={clean_after}", stash.is_ok())
            );
            }
        }
        Err(e) => check!("S5-git-stash", false, format!("fixture: {e}")),
    }

    // ── S6 会话恢复（同 id 续接，历史保留）────────────────────────
    let before_len = chat_text().lines().count();
    let resumed = start_inner(app.clone(), &state, workspace, None, Some(sid.clone())).await;
    let same_id = resumed.as_ref().map(|r| r.session_id == sid).unwrap_or(false);
    let after_len = chat_text().lines().count();
    check!(
        "S6-resume",
        same_id && after_len >= before_len,
        format!("same_id={same_id} lines {before_len}->{after_len}")
    );

    write(&format!("SMOKE DONE pass={pass} fail={fail}"));
    std::process::exit(if fail > 0 { 1 } else { 0 });
}

/// 在 sessions 目录下找包含指定会话 id 的目录（两层结构：cwd 编码/会话 id）。
fn walkdir_find(base: &std::path::Path, sid: &str) -> Option<std::path::PathBuf> {
    for cwd_dir in std::fs::read_dir(base).ok()?.flatten() {
        let cand = cwd_dir.path().join(sid);
        if cand.is_dir() {
            return Some(cand);
        }
    }
    None
}

/// A pasted image: base64 data + mime type.
#[derive(serde::Deserialize)]
pub struct PromptImage {
    pub data: String,
    pub mime: String,
}

/// Send one user prompt (optionally with pasted images for vision models);
/// resolves when the turn completes.
#[tauri::command]
pub async fn agent_prompt(
    app: AppHandle,
    state: State<'_, AgentState>,
    text: String,
    images: Option<Vec<PromptImage>>,
) -> Result<(), String> {
    let (acp_tx, session_id) = {
        let guard = state.handle.lock().await;
        let h = guard.as_ref().ok_or("会话未启动")?;
        (h.acp_tx.clone(), h.session_id.clone())
    };
    let mut blocks = vec![acp::ContentBlock::Text(acp::TextContent::new(text))];
    for img in images.unwrap_or_default() {
        blocks.push(acp::ContentBlock::Image(acp::ImageContent::new(img.data, img.mime)));
    }
    let request = acp::PromptRequest::new(session_id, blocks);
    let result: Result<acp::PromptResponse, _> = acp_send(request, &acp_tx).await;
    let payload = match &result {
        Ok(resp) => serde_json::json!({
            "ok": true,
            "stopReason": serde_json::to_value(resp.stop_reason).unwrap_or(serde_json::Value::Null),
        }),
        Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
    };
    let _ = app.emit("agent://turn-end", payload);
    result.map(|_| ()).map_err(|e| e.to_string())
}

/// Answer a pending permission request. `option_id = None` cancels/denies.
#[tauri::command]
pub async fn agent_permission_respond(
    state: State<'_, AgentState>,
    id: u64,
    option_id: Option<String>,
) -> Result<(), String> {
    let sender = state.pending_permissions.lock().await.remove(&id);
    match sender {
        Some(tx) => {
            let _ = tx.send(option_id);
            Ok(())
        }
        None => Err(format!("没有待处理的权限请求 #{id}")),
    }
}

/// Read a skill's SKILL.md for in-app editing.
///
/// Takes the ABSOLUTE path from the engine's skills/list — skills can live in
/// plugin dirs / project dirs, not just ~/.grok/skills, so deriving the path
/// from a name would silently miss those.
#[tauri::command]
pub async fn skill_read(path: String) -> Result<String, String> {
    let p = PathBuf::from(&path);
    let file = if p.is_dir() { p.join("SKILL.md") } else { p };
    std::fs::read_to_string(&file).map_err(|e| e.to_string())
}

/// Write a skill's SKILL.md content (absolute path, same rule as skill_read).
#[tauri::command]
pub async fn skill_write(path: String, content: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    let file = if p.is_dir() || !path.ends_with(".md") {
        std::fs::create_dir_all(&p).map_err(|e| e.to_string())?;
        p.join("SKILL.md")
    } else {
        p
    };
    std::fs::write(file, content).map_err(|e| e.to_string())
}




// ── Skills (~/.grok/skills/<name>/SKILL.md) ─────────────────────────

fn skills_dir() -> PathBuf {
    xai_grok_shell::util::grok_home::grok_home().join("skills")
}

/// List skills via the engine (x.ai/skills/list). Replaces the old
/// filesystem scan of ~/.grok/skills: the engine also discovers project-level
/// and plugin skills, and knows each skill's enabled state.
#[tauri::command]
pub async fn skills_list(
    state: State<'_, AgentState>,
    workspace: String,
) -> Result<serde_json::Value, String> {
    let v = ext_call(&state, "x.ai/skills/list", serde_json::json!({ "cwd": workspace })).await?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    Ok(v.get("result").cloned().unwrap_or(v))
}

/// Enable/disable a skill (x.ai/skills/toggle). Persists to [skills].disabled
/// in the engine config; returns the full refreshed list.
#[tauri::command]
pub async fn skills_toggle(
    state: State<'_, AgentState>,
    name: String,
    enabled: bool,
    workspace: String,
) -> Result<serde_json::Value, String> {
    let v = ext_call(
        &state,
        "x.ai/skills/toggle",
        serde_json::json!({ "name": name, "enabled": enabled, "cwd": workspace }),
    )
    .await?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    // toggle 只写配置。会话（含之后新建的）用的是 agent 启动时的技能基线
    // 快照——不刷新基线，停用就只是改了个没人读的配置项。实测踩过：停用
    // 后新会话的模型面向清单里技能还在。
    let _ = ext_call(&state, "x.ai/skills/refresh-baseline", serde_json::json!({})).await;
    Ok(v.get("result").cloned().unwrap_or(v))
}

/// Register an extra skills directory (x.ai/skills/add). `path` may be a dir
/// or a SKILL.md; `~` expands engine-side.
#[tauri::command]
pub async fn skills_add_path(
    state: State<'_, AgentState>,
    path: String,
    workspace: String,
) -> Result<serde_json::Value, String> {
    let v = ext_call(
        &state,
        "x.ai/skills/add",
        serde_json::json!({ "path": path, "cwd": workspace }),
    )
    .await?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    let _ = ext_call(&state, "x.ai/skills/refresh-baseline", serde_json::json!({})).await;
    Ok(v.get("result").cloned().unwrap_or(v))
}

/// Unregister a skills path (x.ai/skills/remove).
#[tauri::command]
pub async fn skills_remove_path(
    state: State<'_, AgentState>,
    path: String,
    workspace: String,
) -> Result<serde_json::Value, String> {
    let v = ext_call(
        &state,
        "x.ai/skills/remove",
        serde_json::json!({ "path": path, "cwd": workspace }),
    )
    .await?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    let _ = ext_call(&state, "x.ai/skills/refresh-baseline", serde_json::json!({})).await;
    Ok(v.get("result").cloned().unwrap_or(v))
}

/// Reset skills config to defaults (x.ai/skills/reset).
#[tauri::command]
pub async fn skills_reset(
    state: State<'_, AgentState>,
    workspace: String,
) -> Result<serde_json::Value, String> {
    let v = ext_call(&state, "x.ai/skills/reset", serde_json::json!({ "cwd": workspace })).await?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    let _ = ext_call(&state, "x.ai/skills/refresh-baseline", serde_json::json!({})).await;
    Ok(v.get("result").cloned().unwrap_or(v))
}

/// Skills config summary — paths / ignore / totals (x.ai/skills/config).
#[tauri::command]
pub async fn skills_config(
    state: State<'_, AgentState>,
    workspace: String,
) -> Result<serde_json::Value, String> {
    let v = ext_call(&state, "x.ai/skills/config", serde_json::json!({ "cwd": workspace })).await?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    Ok(v.get("result").cloned().unwrap_or(v))
}

/// Ensure ~/.grok/skills exists and open it in the OS file manager.
#[tauri::command]
pub async fn skills_open() -> Result<(), String> {
    let dir = skills_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::process::Command::new("explorer")
        .arg(&dir)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Create a starter skill: ~/.grok/skills/<name>/SKILL.md with a template.
#[tauri::command]
pub async fn skills_create(name: String, description: String) -> Result<String, String> {
    let safe: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    if safe.is_empty() {
        return Err("名称无效".into());
    }
    let dir = skills_dir().join(&safe);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let md = format!(
        "---\nname: {safe}\ndescription: {desc}\n---\n\n# {safe}\n\n{desc}\n\n## 使用说明\n\n在这里写这个 skill 的具体指令与步骤。\n",
        safe = safe,
        desc = if description.trim().is_empty() { "（填写这个 skill 的用途）" } else { description.trim() },
    );
    let path = dir.join("SKILL.md");
    std::fs::write(&path, md).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

// ── Hooks (~/.grok/hooks/wancode.json, WanCode-managed) ──────────────

#[derive(Serialize, serde::Deserialize, Clone)]
pub struct HookEntry {
    pub event: String,
    pub command: String,
}

fn hooks_path() -> PathBuf {
    xai_grok_shell::util::grok_home::grok_home()
        .join("hooks")
        .join("wancode.json")
}

/// Read the WanCode-managed hooks file as a flat {event, command} list.
#[tauri::command]
pub async fn hooks_list() -> Result<Vec<HookEntry>, String> {
    let text = std::fs::read_to_string(hooks_path()).unwrap_or_default();
    if text.trim().is_empty() {
        return Ok(vec![]);
    }
    let doc: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    if let Some(map) = doc.get("hooks").and_then(|v| v.as_object()) {
        for (event, groups) in map {
            for group in groups.as_array().into_iter().flatten() {
                for h in group.get("hooks").and_then(|v| v.as_array()).into_iter().flatten() {
                    if let Some(cmd) = h.get("command").and_then(|v| v.as_str()) {
                        out.push(HookEntry { event: event.clone(), command: cmd.to_string() });
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Replace the entire WanCode-managed hooks file from a flat list.
#[tauri::command]
pub async fn hooks_save(entries: Vec<HookEntry>) -> Result<(), String> {
    use std::collections::BTreeMap;
    let mut by_event: BTreeMap<String, Vec<serde_json::Value>> = BTreeMap::new();
    for e in entries {
        if e.event.trim().is_empty() || e.command.trim().is_empty() {
            continue;
        }
        by_event
            .entry(e.event)
            .or_default()
            .push(serde_json::json!({ "type": "command", "command": e.command }));
    }
    let hooks: serde_json::Map<String, serde_json::Value> = by_event
        .into_iter()
        .map(|(event, cmds)| (event, serde_json::json!([{ "hooks": cmds }])))
        .collect();
    let doc = serde_json::json!({ "hooks": hooks });
    let path = hooks_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap())
        .map_err(|e| format!("写入 hooks 失败: {e}"))
}


/// Call an `x.ai/*` ACP extension method against the live session and
/// return the raw JSON response.
pub(crate) async fn ext_call(
    state: &State<'_, AgentState>,
    method: &str,
    mut params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let (acp_tx, session_id) = {
        let guard = state.handle.lock().await;
        let h = guard.as_ref().ok_or("会话未启动")?;
        (h.acp_tx.clone(), h.session_id.clone())
    };
    if let Some(obj) = params.as_object_mut() {
        // 引擎里同级方法的命名并不统一：mcp/list 用 camelCase 的 sessionId，
        // 而 mcp/toggle / toggle_tool / auth_trigger 用 snake_case 的
        // session_id。两个都塞进去——没有 deny_unknown_fields，多余的键会被
        // 忽略，但少一个就是静默的 missing field 失败。
        //
        // 例外：参数结构体上带 #[serde(alias)] 的方法，两个键会映射到同一
        // 字段，serde 直接报 duplicate field。目前引擎里只有 rewind/*
        // （snake 为主名）和 debug/*（camel 为主名）用 alias——这两族只塞一个。
        let sid = serde_json::Value::String(session_id.0.to_string());
        if method.starts_with("x.ai/rewind") {
            obj.entry("session_id").or_insert(sid);
        } else if method.starts_with("x.ai/debug") {
            obj.entry("sessionId").or_insert(sid);
        } else {
            obj.entry("sessionId").or_insert(sid.clone());
            obj.entry("session_id").or_insert(sid);
        }
    }
    // #83：git/*（worktree 除外）一律显式带 gitRoot。引擎在会话目录不是
    // 仓库时会静默回退到 workspace-hub 根——嵌入式场景那是本应用自己的
    // 仓库。客户端解析不出仓库就本地拒绝，绝不触发那个回退。
    if method.starts_with("x.ai/git/") && !method.starts_with("x.ai/git/worktree") {
        if let Some(obj) = params.as_object_mut() {
            if !obj.contains_key("gitRoot") && !obj.contains_key("git_root") {
                let root = {
                    let guard = state.handle.lock().await;
                    let h = guard.as_ref().ok_or("会话未启动")?;
                    git2::Repository::discover(&h.cwd)
                        .ok()
                        .and_then(|r| r.workdir().map(|p| p.to_string_lossy().into_owned()))
                };
                let Some(root) = root else {
                    return Err("当前工作区不是 git 仓库".into());
                };
                obj.insert("gitRoot".into(), serde_json::Value::String(root));
            }
        }
    }
    let raw = serde_json::value::to_raw_value(&params).map_err(|e| e.to_string())?;
    let resp: acp::ExtResponse =
        acp_send(acp::ExtRequest::new(method.to_string(), raw.into()), &acp_tx)
            .await
            .map_err(|e| e.to_string())?;
    serde_json::from_str(resp.0.get()).map_err(|e| e.to_string())
}

/// Fire-and-forget ext *notification* (no response), e.g. the `x.ai/queue/*`
/// edit operations — the engine handles those on the notification path, not
/// as requests.
async fn ext_notify(
    state: &State<'_, AgentState>,
    method: &str,
    mut params: serde_json::Value,
) -> Result<(), String> {
    let (acp_tx, session_id) = {
        let guard = state.handle.lock().await;
        let h = guard.as_ref().ok_or("会话未启动")?;
        (h.acp_tx.clone(), h.session_id.clone())
    };
    if let Some(obj) = params.as_object_mut() {
        obj.entry("sessionId")
            .or_insert(serde_json::Value::String(session_id.0.to_string()));
        // Scopes remove/clear to our own items and records the editor.
        obj.entry("owner")
            .or_insert(serde_json::Value::String("wancode".into()));
    }
    let raw = serde_json::value::to_raw_value(&params).map_err(|e| e.to_string())?;
    let _: () = acp_send(
        acp::ExtNotification::new(method.to_string(), raw.into()),
        &acp_tx,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Mid-turn interjection (`x.ai/interject`): steer the agent WITHOUT waiting
/// for the turn to finish and without cancelling it. The session actor drains
/// it at the next safe point. Distinct from queueing (runs after the turn).
///
/// The engine broadcasts `x.ai/session/interjection` to every attached pane;
/// we mint `interjectionId` so the frontend can dedup its own optimistic echo.
#[tauri::command]
pub async fn agent_interject(
    state: State<'_, AgentState>,
    text: String,
    interjection_id: String,
) -> Result<serde_json::Value, String> {
    ext_call(
        &state,
        "x.ai/interject",
        serde_json::json!({ "text": text, "interjectionId": interjection_id }),
    )
    .await
}

/// Edit a queued prompt in place (`x.ai/queue/edit`, notification path).
#[tauri::command]
pub async fn agent_queue_edit(
    state: State<'_, AgentState>,
    id: String,
    new_text: String,
) -> Result<(), String> {
    ext_notify(
        &state,
        "x.ai/queue/edit",
        serde_json::json!({ "id": id, "newText": new_text }),
    )
    .await
}

/// Reorder the queue (`x.ai/queue/reorder`). Full ordered id list wins.
#[tauri::command]
pub async fn agent_queue_reorder(
    state: State<'_, AgentState>,
    ordered_ids: Vec<String>,
) -> Result<(), String> {
    ext_notify(
        &state,
        "x.ai/queue/reorder",
        serde_json::json!({ "orderedIds": ordered_ids }),
    )
    .await
}

/// Promote a queued prompt to a mid-turn interjection (`x.ai/queue/interject`):
/// it runs NOW instead of waiting its turn. Version-guarded like remove.
#[tauri::command]
pub async fn agent_queue_interject(
    state: State<'_, AgentState>,
    id: String,
    expected_version: u64,
) -> Result<(), String> {
    ext_notify(
        &state,
        "x.ai/queue/interject",
        serde_json::json!({ "id": id, "expectedVersion": expected_version }),
    )
    .await
}

/// Toggle plan mode (`x.ai/toggle_plan_mode`, notification path). The engine
/// flips plan⇄default and emits `current_mode_update`, which the UI already
/// follows — so this needs no response handling. Bound to Shift+Tab.
#[tauri::command]
pub async fn agent_toggle_plan_mode(state: State<'_, AgentState>) -> Result<(), String> {
    ext_notify(&state, "x.ai/toggle_plan_mode", serde_json::json!({})).await
}

/// Forget all "always allow" tool-permission grants (`x.ai/permissions/reset`).
#[tauri::command]
pub async fn permissions_reset(state: State<'_, AgentState>) -> Result<(), String> {
    ext_notify(&state, "x.ai/permissions/reset", serde_json::json!({})).await
}

/// Sync the client-side permission mode to the engine
/// (`x.ai/yolo_mode_changed`). Until now bypass/auto were client-side only —
/// the engine still raised permission requests and we auto-answered them.
/// With this the engine skips the round-trip entirely.
///
/// Key casing is the engine's, verbatim: `clientIdentifier` is camelCase,
/// `yolo_mode` / `auto_mode` / `permission_mode` are snake_case.
#[tauri::command]
pub async fn agent_sync_permission_mode(
    state: State<'_, AgentState>,
    yolo: bool,
    auto: bool,
) -> Result<(), String> {
    ext_notify(
        &state,
        "x.ai/yolo_mode_changed",
        // 不传 clientIdentifier：引擎按 origin_client.product == sender 匹配
        // 会话，而我们从未在 initialize meta 里声明过 origin client（= None），
        // 传了标识就永远匹配不上——同步变成静默 no-op（实测踩过：切了自动
        // 模式引擎照样发权限请求）。单客户端应用走 sender_id.is_none() 分支
        // 匹配全部会话即可。
        serde_json::json!({
            "yolo_mode": yolo,
            "auto_mode": auto,
            "permission_mode": if yolo { "yolo" } else if auto { "auto" } else { "default" },
        }),
    )
    .await
}

/// Drop one queued prompt. `expected_version` guards against acting on a stale
/// view (mismatch = benign no-op + the engine rebroadcasts the queue).
#[tauri::command]
pub async fn agent_queue_remove(
    state: State<'_, AgentState>,
    id: String,
    expected_version: u64,
) -> Result<(), String> {
    ext_notify(
        &state,
        "x.ai/queue/remove",
        serde_json::json!({ "id": id, "expectedVersion": expected_version }),
    )
    .await
}

/// Drop every prompt this client queued.
#[tauri::command]
pub async fn agent_queue_clear(state: State<'_, AgentState>) -> Result<(), String> {
    ext_notify(&state, "x.ai/queue/clear", serde_json::json!({})).await
}

/// Compact the conversation to reclaim context (`x.ai/compact_conversation`).
#[tauri::command]
pub async fn agent_compact(
    state: State<'_, AgentState>,
    user_context: Option<String>,
) -> Result<serde_json::Value, String> {
    ext_call(
        &state,
        "x.ai/compact_conversation",
        serde_json::json!({ "userContext": user_context }),
    )
    .await
}

/// Flatten `session_summaries/workspace_list` into a cross-workspace "recent
/// sessions" list for the home screen, newest first.
///
/// The engine groups summaries by cwd; the home screen wants the opposite view
/// — the last N sessions regardless of which project they belong to — so the
/// regrouping happens here rather than in the UI.
#[tauri::command]
pub async fn recent_sessions(
    state: State<'_, AgentState>,
    limit: Option<usize>,
) -> Result<Vec<serde_json::Value>, String> {
    let v = ext_call(
        &state,
        "x.ai/session_summaries/workspace_list",
        serde_json::json!({}),
    )
    .await?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    let map = v
        .get("result")
        .and_then(|r| r.get("all_sessions"))
        .or_else(|| v.get("all_sessions"))
        .and_then(|m| m.as_object())
        .cloned()
        .unwrap_or_default();

    let mut out: Vec<serde_json::Value> = map
        .into_iter()
        .flat_map(|(path, sessions)| {
            let path = path.clone();
            sessions
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(move |s| {
                    let get = |k: &str| s.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
                    serde_json::json!({
                        "path": path,
                        "sessionId": s.get("info").and_then(|i| i.get("id"))
                            .and_then(|x| x.as_str()).unwrap_or("").to_string(),
                        "title": get("session_summary"),
                        "updatedAt": get("updated_at"),
                        "branch": get("head_branch"),
                        "messages": s.get("num_chat_messages")
                            .and_then(|x| x.as_u64()).unwrap_or(0),
                    })
                })
                .collect::<Vec<_>>()
        })
        // 空会话（一条消息都没有）对首页没有意义
        .filter(|s| s.get("messages").and_then(|m| m.as_u64()).unwrap_or(0) > 0)
        .collect();

    out.sort_by(|a, b| {
        b.get("updatedAt")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .cmp(a.get("updatedAt").and_then(|v| v.as_str()).unwrap_or(""))
    });
    out.truncate(limit.unwrap_or(8));
    Ok(out)
}

/// Enveloped ext call: unwrap `{result, error}` — Err on engine error, else
/// the inner result. 90% of the P2 surface is exactly this shape.
pub(crate) async fn ext_ok(
    state: &State<'_, AgentState>,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let v = ext_call(state, method, params).await?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    Ok(v.get("result").cloned().unwrap_or(v))
}

// ── P2.3 MCP 配置引擎化 ────────────────────────────────────────────
// upsert/delete 的字段是 snake_case（server_name）；config 由引擎侧
// McpServerConfig flatten，直接把表单对象平铺进 params。

#[tauri::command]
pub async fn mcp_upsert(
    state: State<'_, AgentState>,
    server_name: String,
    config: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let mut params = config;
    if !params.is_object() {
        params = serde_json::json!({});
    }
    params["server_name"] = serde_json::json!(server_name);
    ext_ok(&state, "x.ai/mcp/upsert", params).await
}

#[tauri::command]
pub async fn mcp_delete(
    state: State<'_, AgentState>,
    server_name: String,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/mcp/delete", serde_json::json!({ "server_name": server_name })).await
}

#[tauri::command]
pub async fn mcp_read_resource(
    state: State<'_, AgentState>,
    server: String,
    uri: String,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/mcp/read_resource", serde_json::json!({ "server": server, "uri": uri }))
        .await
}

#[tauri::command]
pub async fn session_update_mcp_servers(
    state: State<'_, AgentState>,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/session/update_mcp_servers", serde_json::json!({})).await
}

// ── P2.4 终端补全 ───────────────────────────────────────────────────

#[tauri::command]
pub async fn terminal_list(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/terminal/list", serde_json::json!({})).await
}

#[tauri::command]
pub async fn terminal_output(
    state: State<'_, AgentState>,
    terminal_id: String,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/terminal/output", serde_json::json!({ "terminalId": terminal_id })).await
}

#[tauri::command]
pub async fn terminal_create(
    state: State<'_, AgentState>,
    command: String,
    args: Vec<String>,
) -> Result<serde_json::Value, String> {
    ext_ok(
        &state,
        "x.ai/terminal/create",
        serde_json::json!({ "command": command, "args": args }),
    )
    .await
}

#[tauri::command]
pub async fn terminal_background(
    state: State<'_, AgentState>,
    terminal_id: String,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/terminal/background", serde_json::json!({ "terminalId": terminal_id }))
        .await
}

#[tauri::command]
pub async fn terminal_release(
    state: State<'_, AgentState>,
    terminal_id: String,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/terminal/release", serde_json::json!({ "terminalId": terminal_id })).await
}

#[tauri::command]
pub async fn terminal_wait_for_exit(
    state: State<'_, AgentState>,
    terminal_id: String,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/terminal/wait_for_exit", serde_json::json!({ "terminalId": terminal_id }))
        .await
}

/// Reattach to a PTY: replays the full ring buffer as one `isReplay` output
/// notification, then returns {terminalId, rows, cols, exited, exitCode?}.
#[tauri::command]
pub async fn pty_load(
    state: State<'_, AgentState>,
    terminal_id: String,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/terminal/pty/load", serde_json::json!({ "terminalId": terminal_id })).await
}

// ── P2.6 模糊文件搜索（有状态流式协议）────────────────────────────
// open → searchId；change 只 ack；结果经 x.ai/search/fuzzy/status 通知
// 异步到达（前端监听）；close 释放。

#[tauri::command]
pub async fn fuzzy_open(
    state: State<'_, AgentState>,
    workspace: String,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/search/fuzzy/open", serde_json::json!({ "cwd": workspace })).await
}

#[tauri::command]
pub async fn fuzzy_change(
    state: State<'_, AgentState>,
    search_id: String,
    query: String,
    limit: Option<usize>,
) -> Result<serde_json::Value, String> {
    ext_ok(
        &state,
        "x.ai/search/fuzzy/change",
        serde_json::json!({ "searchId": search_id, "query": query, "limit": limit }),
    )
    .await
}

#[tauri::command]
pub async fn fuzzy_close(
    state: State<'_, AgentState>,
    search_id: String,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/search/fuzzy/close", serde_json::json!({ "searchId": search_id })).await
}

// ── P2.7 fs/* 引擎化 ────────────────────────────────────────────────

#[tauri::command]
pub async fn fs_list(
    state: State<'_, AgentState>,
    path: String,
    depth: Option<usize>,
) -> Result<serde_json::Value, String> {
    ext_ok(
        &state,
        "x.ai/fs/list",
        serde_json::json!({ "path": path, "depth": depth.unwrap_or(1) }),
    )
    .await
}

#[tauri::command]
pub async fn fs_read(
    state: State<'_, AgentState>,
    path: String,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/fs/read_file", serde_json::json!({ "path": path })).await
}

#[tauri::command]
pub async fn fs_write(
    state: State<'_, AgentState>,
    path: String,
    content: String,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/fs/write_file", serde_json::json!({ "path": path, "content": content }))
        .await
}

#[tauri::command]
pub async fn fs_exists(
    state: State<'_, AgentState>,
    path: String,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/fs/exists", serde_json::json!({ "path": path })).await
}

#[tauri::command]
pub async fn fs_delete(
    state: State<'_, AgentState>,
    path: String,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/fs/delete_file", serde_json::json!({ "path": path })).await
}

// ── P2.8 会话管理补全 ───────────────────────────────────────────────

#[tauri::command]
pub async fn session_list_engine(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/session/list", serde_json::json!({})).await
}

#[tauri::command]
pub async fn session_close(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/session/close", serde_json::json!({})).await
}

#[tauri::command]
pub async fn session_load_history(
    state: State<'_, AgentState>,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/session/load_history", serde_json::json!({})).await
}

#[tauri::command]
pub async fn session_repair(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/session/repair", serde_json::json!({})).await
}

#[tauri::command]
pub async fn session_updates_fetch(
    state: State<'_, AgentState>,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/session/updates", serde_json::json!({})).await
}

#[tauri::command]
pub async fn session_summaries_for_cwd(
    state: State<'_, AgentState>,
    workspace: String,
) -> Result<serde_json::Value, String> {
    ext_ok(
        &state,
        "x.ai/session_summaries/session_list",
        serde_json::json!({ "cwd": workspace }),
    )
    .await
}

#[tauri::command]
pub async fn workspace_list_recent(
    state: State<'_, AgentState>,
    limit: usize,
) -> Result<serde_json::Value, String> {
    ext_ok(
        &state,
        "x.ai/session_summaries/workspace_list_recent",
        serde_json::json!({ "limit": limit }),
    )
    .await
}

#[tauri::command]
pub async fn workspaces_list(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/workspaces/list", serde_json::json!({})).await
}

#[tauri::command]
pub async fn sessions_roster(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/sessions/list", serde_json::json!({})).await
}

// ── P2.9 记忆 + 杂项 ────────────────────────────────────────────────

#[tauri::command]
pub async fn memory_flush(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/memory/flush", serde_json::json!({})).await
}

#[tauri::command]
pub async fn memory_rewrite(
    state: State<'_, AgentState>,
    text: String,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/memory/rewrite", serde_json::json!({ "text": text })).await
}

#[tauri::command]
pub async fn subagent_get(
    state: State<'_, AgentState>,
    subagent_id: String,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/subagent/get", serde_json::json!({ "subagentId": subagent_id })).await
}

#[tauri::command]
pub async fn agent_recap(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/recap", serde_json::json!({ "auto": false })).await
}

#[tauri::command]
pub async fn agent_suggest(
    state: State<'_, AgentState>,
    text: String,
    cursor: usize,
    workspace: String,
) -> Result<serde_json::Value, String> {
    // suggest 是 raw 响应；ext_ok 对无信封响应会原样返回，兼容。
    ext_ok(
        &state,
        "x.ai/suggest",
        serde_json::json!({
            "text": text, "cursor": cursor, "cwd": workspace,
            "limit": 8, "generation": 0
        }),
    )
    .await
}

#[tauri::command]
pub async fn hooks_engine_list(
    state: State<'_, AgentState>,
    workspace: String,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/hooks/list", serde_json::json!({ "cwd": workspace })).await
}


// ── 交互式 PTY 终端 ────────────────────────────────────────────────
//
// 引擎侧已经把重活做完了：PTY 按 terminalId 寻址、输出按 16ms 批量推送、
// 自带 256KiB 环形缓冲，重连时整段重放。客户端只要转发字节。
//
// 三个不能想当然的地方：
//   1. `pty/input` 是 **通知** 不是请求——当请求发会 method_not_found。
//   2. 输入输出都是 **base64 的裸字节**，不是 UTF-8 文本；PTY 输出会在
//      任意字节边界切断，按文本解码必然切坏多字节字符和转义序列。
//   3. `x.ai/terminal/pty/*` 的响应是带 {result,error} 信封的。

/// Open an interactive PTY. Returns its `terminalId` — every later
/// input/resize/kill and every output notification keys off it.
#[tauri::command]
pub async fn pty_create(
    state: State<'_, AgentState>,
    rows: u16,
    cols: u16,
) -> Result<String, String> {
    let v = ext_call(
        &state,
        "x.ai/terminal/pty/create",
        serde_json::json!({ "rows": rows, "cols": cols }),
    )
    .await?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    v.get("result")
        .and_then(|r| r.get("terminalId"))
        .and_then(|s| s.as_str())
        .map(String::from)
        .ok_or_else(|| format!("pty/create 未返回 terminalId: {v}"))
}

/// Send keystrokes. `data` is base64 of the raw bytes.
///
/// Fire-and-forget by design — the engine handles this on the notification
/// path and silently drops undecodable input, so there is nothing to await.
#[tauri::command]
pub async fn pty_input(
    state: State<'_, AgentState>,
    terminal_id: String,
    data: String,
) -> Result<(), String> {
    ext_notify(
        &state,
        "x.ai/terminal/pty/input",
        serde_json::json!({ "terminalId": terminal_id, "data": data }),
    )
    .await
}

#[tauri::command]
pub async fn pty_resize(
    state: State<'_, AgentState>,
    terminal_id: String,
    rows: u16,
    cols: u16,
) -> Result<(), String> {
    ext_call(
        &state,
        "x.ai/terminal/pty/resize",
        serde_json::json!({ "terminalId": terminal_id, "rows": rows, "cols": cols }),
    )
    .await
    .map(|_| ())
}

/// Kill a terminal (PTY or piped — the engine tries the PTY registry first).
#[tauri::command]
pub async fn pty_kill(state: State<'_, AgentState>, terminal_id: String) -> Result<(), String> {
    ext_call(
        &state,
        "x.ai/terminal/kill",
        serde_json::json!({ "terminalId": terminal_id }),
    )
    .await
    .map(|_| ())
}

/// Cancel a scheduled (cron / `/loop`) task.
///
/// `x.ai/scheduler/delete` is the **only** scheduler ext method the engine
/// exposes — there is no create or list. Tasks are created by the model
/// invoking the `scheduler_create` tool, and the client rebuilds its view from
/// the `x.ai/scheduled_task_*` notification stream. A successful delete makes
/// the engine emit `scheduled_task_deleted`, so the UI drops the row from that
/// notification rather than optimistically removing it here.
#[tauri::command]
pub async fn scheduler_delete(
    state: State<'_, AgentState>,
    task_id: String,
) -> Result<serde_json::Value, String> {
    ext_call(
        &state,
        "x.ai/scheduler/delete",
        serde_json::json!({ "taskId": task_id }),
    )
    .await
}

/// Fork the current session (`x.ai/session/fork`) and return the new id.
///
/// `target_prompt_index` truncates the copy at that prompt, i.e. branch from an
/// earlier point in the conversation; omit it to copy everything.
///
/// Two things about this method differ from the rest of the ext surface:
///   1. The response is **raw** — no `{result, error}` envelope — so we read
///      `newSessionId` straight off the body.
///   2. Fork only writes files to disk; it does **not** start the session. The
///      caller has to `start(resume = newSessionId)` afterwards.
#[tauri::command]
pub async fn session_fork(
    state: State<'_, AgentState>,
    workspace: String,
    target_prompt_index: Option<usize>,
) -> Result<String, String> {
    let source = {
        let guard = state.handle.lock().await;
        guard.as_ref().ok_or("会话未启动")?.session_id.0.to_string()
    };
    let mut params = serde_json::json!({
        "sourceSessionId": source,
        "sourceCwd": workspace,
        "newCwd": workspace,
    });
    if let Some(i) = target_prompt_index {
        params["targetPromptIndex"] = serde_json::json!(i);
    }
    let v = ext_call(&state, "x.ai/session/fork", params).await?;
    v.get("newSessionId")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("fork 未返回 newSessionId: {v}"))
}

/// Grep the workspace (`x.ai/search/content`). Respects .gitignore.
/// 引擎侧用 ripgrep 语义，比我们自己遍历文件靠谱得多。
#[tauri::command]
pub async fn search_content(
    state: State<'_, AgentState>,
    pattern: String,
    is_regex: Option<bool>,
    case_insensitive: Option<bool>,
) -> Result<serde_json::Value, String> {
    ext_call(
        &state,
        "x.ai/search/content",
        serde_json::json!({
            "pattern": pattern,
            "isRegex": is_regex.unwrap_or(false),
            "caseInsensitive": case_insensitive.unwrap_or(true),
            "respectGitignore": true,
            "maxFiles": 200,
            "maxMatches": 500,
        }),
    )
    .await
}

/// Workspaces that have session history, newest-active first.
/// 引擎按 cwd 分组返回全部会话摘要；我们只需要目录清单和各自的会话数。
#[tauri::command]
pub async fn workspace_list(state: State<'_, AgentState>) -> Result<Vec<serde_json::Value>, String> {
    let v = ext_call(
        &state,
        "x.ai/session_summaries/workspace_list",
        serde_json::json!({}),
    )
    .await?;
    let map = v
        .get("result")
        .and_then(|r| r.get("all_sessions"))
        .or_else(|| v.get("all_sessions"))
        .and_then(|m| m.as_object())
        .cloned()
        .unwrap_or_default();

    let mut out: Vec<serde_json::Value> = map
        .into_iter()
        .map(|(path, sessions)| {
            let arr = sessions.as_array().cloned().unwrap_or_default();
            // 取该工作区最近一次活动时间用于排序
            let latest = arr
                .iter()
                .filter_map(|s| {
                    s.get("info")
                        .and_then(|i| i.get("updated_at"))
                        .or_else(|| s.get("updated_at"))
                        .and_then(|u| u.as_str())
                        .map(String::from)
                })
                .max()
                .unwrap_or_default();
            serde_json::json!({
                "path": path,
                "sessions": arr.len(),
                "updatedAt": latest,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.get("updatedAt")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .cmp(a.get("updatedAt").and_then(|v| v.as_str()).unwrap_or(""))
    });
    Ok(out)
}

// ── MCP 实时管理 ────────────────────────────────────────────────
//
// 以前只能改 config.toml 再重开会话。引擎支持按服务器/按工具启停、
// 查授权状态、触发 OAuth，全部即时生效。

/// 服务器与工具清单（含启用状态）。`cache=false` 绕过缓存，
/// 用于 OAuth 授权或断开之后强制刷新。
#[tauri::command]
pub async fn mcp_live_list(
    state: State<'_, AgentState>,
    fresh: Option<bool>,
) -> Result<serde_json::Value, String> {
    ext_call(
        &state,
        "x.ai/mcp/list",
        serde_json::json!({ "cache": !fresh.unwrap_or(false) }),
    )
    .await
}

#[tauri::command]
pub async fn mcp_toggle(
    state: State<'_, AgentState>,
    server_name: String,
    enabled: bool,
) -> Result<serde_json::Value, String> {
    ext_call(
        &state,
        "x.ai/mcp/toggle",
        serde_json::json!({ "server_name": server_name, "enabled": enabled }),
    )
    .await
}

#[tauri::command]
pub async fn mcp_toggle_tool(
    state: State<'_, AgentState>,
    server_name: String,
    tool_name: String,
    enabled: bool,
) -> Result<serde_json::Value, String> {
    ext_call(
        &state,
        "x.ai/mcp/toggle_tool",
        serde_json::json!({
            "server_name": server_name,
            "tool_name": tool_name,
            "enabled": enabled
        }),
    )
    .await
}

#[tauri::command]
pub async fn mcp_auth_status(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    ext_call(&state, "x.ai/mcp/auth_status", serde_json::json!({})).await
}

#[tauri::command]
pub async fn mcp_auth_trigger(
    state: State<'_, AgentState>,
    server_name: String,
) -> Result<serde_json::Value, String> {
    ext_call(
        &state,
        "x.ai/mcp/auth_trigger",
        serde_json::json!({ "server_name": server_name }),
    )
    .await
}

// ── 后台任务 / 子 Agent ─────────────────────────────────────────
//
// 引擎一直在发 `x.ai/task_backgrounded` / `x.ai/task_completed` 通知，
// WanCode 之前直接丢弃，用户无从知道后台还有东西在跑。

/// Background shell tasks for this session (TaskSnapshot 用 snake_case)。
#[tauri::command]
pub async fn tasks_list(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    ext_call(&state, "x.ai/task/list", serde_json::json!({})).await
}

#[tauri::command]
pub async fn task_kill(
    state: State<'_, AgentState>,
    task_id: String,
) -> Result<serde_json::Value, String> {
    ext_call(&state, "x.ai/task/kill", serde_json::json!({ "taskId": task_id })).await
}

/// Running subagents (DTO 用 camelCase)。
#[tauri::command]
pub async fn subagents_list(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    ext_call(&state, "x.ai/subagent/list_running", serde_json::json!({})).await
}

#[tauri::command]
pub async fn subagent_cancel(
    state: State<'_, AgentState>,
    subagent_id: String,
) -> Result<serde_json::Value, String> {
    ext_call(
        &state,
        "x.ai/subagent/cancel",
        serde_json::json!({ "subagentId": subagent_id }),
    )
    .await
}

// ── Git（走引擎的 workspace ops，不再自己 shell 调 git）──────────
//
// 引擎已经处理了 gitRoot 解析、worktree、子模块、CREATE_NO_WINDOW 等；
// 我们只转发。`ext_call` 会自动带上 sessionId，引擎据此定位仓库。

/// Full status: branch / ahead / behind / staged / unstaged。

/// Answer a pending folder-trust prompt. Anything but an explicit `true`
/// leaves repo-local MCP/hooks/LSP gated.
#[tauri::command]
pub async fn agent_trust_respond(
    state: State<'_, AgentState>,
    id: u64,
    trust: bool,
) -> Result<(), String> {
    let sender = state.pending_trust.lock().await.remove(&id);
    sender
        .ok_or("该信任请求已失效")?
        .send(trust)
        .map_err(|_| "回传信任决定失败".to_string())
}

/// Answer a pending `x.ai/ask_user_question`. `answers` maps each question's
/// text to the chosen option labels; `None` = the user dismissed it.
#[tauri::command]
pub async fn agent_question_respond(
    state: State<'_, AgentState>,
    id: u64,
    answers: Option<HashMap<String, Vec<String>>>,
) -> Result<(), String> {
    let sender = state.pending_questions.lock().await.remove(&id);
    sender
        .ok_or("该提问已失效")?
        .send(answers)
        .map_err(|_| "回传答案失败".to_string())
}

/// Slash commands the engine actually knows about (builtins + skills +
/// plugin-provided), rather than a list hardcoded in the UI.
#[tauri::command]
pub async fn agent_commands_list(
    state: State<'_, AgentState>,
    workspace: String,
) -> Result<serde_json::Value, String> {
    ext_call(&state, "x.ai/commands/list", serde_json::json!({ "cwd": workspace })).await
}

/// Previously sent prompts for this workspace, most recent first (↑ recall).
#[tauri::command]
pub async fn agent_prompt_history(
    state: State<'_, AgentState>,
    workspace: String,
) -> Result<Vec<String>, String> {
    let v = ext_call(&state, "x.ai/prompt_history", serde_json::json!({ "cwd": workspace })).await?;
    Ok(v.get("prompts")
        .and_then(|p| p.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default())
}

/// Context/token usage snapshot (`x.ai/session/info`).
#[tauri::command]
pub async fn agent_session_info(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    ext_call(&state, "x.ai/session/info", serde_json::json!({})).await
}

/// v0.15 Review：只读子会话审查未提交改动，返回结构化 findings。
///
/// 引擎的 `x.ai/review` 是假能力（评论体硬编码 null 的只写遥测，见 roadmap
/// 附 B 审计），所以走自建路线：临时新会话 + plan（只读）模式 + 结构化
/// prompt，结果从磁盘会话历史读取，用后即删。后台会话经 background_sessions
/// 屏蔽：不发 agent://update、权限请求自动取消。
#[tauri::command]
pub async fn review_run(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    let (acp_tx, cwd) = {
        let guard = state.handle.lock().await;
        let h = guard.as_ref().ok_or("会话未启动")?;
        (h.acp_tx.clone(), h.cwd.clone())
    };

    // 1. 收集未提交改动的 unified diff（总量截断，防撑爆上下文）
    let diffs = ext_call(
        &state,
        "x.ai/git/diffs",
        serde_json::json!({ "includePatch": true }),
    )
    .await?;
    let env = diffs.get("result").unwrap_or(&diffs);
    let files = env
        .get("data")
        .unwrap_or(env)
        .get("files")
        .and_then(|f| f.as_array())
        .cloned()
        .unwrap_or_default();
    const PATCH_BUDGET: usize = 60_000;
    let mut patches = String::new();
    let mut skipped: Vec<String> = Vec::new();
    for f in &files {
        let path = f.get("path").and_then(|p| p.as_str()).unwrap_or("?");
        match f.get("patch").and_then(|p| p.as_str()) {
            Some(p) if patches.len() + p.len() <= PATCH_BUDGET => {
                patches.push_str(p);
                patches.push('\n');
            }
            _ => skipped.push(path.to_string()),
        }
    }
    if patches.is_empty() {
        return Err("工作区没有可审查的未提交改动".into());
    }

    // 2. 临时会话。客户端不传 mcpServers（引擎只加载我们传入的列表）。
    // 刻意不用 plan 模式：plan 收尾会触发 exit_plan_mode 审批握手，模型
    // 还倾向把结论塞进"计划"而不是普通回复（实测翻过车）。只读靠
    // background_sessions 的权限自动取消兜底——写类工具批不下来。
    let resp: acp::NewSessionResponse =
        acp_send(acp::NewSessionRequest::new(cwd.clone()), &acp_tx)
            .await
            .map_err(|e| format!("创建审查会话失败: {e}"))?;
    let rid = resp.session_id.clone();
    state
        .background_sessions
        .lock()
        .await
        .insert(rid.0.to_string());

    // 3. 结构化审查 prompt
    let prompt = format!(
        "你是严格的代码审查员。审查下面的未提交改动（unified diff）。\
         你只允许读文件辅助理解，绝不修改任何东西。\n\
         只输出一个 JSON 数组，不要任何其它文字或代码围栏。每个元素：\n\
         {{\"file\": \"路径\", \"line\": 行号或null, \"severity\": \"error|warn|info\", \
         \"comment\": \"中文评论，具体指出问题与建议\"}}\n\
         没有问题就输出 []。最多 12 条，按严重度排序。\n\n<diff>\n{patches}\n</diff>"
    );
    let send_fut = async {
        let r: Result<acp::PromptResponse, _> = acp_send(
            acp::PromptRequest::new(
                rid.clone(),
                vec![acp::ContentBlock::Text(acp::TextContent::new(prompt))],
            ),
            &acp_tx,
        )
        .await;
        r
    };
    let turn = tokio::time::timeout(std::time::Duration::from_secs(300), send_fut).await;

    // 4. 从磁盘读最后一条 assistant 消息（ACP PromptResponse 只有 stop_reason，
    //    没有文本，磁盘是唯一取证路径）。turn 结束后引擎的最后一笔写可能
    //    尚未落盘——空结果重试 3 次，每次 500ms。同步 IO 丢 spawn_blocking，
    //    不占 tokio worker（自审 finding L3296/L3300）。
    let sessions_base = xai_grok_shell::util::grok_home::grok_home().join("sessions");
    let rid_str = rid.0.to_string();
    let mut text = String::new();
    for attempt in 0..3u8 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        let base = sessions_base.clone();
        let sid = rid_str.clone();
        let got = tokio::task::spawn_blocking(move || {
            walkdir_find(&base, &sid)
                .map(|d| d.join("chat_history.jsonl"))
                .and_then(|f| std::fs::read_to_string(f).ok())
                .and_then(|s| {
                    s.lines()
                        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                        .filter(|v| v.get("type").and_then(|t| t.as_str()) == Some("assistant"))
                        .filter_map(|v| {
                            v.get("content").and_then(|c| c.as_str()).map(String::from)
                        })
                        .next_back()
                })
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default();
        if !got.is_empty() {
            text = got;
            break;
        }
    }

    // 5. 清理：先删会话，删完（或删失败）再移出屏蔽集——顺序反过来会有
    //    竞态窗口：会话还在引擎里活着却已不被屏蔽，通知会泄进主聊天流。
    let _ = ext_call(
        &state,
        "x.ai/session/delete",
        serde_json::json!({ "sessionId": rid.0.to_string(), "cwd": cwd.to_string_lossy() }),
    )
    .await;
    state
        .background_sessions
        .lock()
        .await
        .remove(rid.0.as_ref());

    // 超时与引擎错误分开报——内层 Err 是引擎拒绝/失败，吞掉它排查会很痛苦
    match turn {
        Err(_) => return Err("审查超时（300 秒）".into()),
        Ok(Err(e)) => return Err(format!("审查会话执行失败: {e}")),
        Ok(Ok(_)) => {}
    }
    if text.is_empty() {
        return Err("审查会话没有产出内容".into());
    }

    // 6. 宽容解析：截取首个 [ 到末个 ]，模型加围栏也能活
    let json_str = match (text.find('['), text.rfind(']')) {
        (Some(a), Some(b)) if b > a => &text[a..=b],
        _ => "",
    };
    let findings: serde_json::Value =
        serde_json::from_str(json_str).unwrap_or(serde_json::Value::Null);
    Ok(serde_json::json!({
        "findings": findings,          // null = 解析失败，前端退回显示 raw
        "raw": text,
        "reviewedFiles": files.len() - skipped.len(),
        "skippedFiles": skipped,
    }))
}


/// List engine rewind points (`x.ai/rewind/points`).
#[tauri::command]
pub async fn agent_rewind_points(
    state: State<'_, AgentState>,
) -> Result<serde_json::Value, String> {
    ext_call(&state, "x.ai/rewind/points", serde_json::json!({})).await
}

/// Time-travel to a prompt index (`x.ai/rewind/execute`).
/// `mode`: "all" | "conversation_only" | "files_only".
#[tauri::command]
pub async fn agent_rewind(
    state: State<'_, AgentState>,
    target_prompt_index: usize,
    mode: String,
    force: bool,
) -> Result<serde_json::Value, String> {
    ext_call(
        &state,
        "x.ai/rewind/execute",
        serde_json::json!({
            "targetPromptIndex": target_prompt_index,
            "mode": mode,
            "force": force,
        }),
    )
    .await
}

/// List workspace files (relative paths) for @-mention autocomplete.
/// Skips common heavy/ignored dirs; capped to keep it snappy.
#[tauri::command]
pub async fn list_workspace_files(workspace: String) -> Result<Vec<String>, String> {
    const SKIP: &[&str] = &[
        "node_modules", ".git", "target", "dist", "build", ".next",
        ".venv", "venv", "__pycache__", ".idea", ".vscode", "vendor",
    ];
    const MAX: usize = 4000;
    let root = PathBuf::from(&workspace);
    if !root.is_dir() {
        return Err("工作区不存在".into());
    }
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        if out.len() >= MAX {
            break;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                if SKIP.contains(&name.as_str()) || name.starts_with('.') {
                    continue;
                }
                stack.push(path);
            } else if let Ok(rel) = path.strip_prefix(&root) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
                if out.len() >= MAX {
                    break;
                }
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Full-text search over stored sessions (`x.ai/session/search`).
#[tauri::command]
pub async fn agent_session_search(
    state: State<'_, AgentState>,
    query: String,
    workspace: String,
) -> Result<serde_json::Value, String> {
    ext_call(
        &state,
        "x.ai/session/search",
        serde_json::json!({
            "query": query,
            "cwd": workspace,
            "limit": 30,
            "includeContent": true,
        }),
    )
    .await
}

/// Rename any stored session (`x.ai/session/rename`). Needs a live engine.
#[tauri::command]
pub async fn agent_session_rename(
    state: State<'_, AgentState>,
    session_id: String,
    title: String,
    workspace: String,
) -> Result<serde_json::Value, String> {
    ext_call(
        &state,
        "x.ai/session/rename",
        serde_json::json!({ "sessionId": session_id, "title": title, "cwd": workspace }),
    )
    .await
}

/// Delete any stored session (`x.ai/session/delete`). Needs a live engine.
#[tauri::command]
pub async fn agent_session_delete(
    state: State<'_, AgentState>,
    session_id: String,
    workspace: String,
) -> Result<serde_json::Value, String> {
    ext_call(
        &state,
        "x.ai/session/delete",
        serde_json::json!({ "sessionId": session_id, "cwd": workspace }),
    )
    .await
}

/// Switch the session mode ("plan" = read-only planning, "default" = agent).
#[tauri::command]
pub async fn agent_set_mode(state: State<'_, AgentState>, mode: String) -> Result<(), String> {
    let (acp_tx, session_id) = {
        let guard = state.handle.lock().await;
        let h = guard.as_ref().ok_or("会话未启动")?;
        (h.acp_tx.clone(), h.session_id.clone())
    };
    let _: acp::SetSessionModeResponse = acp_send(
        acp::SetSessionModeRequest::new(session_id, acp::SessionModeId::new(mode)),
        &acp_tx,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Switch the active model live, without restarting the session or losing
/// context (ACP `session/setModel`). Mirrors Claude Code's `/model`.
#[tauri::command]
pub async fn agent_set_model(state: State<'_, AgentState>, model: String) -> Result<(), String> {
    let (acp_tx, session_id) = {
        let guard = state.handle.lock().await;
        let h = guard.as_ref().ok_or("会话未启动")?;
        (h.acp_tx.clone(), h.session_id.clone())
    };
    let _: acp::SetSessionModelResponse = acp_send(
        acp::SetSessionModelRequest::new(
            session_id,
            acp::ModelId::new(std::sync::Arc::from(model.as_str())),
        ),
        &acp_tx,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Best-effort default working directory when the user hasn't picked one yet,
/// so the composer is usable immediately (Claude Code / Codex launch in cwd).
#[tauri::command]
pub fn default_workspace() -> String {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string())
}

/// Interrupt the current turn.
#[tauri::command]
pub async fn agent_cancel(state: State<'_, AgentState>) -> Result<(), String> {
    let (acp_tx, session_id) = {
        let guard = state.handle.lock().await;
        let h = guard.as_ref().ok_or("会话未启动")?;
        (h.acp_tx.clone(), h.session_id.clone())
    };
    acp_send(acp::CancelNotification::new(session_id), &acp_tx)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}
