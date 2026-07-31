//! #127 能力模型与统一解析器（v0.18.9 兼容性治理第 1 步，复核修订二版）。
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
//! 内置表是**精确 slug allowlist**（无通配），逐项 Option<bool>：未验证的
//! 项写 None（落 unknown），绝不把"没测过"写成"不支持"。匹配维度为
//! provider（按真实 hostname 域名边界判定）+ 上游 slug——绝不按 catalog
//! key（可被用户任意重命名）。结果附带来源（config / built_in / unknown）。
//!
//! 存储契约（复核四版定案）：显式覆盖放**独立文件** `~/.grok/wancode.toml`
//! 的 `[model_capabilities.<catalog_key>]`。config.toml 的任何位置都不放
//! WanCode 专属数据——[model.X] 塞子字段会被引擎记 UnknownField，顶层
//! [wancode] section 也会触发 "config has unrecognized key(s)" 告警：
//! 引擎不该解析 WanCode 专属数据（先例：~/.grok/hooks/wancode.json）。
//! 显式覆盖本就需要定位具体条目，故按 catalog_key 寻址；内置识别仍只按
//! provider + slug，不受 key 重命名影响。
//!
//! 配置解析 fail-visible：类型错误、未知字段返回诊断（CapIssue），
//! 不静默当作未配置。
//!
//! 设置页（provider_ops::model_list）与聊天目录（ACP ModelOption 链）
//! 必须都经由本解析器取能力，杜绝双列表漂移（v0.18.7-B 的教训）；
//! 双真实数据链的一致性门槛在 PR 2 接线时建立。

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
    /// 用户在 ~/.grok/wancode.toml [model_capabilities.<key>] 显式声明。
    Config,
    /// 内置能力表按 provider + slug 精确匹配。
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
    fn from_opt(v: Option<bool>, source: CapSource) -> Cap {
        match v {
            None => Cap::UNKNOWN,
            Some(true) => Cap {
                state: CapState::Supported,
                source,
            },
            Some(false) => Cap {
                state: CapState::Unsupported,
                source,
            },
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

/// 逐项可缺省的能力声明（内置表条目与用户覆盖共用同一形状）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CapOverrides {
    pub text: Option<bool>,
    pub tool_use: Option<bool>,
    pub vision_input: Option<bool>,
    pub reasoning: Option<bool>,
}

/// 配置解析诊断：错误必须可见，不许静默当作未配置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapIssueKind {
    /// 字段值不是布尔（如 vision_input = "true"）。该项按未配置解析，
    /// 但诊断必须上浮到 UI/日志。
    WrongType,
    /// capabilities 子表里出现未知字段（拼错或不属于固有能力，如
    /// image_description）。
    UnknownField,
    /// 能力条目本身不是表。
    NotATable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CapIssue {
    pub field: String,
    pub kind: CapIssueKind,
}

const CAP_FIELDS: [&str; 4] = ["text", "tool_use", "vision_input", "reasoning"];

/// wancode.toml 的文档级解析快照：一次解析、全量诊断、按 key 查询。
/// 设置页与聊天链共享同一份快照（PR 2 接线），避免逐模型重复解析。
///
/// fail-visible 覆盖整份文档：未知**顶层**字段（如拼错的
/// `model_capabilites`）出 UnknownField——"拼错"绝不静默等同"没配置"。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedWanCodeConfig {
    overrides: std::collections::BTreeMap<String, CapOverrides>,
    pub issues: Vec<CapIssue>,
}

/// wancode.toml 允许的顶层字段。新增功能字段时在此登记。
const TOP_LEVEL_FIELDS: [&str; 1] = ["model_capabilities"];

