mod agent;

use xai_grok_paths::AbsPathBuf;

/// M0.4 minimal-link proof: validate the path with grok-build's
/// `xai-grok-paths` types, then return the file contents.
#[tauri::command]
fn read_file(path: String) -> Result<String, String> {
    let abs =
        AbsPathBuf::new(path.into()).map_err(|e| format!("invalid absolute path: {e}"))?;
    std::fs::read_to_string(abs.as_path()).map_err(|e| format!("read failed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_file_roundtrip_through_grok_paths() {
        let dir = std::env::temp_dir().join("wancode-m0-test");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("hello.txt");
        std::fs::write(&file, "你好 WanCode").unwrap();
        let content = read_file(file.to_string_lossy().into_owned()).unwrap();
        assert_eq!(content, "你好 WanCode");
    }

    #[test]
    fn read_file_rejects_relative_path() {
        assert!(read_file("relative/path.txt".into()).is_err());
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(agent::AgentState::default())
        .setup(|app| {
            if let Ok(ws) = std::env::var("WANCODE_AUTOTEST") {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    agent::autotest(handle, ws).await;
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            read_file,
            agent::agent_start,
            agent::agent_prompt,
            agent::agent_permission_respond,
            agent::agent_cancel,
            agent::agent_list_sessions,
            agent::agent_list_mcp,
            agent::agent_session_info,
            agent::agent_rewind_points,
            agent::agent_rewind,
            agent::agent_session_rename,
            agent::agent_session_delete,
            agent::mcp_config_list,
            agent::mcp_config_upsert,
            agent::mcp_config_remove,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
