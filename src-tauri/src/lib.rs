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
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
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
            agent::agent_plan_respond,
            agent::agent_cancel,
            agent::agent_set_mode,
            agent::agent_set_model,
            agent::agent_queue_remove,
            agent::agent_queue_clear,
            agent::agent_compact,
            agent::agent_question_respond,
            agent::agent_trust_respond,
            agent::agent_commands_list,
            agent::agent_prompt_history,
            agent::search_content,
            agent::session_fork,
            agent::scheduler_delete,
            agent::recent_sessions,
            agent::agent_interject,
            agent::agent_queue_edit,
            agent::agent_queue_reorder,
            agent::agent_queue_interject,
            agent::agent_toggle_plan_mode,
            agent::permissions_reset,
            agent::agent_sync_permission_mode,
            agent::worktree_resume_session,
            agent::worktree_list,
            agent::worktree_apply,
            agent::worktree_remove,
            agent::pty_create,
            agent::pty_input,
            agent::pty_resize,
            agent::pty_kill,
            agent::workspace_list,
            agent::mcp_live_list,
            agent::mcp_toggle,
            agent::mcp_toggle_tool,
            agent::mcp_auth_status,
            agent::mcp_auth_trigger,
            agent::tasks_list,
            agent::task_kill,
            agent::subagents_list,
            agent::subagent_cancel,
            agent::git_status_ext,
            agent::git_diffs,
            agent::git_stage,
            agent::git_unstage,
            agent::git_discard,
            agent::git_commit,
            agent::git_branches,
            agent::git_checkout,
            agent::default_workspace,
            agent::agent_list_sessions,
            agent::agent_list_mcp,
            agent::agent_session_info,
            agent::agent_rewind_points,
            agent::agent_rewind,
            agent::agent_session_rename,
            agent::agent_session_delete,
            agent::agent_session_search,
            agent::list_workspace_files,
            agent::hooks_list,
            agent::hooks_save,
            agent::skills_list,
            agent::skills_toggle,
            agent::skills_add_path,
            agent::skills_remove_path,
            agent::skills_reset,
            agent::skills_config,
            agent::skills_create,
            agent::skills_open,
            agent::model_list,
            agent::model_upsert,
            agent::model_remove,
            agent::model_test,
            agent::migrate_env_keys,
            agent::skill_read,
            agent::skill_write,
            agent::mcp_config_list,
            agent::mcp_config_upsert,
            agent::mcp_config_remove,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