impl ParsedWanCodeConfig {
    pub fn parse(doc: &toml_edit::DocumentMut) -> ParsedWanCodeConfig {
        let mut out = ParsedWanCodeConfig::default();
        for (k, _) in doc.iter() {
            if !TOP_LEVEL_FIELDS.contains(&k) {
                out.issues.push(CapIssue {
                    field: k.to_string(),
                    kind: CapIssueKind::UnknownField,
                });
            }
        }
        let Some(mc_item) = doc.get("model_capabilities") else {
            return out;
        };
        let Some(mc) = mc_item.as_table_like() else {
            out.issues.push(CapIssue {
                field: "model_capabilities".into(),
                kind: CapIssueKind::NotATable,
            });
            return out;
        };
        for (key, entry_item) in mc.iter() {
            match entry_item.as_table_like() {
                Some(caps) => {
                    let (ov, mut issues) = CapOverrides::from_caps_table(caps);
                    out.issues.append(&mut issues);
                    out.overrides.insert(key.to_string(), ov);
                }
                None => out.issues.push(CapIssue {
                    field: key.to_string(),
                    kind: CapIssueKind::NotATable,
                }),
            }
        }
        out
    }

    /// 按 catalog key 查询覆盖；无条目 → 默认（无覆盖是常态）。
    pub fn for_model(&self, catalog_key: &str) -> CapOverrides {
        self.overrides.get(catalog_key).copied().unwrap_or_default()
    }
}

impl CapOverrides {
    /// 从能力子表本体读取。
    /// 类型错误 / 未知字段 → 逐条诊断（fail-visible），错误项按未配置解析。
    pub fn from_caps_table(caps: &dyn toml_edit::TableLike) -> (CapOverrides, Vec<CapIssue>) {
        let mut issues = Vec::new();
        let mut get = |k: &str| -> Option<bool> {
            match caps.get(k) {
                None => None,
                Some(v) => match v.as_bool() {
                    Some(b) => Some(b),
                    None => {
                        issues.push(CapIssue {
                            field: k.into(),
                            kind: CapIssueKind::WrongType,
                        });
                        None
                    }
                },
            }
        };
        let ov = CapOverrides {
            text: get("text"),
            tool_use: get("tool_use"),
            vision_input: get("vision_input"),
            reasoning: get("reasoning"),
        };
        for (k, _) in caps.iter() {
            if !CAP_FIELDS.contains(&k) {
                issues.push(CapIssue {
                    field: k.to_string(),
                    kind: CapIssueKind::UnknownField,
                });
            }
        }
        (ov, issues)
    }
}

/// WanCode 自有配置文件：与引擎 config.toml 同目录、引擎绝不解析。
/// （先例：~/.grok/hooks/wancode.json。）
pub fn wancode_config_path() -> std::path::PathBuf {
    xai_grok_shell::util::grok_home::grok_home().join("wancode.toml")
}

/// endpoint 归类：按真实 hostname 的域名边界判定，
/// 杜绝 `notz.ai.evil` / 查询参数携带官方域名的误判。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provider {
    Zhipu,
    DeepSeek,
    Other,
}

fn host_in_domain(host: &str, domain: &str) -> bool {
    host == domain || host.ends_with(&format!(".{domain}"))
}

fn provider_of(base_url: &str) -> Provider {
    let Ok(u) = url::Url::parse(base_url) else {
        return Provider::Other;
    };
    let Some(host) = u.host_str() else {
        return Provider::Other;
    };
    let host = host.to_ascii_lowercase();
    if host_in_domain(&host, "bigmodel.cn") || host_in_domain(&host, "z.ai") {
        Provider::Zhipu
    } else if host_in_domain(&host, "deepseek.com") {
        Provider::DeepSeek
    } else {
        Provider::Other
    }
}

/// 内置能力表：精确 slug allowlist，逐项 Option<bool>。
/// 收录纪律：只写有实证（dogfooding/发布门/官方文档明确）的项；
/// 未验证 → None（unknown），绝不写成 Some(false)。
fn built_in_caps(slug: &str, provider: Provider) -> CapOverrides {
    let s = slug.to_ascii_lowercase();
    match (provider, s.as_str()) {
        // 智谱视觉辅助默认模型：看图为 v0.18.1 起发布门验证路径；
        // 作为 agent 主模型的 tool use / reasoning 未验证 → unknown。
        (Provider::Zhipu, "glm-4v-flash") => CapOverrides {
            text: Some(true),
            vision_input: Some(true),
            tool_use: None,
            reasoning: None,
        },
        // Coding Plan / 开放平台文本编码系列：dogfooding 全程 tool calling；
        // 图片输入被端点拒绝（v0.18.1 修视觉路由的起因，实证）。
        (Provider::Zhipu, "glm-5.2" | "glm-5-turbo" | "glm-4-flash") => CapOverrides {
            text: Some(true),
            tool_use: Some(true),
            vision_input: Some(false),
            reasoning: None,
        },
        // 2026-07-29 smoke 6/6；官方明确非视觉、非 reasoner。
        (Provider::DeepSeek, "deepseek-chat") => CapOverrides {
            text: Some(true),
            tool_use: Some(true),
            vision_input: Some(false),
            reasoning: Some(false),
        },
        // R1 系：reasoning 官方明确；vision 官方明确无；
        // tool calling 随版本变化大，未验证 → unknown。
        (Provider::DeepSeek, "deepseek-reasoner") => CapOverrides {
            text: Some(true),
            reasoning: Some(true),
            vision_input: Some(false),
            tool_use: None,
        },
        _ => CapOverrides::default(),
    }
}

