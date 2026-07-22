//! v0.18-3 步 B：git / worktree 命令族（含 PR 闭环与安全网）。
//!
//! 红线（原样保留于各函数注释）：
//! - 所有 x.ai/git/*（worktree 除外）经 ext_call 注入显式 gitRoot（#83）；
//! - worktree apply 一律 merge 模式；
//! - session_git_root 客户端 git2 解析，绝不触发引擎回退。
use std::path::PathBuf;

use tauri::State;

use crate::agent::{ext_call, ext_ok, AgentState};

// ── P2.5 Git 补全 ───────────────────────────────────────────────────

#[tauri::command]
pub async fn git_stash(
    state: State<'_, AgentState>,
    message: Option<String>,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/git/stash", serde_json::json!({ "message": message })).await
}

#[tauri::command]
pub async fn git_info(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/git/info", serde_json::json!({})).await
}

#[tauri::command]
pub async fn git_current_commit(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/git/current_commit", serde_json::json!({})).await
}

#[tauri::command]
pub async fn git_files(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/git/files", serde_json::json!({})).await
}

#[tauri::command]
pub async fn git_repo_root(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/git/git_repo_root", serde_json::json!({})).await
}

#[tauri::command]
pub async fn git_serialize_changes(
    state: State<'_, AgentState>,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/git/serialize_changes", serde_json::json!({})).await
}

#[tauri::command]
pub async fn git_checkout_commit(
    state: State<'_, AgentState>,
    commit: String,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/git/checkout_commit", serde_json::json!({ "commit": commit })).await
}

#[tauri::command]
pub async fn git_checkout_session_head(
    state: State<'_, AgentState>,
) -> Result<serde_json::Value, String> {
    ext_ok(&state, "x.ai/git/checkout_session_head", serde_json::json!({})).await
}


// ── Git worktree 并行 Agent ────────────────────────────────────────
//
// 用法是：把当前会话在一个独立 worktree 里再开一份，两边各跑各的、互不
// 干扰文件；试完了要么把改动合回主目录，要么整个丢掉。
//
// 注意 list 的请求结构是这一片里唯一不用 camelCase 的：`include_all` 是
// snake_case，且 `repo` 没有 serde(default)——**必须显式传，哪怕是 null**，
// 少了整个反序列化就失败。（官方 TUI 自己发的是 includeAll，那个过滤参数
// 在人家客户端里是静默失效的。）

/// Take the current session into a fresh worktree.
///
/// Returns the **forked** session id (not the one passed in) plus the worktree
/// path — the caller then opens that session at that path.
#[tauri::command]
pub async fn worktree_resume_session(
    state: State<'_, AgentState>,
    workspace: String,
) -> Result<serde_json::Value, String> {
    let source = {
        let guard = state.handle.lock().await;
        guard.as_ref().ok_or("会话未启动")?.session_id.0.to_string()
    };
    let v = ext_call(
        &state,
        "x.ai/git/worktree/resume_session",
        serde_json::json!({
            "sessionId": source,
            "sourceCwd": workspace,
            "copyMode": "dirty",
            "restoreCode": true,
        }),
    )
    .await?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    v.get("result")
        .cloned()
        .ok_or_else(|| format!("resume_session 未返回结果: {v}"))
}

/// List worktrees. See the casing note above — this one is snake_case.
#[tauri::command]
pub async fn worktree_list(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    let v = ext_call(
        &state,
        "x.ai/git/worktree/list",
        serde_json::json!({ "repo": null, "type": [], "include_all": false }),
    )
    .await?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    Ok(v.get("result").cloned().unwrap_or(v))
}

/// Merge a worktree's changes back into the main working directory.
///
/// The response is status-tagged: `{"status":"success", files}` or
/// `{"status":"conflicts", files, conflicts}`. Conflicts are surfaced to the
/// user verbatim — we deliberately do not attempt a three-way merge UI.
#[tauri::command]
pub async fn worktree_apply(
    state: State<'_, AgentState>,
    worktree_path: String,
) -> Result<serde_json::Value, String> {
    let source = {
        let guard = state.handle.lock().await;
        guard.as_ref().ok_or("会话未启动")?.session_id.0.to_string()
    };
    let v = ext_call(
        &state,
        "x.ai/git/worktree/apply",
        serde_json::json!({
            "sessionId": source,
            "worktreePath": worktree_path,
            // merge 而不是 overwrite：overwrite 是无条件把 worktree 的内容写进
            // 主目录，**从不报冲突**——用户在主目录里对同一文件的改动会被静默
            // 销毁。merge 只在主目录没动过时才应用，两边都改了就报冲突。
            "mode": "merge",
        }),
    )
    .await?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    Ok(v.get("result").cloned().unwrap_or(v))
}

