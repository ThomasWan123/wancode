//! v0.20 W2.5：Work 层**真实引擎**验证（codex issue #47 裁断）。
//!
//! 为什么需要这条测试：W2 的全部证据都是单测/lint，而 PR #46 R2 证明了
//! 这类证据**看不见一整类失败**——当时的「零工具」Work 档会让引擎在
//! 构建 agent 时直接 `InvalidConfig`，每个 Work 会话都起不来，而断言
//! JSON 形状的单测全绿。`ToolConfig` 只是 **id 引用**（不含内联定义），
//! 所以 `GrokBuild:todo_write` 还必须能在注册表里解析——同样只有真实
//! 引擎能证明。
//!
//! 做法与 `model_block_over_acp.rs` 同源：隔离 `$GROK_HOME`、模型指向
//! 不存在的本地端口、`spawn_grok_shell` 起进程内真实引擎、走
//! initialize/authenticate/newSession——即 `agent.rs::start_session` 的
//! 序列。本测试**不发模型请求**（只看会话构造期的判定），因此不需要
//! 任何真实 Key、不产生外网流量，可进 PR CI。
//!
//! 覆盖（codex #47 required scope）：
//!   ① Work profile 在真实引擎里能构建会话（#46 R2 那一类失败的正面证据）；
//!   ⑤ **负控制**：无效 curated tool id 必须让同一边界失败——不证明探针
//!      能看见失败，绿灯就没有说服力；
//!   ⑥ 能力面精确断言：Work 会话的 `x.ai/mcp/list` 为空，而同一配置下的
//!      Code 会话能看见 canary MCP（正对照）。
//!
//! 未覆盖（本文件范围外，见 PR 说明）：binding 读回、导入、恢复对立意图、
//! 失败清理——那些走 wancode 自有层，不需要引擎，见 `src/work_seams.rs`。
//!
//! **不链接 `wancode_lib`，改用 `#[path]` 把生产源文件编进本 crate**——与
//! `job_breakaway` 同款理由：引擎 workspace 的 `[profile.dev] panic = "abort"`
//! 与 cargo 强制测试目标 unwind 冲突，链 lib 会编译失败（实测 773 条
//! panic-strategy 错误）。测的仍是**逐字同一实现**：`surface_profiles.rs`
//! 就是生产路径 `agent.rs` 用的那份档。
//!
//! **`harness = false`**：`GROK_HOME` 经引擎 `grok_home()` 的 OnceLock 解析，
//! 一个进程只认第一次；独立进程才能保证隔离真正生效（否则同进程里别的测试
//! 先触发解析，`set_var` 静默失效，引擎会落到开发者真实 `~/.grok`）。

#[path = "../src/surface_profiles.rs"]
mod surface_profiles;

use agent_client_protocol as acp;
use tokio_util::sync::CancellationToken;
use xai_acp_lib::{acp_send, AcpAgentTx};
use xai_grok_pager::acp::spawn::spawn_grok_shell;
use xai_grok_shell::agent::auth_method::AuthMethodKind;
use xai_grok_shell::agent::config::Config as AgentConfig;

/// 模型指向不存在的本地端口：本测试只验证会话构造，不发任何模型请求。
const CONFIG: &str = r#"
[model.canary-model]
name = "模拟·仅用于会话构造"
model = "canary-slug"
base_url = "http://127.0.0.1:34191/v1"
api_key = "key-not-used"
api_backend = "chat_completions"
context_window = 128000
"#;

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

/// 与 `agent.rs` 生产路径同形的 Work 会话 meta：**生产 profile 函数本体**
/// （不复制一份 JSON——复制出来的档测不到生产代码漂移）。
fn work_session_meta() -> serde_json::Map<String, serde_json::Value> {
    // 生产档本体（经 #[path] 编进本 crate，非复制品）。
    let profile = surface_profiles::work_agent_profile().to_string();
    serde_json::from_str(&format!(
        r#"{{"agentProfile":{profile},"x.ai/localExtensionsDisabled":true}}"#
    ))
    .expect("Work session meta 应为合法 JSON")
}

/// 起一个隔离夹具：返回 (tmpdir, grok_home, cwd)。
/// `GROK_HOME` 是 OnceLock——一个进程只认第一次，必须在任何东西碰它之前设好。
fn isolated_fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let grok_home = tmp.path().join(".grok");
    let cwd = tmp.path().join("work-staging");
    std::fs::create_dir_all(&grok_home).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::write(grok_home.join("config.toml"), CONFIG).unwrap();
    unsafe {
        std::env::set_var("GROK_HOME", &grok_home);
    }
    (tmp, cwd)
}

