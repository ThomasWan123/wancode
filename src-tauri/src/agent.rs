//! Embedded grok-build agent session for the WanCode GUI.
//!
//! Mirrors the lifecycle used by `xai-grok-pager`'s headless mode
//! (init → authenticate → new session → prompt), but pumps every ACP
//! notification to the frontend as Tauri events instead of stdout:
//!
//! - `agent://update`      — session updates (message/thought/tool chunks)
//! - `agent://permission`  — tool-call approval requests (answered via
//!                           the `agent_permission_respond` command)
//! - `agent://turn-end`    — a prompt turn finished (with stop reason or error)

use std::collections::HashMap;
use std::path::PathBuf;
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

pub struct AgentHandle {
    acp_tx: AcpAgentTx,
    session_id: acp::SessionId,
    cancel: CancellationToken,
    pub model_ids: Vec<String>,
    /// 会话工作区。git 命令用它本地解析 gitRoot（见 session_git_root）。
    pub cwd: PathBuf,
}

#[derive(Default)]
pub struct AgentState {
    handle: Mutex<Option<AgentHandle>>,
    pending_permissions: Mutex<HashMap<u64, oneshot::Sender<Option<String>>>>,
    next_permission_id: AtomicU64,
    /// Pending `x.ai/exit_plan_mode` approvals → (outcome, feedback).
    pending_plans: Mutex<HashMap<u64, oneshot::Sender<(String, Option<String>)>>>,
    /// Pending `x.ai/ask_user_question` requests: answers keyed by question text.
    pending_questions:
        Mutex<HashMap<u64, oneshot::Sender<Option<HashMap<String, Vec<String>>>>>>,
    /// Pending `x.ai/folder_trust/request` prompts → true = trust.
    pending_trust: Mutex<HashMap<u64, oneshot::Sender<bool>>>,
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
        model_ids: model_ids.clone(),
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

#[derive(Serialize, Clone)]
pub struct McpServerEntry {
    pub name: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub enabled: bool,
}

fn user_config_path() -> PathBuf {
    xai_grok_shell::util::grok_home::grok_home().join("config.toml")
}

// ── Model / API providers (config.toml [model.*] + keyring) ─────────

const KEYRING_SERVICE: &str = "wancode-models";

#[derive(Serialize, Clone)]
pub struct ModelEntry {
    pub key: String,
    pub name: String,
    pub model: String,
    pub base_url: String,
    pub env_key: Option<String>,
    pub has_key: bool,
    /// True if this model's key lives in the WanCode keyring (editable here).
    pub managed: bool,
}

fn wancode_env_key(key: &str) -> String {
    let up: String = key
        .chars()
        .map(|c| if c.is_alphanumeric() { c.to_ascii_uppercase() } else { '_' })
        .collect();
    format!("WANCODE_KEY_{up}")
}

/// List model presets from config.toml.
#[tauri::command]
pub async fn model_list() -> Result<Vec<ModelEntry>, String> {
    let path = user_config_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("配置解析失败: {e}"))?;
    let mut out = Vec::new();
    if let Some(models) = doc.get("model").and_then(|v| v.as_table()) {
        for (key, item) in models.iter() {
            let t = item.as_table_like();
            let get = |k: &str| {
                t.and_then(|t| t.get(k))
                    .and_then(|v| v.as_str())
                    .map(String::from)
            };
            let env_key = get("env_key");
            let managed = env_key.as_deref() == Some(wancode_env_key(key).as_str());
            let has_key = if managed {
                keyring::Entry::new(KEYRING_SERVICE, key)
                    .ok()
                    .and_then(|e| e.get_password().ok())
                    .is_some()
            } else {
                env_key
                    .as_deref()
                    .map(|ek| std::env::var(ek).is_ok())
                    .unwrap_or(false)
                    || get("api_key").is_some()
            };
            out.push(ModelEntry {
                name: get("name").unwrap_or_else(|| key.to_string()),
                model: get("model").unwrap_or_else(|| key.to_string()),
                base_url: get("base_url").unwrap_or_default(),
                env_key,
                has_key,
                managed,
                key: key.to_string(),
            });
        }
    }
    Ok(out)
}

