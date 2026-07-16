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
}

#[derive(Default)]
pub struct AgentState {
    handle: Mutex<Option<AgentHandle>>,
    pending_permissions: Mutex<HashMap<u64, oneshot::Sender<Option<String>>>>,
    next_permission_id: AtomicU64,
}

#[derive(Serialize, Clone)]
pub struct StartResult {
    pub session_id: String,
    pub models: Vec<String>,
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
    let cwd = PathBuf::from(&workspace);
    if !cwd.is_dir() {
        return Err(anyhow!("工作区目录不存在: {workspace}"));
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

    // Trust the workspace folder the user explicitly opened.
    xai_grok_shell::agent::folder_trust::grant_folder_trust(&cwd);

    let cancel = CancellationToken::new();
    let memory_config = agent_config.memory_config.clone();
    let spawned = spawn_grok_shell(agent_config, &cancel, memory_config)
        .await
        .map_err(|e| anyhow!("启动 Agent 失败: {e}"))?;
    let acp_tx = spawned.channel.tx;
    let mut acp_rx = spawned.channel.rx;

    // ── Initialize ─────────────────────────────────────────────────
    let init_req = acp::InitializeRequest::new(acp::ProtocolVersion::V1)
        .client_capabilities(
            acp::ClientCapabilities::new()
                .fs(acp::FileSystemCapabilities::new())
                .terminal(false),
        )
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
        acp_tx,
        session_id: session_id.clone(),
        cancel,
        model_ids: model_ids.clone(),
    });

    Ok(StartResult {
        session_id: session_id.0.to_string(),
        models: model_ids,
    })
}

async fn handle_acp_message(app: &AppHandle, msg: AcpClientMessage) {
    match msg {
        AcpClientMessage::SessionNotification(boxed) => {
            let payload =
                serde_json::to_value(&boxed.request.update).unwrap_or(serde_json::Value::Null);
            if std::env::var("WANCODE_AUTOTEST").is_ok() {
                use std::io::Write;
                let log = std::env::temp_dir().join("wancode-autotest.log");
                if let Ok(mut f) =
                    std::fs::OpenOptions::new().create(true).append(true).open(&log)
                {
                    let _ = writeln!(f, "update: {payload}");
                }
            }
            let _ = app.emit("agent://update", payload);
            let _ = boxed.response_tx.send(Ok(()));
        }
        AcpClientMessage::RequestPermission(req) => {
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
        _ => {}
    }
}

/// Self-test driven by `WANCODE_AUTOTEST=<workspace-dir>`: exercises the full
/// backend glue (start → prompt → events) without the UI and logs the result
/// to `%TEMP%\wancode-autotest.log`.
pub async fn autotest(app: AppHandle, workspace: String) {
    let log = std::env::temp_dir().join("wancode-autotest.log");
    let write = |s: &str| {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log) {
            let _ = writeln!(f, "{s}");
        }
    };
    write("autotest: starting session");
    let state: State<'_, AgentState> = app.state();
    let started =
        start_inner(app.clone(), &state, workspace, Some("glm-4-flash".into()), None).await;
    match &started {
        Ok(r) => write(&format!(
            "autotest: session={} models={:?}",
            r.session_id, r.models
        )),
        Err(e) => {
            write(&format!("autotest: START FAILED: {e:#}"));
            return;
        }
    }
    let (acp_tx, session_id) = {
        let guard = state.handle.lock().await;
        let h = guard.as_ref().expect("session just started");
        (h.acp_tx.clone(), h.session_id.clone())
    };
    let blocks = vec![acp::ContentBlock::Text(acp::TextContent::new(
        "读取当前目录下的 notes.md 文件，然后告诉我文件里的秘密口令是什么".to_string(),
    ))];
    write("autotest: sending prompt");
    let result: Result<acp::PromptResponse, _> =
        acp_send(acp::PromptRequest::new(session_id, blocks), &acp_tx).await;
    match result {
        Ok(resp) => write(&format!(
            "autotest: TURN OK stop_reason={:?}",
            resp.stop_reason
        )),
        Err(e) => write(&format!("autotest: TURN FAILED: {e}")),
    }
}

/// Send one user prompt; resolves when the turn completes.
#[tauri::command]
pub async fn agent_prompt(
    app: AppHandle,
    state: State<'_, AgentState>,
    text: String,
) -> Result<(), String> {
    let (acp_tx, session_id) = {
        let guard = state.handle.lock().await;
        let h = guard.as_ref().ok_or("会话未启动")?;
        (h.acp_tx.clone(), h.session_id.clone())
    };
    let blocks = vec![acp::ContentBlock::Text(acp::TextContent::new(text))];
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
        obj.entry("sessionId")
            .or_insert(serde_json::Value::String(session_id.0.to_string()));
    }
    let raw = serde_json::value::to_raw_value(&params).map_err(|e| e.to_string())?;
    let resp: acp::ExtResponse =
        acp_send(acp::ExtRequest::new(method.to_string(), raw.into()), &acp_tx)
            .await
            .map_err(|e| e.to_string())?;
    serde_json::from_str(resp.0.get()).map_err(|e| e.to_string())
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