/// 权威解析器：设置页与聊天目录唯一入口。
/// catalog key 不参与匹配——key 可重命名，slug + endpoint 才是身份。
pub fn resolve_caps(slug: &str, base_url: &str, overrides: &CapOverrides) -> ModelCaps {
    let built = built_in_caps(slug, provider_of(base_url));
    let pick = |ov: Option<bool>, bi: Option<bool>| -> Cap {
        if ov.is_some() {
            Cap::from_opt(ov, CapSource::Config)
        } else {
            Cap::from_opt(bi, CapSource::BuiltIn)
        }
    };
    // from_opt(None, BuiltIn) 落 Cap::UNKNOWN（来源 Unknown），语义正确
    let mut caps = ModelCaps::UNKNOWN;
    caps.text = pick(overrides.text, built.text);
    caps.tool_use = pick(overrides.tool_use, built.tool_use);
    caps.vision_input = pick(overrides.vision_input, built.vision_input);
    caps.reasoning = pick(overrides.reasoning, built.reasoning);
    caps
}

// ── 图片路径决策（纯函数，UI 在后续 PR 接线） ──────────────────────────

/// 视觉辅助模型的解析状态。"配置了"≠"可用"：已删除、无法路由的辅助
/// 模型必须是 Unavailable，不许拿到 AllowViaDescription。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperStatus<'a> {
    /// 未配置视觉辅助模型。
    Missing,
    /// 配置了但解析失败（条目已删除 / 无法路由 / Key 缺失）。
    Unavailable,
    /// 成功解析到目录内可路由的模型，附其能力。
    Resolved(&'a ModelCaps),
}

