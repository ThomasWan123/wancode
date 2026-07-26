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
        assert_eq!(*sel.catalog_key(), acp::ModelId::new("my-test-model"));
        assert_eq!(sel.entry().info.base_url, "https://api.company.com/v1");
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
        assert_eq!(*sel.catalog_key(), acp::ModelId::new("only"));
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
    use xai_grok_shell::agent::models::{catalog_patch_for_ungated_switch, CatalogModelPatch};

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
        let (m, _a) = catalog(&[
            ("zhipu-glm", "glm-4.6", "https://open.bigmodel.cn/api/paas/v4"),
            ("my-test-model", "glm-4.6", "https://api.company.com/v1"),
        ]);
        assert_eq!(
            catalog_patch_for_ungated_switch(&m, "my-test-model"),
            CatalogModelPatch::Set(acp::ModelId::new("my-test-model"))
        );
    }

    /// RED 核心：重复 slug 绝不写入猜出来的 key——必须 Clear。
    #[test]
    fn ambiguous_slug_never_persists_a_guess() {
        let (m, _a) = catalog(&[
            ("zhipu-glm", "glm-4.6", "https://open.bigmodel.cn/api/paas/v4"),
            ("my-test-model", "glm-4.6", "https://api.company.com/v1"),
        ]);
        assert_eq!(
            catalog_patch_for_ungated_switch(&m, "glm-4.6"),
            CatalogModelPatch::Clear,
            "重复 slug 时写入任一 key 都是把猜测洗成权威身份"
        );
    }

    /// 唯一 slug 走兼容路径：归一化成 key 后**持久化**。
    /// 注意本函数只管持久化——共享 apply 的广播目前仍可能用原始 requested
    /// slug，广播/handle/持久化三者统一到 canonical key 要等 apply_resolved
    /// 拆分。文档不提前宣布未实现的行为。
    #[test]
    fn unique_slug_normalizes_to_its_key() {
        let (m, _a) = catalog(&[("only", "glm-4.6", "https://api.company.com/v1")]);
        assert_eq!(
            catalog_patch_for_ungated_switch(&m, "glm-4.6"),
            CatalogModelPatch::Set(acp::ModelId::new("only"))
        );
    }

    /// 未知 id：清除而非保留陈旧身份。
    #[test]
    fn unknown_request_clears_stale_identity() {
        let (m, _a) = catalog(&[("only", "glm-4.6", "https://api.company.com/v1")]);
        assert_eq!(catalog_patch_for_ungated_switch(&m, "nope"), CatalogModelPatch::Clear);
    }
}

/// Codex 第六轮：给 ungated 底层用的持久化判断，本身也必须是 ungated 的。
///
/// 我上一版用 `available`（仅用户可选）过滤，只想到"不拦截切换"，没想到
/// 反方向：内部路径传**精确的隐藏模型 key** 时 available 里没有它 →
/// 解析失败 → Clear → **一个完全准确的身份被抹掉**。切换当次成功，但下次
/// 恢复只剩 slug，隐藏模型可能恢复不了或重新歧义。所谓"零副作用"不成立。
#[cfg(test)]
mod hidden_model_identity_tests {
    use indexmap::IndexMap;
    use xai_grok_shell::agent::config::{ModelEntry, ModelInfo};
    use xai_grok_shell::agent::models::{catalog_patch_for_ungated_switch, CatalogModelPatch};
    use agent_client_protocol as acp;

    /// 隐藏模型：user_selectable=false（不会出现在 available 里）。
    fn hidden(slug: &str, base_url: &str) -> ModelEntry {
        let mut info = ModelInfo::fallback(slug);
        info.base_url = base_url.to_owned();
        info.user_selectable = false;
        ModelEntry { info, api_key: None, env_key: None, api_base_url: None }
    }

    fn visible(slug: &str, base_url: &str) -> ModelEntry {
        let mut info = ModelInfo::fallback(slug);
        info.base_url = base_url.to_owned();
        ModelEntry { info, api_key: None, env_key: None, api_base_url: None }
    }

    /// 精确的隐藏 key 必须被持久化——它是完全准确的身份，不在用户菜单里
    /// 不代表它不真实。
    #[test]
    fn exact_hidden_key_is_still_persisted() {
        let mut m = IndexMap::new();
        m.insert("internal-hidden".to_owned(), hidden("grok-internal", "https://internal/v1"));
        assert_eq!(
            catalog_patch_for_ungated_switch(&m, "internal-hidden"),
            CatalogModelPatch::Set(acp::ModelId::new("internal-hidden")),
            "隐藏模型的精确 key 被清除会导致下次恢复只剩 slug"
        );
    }

    /// 隐藏模型的唯一 slug 同样归一化成 key。
    #[test]
    fn hidden_unique_slug_normalizes() {
        let mut m = IndexMap::new();
        m.insert("internal-hidden".to_owned(), hidden("grok-internal", "https://internal/v1"));
        assert_eq!(
            catalog_patch_for_ungated_switch(&m, "grok-internal"),
            CatalogModelPatch::Set(acp::ModelId::new("internal-hidden"))
        );
    }

    /// 但隐藏模型参与的重复 slug 仍必须 Clear——不猜这条规则对隐藏模型一视同仁。
    #[test]
    fn hidden_duplicate_slug_still_clears() {
        let mut m = IndexMap::new();
        m.insert("internal-hidden".to_owned(), hidden("glm-4.6", "https://internal/v1"));
        m.insert("public-proxy".to_owned(), visible("glm-4.6", "https://api.company.com/v1"));
        assert_eq!(
            catalog_patch_for_ungated_switch(&m, "glm-4.6"),
            CatalogModelPatch::Clear
        );
    }
}

/// v0.18.6 步2（RED，feature branch）：Codex 指定的四条硬边界测试。
///
/// 其中"同名字面值"三条是他两次点名要的回归护栏——防止未来开发者重新混淆
/// "请求里的精确 key"与"旧文件里的 slug"。目录构造刻意让一个条目的 key
/// 字面上等于另一个条目的 slug：
///     key = glm-4.6      slug = glm-4.6   （条目 A）
///     key = company-proxy slug = glm-4.6   （条目 B）
#[cfg(test)]
mod literal_name_collision_tests {
    use agent_client_protocol as acp;
    use indexmap::IndexMap;
    use xai_grok_shell::agent::config::{ModelEntry, ModelInfo};
    use xai_grok_shell::agent::models::{
        resolve_persisted_model, resolve_requested_model, PersistedModelResolution,
    };

    fn entry(slug: &str, base_url: &str, selectable: bool) -> ModelEntry {
        let mut info = ModelInfo::fallback(slug);
        info.base_url = base_url.to_owned();
        info.user_selectable = selectable;
        ModelEntry { info, api_key: None, env_key: None, api_base_url: None }
    }

