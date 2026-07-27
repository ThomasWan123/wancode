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

/// 冒烟里观察到的现象：新建会话落盘为 `current_model_id = glm-open`（配置键）
/// 且没有 `catalog_model_id`——这是 v0.18.6 之前的字段语义，本分支应当写成
/// `current = glm-4.6`（上游 slug）+ `catalog = glm-open`（配置键）。
///
/// 与其对着一份 summary.json 推测，把它变成可复现的断言。
/// 现状：**失败**，记录一个已确认的未修 bug。暂时 ignore 是为了不让 CI 变红
/// 掩盖其它回归，不是为了把它藏起来——它必须在合并前修掉。
///
/// 现象（本机冒烟 + 本测试两次独立复现）：默认模型新建的会话落盘为
///   current_model_id = "glm-open"（配置键）、catalog_model_id 缺失
/// 而本分支的约定是 current = "glm-4.6"（上游 slug）+ catalog = "glm-open"。
///
/// 诊断证据（本测试 --nocapture 实测）：
///   目录 available = ["glm-open", "glm-coding"]，引擎当前模型 = glm-open
/// 也就是说**目录是满的、glm-open 就在里面**，resolve_model_id 没有理由失败。
/// 我最初"回落到基线 sampling config"的解释因此被推翻——真正把配置键写进
/// current_model_id 的那一处还没有找到，不要从这个错误前提继续往下修。
///
/// 下一步该查的方向（按代价从低到高）：
///   1. persistence::new 之后是否还有别的写入把 current_model_id 覆盖成
///      运行时 key（"Update model if different" 那段、或某条 CurrentModel 消息）；
///   2. session_sampling.model 在默认模型分支的实际取值——打点确认它是
///      glm-4.6 还是 glm-open，据此判断问题在组装还是在覆盖；
///   3. 步6b 的 initial_persisted_identity 是否真的被这条路径调用到。
///
/// 同进程里 GROK_HOME 是 OnceLock，本测试与上一条不能共存于一次运行——
/// 修复时需要把两者合并成共用一份夹具，或拆成两个测试二进制。
#[ignore = "已确认的未修 bug：默认模型新建会话把配置键写进了 current_model_id"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_session_persists_slug_and_key_in_their_own_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let grok_home = tmp.path().join(".grok");
    let cwd = tmp.path().join("proj2");
    std::fs::create_dir_all(&grok_home).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::write(grok_home.join("config.toml"), CONFIG).unwrap();
    unsafe {
        std::env::set_var("GROK_HOME", &grok_home);
    }

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

    let cancel = CancellationToken::new();
    let memory_config = agent_config.memory_config.clone();
    let spawned = spawn_grok_shell(agent_config, &cancel, memory_config)
        .await
        .expect("引擎启动");
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
        .expect("非交互认证方式");
    let _: acp::AuthenticateResponse = acp_send(
        acp::AuthenticateRequest::new(method_id)
            .meta(serde_json::json!({"headless": true}).as_object().cloned()),
        &acp_tx,
    )
    .await
    .expect("authenticate");

    let new_resp: acp::NewSessionResponse = acp_send(
        acp::NewSessionRequest::new(PathBuf::from(cwd.to_string_lossy().to_string())),
        &acp_tx,
    )
    .await
    .expect("new session");

    // ── 诊断：为什么精确键 glm-open 会解析不到？ ─────────────────────
    // 先取证再修，否则只是治结果。NewSessionResponse.models 就是引擎当时
    // 对外暴露的目录快照。
    let avail: Vec<String> = new_resp
        .models
        .as_ref()
        .map(|m| m.available_models.iter().map(|x| x.model_id.0.to_string()).collect())
        .unwrap_or_default();
    let current = new_resp
        .models
        .as_ref()
        .map(|m| m.current_model_id.0.to_string())
        .unwrap_or_else(|| "<none>".into());
    eprintln!("[诊断] 目录 available = {avail:?}");
    eprintln!("[诊断] 引擎当前模型 = {current}");

    // 直接读落盘产物——字段语义只能在磁盘上验证。
    let dir = xai_grok_shell::session::persistence::session_dir(
        &xai_grok_shell::session::info::Info {
            id: new_resp.session_id.clone(),
            cwd: cwd.to_string_lossy().into_owned(),
        },
    );
    let raw = std::fs::read_to_string(dir.join("summary.json")).expect("summary 应已落盘");
    let json: serde_json::Value = serde_json::from_str(&raw).unwrap();

    assert_eq!(
        json["current_model_id"], "glm-4.6",
        "current_model_id 必须是上游 slug；写成配置键会让旧版本与同步端把它         当成上游模型名。实际落盘：{raw}"
    );
    assert_eq!(
        json["catalog_model_id"], "glm-open",
        "首写就该带上配置键，否则崩溃窗口还在。实际落盘：{raw}"
    );

    cancel.cancel();
}
