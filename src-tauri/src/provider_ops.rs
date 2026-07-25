//! v0.18-2 步 B：模型/供应商配置命令族（config.toml [model.*] + keyring）
//! 与 [mcp_servers] 配置命令。全部无 AgentState、无引擎调用——纯文件 IO
//! + keyring + HTTP 连接测试。红线注释随函数原样保留。
use serde::Serialize;

use crate::config_core::{
    apply_provider_preset, seed_default_mcp_into, user_config_path, wancode_env_key,
    write_config_atomic,
};

#[derive(Serialize, Clone)]
pub struct McpServerEntry {
    pub name: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub enabled: bool,
}


// ── Model / API providers (config.toml [model.*] + keyring) ─────────

pub(crate) const KEYRING_SERVICE: &str = "wancode-models";

#[derive(Serialize, Clone)]
pub struct ModelEntry {
    pub key: String,
    pub name: String,
    pub model: String,
    pub base_url: String,
    pub env_key: Option<String>,
    pub has_key: bool,
    /// True if this model's key lives in the WanCode keyring (editable here).
    pub managed: bool,
}

/// List model presets from config.toml.
#[tauri::command]
pub async fn model_list() -> Result<Vec<ModelEntry>, String> {
    let path = user_config_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("配置解析失败: {e}"))?;
    let mut out = Vec::new();
    if let Some(models) = doc.get("model").and_then(|v| v.as_table()) {
        for (key, item) in models.iter() {
            let t = item.as_table_like();
            let get = |k: &str| {
                t.and_then(|t| t.get(k))
                    .and_then(|v| v.as_str())
                    .map(String::from)
            };
            let env_key = get("env_key");
            let managed = env_key.as_deref() == Some(wancode_env_key(key).as_str());
            let has_key = if managed {
                keyring::Entry::new(KEYRING_SERVICE, key)
                    .ok()
                    .and_then(|e| e.get_password().ok())
                    .is_some()
            } else {
                env_key
                    .as_deref()
                    .map(|ek| std::env::var(ek).is_ok())
                    .unwrap_or(false)
                    || get("api_key").is_some()
            };
            out.push(ModelEntry {
                name: get("name").unwrap_or_else(|| key.to_string()),
                model: get("model").unwrap_or_else(|| key.to_string()),
                base_url: get("base_url").unwrap_or_default(),
                env_key,
                has_key,
                managed,
                key: key.to_string(),
            });
        }
    }
    Ok(out)
}

/// Add/update a model preset; stores the API key in the system keyring.
#[tauri::command]
pub async fn model_upsert(
    key: String,
    name: String,
    model: String,
    base_url: String,
    api_key: Option<String>,
) -> Result<(), String> {
    let key = key.trim().to_string();
    if key.is_empty() || model.trim().is_empty() || base_url.trim().is_empty() {
        return Err("名称、模型 ID、base_url 都不能为空".into());
    }
    let env_key = wancode_env_key(&key);
    if let Some(k) = api_key.as_ref().filter(|k| !k.trim().is_empty()) {
        keyring::Entry::new(KEYRING_SERVICE, &key)
            .and_then(|e| e.set_password(k.trim()))
            .map_err(|e| format!("保存密钥到钥匙串失败: {e}"))?;
    }
    let path = user_config_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("配置解析失败: {e}"))?;
    let models = doc["model"]
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .ok_or("model 段类型异常")?;
    let mut entry = toml_edit::Table::new();
    entry["model"] = toml_edit::value(model.trim());
    entry["name"] = toml_edit::value(name.trim());
    entry["base_url"] = toml_edit::value(base_url.trim());
    entry["env_key"] = toml_edit::value(&env_key);
    entry["api_backend"] = toml_edit::value("chat_completions");
    entry["context_window"] = toml_edit::value(128000i64);
    models.insert(&key, toml_edit::Item::Table(entry));
    std::fs::write(&path, doc.to_string()).map_err(|e| format!("写入配置失败: {e}"))
}

/// One-click provider setup for novice users: pick a preset, paste ONE key.
///
/// Writes every model of the preset (shared key in the keyring under each
/// model key), tests the first model, and for 智谱 presets seeds the default
/// web-search MCP servers (see seed_default_mcp).
///
/// Preset ids are stable API: "glm-coding" (Coding Plan 专属端点)、"glm-open"
/// (开放平台)、"deepseek".
#[tauri::command]
pub async fn provider_quick_setup(
    preset: String,
    api_key: String,
) -> Result<serde_json::Value, String> {
    provider_quick_setup_impl(preset, api_key, None, user_config_path()).await
}

