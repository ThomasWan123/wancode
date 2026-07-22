//! v0.18-4 步 B：Review 只读子会话（v0.15 自建审查，engine review 是假能力）。
use tauri::State;
use xai_acp_lib::acp_send;
use agent_client_protocol as acp;

use crate::agent::{ext_call, AgentState};
use crate::autotest::walkdir_find;

/// v0.15 Review：只读子会话审查未提交改动，返回结构化 findings。
///
/// 引擎的 `x.ai/review` 是假能力（评论体硬编码 null 的只写遥测，见 roadmap
/// 附 B 审计），所以走自建路线：临时新会话 + plan（只读）模式 + 结构化
/// prompt，结果从磁盘会话历史读取，用后即删。后台会话经 background_sessions
/// 屏蔽：不发 agent://update、权限请求自动取消。
#[tauri::command]
pub async fn review_run(state: State<'_, AgentState>) -> Result<serde_json::Value, String> {
    let (acp_tx, cwd) = {
        let guard = state.handle.lock().await;
        let h = guard.as_ref().ok_or("会话未启动")?;
        (h.acp_tx.clone(), h.cwd.clone())
    };

    // 1. 收集未提交改动的 unified diff（总量截断，防撑爆上下文）
    let diffs = ext_call(
        &state,
        "x.ai/git/diffs",
        serde_json::json!({ "includePatch": true }),
    )
    .await?;
    let env = diffs.get("result").unwrap_or(&diffs);
    let files = env
        .get("data")
        .unwrap_or(env)
        .get("files")
        .and_then(|f| f.as_array())
        .cloned()
        .unwrap_or_default();
    const PATCH_BUDGET: usize = 60_000;
    let mut patches = String::new();
    let mut skipped: Vec<String> = Vec::new();
    for f in &files {
        let path = f.get("path").and_then(|p| p.as_str()).unwrap_or("?");
        match f.get("patch").and_then(|p| p.as_str()) {
            Some(p) if patches.len() + p.len() <= PATCH_BUDGET => {
                patches.push_str(p);
                patches.push('\n');
            }
            _ => skipped.push(path.to_string()),
        }
    }
    if patches.is_empty() {
        return Err("工作区没有可审查的未提交改动".into());
    }

    // 2. 临时会话。客户端不传 mcpServers（引擎只加载我们传入的列表）。
    // 刻意不用 plan 模式：plan 收尾会触发 exit_plan_mode 审批握手，模型
    // 还倾向把结论塞进"计划"而不是普通回复（实测翻过车）。只读靠
    // background_sessions 的权限自动取消兜底——写类工具批不下来。
    let resp: acp::NewSessionResponse =
        acp_send(acp::NewSessionRequest::new(cwd.clone()), &acp_tx)
            .await
            .map_err(|e| format!("创建审查会话失败: {e}"))?;
    let rid = resp.session_id.clone();
    state
        .background_sessions
        .lock()
        .await
        .insert(rid.0.to_string());

    // 3. 结构化审查 prompt
    let prompt = format!(
        "你是严格的代码审查员。审查下面的未提交改动（unified diff）。\
         你只允许读文件辅助理解，绝不修改任何东西。\n\
         只输出一个 JSON 数组，不要任何其它文字或代码围栏。每个元素：\n\
         {{\"file\": \"路径\", \"line\": 行号或null, \"severity\": \"error|warn|info\", \
         \"comment\": \"中文评论，具体指出问题与建议\"}}\n\
         没有问题就输出 []。最多 12 条，按严重度排序。\n\n<diff>\n{patches}\n</diff>"
    );
    let send_fut = async {
        let r: Result<acp::PromptResponse, _> = acp_send(
            acp::PromptRequest::new(
                rid.clone(),
                vec![acp::ContentBlock::Text(acp::TextContent::new(prompt))],
            ),
            &acp_tx,
        )
        .await;
        r
    };
    let turn = tokio::time::timeout(std::time::Duration::from_secs(300), send_fut).await;

    // 4. 从磁盘读最后一条 assistant 消息（ACP PromptResponse 只有 stop_reason，
    //    没有文本，磁盘是唯一取证路径）。turn 结束后引擎的最后一笔写可能
    //    尚未落盘——空结果重试 3 次，每次 500ms。同步 IO 丢 spawn_blocking，
    //    不占 tokio worker（自审 finding L3296/L3300）。
    let sessions_base = xai_grok_shell::util::grok_home::grok_home().join("sessions");
    let rid_str = rid.0.to_string();
    let mut text = String::new();
    for attempt in 0..3u8 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        let base = sessions_base.clone();
        let sid = rid_str.clone();
        let got = tokio::task::spawn_blocking(move || {
            walkdir_find(&base, &sid)
                .map(|d| d.join("chat_history.jsonl"))
                .and_then(|f| std::fs::read_to_string(f).ok())
                .and_then(|s| {
                    s.lines()
                        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                        .filter(|v| v.get("type").and_then(|t| t.as_str()) == Some("assistant"))
                        .filter_map(|v| {
                            v.get("content").and_then(|c| c.as_str()).map(String::from)
                        })
                        .next_back()
                })
                .unwrap_or_default()
        })
        .await
        .unwrap_or_default();
        if !got.is_empty() {
            text = got;
            break;
        }
    }

    // 5. 清理：先删会话，删完（或删失败）再移出屏蔽集——顺序反过来会有
    //    竞态窗口：会话还在引擎里活着却已不被屏蔽，通知会泄进主聊天流。
    let _ = ext_call(
        &state,
        "x.ai/session/delete",
        serde_json::json!({ "sessionId": rid.0.to_string(), "cwd": cwd.to_string_lossy() }),
    )
    .await;
    state
        .background_sessions
        .lock()
        .await
        .remove(rid.0.as_ref());

    // 超时与引擎错误分开报——内层 Err 是引擎拒绝/失败，吞掉它排查会很痛苦
    match turn {
        Err(_) => return Err("审查超时（300 秒）".into()),
        Ok(Err(e)) => return Err(format!("审查会话执行失败: {e}")),
        Ok(Ok(_)) => {}
    }
    if text.is_empty() {
        return Err("审查会话没有产出内容".into());
    }

    // 6. 宽容解析：截取首个 [ 到末个 ]，模型加围栏也能活
    let json_str = match (text.find('['), text.rfind(']')) {
        (Some(a), Some(b)) if b > a => &text[a..=b],
        _ => "",
    };
    let findings: serde_json::Value =
        serde_json::from_str(json_str).unwrap_or(serde_json::Value::Null);
    Ok(serde_json::json!({
        "findings": findings,          // null = 解析失败，前端退回显示 raw
        "raw": text,
        "reviewedFiles": files.len() - skipped.len(),
        "skippedFiles": skipped,
    }))
}
