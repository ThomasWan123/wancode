//! #129：更新的下载与安装命令——安装那一跳不再交给插件。
//!
//! 插件仍负责 check + 验签下载（这部分 2026-07-30 实证是好的：落盘文件
//! 与官方 sha256 逐字节一致）；被替换的只有最后一跳：插件的 install() 用
//! ShellExecuteW（无法 breakaway）且 exit(0) 前不检查启动结果。这里改为
//! `spawn_breakaway_verified`：显式脱离 Job、确认存活、失败留在应用内报错。

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use semver::Version;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_updater::{Update, UpdaterExt};

const SOURCE_TIMEOUT: Duration = Duration::from_secs(20);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Copy)]
struct UpdateSource {
    name: &'static str,
    endpoint: &'static str,
}

#[cfg(not(feature = "updater-test-endpoint"))]
const UPDATE_SOURCES: &[UpdateSource] = &[
    UpdateSource {
        name: "origin",
        endpoint: "https://github.com/ThomasWan123/wancode/releases/latest/download/latest.json",
    },
    UpdateSource {
        name: "gh-proxy",
        endpoint: "https://gh-proxy.com/https://github.com/ThomasWan123/wancode/releases/latest/download/latest-gh-proxy.json",
    },
];

#[cfg(feature = "updater-test-endpoint")]
const UPDATE_SOURCES: &[UpdateSource] = &[UpdateSource {
    name: "release-test",
    endpoint:
        "https://github.com/ThomasWan123/wancode/releases/download/v0.18.8-rc.1/latest-test.json",
}];

struct Candidate {
    source: UpdateSource,
    version: Version,
    update: Update,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceFailure {
    source: &'static str,
    stage: &'static str,
    detail: String,
}

fn failure_message(prefix: &str, failures: &[SourceFailure]) -> String {
    let ledger = serde_json::to_string(failures).unwrap_or_else(|_| "[]".to_string());
    format!("{prefix}；逐源记录={ledger}")
}

fn highest_version_indexes(versions: &[Version]) -> Vec<usize> {
    let Some(highest) = versions.iter().max() else {
        return Vec::new();
    };
    versions
        .iter()
        .enumerate()
        .filter_map(|(index, version)| (version == highest).then_some(index))
        .collect()
}

enum DownloadAttempt {
    Verified(Vec<u8>),
    Empty,
    VerifyError(String),
    Timeout,
}

/// Apply one download result to the same ledger used by the command. Keeping
/// this decision outside the transport future makes every hostile outcome
/// deterministic to test without weakening plugin-owned minisign verification.
fn accept_download_attempt(
    source: UpdateSource,
    attempt: DownloadAttempt,
    failures: &mut Vec<SourceFailure>,
) -> Option<Vec<u8>> {
    match attempt {
        DownloadAttempt::Verified(bytes) if !bytes.is_empty() => Some(bytes),
        DownloadAttempt::Verified(_) | DownloadAttempt::Empty => {
            failures.push(SourceFailure {
                source: source.name,
                stage: "download-empty",
                detail: "下载结果为 0 字节".to_string(),
            });
            None
        }
        DownloadAttempt::VerifyError(detail) => {
            failures.push(SourceFailure {
                source: source.name,
                stage: "download-verify",
                detail,
            });
            None
        }
        DownloadAttempt::Timeout => {
            failures.push(SourceFailure {
                source: source.name,
                stage: "download-timeout",
                detail: format!("超过 {} 秒", DOWNLOAD_TIMEOUT.as_secs()),
            });
            None
        }
    }
}

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
    // 新一轮检查开始即作废旧暂存，避免失败后仍可用旧 version 调安装命令。
    *state.0.lock().unwrap() = None;

