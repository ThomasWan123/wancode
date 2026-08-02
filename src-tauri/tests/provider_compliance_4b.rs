//! #127-4b Provider 合规套件——工具调用（B 组）+ 多模态路由（D1）。
//!
//! 与 4a 同形态：隔离 `$GROK_HOME` + 进程内引擎 + 生产 ACP 序列 +
//! axum 情景化 mock。独立测试二进制（`$GROK_HOME` 是进程级 OnceLock，
//! 4a/4b 各占一个进程，互不竞争 config.toml）。少量 harness 与 4a 重复
//! ——两个二进制无法共享 crate 内模块，重复量小于共享模块的耦合成本。
//!
//! 4b 的新基建（正是 4a 通道纪律里 panic 的那类消息）：
//!   - RequestPermission → 自动选**首选项**（引擎约定首项为放行，与
//!     agent.rs AUTOTEST 无头路径同款）；
//!   - mock 记录**完整请求体**（B3 角色/工具形状断言的证据源）。
//!
//! 断言纪律（沿 4a 复核标准）：工具往返断言"传输层事实"——第二次请求
//! 必须携带 role:"tool" 结果且 tool_call_id 一一对应；不断言工具执行
//! 本身成功（参数 schema 归引擎所有，工具报错也必须完成往返）。
//!
//! 摘要写 COMPLIANCE_SUMMARY_PATH_4B 或 CARGO_TARGET_TMPDIR，CI 上传
//! artifact `compliance-summary-4b`。

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol as acp;
use axum::extract::State;
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use futures_util::stream;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use xai_acp_lib::{acp_send, AcpClientChannel, AcpClientMessage};
use xai_grok_pager::acp::spawn::spawn_grok_shell;
use xai_grok_shell::agent::auth_method::AuthMethodKind;

const API_KEY: &str = "compliance-4b-secret-key";
/// 24x24 红色 PNG——引擎发送前有两道尺寸门（image_dropped_notice 实测）：
/// 边长 >=8 且总像素 >=512。1x1 与 8x8 都被丢弃，合规样张须 >=512 px。
const PNG_SAMPLE: &str = "iVBORw0KGgoAAAANSUhEUgAAABgAAAAYCAIAAABvFaqvAAAAIElEQVR4nGP4z8BAFUQdU0YNGjVo1KBRg0YNGjWIIgQABMo93z5bbNQAAAAASUVORK5CYII=";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    /// B1+B3：单工具调用往返；请求体形状（role/tools 数组）一并断言。
    ToolRoundtrip,
    /// B2：一个 delta 两条 tool_calls，第二次请求须带两条对应结果。
    ParallelToolCalls,
    /// D1a：转述开启——图片必须打到 helper mock，主模型请求不含图。
    TranscribeRoute,
    /// D1b：转述关闭——图片内联进主模型请求。
    InlineRoute,
    /// helper mock 专属：返回唯一描述标记，供 D1a 断言"描述进入主模型"。
    HelperVision,
}

#[derive(Clone)]
struct Probe {
    scenario: Scenario,
    hits: Arc<AtomicU32>,
    bodies: Arc<Mutex<Vec<serde_json::Value>>>,
}

struct MockProvider {
    base_url: String,
    probe: Probe,
    _shutdown: oneshot::Sender<()>,
}

async fn spawn_mock(scenario: Scenario) -> MockProvider {
    let probe = Probe {
        scenario,
        hits: Arc::new(AtomicU32::new(0)),
        bodies: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/v1/chat/completions", post(handler))
        .with_state(probe.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            })
            .await;
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    MockProvider {
        base_url: format!("http://{addr}/v1"),
        probe,
        _shutdown: tx,
    }
}

fn sse(events: Vec<String>) -> Response {
    Sse::new(stream::iter(
        events
            .into_iter()
            .map(|d| Ok::<_, std::convert::Infallible>(Event::default().data(d)))
            .collect::<Vec<_>>(),
    ))
    .into_response()
}

