//! v0.18-4 步 B：WANCODE_AUTOTEST 无头 smoke 套件（v0.13-1）。
//! 断言全部落磁盘/git2 层；scripts/smoke.ps1 轮询日志取结果。
use tauri::{AppHandle, Manager, State};
use xai_acp_lib::acp_send;
use agent_client_protocol as acp;

use crate::agent::{ext_call, AgentState};
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
    // move 持有路径：闭包由此 'static，C1 逃逸探针分支要把它 Arc 进独立任务。
    let write = move |s: &str| {
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
    // C1-b：`WANCODE_AUTOTEST_ONLY=c1-escape` 只跑逃逸探针实跑（真模型回合，
    // 有 API 成本，手动触发；PASS/FAIL 语义见 cowork_escape_run.rs 文件头）。
    let only_c1 = std::env::var("WANCODE_AUTOTEST_ONLY")
        .map(|v| v.eq_ignore_ascii_case("c1-escape"))
        .unwrap_or(false);
    if only_c1 {
        write("SMOKE S8-c1-escape BEGIN");
        // Arc 化：逃逸探针的每个回合在独立任务里跑（断栈——嵌套 poll 在
        // debug 构建下压爆过 tokio worker 栈），spawn 要求 'static。
        // 本分支随后即 process::exit，write 的移动不影响其余套件。
        let write_arc: std::sync::Arc<dyn Fn(&str) + Send + Sync> = std::sync::Arc::new(write);
        let (p2, f2) = crate::cowork_escape_run::run(
            app.clone(),
            std::path::Path::new(&workspace),
            write_arc.clone(),
        )
        .await;
        pass += p2;
        fail += f2;
        write_arc.as_ref()(&format!("SMOKE DONE pass={pass} fail={fail}"));
        std::process::exit(if fail > 0 { 1 } else { 0 });
    }
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
        let started =
            spawn_start(app.clone(), workspace.clone(), None, NewSurfaceIntent::Work).await;
        let r = match started {
            Ok(r) => r,
            Err(e) => {
                let why = format!("{e:#}");
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
        // 夹具放在**调用方项目之外**的独立目录：既模拟「用户从任意位置选
        // 文件」，也让「Work 不碰调用方项目」这条能被真正断言（codex R2-F1：
        // 把夹具写进 workspace 会让该断言自相矛盾）。
        let user_docs = app_data.join("w25-user-docs");
        let _ = std::fs::create_dir_all(&user_docs);
        let src = user_docs.join("w25-fixture.pdf");
        let _ = std::fs::write(&src, b"%PDF-1.7 w2.5 autotest fixture");
        // 导入前快照：项目目录清单 + 原件字节/权限。
        let proj_before = dir_entry_names(std::path::Path::new(&workspace));
        let src_bytes_before = std::fs::read(&src).unwrap_or_default();
        let src_ro_before = std::fs::metadata(&src).map(|m| m.permissions().readonly()).unwrap_or(false);
        let expect_sha = {
            use sha2::{Digest, Sha256};
            let d = Sha256::digest(&src_bytes_before);
            d.iter().map(|b| format!("{b:02x}")).collect::<String>()
        };
        let ws_from_binding = bound.ok().and_then(|b| b.workspace_id).map(|w| w.as_str().to_string()).unwrap_or_default();
        let imported = crate::work_import::work_import(
            app.clone(),
            ws_from_binding.clone(),
            src.to_string_lossy().into_owned(),
        );
        let (import_ok, import_detail) = match &imported {
            Ok(rec) => {
                let ws_parsed = WorkspaceId::parse(ws_from_binding.clone()).expect("workspace id");
                let staged = workspace_dir_under(app_data.clone(), &ws_parsed)
                    .join(rec.import_id.as_str())
                    .join("original.pdf");
                let staged_bytes = std::fs::read(&staged).unwrap_or_default();
                let staged_ro = std::fs::metadata(&staged).map(|m| m.permissions().readonly()).unwrap_or(false);
                // 暂存字节必须与原件**逐字节相同**，记录的 sha256 必须与实算相同。
                let bytes_match = staged_bytes == src_bytes_before;
                let sha_match = rec.source_sha256 == expect_sha;
                // 原件字节与权限不得变。
                let src_intact = std::fs::read(&src).map(|b| b == src_bytes_before).unwrap_or(false)
                    && std::fs::metadata(&src).map(|m| m.permissions().readonly()).unwrap_or(!src_ro_before) == src_ro_before;
                // 清单必须**恰好**等于返回的那一条（不接受多出记录）。
                let manifest = WorkManifest::read(&manifest_path_under(app_data.clone(), &ws_parsed));
                let manifest_exact = manifest
                    .as_ref()
                    .map(|m| m.imports.len() == 1 && m.imports[0] == *rec)
                    .unwrap_or(false);
                // 调用方项目目录不得被改动。
                let proj_untouched = dir_entry_names(std::path::Path::new(&workspace)) == proj_before;
                (
                    staged_ro && bytes_match && sha_match && src_intact && manifest_exact && proj_untouched,
                    format!(
                        "staged_ro={staged_ro} bytes_match={bytes_match} sha_match={sha_match} \
                         src_intact={src_intact} manifest_exact={manifest_exact} proj_untouched={proj_untouched}"
                    ),
                )
            }
            Err(e) => (false, format!("err={e}")),
        };
        check!("S7-work-import", import_ok, import_detail);

        // ④ 带**对立意图**恢复：binding 必须权威（层与工作区都不变），
        // 且不铸第二个工作区。
        let resumed = spawn_start(
            app.clone(),
            workspace.clone(),
            Some(r.session_id.clone()),
            NewSurfaceIntent::Code, // 故意对立
        )
        .await;
        let resume_ok = match &resumed {
            Ok(rr) => {
                rr.session_id == r.session_id
                    && rr.surface_kind == crate::surface::SurfaceKind::Work
                    && rr.workspace_id.as_deref() == Some(ws_id.as_str())
                    && rr.cwd == r.cwd
            }
            _ => false,
        };
        let resume_detail = match &resumed {
            Ok(rr) => format!(
                "kind={:?} ws={:?} cwd={}",
                rr.surface_kind, rr.workspace_id, rr.cwd
            ),
            Err(e) => format!("err={e:#}"),
        };
        check!("S7-work-resume-opposing", resume_ok, resume_detail);

        // ⑥(生产路径) 在**活着的 Work 会话**上经生产 ext 边界查 MCP 清单。
        // 这条与 CI 探针不同：会话由 start_inner_with_intent 创建，因此
        // apply_work_agent_config_overrides / 生产请求构造都在链路里——档或
        // 覆盖回归会在这里暴露（codex R3-F2 指出探针不走生产构造）。
        let mcp_live = ext_call(&state, "x.ai/mcp/list", serde_json::json!({})).await;
        let mcp_empty = mcp_live
            .as_ref()
            .ok()
            .and_then(|v| v.get("result").and_then(|r| r.get("servers")).or_else(|| v.get("servers")))
            .and_then(|s| s.as_array())
            .map(|a| a.is_empty());
        check!(
            "S7-work-live-session-zero-mcp",
            mcp_empty == Some(true),
            format!("servers_empty={mcp_empty:?} raw={mcp_live:?}")
        );


        // ⑦ 被拒启动不留残留。**必须真的到达 launchability 门**（codex R2-F2：
        // 伪造一个不存在的 session id 会让 ACP LoadSession 先失败，根本没走到
        // 门，那种「拒绝」什么都证明不了）。做法：拿**上面那个已存在的真实
        // 会话**，把它的 sidecar 直接改写成 Cowork——引擎侧会话确实存在、
        // LoadSession 能成功，随后 surface_launchable(Cowork) 在**发布 handle
        // 之前**拒绝。失败原因一并锁死。
        // 关键（第二次实测纠正）：要真正到达 launchability 门，LoadSession
        // 必须先成功——而它按 cwd 定位会话。若拿 Work 会话（cwd = 暂存目录）
        // 改成 Cowork 再恢复，cwd 会变回项目目录，引擎直接 FS_NOT_FOUND，
        // 仍然到不了门。因此这里**在项目目录里新建一个 Code 会话**（cwd 与
        // 恢复时一致），再把它的 sidecar 翻成 Cowork。
        let code_started =
            spawn_start(app.clone(), workspace.clone(), None, NewSurfaceIntent::Code).await;
        let blocked_sid = match &code_started {
            Ok(cr) => cr.session_id.clone(),
            _ => String::new(),
        };
        let cowork_json = serde_json::json!({
            "binding_schema_version": crate::surface::CURRENT_BINDING_SCHEMA_VERSION,
            "session_id": blocked_sid,
            "surface_kind": "cowork",
            "created_policy_version": crate::surface::CURRENT_POLICY_VERSION,
        })
        .to_string();
        let sidecar = surface.gate().store().path_for(&blocked_sid);
        let pre = std::fs::write(&sidecar, cowork_json.as_bytes());
        let ws_dirs_before = dir_entry_names(&crate::work_staging::work_root_under(app_data.clone()));
        let blocked = spawn_start(
            app.clone(),
            workspace.clone(),
            Some(blocked_sid.clone()),
            NewSurfaceIntent::Code,
        )
        .await;
        let after_handle = {
            let g = state.handle.lock().await;
            g.as_ref().map(|h| h.session_id.0.to_string())
        };
        // 锁住失败**边界**：必须是 launchability 门拒绝（SURFACE_NOT_LAUNCHABLE），
        // 而不是 LoadSession 早失败或任何别的原因。
        let err_text = match &blocked {
            Err(e) => format!("{e:#}"),
            Ok(_) => "unexpected success".to_string(),
        };
        let rejected_at_gate = err_text.contains("SURFACE_NOT_LAUNCHABLE");
        // 后置条件：**没有任何 handle**（失败启动先拆旧会话、且绝不发布新
        // handle——首次实测由此纠正了我原先「旧 handle 应原样」的错误断言，
        // 源码注释：「失败宁可『会话未启动』」），且没有新增 Work 工作区目录。
        let no_handle = after_handle.is_none();
        let ws_dirs_after = dir_entry_names(&crate::work_staging::work_root_under(app_data.clone()));
        let no_new_ws = ws_dirs_after == ws_dirs_before;
        // ⑦b 新建 Work 启动在 **binding 写入之后** 失败时的后置状态
        //    （codex R3-F1：上面那条走的是 resumed 路径，只能证明「已存在身份
        //    被拒」，证明不了「新建失败不留 binding/workspace」）。
        //    确定性注入：在 $GROK_HOME/wancode-last-session.json 处建一个**目录**，
        //    使崩溃标记写入必失败——那正是「写 binding 之后、发布 handle 之前」
        //    的唯一确定性失败点（agent.rs：603 写 binding → 637 门 → 648 标记
        //    → 697 handle）。
        let marker_path = xai_grok_shell::util::grok_home::grok_home().join("wancode-last-session.json");
        let _ = std::fs::remove_file(&marker_path);
        let blocked_marker = std::fs::create_dir_all(&marker_path).is_ok();
        let bindings_before = dir_entry_names(surface.gate().store().root_dir());
        let ws_before2 = dir_entry_names(&crate::work_staging::work_root_under(app_data.clone()));
        let fresh_fail =
            spawn_start(app.clone(), workspace.clone(), None, NewSurfaceIntent::Work).await;
        let fresh_err = match &fresh_fail {
            Err(e) => format!("{e:#}"),
            Ok(_) => "unexpected success".to_string(),
        };
        let failed_at_marker = fresh_err.contains("CRASH_RECOVERY_MARKER_FAILED");
        let handle_after_fresh_fail = {
            let g = state.handle.lock().await;
            g.as_ref().map(|h| h.session_id.0.to_string())
        };
        let bindings_after = dir_entry_names(surface.gate().store().root_dir());
        let ws_after2 = dir_entry_names(&crate::work_staging::work_root_under(app_data.clone()));
        let _ = std::fs::remove_dir_all(&marker_path);
        // **设计保证的不变量是「不发布 handle」**（agent.rs 注释：「写失败即取消
        // 本次 Agent——绝不暴露可发送的 handle；引擎可能留下孤立会话，恢复时会被
        // unbound_surface 拦住」）。binding/workspace 是否残留一并**如实记录**，
        // 供评审判断是否需要改设计——不在测试里假装它不存在。
        let new_bindings = bindings_after.len() as i64 - bindings_before.len() as i64;
        let new_ws = ws_after2.len() as i64 - ws_before2.len() as i64;
        check!(
            "S7-work-failed-fresh-start-no-handle",
            blocked_marker && failed_at_marker && handle_after_fresh_fail.is_none(),
            format!(
                "failed_at_marker={failed_at_marker} handle={handle_after_fresh_fail:?}                  new_bindings={new_bindings} new_workspaces={new_ws} err={fresh_err}"
            )
        );

        check!(
            "S7-work-rejected-start-clean",
            pre.is_ok() && rejected_at_gate && no_handle && no_new_ws,
            format!("at_gate={rejected_at_gate} no_handle={no_handle} no_new_ws={no_new_ws} err={err_text}")
        );
    }


    if only_work {
        write(&format!("SMOKE DONE pass={pass} fail={fail}"));
        std::process::exit(if fail > 0 { 1 } else { 0 });
    }

    // ── S1 会话启动（默认模型）──────────────────────────────────────
    write("SMOKE S1-start BEGIN");
    let started = spawn_start(
        app.clone(),
        workspace.clone(),
        None,
        crate::surface_policy::NewSurfaceIntent::Code,
    )
    .await;
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
    let resumed = spawn_start(
        app.clone(),
        workspace.clone(),
        Some(sid.clone()),
        crate::surface_policy::NewSurfaceIntent::Code,
    )
    .await;
    let same_id = resumed.as_ref().map(|r| r.session_id == sid).unwrap_or(false);
    let after_len = chat_text().lines().count();
    check!(
        "S6-resume",
        same_id && after_len >= before_len,
        format!("same_id={same_id} lines {before_len}->{after_len}")
    );

    // ── S9 记忆回路（C3 验收：flush 真实引擎往返）───────────────
    // 隔离 GROK_HOME 的 config 副本里显式开 [memory].enabled——引擎在
    // **会话启动时**解析该开关，所以先写配置再起新会话并做真实 flush。
    // rewrite 暂不纳入产品入口/验收：锁定引擎把模型硬编码为 `grok-build`，
    // 第三方端点不可用；G26 引擎例外获批前，不能把该失败当 PASS。
    write("SMOKE S9-memory BEGIN");
    {
        let cfg_path = xai_grok_shell::util::grok_home::grok_home().join("config.toml");
        let enabled = match std::fs::read_to_string(&cfg_path) {
            Ok(text) => {
                let mut doc: toml_edit::DocumentMut = text.parse().unwrap_or_default();
                crate::memory_ops::write_memory_enabled(&mut doc, true);
                crate::config_core::write_config_atomic(&cfg_path, &doc.to_string()).is_ok()
            }
            Err(e) => {
                write(&format!("SMOKE S9-memory config unreadable: {e}"));
                false
            }
        };
        if !enabled {
            check!("S9-memory-roundtrip", false, "无法开启 [memory].enabled（隔离配置缺失/不可写）");
        } else {
            // 新会话（spawn_start 与 start_inner 一样会拆掉 S6 的会话——
            // 套件尾声，无后续依赖）。
            let s9 = spawn_start(
                app.clone(),
                workspace.clone(),
                None,
                crate::surface_policy::NewSurfaceIntent::Code,
            )
            .await;
            let (s9_sid, s9_err) = match s9 {
                Ok(r) => (r.session_id.clone(), String::new()),
                Err(e) => (String::new(), format!("{e:#}")),
            };
            if s9_sid.is_empty() {
                check!("S9-memory-flush", false, format!("session start: {s9_err}"));
            } else {
                // flush：引擎侧 did_flush 不随响应返回（Empty），回路本身
                // 不报错即通过；记忆未启用时引擎会显式报错（不会假阳性）。
                let flush = ext_call(&state, "x.ai/memory/flush", serde_json::json!({})).await;
                check!(
                    "S9-memory-flush",
                    flush.is_ok(),
                    format!("flush_err={}", flush.as_ref().err().cloned().unwrap_or_default())
                );
            }
        }
    }

    write(&format!("SMOKE DONE pass={pass} fail={fail}"));
    std::process::exit(if fail > 0 { 1 } else { 0 });
}

/// 在**独立任务**里启动会话（debug 构建的栈安全网）。
///
/// autotest() 是一个巨型 future；start_inner 又嵌着引擎的深异步链。两者的
/// poll 帧在 debug 构建下叠加超过 tokio worker 默认栈（C1 逃逸探针与 C3
/// S9 首跑都实测 `tokio-rt-worker has overflowed its stack`）。把会话启动
/// spawn 成全新任务后，它的 poll 链从干净栈开始——JoinHandle.await 本身
/// 不嵌套 poll。release 构建帧小本可不炸，但 smoke 跑的就是 debug。
async fn spawn_start(
    app: AppHandle,
    workspace: String,
    resume: Option<String>,
    intent: crate::surface_policy::NewSurfaceIntent,
) -> Result<crate::agent::StartResult, String> {
    let mut task = tauri::async_runtime::spawn(async move {
        let state = app.state::<crate::agent::AgentState>();
        // Code 走 canonical 包装器（顺带保活 start_inner——否则它在 autotest
        // 全走 spawn_start 后变成死代码，clippy -D warnings 会拦）。
        match intent {
            crate::surface_policy::NewSurfaceIntent::Code => {
                crate::agent::start_inner(app.clone(), &state, workspace, None, resume)
                    .await
                    .map_err(|e| format!("{e:#}"))
            }
            other => crate::agent::start_inner_with_intent(
                app.clone(),
                &state,
                workspace,
                None,
                resume,
                other,
            )
            .await
            .map_err(|e| format!("{e:#}")),
        }
    });
    match tokio::time::timeout(std::time::Duration::from_secs(120), &mut task).await {
        Ok(joined) => joined.map_err(|e| format!("start task join: {e}"))?,
        Err(_) => {
            // Dropping a Tokio JoinHandle detaches the task. Explicit abort is
            // required or a timed-out session start can keep mutating state and
            // make later smoke assertions nondeterministic.
            task.abort();
            let _ = task.await;
            Err("timed out after 120 seconds (start task aborted)".to_string())
        }
    }
}

/// 目录下的条目名（排序后可比）。用于「未被改动」「无新增」类断言。
pub(crate) fn dir_entry_names(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|it| {
            it.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
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
