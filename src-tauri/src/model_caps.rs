//! #127 能力模型与统一解析器（v0.18.9 兼容性治理第 1 步）。
//!
//! 三个概念分开，不许混淆（复核定案）：
//!   - **固有能力**（本模块）：模型自身能做什么——text / tool_use /
//!     vision_input / reasoning。三态：supported / unsupported / unknown，
//!     unknown 不是"默认支持"（fail-visible）。
//!   - **路由角色**：模型被指派承担什么（如 image_description 视觉辅助）
//!     ——那是配置/引擎路由层的事，不是能力字段。
//!   - **数值配置**：context_window 等已有数值语义，不做成布尔能力。
//!
//! 解析优先级：用户显式配置 > 内置能力表 > unknown。
//! 内置表按**上游 slug + endpoint 类型**匹配——绝不按 catalog key
//! （key 可被用户任意重命名）。结果附带来源（config / built_in / unknown）。
//!
//! 设置页（provider_ops::model_list）与聊天目录（ACP ModelOption 链）
//! 必须都经由本解析器取能力，杜绝双列表漂移（v0.18.7-B 的教训）。

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapState {
    Supported,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapSource {
    /// 用户在 config.toml [model.X.capabilities] 显式声明。
    Config,
    /// 内置能力表按 slug + endpoint 匹配。
    BuiltIn,
    /// 无任何依据。
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Cap {
    pub state: CapState,
    pub source: CapSource,
}

impl Cap {
    const UNKNOWN: Cap = Cap {
        state: CapState::Unknown,
        source: CapSource::Unknown,
    };
    fn built_in(supported: bool) -> Cap {
        Cap {
            state: if supported {
                CapState::Supported
            } else {
                CapState::Unsupported
            },
            source: CapSource::BuiltIn,
        }
    }
    fn config(supported: bool) -> Cap {
        Cap {
            state: if supported {
                CapState::Supported
            } else {
                CapState::Unsupported
            },
            source: CapSource::Config,
        }
    }
}

/// 一个模型的固有能力集。字段名即对外（serde）契约。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ModelCaps {
    pub text: Cap,
    pub tool_use: Cap,
    pub vision_input: Cap,
    pub reasoning: Cap,
}

impl ModelCaps {
    const UNKNOWN: ModelCaps = ModelCaps {
        text: Cap::UNKNOWN,
        tool_use: Cap::UNKNOWN,
        vision_input: Cap::UNKNOWN,
        reasoning: Cap::UNKNOWN,
    };
}

/// config.toml [model.X.capabilities] 的原始覆盖（每项可缺省）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CapOverrides {
    pub text: Option<bool>,
    pub tool_use: Option<bool>,
    pub vision_input: Option<bool>,
    pub reasoning: Option<bool>,
}

impl CapOverrides {
    /// 从模型条目的 toml 表读取 capabilities 子表；子表缺失 → 全 None
    /// （旧配置零迁移）。非布尔值按缺省处理（宽容读取，写入端另行校验）。
    pub fn from_toml(item: &dyn toml_edit::TableLike) -> CapOverrides {
        let Some(caps) = item.get("capabilities").and_then(|v| v.as_table_like()) else {
            return CapOverrides::default();
        };
        let get = |k: &str| caps.get(k).and_then(|v| v.as_bool());
        CapOverrides {
            text: get("text"),
            tool_use: get("tool_use"),
            vision_input: get("vision_input"),
            reasoning: get("reasoning"),
        }
    }
}

/// endpoint 归类：内置表的匹配维度之一（slug 在不同服务商可能撞名）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provider {
    Zhipu,
    DeepSeek,
    Other,
}

fn provider_of(base_url: &str) -> Provider {
    let u = base_url.to_ascii_lowercase();
    if u.contains("bigmodel.cn") || u.contains("z.ai") {
        Provider::Zhipu
    } else if u.contains("deepseek.com") {
        Provider::DeepSeek
    } else {
        Provider::Other
    }
}

