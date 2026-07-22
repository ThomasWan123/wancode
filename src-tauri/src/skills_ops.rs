//! v0.18-4 步 B：Skills（SKILL.md 读写 + 引擎 skills/*）与 Hooks 配置。
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::agent::{ext_call, ext_ok, AgentState};

/// Read a skill's SKILL.md for in-app editing.
///
/// Takes the ABSOLUTE path from the engine's skills/list — skills can live in
/// plugin dirs / project dirs, not just ~/.grok/skills, so deriving the path
/// from a name would silently miss those.
#[tauri::command]
pub async fn skill_read(path: String) -> Result<String, String> {
    let p = PathBuf::from(&path);
    let file = if p.is_dir() { p.join("SKILL.md") } else { p };
    std::fs::read_to_string(&file).map_err(|e| e.to_string())
}

/// Write a skill's SKILL.md content (absolute path, same rule as skill_read).
#[tauri::command]
pub async fn skill_write(path: String, content: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    let file = if p.is_dir() || !path.ends_with(".md") {
        std::fs::create_dir_all(&p).map_err(|e| e.to_string())?;
        p.join("SKILL.md")
    } else {
        p
    };
    std::fs::write(file, content).map_err(|e| e.to_string())
}




// ── Skills (~/.grok/skills/<name>/SKILL.md) ─────────────────────────

fn skills_dir() -> PathBuf {
    xai_grok_shell::util::grok_home::grok_home().join("skills")
}

/// List skills via the engine (x.ai/skills/list). Replaces the old
/// filesystem scan of ~/.grok/skills: the engine also discovers project-level
/// and plugin skills, and knows each skill's enabled state.
#[tauri::command]
pub async fn skills_list(
    state: State<'_, AgentState>,
    workspace: String,
) -> Result<serde_json::Value, String> {
    let v = ext_call(&state, "x.ai/skills/list", serde_json::json!({ "cwd": workspace })).await?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    Ok(v.get("result").cloned().unwrap_or(v))
}

/// Enable/disable a skill (x.ai/skills/toggle). Persists to [skills].disabled
/// in the engine config; returns the full refreshed list.
#[tauri::command]
pub async fn skills_toggle(
    state: State<'_, AgentState>,
    name: String,
    enabled: bool,
    workspace: String,
) -> Result<serde_json::Value, String> {
    let v = ext_call(
        &state,
        "x.ai/skills/toggle",
        serde_json::json!({ "name": name, "enabled": enabled, "cwd": workspace }),
    )
    .await?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    // toggle 只写配置。会话（含之后新建的）用的是 agent 启动时的技能基线
    // 快照——不刷新基线，停用就只是改了个没人读的配置项。实测踩过：停用
    // 后新会话的模型面向清单里技能还在。
    let _ = ext_call(&state, "x.ai/skills/refresh-baseline", serde_json::json!({})).await;
    Ok(v.get("result").cloned().unwrap_or(v))
}

/// Register an extra skills directory (x.ai/skills/add). `path` may be a dir
/// or a SKILL.md; `~` expands engine-side.
#[tauri::command]
pub async fn skills_add_path(
    state: State<'_, AgentState>,
    path: String,
    workspace: String,
) -> Result<serde_json::Value, String> {
    let v = ext_call(
        &state,
        "x.ai/skills/add",
        serde_json::json!({ "path": path, "cwd": workspace }),
    )
    .await?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    let _ = ext_call(&state, "x.ai/skills/refresh-baseline", serde_json::json!({})).await;
    Ok(v.get("result").cloned().unwrap_or(v))
}

/// Unregister a skills path (x.ai/skills/remove).
#[tauri::command]
pub async fn skills_remove_path(
    state: State<'_, AgentState>,
    path: String,
    workspace: String,
) -> Result<serde_json::Value, String> {
    let v = ext_call(
        &state,
        "x.ai/skills/remove",
        serde_json::json!({ "path": path, "cwd": workspace }),
    )
    .await?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    let _ = ext_call(&state, "x.ai/skills/refresh-baseline", serde_json::json!({})).await;
    Ok(v.get("result").cloned().unwrap_or(v))
}