/// 粘图发送前的有效路径判定结果。
/// 语义（复核定案）：图片可达性取决于**转述链路**优先，而非主模型。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImagePathDecision {
    /// 转述开启且辅助视觉模型已解析且支持视觉：放行（即使主模型纯文本）。
    AllowViaDescription,
    /// 转述关闭、主模型原生支持视觉：放行。
    AllowNativeVision,
    /// 转述开启但未配置辅助模型：阻断，引导配置。
    BlockNoHelper,
    /// 转述开启、辅助模型配置了但解析失败：阻断，引导修复配置。
    BlockHelperUnavailable,
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
pub fn decide_image_path(
    transcribe_on: bool,
    helper: HelperStatus<'_>,
    main: &ModelCaps,
) -> ImagePathDecision {
    if transcribe_on {
        match helper {
            HelperStatus::Missing => ImagePathDecision::BlockNoHelper,
            HelperStatus::Unavailable => ImagePathDecision::BlockHelperUnavailable,
            HelperStatus::Resolved(h) => match h.vision_input.state {
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

    fn overrides_for(toml: &str, key: &str) -> (CapOverrides, Vec<CapIssue>) {
        let doc: toml_edit::DocumentMut = toml.parse().unwrap();
        let parsed = ParsedWanCodeConfig::parse(&doc);
        (parsed.for_model(key), parsed.issues)
    }

    /// ①：显式 false 覆盖内置 true——用户比内置表更权威。
    #[test]
    fn explicit_false_overrides_built_in_true() {
        let ov = CapOverrides {
            vision_input: Some(false),
            ..Default::default()
        };
        let caps = resolve_caps("glm-4v-flash", ZHIPU_OPEN, &ov);
        assert_eq!(caps.vision_input.state, CapState::Unsupported);
        assert_eq!(caps.vision_input.source, CapSource::Config);
        // 未覆盖的已验证项仍来自内置表
        assert_eq!(caps.text.source, CapSource::BuiltIn);
    }

    /// ②：catalog key 重命名不影响内置匹配——身份是 slug + endpoint。
    /// （解析器签名根本不收 key，此测试锁住"永远不加 key 参数"的契约。）
    #[test]
    fn renamed_catalog_key_still_matches_built_in() {
        let caps = resolve_caps("glm-4v-flash", ZHIPU_OPEN, &CapOverrides::default());
        assert_eq!(caps.vision_input.state, CapState::Supported);
        assert_eq!(caps.vision_input.source, CapSource::BuiltIn);
    }

    /// ③：未知模型保持 unknown——不虚标、不默认支持；
    /// 撞名 slug + 陌生 endpoint 不得借表。
    #[test]
    fn unknown_model_stays_unknown() {
        let caps = resolve_caps("mystery-1", "https://example.com/v1", &CapOverrides::default());
        for c in [caps.text, caps.tool_use, caps.vision_input, caps.reasoning] {
            assert_eq!(c.state, CapState::Unknown);
            assert_eq!(c.source, CapSource::Unknown);
        }
        let caps = resolve_caps("glm-4v-flash", "https://example.com/v1", &CapOverrides::default());
        assert_eq!(caps.vision_input.state, CapState::Unknown);
    }

    /// 内置表纪律：未验证项是 unknown，不是 unsupported。
    /// （glm-4v-flash 的 tool_use 没测过——绝不许写成"不支持"。）
    #[test]
    fn unverified_built_in_items_stay_unknown_not_unsupported() {
        let caps = resolve_caps("glm-4v-flash", ZHIPU_OPEN, &CapOverrides::default());
        assert_eq!(caps.tool_use.state, CapState::Unknown);
        assert_eq!(caps.tool_use.source, CapSource::Unknown);
        let caps = resolve_caps("deepseek-reasoner", DEEPSEEK, &CapOverrides::default());
        assert_eq!(caps.tool_use.state, CapState::Unknown);
        assert_eq!(caps.reasoning.state, CapState::Supported);
    }

    /// provider 判定按真实 hostname 域名边界：伪造域名、路径/查询串里
    /// 携带官方域名都不得误判。
    #[test]
    fn provider_matching_respects_domain_boundaries() {
        // 官方（含子域）
        for u in [
            "https://open.bigmodel.cn/api/paas/v4",
            "https://api.z.ai/v1",
        ] {
            assert_eq!(
                resolve_caps("glm-4v-flash", u, &CapOverrides::default())
                    .vision_input
                    .state,
                CapState::Supported,
                "{u}"
            );
        }
        // 伪造与携带
        for u in [
            "https://notz.ai.evil/v1",
            "https://evil.com/?u=open.bigmodel.cn",
            "https://fakebigmodel.cn.attacker.io/v4",
            "not a url",
        ] {
            assert_eq!(
                resolve_caps("glm-4v-flash", u, &CapOverrides::default())
                    .vision_input
                    .state,
                CapState::Unknown,
                "{u}"
            );
        }
    }

    /// ④：旧配置（无 capabilities 子表）正常解析，零诊断，全走内置/unknown。
    #[test]
    fn legacy_config_without_capabilities_parses() {
        // 老用户没有 wancode.toml——空文档必须零覆盖零诊断
        let (ov, issues) = overrides_for("", "glm-4v-flash");
        assert_eq!(ov, CapOverrides::default());
        assert!(issues.is_empty());
    }

    /// 新配置格式解析：capabilities 子表逐项可缺省。
    #[test]
    fn config_capabilities_subtable_parses() {
        let (ov, issues) = overrides_for(
            r#"
[model_capabilities.custom]
vision_input = true
tool_use = false
"#,
            "custom",
        );
        assert!(issues.is_empty());
        assert_eq!(ov.vision_input, Some(true));
        assert_eq!(ov.tool_use, Some(false));
        assert_eq!(ov.text, None);
        let caps = resolve_caps("custom-llm", "https://example.com/v1", &ov);
        assert_eq!(caps.vision_input.source, CapSource::Config);
        assert_eq!(caps.text.state, CapState::Unknown);
    }

    /// fail-visible：类型错误与未知字段必须出诊断，不许静默当作未配置。
    #[test]
    fn config_errors_produce_visible_diagnostics() {
        let (ov, issues) = overrides_for(
            r#"
[model_capabilities.custom]
vision_input = "true"
visoin_input = true
image_description = true
"#,
            "custom",
        );
        // 类型错误的项按未配置解析，但诊断在
        assert_eq!(ov.vision_input, None);
        assert!(issues.contains(&CapIssue {
            field: "vision_input".into(),
            kind: CapIssueKind::WrongType
        }));
        // 拼错字段与"路由角色混进能力表"都必须点名
        assert!(issues.contains(&CapIssue {
            field: "visoin_input".into(),
            kind: CapIssueKind::UnknownField
        }));
        assert!(issues.contains(&CapIssue {
            field: "image_description".into(),
            kind: CapIssueKind::UnknownField
        }));
        // 能力条目不是表
        let (_, issues) = overrides_for(
            r#"
[model_capabilities]
custom = "all"
"#,
            "custom",
        );
        assert_eq!(
            issues,
            vec![CapIssue {
                field: "custom".into(),
                kind: CapIssueKind::NotATable
            }]
        );
    }

    /// 父级层级 fail-visible："存在但类型错"必须出 NotATable 诊断，
    /// 绝不与"缺失"合并成静默零诊断。
    #[test]
    fn parent_level_type_errors_are_visible() {
        // model_capabilities 本身不是表
        let (ov, issues) = overrides_for(r#"model_capabilities = "bad""#, "custom");
        assert_eq!(ov, CapOverrides::default());
        assert_eq!(
            issues,
            vec![CapIssue {
                field: "model_capabilities".into(),
                kind: CapIssueKind::NotATable
            }]
        );
        // 条目缺失才是零诊断（文件为空 / 表内无该 key）
        let (_, issues) = overrides_for("", "custom");
        assert!(issues.is_empty());
        let (_, issues) = overrides_for("[model_capabilities]
", "custom");
        assert!(issues.is_empty());
    }

    /// 未知顶层字段（最常见：拼错 model_capabilities）必须 UnknownField
    /// 告警——绝不静默当成"没有配置"。config.toml 内容误粘进来同理点名。
    #[test]
    fn misspelled_top_level_field_is_flagged() {
        let (ov, issues) = overrides_for(
            r#"
[model_capabilites.custom]
vision_input = true
"#,
            "custom",
        );
        assert_eq!(ov, CapOverrides::default(), "拼错的表不得生效");
        assert_eq!(
            issues,
            vec![CapIssue {
                field: "model_capabilites".into(),
                kind: CapIssueKind::UnknownField
            }]
        );
        // 误把 config.toml 的 [model.X] 粘进 wancode.toml：同样点名
        let (_, issues) = overrides_for(
            r#"
[model.custom]
base_url = "https://example.com/v1"
"#,
            "custom",
        );
        assert_eq!(
            issues,
            vec![CapIssue {
                field: "model".into(),
                kind: CapIssueKind::UnknownField
            }]
        );
    }

    /// 解析器是纯函数：同输入必同输出（含来源）。
    /// 注：这只证明确定性；设置页与聊天目录两条**真实数据链**的一致性
    /// 门槛在 PR 2 接线时建立。
    #[test]
    fn resolver_is_pure_and_deterministic() {
        let ov = CapOverrides {
            reasoning: Some(true),
            ..Default::default()
        };
        let a = resolve_caps("deepseek-chat", DEEPSEEK, &ov);
        let b = resolve_caps("deepseek-chat", DEEPSEEK, &ov);
        assert_eq!(a, b);
    }

    /// ⑥：文本主模型 + 已解析且支持视觉的辅助 + 转述开启 → 允许粘图。
    /// （这正是产品现状：GLM-5.2 纯文本 + glm-4v-flash 转述。）
    #[test]
    fn text_main_with_resolved_helper_allows_images() {
        let main = resolve_caps("glm-5.2", ZHIPU_CODING, &CapOverrides::default());
        let helper = resolve_caps("glm-4v-flash", ZHIPU_OPEN, &CapOverrides::default());
        assert_eq!(main.vision_input.state, CapState::Unsupported);
        assert_eq!(
            decide_image_path(true, HelperStatus::Resolved(&helper), &main),
            ImagePathDecision::AllowViaDescription
        );
    }

    /// ⑦：辅助模型缺失 → 阻断引导配置；
    /// 配置了但解析失败（已删除/无法路由）→ 阻断引导修复，绝不放行。
    #[test]
    fn missing_or_unavailable_helper_blocks() {
        let main = resolve_caps("glm-5.2", ZHIPU_CODING, &CapOverrides::default());
        assert_eq!(
            decide_image_path(true, HelperStatus::Missing, &main),
            ImagePathDecision::BlockNoHelper
        );
        assert_eq!(
            decide_image_path(true, HelperStatus::Unavailable, &main),
            ImagePathDecision::BlockHelperUnavailable
        );
    }

    /// ⑧：inline 模式（转述关闭）——视觉主模型放行，文本主模型阻断。
    #[test]
    fn inline_mode_checks_main_native_vision() {
        let vision_main = resolve_caps("glm-4v-flash", ZHIPU_OPEN, &CapOverrides::default());
        let text_main = resolve_caps("glm-5.2", ZHIPU_CODING, &CapOverrides::default());
        assert_eq!(
            decide_image_path(false, HelperStatus::Missing, &vision_main),
            ImagePathDecision::AllowNativeVision
        );
        assert_eq!(
            decide_image_path(false, HelperStatus::Missing, &text_main),
            ImagePathDecision::BlockMainNotVision
        );
    }

    /// 辅助/主模型能力未知 → 警告态（可"仍然尝试"），绝不静默放行或硬阻断。
    #[test]
    fn unknown_caps_yield_warnings_not_silent_paths() {
        let unknown = resolve_caps("mystery-1", "https://example.com/v1", &CapOverrides::default());
        let main = resolve_caps("glm-5.2", ZHIPU_CODING, &CapOverrides::default());
        assert_eq!(
            decide_image_path(true, HelperStatus::Resolved(&unknown), &main),
            ImagePathDecision::WarnHelperUnknown
        );
        assert_eq!(
            decide_image_path(false, HelperStatus::Missing, &unknown),
            ImagePathDecision::WarnMainUnknown
        );
    }

    /// 存储契约隔离性：两份**文档**互不掺杂且各自可解析——config.toml
    /// 与功能引入前逐字相同，经引擎 `Config::new_from_toml_cfg` 后模型进
    /// 目录、model_override_warnings 为零；wancode.toml 文档由 WanCode
    /// 解析器独立读取。真实文件 IO（wancode_config_path 落盘/读取）
    /// 属 PR 2 接线范围。
    #[test]
    fn engine_and_wancode_documents_are_isolated() {
        // config.toml：与本功能引入前逐字相同
        let config_toml = r#"
[model.glm-4v-flash]
model = "glm-4v-flash"
base_url = "https://open.bigmodel.cn/api/paas/v4"
env_key = "ZHIPU_API_KEY"
"#;
        let raw: toml::Value = toml::from_str(config_toml).expect("toml parse");
        let cfg = xai_grok_shell::agent::config::Config::new_from_toml_cfg(&raw)
            .expect("engine config load");
        assert!(cfg.config_models.contains_key("glm-4v-flash"));
        assert!(
            cfg.model_override_warnings.is_empty(),
            "实际: {:?}",
            cfg.model_override_warnings
        );

        // wancode.toml：独立文件，引擎从不解析
        let wancode_toml = r#"
[model_capabilities.glm-4v-flash]
vision_input = true
"#;
        let doc: toml_edit::DocumentMut = wancode_toml.parse().unwrap();
        let parsed = ParsedWanCodeConfig::parse(&doc);
        assert!(parsed.issues.is_empty());
        assert_eq!(parsed.for_model("glm-4v-flash").vision_input, Some(true));
    }
}
