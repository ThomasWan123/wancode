//! #127-4a Provider 合规套件——传输/流式（A 组）+ 错误解析（C 组）。
//!
//! 形态：与生产完全一致的链路——隔离 `$GROK_HOME` + `spawn_grok_shell`
//! 进程内引擎 + `initialize → authenticate → newSession → prompt` ACP
//! 序列（即 `agent.rs::start_session` / `agent_prompt` 的序列），provider
//! 端是本地 axum 情景化 mock。零外网、零真实 Key。
//!
//! 断言双侧：ACP 侧看引擎交回什么（文本送达、回合收束、错误可读且不泄
//! Key）；mock 侧看引擎发出什么（请求计数、Authorization）。
//!
//! `$GROK_HOME` 是引擎进程级 OnceLock：整个二进制一个隔离目录，情景间
//! **串行**重写其中的 config.toml（引擎在 spawn 时读盘）。
//!
//! 工具调用（B 组）与多模态（D1）在 4b；凭据端点隔离已由引擎 Gate 1
//! 测试覆盖（CI "Gate 1 引擎侧路由证据"步），不重做；上下文压缩由
//! 引擎 xai-grok-compaction crate 的单测覆盖（128 条，CI "引擎压缩
//! 单测"步），不做 ACP 级重演——矩阵标"引擎层覆盖"而非"通过"。
//!
//! 末尾把结构化摘要写入 JSON 文件（COMPLIANCE_SUMMARY_PATH 或
//! CARGO_TARGET_TMPDIR），CI 上传为 artifact `compliance-summary-4a`
//! 供矩阵回填（PR 5）——cargo 捕获通过测试的 stdout，println 不算导出。

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol as acp;
use axum::extract::State;
use axum::http::HeaderMap;
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

const API_KEY: &str = "compliance-secret-key-do-not-leak";

// ── 情景化 mock provider ────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    /// A1 标准流：两个内容 chunk + finish + [DONE]。
    Standard,
    /// A2a 流结束变体：完整内容但**没有 [DONE] 哨兵**。
    NoDoneMarker,
    /// A3 chunk 里完全没有 usage 字段（本就未发过——显式作为断言维度）。
    NoUsage,
    /// A4 DeepSeek R1 形状：delta 走 reasoning_content + content 混流。
    ReasoningContent,
    /// C1 401 + OpenAI 错误体。
    Err401,
    /// C2 429 + OpenAI 限流错误体。
    Err429,
    /// C3 500 + 非 JSON（HTML）错误体。
    Err500Html,
}

#[derive(Clone)]
struct Probe {
    scenario: Scenario,
    hits: Arc<AtomicU32>,
    auth: Arc<Mutex<Vec<String>>>,
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
        auth: Arc::new(Mutex::new(Vec::new())),
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

fn chunk(delta: serde_json::Value, finish: Option<&str>) -> String {
    serde_json::json!({
        "id": "chatcmpl-compliance",
        "object": "chat.completion.chunk",
        "created": 0,
        "model": "compliance-model",
        "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }]
    })
    .to_string()
}

async fn handler(State(probe): State<Probe>, headers: HeaderMap) -> Response {
    probe.hits.fetch_add(1, Ordering::SeqCst);
    if let Some(a) = headers.get("authorization").and_then(|v| v.to_str().ok()) {
        probe.auth.lock().unwrap().push(a.to_owned());
    }
    let sse = |events: Vec<String>| {
        Sse::new(stream::iter(
            events
                .into_iter()
                .map(|d| Ok::<_, std::convert::Infallible>(Event::default().data(d)))
                .collect::<Vec<_>>(),
        ))
        .into_response()
    };
    match probe.scenario {
        // Standard 与 NoUsage 的线上载荷必须真实不同（复核：同证据不同名
        // 即假覆盖）：Standard 按真实 provider 形状带 usage 终块。
        Scenario::Standard => sse(vec![
            chunk(serde_json::json!({"role": "assistant", "content": "COMPLIANCE-"}), None),
            chunk(serde_json::json!({"content": "OK"}), Some("stop")),
            serde_json::json!({
                "id": "chatcmpl-compliance",
                "object": "chat.completion.chunk",
                "created": 0,
                "model": "compliance-model",
                "choices": [],
                "usage": {"prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7}
            })
            .to_string(),
            "[DONE]".into(),
        ]),
        Scenario::NoUsage => sse(vec![
            chunk(serde_json::json!({"role": "assistant", "content": "COMPLIANCE-"}), None),
            chunk(serde_json::json!({"content": "OK"}), Some("stop")),
            "[DONE]".into(),
        ]),
        Scenario::NoDoneMarker => sse(vec![
            chunk(serde_json::json!({"role": "assistant", "content": "COMPLIANCE-"}), None),
            chunk(serde_json::json!({"content": "OK"}), Some("stop")),
            // 无 [DONE]：流自然 EOF
        ]),
        Scenario::ReasoningContent => sse(vec![
            chunk(
                serde_json::json!({"role": "assistant", "reasoning_content": "thinking hard"}),
                None,
            ),
            chunk(serde_json::json!({"content": "COMPLIANCE-OK"}), Some("stop")),
            "[DONE]".into(),
        ]),
        Scenario::Err401 => (
            axum::http::StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({
                "error": {"message": "Invalid API key provided", "type": "invalid_request_error", "code": "invalid_api_key"}
            })),
        )
            .into_response(),
        Scenario::Err429 => (
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            axum::Json(serde_json::json!({
                "error": {"message": "Rate limit reached", "type": "rate_limit_error", "code": "rate_limit_exceeded"}
            })),
        )
            .into_response(),
        Scenario::Err500Html => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            [("content-type", "text/html")],
            "<html><body>upstream exploded</body></html>",
        )
            .into_response(),
    }
}

