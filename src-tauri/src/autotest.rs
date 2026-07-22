//! v0.18-4 步 B：WANCODE_AUTOTEST 无头 smoke 套件（v0.13-1）。
//! 断言全部落磁盘/git2 层；scripts/smoke.ps1 轮询日志取结果。
use tauri::{AppHandle, Manager, State};
use xai_acp_lib::acp_send;
use agent_client_protocol as acp;

use crate::agent::{ext_call, start_inner, AgentState};
use crate::git_ops::{git_stash, git_status_ext, session_git_root};

/// Self-test driven by `WANCODE_AUTOTEST=<workspace-dir>`: exercises the full
/// backend glue (start → prompt → events) without the UI and logs the result
/// Headless smoke suite (v0.13 refactor safety net).
///
/// `WANCODE_AUTOTEST=<fixture-dir>` 启动即运行：6 个场景全部走真实引擎，
/// 断言全部落在磁盘/git2 层（无 UI 依赖，坐标点击的维护成本教训）。
/// 结果写 %TEMP%/wancode-autotest.log，结尾一行 `SMOKE DONE pass=N fail=M`，
/// 随后进程自杀（scripts/smoke.ps1 轮询日志取结果）。
pub async fn autotest(app: AppHandle, workspace: String) {
    let log = std::env::temp_dir().join("wancode-autotest.log");
    let _ = std::fs::remove_file(&log);
    let write = |s: &str| {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&log) {
            let _ = writeln!(f, "{s}");
        }
    };
    let mut pass = 0u32;
    let mut fail = 0u32;
    macro_rules! check {
        ($name:expr, $ok:expr, $detail:expr) => {{
            let ok: bool = $ok;
            if ok { pass += 1 } else { fail += 1 }
            write(&format!("SMOKE {} {}: {}", $name, if ok { "PASS" } else { "FAIL" }, $detail));
        }};
    }

    let state: State<'_, AgentState> = app.state();

    // ── S1 会话启动（默认模型）──────────────────────────────────────
    let started = start_inner(app.clone(), &state, workspace.clone(), None, None).await;
    let (sid, cwd) = match &started {
        Ok(r) => {
            check!("S1-start", true, format!("session={}", r.session_id));
            (r.session_id.clone(), r.cwd.clone())
        }
        Err(e) => {
            check!("S1-start", false, format!("{e:#}"));
            write(&format!("SMOKE DONE pass={pass} fail={fail}"));
            std::process::exit(1);
        }
    };
    let sessions_base = xai_grok_shell::util::grok_home::grok_home().join("sessions");
    let chat_text = || -> String {
        walkdir_find(&sessions_base, &sid)
            .map(|d| d.join("chat_history.jsonl"))
            .and_then(|f| std::fs::read_to_string(f).ok())
            .unwrap_or_default()
    };
    let acp_tx = {
        let g = state.handle.lock().await;
        g.as_ref().unwrap().acp_tx.clone()
    };
    let send = |text: String| {
        let tx = acp_tx.clone();
        let sid = acp::SessionId::new(sid.clone());
        async move {
            let blocks = vec![acp::ContentBlock::Text(acp::TextContent::new(text))];
            {
                let r: Result<acp::PromptResponse, _> =
                    acp_send(acp::PromptRequest::new(sid, blocks), &tx).await;
                r
            }
        }
    };

    // ── S2 基本回复 ────────────────────────────────────────────────
    let r = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        send("reply with exactly: SMOKE-BASIC".into()),
    )
    .await;
    let detail = match &r {
        Err(_) => "timeout-120s".to_string(),
        Ok(Err(e)) => format!("err={e}"),
        Ok(Ok(resp)) => format!("stop={:?}", resp.stop_reason),
    };
    let ok = matches!(&r, Ok(Ok(_))) && chat_text().contains("SMOKE-BASIC");
    check!("S2-reply", ok, detail);

    // ── S3 忙时排队（长任务 + 两条排队，全部完成且顺序保留）────────
    let long = tauri::async_runtime::spawn(send("Run the command ping -n 8 127.0.0.1 once, then reply SMOKE-LONG".into()));
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let qa = tauri::async_runtime::spawn(send("reply with exactly: SMOKE-QA".into()));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let qb = tauri::async_runtime::spawn(send("reply with exactly: SMOKE-QB".into()));
    let _ = tokio::time::timeout(std::time::Duration::from_secs(180), long).await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(60), qa).await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(60), qb).await;
    let text = chat_text();
    let order_ok = match (text.find("SMOKE-QA"), text.find("SMOKE-QB")) {
        (Some(a), Some(b)) => a < b,
        _ => false,
    };
    check!(
        "S3-queue",
        text.contains("SMOKE-LONG") && order_ok,
        format!("long={} order={order_ok}", text.contains("SMOKE-LONG"))
    );

    // ── S4 回合中插话 ──────────────────────────────────────────────
    let long2 = tauri::async_runtime::spawn(send("Run the command ping -n 20 127.0.0.1 once, then reply SMOKE-D".into()));
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    let ij = ext_call(
        &state,
        "x.ai/interject",
        serde_json::json!({ "text": "Stop now. Reply with exactly: SMOKE-IJ" }),
    )
    .await;
    let _ = tokio::time::timeout(std::time::Duration::from_secs(180), long2).await;
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    let ok = ij.is_ok() && chat_text().contains("SMOKE-IJ");
    check!("S4-interject", ok, format!("call={}", ij.is_ok()));

    // ── S5 Git 状态 + 贮藏（git2 断言，不依赖 git CLI）────────────
    let fixture = (|| -> Result<(), String> {
        let repo = git2::Repository::init(&cwd).map_err(|e| e.to_string())?;
        let f = std::path::Path::new(&cwd).join("smoke.txt");
        std::fs::write(&f, "base").map_err(|e| e.to_string())?;
        let mut idx = repo.index().map_err(|e| e.to_string())?;
        idx.add_path(std::path::Path::new("smoke.txt")).map_err(|e| e.to_string())?;
        idx.write().map_err(|e| e.to_string())?;
        let tree_id = idx.write_tree().map_err(|e| e.to_string())?;
        let tree = repo.find_tree(tree_id).map_err(|e| e.to_string())?;
        let sig = git2::Signature::now("smoke", "smoke@t").map_err(|e| e.to_string())?;
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .map_err(|e| e.to_string())?;
        std::fs::write(&f, "changed").map_err(|e| e.to_string())?;
        Ok(())
    })();
    match fixture {
        Ok(()) => {
            // 事故防线（2026-07-21：一次 stash 打到了宿主仓库，未提交代码
            // 被回退）：先确认客户端解析的 gitRoot 就是 fixture，不是就
            // FAIL 并拒绝执行任何写操作。探针同时落日志供根因分析。
            let resolved = session_git_root(&state).await.ok().flatten();
            write(&format!("SMOKE S5 resolved gitRoot={resolved:?} fixture={cwd}"));
            let fixture_ok = resolved
                .as_deref()
                .map(|r| {
                    let norm = |x: &str| x.replace('/', "\\").trim_end_matches('\\').to_lowercase();
                    norm(r) == norm(&cwd)
                })
                .unwrap_or(false);
            if !fixture_ok {
                check!("S5-git-stash", false, format!("resolved root 不是 fixture：{resolved:?}——拒绝执行 stash"));
            } else {
            let st = git_status_ext(state.clone()).await;
            let has_change = st
                .as_ref()
                .ok()
                .and_then(|v| {
                    v.pointer("/result/unstaged")
                        .or_else(|| v.pointer("/result/data/unstaged"))
                })
                .and_then(|u| u.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(false);
            let stash = git_stash(state.clone(), None).await;
            let clean_after = git2::Repository::open(&cwd)
                .ok()
                .map(|mut r| {
                    let mut n = 0;
                    let _ = r.stash_foreach(|_, _, _| {
                        n += 1;
                        true
                    });
                    let dirty = r
                        .statuses(None)
                        .map(|s| {
                            s.iter().any(|e| {
                                let st = e.status();
                                st != git2::Status::CURRENT && st != git2::Status::WT_NEW
                            })
                        })
                        .unwrap_or(true);
                    n == 1 && !dirty
                })
                .unwrap_or(false);
            check!(
                "S5-git-stash",
                has_change && stash.is_ok() && clean_after,
                format!("change={has_change} stash={} clean={clean_after}", stash.is_ok())
            );
            }
        }
        Err(e) => check!("S5-git-stash", false, format!("fixture: {e}")),
    }

    // ── S6 会话恢复（同 id 续接，历史保留）────────────────────────
    let before_len = chat_text().lines().count();
    let resumed = start_inner(app.clone(), &state, workspace, None, Some(sid.clone())).await;
    let same_id = resumed.as_ref().map(|r| r.session_id == sid).unwrap_or(false);
    let after_len = chat_text().lines().count();
    check!(
        "S6-resume",
        same_id && after_len >= before_len,
        format!("same_id={same_id} lines {before_len}->{after_len}")
    );

    write(&format!("SMOKE DONE pass={pass} fail={fail}"));
    std::process::exit(if fail > 0 { 1 } else { 0 });
}

/// 在 sessions 目录下找包含指定会话 id 的目录（两层结构：cwd 编码/会话 id）。
pub(crate) fn walkdir_find(base: &std::path::Path, sid: &str) -> Option<std::path::PathBuf> {
    for cwd_dir in std::fs::read_dir(base).ok()?.flatten() {
        let cand = cwd_dir.path().join(sid);
        if cand.is_dir() {
            return Some(cand);
        }
    }
    None
}