    fn collision_catalog(
    ) -> (IndexMap<String, ModelEntry>, IndexMap<acp::ModelId, acp::ModelInfo>) {
        let mut models = IndexMap::new();
        // A：key 字面就叫 glm-4.6，slug 也是 glm-4.6
        models.insert("glm-4.6".to_owned(), entry("glm-4.6", "https://official/v1", true));
        // B：key 不同，但 slug 与 A 相同
        models.insert("company-proxy".to_owned(), entry("glm-4.6", "https://proxy/v1", true));
        let available = models
            .iter()
            .filter(|(_, e)| e.info.user_selectable)
            .map(|(k, _)| {
                let id = acp::ModelId::new(k.clone());
                (id.clone(), acp::ModelInfo::new(id, k.clone()))
            })
            .collect();
        (models, available)
    }

    /// ①用户 setModel("glm-4.6")：作为**精确 key** 解析，选中 A，不报歧义。
    #[test]
    fn user_selection_prefers_exact_key_over_shared_slug() {
        let (m, a) = collision_catalog();
        let sel = resolve_requested_model(&m, &a, "glm-4.6").expect("精确 key 必须胜出");
        assert_eq!(*sel.catalog_key(), acp::ModelId::new("glm-4.6"));
        assert_eq!(sel.entry().info.base_url, "https://official/v1");
    }

    /// ②旧会话只有 model_id="glm-4.6"、无 catalog key：必须当**旧 slug** 处理
    /// → 两个条目都匹配 → 歧义。
    /// 这条是整个修复的关键排序：slug 绝不能先当 key 查，否则字面键为 glm-4.6
    /// 的条目会静默压过同 slug 的代理条目，歧义检查根本不触发。
    #[test]
    fn legacy_session_treats_it_as_slug_and_reports_ambiguity() {
        let (m, a) = collision_catalog();
        match resolve_persisted_model(&m, &a, None, "glm-4.6") {
            PersistedModelResolution::Ambiguous { candidates, .. } => {
                let ids: Vec<_> = candidates.iter().map(|c| c.id.as_str()).collect();
                assert_eq!(ids, vec!["glm-4.6", "company-proxy"]);
            }
            other => panic!("旧格式必须按 slug 处理并报歧义，实际: {other:?}"),
        }
    }

    /// ③新会话带 catalog_model_id="glm-4.6"：精确恢复 A。
    #[test]
    fn new_format_session_restores_exact_key() {
        let (m, a) = collision_catalog();
        assert_eq!(
            resolve_persisted_model(&m, &a, Some("glm-4.6"), "glm-4.6"),
            PersistedModelResolution::Exact(acp::ModelId::new("glm-4.6"))
        );
    }

    /// ④（疑似真 RED）内部隐藏模型的恢复不得被 selectable gate 挡住。
    /// 与上一轮 catalog_patch_for_ungated_switch 误用 available 是同类问题：
    /// 隐藏模型有精确 catalog key 却不在 available 里，若恢复按可选性过滤，
    /// 精确身份会解析失败 → 会话恢复不到它自己的模型。
    #[test]
    fn hidden_model_with_exact_key_still_restores() {
        let mut models = IndexMap::new();
        models.insert(
            "internal-hidden".to_owned(),
            entry("grok-internal", "https://internal/v1", false),
        );
        // available 只含可选模型——隐藏模型不在其中
        let available: IndexMap<acp::ModelId, acp::ModelInfo> = IndexMap::new();
        assert_eq!(
            resolve_persisted_model(&models, &available, Some("internal-hidden"), "grok-internal"),
            PersistedModelResolution::Exact(acp::ModelId::new("internal-hidden")),
            "隐藏模型的精确 catalog key 必须能恢复——恢复不是用户选择，不该过 selectable gate"
        );
    }
}

/// v0.18.6 步2b（RED）：歧义必须是**结构化** ACP 错误。
///
/// Codex 的理由是产品性的，不是洁癖：前端要做"加载历史但暂停发送、要求
/// 用户选模型"的 UX，就必须从错误里拿到候选列表。一句人话 message 前端
/// 只能弹个提示，做不了选择器。
#[cfg(test)]
mod ambiguous_error_shape_tests {
    use agent_client_protocol as acp;
    use indexmap::IndexMap;
    use xai_grok_shell::agent::config::{ModelEntry, ModelInfo};
    use xai_grok_shell::agent::models::{
        resolve_requested_model, AmbiguousModelError, ModelSelectionError, MODEL_AMBIGUOUS,
    };