/// ① + ⑥：Work profile 在真实引擎里能构建会话，且该会话零 MCP。
///
/// 这是 #46 R2 那一类失败的直接证据：若 profile 不被引擎接受（空工具集）、
/// 或 `GrokBuild:todo_write` 在注册表里解析不到，newSession 会失败。
async fn work_profile_builds_a_real_session_with_zero_mcp() {
    let (_tmp, cwd) = isolated_fixture();
    let cancel = CancellationToken::new();
    let acp_tx = spawn_authenticated(agent_config_for(&cwd), &cancel).await;

    // 与生产一致：空 mcp_servers + Work agentProfile + 本地扩展隔离。
    let resp: acp::NewSessionResponse = acp_send(
        acp::NewSessionRequest::new(cwd.clone())
            .mcp_servers(Vec::new())
            .meta(Some(work_session_meta())),
        &acp_tx,
    )
    .await
    .expect("Work profile 必须能在真实引擎里构建会话——失败即 #46 R2 那类缺陷");

    // R3 握手：引擎必须确认已应用本地扩展隔离（生产在此 fail-closed）。
    let applied = resp
        .meta
        .as_ref()
        .and_then(|m| m.get("localExtensionsDisabledApplied"))
        .and_then(serde_json::Value::as_bool);
    assert_eq!(
        applied,
        Some(true),
        "引擎必须确认已应用本地扩展隔离；生产路径正是据此 fail-closed"
    );

    // ⑥ 能力面：Work 会话的 MCP 列表必须为空。
    // 与生产 ext_call 同形：RawValue 参数、双写 sessionId/session_id。
    let sid = resp.session_id.0.to_string();
    let raw = serde_json::value::to_raw_value(&serde_json::json!({
        "sessionId": sid, "session_id": sid,
    }))
    .expect("static json");
    let ext: acp::ExtResponse = acp_send(
        acp::ExtRequest::new("x.ai/mcp/list".to_string(), raw.into()),
        &acp_tx,
    )
    .await
    .expect("x.ai/mcp/list 必须可用——Work 的零 MCP 断言依赖它");
    let mcp: serde_json::Value =
        serde_json::from_str(ext.0.get()).expect("mcp/list 响应应为合法 JSON");
    let servers = mcp
        .get("servers")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    assert_eq!(servers, 0, "默认 Work 会话必须零 MCP，实得 {mcp:?}");

    // ── ⑤ 负控制（同一引擎、同一边界）─────────────────────────────
    // 证明本探针**看得见** #46 R2 那一类失败：把工具 id 换成注册表里不存在
    // 的那个，同一 newSession 边界必须失败。若这条也「通过」，说明探针根本
    // 没在检验 profile 构建，上面那条绿灯就不构成证据。
    // 放在同一个测试函数里：GROK_HOME 是进程级 OnceLock，分成两个 #[test]
    // 会互相污染夹具。
    // 与 Work 档同形，只把工具 id 换成注册表里不存在的那个。
    let bogus_meta = serde_json::json!({
        "agentProfile": {
            "name": "wancode-work-negative-control",
            "description": "负控制：未注册的 curated tool id",
            "toolConfig": { "tools": [ { "id": "GrokBuild:definitely_not_a_registered_tool" } ] },
            "injectDefaultTools": false,
            "agentsMd": false,
            "discoverSkills": false,
            "mcpServers": [],
            "mcpInheritance": "none",
            "tools": [],
        },
        "x.ai/localExtensionsDisabled": true,
    })
    .as_object()
    .cloned()
    .unwrap();

    let result: Result<acp::NewSessionResponse, _> = acp_send(
        acp::NewSessionRequest::new(cwd.clone())
            .mcp_servers(Vec::new())
            .meta(Some(bogus_meta)),
        &acp_tx,
    )
    .await;

    let err = result.as_ref().err().map(|e| e.to_string()).unwrap_or_default();
    assert!(
        result.is_err(),
        "未注册的 curated tool id 必须让会话构造失败——否则本探针看不见 \
         profile/注册表类失败，正向用例的绿灯就不成立"
    );
    // 锁住**失败原因**：必须是 agent 构建期的注册表解析失败，而不是任何别的
    // 错误——否则负控制「通过」了却证明不了探针在检验什么。实测引擎原文：
    //   agent building failed: tool error: Requirements unsatisfied:
    //   [... "not found in registry" ... category: "tool_not_found"]
    assert!(
        err.contains("agent building failed") && err.contains("not found in registry"),
        "负控制必须因 agent 构建期注册表解析失败而失败，实得: {err}"
    );


    cancel.cancel();
}

/// `harness = false`：自写入口。断言失败即 panic → 非零退出 → cargo 判失败。
fn main() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(work_profile_builds_a_real_session_with_zero_mcp());
    println!("WORK ENGINE PROBE PASS");
}