/// 可注入版本（G2 单测用）：base_url_override 换掉预设端点、config_path
/// 换掉真实配置文件。生产路径两者都走默认。
pub(crate) async fn provider_quick_setup_impl(
    preset: String,
    api_key: String,
    base_url_override: Option<String>,
    config_path: std::path::PathBuf,
) -> Result<serde_json::Value, String> {
    let api_key = api_key.trim().to_string();
    if api_key.is_empty() {
        return Err("请输入 API Key".into());
    }
    // (key, 显示名, 模型ID)
    let (base_url, models): (&str, Vec<(&str, &str, &str)>) = match preset.as_str() {
        // Coding Plan 是包月订阅的专属端点——按量计费的开放平台 Key 在这里
        // 会 401，反之亦然。这正是小白最容易配错的地方，所以分成两张卡。
        "glm-coding" => (
            "https://open.bigmodel.cn/api/coding/paas/v4",
            vec![("glm-coding", "GLM Coding Plan", "glm-5.2")],
        ),
        "glm-open" => (
            "https://open.bigmodel.cn/api/paas/v4",
            vec![
                ("glm", "智谱 GLM-5.2", "glm-5.2"),
                ("glm-air", "智谱 GLM-5-Air", "glm-5-air"),
            ],
        ),
        "deepseek" => (
            "https://api.deepseek.com",
            vec![
                ("deepseek", "DeepSeek Chat", "deepseek-chat"),
                ("deepseek-r", "DeepSeek Reasoner", "deepseek-reasoner"),
            ],
        ),
        other => return Err(format!("未知预设: {other}")),
    };

    let base_url: String = base_url_override.unwrap_or_else(|| base_url.to_string());
    // 先测连接（用第一个模型），失败就不落任何配置——半配置状态最坑小白。
    let first_model = models[0].2;
    let test = model_test(
        base_url.clone(),
        first_model.to_string(),
        Some(api_key.clone()),
        None,
    )
    .await;
    if let Err(e) = test {
        return Err(format!("连接测试未通过，未保存任何配置。{e}"));
    }

    // ── 配置事务（v0.12.2）─────────────────────────────────────────
    // 顺序：内存组装完整 TOML（模型 + MCP 播种同一事务）→ 临时文件 →
    // 原子替换 → 钥匙串。钥匙串任一项失败 → 回滚本次新写入的钥匙串项 +
    // 原子写回原配置文本。任何路径下都不存在"半配置"。
    let path = config_path;
    let original = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut =
        original.parse().map_err(|e| format!("配置解析失败（原文件未动）: {e}"))?;
    apply_provider_preset(&mut doc, &models, &base_url);
    let mut seeded = false;
    if preset.starts_with("glm") {
        seeded = seed_default_mcp_into(&mut doc);
    }
    write_config_atomic(&path, &doc.to_string())?;

    let mut written_keys: Vec<&str> = Vec::new();
    for (key, _, _) in &models {
        match keyring::Entry::new(KEYRING_SERVICE, key).and_then(|e| e.set_password(&api_key)) {
            Ok(()) => written_keys.push(key),
            Err(e) => {
                // 回滚：删掉本次写入的钥匙串项，恢复原配置
                for k in &written_keys {
                    let _ = keyring::Entry::new(KEYRING_SERVICE, k)
                        .and_then(|en| en.delete_credential());
                }
                let _ = write_config_atomic(&path, &original);
                return Err(format!("保存密钥失败，已回滚全部改动: {e}"));
            }
        }
    }

    if preset.starts_with("glm") {
        // 让 ${ZHIPU_API_KEY} 即刻可解析（无需重启）。
        // Safety: 单线程配置路径，会话尚未启动或与其无关。
        unsafe { std::env::set_var("ZHIPU_API_KEY", &api_key) };
    }

    Ok(serde_json::json!({
        "models": models.iter().map(|(k, n, m)| serde_json::json!({
            "key": k, "name": n, "model": m
        })).collect::<Vec<_>>(),
        "testReply": test.ok(),
        "mcpSeeded": seeded,
    }))
}