/// 内置能力表：按（provider, 上游 slug）给出已知能力。
/// 只收录我们真实验证过或官方文档明确的组合；拿不准的一律不进表
/// （落到 unknown，宁可让 UI 显示"未知"也不虚标）。
fn built_in_caps(slug: &str, provider: Provider) -> Option<ModelCaps> {
    let s = slug.to_ascii_lowercase();
    let known = |tool_use: bool, vision: bool, reasoning: bool| {
        Some(ModelCaps {
            text: Cap::built_in(true),
            tool_use: Cap::built_in(tool_use),
            vision_input: Cap::built_in(vision),
            reasoning: Cap::built_in(reasoning),
        })
    };
    match provider {
        Provider::Zhipu => {
            // 视觉系列：glm-4v* / glm-*.*v（如 glm-4.5v）
            if s.starts_with("glm-4v") || (s.starts_with("glm-") && s.ends_with('v')) {
                // glm-4v-flash 实测：看图可用；作为 agent 主模型工具调用未验证
                known(false, true, false)
            } else if s.starts_with("glm-5") || s.starts_with("glm-4") {
                // 文本编码系列（Coding Plan 主路径 + 开放平台）：
                // dogfooding 全程 tool calling；不接受图片输入
                known(true, false, false)
            } else {
                None
            }
        }
        Provider::DeepSeek => {
            if s == "deepseek-chat" {
                known(true, false, false)
            } else if s == "deepseek-reasoner" {
                // R1 系：reasoning_content 流；vision 无；tool calling 随版本
                // 变化大，不虚标 → 单项留 unknown
                Some(ModelCaps {
                    text: Cap::built_in(true),
                    tool_use: Cap::UNKNOWN,
                    vision_input: Cap::built_in(false),
                    reasoning: Cap::built_in(true),
                })
            } else {
                None
            }
        }
        Provider::Other => None,
    }
}

/// 权威解析器：设置页与聊天目录唯一入口。
/// catalog key 不参与匹配——key 可重命名，slug + endpoint 才是身份。
pub fn resolve_caps(slug: &str, base_url: &str, overrides: &CapOverrides) -> ModelCaps {
    let mut caps = built_in_caps(slug, provider_of(base_url)).unwrap_or(ModelCaps::UNKNOWN);
    if let Some(v) = overrides.text {
        caps.text = Cap::config(v);
    }
    if let Some(v) = overrides.tool_use {
        caps.tool_use = Cap::config(v);
    }
    if let Some(v) = overrides.vision_input {
        caps.vision_input = Cap::config(v);
    }
    if let Some(v) = overrides.reasoning {
        caps.reasoning = Cap::config(v);
    }
    caps
}

// ── 图片路径决策（纯函数，UI 在后续 PR 接线） ──────────────────────────

/// 粘图发送前的有效路径判定结果。
/// 语义（复核定案）：图片可达性取决于**转述链路**优先，而非主模型。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImagePathDecision {
    /// 转述开启且辅助视觉模型有效：放行（即使主模型纯文本）。
    AllowViaDescription,
    /// 转述关闭、主模型原生支持视觉：放行。
    AllowNativeVision,
    /// 转述开启但辅助模型缺失：阻断，引导配置视觉辅助模型。
    BlockNoHelper,
    /// 转述开启但辅助模型明确不支持视觉：阻断，引导更换。
    BlockHelperNotVision,
    /// 转述开启、辅助模型能力未知：警告 + 可"仍然尝试"。
    WarnHelperUnknown,
    /// 转述关闭、主模型明确不支持视觉：阻断。
    BlockMainNotVision,
    /// 转述关闭、主模型能力未知：警告 + 可"仍然尝试"。
    WarnMainUnknown,
}