// ── 生产同款引擎启动 ────────────────────────────────────────────────────

fn write_config(grok_home: &std::path::Path, base_url: &str) {
    let config = format!(
        r#"
[models]
default = "compliance"

[model.compliance]
name = "合规套件模型"
model = "compliance-model"
base_url = "{base_url}"
api_key = "{API_KEY}"
api_backend = "chat_completions"
context_window = 128000
# 5xx 重试预算收敛（默认 15 次指数退避会超出测试窗口；重试行为本身
# 以 mock hits >= 2 断言）
max_retries = 1
"#
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
    // 认证阶段可能已有通知积压，先清空（逐条 ack）
    while let Ok(msg) = channel.rx.try_recv() {
        ack(msg);
    }
    channel
}

fn ack(msg: AcpClientMessage) {
    // 本套件（4a）不含工具执行：只应收到通知类消息（会话通知 + 引擎的
    // Ext 广播如 mcp/servers_updated）；请求类（权限/文件/终端）一律明确
    // 失败——静默吞掉会把 4b 范围的问题伪装成超时。
    match msg {
        AcpClientMessage::SessionNotification(b) => {
            let _ = b.response_tx.send(Ok(()));
        }
        AcpClientMessage::ExtNotification(b) => {
            let _ = b.response_tx.send(Ok(()));
        }
        other => panic!("4a 情景不应产生请求类消息：{other:?}"),
    }
}

/// 一个 prompt 回合：并发收集会话通知文本，直到 PromptResponse 返回。
/// 返回（拼接的可见文本+通知原文, prompt 结果）。
async fn run_prompt(
    channel: &mut AcpClientChannel,
    session_id: &acp::SessionId,
    text: &str,
) -> (String, Result<acp::PromptResponse, String>) {
    let req = acp::PromptRequest::new(
        session_id.clone(),
        vec![acp::ContentBlock::Text(acp::TextContent::new(
            text.to_string(),
        ))],
    );
    let fut = acp_send(req, &channel.tx);
    tokio::pin!(fut);
    let mut collected = String::new();
    let deadline = tokio::time::sleep(Duration::from_secs(60));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            resp = &mut fut => {
                // 回合结束后可能还有残余通知，清空
                while let Ok(msg) = channel.rx.try_recv() {
                    if let AcpClientMessage::SessionNotification(b) = &msg {
                        collected.push_str(&serde_json::to_value(&b.request.update)
                            .map(|v| v.to_string()).unwrap_or_default());
                    }
                    ack(msg);
                }
                return (collected, resp.map_err(|e| e.to_string()));
            }
            msg = channel.rx.recv() => match msg {
                Some(msg) => {
                    if let AcpClientMessage::SessionNotification(b) = &msg {
                        collected.push_str(&serde_json::to_value(&b.request.update)
                            .map(|v| v.to_string()).unwrap_or_default());
                    }
                    ack(msg);
                }
                None => return (collected, Err("ACP 通道意外关闭".into())),
            },
            _ = &mut deadline => return (collected, Err("回合 60s 未收束（悬挂）".into())),
        }
    }
}