/// Add/update a model preset; stores the API key in the system keyring.
#[tauri::command]
pub async fn model_upsert(
    key: String,
    name: String,
    model: String,
    base_url: String,
    api_key: Option<String>,
) -> Result<(), String> {
    let key = key.trim().to_string();
    if key.is_empty() || model.trim().is_empty() || base_url.trim().is_empty() {
        return Err("名称、模型 ID、base_url 都不能为空".into());
    }
    let env_key = wancode_env_key(&key);
    if let Some(k) = api_key.as_ref().filter(|k| !k.trim().is_empty()) {
        keyring::Entry::new(KEYRING_SERVICE, &key)
            .and_then(|e| e.set_password(k.trim()))
            .map_err(|e| format!("保存密钥到钥匙串失败: {e}"))?;
    }
    let path = user_config_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("配置解析失败: {e}"))?;
    let models = doc["model"]
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .ok_or("model 段类型异常")?;
    let mut entry = toml_edit::Table::new();
    entry["model"] = toml_edit::value(model.trim());
    entry["name"] = toml_edit::value(name.trim());
    entry["base_url"] = toml_edit::value(base_url.trim());
    entry["env_key"] = toml_edit::value(&env_key);
    entry["api_backend"] = toml_edit::value("chat_completions");
    entry["context_window"] = toml_edit::value(128000i64);
    models.insert(&key, toml_edit::Item::Table(entry));
    std::fs::write(&path, doc.to_string()).map_err(|e| format!("写入配置失败: {e}"))
}

/// One-click provider setup for novice users: pick a preset, paste ONE key.
///
/// Writes every model of the preset (shared key in the keyring under each
/// model key), tests the first model, and for 智谱 presets seeds the default
/// web-search MCP servers (see seed_default_mcp).
///
/// Preset ids are stable API: "glm-coding" (Coding Plan 专属端点)、"glm-open"
/// (开放平台)、"deepseek".
#[tauri::command]
pub async fn provider_quick_setup(
    preset: String,
    api_key: String,
) -> Result<serde_json::Value, String> {
    let api_key = api_key.trim().to_string();
    if api_key.is_empty() {
        return Err("请输入 API Key".into());
    }
    // (key, 显示名, 模型ID)
    let (base_url, models): (&str, Vec<(&str, &str, &str)>) = match preset.as_str() {
        // Coding Plan 是包月订阅的专属端点——按量计费的开放平台 Key 在这里
        // 会 401，反之亦然。这正是小白最容易配错的地方，所以分成两张卡。
        "glm-coding" => (
            "https://open.bigmodel.cn/api/coding/paas/v4",
            vec![("glm-coding", "GLM Coding Plan", "glm-5.2")],
        ),
        "glm-open" => (
            "https://open.bigmodel.cn/api/paas/v4",
            vec![
                ("glm", "智谱 GLM-5.2", "glm-5.2"),
                ("glm-air", "智谱 GLM-5-Air", "glm-5-air"),
            ],
        ),
        "deepseek" => (
            "https://api.deepseek.com",
            vec![
                ("deepseek", "DeepSeek Chat", "deepseek-chat"),
                ("deepseek-r", "DeepSeek Reasoner", "deepseek-reasoner"),
            ],
        ),
        other => return Err(format!("未知预设: {other}")),
    };

    // 先测连接（用第一个模型），失败就不落任何配置——半配置状态最坑小白。
    let first_model = models[0].2;
    let test = model_test(
        base_url.to_string(),
        first_model.to_string(),
        Some(api_key.clone()),
        None,
    )
    .await;
    if let Err(e) = test {
        return Err(format!("连接测试未通过，未保存任何配置。{e}"));
    }

    // ── 配置事务（v0.12.2）─────────────────────────────────────────
    // 顺序：内存组装完整 TOML（模型 + MCP 播种同一事务）→ 临时文件 →
    // 原子替换 → 钥匙串。钥匙串任一项失败 → 回滚本次新写入的钥匙串项 +
    // 原子写回原配置文本。任何路径下都不存在"半配置"。
    let path = user_config_path();
    let original = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut =
        original.parse().map_err(|e| format!("配置解析失败（原文件未动）: {e}"))?;
    apply_provider_preset(&mut doc, &models, base_url);
    let mut seeded = false;
    if preset.starts_with("glm") {
        seeded = seed_default_mcp_into(&mut doc);
    }
    write_config_atomic(&path, &doc.to_string())?;

    let mut written_keys: Vec<&str> = Vec::new();
    for (key, _, _) in &models {
        match keyring::Entry::new(KEYRING_SERVICE, key).and_then(|e| e.set_password(&api_key)) {
            Ok(()) => written_keys.push(key),
            Err(e) => {
                // 回滚：删掉本次写入的钥匙串项，恢复原配置
                for k in &written_keys {
                    let _ = keyring::Entry::new(KEYRING_SERVICE, k)
                        .and_then(|en| en.delete_credential());
                }
                let _ = write_config_atomic(&path, &original);
                return Err(format!("保存密钥失败，已回滚全部改动: {e}"));
            }
        }
    }

    if preset.starts_with("glm") {
        // 让 ${ZHIPU_API_KEY} 即刻可解析（无需重启）。
        // Safety: 单线程配置路径，会话尚未启动或与其无关。
        unsafe { std::env::set_var("ZHIPU_API_KEY", &api_key) };
    }

    Ok(serde_json::json!({
        "models": models.iter().map(|(k, n, m)| serde_json::json!({
            "key": k, "name": n, "model": m
        })).collect::<Vec<_>>(),
        "testReply": test.ok(),
        "mcpSeeded": seeded,
    }))
}

