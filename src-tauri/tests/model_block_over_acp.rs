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
use xai_acp_lib::{AcpAgentTx, acp_send};
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

/// 与 `glm-4.6` 不同家族的唯一剩余模型。恢复旧会话时不能静默切到它；
/// 必须保留历史身份并把 `model_unavailable` 交给客户端。
const UNRELATED_ONLY_CONFIG: &str = r#"
[model.grok-build-solo]
name = "模拟·无关模型"
model = "solo-slug"
base_url = "http://127.0.0.1:34103/v1"
api_key = "key-for-solo"
api_backend = "chat_completions"
context_window = 128000
"#;

const SESSION_ID: &str = "acp-legacy-session";

fn agent_config_for(cwd: &std::path::Path) -> AgentConfig {
    let raw_config = xai_grok_shell::config::load_effective_config().unwrap();
    let mut agent_config = AgentConfig::new_from_toml_cfg(&raw_config).unwrap();
    agent_config.resolve_runtime_fields(&xai_grok_shell::agent::config::RuntimeResolutionContext {
        raw_config: &raw_config,
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
    agent_config.mode = xai_grok_shell::agent::config::AgentMode::Headless;
    agent_config.default_yolo_mode = false;
    agent_config
}

async fn spawn_authenticated(config: AgentConfig, cancel: &CancellationToken) -> AcpAgentTx {
    let memory_config = config.memory_config.clone();
    let spawned = spawn_grok_shell(config, cancel, memory_config)
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
    acp_tx
}

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
async fn acp_load_block_and_new_session_identity_are_preserved() {
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

    // 桌面端侧栏调用的就是这条合并列表。先锁住“真实落盘的旧会话能被
    // 当前工作区发现”，否则后续 GUI 冒烟可能只是打开了一个根本不在
    // 列表里的夹具，点击流程全部空跑。
    let listed =
        xai_grok_shell::session::merge::fetch_merged(None, Some(&cwd.to_string_lossy()), None, 30)
            .await;
    assert!(
        listed.iter().any(|s| s.session_id == SESSION_ID),
        "旧格式会话必须出现在桌面端使用的工作区列表中：{listed:?}"
    );

    // ── 与 agent.rs::start_session 同一套启动序列 ───────────────────
    let cancel = CancellationToken::new();
    let acp_tx = spawn_authenticated(agent_config_for(&cwd), &cancel).await;

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

    // 用户从歧义选择器选定 Coding Plan 后，同一会话再次恢复必须把
    // canonical key 放进 LoadSessionResponse.models。否则真实采样已经走
    // glm-coding，桌面下拉却仍显示启动默认的 glm-open。
    let _: acp::SetSessionModelResponse = acp_send(
        acp::SetSessionModelRequest::new(
            acp::SessionId::new(SESSION_ID),
            acp::ModelId::new("glm-coding"),
        ),
        &acp_tx,
    )
    .await
    .expect("选择精确 catalog key");
    let restored: acp::LoadSessionResponse = acp_send(
        acp::LoadSessionRequest::new(
            acp::SessionId::new(SESSION_ID),
            PathBuf::from(cwd.to_string_lossy().to_string()),
        ),
        &acp_tx,
    )
    .await
    .expect("写回身份后再次恢复");
    assert!(
        restored
            .meta
            .as_ref()
            .and_then(|m| m.get("x.ai/modelBlock"))
            .is_none(),
        "精确身份写回后不应再次歧义"
    );
    assert_eq!(
        restored
            .models
            .as_ref()
            .map(|m| m.current_model_id.0.as_ref()),
        Some("glm-coding"),
        "恢复响应必须与真实采样路由使用同一个 canonical key"
    );

    // ── 同一隔离引擎里再建一个默认模型会话 ─────────────────────────
    // `GROK_HOME` 是进程级 OnceLock，因此把 legacy 恢复与默认首写放在同
    // 一个测试/夹具中，既避免并发污染，也让 CI 真正执行这条回归而非 ignore。
    let new_resp: acp::NewSessionResponse = acp_send(
        acp::NewSessionRequest::new(PathBuf::from(cwd.to_string_lossy().to_string())),
        &acp_tx,
    )
    .await
    .expect("new session");

    // 直接读落盘产物。这里曾先正确写入 slug+key，随后 session actor 的
    // CurrentModel 初始化消息又用 runtime key 覆盖 current 并清掉 catalog。
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
        "current_model_id 必须是上游 slug；实际落盘：{raw}"
    );
    assert_eq!(
        json["catalog_model_id"], "glm-coding",
        "新会话应原子写入当时的 canonical key，否则崩溃窗口还在。实际落盘：{raw}"
    );

    cancel.cancel();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // 真正复现桌面重启：第一套引擎完全退出，再从同一磁盘记录启动一套
    // 全新的引擎。热恢复正确而冷恢复回到默认模型，界面仍会撒谎。
    let cold_cancel = CancellationToken::new();
    let cold_tx = spawn_authenticated(agent_config_for(&cwd), &cold_cancel).await;
    let cold_restored: acp::LoadSessionResponse = acp_send(
        acp::LoadSessionRequest::new(
            acp::SessionId::new(SESSION_ID),
            PathBuf::from(cwd.to_string_lossy().to_string()),
        ),
        &cold_tx,
    )
    .await
    .expect("全新引擎冷恢复");
    assert_eq!(
        cold_restored
            .models
            .as_ref()
            .map(|m| m.current_model_id.0.as_ref()),
        Some("glm-coding"),
        "冷恢复响应必须返回已持久化的 canonical key"
    );
    cold_cancel.cancel();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // ── 原模型被删除且只剩不同家族模型 ─────────────────────────────
    // 此时不能把唯一剩余条目当作“方便的默认值”静默写回。历史仍可读，
    // 发送保持阻塞，直到用户明确从下拉选择另一个模型。
    std::fs::write(grok_home.join("config.toml"), UNRELATED_ONLY_CONFIG).unwrap();
    let unavailable_cancel = CancellationToken::new();
    let unavailable_tx =
        spawn_authenticated(agent_config_for(&cwd), &unavailable_cancel).await;
    let unavailable: acp::LoadSessionResponse = acp_send(
        acp::LoadSessionRequest::new(
            acp::SessionId::new(SESSION_ID),
            PathBuf::from(cwd.to_string_lossy().to_string()),
        ),
        &unavailable_tx,
    )
    .await
    .expect("原模型已删除时历史仍应可读");

    let unavailable_block = unavailable
        .meta
        .as_ref()
        .and_then(|m| m.get("x.ai/modelBlock"))
        .expect("不得静默切到无关 fallback；必须返回 model_unavailable");
    assert_eq!(
        unavailable_block.get("kind").and_then(|k| k.as_str()),
        Some("model_unavailable")
    );
    assert_eq!(
        unavailable_block.get("requested").and_then(|r| r.as_str()),
        Some("glm-4.6")
    );

    let legacy_dir = xai_grok_shell::session::persistence::session_dir(
        &xai_grok_shell::session::info::Info {
            id: acp::SessionId::new(SESSION_ID),
            cwd: cwd.to_string_lossy().into_owned(),
        },
    );
    let after_load =
        std::fs::read_to_string(legacy_dir.join("summary.json")).expect("旧会话仍应存在");
    let after_load_json: serde_json::Value = serde_json::from_str(&after_load).unwrap();
    assert_eq!(
        after_load_json["current_model_id"], "glm-4.6",
        "阻塞恢复不得把无关 fallback 的 slug 写进历史身份：{after_load}"
    );
    assert_eq!(
        after_load_json["catalog_model_id"], "glm-coding",
        "阻塞恢复不得把无关 fallback 的 key 写进历史身份：{after_load}"
    );
    unavailable_cancel.cancel();
}