    fn dup_catalog() -> (IndexMap<String, ModelEntry>, IndexMap<acp::ModelId, acp::ModelInfo>) {
        let mut models = IndexMap::new();
        // 两个 key 都不等于 slug，确保走到歧义分支而非精确 key 命中
        for (key, base) in [
            ("zhipu", "https://user:s3cret@open.bigmodel.cn/api/coding/paas/v4?token=abc"),
            ("proxy", "https://llm.corp.internal:8443/v1"),
        ] {
            let mut info = ModelInfo::fallback("glm-4.6");
            info.base_url = base.to_owned();
            info.name = Some(format!("GLM 4.6 ({key})"));
            models.insert(
                key.to_owned(),
                ModelEntry { info, api_key: None, env_key: None, api_base_url: None },
            );
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

    #[test]
    fn ambiguous_carries_candidates_the_frontend_can_render() {
        let (m, a) = dup_catalog();
        let err = resolve_requested_model(&m, &a, "glm-4.6").expect_err("重复 slug 必须失败");
        let parsed = AmbiguousModelError::from_acp_error(&err.into_acp_error())
            .expect("歧义必须是结构化载荷，不能只是一句 message");
        assert_eq!(parsed.code, MODEL_AMBIGUOUS);
        assert_eq!(parsed.requested, "glm-4.6");
        let labels: Vec<_> = parsed
            .candidates
            .iter()
            .map(|c| (c.id.as_str(), c.endpoint_label.as_str()))
            .collect();
        assert_eq!(
            labels,
            vec![
                ("zhipu", "open.bigmodel.cn"),
                ("proxy", "llm.corp.internal:8443"),
            ],
            "候选必须带可区分的端点标签——用户正是靠它分辨同名模型"
        );
    }

    /// 端点标签是要显示给用户、也要进日志的，绝不能把凭据带出来。
    #[test]
    fn structured_payload_never_leaks_credentials_or_query() {
        let (m, a) = dup_catalog();
        let err = resolve_requested_model(&m, &a, "glm-4.6").unwrap_err();
        let json = serde_json::to_string(&match err {
            ModelSelectionError::Ambiguous { requested, candidates } => {
                AmbiguousModelError::new(requested, candidates)
            }
            other => panic!("expected ambiguous, got {other:?}"),
        })
        .unwrap();
        for leak in ["s3cret", "token=abc", "user:", "/api/coding"] {
            assert!(!json.contains(leak), "结构化载荷泄漏了 {leak}: {json}");
        }
    }

    /// 未知模型不是歧义——不能把两种失败混成一个码，否则前端会对着空候选
    /// 列表弹选择器。
    #[test]
    fn unknown_model_is_not_reported_as_ambiguous() {
        let (m, a) = dup_catalog();
        let err = resolve_requested_model(&m, &a, "no-such-model").unwrap_err();
        assert!(matches!(err, ModelSelectionError::Unknown(_)));
        assert!(AmbiguousModelError::from_acp_error(&err.into_acp_error()).is_none());
    }
}

/// v0.18.6 步2c：原子解析——key 与 entry 必须同时来自同一条目。
///
/// 这是 apply_resolved 存在的全部理由。旧路径分两步取（resolve_model_id 拿
/// entry、另一处 resolve_catalog_key 拿 key），两步用不同的匹配规则
/// （first-match vs last-wins），重复 slug 下就能取出 A 的 key 配 B 的 entry
/// ——也就是"选中模型 X，请求却发往 Y 的端点"。
#[cfg(test)]
mod atomic_resolution_tests {
    use agent_client_protocol as acp;
    use indexmap::IndexMap;
    use xai_grok_shell::agent::config::{ModelEntry, ModelInfo};
    use xai_grok_shell::agent::models::resolve_trusted_model;

    fn catalog() -> IndexMap<String, ModelEntry> {
        let mut m = IndexMap::new();
        for (key, slug, base) in [
            ("glm-coding", "glm-4.6", "https://open.bigmodel.cn/api/coding/paas/v4"),
            ("glm-open", "glm-4.6", "https://open.bigmodel.cn/api/paas/v4"),
            ("solo", "deepseek-chat", "https://api.deepseek.com"),
        ] {
            let mut info = ModelInfo::fallback(slug);
            info.base_url = base.to_owned();
            m.insert(
                key.to_owned(),
                ModelEntry { info, api_key: None, env_key: None, api_base_url: None },
            );
        }
        m
    }

    /// 精确 key 必须连带它**自己**的 entry —— 这正是 v0.18.5 那个 bug 的形状：
    /// 选 Coding Plan 端点，请求却发到开放平台端点。
    #[test]
    fn exact_key_carries_its_own_endpoint_not_a_slug_sibling() {
        let sel = resolve_trusted_model(&catalog(), "glm-coding").expect("精确 key");
        assert_eq!(*sel.catalog_key(), acp::ModelId::new("glm-coding"));
        assert_eq!(sel.entry().info.base_url, "https://open.bigmodel.cn/api/coding/paas/v4");
        assert_eq!(sel.entry().info.model, "glm-4.6", "上游 slug 仍是共享的那个");
    }

    #[test]
    fn unique_slug_carries_the_matching_entry() {
        let sel = resolve_trusted_model(&catalog(), "deepseek-chat").expect("唯一 slug");
        assert_eq!(*sel.catalog_key(), acp::ModelId::new("solo"));
        assert_eq!(sel.entry().info.base_url, "https://api.deepseek.com");
    }

    /// 重复 slug 返回 None 而不是随便挑一个：内部路径宁可退回默认模型，
    /// 也不能把一次抛硬币当成身份写进会话。
    #[test]
    fn duplicate_slug_yields_none_never_a_coin_flip() {
        assert!(resolve_trusted_model(&catalog(), "glm-4.6").is_none());
        assert!(resolve_trusted_model(&catalog(), "nope").is_none());
    }
}

/// v0.18.6 步3（RED）：歧义的**存在与否**必须按全目录判定。
///
/// Codex 第十轮纠正的一条规则，比我原来的写法更严：available 只该决定
/// "给用户展示哪些可选操作"，不该决定"是不是存在歧义"。我原先按可选集
/// 统计匹配数，理由是"隐藏模型没法当候选给用户点"——但由此推出的行为是
/// 静默选中那个可见的，这本身就是猜，正是前几轮反复禁止的那件事。
/// 候选是否可选，改由 ModelCandidate.selectable 表达，交给前端渲染。
#[cfg(test)]
mod ambiguity_spans_full_catalog_tests {
    use agent_client_protocol as acp;
    use indexmap::IndexMap;
    use xai_grok_shell::agent::config::{ModelEntry, ModelInfo};
    use xai_grok_shell::agent::models::{resolve_persisted_model, PersistedModelResolution};

    fn entry(slug: &str, base: &str, selectable: bool) -> ModelEntry {
        let mut info = ModelInfo::fallback(slug);
        info.base_url = base.to_owned();
        info.user_selectable = selectable;
        ModelEntry { info, api_key: None, env_key: None, api_base_url: None }
    }

    /// 一可见 + 一隐藏、同 slug、旧会话只有 slug：真实情况就是两个候选，
    /// 不能因为其中一个用户点不了就当它不存在、静默迁移到另一个。
    #[test]
    fn one_visible_one_hidden_sharing_a_slug_is_still_ambiguous() {
        let mut models = IndexMap::new();
        models.insert("visible-proxy".to_owned(), entry("glm-4.6", "https://proxy/v1", true));
        models.insert("hidden-model".to_owned(), entry("glm-4.6", "https://internal/v1", false));
        let available: IndexMap<acp::ModelId, acp::ModelInfo> = [("visible-proxy", ())]
            .iter()
            .map(|(k, _)| {
                let id = acp::ModelId::new((*k).to_owned());
                (id.clone(), acp::ModelInfo::new(id, (*k).to_owned()))
            })
            .collect();

        match resolve_persisted_model(&models, &available, None, "glm-4.6") {
            PersistedModelResolution::Ambiguous { candidates, .. } => {
                let seen: Vec<_> = candidates
                    .iter()
                    .map(|c| (c.id.as_str(), c.selectable))
                    .collect();
                assert_eq!(
                    seen,
                    vec![("visible-proxy", true), ("hidden-model", false)],
                    "两个候选都要给出，可选性用字段表达而不是靠过滤掉一个"
                );
            }
            other => panic!("必须报歧义，绝不静默选可见的那个，实际: {other:?}"),
        }
    }

    /// 全目录唯一才迁移——即使那唯一一条是隐藏的。
    #[test]
    fn unique_across_full_catalog_migrates_even_when_hidden() {
        let mut models = IndexMap::new();
        models.insert("hidden-only".to_owned(), entry("grok-internal", "https://i/v1", false));
        let available: IndexMap<acp::ModelId, acp::ModelInfo> = IndexMap::new();
        assert_eq!(
            resolve_persisted_model(&models, &available, None, "grok-internal"),
            PersistedModelResolution::Migrated(acp::ModelId::new("hidden-only"))
        );
    }
}

/// v0.18.6 步4（RED）：被阻塞的会话如何解锁。
///
/// Codex 第十一轮抓到的新 P0：歧义会话虽然在加载时被挂起，但 prompt 路径
/// 用旧的 last-wins 解析器自动解锁——第一次发消息就重新猜一次，并且这次
/// 会被 apply_resolved 固化成权威身份。我上一轮"历史可读、暂停发送、等待
/// 用户选择"的说法因此是假的。
///
/// 解锁决策抽成纯函数，两类阻塞共用同一条解析路径，旧 last-wins 彻底出局。
#[cfg(test)]
mod blocked_session_recovery_tests {
    use agent_client_protocol as acp;
    use indexmap::IndexMap;
    use xai_grok_shell::agent::config::{ModelEntry, ModelInfo};
    use xai_grok_shell::agent::models::{
        recover_blocked_model, ModelCandidate, ModelSessionBlock,
    };

    fn entry(slug: &str, base: &str) -> ModelEntry {
        let mut info = ModelInfo::fallback(slug);
        info.base_url = base.to_owned();
        ModelEntry { info, api_key: None, env_key: None, api_base_url: None }
    }

    fn catalog(
        entries: &[(&str, &str, &str)],
    ) -> (IndexMap<String, ModelEntry>, IndexMap<acp::ModelId, acp::ModelInfo>) {
        let mut models = IndexMap::new();
        for (key, slug, base) in entries {
            models.insert((*key).to_owned(), entry(slug, base));
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

    fn ambiguous_block() -> ModelSessionBlock {
        ModelSessionBlock::Ambiguous {
            requested: "glm-4.6".to_owned(),
            candidates: vec![
                ModelCandidate {
                    id: "zhipu".to_owned(),
                    name: "zhipu".to_owned(),
                    endpoint_label: "open.bigmodel.cn".to_owned(),
                    selectable: true,
                },
                ModelCandidate {
                    id: "proxy".to_owned(),
                    name: "proxy".to_owned(),
                    endpoint_label: "llm.corp".to_owned(),
                    selectable: true,
                },
            ],
        }
    }

    /// 核心一条：歧义仍然存在时，发消息绝不能把锁解开。
    #[test]
    fn ambiguous_block_survives_a_prompt_while_still_ambiguous() {
        let (m, a) = catalog(&[
            ("zhipu", "glm-4.6", "https://open.bigmodel.cn/v1"),
            ("proxy", "glm-4.6", "https://llm.corp/v1"),
        ]);
        assert_eq!(
            recover_blocked_model(&m, &a, &ambiguous_block()),
            None,
            "第一次 prompt 不得靠 last-wins 自行解锁——那正是原始事故"
        );
    }

    /// 目录变了、slug 在全目录里变成唯一，这时解锁不是猜，可以放行。
    #[test]
    fn ambiguous_block_clears_once_the_catalog_makes_it_unique() {
        let (m, a) = catalog(&[("zhipu", "glm-4.6", "https://open.bigmodel.cn/v1")]);
        assert_eq!(
            recover_blocked_model(&m, &a, &ambiguous_block()),
            Some(acp::ModelId::new("zhipu"))
        );
    }

    /// 不可用类阻塞的复查同样必须走新解析器：slug 仍对应多条时不许挑一个。
    #[test]
    fn unavailable_block_never_falls_back_to_last_wins() {
        let (m, a) = catalog(&[
            ("zhipu", "glm-4.6", "https://open.bigmodel.cn/v1"),
            ("proxy", "glm-4.6", "https://llm.corp/v1"),
        ]);
        let block = ModelSessionBlock::Unavailable {
            persisted_model: acp::ModelId::new("glm-4.6"),
        };
        assert_eq!(recover_blocked_model(&m, &a, &block), None);
    }

    /// 模型真的回来了、且唯一，才自动恢复。
    #[test]
    fn unavailable_block_recovers_when_the_model_is_back_and_unique() {
        let (m, a) = catalog(&[("zhipu", "glm-4.6", "https://open.bigmodel.cn/v1")]);
        let block = ModelSessionBlock::Unavailable {
            persisted_model: acp::ModelId::new("glm-4.6"),
        };
        assert_eq!(
            recover_blocked_model(&m, &a, &block),
            Some(acp::ModelId::new("zhipu"))
        );
    }

    /// 解析得到的 key 当前不可选时不放行——恢复要的是"现在能用"。
    #[test]
    fn recovery_requires_the_model_to_be_usable_now() {
        let (m, _) = catalog(&[("zhipu", "glm-4.6", "https://open.bigmodel.cn/v1")]);
        let empty: IndexMap<acp::ModelId, acp::ModelInfo> = IndexMap::new();
        let block = ModelSessionBlock::Unavailable {
            persisted_model: acp::ModelId::new("glm-4.6"),
        };
        assert_eq!(recover_blocked_model(&m, &empty, &block), None);
    }
}

/// v0.18.6 步5（RED）：恢复必须"先落地、后解锁"，且这次测的是真实容器。
///
/// Codex 第十二轮两条纠正，都对：
/// 1. 原顺序是先 remove(block) 再 apply，两条失败分支只 warn 就继续发消息
///    ——恢复失败的会话不但发了，而且下次也拦不住了。
/// 2. 我上一条说"5 条测试覆盖状态转移"是过度表述：那 5 条只测了纯决策
///    函数 recover_blocked_model，从没碰过存放 block 的那张表。
/// 所以落定动作抽成 settle_recovery，对真实 HashMap 操作，下面直接断言表本身。
#[cfg(test)]
mod recovery_commit_order_tests {
    use agent_client_protocol as acp;
    use std::collections::HashMap;
    use xai_grok_shell::agent::models::{
        settle_recovery, ModelSessionBlock, RecoveryOutcome,
    };

    fn blocked() -> HashMap<String, ModelSessionBlock> {
        let mut m = HashMap::new();
        m.insert(
            "sess-1".to_owned(),
            ModelSessionBlock::Unavailable {
                persisted_model: acp::ModelId::new("glm-4.6"),
            },
        );
        m
    }

    /// 切换失败：表里 block 必须还在，且不许继续发用户消息。
    #[test]
    fn failed_apply_keeps_the_block_and_holds_the_prompt() {
        let mut blocks = blocked();
        assert_eq!(settle_recovery(&mut blocks, "sess-1", false), RecoveryOutcome::Hold);
        assert!(
            blocks.contains_key("sess-1"),
            "恢复失败却把 block 删了，等于这次发错、下次也拦不住"
        );
    }

    /// 切换成功：block 清除，放行。
    #[test]
    fn successful_apply_clears_the_block_and_continues() {
        let mut blocks = blocked();
        assert_eq!(settle_recovery(&mut blocks, "sess-1", true), RecoveryOutcome::Continue);
        assert!(!blocks.contains_key("sess-1"));
    }

    /// 只动本会话，别的会话的 block 不受影响。
    #[test]
    fn settling_one_session_leaves_other_blocks_alone() {
        let mut blocks = blocked();
        blocks.insert(
            "sess-2".to_owned(),
            ModelSessionBlock::Ambiguous {
                requested: "glm-4.6".to_owned(),
                candidates: Vec::new(),
            },
        );
        settle_recovery(&mut blocks, "sess-1", true);
        assert!(blocks.contains_key("sess-2"));
    }
}

/// v0.18.6 Gate 1a：身份 → 端点 + 凭据，全链路无串台。
///
/// 这是整件事最初的事故形态：用户新建了一个自定义模型，聊天区选中它，
/// 请求却发去了智谱非 coding 端点。此前每一轮的验证都停在"解析出的 key
/// 对不对"，而真正伤人的是 key 之后那两跳——entry 决定 base_url，entry
/// 决定用哪把 Key。两个条目共享同一个上游 slug 时，这两跳只要有一跳按
/// slug 而不是按 entry 走，就串台。
///
/// 这条测试跑的是真实生产函数链：
///     resolve_requested_model → selection.entry()
///                             → resolve_credentials → sampling_config_for_model
/// 覆盖到"请求参数"为止。它**不**覆盖 SamplerConfig 交给 HTTP 客户端之后
/// 的那一跳（那部分在 xai-grok-http 内部），所以 Gate 1 尚未完全关闭。
#[cfg(test)]
mod endpoint_and_key_isolation_tests {
    use agent_client_protocol as acp;
    use indexmap::IndexMap;
    use xai_grok_shell::agent::config::{
        resolve_credentials, sampling_config_for_model, ModelEntry, ModelInfo,
    };
    use xai_grok_shell::agent::models::resolve_requested_model;

    const ZHIPU_OPEN: &str = "https://open.bigmodel.cn/api/paas/v4";
    const ZHIPU_CODING: &str = "https://open.bigmodel.cn/api/coding/paas/v4";

    const OPEN: (&str, &str, &str) = ("glm-open", ZHIPU_OPEN, "key-for-open-platform");
    const CODING: (&str, &str, &str) = ("glm-coding", ZHIPU_CODING, "key-for-coding-plan");

    /// 两个条目共享上游 slug `glm-4.6`，但端点与 Key 完全不同——正是事故里
    /// "开放平台 vs Coding Plan"那对，两者的 Key 互不通用，串台即 401。
    /// 插入顺序由参数决定：last-wins 的症状就是"排最后的那个赢"，不把顺序
    /// 真的换过来，就谈不上验证过顺序无关。
    fn crossover_catalog(
        order: [(&str, &str, &str); 2],
    ) -> (IndexMap<String, ModelEntry>, IndexMap<acp::ModelId, acp::ModelInfo>) {
        let mut models = IndexMap::new();
        for (key, base, api_key) in order {
            let mut info = ModelInfo::fallback("glm-4.6");
            info.base_url = base.to_owned();
            models.insert(
                key.to_owned(),
                ModelEntry {
                    info,
                    api_key: Some(api_key.to_owned()),
                    env_key: None,
                    api_base_url: None,
                },
            );
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

    /// 断言的是 **SamplerConfig 自己的字段**，不是它上游那份 ResolvedCredentials
    /// 的副本。之前我读的是副本，等于 sampling_config_for_model 把端点或 Key
    /// 写错了测试也照样绿——那是假覆盖。
    fn route(order: [(&str, &str, &str); 2], key: &str) -> (String, Option<String>, String) {
        let (models, available) = crossover_catalog(order);
        let selection = resolve_requested_model(&models, &available, key)
            .unwrap_or_else(|e| panic!("{key} 必须能精确解析: {e:?}"));
        let creds = resolve_credentials(selection.entry(), None);
        let sampling =
            sampling_config_for_model(selection.entry(), creds, None, None, None, None);
        (sampling.base_url, sampling.api_key, sampling.model)
    }

    /// 选 Coding Plan：必须发往 coding 端点、带 coding 的 Key，
    /// 且绝不能出现开放平台那一方的任何东西。
    #[test]
    fn choosing_coding_plan_never_leaks_into_the_open_platform_entry() {
        let (base_url, api_key, model) = route([OPEN, CODING], "glm-coding");
        assert_eq!(base_url, ZHIPU_CODING);
        assert_eq!(api_key.as_deref(), Some("key-for-coding-plan"));
        assert_ne!(base_url, ZHIPU_OPEN, "端点串台——这正是用户报的那个 bug");
        assert_ne!(
            api_key.as_deref(),
            Some("key-for-open-platform"),
            "Key 串台——两个平台的 Key 互不通用，串了就是 401"
        );
        // 上游 slug 仍然是共享的那个：身份归一化不该改写发给上游的模型名。
        assert_eq!(model, "glm-4.6");
    }

    /// 反方向同样成立。只测一个方向正是我前几轮反复犯的错。
    #[test]
    fn choosing_open_platform_never_leaks_into_the_coding_entry() {
        let (base_url, api_key, model) = route([OPEN, CODING], "glm-open");
        assert_eq!(base_url, ZHIPU_OPEN);
        assert_eq!(api_key.as_deref(), Some("key-for-open-platform"));
        assert_ne!(base_url, ZHIPU_CODING);
        assert_ne!(api_key.as_deref(), Some("key-for-coding-plan"));
        assert_eq!(model, "glm-4.6");
    }

    /// 目录顺序不得影响路由——这次真的把顺序换过来。
    ///
    /// 上一版这条测试两次调用的是同一个目录，插入顺序根本没变，只不过再次
    /// 证明了两个 key 不相等；名字写了顺序，测试里没有顺序。last-wins 恰恰
    /// 只在顺序变化时露出马脚，所以那是最不该省的一维。
    #[test]
    fn routing_is_identical_under_both_catalog_orders() {
        for key in ["glm-coding", "glm-open"] {
            let forward = route([OPEN, CODING], key);
            let reversed = route([CODING, OPEN], key);
            assert_eq!(
                forward, reversed,
                "{key} 的路由随目录顺序变了——说明结果来自位置而不是 entry"
            );
        }
        // 并且两个 key 在任一顺序下都各走各的，没有并到同一端点。
        for order in [[OPEN, CODING], [CODING, OPEN]] {
            let (c_base, c_key, _) = route(order, "glm-coding");
            let (o_base, o_key, _) = route(order, "glm-open");
            assert_eq!(c_base, ZHIPU_CODING);
            assert_eq!(o_base, ZHIPU_OPEN);
            // 断言等于"各自正确的那把"，不是只断言两把不相等：端点没换、
            // 凭据互换的顺序 bug 也满足 !=，却照样是 401。
            assert_eq!(c_key.as_deref(), Some("key-for-coding-plan"));
            assert_eq!(o_key.as_deref(), Some("key-for-open-platform"));
        }
    }
}

/// Gate 1a 续：直接复刻事故形态——从**持久化记录**出发决定端点。
///
/// 上面三条用的是精确 key，而精确 key 那条路原本就没坏，所以它们守的是
/// 不变量、不是事故。真正出事的形态是：会话文件里只存了上游 slug，恢复时
/// 拿 slug 去猜 key，猜中了另一家代理的条目，于是请求发去了别人的端点。
/// 这里从 Summary 的两种形态出发，一路走到端点。
#[cfg(test)]
mod persisted_record_routing_tests {
    use agent_client_protocol as acp;
    use indexmap::IndexMap;
    use xai_grok_shell::agent::config::{resolve_credentials, ModelEntry, ModelInfo};
    use xai_grok_shell::agent::models::{resolve_persisted_model, PersistedModelResolution};

    const ZHIPU_OPEN: &str = "https://open.bigmodel.cn/api/paas/v4";
    const ZHIPU_CODING: &str = "https://open.bigmodel.cn/api/coding/paas/v4";

    fn catalog() -> (IndexMap<String, ModelEntry>, IndexMap<acp::ModelId, acp::ModelInfo>) {
        let mut models = IndexMap::new();
        for (key, base, api_key) in [
            ("glm-open", ZHIPU_OPEN, "key-for-open-platform"),
            ("glm-coding", ZHIPU_CODING, "key-for-coding-plan"),
        ] {
            let mut info = ModelInfo::fallback("glm-4.6");
            info.base_url = base.to_owned();
            models.insert(
                key.to_owned(),
                ModelEntry {
                    info,
                    api_key: Some(api_key.to_owned()),
                    env_key: None,
                    api_base_url: None,
                },
            );
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

    /// 旧格式记录（只有 slug）：绝不许解析出任何端点。
    /// 这一条在修复前是会失败的——旧的 .rev() 扫描会挑中目录里最后那条，
    /// 也就是 glm-coding，于是用户明明配的是开放平台却发去了 coding 端点，
    /// 反之亦然。现在它必须停在歧义上，一个端点都不选。
    #[test]
    fn legacy_slug_only_record_resolves_to_no_endpoint_at_all() {
        let (models, available) = catalog();
        match resolve_persisted_model(&models, &available, None, "glm-4.6") {
            PersistedModelResolution::Ambiguous { candidates, .. } => {
                let endpoints: Vec<_> =
                    candidates.iter().map(|c| c.endpoint_label.as_str()).collect();
                assert_eq!(endpoints.len(), 2, "两家都要列出来给用户选");
            }
            other => panic!(
                "旧格式记录必须停在歧义，不得自行选出一个端点，实际: {other:?}"
            ),
        }
    }

    /// 新格式记录（带 catalog_model_id）：精确恢复到它自己的端点和 Key。
    #[test]
    fn new_format_record_restores_its_own_endpoint_and_key() {
        let (models, available) = catalog();
        for (key, want_base, want_key) in [
            ("glm-coding", ZHIPU_CODING, "key-for-coding-plan"),
            ("glm-open", ZHIPU_OPEN, "key-for-open-platform"),
        ] {
            let resolved =
                match resolve_persisted_model(&models, &available, Some(key), "glm-4.6") {
                    PersistedModelResolution::Exact(id) => id,
                    other => panic!("{key} 必须精确恢复，实际: {other:?}"),
                };
            let entry = models.get(resolved.0.as_ref()).expect("key 必在目录中");
            let creds = resolve_credentials(entry, None);
            assert_eq!(creds.base_url, want_base);
            assert_eq!(creds.api_key.as_deref(), Some(want_key));
        }
    }
}

/// v0.18.6 步6（RED）：新会话的第一次 Summary 落盘必须**同一次写入**里带上
/// 双身份（current_model_id + catalog_model_id）。
///
/// 崩溃窗口：此前 Summary::new 硬编码 catalog_model_id: None，先落盘，
/// 等 apply_resolved 的 CurrentModel 消息再补 key。两步之间进程崩溃/断电，
/// 磁盘上就留下一份"只有 slug 语义"的记录——如果目录里恰有同名字面 key
/// 与共享 slug（glm-4.6 那对），下次恢复直接掉进歧义，用户被迫重选一个
/// 他明明已经选过的模型。原子双写把这个窗口整个删掉。
///
/// 测试直接读**磁盘上的 summary.json**，不是内存对象——写入原子性只能在
/// 落盘产物上验证。
#[cfg(test)]
mod atomic_dual_identity_write_tests {
    use agent_client_protocol as acp;
    use xai_grok_shell::session::info::Info;
    use xai_grok_shell::session::storage::jsonl::JsonlStorageAdapter;
    use xai_grok_shell::session::storage::StorageAdapter;

    fn temp_session_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wancode-dual-write-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn info(dir: &std::path::Path) -> Info {
        Info {
            id: acp::SessionId::new("dual-write-test"),
            cwd: dir.to_string_lossy().into_owned(),
        }
    }

    /// 首写即双身份：磁盘 JSON 里两个字段都在，且都是传进去的值。
    #[tokio::test]
    async fn first_summary_write_carries_both_identities() {
        let dir = temp_session_dir("first");
        let adapter = JsonlStorageAdapter::with_explicit_session_dir(dir.clone());
        adapter
            .init_session_with_catalog(
                &info(&dir),
                acp::ModelId::new("glm-4.6"),
                Some(acp::ModelId::new("glm-coding")),
            )
            .await
            .unwrap();
        let raw = std::fs::read_to_string(dir.join("summary.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(json["current_model_id"], "glm-4.6");
        assert_eq!(
            json["catalog_model_id"], "glm-coding",
            "第一次落盘就必须有 key——这之后的任何一步崩溃都不再产生无身份记录"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 调用方不知道 key 时（None）不得编造：字段整个缺席，落回旧格式语义。
    #[tokio::test]
    async fn unknown_key_is_omitted_not_invented() {
        let dir = temp_session_dir("none");
        let adapter = JsonlStorageAdapter::with_explicit_session_dir(dir.clone());
        adapter
            .init_session_with_catalog(&info(&dir), acp::ModelId::new("glm-4.6"), None)
            .await
            .unwrap();
        let raw = std::fs::read_to_string(dir.join("summary.json")).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(
            json.get("catalog_model_id").is_none(),
            "不知道就空着——宁可留空绝不写错，这条原则在步1c已经立过"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 已存在的会话再 init（恢复路径）：磁盘上已有的身份不得被覆盖。
    #[tokio::test]
    async fn reinit_of_existing_session_preserves_stored_identity() {
        let dir = temp_session_dir("reinit");
        let adapter = JsonlStorageAdapter::with_explicit_session_dir(dir.clone());
        adapter
            .init_session_with_catalog(
                &info(&dir),
                acp::ModelId::new("glm-4.6"),
                Some(acp::ModelId::new("glm-coding")),
            )
            .await
            .unwrap();
        // 二次 init 传入不同身份——存在即加载，绝不改写。
        let summary = adapter
            .init_session_with_catalog(
                &info(&dir),
                acp::ModelId::new("other-slug"),
                Some(acp::ModelId::new("other-key")),
            )
            .await
            .unwrap();
        assert_eq!(summary.current_model_id, acp::ModelId::new("glm-4.6"));
        assert_eq!(
            summary.catalog_model_id,
            Some(acp::ModelId::new("glm-coding")),
            "已有记录的身份是历史事实，init 只许读不许写"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// v0.18.6 步6b（RED）：首写双身份的**组装**也要有测试——字段各归其位。
///
/// Codex 第十六轮拒收步6的理由：存储层三条测试只证明 init_session 收到
/// 正确的 (slug, key) 后能一次写入，没证明 new_session 真的传了正确的一对。
/// 而生产代码恰好把两个字段写成了同一个 catalog key：current_model_id 的
/// 语义是上游 slug，旧版本/同步端/一切只读该字段的消费者会把配置键当
/// 上游模型名。组装逻辑抽成纯函数，用真实事故样本测。
#[cfg(test)]
mod initial_identity_assembly_tests {
    use agent_client_protocol as acp;
    use indexmap::IndexMap;
    use xai_grok_shell::agent::config::{ModelEntry, ModelInfo};
    use xai_grok_shell::agent::models::initial_persisted_identity;

    fn catalog_with(key: &str, slug: &str) -> IndexMap<String, ModelEntry> {
        let mut models = IndexMap::new();
        models.insert(
            key.to_owned(),
            ModelEntry {
                info: ModelInfo::fallback(slug),
                api_key: None,
                env_key: None,
                api_base_url: None,
            },
        );
        models
    }

    /// 事故样本：entry key=glm-coding、slug=glm-4.6。
    /// 首写必须是 (current=glm-4.6, catalog=Some(glm-coding))——各归其位，
    /// 不是两个字段都写 key。
    #[test]
    fn key_and_slug_land_in_their_own_fields() {
        let models = catalog_with("glm-coding", "glm-4.6");
        let (current, catalog) = initial_persisted_identity(
            &models,
            &acp::ModelId::new("glm-coding"),
            "glm-4.6",
        );
        assert_eq!(current, acp::ModelId::new("glm-4.6"), "current_model_id 是上游 slug");
        assert_eq!(
            catalog,
            Some(acp::ModelId::new("glm-coding")),
            "catalog_model_id 是配置键"
        );
    }

    /// 运行时 id 不在目录里（目录拉取还没完成等）：key 留空不编造，
    /// slug 仍取 sampling config 的——那是真正要发给上游的名字。
    #[test]
    fn unknown_runtime_id_leaves_the_key_empty() {
        let models = catalog_with("glm-coding", "glm-4.6");
        let (current, catalog) = initial_persisted_identity(
            &models,
            &acp::ModelId::new("vanished-model"),
            "some-upstream-slug",
        );
        assert_eq!(current, acp::ModelId::new("some-upstream-slug"));
        assert_eq!(catalog, None);
    }

    /// key 存在但其条目的 slug 与将要持久化的 slug 不一致：fail-closed，
    /// key 不写。裸 contains_key 会把"A 的 key 配 B 的模型"记成权威身份——
    /// 目录热重载与建会话竞态、或未来调用者传错参数时都可能发生。
    #[test]
    fn key_whose_entry_slug_mismatches_is_dropped() {
        let models = catalog_with("glm-coding", "glm-4.6");
        let (current, catalog) = initial_persisted_identity(
            &models,
            &acp::ModelId::new("glm-coding"),
            "some-other-slug",
        );
        assert_eq!(current, acp::ModelId::new("some-other-slug"));
        assert_eq!(
            catalog, None,
            "担保不了的身份宁可留空——key 与 slug 必须作为一对被验证"
        );
    }

    /// key 与 slug 字面相同（key=glm-4.6, slug=glm-4.6）：两个字段值相同是
    /// 巧合而非混淆，key 照写——这份记录在同名字面值目录下恢复时靠它免于歧义。
    #[test]
    fn literal_coincidence_still_writes_the_key() {
        let models = catalog_with("glm-4.6", "glm-4.6");
        let (current, catalog) = initial_persisted_identity(
            &models,
            &acp::ModelId::new("glm-4.6"),
            "glm-4.6",
        );
        assert_eq!(current, acp::ModelId::new("glm-4.6"));
        assert_eq!(catalog, Some(acp::ModelId::new("glm-4.6")));
    }
}

/// v0.18.6 步7（RED）：fork/copy 的模型身份继承。
///
/// 现状：copy_session_data_sync 里硬写 catalog_model_id: None（注释还是我
/// 早先写的"由后续 SetSessionModel 或加载期迁移写入"）。于是源会话明明有
/// 精确 key，fork 出来只剩 slug——重复 slug 目录下一加载就是歧义，用户被迫
/// 重选一个上游会话早就定过的模型。新会话首写补上了窗口，fork 这条路还漏着。
///
/// 规则（Codex 定）：
///   无 new_model_id → 原样继承源的两个字段
///   显式 key → 写它
///   唯一 slug → 归一化成它的 key
///   重复 / 未知 → 留空，绝不猜
///
/// 无覆盖时的继承不需要目录（照抄两个字段即可），所以只有"有覆盖"这条
/// 需要严格解析——解析发生在持有目录的调用方，存储层不碰目录。
#[cfg(test)]
mod fork_identity_tests {
    use agent_client_protocol as acp;
    use indexmap::IndexMap;
    use xai_grok_shell::agent::config::{ModelEntry, ModelInfo};
    use xai_grok_shell::agent::models::{fork_model_override, inherited_fork_identity};

    fn entry(slug: &str, base: &str) -> ModelEntry {
        let mut info = ModelInfo::fallback(slug);
        info.base_url = base.to_owned();
        ModelEntry { info, api_key: None, env_key: None, api_base_url: None }
    }

    fn catalog() -> (IndexMap<String, ModelEntry>, IndexMap<acp::ModelId, acp::ModelInfo>) {
        let mut models = IndexMap::new();
        models.insert("glm-open".to_owned(), entry("glm-4.6", "https://open/v1"));
        models.insert("glm-coding".to_owned(), entry("glm-4.6", "https://coding/v1"));
        models.insert("solo".to_owned(), entry("deepseek-chat", "https://ds/v1"));
        let available = models
            .keys()
            .map(|k| {
                let id = acp::ModelId::new(k.clone());
                (id.clone(), acp::ModelInfo::new(id, k.clone()))
            })
            .collect();
        (models, available)
    }

    /// 无覆盖：两个字段原样继承。源的 key 是它自己定过的事实，fork 不该丢。
    #[test]
    fn without_override_both_fields_are_inherited_verbatim() {
        let got = inherited_fork_identity(
            &acp::ModelId::new("glm-4.6"),
            Some(&acp::ModelId::new("glm-coding")),
        );
        assert_eq!(
            got,
            (acp::ModelId::new("glm-4.6"), Some(acp::ModelId::new("glm-coding"))),
            "fork 丢掉源 key，等于让子会话重新掉进它父辈已经解决过的歧义"
        );
    }

    /// 源本来就没有 key（旧格式会话）：继承后依然没有，不凭空造。
    #[test]
    fn without_override_a_keyless_source_stays_keyless() {
        assert_eq!(
            inherited_fork_identity(&acp::ModelId::new("glm-4.6"), None),
            (acp::ModelId::new("glm-4.6"), None)
        );
    }

    /// 显式给精确 key：slug 取该条目的上游名，key 写它自己。
    #[test]
    fn explicit_key_override_resolves_to_slug_and_key() {
        let (m, a) = catalog();
        assert_eq!(
            fork_model_override(&m, &a, "glm-coding"),
            (acp::ModelId::new("glm-4.6"), Some(acp::ModelId::new("glm-coding")))
        );
    }

    /// 给的是全目录唯一的 slug：归一化成它的 key。
    #[test]
    fn unique_slug_override_is_normalized_to_its_key() {
        let (m, a) = catalog();
        assert_eq!(
            fork_model_override(&m, &a, "deepseek-chat"),
            (acp::ModelId::new("deepseek-chat"), Some(acp::ModelId::new("solo")))
        );
    }

    /// 给的是重复 slug：留空。fork 不是用户在选模型的时刻，没人可问，
    /// 猜一个写进去就是把歧义洗成权威。
    #[test]
    fn duplicate_slug_override_writes_no_key() {
        let (m, a) = catalog();
        assert_eq!(
            fork_model_override(&m, &a, "glm-4.6"),
            (acp::ModelId::new("glm-4.6"), None)
        );
    }

    /// 目录里根本没有：字面值照旧带走（保持既有 fork 行为不硬失败），但不写 key。
    #[test]
    fn unknown_override_keeps_the_literal_but_writes_no_key() {
        let (m, a) = catalog();
        assert_eq!(
            fork_model_override(&m, &a, "never-configured"),
            (acp::ModelId::new("never-configured"), None)
        );
    }
}

/// v0.18.6 步8（RED）：Tauri 边界必须保留 `ambiguous_model_id` 结构化载荷。
///
/// agent_set_model 原来是 .map_err(|e| e.to_string())——引擎辛苦构造的候选
/// 列表在这一行全没了，前端只剩一句话，弹不出选择器，用户无从选起。
/// 整条链的最后一米把前面所有工作作废。
#[cfg(test)]
mod tauri_boundary_payload_tests {
    use agent_client_protocol as acp;
    use indexmap::IndexMap;
    use xai_grok_shell::agent::config::{ModelEntry, ModelInfo};
    use xai_grok_shell::agent::models::resolve_requested_model;

    use crate::engine_ops::ModelSwitchError;

    fn dup_catalog() -> (IndexMap<String, ModelEntry>, IndexMap<acp::ModelId, acp::ModelInfo>) {
        let mut models = IndexMap::new();
        for (key, base) in [
            ("glm-open", "https://open.bigmodel.cn/api/paas/v4"),
            ("glm-coding", "https://open.bigmodel.cn/api/coding/paas/v4"),
        ] {
            let mut info = ModelInfo::fallback("glm-4.6");
            info.base_url = base.to_owned();
            info.name = Some(format!("GLM ({key})"));
            models.insert(
                key.to_owned(),
                ModelEntry { info, api_key: None, env_key: None, api_base_url: None },
            );
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

    /// 歧义错误穿过边界后仍带候选，且序列化成前端能判别的形状。
    #[test]
    fn ambiguity_survives_the_boundary_with_its_candidates() {
        let (m, a) = dup_catalog();
        let acp_err = resolve_requested_model(&m, &a, "glm-4.6").unwrap_err().into_acp_error();

        let mapped = ModelSwitchError::from_acp(&acp_err);
        let json = serde_json::to_value(&mapped).unwrap();

        assert_eq!(json["kind"], "ambiguous_model_id", "前端靠 kind 分支");
        assert_eq!(json["requested"], "glm-4.6");
        let cands = json["candidates"].as_array().expect("必须是数组");
        assert_eq!(cands.len(), 2);
        assert_eq!(cands[0]["id"], "glm-open");
        assert_eq!(cands[0]["endpointLabel"], "open.bigmodel.cn");
        assert_eq!(cands[0]["selectable"], true);
    }

    /// 普通错误退化成消息，不伪装成歧义——否则前端会对着空候选弹选择器。
    #[test]
    fn ordinary_errors_degrade_to_a_message() {
        let err = acp::Error::internal_error().data("session actor closed");
        let json = serde_json::to_value(ModelSwitchError::from_acp(&err)).unwrap();
        assert_eq!(json["kind"], "error");
        assert!(json["message"].as_str().unwrap().contains("session actor closed"));
        assert!(json.get("candidates").is_none());
    }

    /// 未知模型走普通错误分支——它不是歧义。
    #[test]
    fn unknown_model_is_not_dressed_up_as_ambiguity() {
        let (m, a) = dup_catalog();
        let acp_err = resolve_requested_model(&m, &a, "nope").unwrap_err().into_acp_error();
        let json = serde_json::to_value(ModelSwitchError::from_acp(&acp_err)).unwrap();
        assert_eq!(json["kind"], "error");
    }
}