    let mut candidates = Vec::new();
    let mut failures = Vec::new();
    let mut successful_checks = 0usize;
    for source in UPDATE_SOURCES {
        let endpoint = match tauri::Url::parse(source.endpoint) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                failures.push(SourceFailure {
                    source: source.name,
                    stage: "manifest-url",
                    detail: error.to_string(),
                });
                continue;
            }
        };
        let builder = match app
            .updater_builder()
            .endpoints(vec![endpoint])
            .map(|builder| builder.timeout(SOURCE_TIMEOUT))
        {
            Ok(builder) => builder,
            Err(error) => {
                failures.push(SourceFailure {
                    source: source.name,
                    stage: "manifest-config",
                    detail: error.to_string(),
                });
                continue;
            }
        };
        let updater = match builder.build() {
            Ok(updater) => updater,
            Err(error) => {
                failures.push(SourceFailure {
                    source: source.name,
                    stage: "manifest-config",
                    detail: error.to_string(),
                });
                continue;
            }
        };
        let checked =
            tokio::time::timeout(SOURCE_TIMEOUT + Duration::from_secs(2), updater.check()).await;
        match checked {
            Ok(Ok(Some(update))) => match Version::parse(&update.version) {
                Ok(version) => {
                    successful_checks += 1;
                    candidates.push(Candidate {
                        source: *source,
                        version,
                        update,
                    });
                }
                Err(error) => failures.push(SourceFailure {
                    source: source.name,
                    stage: "manifest-version",
                    detail: error.to_string(),
                }),
            },
            Ok(Ok(None)) => successful_checks += 1,
            Ok(Err(error)) => failures.push(SourceFailure {
                source: source.name,
                stage: "manifest-check",
                detail: error.to_string(),
            }),
            Err(_) => failures.push(SourceFailure {
                source: source.name,
                stage: "manifest-timeout",
                detail: format!("超过 {} 秒", SOURCE_TIMEOUT.as_secs()),
            }),
        }
    }

    if candidates.is_empty() {
        return if successful_checks > 0 {
            Ok(None)
        } else {
            Err(failure_message("所有更新清单源均不可用", &failures))
        };
    }

    // 只尝试最高版本组；最高版本下载失败时绝不静默退回旧版本。
    let versions: Vec<Version> = candidates
        .iter()
        .map(|candidate| candidate.version.clone())
        .collect();
    let selected = highest_version_indexes(&versions);
    let target_version = candidates[selected[0]].update.version.clone();
    let mut verified = None;
    for index in selected {
        let candidate = &candidates[index];
        let progress_app = app.clone();
        let version_for_progress = target_version.clone();
        let source_for_progress = candidate.source.name;
        let mut received: u64 = 0;
        let download = candidate.update.download(
            move |chunk, total| {
                received += chunk as u64;
                let pct: i32 = match total {
                    Some(total) if total > 0 => ((received * 100) / total).min(100) as i32,
                    _ => -1,
                };
                let _ = progress_app.emit(
                    "updater://progress",
                    serde_json::json!({
                        "version": version_for_progress,
                        "pct": pct,
                        "source": source_for_progress,
                    }),
                );
            },
            || {},
        );
        let attempt = match tokio::time::timeout(DOWNLOAD_TIMEOUT, download).await {
            Ok(Ok(bytes)) if bytes.is_empty() => DownloadAttempt::Empty,
            Ok(Ok(bytes)) => DownloadAttempt::Verified(bytes),
            Ok(Err(error)) => DownloadAttempt::VerifyError(error.to_string()),
            Err(_) => DownloadAttempt::Timeout,
        };
        if let Some(bytes) = accept_download_attempt(candidate.source, attempt, &mut failures) {
            verified = Some(bytes);
            break;
        }
    }
    let bytes = verified.ok_or_else(|| {
        failure_message(
            &format!("v{target_version} 的所有候选源均下载或验签失败（未降级）"),
            &failures,
        )
    })?;
    let version = target_version;

    // 落盘到真随机后缀的临时目录（tempfile 的 CSPRNG 命名 + 独占创建），
    // keep() 解除自动删除——目录要活到 install，且事后可取证（这次破案
    // 就靠 %TEMP% 里那个完整的安装器）。
    let dir = tempfile::Builder::new()
        .prefix(&format!("wancode-updater-{version}-"))
        .tempdir()
        .map_err(|e| format!("创建临时目录失败: {e}"))?
        .keep();
    let path = dir.join(format!("wancode-{version}-installer.exe"));
    std::fs::write(&path, &bytes).map_err(|e| format!("写入安装器失败: {e}"))?;

    *state.0.lock().unwrap() = Some(StagedInstaller {
        version: version.clone(),
        path,
        sha256: sha256_hex(&bytes),
    });
    Ok(Some(version))
}

#[cfg(test)]
mod mirror_tests {
    use super::*;

    const ORIGIN: UpdateSource = UpdateSource {
        name: "origin",
        endpoint: "https://origin.invalid/latest.json",
    };
    const MIRROR: UpdateSource = UpdateSource {
        name: "mirror",
        endpoint: "https://mirror.invalid/latest.json",
    };

    #[test]
    fn highest_version_group_preserves_source_priority() {
        let versions = [
            Version::parse("0.20.0").unwrap(),
            Version::parse("0.20.0").unwrap(),
            Version::parse("0.19.9").unwrap(),
        ];
        assert_eq!(highest_version_indexes(&versions), vec![0, 1]);
    }

    #[test]
    fn highest_version_failure_cannot_select_a_lower_version() {
        let versions = [
            Version::parse("0.20.1").unwrap(),
            Version::parse("0.20.0").unwrap(),
            Version::parse("0.20.0").unwrap(),
        ];
        assert_eq!(highest_version_indexes(&versions), vec![0]);
    }

    #[test]
    fn empty_candidate_set_is_explicit() {
        assert!(highest_version_indexes(&[]).is_empty());
    }

