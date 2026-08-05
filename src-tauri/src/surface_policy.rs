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
// 临时：切片 2（start_inner/agent_set_model 接线）后移除。
#[allow(dead_code)]
pub(crate) enum NewSurfaceIntent {
    Code,
    Chat,
}

// 临时：切片 2（start_inner/agent_set_model 接线）后移除。
#[allow(dead_code)]
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
    /// 存在会对 Chat 会话生效的全局/plugin hooks（~/.grok/hooks、
    /// hooks-paths、Claude/Cursor 全局 hooks）。hooks 可在
    /// UserPromptSubmit/Stop 等事件执行命令、写任意路径，直接违反
    /// Chat 零文件/零执行承诺；私有 cwd 只隔项目 hooks 隔不了全局。
    /// 空 hooks 配置会退回磁盘 discovery，故必须启动前探测阻塞。
    GlobalHooksConflict { hook_count: usize, detail: String },
    /// 存在可能向引擎贡献 hooks/MCP 的插件输入面。G26 下引擎插件
    /// discovery 本体不可达，按输入面超集拦截（多拦不漏）。
    PluginExtensionsConflict { sources: Vec<String> },
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
            SurfacePolicyError::GlobalHooksConflict { hook_count, detail } => write!(
                f,
                "global_hooks_conflict: {hook_count} 个全局 hooks 会对 Chat 生效（{detail}）"
            ),
            SurfacePolicyError::PluginExtensionsConflict { sources } => write!(
                f,
                "plugin_extensions_conflict: {} 个插件输入面非空（{}）",
                sources.len(),
                sources.join("; ")
            ),
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
// 临时：切片 2（start_inner/agent_set_model 接线）后移除。
#[allow(dead_code)]
pub(crate) fn chat_agent_profile() -> serde_json::Value {
    use xai_grok_tools::implementations::grok_build::{WebFetchTool, WebSearchTool};
    use xai_grok_tools::registry::types::ToolConfig;
    let web_search = ToolConfig::from(&WebSearchTool);
    let web_fetch = ToolConfig::from(&WebFetchTool);
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
        // hosted tools 裁剪（复核 P0：不设 tools 时 AgentDefinition.tools
        // 为空 = 继承全部，引擎会把 x_search 等 hosted tools 视为允许）。
        // typed toolConfig 仍是函数工具主防线；这里专门约束 hosted 面。
        "tools": ["web_search", "web_fetch"],
        // 冗余兜底（非主防线）：hosted x_search + 常见文件/执行工具。
        "disallowedTools": [
            "x_search",
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
// 临时：切片 2（start_inner/agent_set_model 接线）后移除。
#[allow(dead_code)]
pub(crate) fn chat_startup_hints() -> serde_json::Value {
    serde_json::json!({ "skipGitStatus": true })
}

/// agent_type 冲突门：从 config.toml 文档判定模型是否可用于 Chat。
/// `doc` = toml_edit 解析后的用户配置；`model_id` = catalog key。
/// 返回 Ok(()) 仅当模型存在且未 pin agent_type。
// 临时：切片 2（start_inner/agent_set_model 接线）后移除。
#[allow(dead_code)]
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

/// 磁盘全局 hooks 门（复核三：**名实相符**——只覆盖磁盘全局 hooks：
/// ~/.grok/hooks、hooks-paths、Claude/Cursor 全局；plugin 携带的文件型/
/// inline hooks 由 [`ensure_no_plugin_extensions`] 在源头拦截）。
/// 用**引擎自己的 discovery**只读探测（git_root=None + untrusted ⇒ 仅
/// 全局来源），命中即阻塞；报错 fail-closed；空 hooks:{} 不可作覆盖
/// （会退回磁盘 discovery）。
///
/// compat 面（复核三 P1）：用 `CompatConfig::default()`——VendorCompat
/// 全字段 true（全 vendor 全面开启），是任何 compat_resolved（config +
/// remote settings 只会**关闭**面）的**超集**：门的发现面 ≥ 引擎实际
/// 加载面，零漏、只可能多拦。resolve_compat_config 为引擎私有且
/// remote settings 不可达（G26），复现 resolved 反而引入分歧窗口；
/// default=全开由测试 2c-7 钉死，上游翻转即红。
// 临时：切片 2（start_inner/agent_set_model 接线）后移除。
#[allow(dead_code)]
pub(crate) fn ensure_no_disk_global_hooks() -> Result<(), SurfacePolicyError> {
    let (registry, errors) = xai_grok_shell::util::hooks::discover_hooks(
        None,
        &xai_grok_tools::types::compat::CompatConfig::default(),
        false,
    );
    classify_global_hooks(registry.len(), &errors)
}

/// 纯判定内核（可测）：全局 hooks 数量/发现错误 → 门裁决。
fn classify_global_hooks(
    hook_count: usize,
    errors: &[impl std::fmt::Display],
) -> Result<(), SurfacePolicyError> {
    if !errors.is_empty() {
        return Err(SurfacePolicyError::GlobalHooksConflict {
            hook_count,
            detail: format!(
                "hooks discovery 报错 {} 条（fail-closed）：{}",
                errors.len(),
                errors.first().map(|e| e.to_string()).unwrap_or_default()
            ),
        });
    }
    if hook_count > 0 {
        return Err(SurfacePolicyError::GlobalHooksConflict {
            hook_count,
            detail: "全局/plugin hooks 会在 Chat 会话事件上执行命令".into(),
        });
    }
    Ok(())
}

/// 插件扩展门（复核四 P0-1/P0-3 定案：**方案 A 完整镜像**）。
///
/// G26 接口冲突记录：引擎插件 discovery 本体在 xai-grok-agent（新增
/// 依赖改 Cargo.lock、破清单哈希，G26 禁止），无法直接调用。本门
/// **完整镜像**引擎 discovery 的全部来源（xai-grok-agent
/// plugins/discovery.rs 文档序，锁定引擎 commit 下逐条对齐；漂移锁
/// 测试 2c-10 钉住引擎来源文件字节，引擎升级即红、强制重审本清单）：
///   1. config `[plugins].cli_plugin_dirs`
///   2. 项目 `.grok/plugins` / `.claude/plugins`（Chat 用私有中性
///      cwd，项目面天然为空——仍由 cwd 决策保证，不在本门枚举）
///   3. `$GROK_HOME/plugins/`
///   4. `~/.claude/plugins/`（目录非空即拦，天然涵盖其内的
///      installed_plugins.json 与 known_marketplaces.json 及其指向）
///   5. `~/.grok/installed-plugins/`（install registry）
///   6. config `[plugins].paths` 与 `[plugins].enabled`
///   7. `~/.claude/settings.json` enabledPlugins
///
/// 任一来源非空 ⇒ 阻塞 Chat。目录判定 = read_dir 有任意条目（超集，
/// 不解析 manifest）。**热重载安全性由此推出**：会话全期所有来源为
/// 空 ⇒ reload 无物可载（方案 A 的生命周期论证）。
///
/// 根目录可注入（判别测试用）；生产包装取真实 grok_home 与 ~/.claude。
// 临时：切片 2（start_inner/agent_set_model 接线）后移除。
#[allow(dead_code)]
pub(crate) fn ensure_no_plugin_extensions(
    doc: &toml_edit::DocumentMut,
) -> Result<(), SurfacePolicyError> {
    let grok_home = xai_grok_shell::util::grok_home::grok_home();
    let claude_home = std::env::var_os("USERPROFILE")
        .map(std::path::PathBuf::from)
        .map(|h| h.join(".claude"));
    scan_plugin_sources(doc, &grok_home, claude_home.as_deref())
}

/// 判定内核（可注入根，判别测试直击）。
fn scan_plugin_sources(
    doc: &toml_edit::DocumentMut,
    grok_home: &std::path::Path,
    claude_home: Option<&std::path::Path>,
) -> Result<(), SurfacePolicyError> {
    let mut sources = Vec::new();
    // 来源 1/6：config [plugins] 三数组。
    if let Some(p) = doc.get("plugins").and_then(|v| v.as_table_like()) {
        for key in ["enabled", "paths", "cli_plugin_dirs"] {
            let non_empty = p
                .get(key)
                .and_then(|v| v.as_array())
                .is_some_and(|a| !a.is_empty());
            if non_empty {
                sources.push(format!("config.toml [plugins].{key}"));
            }
        }
    }
    // 来源 3/5：$GROK_HOME/plugins 与 install registry。
    let dir_non_empty = |p: &std::path::Path| {
        std::fs::read_dir(p).is_ok_and(|mut d| d.next().is_some())
    };
    if dir_non_empty(&grok_home.join("plugins")) {
        sources.push("$GROK_HOME/plugins".into());
    }
    if dir_non_empty(&grok_home.join("installed-plugins")) {
        sources.push("$GROK_HOME/installed-plugins（install registry）".into());
    }
    // 来源 4/7：~/.claude 面。
    if let Some(claude) = claude_home {
        if dir_non_empty(&claude.join("plugins")) {
            sources.push("~/.claude/plugins".into());
        }
        if let Ok(text) = std::fs::read_to_string(claude.join("settings.json")) {
            let enabled_non_empty = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| {
                    v.get("enabledPlugins")
                        .map(|e| e.as_object().is_some_and(|o| !o.is_empty()))
                })
                .unwrap_or(false);
            if enabled_non_empty {
                sources.push("~/.claude/settings.json enabledPlugins".into());
            }
        }
    }
    if sources.is_empty() {
        Ok(())
    } else {
        Err(SurfacePolicyError::PluginExtensionsConflict { sources })
    }
}

/// Chat 的 managed MCP 关闭（复核四 P0-2 定案）：**会话实例级**——
/// wancode 自建 AgentConfig（agent.rs new_from_toml_cfg →
/// resolve_runtime_fields → spawn_grok_shell），Chat 分支在 resolve
/// 后直接置 `managed_mcps_enabled=false` 与
/// `managed_mcp_gateway_tools_enabled=false`，只约束本次 spawn，
/// 不触进程环境（env 方案有 Chat/Code 并发串线窗口，已否决删除）。
/// mcp_servers=[] 不等于零 MCP：managed/plugin/热重载 MCP 都在会话
/// 建立后另行合并——managed 由本覆盖关死，plugin 由插件门源头拦截。
// 临时：切片 2（start_inner 接线）后移除。
#[allow(dead_code)]
pub(crate) fn apply_chat_agent_config_overrides(
    cfg: &mut xai_grok_shell::agent::config::Config,
) {
    cfg.managed_mcps_enabled = false;
    cfg.managed_mcp_gateway_tools_enabled = false;
}

#[cfg(test)]
mod tests {
    use super::*;
    use xai_grok_shell::agent::config::AgentDefinition;

    /// 2c-10 漂移锁基线：锁定引擎 commit 63d4edab 下三个 discovery
    /// 来源文件的联合 sha256。引擎升级后按测试指引重审并更新。
    const PLUGIN_DISCOVERY_SOURCES_SHA256: &str =
        "2bf7f4066dc5590ee5a5a1b2de10af2a43a3ab3b9e20f3ea3161af7a12e1091b";

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
        // hosted tools allowlist（复核 P0：空 = 继承全部 = x_search 漏入）。
        assert_eq!(
            def.tools,
            vec!["web_search".to_string(), "web_fetch".to_string()],
            "AgentDefinition.tools 必须精确 = web_search+web_fetch（hosted 面）"
        );
        assert!(
            def.disallowed_tools.contains(&"x_search".to_string()),
            "x_search 必须在 denylist"
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
            ToolConfig::from(&WebFetchTool).id,
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

    // 2c-5：全局 hooks 门判定内核（复核 P0-2）——三态：零 hooks 放行、
    // 任意 hooks 阻塞、discovery 报错 fail-closed 阻塞。
    #[test]
    fn global_hooks_gate_three_states() {
        let no_errs: &[String] = &[];
        classify_global_hooks(0, no_errs).expect("零全局 hooks 放行");
        match classify_global_hooks(2, no_errs) {
            Err(SurfacePolicyError::GlobalHooksConflict { hook_count, .. }) => {
                assert_eq!(hook_count, 2)
            }
            other => panic!("期望 GlobalHooksConflict，得到 {other:?}"),
        }
        // discovery 报错：即使计数为 0 也 fail-closed。
        let errs = vec!["hook file unreadable".to_string()];
        assert!(matches!(
            classify_global_hooks(0, &errs),
            Err(SurfacePolicyError::GlobalHooksConflict { .. })
        ));
        // 契约串。
        let e = SurfacePolicyError::GlobalHooksConflict {
            hook_count: 1,
            detail: "d".into(),
        };
        assert!(policy_blocked_message(&e).contains("\"code\":\"global_hooks_conflict\""));
    }

    // 2c-6：生产探测函数在本机可执行（结果依机器状态而定，只断言
    // 不 panic 且错误为结构化类型——真实门行为由 CI 干净环境与
    // 后续请求体测试覆盖）。
    #[test]
    fn global_hooks_probe_runs() {
        match ensure_no_disk_global_hooks() {
            Ok(()) => {}
            Err(SurfacePolicyError::GlobalHooksConflict { .. }) => {}
            Err(other) => panic!("探测只应产生 GlobalHooksConflict，得到 {other:?}"),
        }
    }

    // 2c-7：compat 超集面钉死——CompatConfig::default() 的 vendor hooks
    // 面必须全 true（探测面 ≥ 引擎 resolved 面的前提）。上游翻转默认值
    // 本测试即红，届时改为显式全开构造。
    #[test]
    fn default_compat_is_superset_for_hooks() {
        let c = xai_grok_tools::types::compat::CompatConfig::default();
        assert!(c.claude.hooks, "claude hooks 面必须默认开启（超集前提）");
        assert!(c.cursor.hooks, "cursor hooks 面必须默认开启（超集前提）");
    }

    // 2c-8：插件扩展门——config 输入面三态（claude 磁盘面依机器状态，
    // 干净环境行为由 CI 覆盖；此处注入 config 面）。
    #[test]
    fn plugin_extensions_gate_on_config_inputs() {
        let d = doc("[model.glm]\n");
        match ensure_no_plugin_extensions(&d) {
            Ok(()) => {}
            Err(SurfacePolicyError::PluginExtensionsConflict { sources }) => {
                assert!(sources.iter().all(|s| s.contains(".claude")));
            }
            Err(other) => panic!("非法错误 {other:?}"),
        }
        let d = doc("[plugins]\nenabled=[\"foo\"]\n");
        match ensure_no_plugin_extensions(&d) {
            Err(SurfacePolicyError::PluginExtensionsConflict { sources }) => {
                assert!(sources.iter().any(|s| s.contains("[plugins].enabled")));
            }
            other => panic!("期望 PluginExtensionsConflict，得到 {other:?}"),
        }
        let d = doc("[plugins]\npaths=[\"/x\"]\n");
        assert!(matches!(
            ensure_no_plugin_extensions(&d),
            Err(SurfacePolicyError::PluginExtensionsConflict { .. })
        ));
        let e = SurfacePolicyError::PluginExtensionsConflict { sources: vec!["s".into()] };
        assert!(policy_blocked_message(&e).contains("\"code\":\"plugin_extensions_conflict\""));
    }

    // 2c-9（复核四判别①②）：config 为空时，仅 $GROK_HOME/plugins 有
    // 插件 → 必须阻断；仅 install registry（installed-plugins/）有
    // 插件 → 必须阻断；全部来源为空 → 放行。
    #[test]
    fn plugin_gate_blocks_disk_sources_with_empty_config() {
        let d = doc("[model.glm]\n");
        let tmp = tempfile::tempdir().unwrap();
        let grok = tmp.path().join("grok-home");
        std::fs::create_dir_all(&grok).unwrap();
        // 全空：放行（claude_home 注入空目录，隔离本机状态）。
        let claude = tmp.path().join("claude-home");
        std::fs::create_dir_all(&claude).unwrap();
        scan_plugin_sources(&d, &grok, Some(&claude)).expect("全部来源为空须放行");
        // 仅 $GROK_HOME/plugins 有一个插件目录：阻断。
        std::fs::create_dir_all(grok.join("plugins").join("evil-plugin")).unwrap();
        match scan_plugin_sources(&d, &grok, Some(&claude)) {
            Err(SurfacePolicyError::PluginExtensionsConflict { sources }) => {
                assert!(sources.iter().any(|s| s.contains("$GROK_HOME/plugins")));
            }
            other => panic!("期望阻断，得到 {other:?}"),
        }
        std::fs::remove_dir_all(grok.join("plugins")).unwrap();
        // 仅 install registry 有条目：阻断。
        std::fs::create_dir_all(grok.join("installed-plugins").join("mk-plugin")).unwrap();
        assert!(matches!(
            scan_plugin_sources(&d, &grok, Some(&claude)),
            Err(SurfacePolicyError::PluginExtensionsConflict { .. })
        ));
        std::fs::remove_dir_all(grok.join("installed-plugins")).unwrap();
        // 仅 ~/.claude/plugins 目录非空（涵盖 installed_plugins.json /
        // known_marketplaces.json 任何形态）：阻断。
        std::fs::create_dir_all(claude.join("plugins")).unwrap();
        std::fs::write(claude.join("plugins").join("installed_plugins.json"), "{}").unwrap();
        assert!(matches!(
            scan_plugin_sources(&d, &grok, Some(&claude)),
            Err(SurfacePolicyError::PluginExtensionsConflict { .. })
        ));
    }

    // 2c-10（复核四 P0-3 漂移锁）：镜像清单对齐的引擎 discovery 来源
    // 文件字节哈希。引擎升级（lock bump）改动这些文件时本测试必红，
    // 强制重审 scan_plugin_sources 的来源清单后更新哈希。
    #[test]
    fn plugin_discovery_source_drift_lock() {
        use sha2::{Digest, Sha256};
        let engine = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../grok-build/crates/codegen/xai-grok-agent/src/plugins");
        let mut hasher = Sha256::new();
        for f in ["discovery.rs", "marketplace.rs", "install_registry.rs"] {
            let bytes = std::fs::read(engine.join(f))
                .unwrap_or_else(|e| panic!("读引擎 {f} 失败：{e}"));
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(&bytes);
        }
        let digest = format!("{:x}", hasher.finalize());
        assert_eq!(
            digest, PLUGIN_DISCOVERY_SOURCES_SHA256,
            "引擎插件 discovery 来源文件变了：重审 scan_plugin_sources \
             镜像清单（discovery.rs 文档的来源序），确认后更新此哈希"
        );
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
