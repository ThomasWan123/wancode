//! v0.19-2c 暗接线：Chat 层策略的真实执行物料（设计稿 v2 终审版）。
//!
//! 本模块只产「策略 → 引擎输入」的映射与守门判定，不做 IO 接线
//! （接线在 agent.rs 的 start_inner / agent_set_model）。
//!
//! 核心决策（评审定案）：
//! - 工具裁剪杠杆 = NewSessionRequest `_meta.agentProfile` 内联完整
//!   AgentDefinition：显式最小 tool_config（typed 构造，真实内部 ID
//!   `GrokBuild:web_search` / `GrokBuild:web_fetch`），
//!   inject_default_tools/agents_md/discover_skills 全关、
//!   mcp_servers=[]、mcp_inheritance=none。`tools` 字符串 allowlist
//!   不作主防线（引擎解析失败 fail-open）。
//! - **私有中性 cwd 是安全边界不是性能优化**：agents_md=false 后引擎
//!   builder 仍做 Git discover/gitignore 初始化——Chat 的 engine cwd
//!   必须是 app_data_dir()/chat-runtime/（非 git、无项目配置可读），
//!   另设 startupHints.skipGitStatus=true。
//! - **agent_type 冲突门（fail-closed 超集）**：G26 禁止新增
//!   xai-grok-agent 依赖（会改 Cargo.lock/清单哈希），引擎的
//!   is_strict_harness_agent_type 不可达——收紧为「Chat 仅允许
//!   agent_type 为空的模型」：任何 pin agent_type 的模型（strict 与否）
//!   都与 wancode-chat profile 冲突，一律结构化阻塞，绝不静默回落
//!   完整工具集。判定源 = config.toml `[model.X].agent_type`（与引擎
//!   同源）。

use serde::Serialize;

/// 新会话的层意图。**刻意不是 SurfaceKind**：恢复会话没有意图参数
/// （一切从 sidecar binding 派生），未来调用者无法借参数把已有会话
/// 重新归属。生产 agent_start 固定传 Code；Chat 仅测试/内部可达。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NewSurfaceIntent {
    Code,
    Chat,
}

impl NewSurfaceIntent {
    pub(crate) fn surface_kind(self) -> crate::surface::SurfaceKind {
        match self {
            NewSurfaceIntent::Code => crate::surface::SurfaceKind::Code,
            NewSurfaceIntent::Chat => crate::surface::SurfaceKind::Chat,
        }
    }
}

/// 策略执行层的结构化错误（serde tag=code，前端契约
/// `SURFACE_POLICY_BLOCKED: {json}`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum SurfacePolicyError {
    /// Chat 只允许 agent_type 为空的模型：pin 了 agent_type 的模型
    /// （strict harness 与否）会压过/对抗 `_meta.agentProfile`，
    /// 静默恢复完整工具集——fail-closed 超集，一律拒绝。
    AgentTypeConflict {
        model_id: String,
        agent_type: String,
    },
    /// Chat 恢复/切换时无法确定模型的 agent_type（配置读不到、
    /// 模型不在 config）——fail-closed。
    ModelUnresolvable { model_id: String, reason: String },
}

impl std::fmt::Display for SurfacePolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SurfacePolicyError::AgentTypeConflict { model_id, agent_type } => write!(
                f,
                "agent_type_conflict: 模型 {model_id} pin 了 agent_type={agent_type}，与 Chat 层 profile 冲突"
            ),
            SurfacePolicyError::ModelUnresolvable { model_id, reason } => {
                write!(f, "model_unresolvable: 模型 {model_id} 无法确定 agent_type：{reason}")
            }
        }
    }
}

/// 策略错误 → 前端契约字符串。
pub fn policy_blocked_message(e: &SurfacePolicyError) -> String {
    format!(
        "SURFACE_POLICY_BLOCKED: {}",
        serde_json::to_string(e).unwrap_or_else(|_| e.to_string())
    )
}

/// Chat 层的内联 AgentDefinition（`_meta.agentProfile` 载荷）。
///
/// typed 构造：工具条目来自 `ToolConfig::from(&WebSearchTool/&WebFetchTool)`
/// ——真实内部 ID（`GrokBuild:*`）由工具自己报告，裸字符串不是契约。
/// 形状由单测的 serialize → `AgentDefinition::from_json` 往返锁定。
pub(crate) fn chat_agent_profile() -> serde_json::Value {
    use xai_grok_tools::implementations::grok_build::{WebFetchTool, WebSearchTool};
    use xai_grok_tools::registry::types::ToolConfig;
    let web_search = ToolConfig::from(&WebSearchTool);
    let web_fetch = ToolConfig::from(&WebFetchTool::default());
    serde_json::json!({
        "name": "wancode-chat",
        "description": "WanCode Chat 层：轻对话，不访问用户/项目文件系统",
        // 显式最小 tool_config——主防线。
        "toolConfig": {
            "tools": [web_search, web_fetch],
        },
        // 关掉 session 级可选工具叠加（memory/lsp/image_gen/OpenCode
        // write 垫底/plan 工具等）——不叠加就没有渗漏面。
        "injectDefaultTools": false,
        // 不读 AGENTS.md/项目规则（私有 cwd 里本也没有，双保险）。
        "agentsMd": false,
        // 不做技能发现（技能可携带文件/执行指令面）。
        "discoverSkills": false,
        // 零自定义 MCP（第一版）：不带 server、不继承会话 MCP。
        "mcpServers": [],
        "mcpInheritance": "none",
        // 冗余兜底（非主防线）：denylist 常见文件/执行/版本控制工具。
        "disallowedTools": [
            "GrokBuild:run_terminal_cmd",
            "GrokBuild:read_file",
            "GrokBuild:write_file",
            "GrokBuild:search_replace",
            "GrokBuild:list_dir",
            "GrokBuild:grep",
        ],
    })
}