/// Remove a model preset and its keyring entry.
#[tauri::command]
pub async fn model_remove(key: String) -> Result<(), String> {
    let _ = keyring::Entry::new(KEYRING_SERVICE, &key).and_then(|e| e.delete_credential());
    let path = user_config_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("配置解析失败: {e}"))?;
    let mut removed_model_id: Option<String> = None;
    let mut survivor_model_id: Option<String> = None;
    if let Some(models) = doc.get_mut("model").and_then(|v| v.as_table_mut()) {
        removed_model_id = models
            .get(&key)
            .and_then(|e| e.get("model"))
            .and_then(|v| v.as_str())
            .map(String::from);
        models.remove(&key);
        survivor_model_id = models
            .iter()
            .next()
            .and_then(|(_, e)| e.get("model"))
            .and_then(|v| v.as_str())
            .map(String::from);
    }
    // [models].default 指向被删模型时必须跟着清理：悬空 default 会让引擎在
    // 下次启动时直接 panic（capacity overflow，实测）。有幸存者就指过去，
    // 一个不剩就删掉整个 [models] 段（零模型时前端不会再启动引擎）。
    if let Some(removed) = removed_model_id {
        let dangling = doc
            .get("models")
            .and_then(|m| m.get("default"))
            .and_then(|v| v.as_str())
            .is_some_and(|d| d == removed);
        if dangling {
            match survivor_model_id {
                Some(next) => {
                    if let Some(models_tbl) = doc.get_mut("models").and_then(|v| v.as_table_mut()) {
                        models_tbl["default"] = toml_edit::value(next);
                    }
                }
                None => {
                    doc.remove("models");
                }
            }
        }
    }
    std::fs::write(&path, doc.to_string()).map_err(|e| format!("写入配置失败: {e}"))
}

/// Test a provider: minimal chat completion against base_url. Returns the
/// model's reply text on success, or an error string.
#[tauri::command]
pub async fn model_test(
    base_url: String,
    model: String,
    api_key: Option<String>,
    key: Option<String>,
) -> Result<String, String> {
    // Resolve the key: explicit api_key, else keyring by preset key.
    let token = match api_key.filter(|k| !k.trim().is_empty()) {
        Some(k) => k,
        None => key
            .and_then(|k| keyring::Entry::new(KEYRING_SERVICE, &k).ok())
            .and_then(|e| e.get_password().ok())
            .ok_or("没有可用的 API Key")?,
    };
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": model,
        "messages": [{ "role": "user", "content": "ping" }],
        "max_tokens": 5,
        "stream": false,
    });
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .bearer_auth(token.trim())
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("请求失败: {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        let msg = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()).map(String::from))
            .unwrap_or_else(|| text.chars().take(200).collect());
        return Err(format!("HTTP {}: {}", status.as_u16(), msg));
    }
    let reply = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| {
            v.get("choices")?
                .get(0)?
                .get("message")?
                .get("content")?
                .as_str()
                .map(String::from)
        })
        .unwrap_or_else(|| "(ok)".into());
    Ok(reply.chars().take(80).collect())
}

/// Migrate plaintext env-var keys into the OS keyring: for each preset whose
/// env_key is a plain env var (not WANCODE_KEY_*) that currently resolves,
/// copy the value into the keyring and switch the preset to a keyring-backed
/// env_key. Non-destructive to the user's system env vars. Returns count moved.
#[tauri::command]
pub async fn migrate_env_keys() -> Result<usize, String> {
    let path = user_config_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("配置解析失败: {e}"))?;
    let mut moved = 0usize;
    // Collect keys to migrate first (avoid borrow conflicts).
    let mut todo: Vec<(String, String)> = Vec::new(); // (preset_key, plaintext_value)
    if let Some(models) = doc.get("model").and_then(|v| v.as_table()) {
        for (key, item) in models.iter() {
            let env_key = item
                .as_table_like()
                .and_then(|t| t.get("env_key"))
                .and_then(|v| v.as_str());
            if let Some(ek) = env_key {
                if ek == wancode_env_key(key) {
                    continue; // already managed
                }
                if let Ok(val) = std::env::var(ek) {
                    if !val.is_empty() {
                        todo.push((key.to_string(), val));
                    }
                }
            }
        }
    }
    for (key, val) in todo {
        if keyring::Entry::new(KEYRING_SERVICE, &key)
            .and_then(|e| e.set_password(&val))
            .is_ok()
        {
            if let Some(models) = doc.get_mut("model").and_then(|v| v.as_table_mut()) {
                if let Some(entry) = models.get_mut(&key).and_then(|i| i.as_table_mut()) {
                    entry["env_key"] = toml_edit::value(wancode_env_key(&key));
                }
            }
            moved += 1;
        }
    }
    if moved > 0 {
        std::fs::write(&path, doc.to_string()).map_err(|e| format!("写入配置失败: {e}"))?;
    }
    Ok(moved)
}


/// Read `[mcp_servers]` entries from the user config.
#[tauri::command]
pub async fn mcp_config_list() -> Result<Vec<McpServerEntry>, String> {
    let path = user_config_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("配置解析失败: {e}"))?;
    let mut out = Vec::new();
    if let Some(servers) = doc.get("mcp_servers").and_then(|v| v.as_table()) {
        for (name, item) in servers.iter() {
            let t = item.as_table_like();
            let get_str = |k: &str| {
                t.and_then(|t| t.get(k))
                    .and_then(|v| v.as_str())
                    .map(String::from)
            };
            out.push(McpServerEntry {
                name: name.to_string(),
                command: get_str("command"),
                url: get_str("url"),
                args: t
                    .and_then(|t| t.get("args"))
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
                enabled: t
                    .and_then(|t| t.get("enabled"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true),
            });
        }
    }
    Ok(out)
}