/// transcribe_on：GROK_IMAGE_TRANSCRIBE 是否启用（当前默认 1，见 lib.rs）。
/// helper：已配置的视觉辅助模型能力（None = 未配置）。
/// main：当前主模型能力。
pub fn decide_image_path(
    transcribe_on: bool,
    helper: Option<&ModelCaps>,
    main: &ModelCaps,
) -> ImagePathDecision {
    if transcribe_on {
        match helper {
            None => ImagePathDecision::BlockNoHelper,
            Some(h) => match h.vision_input.state {
                CapState::Supported => ImagePathDecision::AllowViaDescription,
                CapState::Unsupported => ImagePathDecision::BlockHelperNotVision,
                CapState::Unknown => ImagePathDecision::WarnHelperUnknown,
            },
        }
    } else {
        match main.vision_input.state {
            CapState::Supported => ImagePathDecision::AllowNativeVision,
            CapState::Unsupported => ImagePathDecision::BlockMainNotVision,
            CapState::Unknown => ImagePathDecision::WarnMainUnknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZHIPU_OPEN: &str = "https://open.bigmodel.cn/api/paas/v4";
    const ZHIPU_CODING: &str = "https://open.bigmodel.cn/api/coding/paas/v4";
    const DEEPSEEK: &str = "https://api.deepseek.com";

    fn parse_overrides(toml: &str) -> CapOverrides {
        let doc: toml_edit::DocumentMut = toml.parse().unwrap();
        let item = doc
            .get("model")
            .and_then(|m| m.as_table())
            .and_then(|t| t.iter().next().map(|(_, v)| v))
            .and_then(|v| v.as_table_like())
            .unwrap();
        CapOverrides::from_toml(item)
    }

    /// RED ①：显式 false 覆盖内置 true——用户比内置表更权威。
    #[test]
    fn explicit_false_overrides_built_in_true() {
        let ov = CapOverrides {
            vision_input: Some(false),
            ..Default::default()
        };
        let caps = resolve_caps("glm-4v-flash", ZHIPU_OPEN, &ov);
        assert_eq!(caps.vision_input.state, CapState::Unsupported);
        assert_eq!(caps.vision_input.source, CapSource::Config);
        // 未覆盖的项仍来自内置表
        assert_eq!(caps.text.source, CapSource::BuiltIn);
    }

    /// RED ②：catalog key 重命名不影响内置匹配——身份是 slug + endpoint。
    /// （解析器签名根本不收 key，此测试锁住"永远不加 key 参数"的契约。）
    #[test]
    fn renamed_catalog_key_still_matches_built_in() {
        // 模拟用户把 key 改成 "my-eyes"，slug/base_url 不变
        let caps = resolve_caps("glm-4v-flash", ZHIPU_OPEN, &CapOverrides::default());
        assert_eq!(caps.vision_input.state, CapState::Supported);
        assert_eq!(caps.vision_input.source, CapSource::BuiltIn);
    }

    /// RED ③：未知模型保持 unknown——不虚标、不默认支持。
    #[test]
    fn unknown_model_stays_unknown() {
        let caps = resolve_caps("mystery-1", "https://example.com/v1", &CapOverrides::default());
        for c in [caps.text, caps.tool_use, caps.vision_input, caps.reasoning] {
            assert_eq!(c.state, CapState::Unknown);
            assert_eq!(c.source, CapSource::Unknown);
        }
        // 撞名 slug 但陌生 endpoint：同样不得借智谱的表
        let caps = resolve_caps("glm-4v-flash", "https://example.com/v1", &CapOverrides::default());
        assert_eq!(caps.vision_input.state, CapState::Unknown);
    }

    /// RED ④：旧配置（无 capabilities 子表）正常反序列化，全走内置/unknown。
    #[test]
    fn legacy_config_without_capabilities_parses() {
        let ov = parse_overrides(
            r#"
[model.glm-4v-flash]
model = "glm-4v-flash"
base_url = "https://open.bigmodel.cn/api/paas/v4"
env_key = "ZHIPU_API_KEY"
"#,
        );
        assert_eq!(ov, CapOverrides::default());
        let caps = resolve_caps("glm-4v-flash", ZHIPU_OPEN, &ov);
        assert_eq!(caps.vision_input.state, CapState::Supported);
        assert_eq!(caps.vision_input.source, CapSource::BuiltIn);
    }

    /// 新配置格式解析：capabilities 子表逐项可缺省。
    #[test]
    fn config_capabilities_subtable_parses() {
        let ov = parse_overrides(
            r#"
[model.custom]
model = "custom-llm"
base_url = "https://example.com/v1"

[model.custom.capabilities]
vision_input = true
tool_use = false
"#,
        );
        assert_eq!(ov.vision_input, Some(true));
        assert_eq!(ov.tool_use, Some(false));
        assert_eq!(ov.text, None);
        let caps = resolve_caps("custom-llm", "https://example.com/v1", &ov);
        assert_eq!(caps.vision_input.state, CapState::Supported);
        assert_eq!(caps.vision_input.source, CapSource::Config);
        assert_eq!(caps.text.state, CapState::Unknown);
    }

    /// RED ⑤：设置页与聊天目录必须得到完全相同的结果——两条链的适配层
    /// 都只许传 (slug, base_url, overrides) 进同一个 resolve_caps。
    /// 此测试锁住解析器为纯函数：同输入必同输出（含来源）。
    #[test]
    fn settings_and_chat_chains_get_identical_results() {
        let ov = CapOverrides {
            reasoning: Some(true),
            ..Default::default()
        };
        let a = resolve_caps("deepseek-chat", DEEPSEEK, &ov); // 设置页链
        let b = resolve_caps("deepseek-chat", DEEPSEEK, &ov); // 聊天目录链
        assert_eq!(a, b);
    }

    /// RED ⑥：文本主模型 + 有效视觉辅助 + 转述开启 → 允许粘图。
    /// （这正是产品现状：GLM-5.2 纯文本 + glm-4v-flash 转述。）
    #[test]
    fn text_main_with_valid_helper_allows_images() {
        let main = resolve_caps("glm-5.2", ZHIPU_CODING, &CapOverrides::default());
        let helper = resolve_caps("glm-4v-flash", ZHIPU_OPEN, &CapOverrides::default());
        assert_eq!(main.vision_input.state, CapState::Unsupported);
        assert_eq!(
            decide_image_path(true, Some(&helper), &main),
            ImagePathDecision::AllowViaDescription
        );
    }

    /// RED ⑦：转述开启但辅助模型缺失 → 阻断并引导配置。
    #[test]
    fn missing_helper_blocks() {
        let main = resolve_caps("glm-5.2", ZHIPU_CODING, &CapOverrides::default());
        assert_eq!(
            decide_image_path(true, None, &main),
            ImagePathDecision::BlockNoHelper
        );
    }

    /// RED ⑧：inline 模式（转述关闭）——视觉主模型放行，文本主模型阻断。
    #[test]
    fn inline_mode_checks_main_native_vision() {
        let vision_main = resolve_caps("glm-4v-flash", ZHIPU_OPEN, &CapOverrides::default());
        let text_main = resolve_caps("glm-5.2", ZHIPU_CODING, &CapOverrides::default());
        assert_eq!(
            decide_image_path(false, None, &vision_main),
            ImagePathDecision::AllowNativeVision
        );
        assert_eq!(
            decide_image_path(false, None, &text_main),
            ImagePathDecision::BlockMainNotVision
        );
    }

    /// 补充：辅助/主模型能力未知 → 警告态（可"仍然尝试"），绝不静默放行或硬阻断。
    #[test]
    fn unknown_caps_yield_warnings_not_silent_paths() {
        let unknown = resolve_caps("mystery-1", "https://example.com/v1", &CapOverrides::default());
        let main = resolve_caps("glm-5.2", ZHIPU_CODING, &CapOverrides::default());
        assert_eq!(
            decide_image_path(true, Some(&unknown), &main),
            ImagePathDecision::WarnHelperUnknown
        );
        assert_eq!(
            decide_image_path(false, None, &unknown),
            ImagePathDecision::WarnMainUnknown
        );
    }
}