fn text_chunks(text: &str) -> Vec<String> {
    vec![
        serde_json::json!({
            "id": "c", "object": "chat.completion.chunk", "created": 0, "model": "m",
            "choices": [{"index": 0, "delta": {"role": "assistant", "content": text}, "finish_reason": "stop"}]
        })
        .to_string(),
        "[DONE]".into(),
    ]
}

fn tool_call_chunks(calls: &[(&str, &str, &str)]) -> Vec<String> {
    // calls: (id, name, arguments-json)
    let tool_calls: Vec<serde_json::Value> = calls
        .iter()
        .enumerate()
        .map(|(i, (id, name, args))| {
            serde_json::json!({
                "index": i, "id": id, "type": "function",
                "function": {"name": name, "arguments": args}
            })
        })
        .collect();
    vec![
        serde_json::json!({
            "id": "c", "object": "chat.completion.chunk", "created": 0, "model": "m",
            "choices": [{"index": 0, "delta": {"role": "assistant", "tool_calls": tool_calls}, "finish_reason": null}]
        })
        .to_string(),
        serde_json::json!({
            "id": "c", "object": "chat.completion.chunk", "created": 0, "model": "m",
            "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]
        })
        .to_string(),
        "[DONE]".into(),
    ]
}

async fn handler(
    State(probe): State<Probe>,
    axum::extract::Json(body): axum::extract::Json<serde_json::Value>,
) -> Response {
    probe.hits.fetch_add(1, Ordering::SeqCst);
    probe.bodies.lock().unwrap().push(body.clone());
    // 分流按**结构**而非用户文本（复核定案：辅助请求会内嵌用户查询，
    // 文本判据必然误判）：
    //   messages 含 role=="tool" → 工具结果已回，给终稿；
    //   tools 为非空数组       → 主工具请求，给 tool_calls；
    //   其余（辅助标题/建议）   → 短文本。
    let has_tool_results = body["messages"]
        .as_array()
        .map(|msgs| {
            msgs.iter()
                .any(|m| m["role"].as_str() == Some("tool"))
        })
        .unwrap_or(false);
    let is_tool_request = body["tools"]
        .as_array()
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    match probe.scenario {
        Scenario::ToolRoundtrip => {
            if has_tool_results {
                sse(text_chunks("TOOLS-DONE"))
            } else if is_tool_request {
                sse(tool_call_chunks(&[(
                    "call_1",
                    "list_dir",
                    r#"{"path":"."}"#,
                )]))
            } else {
                sse(text_chunks("AUX-DONE"))
            }
        }
        Scenario::ParallelToolCalls => {
            if has_tool_results {
                sse(text_chunks("PARALLEL-DONE"))
            } else if is_tool_request {
                sse(tool_call_chunks(&[
                    ("call_a", "list_dir", r#"{"path":"."}"#),
                    ("call_b", "list_dir", r#"{"path":".."}"#),
                ]))
            } else {
                sse(text_chunks("AUX-DONE"))
            }
        }
        // 主 mock：只回文本（断言对象是**请求**形状与描述透传）
        Scenario::TranscribeRoute | Scenario::InlineRoute => {
            sse(text_chunks("MM-DONE"))
        }
        // helper mock：返回唯一描述标记——D1a 据此断言"图片 → helper →
        // 文字描述 → main"整条链路，而非仅两端各自形状。
        Scenario::HelperVision => sse(text_chunks("VISION-DESCRIPTION-4B")),
    }
}

// ── 引擎启动（与 4a 同款） ──────────────────────────────────────────────

fn write_config(grok_home: &std::path::Path, main_url: &str, helper_url: Option<&str>) {
    let helper_section = helper_url
        .map(|u| {
            format!(
                r#"
[model.helper-eyes]
name = "合规视觉辅助"
model = "helper-vision-model"
base_url = "{u}"
api_key = "{API_KEY}"
api_backend = "chat_completions"
context_window = 128000
max_retries = 1
"#
            )
        })
        .unwrap_or_default();
    // image_description 取运行时模型 id（= slug，非 catalog key）：
    // 4a 实录 user_message_chunk meta 的 modelId 即 slug。用户真实配置
    // key==slug（glm-4v-flash）掩盖了这一区分。
    let helper_route = if helper_url.is_some() {
        "image_description = \"helper-vision-model\"\n"
    } else {
        ""
    };
    let config = format!(
        r#"
[models]
default = "compliance"
{helper_route}
[model.compliance]
name = "合规套件模型"
model = "compliance-model"
base_url = "{main_url}"
api_key = "{API_KEY}"
api_backend = "chat_completions"
context_window = 128000
max_retries = 1
{helper_section}"#
    );
    std::fs::write(grok_home.join("config.toml"), config).unwrap();
}

fn agent_config_for(cwd: &std::path::Path) -> xai_grok_shell::agent::config::Config {
    let raw = xai_grok_shell::config::load_effective_config().unwrap();
    let mut cfg = xai_grok_shell::agent::config::Config::new_from_toml_cfg(&raw).unwrap();
    cfg.resolve_runtime_fields(&xai_grok_shell::agent::config::RuntimeResolutionContext {
        raw_config: &raw,
        remote_settings: None,
        cwd: Some(cwd),
        is_headless: true,
        cli_subagents: None,
        cli_web_search_model: None,
        cli_session_summary_model: None,
        cli_experimental_memory: false,
        cli_no_memory: false,
        disable_web_search: true,
        todo_gate: false,
        laziness_debug_log: None,
        storage_mode: None,
    });
    cfg.mode = xai_grok_shell::agent::config::AgentMode::Headless;
    cfg.default_yolo_mode = false;
    cfg
}

async fn spawn_engine(cwd: &std::path::Path, cancel: &CancellationToken) -> AcpClientChannel {
    let config = agent_config_for(cwd);
    let memory_config = config.memory_config.clone();
    let spawned = spawn_grok_shell(config, cancel, memory_config)
        .await
        .expect("引擎应能以隔离配置启动");
    let mut channel = spawned.channel;
    let init: acp::InitializeResponse = acp_send(
        acp::InitializeRequest::new(acp::ProtocolVersion::V1)
            .client_capabilities(acp::ClientCapabilities::new().terminal(false)),
        &channel.tx,
    )
    .await
    .expect("initialize");
    let method = init
        .auth_methods
        .iter()
        .find(|m| !AuthMethodKind::from_id(m.id()).needs_interactive_login())
        .map(|m| m.id().clone())
        .expect("api_key 配置应有非交互认证");
    let _: acp::AuthenticateResponse = acp_send(
        acp::AuthenticateRequest::new(method)
            .meta(serde_json::json!({"headless": true}).as_object().cloned()),
        &channel.tx,
    )
    .await
    .expect("authenticate");
    while let Ok(msg) = channel.rx.try_recv() {
        pump(msg, &mut String::new());
    }
    channel
}

/// 4b 消息泵：会话/Ext 通知 ack 并记录；权限请求自动选**首选项**放行
/// （引擎约定首项为放行——与 agent.rs 无头 AUTOTEST 路径同款）；
/// 其余请求类（文件/终端）仍 panic。
fn pump(msg: AcpClientMessage, transcript: &mut String) {
    match msg {
        AcpClientMessage::SessionNotification(b) => {
            transcript.push_str(
                &serde_json::to_value(&b.request.update)
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            );
            let _ = b.response_tx.send(Ok(()));
        }
        AcpClientMessage::ExtNotification(b) => {
            let _ = b.response_tx.send(Ok(()));
        }
        AcpClientMessage::RequestPermission(b) => {
            let outcome = match b.request.options.first().map(|o| o.option_id.clone()) {
                Some(id) => acp::RequestPermissionOutcome::Selected(
                    acp::SelectedPermissionOutcome::new(id),
                ),
                None => acp::RequestPermissionOutcome::Cancelled,
            };
            let _ = b
                .response_tx
                .send(Ok(acp::RequestPermissionResponse::new(outcome)));
        }
        other => panic!("4b 情景不应产生文件/终端类请求：{other:?}"),
    }
}

async fn run_prompt(
    channel: &mut AcpClientChannel,
    session_id: &acp::SessionId,
    blocks: Vec<acp::ContentBlock>,
) -> (String, Result<acp::PromptResponse, String>) {
    let req = acp::PromptRequest::new(session_id.clone(), blocks);
    let fut = acp_send(req, &channel.tx);
    tokio::pin!(fut);
    let mut transcript = String::new();
    let deadline = tokio::time::sleep(Duration::from_secs(90));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            resp = &mut fut => {
                while let Ok(msg) = channel.rx.try_recv() {
                    pump(msg, &mut transcript);
                }
                return (transcript, resp.map_err(|e| e.to_string()));
            }
            msg = channel.rx.recv() => match msg {
                Some(msg) => pump(msg, &mut transcript),
                None => return (transcript, Err("ACP 通道意外关闭".into())),
            },
            _ = &mut deadline => return (transcript, Err("回合 90s 未收束（悬挂）".into())),
        }
    }
}

