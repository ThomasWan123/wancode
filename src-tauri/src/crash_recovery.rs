//! v0.18-2 步 B：崩溃恢复（v0.12.2 机制原样搬入）。
use std::path::PathBuf;

// ── 崩溃恢复（v0.12.2）────────────────────────────────────────────
// 会话启动时写 dirty 标记，优雅退出改 clean。下次启动发现 dirty →
// 前端横幅一键恢复。指标「崩溃恢复率 100%」的执行机制。

pub(crate) fn last_session_marker_path() -> PathBuf {
    xai_grok_shell::util::grok_home::grok_home().join("wancode-last-session.json")
}

pub(crate) fn write_session_marker(session_id: &str, workspace: &str, clean: bool) {
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