// ── 套件主体（串行情景；GROK_HOME 为进程级 OnceLock） ───────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn provider_compliance_transport_and_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let grok_home = tmp.path().join("grok-home");
    let cwd = tmp.path().join("ws");
    std::fs::create_dir_all(&grok_home).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    // SAFETY: 测试二进制入口，引擎线程未启
    unsafe { std::env::set_var("GROK_HOME", &grok_home) };

    let mut summary = Vec::new();
    for scenario in [
        Scenario::Standard,
        Scenario::NoDoneMarker,
        Scenario::NoUsage,
        Scenario::ReasoningContent,
        Scenario::Err401,
        Scenario::Err429,
        Scenario::Err500Html,
    ] {
        let mock = spawn_mock(scenario).await;
        write_config(&grok_home, &mock.base_url);
        let cancel = CancellationToken::new();
        let mut channel = spawn_engine(&cwd, &cancel).await;
        let resp: acp::NewSessionResponse = acp_send(
            acp::NewSessionRequest::new(cwd.clone()).mcp_servers(Vec::new()),
            &channel.tx,
        )
        .await
        .expect("newSession");
        let (transcript, result) =
            run_prompt(&mut channel, &resp.session_id, "reply with the marker").await;

        assert!(
            mock.probe.hits.load(Ordering::SeqCst) >= 1,
            "{scenario:?}: 请求必须到达 mock provider"
        );
        let auth = mock.probe.auth.lock().unwrap().join(",");
        assert!(
            auth.contains(API_KEY),
            "{scenario:?}: Authorization 必须携带配置的 Key"
        );

        match scenario {
            Scenario::Standard | Scenario::NoDoneMarker | Scenario::NoUsage => {
                result.as_ref().unwrap_or_else(|e| {
                    panic!("{scenario:?}: 回合必须正常收束，实际错误：{e}")
                });
                assert!(
                    transcript.contains("COMPLIANCE-") && transcript.contains("OK"),
                    "{scenario:?}: 回复文本必须完整送达，transcript={transcript}"
                );
            }
            Scenario::ReasoningContent => {
                result.as_ref().unwrap_or_else(|e| {
                    panic!("ReasoningContent: 回合必须正常收束，实际错误：{e}")
                });
                assert!(
                    transcript.contains("COMPLIANCE-OK"),
                    "正文必须送达：{transcript}"
                );
                // 正向：reasoning 必须保留进思考通知（agent_thought_chunk，
                // 本机探针实证引擎会发该块）
                assert!(
                    transcript.contains(r#""sessionUpdate":"agent_thought_chunk""#)
                        && transcript.contains("thinking hard"),
                    "reasoning_content 必须以 agent_thought_chunk 送达：{transcript}"
                );
                // 反向：思考内容不得混进正文块（agent_message_chunk）
                for update in transcript.split('}') {
                    if update.contains("agent_message_chunk") {
                        assert!(
                            !update.contains("thinking hard"),
                            "reasoning_content 不得混入正文块：{update}"
                        );
                    }
                }
            }
            Scenario::Err401 | Scenario::Err429 | Scenario::Err500Html => {
                // 错误必须以 Err 收束；悬挂超时是独立文案，绝不与"错误可见"
                // 混同——上一版的假覆盖正在此处（500 实为重试预算未耗尽）。
                let err = result
                    .as_ref()
                    .err()
                    .unwrap_or_else(|| panic!("{scenario:?}: 回合必须以错误收束"))
                    .clone();
                assert!(
                    !err.contains("未收束"),
                    "{scenario:?}: 不许把悬挂超时当作错误可见：{err}"
                );
                // 逐情景区分性断言（形状取自引擎实际输出）
                match scenario {
                    Scenario::Err401 => assert!(
                        err.contains("401") && err.contains("Invalid API key provided"),
                        "401 必须携带状态与供应商 message：{err}"
                    ),
                    Scenario::Err429 => assert!(
                        err.contains("Rate limited") && err.contains("429"),
                        "429 必须归类为限流：{err}"
                    ),
                    Scenario::Err500Html => {
                        assert!(err.contains("500"), "5xx 必须携带状态：{err}");
                        // 非 JSON 体不得导致崩溃/悬挂；且 5xx 走了重试
                        // （max_retries=1 → 恰好 2 次请求）
                        // max_retries=1 的契约是**恰好** 1+1=2 次：
                        // >=2 会把失控重试误判为通过。
                        assert_eq!(
                            mock.probe.hits.load(Ordering::SeqCst),
                            2,
                            "max_retries=1 必须恰好请求 2 次"
                        );
                    }
                    _ => unreachable!(),
                }
                assert!(
                    !format!("{transcript} {err}").contains(API_KEY),
                    "{scenario:?}: 错误信息绝不许包含 API Key"
                );
            }
        }
        cancel.cancel();
        summary.push(serde_json::json!({
            "scenario": format!("{scenario:?}"),
            "pass": true,
        }));
        // 引擎异步退出，稍候释放端口/文件句柄
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // 矩阵回填用结构化摘要（PR 5 消费）。字段即全部内容：suite、逐情景
    // 结果（任一失败整测早已 panic，走到这里即全过）、情景数、生成时间
    // （unix 秒）、CI 提交（GITHUB_SHA，本地运行为 null）。
    let generated_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let out = serde_json::json!({
        "suite": "4a-transport-errors",
        "scenarios": summary,
        "total": 7,
        "generated_unix": generated_unix,
        "ci_sha": std::env::var("GITHUB_SHA").ok(),
    });
    // 真实导出通道：写 JSON 文件（cargo 捕获通过测试的 stdout，println
    // 在 CI 日志里不可见——只打印不算导出）。路径优先取
    // COMPLIANCE_SUMMARY_PATH（CI 设为 workspace 内并上传 artifact），
    // 本地缺省落 CARGO_TARGET_TMPDIR。
    let path = std::env::var("COMPLIANCE_SUMMARY_PATH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
                .join("compliance-summary-4a.json")
        });
    std::fs::write(&path, out.to_string()).expect("摘要文件必须写出——导出即契约");
    println!("COMPLIANCE_SUMMARY written to {}: {out}", path.display());
}
