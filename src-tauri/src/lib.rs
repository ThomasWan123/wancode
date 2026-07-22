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

/// v0.17-3 Job Object 一期：把 wancode 自身放进一个 kill-on-close 的
/// Job，PTY shell / MCP 服务器 / 引擎 bash 工具起的所有子孙进程自动
/// 继承——应用退出（含崩溃）时进程树必死，根治孤儿 node/ping 进程
/// （smoke 期间多次观察到残留）。附带每进程内存上限兜底失控分配。
///
/// 失败容忍：老系统/已在禁止嵌套的 Job 里（少见，Win10+ 支持嵌套）
/// 就打日志继续跑——这是治理增强，不是启动前置条件。
/// Job 句柄有意泄漏：它必须活到进程终结，OS 届时自动关闭并触发清杀。
#[cfg(windows)]
fn setup_job_object() {
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            eprintln!("[job] CreateJobObject 失败，进程树治理未启用");
            return;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        // 8GB/进程：主进程与 webview 也在 Job 里，上限要给它们留足余量
        info.ProcessMemoryLimit = 8 * 1024 * 1024 * 1024;
        if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) == 0
        {
            eprintln!("[job] SetInformationJobObject 失败，进程树治理未启用");
            return;
        }
        if AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
            eprintln!("[job] AssignProcessToJobObject 失败（可能已在禁嵌套 Job 内），进程树治理未启用");
        }
        // 句柄故意不关：进程退出时 OS 关闭句柄 → kill-on-close 生效
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(windows)]
    setup_job_object();
    // 引擎的 worktree DB（xai-fast-worktree::resolve_grok_home）只认
    // $GROK_HOME / $HOME，而 Windows 默认没有 HOME（只有 USERPROFILE）——
    // 从 Git Bash 启动恰好带 HOME 所以"偶尔正常"，从资源管理器/PowerShell
    // 启动 worktree 列表必报 "hub error: neither $GROK_HOME nor $HOME is
    // set"。启动最早处兜底：把 GROK_HOME 指向引擎其余部分实际在用的
    // grok_home()（~/.grok，经 USERPROFILE 解析），保证两套 home 解析一致。
    if std::env::var("GROK_HOME").is_err() {
        let home = xai_grok_shell::util::grok_home::grok_home();
        // SAFETY: 单线程启动早期，engine 线程尚未 spawn
        unsafe { std::env::set_var("GROK_HOME", &home) };
    }
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
            agent::mcp_upsert,
            agent::mcp_delete,
            agent::mcp_read_resource,
            agent::session_update_mcp_servers,
            agent::terminal_list,
            agent::terminal_output,
            agent::terminal_create,
            agent::terminal_background,
            agent::terminal_release,
            agent::terminal_wait_for_exit,
            agent::pty_load,
            agent::git_stash,
            agent::git_info,
            agent::git_current_commit,
            agent::git_files,
            agent::git_repo_root,
            agent::git_serialize_changes,
            agent::git_checkout_commit,
            agent::git_checkout_session_head,
            agent::fuzzy_open,
            agent::fuzzy_change,
            agent::fuzzy_close,
            agent::fs_list,
            agent::fs_read,
            agent::fs_write,
            agent::fs_exists,
            agent::fs_delete,
            agent::session_list_engine,
            agent::session_close,
            agent::session_load_history,
            agent::session_repair,
            agent::session_updates_fetch,
            agent::session_summaries_for_cwd,
            agent::workspace_list_recent,
            agent::workspaces_list,
            agent::sessions_roster,
            agent::memory_flush,
            agent::memory_rewrite,
            agent::subagent_get,
            agent::agent_recap,
            agent::agent_suggest,
            agent::hooks_engine_list,
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
            agent::review_run,
            agent::git_create_pr,
            agent::git_pr_status,
            agent::worktree_precheck,
            agent::worktree_snapshot,
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
            agent::provider_quick_setup,
            agent::model_test,
            agent::migrate_env_keys,
            agent::skill_read,
            agent::skill_write,
            agent::mcp_config_list,
            agent::mcp_config_upsert,
            agent::mcp_config_remove,
            agent::crash_recovery_info,
            agent::crash_recovery_ack,
        ])
        .on_window_event(|_w, e| {
            // 优雅关闭 → 标记 clean，崩溃则标记保持 dirty（下次启动出恢复横幅）
            if matches!(e, tauri::WindowEvent::CloseRequested { .. }) {
                agent::mark_clean_exit();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