/// Atomically replace a config file: write to a sibling temp file, then
/// rename over the target. On Windows `std::fs::rename` maps to
/// MoveFileExW(REPLACE_EXISTING) — the reader never sees a half-written file.
fn write_config_atomic(path: &std::path::Path, content: &str) -> Result<(), String> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, content).map_err(|e| format!("写入临时配置失败: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("原子替换配置失败: {e}")
    })
}

/// Apply a provider preset's model entries onto an in-memory config doc.
/// 纯函数：不做 IO，事务由调用方统一提交（v0.12.2 配置事务化）。
fn apply_provider_preset(
    doc: &mut toml_edit::DocumentMut,
    models: &[(&str, &str, &str)],
    base_url: &str,
) {
    let tbl = doc["model"]
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .expect("model 段");
    for (key, name, model) in models {
        let mut entry = toml_edit::Table::new();
        entry["model"] = toml_edit::value(*model);
        entry["name"] = toml_edit::value(*name);
        entry["base_url"] = toml_edit::value(base_url);
        entry["env_key"] = toml_edit::value(wancode_env_key(key));
        entry["api_backend"] = toml_edit::value("chat_completions");
        entry["context_window"] = toml_edit::value(128000i64);
        tbl.insert(key, toml_edit::Item::Table(entry));
    }
}

/// In-memory MCP seeding — same rules as before (marker once / never
/// overwrite), but operating on the doc so it commits in the SAME atomic
/// write as the models. Returns whether anything was seeded.
fn seed_default_mcp_into(doc: &mut toml_edit::DocumentMut) -> bool {
    let already = doc
        .get("ui")
        .and_then(|u| u.get("wancode_mcp_seeded"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if already {
        return false;
    }
    let servers = doc["mcp_servers"]
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .expect("mcp_servers 段");
    for (name, url) in [
        ("web-search", "https://open.bigmodel.cn/api/mcp/web_search/mcp"),
        ("web-reader", "https://open.bigmodel.cn/api/mcp/web_reader/mcp"),
    ] {
        if servers.contains_key(name) {
            continue;
        }
        let mut entry = toml_edit::Table::new();
        entry["url"] = toml_edit::value(url);
        let mut headers = toml_edit::Table::new();
        headers["Authorization"] = toml_edit::value("Bearer ${ZHIPU_API_KEY}");
        entry["headers"] = toml_edit::Item::Table(headers);
        servers.insert(name, toml_edit::Item::Table(entry));
    }
    let ui = doc["ui"]
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .expect("ui 段");
    ui["wancode_mcp_seeded"] = toml_edit::value(true);
    true
}


/// Remove a model preset and its keyring entry.
#[tauri::command]
pub async fn model_remove(key: String) -> Result<(), String> {
    let _ = keyring::Entry::new(KEYRING_SERVICE, &key).and_then(|e| e.delete_credential());
    let path = user_config_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("配置解析失败: {e}"))?;
    let mut removed_model_id: Option<String> = None;
    let mut survivor_model_id: Option<String> = None;
    if let Some(models) = doc.get_mut("model").and_then(|v| v.as_table_mut()) {
        removed_model_id = models
            .get(&key)
            .and_then(|e| e.get("model"))
            .and_then(|v| v.as_str())
            .map(String::from);
        models.remove(&key);
        survivor_model_id = models
            .iter()
            .next()
            .and_then(|(_, e)| e.get("model"))
            .and_then(|v| v.as_str())
            .map(String::from);
    }
    // [models].default 指向被删模型时必须跟着清理：悬空 default 会让引擎在
    // 下次启动时直接 panic（capacity overflow，实测）。有幸存者就指过去，
    // 一个不剩就删掉整个 [models] 段（零模型时前端不会再启动引擎）。
    if let Some(removed) = removed_model_id {
        let dangling = doc
            .get("models")
            .and_then(|m| m.get("default"))
            .and_then(|v| v.as_str())
            .is_some_and(|d| d == removed);
        if dangling {
            match survivor_model_id {
                Some(next) => {
                    if let Some(models_tbl) = doc.get_mut("models").and_then(|v| v.as_table_mut()) {
                        models_tbl["default"] = toml_edit::value(next);
                    }
                }
                None => {
                    doc.remove("models");
                }
            }
        }
    }
    std::fs::write(&path, doc.to_string()).map_err(|e| format!("写入配置失败: {e}"))
}

/// Test a provider: minimal chat completion against base_url. Returns the
/// model's reply text on success, or an error string.
#[tauri::command]
pub async fn model_test(
    base_url: String,
    model: String,
    api_key: Option<String>,
    key: Option<String>,
) -> Result<String, String> {
    // Resolve the key: explicit api_key, else keyring by preset key.
    let token = match api_key.filter(|k| !k.trim().is_empty()) {
        Some(k) => k,
        None => key
            .and_then(|k| keyring::Entry::new(KEYRING_SERVICE, &k).ok())
            .and_then(|e| e.get_password().ok())
            .ok_or("没有可用的 API Key")?,
    };
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": model,
        "messages": [{ "role": "user", "content": "ping" }],
        "max_tokens": 5,
        "stream": false,
    });
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .bearer_auth(token.trim())
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let msg = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()).map(String::from))
            .unwrap_or_else(|| text.chars().take(200).collect());
        return Err(format!("HTTP {}: {}", status.as_u16(), msg));
    }
    let reply = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| {
            v.get("choices")?
                .get(0)?
                .get("message")?
                .get("content")?
                .as_str()
                .map(String::from)
        })
        .unwrap_or_else(|| "(ok)".into());
    Ok(reply.chars().take(80).collect())
}