/// Chat 的 startupHints（`_meta.startupHints` 载荷）：跳过 git 状态
/// 注入。私有中性 cwd 才是边界，本 hint 只是降噪。
pub(crate) fn chat_startup_hints() -> serde_json::Value {
    serde_json::json!({ "skipGitStatus": true })
}

/// agent_type 冲突门：从 config.toml 文档判定模型是否可用于 Chat。
/// `doc` = toml_edit 解析后的用户配置；`model_id` = catalog key。
/// 返回 Ok(()) 仅当模型存在且未 pin agent_type。
pub(crate) fn ensure_chat_model_allowed(
    doc: &toml_edit::DocumentMut,
    model_id: &str,
) -> Result<(), SurfacePolicyError> {
    let entry = doc
        .get("model")
        .and_then(|m| m.as_table())
        .and_then(|t| t.get(model_id))
        .and_then(|e| e.as_table_like());
    let Some(entry) = entry else {
        return Err(SurfacePolicyError::ModelUnresolvable {
            model_id: model_id.to_string(),
            reason: "config.toml 无此模型条目".into(),
        });
    };
    match entry.get("agent_type").and_then(|v| v.as_str()) {
        None => Ok(()),
        Some(t) if t.trim().is_empty() => Ok(()),
        Some(t) => Err(SurfacePolicyError::AgentTypeConflict {
            model_id: model_id.to_string(),
            agent_type: t.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xai_grok_shell::agent::config::AgentDefinition;

    // 2c-1：`_meta.agentProfile` 形状锁定——serialize → 引擎
    // AgentDefinition::from_json 往返，防形状漂移（终审硬约束 A）。
    #[test]
    fn chat_profile_round_trips_through_engine_parser() {
        let profile = chat_agent_profile();
        let def = AgentDefinition::from_json(&profile)
            .expect("引擎必须能解析 Chat profile——解析失败即形状漂移");
        assert_eq!(def.name, "wancode-chat");
        // 主防线：显式最小 tool_config，真实内部 ID。
        let ids: Vec<&str> = def.tool_config.tools.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["GrokBuild:web_search", "GrokBuild:web_fetch"],
            "工具集必须精确等于 web_search+web_fetch（限定名）"
        );
        // 五关闭项。
        assert!(!def.inject_default_tools, "inject_default_tools 必须 false");
        assert!(!def.agents_md, "agents_md 必须 false");
        assert!(!def.discover_skills, "discover_skills 必须 false");
        assert!(def.mcp_servers.is_empty(), "零自定义 MCP");
        // McpInheritance 类型未经 shell re-export——用序列化形状断言
        // （camelCase 字段 mcpInheritance == "none"），与引擎解析同源。
        let back = serde_json::to_value(&def).expect("AgentDefinition 可序列化");
        assert_eq!(
            back.get("mcpInheritance").and_then(|v| v.as_str()),
            Some("none"),
            "MCP 继承必须 none"
        );
    }

    // 2c-2：typed 构造的 ID 来源正确性——ToolConfig::from(&Tool) 的 id
    // 与我们冗余 denylist 里的限定名风格一致（GrokBuild: 前缀）。
    #[test]
    fn typed_tool_ids_are_qualified() {
        use xai_grok_tools::implementations::grok_build::{WebFetchTool, WebSearchTool};
        use xai_grok_tools::registry::types::ToolConfig;
        assert_eq!(ToolConfig::from(&WebSearchTool).id, "GrokBuild:web_search");
        assert_eq!(
            ToolConfig::from(&WebFetchTool::default()).id,
            "GrokBuild:web_fetch"
        );
    }

    fn doc(text: &str) -> toml_edit::DocumentMut {
        text.parse().unwrap()
    }

    // 2c-3：agent_type 冲突门——三态判定（终审硬约束 B 的判定内核）。
    #[test]
    fn chat_model_gate_three_states() {
        // 无 agent_type：放行。
        let d = doc("[model.glm]\nname=\"GLM\"\nmodel=\"glm-5.2\"\n");
        ensure_chat_model_allowed(&d, "glm").expect("未 pin agent_type 须放行");
        // pin 了 agent_type（无论是否 strict）：结构化拒绝。
        let d = doc("[model.cdx]\nagent_type=\"codex\"\n");
        match ensure_chat_model_allowed(&d, "cdx") {
            Err(SurfacePolicyError::AgentTypeConflict { agent_type, .. }) => {
                assert_eq!(agent_type, "codex")
            }
            other => panic!("期望 AgentTypeConflict，得到 {other:?}"),
        }
        // 模型不存在：fail-closed。
        let d = doc("[model.glm]\n");
        assert!(matches!(
            ensure_chat_model_allowed(&d, "ghost"),
            Err(SurfacePolicyError::ModelUnresolvable { .. })
        ));
        // 契约串可解析。
        let e = SurfacePolicyError::AgentTypeConflict {
            model_id: "m".into(),
            agent_type: "codex".into(),
        };
        let msg = policy_blocked_message(&e);
        assert!(msg.starts_with("SURFACE_POLICY_BLOCKED: {"));
        assert!(msg.contains("\"code\":\"agent_type_conflict\""));
    }

    // 2c-4：startupHints 形状（skipGitStatus 为引擎 StartupHints 真实字段，
    // serde camelCase）。
    #[test]
    fn chat_startup_hints_shape() {
        assert_eq!(
            chat_startup_hints(),
            serde_json::json!({ "skipGitStatus": true })
        );
    }
}
