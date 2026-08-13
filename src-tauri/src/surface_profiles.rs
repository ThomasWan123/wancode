//! Chat / Work 层的内联 `agentProfile`（`_meta.agentProfile` 载荷）。
//!
//! 独立成文件的原因（W2.5）：这两个构造器**只依赖外部 crate**，不引用任何
//! `crate::` 项，因此可以被 `tests/work_surface_engine.rs` 用 `#[path]` 直接
//! 编进测试 crate——那条探针要用**生产档本体**去真实引擎里建会话，而链接
//! `wancode_lib` 会因引擎 workspace 的 `[profile.dev] panic = "abort"` 与
//! cargo 强制测试目标 unwind 冲突而编译失败（与 `job_breakaway` 同款做法：
//! 测的仍是逐字同一实现）。

/// Chat 层的内联 AgentDefinition（`_meta.agentProfile` 载荷）。
///
/// typed 构造：工具条目来自 `ToolConfig::from(&WebSearchTool/&WebFetchTool)`
/// ——真实内部 ID（`GrokBuild:*`）由工具自己报告，裸字符串不是契约。
/// 形状由单测的 serialize → `AgentDefinition::from_json` 往返锁定。
pub fn chat_agent_profile() -> serde_json::Value {
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
/// Work 层 agentProfile（`_meta.agentProfile`）：文档工作台，**零代码执行、
/// 默认无联网/MCP**（设计 §1 D8 一期 MVP）。比 Chat 更严——连 web 也不注入
/// （默认 Work 会话工具 schema 缺席；联网 Work 会话是未来的显式 opt-in）。
/// 文档读取/检索由 W3 的锚点级检索提供，不是裸文件工具。codex W2-fe-b R1：
/// 之前 Work 会 fall through 到 Code 能力档（全工具 + 配置 MCP），违反边界。
pub fn work_agent_profile() -> serde_json::Value {
    use xai_grok_tools::implementations::grok_build::TodoWriteTool;
    use xai_grok_tools::registry::types::ToolConfig;
    // 引擎硬约束（codex W2-fe-b R2，xai-grok-agent/src/builder.rs:686）：
    // `!inject_default_tools && tool_config.tools.is_empty()` → InvalidConfig，
    // 即**空工具集 + 不注入默认工具**这一组合无法构建 agent。因此不能用
    // 「零工具」表达零能力，必须给一个**零能力面**的工具：todo_write 只操作
    // 会话内存状态（Resources serde），无文件系统、无网络、无进程执行。
    // 联网/代码执行/MCP 依旧全部缺席——能力边界由「有哪些工具」决定，
    // 而不是「有没有工具」。W3 的文档检索工具落地后在此加入。
    let todo = ToolConfig::from(&TodoWriteTool);
    serde_json::json!({
        "name": "wancode-work",
        "description": "WanCode Work 层：文档工作台，零代码执行、默认无联网/MCP",
        // 显式最小 tool_config——主防线：仅零能力面的 todo_write。
        "toolConfig": { "tools": [todo] },
        "injectDefaultTools": false,
        "agentsMd": false,
        "discoverSkills": false,
        "mcpServers": [],
        "mcpInheritance": "none",
        // hosted tools 全裁剪：默认 Work 无联网、无代码执行。
        "tools": [],
        "disallowedTools": [
            "x_search",
            "web_search",
            "web_fetch",
            "GrokBuild:run_terminal_cmd",
            "GrokBuild:read_file",
            "GrokBuild:write_file",
            "GrokBuild:search_replace",
            "GrokBuild:list_dir",
            "GrokBuild:grep",
        ],
    })
}