/// Reset skills config to defaults (x.ai/skills/reset).
#[tauri::command]
pub async fn skills_reset(
    state: State<'_, AgentState>,
    workspace: String,
) -> Result<serde_json::Value, String> {
    let v = ext_call(&state, "x.ai/skills/reset", serde_json::json!({ "cwd": workspace })).await?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    let _ = ext_call(&state, "x.ai/skills/refresh-baseline", serde_json::json!({})).await;
    Ok(v.get("result").cloned().unwrap_or(v))
}

/// Skills config summary — paths / ignore / totals (x.ai/skills/config).
#[tauri::command]
pub async fn skills_config(
    state: State<'_, AgentState>,
    workspace: String,
) -> Result<serde_json::Value, String> {
    let v = ext_call(&state, "x.ai/skills/config", serde_json::json!({ "cwd": workspace })).await?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    Ok(v.get("result").cloned().unwrap_or(v))
}

/// Ensure ~/.grok/skills exists and open it in the OS file manager.
#[tauri::command]
pub async fn skills_open() -> Result<(), String> {
    let dir = skills_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    std::process::Command::new("explorer")
        .arg(&dir)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Create a starter skill: ~/.grok/skills/<name>/SKILL.md with a template.
#[tauri::command]
pub async fn skills_create(name: String, description: String) -> Result<String, String> {
    let safe: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    if safe.is_empty() {
        return Err("名称无效".into());
    }
    let dir = skills_dir().join(&safe);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let md = format!(
        "---\nname: {safe}\ndescription: {desc}\n---\n\n# {safe}\n\n{desc}\n\n## 使用说明\n\n在这里写这个 skill 的具体指令与步骤。\n",
        safe = safe,
        desc = if description.trim().is_empty() { "（填写这个 skill 的用途）" } else { description.trim() },
    );
    let path = dir.join("SKILL.md");
    std::fs::write(&path, md).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

// ── Hooks (~/.grok/hooks/wancode.json, WanCode-managed) ──────────────

#[derive(Serialize, serde::Deserialize, Clone)]
pub struct HookEntry {
    pub event: String,
    pub command: String,
}

fn hooks_path() -> PathBuf {
    xai_grok_shell::util::grok_home::grok_home()
        .join("hooks")
        .join("wancode.json")
}

/// Read the WanCode-managed hooks file as a flat {event, command} list.
#[tauri::command]
pub async fn hooks_list() -> Result<Vec<HookEntry>, String> {
    let text = std::fs::read_to_string(hooks_path()).unwrap_or_default();
    if text.trim().is_empty() {
        return Ok(vec![]);
    }
    let doc: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    if let Some(map) = doc.get("hooks").and_then(|v| v.as_object()) {
        for (event, groups) in map {
            for group in groups.as_array().into_iter().flatten() {
                for h in group.get("hooks").and_then(|v| v.as_array()).into_iter().flatten() {
                    if let Some(cmd) = h.get("command").and_then(|v| v.as_str()) {
                        out.push(HookEntry { event: event.clone(), command: cmd.to_string() });
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Replace the entire WanCode-managed hooks file from a flat list.
#[tauri::command]
pub async fn hooks_save(entries: Vec<HookEntry>) -> Result<(), String> {
    use std::collections::BTreeMap;
    let mut by_event: BTreeMap<String, Vec<serde_json::Value>> = BTreeMap::new();
    for e in entries {
        if e.event.trim().is_empty() || e.command.trim().is_empty() {
            continue;
        }
        by_event
            .entry(e.event)
            .or_default()
            .push(serde_json::json!({ "type": "command", "command": e.command }));
    }
    let hooks: serde_json::Map<String, serde_json::Value> = by_event
        .into_iter()
        .map(|(event, cmds)| (event, serde_json::json!([{ "hooks": cmds }])))
        .collect();
    let doc = serde_json::json!({ "hooks": hooks });
    let path = hooks_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&doc).unwrap())
        .map_err(|e| format!("写入 hooks 失败: {e}"))
}