/// Add or replace a stdio/HTTP MCP server in the user config.
#[tauri::command]
pub async fn mcp_config_upsert(
    name: String,
    command: Option<String>,
    args: Vec<String>,
    url: Option<String>,
) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("名称不能为空".into());
    }
    let path = user_config_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("配置解析失败: {e}"))?;
    let servers = doc["mcp_servers"]
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .ok_or("mcp_servers 段类型异常")?;
    let mut entry = toml_edit::Table::new();
    match (&command, &url) {
        (Some(cmd), _) if !cmd.trim().is_empty() => {
            entry["command"] = toml_edit::value(cmd.trim());
            if !args.is_empty() {
                let mut arr = toml_edit::Array::new();
                for a in &args {
                    arr.push(a.as_str());
                }
                entry["args"] = toml_edit::value(arr);
            }
        }
        (_, Some(u)) if !u.trim().is_empty() => {
            entry["url"] = toml_edit::value(u.trim());
        }
        _ => return Err("command 与 url 至少填一个".into()),
    }
    servers.insert(name.trim(), toml_edit::Item::Table(entry));
    std::fs::write(&path, doc.to_string()).map_err(|e| format!("写入配置失败: {e}"))
}

/// Remove an MCP server from the user config.
#[tauri::command]
pub async fn mcp_config_remove(name: String) -> Result<(), String> {
    let path = user_config_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("配置解析失败: {e}"))?;
    if let Some(servers) = doc.get_mut("mcp_servers").and_then(|v| v.as_table_mut()) {
        servers.remove(&name);
    }
    std::fs::write(&path, doc.to_string()).map_err(|e| format!("写入配置失败: {e}"))
}

/// Inject managed model keys from keyring into the process env so the engine's
/// `env_key` lookup resolves them. Call before starting a session.
pub(crate) fn inject_managed_keys() {
    let path = user_config_path();
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else { return };
    if let Some(models) = doc.get("model").and_then(|v| v.as_table()) {
        for (key, item) in models.iter() {
            let pw = keyring::Entry::new(KEYRING_SERVICE, key)
                .ok()
                .and_then(|e| e.get_password().ok());
            let Some(pw) = pw else { continue };
            let env_key = wancode_env_key(key);
            if std::env::var(&env_key).is_err() {
                // Safety: single-threaded startup path before session spawn.
                unsafe { std::env::set_var(&env_key, &pw) };
            }
            // 播种的默认 MCP 用 ${ZHIPU_API_KEY} 引用——智谱模型的 Key 顺带
            // 导出到这个名字，重启后 web-search MCP 才能解析出授权头。
            let is_zhipu = item
                .get("base_url")
                .and_then(|v| v.as_str())
                .is_some_and(|u| u.contains("bigmodel.cn"));
            if is_zhipu && std::env::var("ZHIPU_API_KEY").is_err() {
                unsafe { std::env::set_var("ZHIPU_API_KEY", &pw) };
            }
        }
    }
}

