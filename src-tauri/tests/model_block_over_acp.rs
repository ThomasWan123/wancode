//! v0.18.6：真实引擎经 ACP 恢复一个旧格式会话，必须在 LoadSessionResponse
//! 的 meta 里交回 `x.ai/modelBlock`。
//!
//! 这是整条链上唯一没有任何自动化证据的一环。此前的覆盖是：引擎内部的解析
//! 与路由有集成测试，前端的渲染与状态机有 RTL——中间这段
//! `MvpAgent::load_session → meta["x.ai/modelBlock"] → Tauri` 只有代码层
//! 保证。而这条缝正是历史上出问题最多的地方：`endpoint_label` 的
//! snake_case/camelCase 错配就发生在这里，两侧各自都对，接起来是错的。
//!
//! 做法与生产完全一致：`spawn_grok_shell` 起进程内引擎、initialize、
//! authenticate、loadSession——就是 `agent.rs::start_session` 的序列。
//! 配置写进隔离的 `$GROK_HOME`，模型指向不存在的本地端口（本测试不发请求，
//! 只看恢复时的判定），因此不需要任何真实 Key、不产生外网流量。

use std::path::PathBuf;

use agent_client_protocol as acp;
use tokio_util::sync::CancellationToken;
use xai_acp_lib::acp_send;
use xai_grok_pager::acp::spawn::spawn_grok_shell;
use xai_grok_shell::agent::auth_method::AuthMethodKind;
use xai_grok_shell::agent::config::Config as AgentConfig;

/// 两个条目共享上游 slug `glm-4.6`，端点不同——用户报的那个事故的形状。
const CONFIG: &str = r#"
[model.glm-open]
name = "模拟·开放平台"
model = "glm-4.6"
base_url = "http://127.0.0.1:34101/v1"
api_key = "key-for-open"
api_backend = "chat_completions"
context_window = 128000

[model.glm-coding]
name = "模拟·Coding Plan"
model = "glm-4.6"
base_url = "http://127.0.0.1:34102/v1"
api_key = "key-for-coding"
api_backend = "chat_completions"
context_window = 128000
"#;

const SESSION_ID: &str = "acp-legacy-session";

/// 落一份 v0.18.6 之前形状的会话记录：只有 slug，没有 catalog_model_id。
///
/// 用**生产写入器**落盘，不手工摆文件——会话目录由 cwd 参与推导
/// （`sessions_cwd_dir(cwd).join(id)`），手写路径既容易猜错，也测不到真实
/// 布局。两参 `init_session` 恰好就是不写 catalog key 的那个入口。
async fn write_legacy_session(grok_home: &std::path::Path, cwd: &std::path::Path) {
    use xai_grok_shell::session::storage::jsonl::JsonlStorageAdapter;
    use xai_grok_shell::session::storage::StorageAdapter;
    let adapter = JsonlStorageAdapter::with_root(grok_home.to_path_buf());
    let info = xai_grok_shell::session::info::Info {
        id: acp::SessionId::new(SESSION_ID),
        cwd: cwd.to_string_lossy().into_owned(),
    };
    adapter
        .init_session(&info, acp::ModelId::new("glm-4.6"))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn loading_a_legacy_ambiguous_session_returns_a_structured_model_block() {
    let tmp = tempfile::tempdir().unwrap();
    let grok_home = tmp.path().join(".grok");
    let cwd = tmp.path().join("proj");
    std::fs::create_dir_all(&grok_home).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::write(grok_home.join("config.toml"), CONFIG).unwrap();

    // 必须在**任何**东西碰 grok_home() 之前设好：它是 OnceLock，一个进程只
    // 认第一次；而会话目录由 sessions_cwd_dir() 推导，那个函数走的正是全局
    // grok_home，不是写入器构造时传的 root。先写会话再设环境变量，写入器和
    // 引擎会落在两个不同的目录里。
    unsafe {
        std::env::set_var("GROK_HOME", &grok_home);
    }
    write_legacy_session(&grok_home, &cwd).await;

    // ── 与 agent.rs::start_session 同一套启动序列 ───────────────────
    let raw_config = xai_grok_shell::config::load_effective_config().unwrap();
    let mut agent_config = AgentConfig::new_from_toml_cfg(&raw_config).unwrap();
    agent_config.resolve_runtime_fields(&xai_grok_shell::agent::config::RuntimeResolutionContext {
        raw_config: &raw_config,
        remote_settings: None,
        cwd: Some(&cwd),
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
    agent_config.mode = xai_grok_shell::agent::config::AgentMode::Headless;
    agent_config.default_yolo_mode = false;

    let cancel = CancellationToken::new();
    let memory_config = agent_config.memory_config.clone();
    let spawned = spawn_grok_shell(agent_config, &cancel, memory_config)
        .await
        .expect("引擎应能以隔离配置启动");
    let acp_tx = spawned.channel.tx;

    let init_resp: acp::InitializeResponse = acp_send(
        acp::InitializeRequest::new(acp::ProtocolVersion::V1)
            .client_capabilities(acp::ClientCapabilities::new().terminal(false)),
        &acp_tx,
    )
    .await
    .expect("initialize");

    let method_id = init_resp
        .auth_methods
        .iter()
        .find(|m| !AuthMethodKind::from_id(m.id()).needs_interactive_login())
        .map(|m| m.id().clone())
        .expect("配置里有 api_key，应当存在非交互认证方式");
    let _: acp::AuthenticateResponse = acp_send(
        acp::AuthenticateRequest::new(method_id)
            .meta(serde_json::json!({"headless": true}).as_object().cloned()),
        &acp_tx,
    )
    .await
    .expect("authenticate");

    // ── 恢复那份旧记录 ──────────────────────────────────────────────
    let resp: acp::LoadSessionResponse = acp_send(
        acp::LoadSessionRequest::new(
            acp::SessionId::new(SESSION_ID),
            PathBuf::from(cwd.to_string_lossy().to_string()),
        ),
        &acp_tx,
    )
    .await
    .expect("会话应当能加载——历史可读是设计的一部分，被挡住的只是发送");

    let meta = resp.meta.as_ref().expect("load 响应必须带 meta");
    let block = meta
        .get("x.ai/modelBlock")
        .expect("重复 slug 的旧记录必须带回 modelBlock——否则客户端第一次察觉到问题是一个空 EndTurn");

    assert_eq!(
        block.get("kind").and_then(|k| k.as_str()),
        Some("ambiguous_model_id")
    );
    assert_eq!(
        block.get("requested").and_then(|r| r.as_str()),
        Some("glm-4.6")
    );

    let candidates = block
        .get("candidates")
        .and_then(|c| c.as_array())
        .expect("必须带候选，否则前端只能弹一个没有选项的框");
    assert_eq!(candidates.len(), 2, "两个条目都要给出：{candidates:?}");

    for c in candidates {
        // camelCase，且端点非空——这两条正是那次 snake_case 事故的形状。
        let label = c
            .get("endpointLabel")
            .and_then(|e| e.as_str())
            .expect("候选必须有 endpointLabel（camelCase）");
        assert!(
            label.contains("127.0.0.1"),
            "端点标签必须是真实 host，空白等于选择器没用：{label}"
        );
        assert!(c.get("endpoint_label").is_none(), "snake_case 不得泄漏到线上");
    }
    let ids: Vec<&str> = candidates
        .iter()
        .filter_map(|c| c.get("id").and_then(|i| i.as_str()))
        .collect();
    assert!(ids.contains(&"glm-open") && ids.contains(&"glm-coding"), "{ids:?}");

    cancel.cancel();
}
