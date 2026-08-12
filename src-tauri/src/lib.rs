mod agent;
mod config_core;
mod crash_recovery;
mod git_ops;
mod skills_ops;
mod engine_ops;
mod review_ops;
mod autotest;
mod provider_ops;
pub mod caps_snapshot;
pub mod model_caps;
pub mod updater_launch;
mod updater_ops;
pub mod surface;
pub mod surface_gate;
pub mod surface_policy;
pub mod work_staging;
pub mod work_import;
#[cfg(test)]
mod plugin_lifecycle_probe;
#[cfg(test)]
mod compat_contracts;

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
        JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            eprintln!("[job] CreateJobObject 失败，进程树治理未启用");
            return;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        // BREAKAWAY_OK 只"允许"显式脱离（#129：更新安装器必须活过应用退出），
        // 不是 SILENT——AI 起的子进程仍然全部留在 Job 里被治理。脱离必须由
        // 创建方显式传 CREATE_BREAKAWAY_FROM_JOB（见 updater_launch）。
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_PROCESS_MEMORY
            | JOB_OBJECT_LIMIT_BREAKAWAY_OK;
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
    // 多模型产品决策：图片永远走 image_description 视觉辅助模型转述
    // （引擎本地补丁开关），纯文本主模型（GLM-5.2 coding 端点等）也能粘图。
    // 用户可在启动前显式设 GROK_IMAGE_TRANSCRIBE=0 关闭（则回退内联，仅视觉主模型可用）。
    if std::env::var("GROK_IMAGE_TRANSCRIBE").is_err() {
        // SAFETY: 同上，启动早期单线程
        unsafe { std::env::set_var("GROK_IMAGE_TRANSCRIBE", "1") };
    }
    // 智谱 glm-4v-flash（默认视觉辅助模型）max_tokens 上限 1024，
    // 引擎 describe 默认 4096 会被 400 拒绝。单图文字描述 1024 足够。
    if std::env::var("GROK_IMAGE_DESCRIBE_MAX_TOKENS").is_err() {
        // SAFETY: 同上
        unsafe { std::env::set_var("GROK_IMAGE_DESCRIBE_MAX_TOKENS", "1024") };
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(agent::AgentState::default())
        .manage(caps_snapshot::CapsState::init())
        .manage(updater_ops::PendingUpdate::default())
        .setup(|app| {
            // ── v0.19-2a 迁移门：sidecar 落 app_data_dir()/surface-bindings/，
            // 启动即触发迁移；agent_start（含 autotest 的 start_inner 路径）
            // 都在 start_inner 顶部等待同一个门。
            {
                use tauri::Manager;
                // autotest/smoke 用隔离 sidecar 根（工作区目录内），避免
                // 开发分支的 smoke 反复改写真实用户的迁移状态与 binding。
                let root = if let Ok(ws) = std::env::var("WANCODE_AUTOTEST") {
                    std::path::PathBuf::from(ws).join("surface-bindings")
                } else {
                    app.path()
                        .app_data_dir()
                        .map_err(|e| format!("app_data_dir 不可用: {e}"))?
                        .join("surface-bindings")
                };
                app.manage(surface_gate::SurfaceState::new(root));
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let state = handle.state::<surface_gate::SurfaceState>();
                    if let Err(e) = state.ensure_migrated().await {
                        // 启动预热失败只记日志：真正的门在 start_inner，
                        // 会话启动时会重试并把结构化错误给前端。
                        tracing::warn!("surface 迁移预热失败（会话启动时将重试）：{e}");
                    }
                });
            }
            if let Ok(ws) = std::env::var("WANCODE_AUTOTEST") {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    autotest::autotest(handle, ws).await;
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            updater_ops::updater_download,
            updater_ops::updater_install,
            read_file,
            agent::agent_start,
            agent::agent_prompt,
            agent::agent_permission_respond,
            agent::agent_plan_respond,
            agent::agent_cancel,
            engine_ops::agent_set_mode,
            engine_ops::agent_set_model,
            agent::agent_queue_remove,
            agent::agent_queue_clear,
            agent::agent_compact,
            agent::agent_question_respond,
            agent::agent_trust_respond,
            engine_ops::agent_commands_list,
            engine_ops::agent_prompt_history,
            engine_ops::search_content,
            engine_ops::session_fork,
            engine_ops::scheduler_delete,
            agent::recent_sessions,
            agent::agent_interject,
            agent::agent_queue_edit,
            agent::agent_queue_reorder,
            agent::agent_queue_interject,
            agent::agent_toggle_plan_mode,
            agent::permissions_reset,
            agent::agent_sync_permission_mode,
            engine_ops::models_reload_live,
            engine_ops::mcp_upsert,
            engine_ops::mcp_delete,
            engine_ops::mcp_read_resource,
            engine_ops::session_update_mcp_servers,
            engine_ops::terminal_list,
            engine_ops::terminal_output,
            engine_ops::terminal_create,
            engine_ops::terminal_background,
            engine_ops::terminal_release,
            engine_ops::terminal_wait_for_exit,
            engine_ops::pty_load,
            git_ops::git_stash,
            git_ops::git_info,
            git_ops::git_current_commit,
            git_ops::git_files,
            git_ops::git_repo_root,
            git_ops::git_serialize_changes,
            git_ops::git_checkout_commit,
            git_ops::git_checkout_session_head,
            engine_ops::fuzzy_open,
            engine_ops::fuzzy_change,
            engine_ops::fuzzy_close,
            engine_ops::fs_list,
            engine_ops::fs_read,
            engine_ops::fs_write,
            engine_ops::fs_exists,
            engine_ops::fs_delete,
            engine_ops::session_list_engine,
            engine_ops::session_close,
            engine_ops::session_load_history,
            engine_ops::session_repair,
            engine_ops::session_updates_fetch,
            engine_ops::session_summaries_for_cwd,
            engine_ops::workspace_list_recent,
            engine_ops::workspaces_list,
            engine_ops::sessions_roster,
            engine_ops::memory_flush,
            engine_ops::memory_rewrite,
            engine_ops::subagent_get,
            engine_ops::agent_recap,
            engine_ops::agent_suggest,
            engine_ops::hooks_engine_list,
            git_ops::worktree_resume_session,
            git_ops::worktree_list,
            git_ops::worktree_apply,
            git_ops::worktree_remove,
            engine_ops::pty_create,
            engine_ops::pty_input,
            engine_ops::pty_resize,
            engine_ops::pty_kill,
            engine_ops::workspace_list,
            engine_ops::mcp_live_list,
            engine_ops::mcp_toggle,
            engine_ops::mcp_toggle_tool,
            engine_ops::mcp_auth_status,
            engine_ops::mcp_auth_trigger,
            engine_ops::tasks_list,
            engine_ops::task_kill,
            engine_ops::subagents_list,
            engine_ops::subagent_cancel,
            git_ops::git_status_ext,
            git_ops::git_diffs,
            git_ops::git_stage,
            git_ops::git_unstage,
            git_ops::git_discard,
            git_ops::git_commit,
            git_ops::git_branches,
            git_ops::git_checkout,
            agent::default_workspace,
            agent::chat_workspace,
            work_import::work_import,
            agent::agent_list_sessions,
            agent::agent_list_mcp,
            engine_ops::agent_session_info,
            engine_ops::agent_rewind_points,
            review_ops::review_run,
            git_ops::git_create_pr,
            git_ops::git_pr_status,
            git_ops::worktree_precheck,
            git_ops::worktree_snapshot,
            engine_ops::agent_rewind,
            engine_ops::agent_session_rename,
            engine_ops::agent_session_delete,
            engine_ops::agent_session_search,
            engine_ops::list_workspace_files,
            skills_ops::hooks_list,
            skills_ops::hooks_save,
            skills_ops::skills_list,
            skills_ops::skills_toggle,
            skills_ops::skills_add_path,
            skills_ops::skills_remove_path,
            skills_ops::skills_reset,
            skills_ops::skills_config,
            skills_ops::skills_create,
            skills_ops::skills_open,
            provider_ops::model_list,
            provider_ops::image_send_check,
            provider_ops::model_caps_diagnostics,
            provider_ops::model_caps_reload,
            provider_ops::model_upsert,
            provider_ops::model_remove,
            provider_ops::provider_quick_setup,
            provider_ops::model_test,
            provider_ops::migrate_env_keys,
            skills_ops::skill_read,
            skills_ops::skill_write,
            provider_ops::mcp_config_list,
            provider_ops::mcp_config_upsert,
            provider_ops::mcp_config_remove,
            crash_recovery::crash_recovery_info,
            crash_recovery::crash_recovery_ack,
        ])
        .on_window_event(|_w, e| {
            // 优雅关闭 → 标记 clean，崩溃则标记保持 dirty（下次启动出恢复横幅）
            if matches!(e, tauri::WindowEvent::CloseRequested { .. }) {
                crash_recovery::mark_clean_exit();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