/// v0.16：apply 冲突预检。引擎 merge 模式虽然会报冲突，但那是在改动已经
/// 部分落盘之后；预检在动手前就把"两边都改了同一文件"亮出来让用户决策。
/// 双侧都用客户端 git2 直查（#83 同款思路：不信引擎的 cwd 回退）。
#[tauri::command]
pub async fn worktree_precheck(
    state: State<'_, AgentState>,
    worktree_path: String,
) -> Result<serde_json::Value, String> {
    let cwd = {
        let guard = state.handle.lock().await;
        guard.as_ref().ok_or("会话未启动")?.cwd.clone()
    };
    tokio::task::spawn_blocking(move || {
        fn changed_paths(repo_path: &std::path::Path) -> Result<Vec<String>, String> {
            let repo = git2::Repository::discover(repo_path)
                .map_err(|e| format!("打不开仓库 {}: {e}", repo_path.display()))?;
            let mut opts = git2::StatusOptions::new();
            opts.include_untracked(true).recurse_untracked_dirs(true);
            let st = repo.statuses(Some(&mut opts)).map_err(|e| e.to_string())?;
            Ok(st
                .iter()
                .filter(|e| e.status() != git2::Status::CURRENT)
                .filter_map(|e| e.path().map(String::from))
                .collect())
        }
        let main_changed = changed_paths(&cwd)?;
        let wt_changed = changed_paths(std::path::Path::new(&worktree_path))?;
        let main_set: std::collections::HashSet<&str> =
            main_changed.iter().map(String::as_str).collect();
        let overlap: Vec<String> = wt_changed
            .iter()
            .filter(|p| main_set.contains(p.as_str()))
            .cloned()
            .collect();
        Ok(serde_json::json!({
            "mainChanged": main_changed.len(),
            "wtChanged": wt_changed.len(),
            "overlap": overlap,
        }))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// v0.16：删除前快照。把 worktree 的全部未提交改动（含未跟踪文件）导出为
/// unified patch 存 ~/.grok/wancode-wt-snapshots/，返回路径；树干净返回 null。
/// 有了它，force 删除才谈得上"可反悔"。
#[tauri::command]
pub async fn worktree_snapshot(worktree_path: String) -> Result<serde_json::Value, String> {
    tokio::task::spawn_blocking(move || {
        let repo = git2::Repository::open(&worktree_path)
            .map_err(|e| format!("打不开 worktree: {e}"))?;
        let head_tree = repo
            .head()
            .ok()
            .and_then(|h| h.peel_to_tree().ok());
        let mut opts = git2::DiffOptions::new();
        opts.include_untracked(true)
            .recurse_untracked_dirs(true)
            .show_untracked_content(true);
        let diff = repo
            .diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))
            .map_err(|e| e.to_string())?;
        let mut patch = String::new();
        diff.print(git2::DiffFormat::Patch, |_, _, line| {
            let prefix = match line.origin() {
                '+' | '-' | ' ' => Some(line.origin()),
                _ => None,
            };
            if let Some(p) = prefix {
                patch.push(p);
            }
            patch.push_str(&String::from_utf8_lossy(line.content()));
            true
        })
        .map_err(|e| e.to_string())?;
        if patch.trim().is_empty() {
            return Ok(serde_json::Value::Null);
        }
        let dir = xai_grok_shell::util::grok_home::grok_home().join("wancode-wt-snapshots");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let base = std::path::Path::new(&worktree_path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "worktree".into());
        let file = dir.join(format!("{base}-{ts}.patch"));
        let header = format!(
            "# WanCode worktree snapshot\n# source: {worktree_path}\n# 恢复：git apply <本文件>\n\n"
        );
        std::fs::write(&file, header + &patch).map_err(|e| e.to_string())?;
        Ok(serde_json::json!({ "path": file.to_string_lossy() }))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Remove a worktree. `idOrPath` and the legacy `worktreePath` are mutually
/// exclusive — sending both is an error, so only send one.
#[tauri::command]
pub async fn worktree_remove(
    state: State<'_, AgentState>,
    id_or_path: String,
    force: bool,
) -> Result<serde_json::Value, String> {
    let v = ext_call(
        &state,
        "x.ai/git/worktree/remove",
        serde_json::json!({ "idOrPath": id_or_path, "force": force, "dryRun": false }),
    )
    .await?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    Ok(v.get("result").cloned().unwrap_or(v))
}

/// Resolve the session workspace's git root LOCALLY (git2 discover).
///
/// #83 的根修：引擎的 git/* 在「会话目录不是仓库」时把 resolve 失败
/// `.ok()` 吞成 None，随后 git_op_cwd 回退到 workspace-hub 根——嵌入式
/// 场景那是本应用自己的仓库，于是「不是仓库」被静默替换成「另一个仓库
/// 的状态」，stash/丢弃会打错目标。gitRoot 一律客户端解析、显式传入；
/// 解析不出就本地拒绝，引擎的回退路径永远不被触发。
pub(crate) async fn session_git_root(state: &State<'_, AgentState>) -> Result<Option<String>, String> {
    let cwd = state
        .handle
        .lock()
        .await
        .as_ref()
        .map(|h| h.cwd.clone())
        .ok_or("会话未启动")?;
    Ok(git2::Repository::discover(&cwd)
        .ok()
        .and_then(|r| r.workdir().map(|p| p.to_string_lossy().into_owned())))
}

#[tauri::command]
pub async fn git_status_ext(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    let Some(root) = session_git_root(&state).await? else {
        // 明确的「不是仓库」标记；前端渲染为 非 git 仓库，绝不显示别的仓库
        return Ok(serde_json::json!({ "result": { "data": null } }));
    };
    ext_call(
        &state,
        "x.ai/git/status",
        serde_json::json!({ "gitRoot": root, "includeUntracked": true, "includeStats": true }),
    )
    .await
}

/// Unified diffs for the given paths (empty = every change).
/// `include_patch` 打开时返回每个文件的 unified diff 文本（工作台 Diff 视图用）。
#[tauri::command]
pub async fn git_diffs(
    state: State<'_, AgentState>,
    paths: Option<Vec<String>>,
    include_patch: Option<bool>,
) -> Result<serde_json::Value, String> {
    ext_call(
        &state,
        "x.ai/git/diffs",
        // 注意：不传 maxPatchBytes/maxPatchLines——引擎的语义是"任一文件
        // 超限则整个请求失败"（check_diff_size_limits），一把 lock 文件就能
        // 把整个 Diff 面板打死。超大 patch 由前端截断显示。
        serde_json::json!({
            "paths": paths,
            "includePatch": include_patch.unwrap_or(false),
        }),
    )
    .await
}

/// Stage / unstage / discard the given paths (`None` = all).
#[tauri::command]
pub async fn git_stage(
    state: State<'_, AgentState>,
    paths: Option<Vec<String>>,
) -> Result<serde_json::Value, String> {
    ext_call(&state, "x.ai/git/stage", serde_json::json!({ "paths": paths })).await
}

#[tauri::command]
pub async fn git_unstage(
    state: State<'_, AgentState>,
    paths: Option<Vec<String>>,
) -> Result<serde_json::Value, String> {
    ext_call(&state, "x.ai/git/unstage", serde_json::json!({ "paths": paths })).await
}

#[tauri::command]
pub async fn git_discard(
    state: State<'_, AgentState>,
    paths: Option<Vec<String>>,
) -> Result<serde_json::Value, String> {
    ext_call(&state, "x.ai/git/discard", serde_json::json!({ "paths": paths })).await
}

/// Commit what is currently staged.
#[tauri::command]
pub async fn git_commit(
    state: State<'_, AgentState>,
    message: String,
    amend: Option<bool>,
) -> Result<serde_json::Value, String> {
    ext_call(
        &state,
        "x.ai/git/commit",
        serde_json::json!({ "message": message, "amend": amend.unwrap_or(false) }),
    )
    .await
}

/// Branch list, and checkout.
#[tauri::command]
pub async fn git_branches(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    ext_call(&state, "x.ai/git/branches", serde_json::json!({})).await
}

#[tauri::command]
pub async fn git_checkout(
    state: State<'_, AgentState>,
    branch: String,
) -> Result<serde_json::Value, String> {
    ext_call(&state, "x.ai/git/checkout", serde_json::json!({ "branch": branch })).await
}

/// v0.15 PR 闭环：推送当前分支 + 本地 gh 创建 PR。
///
/// 引擎 git/* 没有 push，PR 能力走本地 git/gh CLI。Windows 下必须
/// CREATE_NO_WINDOW，否则每条命令闪一个控制台（老坑）。
#[tauri::command]
pub async fn git_create_pr(
    state: State<'_, AgentState>,
    title: String,
    body: Option<String>,
) -> Result<serde_json::Value, String> {
    let cwd = {
        let guard = state.handle.lock().await;
        guard.as_ref().ok_or("会话未启动")?.cwd.clone()
    };
    let run = move |program: &'static str, args: Vec<String>| {
        let cwd = cwd.clone();
        async move {
            tokio::task::spawn_blocking(move || {
                let mut cmd = std::process::Command::new(program);
                cmd.args(&args).current_dir(&cwd);
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
                }
                let out = cmd
                    .output()
                    .map_err(|e| format!("{program} 启动失败: {e}"))?;
                let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                if out.status.success() {
                    Ok(stdout)
                } else {
                    Err(format!(
                        "{program} {} 失败: {}",
                        args.first().map(String::as_str).unwrap_or(""),
                        if stderr.is_empty() { stdout } else { stderr }
                    ))
                }
            })
            .await
            .map_err(|e| e.to_string())?
        }
    };

    // gh 可用性（未安装/未登录都在这一步报清楚）
    run("gh", vec!["auth".into(), "status".into()])
        .await
        .map_err(|e| format!("GitHub CLI 不可用（需安装 gh 并 gh auth login）: {e}"))?;

    let branch = run("git", vec!["rev-parse".into(), "--abbrev-ref".into(), "HEAD".into()]).await?;
    if branch == "main" || branch == "master" {
        return Err(format!(
            "当前在 {branch} 分支。先让 AI 建一个特性分支并提交，再创建 PR。"
        ));
    }

    run(
        "git",
        vec!["push".into(), "-u".into(), "origin".into(), "HEAD".into()],
    )
    .await?;

    let mut args = vec![
        "pr".into(),
        "create".into(),
        "--title".into(),
        title,
        "--body".into(),
        body.unwrap_or_else(|| "由 WanCode 创建。".into()),
    ];
    args.push("--head".into());
    args.push(branch.clone());
    let url = run("gh", args).await?;
    Ok(serde_json::json!({ "url": url, "branch": branch }))
}

/// v0.15-5：当前分支的 PR 状态（gh pr view）。没有 PR / 没装 gh / 未登录
/// 都返回 null——这是"锦上添花"信息，任何失败都不该打扰用户。
#[tauri::command]
pub async fn git_pr_status(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    let cwd = {
        let guard = state.handle.lock().await;
        guard.as_ref().ok_or("会话未启动")?.cwd.clone()
    };
    let out = tokio::task::spawn_blocking(move || {
        let mut cmd = std::process::Command::new("gh");
        cmd.args([
            "pr",
            "view",
            "--json",
            "number,state,url,title,statusCheckRollup",
        ])
        .current_dir(&cwd);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        cmd.output()
    })
    .await
    .map_err(|e| e.to_string())?;
    let out = match out {
        Ok(o) if o.status.success() => o,
        _ => return Ok(serde_json::Value::Null),
    };
    let v: serde_json::Value = match serde_json::from_slice(&out.stdout) {
        Ok(v) => v,
        Err(_) => return Ok(serde_json::Value::Null),
    };
    // CI 汇总：statusCheckRollup 是 checks 数组，归并成 pass/fail/pending 计数
    let checks = v
        .get("statusCheckRollup")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();
    let mut pass = 0;
    let mut fail = 0;
    let mut pending = 0;
    for c in &checks {
        let concl = c
            .get("conclusion")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_ascii_uppercase();
        let status = c
            .get("status")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_ascii_uppercase();
        match concl.as_str() {
            "SUCCESS" | "NEUTRAL" | "SKIPPED" => pass += 1,
            "FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" => fail += 1,
            _ if status == "COMPLETED" => pass += 1,
            _ => pending += 1,
        }
    }
    Ok(serde_json::json!({
        "number": v.get("number"),
        "state": v.get("state"),
        "url": v.get("url"),
        "title": v.get("title"),
        "ci": { "pass": pass, "fail": fail, "pending": pending, "total": checks.len() },
    }))
}

#[cfg(test)]
mod worktree_safety_tests {
    /// v0.16 删除前快照：临时仓库 + 未提交改动 → 生成含 diff 的 patch 文件；
    /// 干净树 → 返回 null 不落文件。
    #[tokio::test]
    async fn snapshot_writes_patch_for_dirty_tree() {
        let dir = std::env::temp_dir().join(format!(
            "wancode-wtsnap-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let repo = git2::Repository::init(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "base\n").unwrap();
        let mut idx = repo.index().unwrap();
        idx.add_path(std::path::Path::new("a.txt")).unwrap();
        idx.write().unwrap();
        let tree = repo.find_tree(idx.write_tree().unwrap()).unwrap();
        let sig = git2::Signature::now("t", "t@t").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[]).unwrap();

        // 干净树 → null
        let clean = super::worktree_snapshot(dir.to_string_lossy().into_owned())
            .await
            .unwrap();
        assert!(clean.is_null(), "干净树不应产生快照: {clean:?}");

        // 改跟踪文件 + 加未跟踪文件 → 快照含两者
        std::fs::write(dir.join("a.txt"), "changed\n").unwrap();
        std::fs::write(dir.join("new.txt"), "brand new\n").unwrap();
        let snap = super::worktree_snapshot(dir.to_string_lossy().into_owned())
            .await
            .unwrap();
        let path = snap.get("path").and_then(|p| p.as_str()).expect("应返回 path");
        let content = std::fs::read_to_string(path).unwrap();
        assert!(content.contains("changed"), "patch 缺跟踪文件改动");
        assert!(content.contains("brand new"), "patch 缺未跟踪文件内容");
        assert!(content.contains("git apply"), "缺恢复说明头");
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
