//! v0.18-2 步 B：崩溃恢复（v0.12.2 机制原样搬入）。
use std::path::PathBuf;

use crate::config_core::write_config_atomic;

// ── 崩溃恢复（v0.12.2）────────────────────────────────────────────
// 会话启动时写 dirty 标记，优雅退出改 clean。下次启动发现 dirty →
// 前端横幅一键恢复。指标「崩溃恢复率 100%」的执行机制。

pub(crate) fn last_session_marker_path() -> PathBuf {
    xai_grok_shell::util::grok_home::grok_home().join("wancode-last-session.json")
}

pub(crate) fn write_session_marker(
    session_id: &str,
    workspace: &str,
    clean: bool,
) -> Result<(), String> {
    write_marker_at(
        &last_session_marker_path(),
        session_id,
        workspace,
        clean,
    )
}

fn write_marker_at(
    path: &std::path::Path,
    session_id: &str,
    workspace: &str,
    clean: bool,
) -> Result<(), String> {
    let v = serde_json::json!({
        "sessionId": session_id,
        "workspace": workspace,
        "cleanExit": clean,
    });
    write_config_atomic(path, &v.to_string())
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
pub fn crash_recovery_ack() -> Result<(), String> {
    acknowledge_marker(&last_session_marker_path())
}

/// Graceful-exit hook（lib.rs 在窗口关闭时调用）。
pub fn mark_clean_exit() {
    let _ = crash_recovery_ack();
}

fn acknowledge_marker(path: &std::path::Path) -> Result<(), String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("读取崩溃恢复标记失败: {error}")),
    };
    let mut value = serde_json::from_str::<serde_json::Value>(&text)
        .map_err(|error| format!("解析崩溃恢复标记失败: {error}"))?;
    value["cleanExit"] = serde_json::json!(true);
    write_config_atomic(path, &value.to_string())
}

#[cfg(test)]
mod tests {
    use super::{acknowledge_marker, write_marker_at};

    #[test]
    fn dirty_marker_write_is_atomic_and_complete() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("marker.json");

        write_marker_at(&path, "s1", "D:/repo", false).unwrap();

        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["sessionId"], "s1");
        assert_eq!(value["workspace"], "D:/repo");
        assert_eq!(value["cleanExit"], false);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn marker_write_failure_is_visible_and_does_not_create_a_target() {
        let dir = tempfile::tempdir().unwrap();
        let missing_parent = dir.path().join("missing");
        let path = missing_parent.join("marker.json");

        let error = write_marker_at(&path, "s1", "D:/repo", false).unwrap_err();

        assert!(error.contains("写入临时配置失败"));
        assert!(!path.exists());
    }

    #[test]
    fn acknowledgement_is_atomic_and_preserves_marker_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("marker.json");
        std::fs::write(
            &path,
            r#"{"sessionId":"s1","workspace":"D:/repo","cleanExit":false}"#,
        )
        .unwrap();

        acknowledge_marker(&path).unwrap();

        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["sessionId"], "s1");
        assert_eq!(value["workspace"], "D:/repo");
        assert_eq!(value["cleanExit"], true);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn corrupt_marker_is_not_overwritten_or_silently_acknowledged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("marker.json");
        std::fs::write(&path, b"{truncated").unwrap();

        let error = acknowledge_marker(&path).unwrap_err();

        assert!(error.contains("解析崩溃恢复标记失败"));
        assert_eq!(std::fs::read(&path).unwrap(), b"{truncated");
    }

    #[test]
    fn missing_marker_acknowledgement_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        acknowledge_marker(&dir.path().join("missing.json")).unwrap();
    }
}
