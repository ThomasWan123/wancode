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

    write("SMOKE BEGIN");
    let state: State<'_, AgentState> = app.state();

    // W2.5：`WANCODE_AUTOTEST_ONLY=work` 只跑 Work 全流程（S7）。S7 不需要
    // 任何模型回包，因此这条路径**不消耗 API 调用**，可反复重跑;完整套件
    // 的 S1–S6 仍打真实模型，保持原样。
    let only_work = std::env::var("WANCODE_AUTOTEST_ONLY")
        .map(|v| v.eq_ignore_ascii_case("work"))
        .unwrap_or(false);
    // ── S7 Work 全流程（W2.5 / codex #47 ①②③④⑦）─────────────────
    // 这一段走的是**生产入口** start_inner_with_intent(..., Work)，不是把
    // 副作用手工摆出来：新建 → 持久 binding 读回 → 经 work_import 命令边界
    // 导入 → 带对立意图恢复 → 被拒启动不留 handle/binding。
    write("SMOKE S7-work BEGIN");
    {
        use crate::surface_policy::NewSurfaceIntent;
        use crate::work_staging::{manifest_path_under, workspace_dir_under, WorkManifest, WorkspaceId};
        use tauri::Manager;

        let app_data = app.path().app_data_dir().expect("app_data_dir");
        // ① 新建 Work 会话：workspace 参数**故意传用户项目目录**，用于证明
        // Work 不会用它当 cwd（Work 必须落在自己的暂存目录）。
        let started = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            crate::agent::start_inner_with_intent(
                app.clone(),
                &state,
                workspace.clone(),
                None,
                None,
                NewSurfaceIntent::Work,
            ),
        )
        .await;
        let r = match started {
            Ok(Ok(r)) => r,
            other => {
                let why = match other {
                    Ok(Err(e)) => format!("{e:#}"),
                    Err(_) => "timed out after 120 seconds".to_string(),
                    Ok(Ok(_)) => unreachable!(),
                };
                check!("S7-work-start", false, why);
                write(&format!("SMOKE DONE pass={pass} fail={fail}"));
                std::process::exit(1);
            }
        };
        let ws_id = r.workspace_id.clone().unwrap_or_default();
        let kind_ok = r.surface_kind == crate::surface::SurfaceKind::Work;
        let id_ok = WorkspaceId::parse(ws_id.clone()).is_ok();
        let expect_cwd = workspace_dir_under(
            app_data.clone(),
            &WorkspaceId::parse(ws_id.clone()).unwrap_or_else(|_| WorkspaceId::mint()),
        );
        // cwd 必须是暂存目录，且**不是**传进去的用户项目目录。
        let cwd_ok = std::path::Path::new(&r.cwd) == expect_cwd.as_path()
            && std::path::Path::new(&r.cwd) != std::path::Path::new(&workspace);
        check!(
            "S7-work-start",
            kind_ok && id_ok && cwd_ok,
            format!("kind={:?} ws={ws_id} cwd={} expect={}", r.surface_kind, r.cwd, expect_cwd.display())
        );

        // ② 持久 binding 读回：盘上的身份必须与返回值一致。
        let surface = app.state::<crate::surface_gate::SurfaceState>();
        let bound = surface.resolve(&r.session_id);
        let binding_ok = bound
            .as_ref()
            .map(|b| {
                b.session_id == r.session_id
                    && b.surface_kind == crate::surface::SurfaceKind::Work
                    && b.workspace_id.as_ref().map(|w| w.as_str()) == Some(ws_id.as_str())
            })
            .unwrap_or(false);
        check!("S7-work-binding", binding_ok, format!("{bound:?}"));

        // ③ 经 work_import 命令边界导入（用**从 binding 读回**的 id）。
        let src = std::path::Path::new(&workspace).join("w25-fixture.pdf");
        let _ = std::fs::write(&src, b"%PDF-1.7 w2.5 autotest fixture");
        let ws_from_binding = bound.ok().and_then(|b| b.workspace_id).map(|w| w.as_str().to_string()).unwrap_or_default();
        let imported = crate::work_import::work_import(
            app.clone(),
            ws_from_binding.clone(),
            src.to_string_lossy().into_owned(),
        );
        let import_ok = match &imported {
            Ok(rec) => {
                let staged = workspace_dir_under(app_data.clone(), &WorkspaceId::parse(ws_from_binding.clone()).unwrap())
                    .join(rec.import_id.as_str())
                    .join("original.pdf");
                let staged_ro = std::fs::metadata(&staged).map(|m| m.permissions().readonly()).unwrap_or(false);
                let src_intact = std::fs::read(&src).map(|b| b == b"%PDF-1.7 w2.5 autotest fixture").unwrap_or(false);
                let in_manifest = WorkManifest::read(&manifest_path_under(
                    app_data.clone(),
                    &WorkspaceId::parse(ws_from_binding.clone()).unwrap(),
                ))
                .map(|m| m.imports.iter().any(|i| i.import_id == rec.import_id))
                .unwrap_or(false);
                staged_ro && src_intact && in_manifest
            }
            Err(_) => false,
        };
        check!("S7-work-import", import_ok, format!("{imported:?}"));

        // ④ 带**对立意图**恢复：binding 必须权威（层与工作区都不变），
        // 且不铸第二个工作区。
        let resumed = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            crate::agent::start_inner_with_intent(
                app.clone(),
                &state,
                workspace.clone(),
                None,
                Some(r.session_id.clone()),
                NewSurfaceIntent::Code, // 故意对立
            ),
        )
        .await;
        let resume_ok = match &resumed {
            Ok(Ok(rr)) => {
                rr.session_id == r.session_id
                    && rr.surface_kind == crate::surface::SurfaceKind::Work
                    && rr.workspace_id.as_deref() == Some(ws_id.as_str())
                    && rr.cwd == r.cwd
            }
            _ => false,
        };
        let resume_detail = match &resumed {
            Ok(Ok(rr)) => format!(
                "kind={:?} ws={:?} cwd={}",
                rr.surface_kind, rr.workspace_id, rr.cwd
            ),
            Ok(Err(e)) => format!("err={e:#}"),
            Err(_) => "timed out".to_string(),
        };
        check!("S7-work-resume-opposing", resume_ok, resume_detail);

        // ⑦ 被拒启动不留残留：给一个新会话 id 预置 **Cowork** binding
        // （Cowork 端到端未打通，surface_launchable 会在发布 handle 前拒绝），
        // 然后恢复它——必须失败，且 handle 不被替换成它。
        let blocked_sid = format!("{}-cowork-blocked", r.session_id);
        let pre = surface.gate().store().write(&crate::surface::SurfaceBinding::new(
            &blocked_sid,
            crate::surface::SurfaceKind::Cowork,
        ));
        let before_handle = {
            let g = state.handle.lock().await;
            g.as_ref().map(|h| h.session_id.0.to_string())
        };
        let blocked = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            crate::agent::start_inner_with_intent(
                app.clone(),
                &state,
                workspace.clone(),
                None,
                Some(blocked_sid.clone()),
                NewSurfaceIntent::Code,
            ),
        )
        .await;
        let after_handle = {
            let g = state.handle.lock().await;
            g.as_ref().map(|h| h.session_id.0.to_string())
        };
        let rejected = matches!(blocked, Ok(Err(_)));
        // 该守的属性是「**不给被拒的层发布 handle**」。注意 handle 变成 None
        // 是既有的正确语义、不是回归：start_inner 先拆旧会话再过门（源码注释
        // 「失败宁可『会话未启动』」，防僵尸 handle），因此**任何**失败启动都
        // 会让当前会话归零——首次实测正是被这条纠正了断言（原来错误地要求
        // 旧 handle 原样保留）。
        let no_handle_for_blocked = after_handle.as_deref() != Some(blocked_sid.as_str());
        // 也不得为被拒会话铸出新的 Work 工作区目录（预置的是 Cowork binding，
        // 本就无 workspace_id；若 work/ 下出现第二个目录即说明走过铸造路径）。
        let ws_root = crate::work_staging::work_root_under(app_data.clone());
        let stray_ws = std::fs::read_dir(&ws_root)
            .map(|it| {
                it.filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .any(|n| n != ws_id)
            })
            .unwrap_or(false);
        check!(
            "S7-work-rejected-start-clean",
            pre.is_ok() && rejected && no_handle_for_blocked && !stray_ws,
            format!(
                "rejected={rejected} no_handle_for_blocked={no_handle_for_blocked} stray_ws={stray_ws} handle {before_handle:?}->{after_handle:?}"
            )
        );
    }


    if only_work {
        write(&format!("SMOKE DONE pass={pass} fail={fail}"));
        std::process::exit(if fail > 0 { 1 } else { 0 });
    }

    // ── S1 会话启动（默认模型）──────────────────────────────────────
    write("SMOKE S1-start BEGIN");
    let started = match tokio::time::timeout(
        std::time::Duration::from_secs(120),
        start_inner(app.clone(), &state, workspace.clone(), None, None),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            check!("S1-start", false, "timed out after 120 seconds");
            write(&format!("SMOKE DONE pass={pass} fail={fail}"));
            std::process::exit(1);
        }
    };
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
    let resumed = start_inner(app.clone(), &state, workspace.clone(), None, Some(sid.clone())).await;
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