#[cfg(test)]
mod quick_setup_gate_tests {
    /// G2（AUTO 化）：连接测试失败 → 报错且**零落盘**。
    /// 端点用 127.0.0.1:9（保留端口，连接必拒），config 指向临时路径。
    /// 钥匙串零写入由失败点位置保证：连接测试在任何写入之前。
    #[tokio::test]
    async fn quick_setup_fails_closed_on_unreachable_endpoint() {
        let dir = std::env::temp_dir().join("wancode-g2-test");
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join(format!(
            "config-{}.toml",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let r = super::provider_quick_setup_impl(
            "glm-coding".into(),
            "sk-fake-key-for-test".into(),
            Some("http://127.0.0.1:9".into()),
            cfg.clone(),
        )
        .await;
        let err = r.expect_err("不可达端点必须失败");
        assert!(err.contains("未保存任何配置"), "错误应声明零落盘: {err}");
        assert!(!cfg.exists(), "config 不得被创建（fail-closed 破防）");
    }

    /// 未知预设直接拒绝，同样零落盘。
    #[tokio::test]
    async fn quick_setup_rejects_unknown_preset() {
        let cfg = std::env::temp_dir().join("wancode-g2-nopreset.toml");
        let _ = std::fs::remove_file(&cfg);
        let r = super::provider_quick_setup_impl(
            "not-a-preset".into(),
            "sk-x".into(),
            None,
            cfg.clone(),
        )
        .await;
        assert!(r.is_err());
        assert!(!cfg.exists());
    }
}

/// v0.18.6 步 0（RED）：模型身份不变量。
///
/// 根因（双证据）：`session/acp_session_impl/model_switch.rs:13` 把
/// `sampling_config.model`（上游 slug）包成 `model_id` 写进
/// `PersistenceMsg::CurrentModel`；恢复时按 slug 反查、`.rev()` 最后一个胜出。
/// 磁盘实证：config key = `my-test-model` 的会话，events.jsonl 里存的是
/// `"model_id":"glm-4.6"`。两个条目共用同一 slug（同模型走不同代理，是合法
/// 需求）时，恢复会话就会串到别的端点——用户实报的正是这一现象。
///
/// 不变量：config key 是唯一身份；slug 只有唯一匹配时才允许兜底；重复时
/// 必须报歧义让用户选，**绝不猜**。
#[cfg(test)]
mod model_identity_tests {
    use agent_client_protocol as acp;
    use indexmap::IndexMap;
    use xai_grok_shell::agent::config::{ModelEntry, ModelInfo};
    use xai_grok_shell::agent::models::{resolve_persisted_model, PersistedModelResolution};

    fn entry(slug: &str, base_url: &str, name: &str) -> ModelEntry {
        let mut info = ModelInfo::fallback(slug);
        info.base_url = base_url.to_owned();
        info.name = Some(name.to_owned());
        ModelEntry { info, api_key: None, env_key: None, api_base_url: None }
    }

    /// 目录：两个条目共用 slug `glm-4.6`，分别指向智谱官方与自建代理。
    fn dup_slug_catalog() -> (IndexMap<String, ModelEntry>, IndexMap<acp::ModelId, acp::ModelInfo>) {
        let mut models = IndexMap::new();
        models.insert(
            "zhipu-glm".to_owned(),
            entry("glm-4.6", "https://open.bigmodel.cn/api/paas/v4", "GLM 智谱官方"),
        );
        models.insert(
            "my-test-model".to_owned(),
            entry("glm-4.6", "https://api.company.com/v1", "GLM 公司代理"),
        );
        let available = available_of(&models);
        (models, available)
    }

    fn available_of(
        models: &IndexMap<String, ModelEntry>,
    ) -> IndexMap<acp::ModelId, acp::ModelInfo> {
        models
            .iter()
            .map(|(k, e)| {
                let id = acp::ModelId::new(k.clone());
                let name = e.info.name.clone().unwrap_or_else(|| e.info.model.clone());
                (id.clone(), acp::ModelInfo::new(id, name))
            })
            .collect()
    }

    /// RED ①：新格式会话带 catalog_model_id —— slug 重复也必须精确命中原条目。
    /// 这是修复的核心：`.rev()` 猜测被 config key 取代。
    #[test]
    fn exact_catalog_key_wins_over_duplicate_slug() {
        let (models, available) = dup_slug_catalog();
        let r = resolve_persisted_model(&models, &available, Some("my-test-model"), "glm-4.6");
        assert_eq!(
            r,
            PersistedModelResolution::Exact(acp::ModelId::new("my-test-model")),
            "带 catalog_model_id 的会话必须精确恢复到该 config key，不得按 slug 重猜"
        );
    }

    /// RED ②：旧格式（无 catalog_model_id）+ slug 唯一 → 自动迁移。
    #[test]
    fn legacy_unique_slug_migrates_to_key() {
        let mut models = IndexMap::new();
        models.insert(
            "only-one".to_owned(),
            entry("glm-4.6", "https://api.company.com/v1", "唯一条目"),
        );
        let available = available_of(&models);
        let r = resolve_persisted_model(&models, &available, None, "glm-4.6");
        assert_eq!(
            r,
            PersistedModelResolution::Migrated(acp::ModelId::new("only-one")),
            "旧会话 slug 唯一匹配时应自动迁移为 config key"
        );
    }

    /// RED ③：旧格式 + slug 重复 → 必须歧义报错，附带候选，绝不静默挑一个。
    /// endpoint_label 只给主机名（不得含完整 URL/查询参数/Key）。
    #[test]
    fn legacy_duplicate_slug_is_ambiguous_never_guesses() {
        let (models, available) = dup_slug_catalog();
        let r = resolve_persisted_model(&models, &available, None, "glm-4.6");
        match r {
            PersistedModelResolution::Ambiguous { legacy_model, candidates } => {
                assert_eq!(legacy_model, "glm-4.6");
                let ids: Vec<_> = candidates.iter().map(|c| c.id.as_str()).collect();
                assert_eq!(ids, vec!["zhipu-glm", "my-test-model"], "候选需按目录顺序稳定给出");
                let labels: Vec<_> = candidates.iter().map(|c| c.endpoint_label.as_str()).collect();
                assert_eq!(
                    labels,
                    vec!["open.bigmodel.cn", "api.company.com"],
                    "endpoint_label 必须只有主机名"
                );
                assert!(
                    candidates.iter().all(|c| !c.endpoint_label.contains("http")
                        && !c.endpoint_label.contains('/')),
                    "endpoint_label 不得泄漏完整 URL"
                );
            }
            other => panic!("重复 slug 必须报歧义，实际: {other:?}"),
        }
    }

    /// RED ④：catalog_model_id 指向已删除的条目、但 slug 唯一 → 迁移到幸存条目。
    #[test]
    fn stale_catalog_key_with_unique_slug_migrates() {
        let mut models = IndexMap::new();
        models.insert(
            "survivor".to_owned(),
            entry("glm-4.6", "https://api.company.com/v1", "幸存条目"),
        );
        let available = available_of(&models);
        let r = resolve_persisted_model(&models, &available, Some("deleted-key"), "glm-4.6");
        assert_eq!(
            r,
            PersistedModelResolution::Migrated(acp::ModelId::new("survivor")),
            "key 已删但 slug 唯一时应迁移，而不是判定找不到"
        );
    }

    /// RED ⑤：catalog_model_id 已删除且 slug 重复 → 要求用户重选。
    #[test]
    fn stale_catalog_key_with_duplicate_slug_requires_user_choice() {
        let (models, available) = dup_slug_catalog();
        let r = resolve_persisted_model(&models, &available, Some("deleted-key"), "glm-4.6");
        assert!(
            matches!(r, PersistedModelResolution::Ambiguous { .. }),
            "key 已删且 slug 重复时必须让用户重选，实际: {r:?}"
        );
    }

    /// 轻量特征测试（按 Codex gate 1 重新定位）：仅证明目录条目各自携带自己的
    /// base_url，**不构成主路由保护**——它证明不了
    /// `setModel → resolve_model_id → prepare_sampling_config_for_model → base_url/credentials`
    /// 这条真实链路未被破坏（那只是 IndexMap::get 能用）。主路由保护由双 mock
    /// 端点集成测试承担：正确端点收到 1 次请求且拿到对应 Key，错误端点 0 次
    /// 请求且绝不收到另一个 Key——列为 v0.18.6 合并硬门槛。
    #[test]
    fn direct_key_lookup_keeps_its_own_endpoint() {
        let (models, _) = dup_slug_catalog();
        assert_eq!(
            models.get("my-test-model").unwrap().info.base_url,
            "https://api.company.com/v1"
        );
        assert_eq!(
            models.get("zhipu-glm").unwrap().info.base_url,
            "https://open.bigmodel.cn/api/paas/v4"
        );
    }
}

/// Gate 4（Codex 复核）：端点标签脱敏必须被测试钉死——原先的测试数据里
/// 根本没有 userinfo，等于「实现声称脱敏、测试从未验证」。
#[cfg(test)]
mod endpoint_label_redaction_tests {
    use xai_grok_shell::agent::models::endpoint_authority_label;

    /// URL 里嵌了凭据、路径、查询、片段——标签只能留 authority。
    /// 同时覆盖 IPv6 + 端口：UI 需要 `localhost:9999` 这类可区分的标签，
    /// 所以保留端口是有意为之（函数因此叫 authority 而非 host）。
    #[test]
    fn userinfo_path_query_fragment_are_all_stripped() {
        let label = endpoint_authority_label("https://user:pass@[::1]:9999/path?q=secret#frag");
        assert_eq!(label, "[::1]:9999");
        for leaked in ["user", "pass", "path", "secret", "frag", "http", "/", "?", "#", "@"] {
            assert!(
                !label.contains(leaked),
                "端点标签泄漏了 {leaked:?}：{label}（凭据/路径绝不能进 UI）"
            );
        }
    }

    #[test]
    fn plain_https_keeps_host_and_drops_path() {
        assert_eq!(
            endpoint_authority_label("https://open.bigmodel.cn/api/paas/v4"),
            "open.bigmodel.cn"
        );
    }

    /// 端口保留（同主机不同代理端口需可区分）。
    #[test]
    fn port_is_kept_for_disambiguation() {
        assert_eq!(endpoint_authority_label("http://localhost:9999/v1"), "localhost:9999");
    }
}

/// v0.18.6 步1b（RED）：Codex 第三轮复核指出的两处语义风险。
///
/// 风险一：交互切换用 `resolve_catalog_key`（`.rev()` last-wins）解析请求 id。
/// 客户端发 slug 且重复时，它静默猜一个，而我们随后把这个猜测**永久写入**
/// `catalog_model_id`——等于把一次错误猜测洗成权威身份，比修复前更糟。
/// 用户选择路径必须严格：精确 key / slug 唯一 / 重复报错 / 不存在报错。
///
/// 风险二：`catalog_model_id: None` 当前语义是"保留旧 key"。对 CurrentModel
/// 这种"模型已变更"的消息，保留陈旧身份意味着 slug 换了而 key 没换，恢复时
/// key 优先 → 回到错误模型。必须三态显式化。
#[cfg(test)]
mod model_selection_tests {
    use agent_client_protocol as acp;
    use indexmap::IndexMap;
    use xai_grok_shell::agent::config::{ModelEntry, ModelInfo};
    use xai_grok_shell::agent::models::{
        next_catalog_model_id, resolve_requested_model, CatalogModelPatch, ModelSelectionError,
    };

    fn entry(slug: &str, base_url: &str, name: &str) -> ModelEntry {
        let mut info = ModelInfo::fallback(slug);
        info.base_url = base_url.to_owned();
        info.name = Some(name.to_owned());
        ModelEntry { info, api_key: None, env_key: None, api_base_url: None }
    }

    fn dup_catalog() -> (IndexMap<String, ModelEntry>, IndexMap<acp::ModelId, acp::ModelInfo>) {
        let mut models = IndexMap::new();
        models.insert(
            "zhipu-glm".to_owned(),
            entry("glm-4.6", "https://open.bigmodel.cn/api/paas/v4", "GLM 智谱官方"),
        );
        models.insert(
            "my-test-model".to_owned(),
            entry("glm-4.6", "https://api.company.com/v1", "GLM 公司代理"),
        );
        let available = models
            .keys()
            .map(|k| {
                let id = acp::ModelId::new(k.clone());
                (id.clone(), acp::ModelInfo::new(id, k.clone()))
            })
            .collect();
        (models, available)
    }

    /// RED：精确 key 必须原样成立，且带回它自己的条目（一次解析出 key+entry，
    /// 避免"先 resolve_model_id 找 entry、再另一个函数找 key"两次解析将来又分歧）。
    #[test]
    fn exact_key_resolves_with_its_own_entry() {
        let (models, available) = dup_catalog();
        let sel = resolve_requested_model(&models, &available, "my-test-model")
            .expect("精确 key 必须解析成功");
        assert_eq!(sel.catalog_key, acp::ModelId::new("my-test-model"));
        assert_eq!(sel.entry.info.base_url, "https://api.company.com/v1");
    }

    /// RED：slug 唯一时兼容成功并归一到 key。
    #[test]
    fn unique_slug_resolves_to_its_key() {
        let mut models = IndexMap::new();
        models.insert("only".to_owned(), entry("glm-4.6", "https://api.company.com/v1", "唯一"));
        let available = models
            .keys()
            .map(|k| {
                let id = acp::ModelId::new(k.clone());
                (id.clone(), acp::ModelInfo::new(id, k.clone()))
            })
            .collect();
        let sel = resolve_requested_model(&models, &available, "glm-4.6").expect("唯一 slug 应成功");
        assert_eq!(sel.catalog_key, acp::ModelId::new("only"));
    }

    /// RED（本轮核心）：重复 slug 必须报歧义，**绝不 last-wins 猜测**。
    /// 猜出来的 key 会被持久化，把错误固化成身份。
    #[test]
    fn duplicate_slug_is_rejected_not_guessed() {
        let (models, available) = dup_catalog();
        match resolve_requested_model(&models, &available, "glm-4.6") {
            Err(ModelSelectionError::Ambiguous { requested, candidates }) => {
                assert_eq!(requested, "glm-4.6");
                let ids: Vec<_> = candidates.iter().map(|c| c.id.as_str()).collect();
                assert_eq!(ids, vec!["zhipu-glm", "my-test-model"]);
            }
            other => panic!("重复 slug 必须报歧义而非猜测，实际: {other:?}"),
        }
    }

    /// RED：不存在的 id 报 Unknown，不得回退到任意条目。
    #[test]
    fn unknown_id_is_rejected() {
        let (models, available) = dup_catalog();
        assert!(matches!(
            resolve_requested_model(&models, &available, "nope"),
            Err(ModelSelectionError::Unknown(_))
        ));
    }

    /// RED：三态补丁语义——Clear 必须真的清除陈旧身份。
    /// 场景：已有 catalog=proxy-a，某内部路径把模型换成别的 slug 却不知道 key。
    /// 若保留 proxy-a，恢复时 key 优先 → 回到错误模型。
    #[test]
    fn clear_wipes_stale_identity() {
        let existing = acp::ModelId::new("proxy-a");
        assert_eq!(next_catalog_model_id(Some(&existing), &CatalogModelPatch::Clear), None);
    }

    /// RED：Set 覆盖；Preserve 才保留（保留必须显式，不能是 None 的默认行为）。
    #[test]
    fn set_overwrites_and_preserve_is_explicit() {
        let existing = acp::ModelId::new("proxy-a");
        let next = acp::ModelId::new("proxy-b");
        assert_eq!(
            next_catalog_model_id(Some(&existing), &CatalogModelPatch::Set(next.clone())),
            Some(next)
        );
        assert_eq!(
            next_catalog_model_id(Some(&existing), &CatalogModelPatch::Preserve),
            Some(existing)
        );
    }
}

/// v0.18.6 步1c（RED）：切断"猜测 → 权威身份"这条链。
///
/// 953b50e 把 last-wins 的 resolve_catalog_key 结果直接持久化成
/// catalog_model_id。原来每次恢复都是"临时猜错"，那样一改就变成"猜测被写成
/// 权威记录"——危害升级。修复原则：**绝不持久化猜测**。请求无法唯一识别时
/// 清除身份（回落 slug 解析，会去问用户），而不是发明一个。
///
/// 注意这只决定"持久化什么"，刻意不拦截切换本身——共享 apply 是 ungated
/// 底层，new_session/load_session 直接调用它，内部隐藏模型必须继续可用。
#[cfg(test)]
mod switch_identity_patch_tests {
    use agent_client_protocol as acp;
    use indexmap::IndexMap;
    use xai_grok_shell::agent::config::{ModelEntry, ModelInfo};
    use xai_grok_shell::agent::models::{catalog_patch_for_switch, CatalogModelPatch};

    fn entry(slug: &str, base_url: &str) -> ModelEntry {
        let mut info = ModelInfo::fallback(slug);
        info.base_url = base_url.to_owned();
        ModelEntry { info, api_key: None, env_key: None, api_base_url: None }
    }

    fn catalog(
        pairs: &[(&str, &str, &str)],
    ) -> (IndexMap<String, ModelEntry>, IndexMap<acp::ModelId, acp::ModelInfo>) {
        let mut models = IndexMap::new();
        for (key, slug, url) in pairs {
            models.insert((*key).to_owned(), entry(slug, url));
        }
        let available = models
            .keys()
            .map(|k| {
                let id = acp::ModelId::new(k.clone());
                (id.clone(), acp::ModelInfo::new(id, k.clone()))
            })
            .collect();
        (models, available)
    }

    /// 明确的 key → 写入该身份。
    #[test]
    fn unambiguous_request_sets_its_key() {
        let (m, a) = catalog(&[
            ("zhipu-glm", "glm-4.6", "https://open.bigmodel.cn/api/paas/v4"),
            ("my-test-model", "glm-4.6", "https://api.company.com/v1"),
        ]);
        assert_eq!(
            catalog_patch_for_switch(&m, &a, "my-test-model"),
            CatalogModelPatch::Set(acp::ModelId::new("my-test-model"))
        );
    }

    /// RED 核心：重复 slug 绝不写入猜出来的 key——必须 Clear。
    #[test]
    fn ambiguous_slug_never_persists_a_guess() {
        let (m, a) = catalog(&[
            ("zhipu-glm", "glm-4.6", "https://open.bigmodel.cn/api/paas/v4"),
            ("my-test-model", "glm-4.6", "https://api.company.com/v1"),
        ]);
        assert_eq!(
            catalog_patch_for_switch(&m, &a, "glm-4.6"),
            CatalogModelPatch::Clear,
            "重复 slug 时写入任一 key 都是把猜测洗成权威身份"
        );
    }

    /// 唯一 slug 走兼容路径：归一化成 key 后写入（广播/持久化都用归一后的 key）。
    #[test]
    fn unique_slug_normalizes_to_its_key() {
        let (m, a) = catalog(&[("only", "glm-4.6", "https://api.company.com/v1")]);
        assert_eq!(
            catalog_patch_for_switch(&m, &a, "glm-4.6"),
            CatalogModelPatch::Set(acp::ModelId::new("only"))
        );
    }

    /// 未知 id：清除而非保留陈旧身份。
    #[test]
    fn unknown_request_clears_stale_identity() {
        let (m, a) = catalog(&[("only", "glm-4.6", "https://api.company.com/v1")]);
        assert_eq!(catalog_patch_for_switch(&m, &a, "nope"), CatalogModelPatch::Clear);
    }
}
