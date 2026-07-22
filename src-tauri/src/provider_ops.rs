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

    // 先测连接（用第一个模型），失败就不落任何配置——半配置状态最坑小白。
    let first_model = models[0].2;
    let test = model_test(
        base_url.to_string(),
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
    let path = user_config_path();
    let original = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut =
        original.parse().map_err(|e| format!("配置解析失败（原文件未动）: {e}"))?;
    apply_provider_preset(&mut doc, &models, base_url);
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
