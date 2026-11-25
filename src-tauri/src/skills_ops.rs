//! v0.18-4 步 B：Skills（SKILL.md 读写 + 引擎 skills/*）与 Hooks 配置。
use std::path::PathBuf;

use serde::Serialize;
use tauri::State;

use crate::agent::{ext_call, AgentState};

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

#[derive(Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
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
    let path = hooks_path();
    match load_hooks_document(&path)? {
        Some(doc) => Ok(list_hooks(&doc)),
        None => Ok(vec![]),
    }
}

/// Read the hooks document on disk. A missing or empty file is `None`.
/// Anything else that goes wrong is an error, never "treated as empty":
/// treating an unreadable file as empty and then saving would overwrite it.
fn load_hooks_document(path: &std::path::Path) -> Result<Option<serde_json::Value>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("读取 hooks 文件失败: {e}")),
    };
    if text.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| format!("hooks 文件不是合法 JSON，未改动: {e}"))
}

/// Flatten a hooks document into the list the settings page shows: every hook
/// that carries a `command`, across every group of every event.
pub(crate) fn list_hooks(doc: &serde_json::Value) -> Vec<HookEntry> {
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
    out
}

/// Apply the settings page's flat list to the hooks document on disk and
/// return the document to write back.
///
/// The page only knows `{event, command}`, but an engine hook can also carry
/// `timeout` and `env`, and each group a `matcher`. Saving used to rebuild the
/// whole file from the list, so any field the form does not model was lost the
/// moment the user added or deleted a single hook from the page. Rules now:
///
/// - a hook the page can list (it has a `command`) survives if the list still
///   names it, with every field it had; otherwise it is removed;
/// - a hook the page cannot list (for example an `http` hook, which only has a
///   `url`) is never touched;
/// - a listed entry that is not on disk yet is appended as one
///   `{type: command, command}` hook;
/// - a group emptied of hooks is dropped, and so is an event emptied of groups;
/// - every other top-level key of the document is kept.
///
/// A structure any of this fails to recognise is an error and nothing is
/// written: a refused save costs the user a retry, an overwrite on a guess
/// costs them their file.
pub(crate) fn merge_hooks_document(
    existing: Option<&serde_json::Value>,
    entries: &[HookEntry],
) -> Result<serde_json::Value, String> {
    use std::collections::BTreeMap;
    const UNRECOGNISED: &str = "hooks 文件结构无法识别，未改动（应为 {\"hooks\": {事件: [{hooks: [...]}]}}）";

    // Each listed entry keeps at most one same-event, same-command hook on
    // disk; repeated rows each keep their own.
    let mut wanted: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for e in entries {
        if e.event.trim().is_empty() || e.command.trim().is_empty() {
            continue;
        }
        *wanted.entry((e.event.as_str(), e.command.as_str())).or_default() += 1;
    }
    let mut remaining = wanted;

    let mut root = match existing {
        Some(value) => value.as_object().cloned().ok_or_else(|| UNRECOGNISED.to_string())?,
        None => serde_json::Map::new(),
    };
    let mut merged: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    if let Some(events) = root.get("hooks") {
        let events = events.as_object().ok_or_else(|| UNRECOGNISED.to_string())?;
        for (event, groups) in events {
            let groups = groups.as_array().ok_or_else(|| UNRECOGNISED.to_string())?;
            let mut kept_groups = Vec::new();
            for group in groups {
                let group_obj = group.as_object().ok_or_else(|| UNRECOGNISED.to_string())?;
                let hooks = group_obj
                    .get("hooks")
                    .and_then(|v| v.as_array())
                    .ok_or_else(|| UNRECOGNISED.to_string())?;
                let mut kept = Vec::new();
                for hook in hooks {
                    let command = hook.get("command").and_then(|v| v.as_str());
                    match command.and_then(|c| remaining.get_mut(&(event.as_str(), c))) {
                        // Still listed: keep the hook exactly as it is on disk.
                        Some(left) if *left > 0 => {
                            *left -= 1;
                            kept.push(hook.clone());
                        }
                        // Listed before, deleted from the page since: drop it.
                        Some(_) => {}
                        // Not in the list at all — either the page deleted it,
                        // or the page never could list it (no `command`).
                        None if command.is_none() => kept.push(hook.clone()),
                        None => {}
                    }
                }
                if kept.is_empty() {
                    continue;
                }
                let mut group = group_obj.clone();
                group.insert("hooks".into(), serde_json::Value::Array(kept));
                kept_groups.push(serde_json::Value::Object(group));
            }
            if !kept_groups.is_empty() {
                merged.insert(event.clone(), serde_json::Value::Array(kept_groups));
            }
        }
    }

    // New entries share one group per event, appended after the existing ones.
    let mut added: BTreeMap<&str, Vec<serde_json::Value>> = BTreeMap::new();
    for ((event, command), left) in remaining {
        for _ in 0..left {
            added
                .entry(event)
                .or_default()
                .push(serde_json::json!({ "type": "command", "command": command }));
        }
    }
    for (event, commands) in added {
        let group = serde_json::json!({ "hooks": commands });
        match merged.get_mut(event) {
            Some(serde_json::Value::Array(groups)) => groups.push(group),
            _ => {
                merged.insert(event.to_string(), serde_json::Value::Array(vec![group]));
            }
        }
    }

    root.insert("hooks".into(), serde_json::Value::Object(merged));
    Ok(serde_json::Value::Object(root))
}

