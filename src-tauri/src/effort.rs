//! C2（v0.20）：推理强度选择器的 wire 解析层。
//!
//! 引擎侧契约（xai-grok-shell `session_config.rs` / `config.rs`）：
//! - `NewSession`/`LoadSession` 响应的 `_meta["x.ai/sessionConfig"]` 是
//!   `{id, category, label, description?, selected}` 数组；`category == "mode"`
//!   的条目即当前模型的强度菜单（含引擎的 legacy 五档兜底），`selected` 为当前档。
//! - `available_models` 每个 `ModelInfo.meta` 带能力位：`supportsReasoningEffort`
//!   （bool）、`reasoningEfforts`（`{id, value, label, ...}` 数组，可空——空时
//!   引擎回落 legacy 五档）、`reasoningEffort`（该模型的配置默认档）。
//!
//! 能力门原则（C2 验收）：**unknown ≠ advertised**——`supportsReasoningEffort`
//! 缺席/false 时本模块一律返回空菜单，前端不得显示选择器。引擎在 set_model
//! 侧对不支持的模型会忽略强度覆盖（`model_switch.rs` warn），双保险。

/// 一个可选强度档（前端下拉的一行）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EffortChoice {
    pub id: String,
    /// 发给引擎 `reasoningEffort` 的 canonical 值。自定义菜单 id（如
    /// `deep`）可映射到不同值（如 `xhigh`），两者不可混用。
    pub value: String,
    pub label: String,
}