/// Migrate plaintext env-var keys into the OS keyring: for each preset whose
/// env_key is a plain env var (not WANCODE_KEY_*) that currently resolves,
/// copy the value into the keyring and switch the preset to a keyring-backed
/// env_key. Non-destructive to the user's system env vars. Returns count moved.
#[tauri::command]
pub async fn migrate_env_keys() -> Result<usize, String> {
    let path = user_config_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("配置解析失败: {e}"))?;
    let mut moved = 0usize;
    // Collect keys to migrate first (avoid borrow conflicts).
    let mut todo: Vec<(String, String)> = Vec::new(); // (preset_key, plaintext_value)
    if let Some(models) = doc.get("model").and_then(|v| v.as_table()) {
        for (key, item) in models.iter() {
            let env_key = item
                .as_table_like()
                .and_then(|t| t.get("env_key"))
                .and_then(|v| v.as_str());
            if let Some(ek) = env_key {
                if ek == wancode_env_key(key) {
                    continue; // already managed
                }
                if let Ok(val) = std::env::var(ek) {
                    if !val.is_empty() {
                        todo.push((key.to_string(), val));
                    }
                }
            }
        }
    }
    for (key, val) in todo {
        if keyring::Entry::new(KEYRING_SERVICE, &key)
            .and_then(|e| e.set_password(&val))
            .is_ok()
        {
            if let Some(models) = doc.get_mut("model").and_then(|v| v.as_table_mut()) {
                if let Some(entry) = models.get_mut(&key).and_then(|i| i.as_table_mut()) {
                    entry["env_key"] = toml_edit::value(wancode_env_key(&key));
                }
            }
            moved += 1;
        }
    }
    if moved > 0 {
        std::fs::write(&path, doc.to_string()).map_err(|e| format!("写入配置失败: {e}"))?;
    }
    Ok(moved)
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

// ── 崩溃恢复（v0.12.2）────────────────────────────────────────────
// 会话启动时写 dirty 标记，优雅退出改 clean。下次启动发现 dirty →
// 前端横幅一键恢复。指标「崩溃恢复率 100%」的执行机制。

fn last_session_marker_path() -> PathBuf {
    xai_grok_shell::util::grok_home::grok_home().join("wancode-last-session.json")
}

fn write_session_marker(session_id: &str, workspace: &str, clean: bool) {
    let v = serde_json::json!({
        "sessionId": session_id,
        "workspace": workspace,
        "cleanExit": clean,
    });
    let _ = std::fs::write(last_session_marker_path(), v.to_string());
}

/// Dirty marker from a previous run, if any（读取后不清除——由前端在
/// 恢复或忽略后调用 crash_recovery_ack）。
#[tauri::command]
pub fn crash_recovery_info() -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(last_session_marker_path()).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    if v.get("cleanExit").and_then(|b| b.as_bool()) == Some(false) {
        Some(v)
    } else {
        None
    }
}

/// 前端已处理（恢复或忽略）——把标记改 clean，避免横幅重复出现。
#[tauri::command]
pub fn crash_recovery_ack() {
    if let Ok(text) = std::fs::read_to_string(last_session_marker_path()) {
        if let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&text) {
            v["cleanExit"] = serde_json::json!(true);
            let _ = std::fs::write(last_session_marker_path(), v.to_string());
        }
    }
}

/// Graceful-exit hook（lib.rs 在窗口关闭时调用）。
pub fn mark_clean_exit() {
    crash_recovery_ack();
}

/// Startup model-config verdict. See `validate_startup_models`.
pub enum StartupModels {
    Ok,
    NoModels,
    /// default 悬空但已就地修复（写回磁盘），携带修复后的模型 id
    RepairedDefault(String),
    Invalid(String),
}

