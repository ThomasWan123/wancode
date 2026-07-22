//! v0.18-1 步 B 第一刀：配置核心纯函数 + 启动门控 + 其单测。
//!
//! 从 agent.rs 搬出的自包含块：无 AgentState 依赖、无引擎调用，
//! 全部可独立单测。红线注释随函数原样保留。
use std::path::PathBuf;

/// 模型 key → WANCODE_KEY_* 环境变量名（keyring 注入引擎 env_key 用）。
pub(crate) fn wancode_env_key(key: &str) -> String {
    let up: String = key
        .chars()
        .map(|c| if c.is_alphanumeric() { c.to_ascii_uppercase() } else { '_' })
        .collect();
    format!("WANCODE_KEY_{up}")
}

/// 用户配置路径（~/.grok/config.toml）。
pub(crate) fn user_config_path() -> PathBuf {
    xai_grok_shell::util::grok_home::grok_home().join("config.toml")
}

/// Atomically replace a config file: write to a sibling temp file, then
/// rename over the target. On Windows `std::fs::rename` maps to
/// MoveFileExW(REPLACE_EXISTING) — the reader never sees a half-written file.
pub(crate) fn write_config_atomic(path: &std::path::Path, content: &str) -> Result<(), String> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, content).map_err(|e| format!("写入临时配置失败: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("原子替换配置失败: {e}")
    })
}

/// Apply a provider preset's model entries onto an in-memory config doc.
/// 纯函数：不做 IO，事务由调用方统一提交（v0.12.2 配置事务化）。
pub(crate) fn apply_provider_preset(
    doc: &mut toml_edit::DocumentMut,
    models: &[(&str, &str, &str)],
    base_url: &str,
) {
    let tbl = doc["model"]
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .expect("model 段");
    for (key, name, model) in models {
        let mut entry = toml_edit::Table::new();
        entry["model"] = toml_edit::value(*model);
        entry["name"] = toml_edit::value(*name);
        entry["base_url"] = toml_edit::value(base_url);
        entry["env_key"] = toml_edit::value(wancode_env_key(key));
        entry["api_backend"] = toml_edit::value("chat_completions");
        entry["context_window"] = toml_edit::value(128000i64);
        tbl.insert(key, toml_edit::Item::Table(entry));
    }
}