/// 从 session 响应 meta 拆强度菜单与当前档。
/// 返回 `(options, current_id)`；菜单为空 = 当前模型不支持强度选择。
pub fn parse_session_config_effort(
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> (Vec<EffortChoice>, Option<String>) {
    let Some(arr) = meta
        .and_then(|m| m.get("x.ai/sessionConfig"))
        .and_then(|v| v.as_array())
    else {
        return (Vec::new(), None);
    };
    let mut options = Vec::new();
    let mut current = None;
    for opt in arr {
        if opt.get("category").and_then(|c| c.as_str()) != Some("mode") {
            continue;
        }
        let Some(id) = opt.get("id").and_then(|i| i.as_str()) else {
            continue;
        };
        let label = opt.get("label").and_then(|l| l.as_str()).unwrap_or(id);
        if opt.get("selected").and_then(|s| s.as_bool()).unwrap_or(false) {
            current = Some(id.to_string());
        }
        options.push(EffortChoice {
            id: id.to_string(),
            // sessionConfig 只带 id；调用方会用当前模型 catalog 的
            // reasoningEfforts 按 id 补上真实 value。legacy 菜单 id=value。
            value: id.to_string(),
            label: label.to_string(),
        });
    }
    (options, current)
}

/// 从单个 `ModelInfo.meta` 拆能力位：`(supports, options, default)`。
/// `supports = false` 时 options/default 恒为空（能力未知不得广告）。
pub fn parse_model_effort_meta(
    meta: Option<&serde_json::Map<String, serde_json::Value>>,
) -> (bool, Vec<EffortChoice>, Option<String>) {
    let supports = meta
        .and_then(|m| m.get("supportsReasoningEffort"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !supports {
        return (false, Vec::new(), None);
    }
    let options = meta
        .and_then(|m| m.get("reasoningEfforts"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|el| {
                    let id = el.get("id").and_then(|i| i.as_str())?;
                    let value = el.get("value").and_then(|v| v.as_str()).unwrap_or(id);
                    let label = el.get("label").and_then(|l| l.as_str()).unwrap_or(id);
                    Some(EffortChoice {
                        id: id.to_string(),
                        value: value.to_string(),
                        label: label.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let default = meta
        .and_then(|m| m.get("reasoningEffort"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    (true, options, default)
}

/// 用 catalog 菜单补齐 sessionConfig 缺失的 canonical value。
pub fn reconcile_session_effort_values(
    session: &mut [EffortChoice],
    catalog: &[EffortChoice],
) {
    for choice in session {
        if let Some(catalog_choice) = catalog
            .iter()
            .find(|candidate| candidate.id == choice.id)
        {
            choice.value.clone_from(&catalog_choice.value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(v: serde_json::Value) -> Option<serde_json::Map<String, serde_json::Value>> {
        v.as_object().cloned()
    }

    #[test]
    fn session_config_extracts_mode_options_and_selection() {
        let m = meta(serde_json::json!({
            "x.ai/sessionConfig": [
                {"id": "glm-5.2", "category": "model", "label": "GLM 5.2", "selected": true},
                {"id": "low", "category": "mode", "label": "Low", "selected": false},
                {"id": "high", "category": "mode", "label": "High", "selected": true}
            ]
        }));
        let (opts, cur) = parse_session_config_effort(m.as_ref());
        assert_eq!(opts.len(), 2, "只要 mode 条目，model 条目不得混入");
        assert_eq!(opts[1].label, "High");
        assert_eq!(opts[1].value, "high");
        assert_eq!(cur.as_deref(), Some("high"));
    }

    #[test]
    fn session_config_absent_or_modeless_means_no_selector() {
        assert_eq!(parse_session_config_effort(None), (vec![], None));
        let m = meta(serde_json::json!({
            "x.ai/sessionConfig": [
                {"id": "glm-5.2", "category": "model", "label": "GLM 5.2", "selected": true}
            ]
        }));
        assert_eq!(parse_session_config_effort(m.as_ref()), (vec![], None));
    }

    #[test]
    fn model_meta_capability_gate() {
        // 缺席 → 不支持（unknown ≠ advertised）
        assert_eq!(parse_model_effort_meta(None), (false, vec![], None));
        let off = meta(serde_json::json!({"supportsReasoningEffort": false}));
        assert_eq!(parse_model_effort_meta(off.as_ref()), (false, vec![], None));
        // 支持但菜单为空（引擎回落 legacy 五档，由 sessionConfig 表达）
        let bare = meta(serde_json::json!({"supportsReasoningEffort": true}));
        assert_eq!(parse_model_effort_meta(bare.as_ref()), (true, vec![], None));
        // 全形态
        let full = meta(serde_json::json!({
            "supportsReasoningEffort": true,
            "reasoningEffort": "medium",
            "reasoningEfforts": [
                {"id": "low", "value": "low", "label": "Low"},
                {"id": "high", "value": "high", "label": "High", "default": true}
            ]
        }));
        let (supports, opts, default) = parse_model_effort_meta(full.as_ref());
        assert!(supports);
        assert_eq!(opts.len(), 2);
        assert_eq!(opts[1].id, "high");
        assert_eq!(opts[1].value, "high");
        assert_eq!(default.as_deref(), Some("medium"));
    }

    #[test]
    fn model_meta_ignores_malformed_entries() {
        let m = meta(serde_json::json!({
            "supportsReasoningEffort": true,
            "reasoningEfforts": [{"label": "no id"}, "junk", {"id": "low"}]
        }));
        let (_, opts, _) = parse_model_effort_meta(m.as_ref());
        assert_eq!(opts.len(), 1);
        assert_eq!(opts[0].label, "low", "缺 label 时回落 id");
        assert_eq!(opts[0].value, "low", "缺 value 时回落 id");
    }

    #[test]
    fn session_menu_uses_catalog_value_for_custom_ids() {
        let session_meta = meta(serde_json::json!({
            "x.ai/sessionConfig": [
                {"id": "deep", "category": "mode", "label": "Deep", "selected": true}
            ]
        }));
        let model_meta = meta(serde_json::json!({
            "supportsReasoningEffort": true,
            "reasoningEfforts": [
                {"id": "deep", "value": "xhigh", "label": "Deep"}
            ]
        }));
        let (mut session, _) = parse_session_config_effort(session_meta.as_ref());
        let (_, catalog, _) = parse_model_effort_meta(model_meta.as_ref());
        reconcile_session_effort_values(&mut session, &catalog);
        assert_eq!(session[0].id, "deep");
        assert_eq!(session[0].value, "xhigh");
    }
}