/// The single startup gate: every engine-boot path must pass through this.
///
/// 纯配置检查，不碰引擎。规则：
/// - 无 [model.*] 条目 → NoModels（前端应转向导）
/// - [models].default 指向不存在的模型 → 自动改指第一个存在的模型并写回；
///   写回失败 → Invalid
/// - 配置文件解析失败 → Invalid（绝不吞成"没有模型"，那会误开向导覆盖
///   用户配置的心智）
pub fn validate_startup_models() -> StartupModels {
    validate_startup_models_at(&user_config_path())
}

/// 路径可注入版本——单测用（首启状态矩阵，v0.12.2）。
pub fn validate_startup_models_at(path: &std::path::Path) -> StartupModels {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return StartupModels::NoModels,
        Err(e) => return StartupModels::Invalid(format!("读取配置失败: {e}")),
    };
    let mut doc: toml_edit::DocumentMut = match text.parse() {
        Ok(d) => d,
        Err(e) => return StartupModels::Invalid(format!("配置解析失败: {e}")),
    };

    let model_ids: Vec<String> = doc
        .get("model")
        .and_then(|v| v.as_table())
        .map(|t| {
            t.iter()
                .filter_map(|(_, e)| e.get("model").and_then(|v| v.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if model_ids.is_empty() {
        return StartupModels::NoModels;
    }

    let dangling = doc
        .get("models")
        .and_then(|m| m.get("default"))
        .and_then(|v| v.as_str())
        .map(|d| !model_ids.iter().any(|m| m == d))
        .unwrap_or(false);
    if dangling {
        let fixed = model_ids[0].clone();
        if let Some(models_tbl) = doc.get_mut("models").and_then(|v| v.as_table_mut()) {
            models_tbl["default"] = toml_edit::value(fixed.as_str());
        }
        if let Err(e) = std::fs::write(path, doc.to_string()) {
            return StartupModels::Invalid(format!("default 悬空且写回修复失败: {e}"));
        }
        return StartupModels::RepairedDefault(fixed);
    }
    StartupModels::Ok
}

#[cfg(test)]
mod config_txn_tests {
    use super::*;

    const GLM_OPEN: &[(&str, &str, &str)] =
        &[("glm", "智谱 GLM-5.2", "glm-5.2"), ("glm-air", "智谱 GLM-5-Air", "glm-5-air")];

    #[test]
    fn preset_writes_all_models_with_env_keys() {
        let mut doc = toml_edit::DocumentMut::new();
        apply_provider_preset(&mut doc, GLM_OPEN, "https://open.bigmodel.cn/api/paas/v4");
        let out = doc.to_string();
        assert!(out.contains(r#"[model.glm]"#));
        assert!(out.contains(r#"[model.glm-air]"#));
        assert!(out.contains(r#"env_key = "WANCODE_KEY_GLM""#));
        assert!(out.contains(r#"model = "glm-5-air""#));
    }

    #[test]
    fn seeding_is_once_and_never_overwrites() {
        let mut doc: toml_edit::DocumentMut = r#"
[mcp_servers.web-reader]
url = "https://example.com/custom"
"#
        .parse()
        .unwrap();
        assert!(seed_default_mcp_into(&mut doc));
        let out = doc.to_string();
        // 已有的 web-reader 绝不覆盖；web-search 补上；标记写入
        assert!(out.contains("https://example.com/custom"));
        assert!(!out.contains("web_reader/mcp"));
        assert!(out.contains("web_search/mcp"));
        assert!(out.contains("wancode_mcp_seeded = true"));
        // 零明文：只允许环境变量引用
        assert!(out.contains("Bearer ${ZHIPU_API_KEY}"));
        // 第二次是 no-op
        assert!(!seed_default_mcp_into(&mut doc));
    }

    #[test]
    fn atomic_write_replaces_existing() {
        let p = std::env::temp_dir().join(format!("wancode-atomic-{}.toml", std::process::id()));
        std::fs::write(&p, "old").unwrap();
        write_config_atomic(&p, "new-content").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new-content");
        // 临时文件不残留
        let tmp = p.with_extension(format!("tmp-{}", std::process::id()));
        assert!(!tmp.exists());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn preset_plus_seed_commit_in_one_doc() {
        // 模型与播种同一事务：单个 doc 内两者都在，一次写盘
        let mut doc = toml_edit::DocumentMut::new();
        apply_provider_preset(&mut doc, GLM_OPEN, "https://open.bigmodel.cn/api/paas/v4");
        assert!(seed_default_mcp_into(&mut doc));
        let out = doc.to_string();
        assert!(out.contains("[model.glm]") && out.contains("web_search/mcp"));
    }
}

#[cfg(test)]
mod startup_gate_tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(name: &str, content: Option<&str>) -> PathBuf {
        let p = std::env::temp_dir().join(format!("wancode-test-{name}-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&p);
        if let Some(c) = content {
            std::fs::write(&p, c).unwrap();
        }
        p
    }

    const VALID: &str = r#"
[models]
default = "glm-5.2"

[model.glm]
model = "glm-5.2"
name = "g"
base_url = "https://x"
"#;

    const DANGLING: &str = r#"
[models]
default = "ghost"

[model.glm]
model = "glm-5.2"
name = "g"
base_url = "https://x"
"#;

    #[test]
    fn no_file_means_no_models() {
        let p = tmp("nofile", None);
        assert!(matches!(validate_startup_models_at(&p), StartupModels::NoModels));
    }

    #[test]
    fn features_only_means_no_models() {
        let p = tmp("features", Some("[features]
telemetry = false
"));
        assert!(matches!(validate_startup_models_at(&p), StartupModels::NoModels));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn valid_config_is_ok() {
        let p = tmp("valid", Some(VALID));
        assert!(matches!(validate_startup_models_at(&p), StartupModels::Ok));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn dangling_default_gets_repaired_and_persisted() {
        let p = tmp("dangling", Some(DANGLING));
        assert!(matches!(
            validate_startup_models_at(&p),
            StartupModels::RepairedDefault(f) if f == "glm-5.2"
        ));
        // 修复必须落盘——只修内存等于没修（下次启动照样崩）
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(after.contains(r#"default = "glm-5.2""#));
        assert!(matches!(validate_startup_models_at(&p), StartupModels::Ok));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn broken_toml_is_invalid_not_no_models() {
        // 解析失败绝不能吞成 NoModels：那会开向导、把用户带向覆盖自己配置的路
        let p = tmp("broken", Some("[model.glm
model = \"x\""));
        assert!(matches!(validate_startup_models_at(&p), StartupModels::Invalid(_)));
        let _ = std::fs::remove_file(&p);
    }
}

/// Inject managed model keys from keyring into the process env so the engine's
/// `env_key` lookup resolves them. Call before starting a session.
fn inject_managed_keys() {
    let path = user_config_path();
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else { return };
    if let Some(models) = doc.get("model").and_then(|v| v.as_table()) {
        for (key, item) in models.iter() {
            let pw = keyring::Entry::new(KEYRING_SERVICE, key)
                .ok()
                .and_then(|e| e.get_password().ok());
            let Some(pw) = pw else { continue };
            let env_key = wancode_env_key(key);
            if std::env::var(&env_key).is_err() {
                // Safety: single-threaded startup path before session spawn.
                unsafe { std::env::set_var(&env_key, &pw) };
            }
            // 播种的默认 MCP 用 ${ZHIPU_API_KEY} 引用——智谱模型的 Key 顺带
            // 导出到这个名字，重启后 web-search MCP 才能解析出授权头。
            let is_zhipu = item
                .get("base_url")
                .and_then(|v| v.as_str())
                .is_some_and(|u| u.contains("bigmodel.cn"));
            if is_zhipu && std::env::var("ZHIPU_API_KEY").is_err() {
                unsafe { std::env::set_var("ZHIPU_API_KEY", &pw) };
            }
        }
    }
}

// ── Skills (~/.grok/skills/<name>/SKILL.md) ─────────────────────────

#[derive(Serialize, Clone)]
pub struct SkillEntry {
    pub name: String,
    pub description: String,
    pub path: String,
}

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

/// Read `[mcp_servers]` entries from the user config.
#[tauri::command]
pub async fn mcp_config_list() -> Result<Vec<McpServerEntry>, String> {
    let path = user_config_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("配置解析失败: {e}"))?;
    let mut out = Vec::new();
    if let Some(servers) = doc.get("mcp_servers").and_then(|v| v.as_table()) {
        for (name, item) in servers.iter() {
            let t = item.as_table_like();
            let get_str = |k: &str| {
                t.and_then(|t| t.get(k))
                    .and_then(|v| v.as_str())
                    .map(String::from)
            };
            out.push(McpServerEntry {
                name: name.to_string(),
                command: get_str("command"),
                url: get_str("url"),
                args: t
                    .and_then(|t| t.get("args"))
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
                enabled: t
                    .and_then(|t| t.get("enabled"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true),
            });
        }
    }
    Ok(out)
}

/// Add or replace a stdio/HTTP MCP server in the user config.
#[tauri::command]
pub async fn mcp_config_upsert(
    name: String,
    command: Option<String>,
    args: Vec<String>,
    url: Option<String>,
) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("名称不能为空".into());
    }
    let path = user_config_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("配置解析失败: {e}"))?;
    let servers = doc["mcp_servers"]
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .ok_or("mcp_servers 段类型异常")?;
    let mut entry = toml_edit::Table::new();
    match (&command, &url) {
        (Some(cmd), _) if !cmd.trim().is_empty() => {
            entry["command"] = toml_edit::value(cmd.trim());
            if !args.is_empty() {
                let mut arr = toml_edit::Array::new();
                for a in &args {
                    arr.push(a.as_str());
                }
                entry["args"] = toml_edit::value(arr);
            }
        }
        (_, Some(u)) if !u.trim().is_empty() => {
            entry["url"] = toml_edit::value(u.trim());
        }
        _ => return Err("command 与 url 至少填一个".into()),
    }
    servers.insert(name.trim(), toml_edit::Item::Table(entry));
    std::fs::write(&path, doc.to_string()).map_err(|e| format!("写入配置失败: {e}"))
}

/// Remove an MCP server from the user config.
#[tauri::command]
pub async fn mcp_config_remove(name: String) -> Result<(), String> {
    let path = user_config_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("配置解析失败: {e}"))?;
    if let Some(servers) = doc.get_mut("mcp_servers").and_then(|v| v.as_table_mut()) {
        servers.remove(&name);
    }
    std::fs::write(&path, doc.to_string()).map_err(|e| format!("写入配置失败: {e}"))
}

/// Call an `x.ai/*` ACP extension method against the live session and
/// return the raw JSON response.
async fn ext_call(
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
async fn ext_ok(
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

// ── P2.5 Git 补全 ───────────────────────────────────────────────────

#[tauri::command]
pub async fn git_stash(
    state: State<'_, AgentState>,
    message: Option<String>,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/git/stash", serde_json::json!({ "message": message })).await
}

#[tauri::command]
pub async fn git_info(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/git/info", serde_json::json!({})).await
}

#[tauri::command]
pub async fn git_current_commit(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/git/current_commit", serde_json::json!({})).await
}

#[tauri::command]
pub async fn git_files(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/git/files", serde_json::json!({})).await
}

#[tauri::command]
pub async fn git_repo_root(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/git/git_repo_root", serde_json::json!({})).await
}

#[tauri::command]
pub async fn git_serialize_changes(
    state: State<'_, AgentState>,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/git/serialize_changes", serde_json::json!({})).await
}

#[tauri::command]
pub async fn git_checkout_commit(
    state: State<'_, AgentState>,
    commit: String,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/git/checkout_commit", serde_json::json!({ "commit": commit })).await
}

#[tauri::command]
pub async fn git_checkout_session_head(
    state: State<'_, AgentState>,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/git/checkout_session_head", serde_json::json!({})).await
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

// ── Git worktree 并行 Agent ────────────────────────────────────────
//
// 用法是：把当前会话在一个独立 worktree 里再开一份，两边各跑各的、互不
// 干扰文件；试完了要么把改动合回主目录，要么整个丢掉。
//
// 注意 list 的请求结构是这一片里唯一不用 camelCase 的：`include_all` 是
// snake_case，且 `repo` 没有 serde(default)——**必须显式传，哪怕是 null**，
// 少了整个反序列化就失败。（官方 TUI 自己发的是 includeAll，那个过滤参数
// 在人家客户端里是静默失效的。）

/// Take the current session into a fresh worktree.
///
/// Returns the **forked** session id (not the one passed in) plus the worktree
/// path — the caller then opens that session at that path.
#[tauri::command]
pub async fn worktree_resume_session(
    state: State<'_, AgentState>,
    workspace: String,
) -> Result<serde_json::Value, String> {
    let source = {
        let guard = state.handle.lock().await;
        guard.as_ref().ok_or("会话未启动")?.session_id.0.to_string()
    };
    let v = ext_call(
        &state,
        "x.ai/git/worktree/resume_session",
        serde_json::json!({
            "sessionId": source,
            "sourceCwd": workspace,
            "copyMode": "dirty",
            "restoreCode": true,
        }),
    )
    .await?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    v.get("result")
        .cloned()
        .ok_or_else(|| format!("resume_session 未返回结果: {v}"))
}

/// List worktrees. See the casing note above — this one is snake_case.
#[tauri::command]
pub async fn worktree_list(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    let v = ext_call(
        &state,
        "x.ai/git/worktree/list",
        serde_json::json!({ "repo": null, "type": [], "include_all": false }),
    )
    .await?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    Ok(v.get("result").cloned().unwrap_or(v))
}

/// Merge a worktree's changes back into the main working directory.
///
/// The response is status-tagged: `{"status":"success", files}` or
/// `{"status":"conflicts", files, conflicts}`. Conflicts are surfaced to the
/// user verbatim — we deliberately do not attempt a three-way merge UI.
#[tauri::command]
pub async fn worktree_apply(
    state: State<'_, AgentState>,
    worktree_path: String,
) -> Result<serde_json::Value, String> {
    let source = {
        let guard = state.handle.lock().await;
        guard.as_ref().ok_or("会话未启动")?.session_id.0.to_string()
    };
    let v = ext_call(
        &state,
        "x.ai/git/worktree/apply",
        serde_json::json!({
            "sessionId": source,
            "worktreePath": worktree_path,
            // merge 而不是 overwrite：overwrite 是无条件把 worktree 的内容写进
            // 主目录，**从不报冲突**——用户在主目录里对同一文件的改动会被静默
            // 销毁。merge 只在主目录没动过时才应用，两边都改了就报冲突。
            "mode": "merge",
        }),
    )
    .await?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    Ok(v.get("result").cloned().unwrap_or(v))
}

/// Remove a worktree. `idOrPath` and the legacy `worktreePath` are mutually
/// exclusive — sending both is an error, so only send one.
#[tauri::command]
pub async fn worktree_remove(
    state: State<'_, AgentState>,
    id_or_path: String,
    force: bool,
) -> Result<serde_json::Value, String> {
    let v = ext_call(
        &state,
        "x.ai/git/worktree/remove",
        serde_json::json!({ "idOrPath": id_or_path, "force": force, "dryRun": false }),
    )
    .await?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    Ok(v.get("result").cloned().unwrap_or(v))
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
/// Resolve the session workspace's git root LOCALLY (git2 discover).
///
/// #83 的根修：引擎的 git/* 在「会话目录不是仓库」时把 resolve 失败
/// `.ok()` 吞成 None，随后 git_op_cwd 回退到 workspace-hub 根——嵌入式
/// 场景那是本应用自己的仓库，于是「不是仓库」被静默替换成「另一个仓库
/// 的状态」，stash/丢弃会打错目标。gitRoot 一律客户端解析、显式传入；
/// 解析不出就本地拒绝，引擎的回退路径永远不被触发。
async fn session_git_root(state: &State<'_, AgentState>) -> Result<Option<String>, String> {
    let cwd = state
        .handle
        .lock()
        .await
        .as_ref()
        .map(|h| h.cwd.clone())
        .ok_or("会话未启动")?;
    Ok(git2::Repository::discover(&cwd)
        .ok()
        .and_then(|r| r.workdir().map(|p| p.to_string_lossy().into_owned())))
}

#[tauri::command]
pub async fn git_status_ext(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    let Some(root) = session_git_root(&state).await? else {
        // 明确的「不是仓库」标记；前端渲染为 非 git 仓库，绝不显示别的仓库
        return Ok(serde_json::json!({ "result": { "data": null } }));
    };
    ext_call(
        &state,
        "x.ai/git/status",
        serde_json::json!({ "gitRoot": root, "includeUntracked": true, "includeStats": true }),
    )
    .await
}

/// Unified diffs for the given paths (empty = every change).
#[tauri::command]
pub async fn git_diffs(
    state: State<'_, AgentState>,
    paths: Option<Vec<String>>,
) -> Result<serde_json::Value, String> {
    ext_call(&state, "x.ai/git/diffs", serde_json::json!({ "paths": paths })).await
}

/// Stage / unstage / discard the given paths (`None` = all).
#[tauri::command]
pub async fn git_stage(
    state: State<'_, AgentState>,
    paths: Option<Vec<String>>,
) -> Result<serde_json::Value, String> {
    ext_call(&state, "x.ai/git/stage", serde_json::json!({ "paths": paths })).await
}

#[tauri::command]
pub async fn git_unstage(
    state: State<'_, AgentState>,
    paths: Option<Vec<String>>,
) -> Result<serde_json::Value, String> {
    ext_call(&state, "x.ai/git/unstage", serde_json::json!({ "paths": paths })).await
}

#[tauri::command]
pub async fn git_discard(
    state: State<'_, AgentState>,
    paths: Option<Vec<String>>,
) -> Result<serde_json::Value, String> {
    ext_call(&state, "x.ai/git/discard", serde_json::json!({ "paths": paths })).await
}

/// Commit what is currently staged.
#[tauri::command]
pub async fn git_commit(
    state: State<'_, AgentState>,
    message: String,
    amend: Option<bool>,
) -> Result<serde_json::Value, String> {
    ext_call(
        &state,
        "x.ai/git/commit",
        serde_json::json!({ "message": message, "amend": amend.unwrap_or(false) }),
    )
    .await
}

/// Branch list, and checkout.
#[tauri::command]
pub async fn git_branches(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    ext_call(&state, "x.ai/git/branches", serde_json::json!({})).await
}

#[tauri::command]
pub async fn git_checkout(
    state: State<'_, AgentState>,
    branch: String,
) -> Result<serde_json::Value, String> {
    ext_call(&state, "x.ai/git/checkout", serde_json::json!({ "branch": branch })).await
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