/// Write the settings page's flat list back to the WanCode-managed hooks file.
#[tauri::command]
pub async fn hooks_save(entries: Vec<HookEntry>) -> Result<(), String> {
    let path = hooks_path();
    let existing = load_hooks_document(&path)?;
    let doc = merge_hooks_document(existing.as_ref(), &entries)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    crate::config_core::write_config_atomic(&path, &serde_json::to_string_pretty(&doc).unwrap())
}

#[cfg(test)]
mod hooks_tests {
    use super::*;

    fn entry(event: &str, command: &str) -> HookEntry {
        HookEntry { event: event.into(), command: command.into() }
    }

    /// One command hook that carries fields the settings page has no form for,
    /// plus a second listed hook, plus one http hook the page cannot list at all.
    const RICH: &str = r#"{
      "hooks": {
        "PreToolUse": [
          { "matcher": "Bash", "hooks": [
            { "type": "command", "command": "echo lint", "timeout": 30, "env": { "PATH": "/x" } },
            { "type": "command", "command": "echo gone" }
          ] }
        ],
        "Stop": [
          { "hooks": [ { "type": "http", "url": "https://example.test/hook", "timeout": 5 } ] }
        ]
      }
    }"#;

    fn rich() -> serde_json::Value {
        serde_json::from_str(RICH).unwrap()
    }

    fn merge(existing: Option<&serde_json::Value>, entries: &[HookEntry]) -> serde_json::Value {
        merge_hooks_document(existing, entries).unwrap()
    }

    fn hooks_under(doc: &serde_json::Value, event: &str) -> Vec<serde_json::Value> {
        doc["hooks"][event]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|g| g["hooks"].as_array().unwrap().iter().cloned())
            .collect()
    }

    #[test]
    fn keeping_a_hook_keeps_every_field_it_had() {
        let doc = merge(Some(&rich()), &[entry("PreToolUse", "echo lint")]);
        let kept = &doc["hooks"]["PreToolUse"][0];
        // 分组的 matcher 与钩子自己的 timeout / env 都要还在
        assert_eq!(kept["matcher"], "Bash");
        assert_eq!(kept["hooks"][0]["timeout"], 30);
        assert_eq!(kept["hooks"][0]["env"]["PATH"], "/x");
        assert_eq!(kept["hooks"][0]["type"], "command");
    }

    #[test]
    fn deleting_one_hook_leaves_its_siblings_and_their_group_intact() {
        let doc = merge(Some(&rich()), &[entry("PreToolUse", "echo gone")]);
        let group = &doc["hooks"]["PreToolUse"][0];
        assert_eq!(group["matcher"], "Bash");
        assert_eq!(group["hooks"].as_array().unwrap().len(), 1);
        assert_eq!(group["hooks"][0]["command"], "echo gone");
    }

    #[test]
    fn a_hook_the_page_cannot_list_survives_every_save() {
        let doc = merge(Some(&rich()), &[]);
        // 两条 command 钩子都从列表里删了，http 钩子界面从来列不出来，必须留下
        assert!(doc["hooks"].get("PreToolUse").is_none());
        assert_eq!(doc["hooks"]["Stop"][0]["hooks"][0]["url"], "https://example.test/hook");
        assert_eq!(doc["hooks"]["Stop"][0]["hooks"][0]["timeout"], 5);
    }

    #[test]
    fn emptying_a_group_drops_the_group_and_emptying_an_event_drops_the_event() {
        let existing = serde_json::json!({ "hooks": { "Stop": [
            { "matcher": "Write", "hooks": [ { "type": "command", "command": "a" } ] },
            { "hooks": [ { "type": "command", "command": "b" }, { "type": "command", "command": "c" } ] }
        ] } });
        let doc = merge(Some(&existing), &[entry("Stop", "c")]);
        let groups = doc["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(groups.len(), 1, "只剩一个分组: {doc}");
        assert!(groups[0].get("matcher").is_none());
        assert_eq!(list_hooks(&doc), vec![entry("Stop", "c")]);
    }

    #[test]
    fn adding_a_hook_appends_a_group_and_touches_nothing_else() {
        let before = rich();
        // The list names "echo lint" only, so "echo gone" goes with this save.
        // Everything else about the three events must stay exactly as it was.
        let doc = merge(
            Some(&before),
            &[entry("PreToolUse", "echo lint"), entry("PostToolUse", "echo new")],
        );
        assert_eq!(doc["hooks"]["PostToolUse"], serde_json::json!([{ "hooks": [
            { "type": "command", "command": "echo new" }
        ] }]));
        assert_eq!(doc["hooks"]["PreToolUse"], serde_json::json!([{ "matcher": "Bash", "hooks": [
            { "type": "command", "command": "echo lint", "timeout": 30, "env": { "PATH": "/x" } }
        ] }]));
        assert_eq!(doc["hooks"]["Stop"], before["hooks"]["Stop"]);
        assert_eq!(doc["hooks"].as_object().unwrap().len(), 3);
    }

    #[test]
    fn a_new_hook_joins_its_event_after_the_existing_groups() {
        let doc = merge(Some(&rich()), &[entry("PreToolUse", "echo lint"), entry("PreToolUse", "echo new")]);
        let groups = doc["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0]["hooks"][0]["command"], "echo lint");
        assert_eq!(groups[1], serde_json::json!({ "hooks": [ { "type": "command", "command": "echo new" } ] }));
    }

    #[test]
    fn an_empty_list_without_a_file_writes_an_empty_document() {
        assert_eq!(merge(None, &[]), serde_json::json!({ "hooks": {} }));
    }

    #[test]
    fn a_first_save_without_a_file_creates_the_listed_hooks() {
        let doc = merge(None, &[entry("Stop", "echo done"), entry("Stop", "echo two")]);
        assert_eq!(doc, serde_json::json!({ "hooks": { "Stop": [{ "hooks": [
            { "type": "command", "command": "echo done" },
            { "type": "command", "command": "echo two" }
        ] }] } }));
    }

    #[test]
    fn blank_entries_are_ignored() {
        let doc = merge(None, &[entry("  ", "x"), entry("Stop", "  ")]);
        assert_eq!(doc, serde_json::json!({ "hooks": {} }));
    }

    #[test]
    fn other_top_level_keys_are_preserved() {
        let mut existing = rich();
        existing["$schema"] = serde_json::json!("https://example.test/hooks.schema.json");
        let doc = merge(Some(&existing), &[]);
        assert_eq!(doc["$schema"], "https://example.test/hooks.schema.json");
    }

    #[test]
    fn an_unrecognised_structure_is_refused_instead_of_overwritten() {
        let good = rich();
        assert!(merge_hooks_document(Some(&good), &[]).is_ok(), "正向对照：认识的形状能过");
        for bad in [
            serde_json::json!({ "hooks": [] }),
            serde_json::json!({ "hooks": { "Stop": { "hooks": [] } } }),
            serde_json::json!({ "hooks": { "Stop": [ { "matcher": "x" } ] } }),
            serde_json::json!({ "hooks": { "Stop": [ [] ] } }),
            serde_json::json!([]),
        ] {
            let err = merge_hooks_document(Some(&bad), &[entry("Stop", "echo hi")]).expect_err(&format!("必须拒绝: {bad}"));
            assert!(err.contains("未改动"), "错误要点明文件没被改动: {err}");
        }
    }

    #[test]
    fn duplicate_rows_match_existing_hooks_one_to_one() {
        let existing = serde_json::json!({ "hooks": { "Stop": [
            { "hooks": [ { "type": "command", "command": "echo same", "timeout": 9 } ] },
            { "hooks": [ { "type": "command", "command": "echo same" } ] }
        ] } });
        let one = merge(Some(&existing), &[entry("Stop", "echo same")]);
        let kept = hooks_under(&one, "Stop");
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0]["timeout"], 9, "留下的是先匹配到的那条，字段原样");
        let two = merge(Some(&existing), &[entry("Stop", "echo same"), entry("Stop", "echo same")]);
        assert_eq!(hooks_under(&two, "Stop").len(), 2);
    }

    #[test]
    fn what_the_page_lists_is_exactly_what_the_next_save_starts_from() {
        // 读出来 → 原样保存 → 再读出来：每一轮都必须稳定，否则每保存一次文件都会漂移。
        let doc = rich();
        for round in 0..3 {
            let listed = list_hooks(&doc);
            let next = merge(Some(&doc), &listed);
            assert_eq!(next, doc, "第 {round} 轮保存改动了文件");
            assert_eq!(list_hooks(&next), listed);
        }
        // 删掉一条再保存，文件与界面视图仍然一致
        let after_delete = merge(Some(&doc), &[entry("PreToolUse", "echo lint")]);
        assert_eq!(list_hooks(&after_delete), vec![entry("PreToolUse", "echo lint")]);
    }

    #[test]
    fn load_treats_a_missing_or_empty_file_as_absent_and_a_broken_one_as_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wancode.json");
        assert!(matches!(load_hooks_document(&path), Ok(None)));
        std::fs::write(&path, "  \n").unwrap();
        assert!(matches!(load_hooks_document(&path), Ok(None)));
        std::fs::write(&path, "{\"hooks\": [").unwrap();
        let err = load_hooks_document(&path).expect_err("坏 JSON 必须报错");
        assert!(err.contains("不是合法 JSON") && err.contains("未改动"), "{err}");
    }
}