fn tool_result_entries(body: &serde_json::Value) -> Vec<String> {
    body.get("messages")
        .and_then(|m| m.as_array())
        .map(|msgs| {
            msgs.iter()
                .filter(|m| m.get("role").and_then(|r| r.as_str()) == Some("tool"))
                .filter_map(|m| {
                    m.get("tool_call_id")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                })
                .collect()
        })
        .unwrap_or_default()
}

fn body_text(body: &serde_json::Value) -> String {
    body.to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn provider_compliance_tools_and_multimodal() {
    let tmp = tempfile::tempdir().unwrap();
    let grok_home = tmp.path().join("grok-home");
    let cwd = tmp.path().join("ws");
    std::fs::create_dir_all(&grok_home).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    // SAFETY: 测试二进制入口，引擎线程未启
    unsafe { std::env::set_var("GROK_HOME", &grok_home) };

    let mut summary = Vec::new();
    for scenario in [
        Scenario::ToolRoundtrip,
        Scenario::ParallelToolCalls,
        Scenario::TranscribeRoute,
        Scenario::InlineRoute,
    ] {
        let main_mock = spawn_mock(scenario).await;
        // 多模态情景配独立 helper mock；helper 只在 TranscribeRoute 被路由
        let helper_mock = match scenario {
            Scenario::TranscribeRoute | Scenario::InlineRoute => {
                Some(spawn_mock(Scenario::HelperVision).await)
            }
            _ => None,
        };
        write_config(
            &grok_home,
            &main_mock.base_url,
            helper_mock.as_ref().map(|m| m.base_url.as_str()),
        );
        // 转述开关：引擎 transcribe_images_enabled() 动态读 env
        // SAFETY: 情景串行，无并发读写
        unsafe {
            match scenario {
                Scenario::TranscribeRoute => std::env::set_var("GROK_IMAGE_TRANSCRIBE", "1"),
                Scenario::InlineRoute => std::env::set_var("GROK_IMAGE_TRANSCRIBE", "0"),
                _ => std::env::remove_var("GROK_IMAGE_TRANSCRIBE"),
            }
        }

        let cancel = CancellationToken::new();
        let mut channel = spawn_engine(&cwd, &cancel).await;
        let resp: acp::NewSessionResponse = acp_send(
            acp::NewSessionRequest::new(cwd.clone()).mcp_servers(Vec::new()),
            &channel.tx,
        )
        .await
        .expect("newSession");

        let blocks = match scenario {
            Scenario::TranscribeRoute | Scenario::InlineRoute => vec![
                acp::ContentBlock::Text(acp::TextContent::new("describe this".to_string())),
                acp::ContentBlock::Image(acp::ImageContent::new(
                    PNG_SAMPLE.to_string(),
                    "image/png".to_string(),
                )),
            ],
            _ => vec![acp::ContentBlock::Text(acp::TextContent::new(
                "use tools".to_string(),
            ))],
        };
        let (transcript, result) = run_prompt(&mut channel, &resp.session_id, blocks).await;

        match scenario {
            Scenario::ToolRoundtrip => {
                result
                    .as_ref()
                    .unwrap_or_else(|e| panic!("ToolRoundtrip 回合必须收束：{e}"));
                let bodies = main_mock.probe.bodies.lock().unwrap().clone();
                // 引擎的辅助请求（标题/建议生成回落会话客户端）可能与主
                // 回合交错抢占下标——按**内容**选请求，不按位置（CI 时序
                // 实锤过 bodies[0] 被辅助请求占据）。
                // 可能多条请求都含用户查询文本（辅助标题/建议请求会内嵌
                // 会话内容）——主回合的判据是"带 tools 声明的那条"；若一条
                // 都没有，携带全部候选的诊断切片失败。
                let candidates: Vec<&serde_json::Value> = bodies
                    .iter()
                    .filter(|b| body_text(b).contains("use tools"))
                    .collect();
                assert!(!candidates.is_empty(), "必须存在携带用户查询的请求");
                let main_req = candidates
                    .iter()
                    .find(|b| {
                        let t = body_text(b);
                        t.contains(r#""tools""#) && t.contains("list_dir")
                    })
                    .copied()
                    .unwrap_or_else(|| {
                        let diag: Vec<String> = candidates
                            .iter()
                            .map(|b| {
                                let t = body_text(b);
                                format!(
                                    "len={} model={} has_tools_key={} head={}",
                                    t.len(),
                                    b["model"].as_str().unwrap_or("?"),
                                    t.contains(r#""tools""#),
                                    t.chars().take(400).collect::<String>()
                                )
                            })
                            .collect();
                        panic!(
                            "主回合请求必须携带工具声明（含 list_dir）。候选 {} 条：
{}",
                            diag.len(),
                            diag.join("
---
")
                        )
                    });
                // B3 请求形状：结构化断言，不止字符串包含——
                // tools 为非空数组、条目 type=="function"、
                // function.name=="list_dir" 且 function.parameters 是对象。
                let tools_arr = main_req["tools"].as_array().expect("tools 必须是数组");
                assert!(!tools_arr.is_empty(), "tools 数组不得为空");
                let list_dir = tools_arr
                    .iter()
                    .find(|t| t["function"]["name"].as_str() == Some("list_dir"))
                    .expect("tools 必须含 function.name == list_dir 的条目");
                assert_eq!(
                    list_dir["type"].as_str(),
                    Some("function"),
                    "工具条目 type 必须为 function"
                );
                assert!(
                    list_dir["function"]["parameters"].is_object(),
                    "function.parameters 必须是对象"
                );
                let first_role = main_req["messages"][0]["role"].as_str().unwrap_or("");
                assert!(
                    first_role == "system" || first_role == "user",
                    "messages[0].role 形状异常：{first_role}"
                );
                // 往返：存在携带工具结果的后续请求，且 id 精确对应
                let ids = bodies
                    .iter()
                    .map(tool_result_entries)
                    .find(|ids| !ids.is_empty())
                    .expect("必须存在携带工具结果的后续请求");
                assert_eq!(ids, vec!["call_1"], "工具结果的 tool_call_id 必须精确对应");
                assert!(
                    transcript.contains("TOOLS-DONE"),
                    "最终文本必须送达：{transcript}"
                );
            }
            Scenario::ParallelToolCalls => {
                result
                    .as_ref()
                    .unwrap_or_else(|e| panic!("ParallelToolCalls 回合必须收束：{e}"));
                let bodies = main_mock.probe.bodies.lock().unwrap().clone();
                let mut ids = bodies
                    .iter()
                    .map(tool_result_entries)
                    .find(|ids| !ids.is_empty())
                    .expect("必须存在携带工具结果的后续请求");
                ids.sort();
                assert_eq!(
                    ids,
                    vec!["call_a", "call_b"],
                    "两条并行调用的结果必须都在同一后续请求里"
                );
                assert!(transcript.contains("PARALLEL-DONE"));
            }
            Scenario::TranscribeRoute => {
                result
                    .as_ref()
                    .unwrap_or_else(|e| panic!("TranscribeRoute 回合必须收束：{e}"));
                let helper = helper_mock.as_ref().unwrap();
                assert!(
                    helper.probe.hits.load(Ordering::SeqCst) >= 1,
                    "转述开启：图片必须路由到 helper"
                );
                // 按内容选（不按下标——本 PR 的时序教训）
                assert!(
                    helper
                        .probe
                        .bodies
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|b| body_text(b).contains(PNG_SAMPLE)),
                    "helper 请求必须携带图片数据"
                );
                for b in main_mock.probe.bodies.lock().unwrap().iter() {
                    assert!(
                        !body_text(b).contains(PNG_SAMPLE),
                        "主模型请求不得内联图片（转述已接管）"
                    );
                }
                // 链路闭环：helper 的描述文本必须进入主模型请求——否则
                // "引擎丢弃转述结果"也能通过前两条断言。
                assert!(
                    main_mock
                        .probe
                        .bodies
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|b| body_text(b).contains("VISION-DESCRIPTION-4B")),
                    "主模型请求必须包含 helper 返回的描述标记"
                );
            }
            Scenario::InlineRoute => {
                result
                    .as_ref()
                    .unwrap_or_else(|e| panic!("InlineRoute 回合必须收束：{e}"));
                let bodies = main_mock.probe.bodies.lock().unwrap().clone();
                assert!(
                    bodies.iter().any(|b| body_text(b).contains(PNG_SAMPLE)),
                    "转述关闭：图片必须内联进主模型请求"
                );
                let helper = helper_mock.as_ref().unwrap();
                assert_eq!(
                    helper.probe.hits.load(Ordering::SeqCst),
                    0,
                    "转述关闭：helper 不得被调用"
                );
            }
            // 仅作 helper mock 的响应脚本，不是独立情景
            Scenario::HelperVision => unreachable!("HelperVision 不进情景循环"),
        }
        cancel.cancel();
        summary.push(serde_json::json!({
            "scenario": format!("{scenario:?}"),
            "pass": true,
        }));
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // 环境复位（本测试进程私有，防对后续本地手跑造成惊吓）
    unsafe { std::env::remove_var("GROK_IMAGE_TRANSCRIBE") };

    let generated_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // #126 B1：engine_commit 来自构建清单（vendor/grok-build.lock），与
    // compatibility.md / 发布证据落点闭环。读不到 = 证据链断，直接 fail。
    let lock = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../vendor/grok-build.lock"),
    )
    .expect("构建清单 vendor/grok-build.lock 必须可读");
    let engine_commit = lock
        .lines()
        .find_map(|l| l.strip_prefix("commit="))
        .expect("构建清单缺 commit= 行")
        .to_string();
    let out = serde_json::json!({
        "suite": "4b-tools-multimodal",
        "scenarios": summary,
        "total": 4,
        "generated_unix": generated_unix,
        "ci_sha": std::env::var("GITHUB_SHA").ok(),
        "engine_commit": engine_commit,
    });
    let path = std::env::var("COMPLIANCE_SUMMARY_PATH_4B")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
                .join("compliance-summary-4b.json")
        });
    std::fs::write(&path, out.to_string()).expect("摘要文件必须写出——导出即契约");
    println!("COMPLIANCE_SUMMARY written to {}: {out}", path.display());
}
