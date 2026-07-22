//! v0.18-4 步 B：引擎透传命令大族——MCP 配置/实时、终端与 PTY、fuzzy、
//! fs/*、会话管理、记忆、后台任务/子 Agent、rewind、模式/模型切换。
//! 基本全部是 ext_call/ext_ok/ext_notify 一行转发；语义红线见各注释。
use std::path::PathBuf;

use agent_client_protocol as acp;
use tauri::State;
use xai_acp_lib::acp_send;

use crate::agent::{ext_call, ext_notify, ext_ok, AgentState};

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