    #[test]
    fn positive_origin_control_accepts_verified_bytes_once() {
        let mut failures = Vec::new();
        let accepted = accept_download_attempt(
            ORIGIN,
            DownloadAttempt::Verified(b"MZ-origin".to_vec()),
            &mut failures,
        );
        assert_eq!(accepted.as_deref(), Some(b"MZ-origin".as_slice()));
        assert!(failures.is_empty());
    }

    #[test]
    fn positive_mirror_control_activates_after_origin_failure() {
        let mut failures = Vec::new();
        assert!(accept_download_attempt(
            ORIGIN,
            DownloadAttempt::VerifyError("origin offline".into()),
            &mut failures,
        )
        .is_none());
        let accepted = accept_download_attempt(
            MIRROR,
            DownloadAttempt::Verified(b"MZ-mirror".to_vec()),
            &mut failures,
        );
        assert_eq!(accepted.as_deref(), Some(b"MZ-mirror".as_slice()));
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].source, "origin");
    }

    #[test]
    fn zero_byte_is_ledgered_and_never_accepted() {
        let mut failures = Vec::new();
        assert!(accept_download_attempt(MIRROR, DownloadAttempt::Empty, &mut failures).is_none());
        assert_eq!(failures[0].stage, "download-empty");
    }

    #[test]
    fn defensive_empty_verified_variant_is_still_rejected() {
        let mut failures = Vec::new();
        assert!(accept_download_attempt(
            MIRROR,
            DownloadAttempt::Verified(Vec::new()),
            &mut failures,
        )
        .is_none());
        assert_eq!(failures[0].stage, "download-empty");
    }

    #[test]
    fn truncated_or_corrupt_bytes_are_plugin_verification_failures() {
        for detail in ["truncated minisign payload", "signature mismatch"] {
            let mut failures = Vec::new();
            assert!(accept_download_attempt(
                MIRROR,
                DownloadAttempt::VerifyError(detail.into()),
                &mut failures,
            )
            .is_none());
            assert_eq!(failures[0].stage, "download-verify");
            assert_eq!(failures[0].detail, detail);
        }
    }

    #[test]
    fn timeout_is_bounded_and_ledgered() {
        let mut failures = Vec::new();
        assert!(accept_download_attempt(MIRROR, DownloadAttempt::Timeout, &mut failures).is_none());
        assert_eq!(failures[0].stage, "download-timeout");
        assert!(failures[0]
            .detail
            .contains(&DOWNLOAD_TIMEOUT.as_secs().to_string()));
    }

    #[test]
    fn stale_manifest_is_excluded_from_the_highest_group() {
        let versions = [
            Version::parse("0.19.9").unwrap(),
            Version::parse("0.20.0").unwrap(),
        ];
        assert_eq!(highest_version_indexes(&versions), vec![1]);
    }

    #[test]
    fn all_sources_failed_produces_complete_structured_ledger() {
        let mut failures = Vec::new();
        assert!(accept_download_attempt(ORIGIN, DownloadAttempt::Timeout, &mut failures).is_none());
        assert!(accept_download_attempt(MIRROR, DownloadAttempt::Empty, &mut failures).is_none());
        let message = failure_message("all failed", &failures);
        assert!(message.contains("origin"));
        assert!(message.contains("mirror"));
        assert!(message.contains("download-timeout"));
        assert!(message.contains("download-empty"));
    }

    #[test]
    fn forged_higher_version_cannot_fall_back_to_valid_lower_bytes() {
        let versions = [
            Version::parse("99.0.0").unwrap(),
            Version::parse("0.20.0").unwrap(),
        ];
        let selected = highest_version_indexes(&versions);
        assert_eq!(selected, vec![0]);

        let mut failures = Vec::new();
        assert!(accept_download_attempt(
            MIRROR,
            DownloadAttempt::VerifyError("signature mismatch".into()),
            &mut failures,
        )
        .is_none());
        assert_eq!(failures.len(), 1, "lower version must never be attempted");
    }

    #[test]
    fn same_version_retry_stops_after_first_verified_source() {
        let versions = [
            Version::parse("0.20.0").unwrap(),
            Version::parse("0.20.0").unwrap(),
        ];
        assert_eq!(highest_version_indexes(&versions), vec![0, 1]);
        let mut failures = Vec::new();
        let accepted = accept_download_attempt(
            ORIGIN,
            DownloadAttempt::Verified(b"MZ".to_vec()),
            &mut failures,
        );
        assert!(accepted.is_some());
        assert!(
            failures.is_empty(),
            "mirror must not be touched after success"
        );
    }
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
        return Err(
            "已下载安装器与验签内容不一致（文件已被改动），已拒绝执行；请重新检查更新".into(),
        );
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