/// In-memory MCP seeding — same rules as before (marker once / never
/// overwrite), but operating on the doc so it commits in the SAME atomic
/// write as the models. Returns whether anything was seeded.
pub(crate) fn seed_default_mcp_into(doc: &mut toml_edit::DocumentMut) -> bool {
    let already = doc
        .get("ui")
        .and_then(|u| u.get("wancode_mcp_seeded"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if already {
        return false;
    }
    let servers = doc["mcp_servers"]
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .expect("mcp_servers 段");
    for (name, url) in [
        ("web-search", "https://open.bigmodel.cn/api/mcp/web_search/mcp"),
        ("web-reader", "https://open.bigmodel.cn/api/mcp/web_reader/mcp"),
    ] {
        if servers.contains_key(name) {
            continue;
        }
        let mut entry = toml_edit::Table::new();
        entry["url"] = toml_edit::value(url);
        let mut headers = toml_edit::Table::new();
        headers["Authorization"] = toml_edit::value("Bearer ${ZHIPU_API_KEY}");
        entry["headers"] = toml_edit::Item::Table(headers);
        servers.insert(name, toml_edit::Item::Table(entry));
    }
    let ui = doc["ui"]
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .expect("ui 段");
    ui["wancode_mcp_seeded"] = toml_edit::value(true);
    true
}


/// Startup model-config verdict. See `validate_startup_models`.
pub enum StartupModels {
    Ok,
    NoModels,
    /// default 悬空但已就地修复（写回磁盘），携带修复后的模型 id
    RepairedDefault(String),
    Invalid(String),
}

/// The single startup gate: every engine-boot path must pass through this.
///
/// 纯配置检查，不碰引擎。规则：
/// - 无 [model.*] 条目 → NoModels（前端应转向导）
/// - [models].default 指向不存在的模型 → 自动改指第一个存在的模型并写回；
///   写回失败 → Invalid
/// - 配置文件解析失败 → Invalid（绝不吞成"没有模型"，那会误开向导覆盖
///   用户配置的心智）
pub fn validate_startup_models() -> StartupModels {
    validate_startup_models_at(&user_config_path())
}

/// 路径可注入版本——单测用（首启状态矩阵，v0.12.2）。
pub fn validate_startup_models_at(path: &std::path::Path) -> StartupModels {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return StartupModels::NoModels,
        Err(e) => return StartupModels::Invalid(format!("读取配置失败: {e}")),
    };
    let mut doc: toml_edit::DocumentMut = match text.parse() {
        Ok(d) => d,
        Err(e) => return StartupModels::Invalid(format!("配置解析失败: {e}")),
    };

    let model_ids: Vec<String> = doc
        .get("model")
        .and_then(|v| v.as_table())
        .map(|t| {
            t.iter()
                .filter_map(|(_, e)| e.get("model").and_then(|v| v.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if model_ids.is_empty() {
        return StartupModels::NoModels;
    }

    let dangling = doc
        .get("models")
        .and_then(|m| m.get("default"))
        .and_then(|v| v.as_str())
        .map(|d| !model_ids.iter().any(|m| m == d))
        .unwrap_or(false);
    if dangling {
        let fixed = model_ids[0].clone();
        if let Some(models_tbl) = doc.get_mut("models").and_then(|v| v.as_table_mut()) {
            models_tbl["default"] = toml_edit::value(fixed.as_str());
        }
        if let Err(e) = std::fs::write(path, doc.to_string()) {
            return StartupModels::Invalid(format!("default 悬空且写回修复失败: {e}"));
        }
        return StartupModels::RepairedDefault(fixed);
    }
    StartupModels::Ok
}


#[cfg(test)]
mod config_txn_tests {
    use super::*;

    const GLM_OPEN: &[(&str, &str, &str)] =
        &[("glm", "智谱 GLM-5.2", "glm-5.2"), ("glm-air", "智谱 GLM-5-Air", "glm-5-air")];

    #[test]
    fn preset_writes_all_models_with_env_keys() {
        let mut doc = toml_edit::DocumentMut::new();
        apply_provider_preset(&mut doc, GLM_OPEN, "https://open.bigmodel.cn/api/paas/v4");
        let out = doc.to_string();
        assert!(out.contains(r#"[model.glm]"#));
        assert!(out.contains(r#"[model.glm-air]"#));
        assert!(out.contains(r#"env_key = "WANCODE_KEY_GLM""#));
        assert!(out.contains(r#"model = "glm-5-air""#));
    }

    #[test]
    fn seeding_is_once_and_never_overwrites() {
        let mut doc: toml_edit::DocumentMut = r#"
[mcp_servers.web-reader]
url = "https://example.com/custom"
"#
        .parse()
        .unwrap();
        assert!(seed_default_mcp_into(&mut doc));
        let out = doc.to_string();
        // 已有的 web-reader 绝不覆盖；web-search 补上；标记写入
        assert!(out.contains("https://example.com/custom"));
        assert!(!out.contains("web_reader/mcp"));
        assert!(out.contains("web_search/mcp"));
        assert!(out.contains("wancode_mcp_seeded = true"));
        // 零明文：只允许环境变量引用
        assert!(out.contains("Bearer ${ZHIPU_API_KEY}"));
        // 第二次是 no-op
        assert!(!seed_default_mcp_into(&mut doc));
    }

    #[test]
    fn atomic_write_replaces_existing() {
        let p = std::env::temp_dir().join(format!("wancode-atomic-{}.toml", std::process::id()));
        std::fs::write(&p, "old").unwrap();
        write_config_atomic(&p, "new-content").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new-content");
        // 临时文件不残留
        let tmp = p.with_extension(format!("tmp-{}", std::process::id()));
        assert!(!tmp.exists());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn preset_plus_seed_commit_in_one_doc() {
        // 模型与播种同一事务：单个 doc 内两者都在，一次写盘
        let mut doc = toml_edit::DocumentMut::new();
        apply_provider_preset(&mut doc, GLM_OPEN, "https://open.bigmodel.cn/api/paas/v4");
        assert!(seed_default_mcp_into(&mut doc));
        let out = doc.to_string();
        assert!(out.contains("[model.glm]") && out.contains("web_search/mcp"));
    }
}

#[cfg(test)]
mod startup_gate_tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(name: &str, content: Option<&str>) -> PathBuf {
        let p = std::env::temp_dir().join(format!("wancode-test-{name}-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&p);
        if let Some(c) = content {
            std::fs::write(&p, c).unwrap();
        }
        p
    }

    const VALID: &str = r#"
[models]
default = "glm-5.2"

[model.glm]
model = "glm-5.2"
name = "g"
base_url = "https://x"
"#;

    const DANGLING: &str = r#"
[models]
default = "ghost"

[model.glm]
model = "glm-5.2"
name = "g"
base_url = "https://x"
"#;

    #[test]
    fn no_file_means_no_models() {
        let p = tmp("nofile", None);
        assert!(matches!(validate_startup_models_at(&p), StartupModels::NoModels));
    }

    #[test]
    fn features_only_means_no_models() {
        let p = tmp("features", Some("[features]
telemetry = false
"));
        assert!(matches!(validate_startup_models_at(&p), StartupModels::NoModels));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn valid_config_is_ok() {
        let p = tmp("valid", Some(VALID));
        assert!(matches!(validate_startup_models_at(&p), StartupModels::Ok));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn dangling_default_gets_repaired_and_persisted() {
        let p = tmp("dangling", Some(DANGLING));
        assert!(matches!(
            validate_startup_models_at(&p),
            StartupModels::RepairedDefault(f) if f == "glm-5.2"
        ));
        // 修复必须落盘——只修内存等于没修（下次启动照样崩）
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(after.contains(r#"default = "glm-5.2""#));
        assert!(matches!(validate_startup_models_at(&p), StartupModels::Ok));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn broken_toml_is_invalid_not_no_models() {
        // 解析失败绝不能吞成 NoModels：那会开向导、把用户带向覆盖自己配置的路
        let p = tmp("broken", Some("[model.glm
model = \"x\""));
        assert!(matches!(validate_startup_models_at(&p), StartupModels::Invalid(_)));
        let _ = std::fs::remove_file(&p);
    }
}

