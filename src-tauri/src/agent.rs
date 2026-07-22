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
use crate::provider_ops::{inject_managed_keys};
use crate::config_core::{validate_startup_models, StartupModels};
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
    pub(crate) background_sessions: Mutex<std::collections::HashSet<String>>,
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

pub(crate) async fn start_inner(
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
pub(crate) async fn ext_notify(
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
