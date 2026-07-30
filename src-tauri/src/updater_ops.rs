//! #129：更新的下载与安装命令——安装那一跳不再交给插件。
//!
//! 插件仍负责 check + 验签下载（这部分 2026-07-30 实证是好的：落盘文件
//! 与官方 sha256 逐字节一致）；被替换的只有最后一跳：插件的 install() 用
//! ShellExecuteW（无法 breakaway）且 exit(0) 前不检查启动结果。这里改为
//! `spawn_breakaway_verified`：显式脱离 Job、确认存活、失败留在应用内报错。

use std::path::PathBuf;
use std::sync::Mutex;

use tauri::{AppHandle, Emitter, State};
use tauri_plugin_updater::UpdaterExt;

/// download 与 install 两条命令之间暂存的安装器。
#[derive(Clone)]
pub struct StagedInstaller {
    /// install 时校验前端所见与实际下载的是同一个版本。
    pub version: String,
    pub path: PathBuf,
    /// 验签通过的那份字节的 sha256——启动前重新哈希文件比对，把
    /// "minisign 验过的字节"与"最终执行的文件"绑死（防落盘后被换）。
    pub sha256: String,
}

#[derive(Default)]
pub struct PendingUpdate(pub Mutex<Option<StagedInstaller>>);

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

/// 检查并下载更新（插件验签），落盘为临时 exe，进度经 `updater://progress`
/// 事件发给前端（payload：0-100 或 -1 表示未知总长）。
/// 返回发现的版本号；无更新返回 Ok(None)。
#[tauri::command]
pub async fn updater_download(
    app: AppHandle,
    state: State<'_, PendingUpdate>,
) -> Result<Option<String>, String> {
    let updater = app
        .updater_builder()
        .build()
        .map_err(|e| format!("updater 初始化失败: {e}"))?;
    let Some(update) = updater.check().await.map_err(|e| format!("检查更新失败: {e}"))? else {
        return Ok(None);
    };
    let version = update.version.clone();

    let progress_app = app.clone();
    let version_for_progress = version.clone();
    let mut received: u64 = 0;
    let bytes = update
        .download(
            move |chunk, total| {
                received += chunk as u64;
                let pct: i32 = match total {
                    Some(t) if t > 0 => ((received * 100) / t).min(100) as i32,
                    _ => -1,
                };
                let _ = progress_app.emit(
                    "updater://progress",
                    serde_json::json!({ "version": version_for_progress, "pct": pct }),
                );
            },
            || {},
        )
        .await
        .map_err(|e| format!("下载失败: {e}"))?;

    // 落盘到带随机后缀的临时目录（复核要求：路径不可预测），文件名带
    // 版本便于事后取证（这次破案就靠 %TEMP% 里那个完整的安装器）。
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "wancode-updater-{version}-{nonce:08x}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建临时目录失败: {e}"))?;
    let path = dir.join(format!("wancode-{version}-installer.exe"));
    std::fs::write(&path, &bytes).map_err(|e| format!("写入安装器失败: {e}"))?;

    *state.0.lock().unwrap() = Some(StagedInstaller {
        version: version.clone(),
        path,
        sha256: sha256_hex(&bytes),
    });
    Ok(Some(version))
}

/// 以 breakaway 方式拉起已下载的安装器；确认存活后退出应用。
///
/// 成功路径**不返回**（app.exit）；一切失败路径返回 Err 留在应用内显示
/// ——这正是 #129 的验收要求：起不来必须可见。
#[tauri::command]
pub async fn updater_install(
    app: AppHandle,
    state: State<'_, PendingUpdate>,
    version: String,
) -> Result<(), String> {
    let staged = state
        .0
        .lock()
        .unwrap()
        .clone()
        .ok_or("没有已下载的更新——请先检查更新")?;
    if staged.version != version {
        return Err(format!(
            "版本不一致：前端请求 {version}，已下载 {}",
            staged.version
        ));
    }

    // 启动前把待执行文件重新哈希，与下载时验签通过的那份字节比对。
    let on_disk = std::fs::read(&staged.path).map_err(|e| format!("读取已下载安装器失败: {e}"))?;
    if sha256_hex(&on_disk) != staged.sha256 {
        return Err("已下载安装器与验签内容不一致（文件已被改动），已拒绝执行；请重新检查更新".into());
    }
    let path = staged.path;

    #[cfg(windows)]
    {
        // 完整复刻 tauri-plugin-updater 2.10.1 的 NSIS 参数：
        //   /P /R（passive 模式）+ /UPDATE + /ARGS <转义后的当前启动参数>
        // /ARGS 用于 /R 重启应用时恢复启动上下文，插件无条件附带。
        use std::ffi::OsString;
        let current_args: Vec<String> = std::env::args_os()
            .skip(1)
            .collect::<Vec<OsString>>()
            .iter()
            .map(|a| crate::updater_launch::win::escape_nsis_current_exe_arg(a))
            .collect();
        let mut args: Vec<&str> = vec!["/P", "/R", "/UPDATE", "/ARGS"];
        args.extend(current_args.iter().map(String::as_str));

        let pid = crate::updater_launch::win::spawn_breakaway_verified(&path, &args, 1200)
            .map_err(|e| format!("安装器启动失败（应用未退出，可重试或手动安装）: {e}"))?;
        tracing::info!(pid, path = %path.display(), "updater: installer launched outside job, exiting app");
        app.exit(0);
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        let _ = app;
        // 当前只发行 Windows 安装包；若未来出 macOS/Linux 包，这里应回落
        // 插件默认 install()（那些平台没有 Job 问题），而不是报错。
        Err("in-app update install is Windows-only in this build".into())
    }
}
