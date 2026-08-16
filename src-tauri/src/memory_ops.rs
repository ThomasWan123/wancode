//! C3（v0.20）：项目记忆的设置面与文件层读写。
//!
//! 引擎方法 `memory/flush` / `memory/rewrite` 早已存在（见 engine_ops），
//! 本模块补上设置面需要的其余半边：记忆开关（`[memory].enabled`，引擎在
//! **会话启动时**解析——改动只对新会话生效）与 MEMORY.md 的读写。
//!
//! 文件层全部落在客户端（G26 零引擎改动）：
//! - 全局记忆 `~/.grok/memory/MEMORY.md`——路径确定，直接读写；
//! - 工作区记忆 `~/.grok/memory/{slug}-{hash8}/MEMORY.md`——hash8 由引擎用
//!   blake3 算出，客户端不重复实现哈希，只做**按 slug 前缀的 best-effort
//!   发现**（唯一前缀命中才认；零/多命中都如实返回 None）。这只是展示
//!   便利，不是安全边界——猜错的最坏结果是「找不到」，绝不写错目录。

use std::path::{Path, PathBuf};

use tauri::State;

use crate::agent::AgentState;

/// 记忆根目录（`~/.grok/memory`）。
fn memory_root() -> PathBuf {
    xai_grok_shell::util::grok_home::grok_home().join("memory")
}

/// 读取 `[memory].enabled`；缺段/缺键/文件不存在一律 false（与引擎默认一致）。
pub(crate) fn read_memory_enabled(doc: &toml_edit::DocumentMut) -> bool {
    doc.get("memory")
        .and_then(|m| m.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// 在 doc 上就位 `[memory].enabled`（纯函数；事务由调用方提交）。
pub(crate) fn write_memory_enabled(doc: &mut toml_edit::DocumentMut, enabled: bool) {
    let tbl = doc["memory"]
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .expect("memory 段");
    tbl["enabled"] = toml_edit::value(enabled);
}

#[tauri::command]
pub fn memory_config_get() -> Result<bool, String> {
    let path = crate::config_core::user_config_path();
    let text = std::fs::read_to_string(&path).map_err(|e| format!("读取配置失败: {e}"))?;
    let doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("配置解析失败: {e}"))?;
    Ok(read_memory_enabled(&doc))
}

/// 开关记忆。**只对新会话生效**（引擎在会话启动时解析配置）——前端必须
/// 把这个语义显示出来，否则用户会以为当前会话立即获得记忆。
#[tauri::command]
pub fn memory_config_set(enabled: bool) -> Result<(), String> {
    let path = crate::config_core::user_config_path();
    let text = std::fs::read_to_string(&path).map_err(|e| format!("读取配置失败: {e}"))?;
    let mut doc: toml_edit::DocumentMut =
        text.parse().map_err(|e| format!("配置解析失败: {e}"))?;
    write_memory_enabled(&mut doc, enabled);
    crate::config_core::write_config_atomic(&path, &doc.to_string())
}

#[tauri::command]
pub fn memory_read_global() -> Result<String, String> {
    let p = memory_root().join("MEMORY.md");
    match std::fs::read_to_string(&p) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("读取全局记忆失败: {e}")),
    }
}

/// 追加到全局 MEMORY.md（目录按需创建；条目间用 `---` 分隔，与引擎的
/// session-log 分块约定一致——chunker 把 `---` 分段当作独立条目）。
#[tauri::command]
pub fn memory_append_global(text: String) -> Result<(), String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("内容为空，未写入".into());
    }
    let root = memory_root();
    std::fs::create_dir_all(&root).map_err(|e| format!("创建记忆目录失败: {e}"))?;
    let p = root.join("MEMORY.md");
    let mut body = std::fs::read_to_string(&p).unwrap_or_default();
    if !body.trim().is_empty() {
        body.push_str("\n\n---\n\n");
    }
    body.push_str(trimmed);
    body.push('\n');
    std::fs::write(&p, body).map_err(|e| format!("写入全局记忆失败: {e}"))
}

#[derive(serde::Serialize)]
pub struct WorkspaceMemory {
    pub dir_name: String,
    pub content: String,
}

/// 宽松 slugify：小写、非字母数字折成 `-`、折叠重复、40 字符上限。
/// 与引擎 slugify 的**常见形态**对齐（目录名/仓库名），用于前缀发现；
/// 不追求逐字一致——多命中时我们宁可不认。
pub(crate) fn slugify_loose(name: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in name.chars().flat_map(|c| c.to_lowercase()) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
        if out.len() >= 40 {
            break;
        }
    }
    out.trim_end_matches('-').to_string()
}

/// 按 `{slug}-` 前缀在记忆根里找唯一候选目录。零/多命中 → None。
pub(crate) fn find_workspace_memory_dir(root: &Path, workspace: &Path) -> Option<PathBuf> {
    let folder = workspace.file_name()?.to_string_lossy();
    let slug = slugify_loose(&folder);
    if slug.is_empty() {
        return None;
    }
    let prefix = format!("{slug}-");
    let mut hits: Vec<PathBuf> = std::fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().starts_with(prefix.as_str()))
                .unwrap_or(false)
        })
        .collect();
    if hits.len() == 1 { hits.pop() } else { None }
}

/// 读当前工作区的 MEMORY.md（best-effort：目录发现不了就返回 None，
/// 前端显示「尚无工作区记忆」，绝不猜写）。
#[tauri::command]
pub fn memory_read_workspace(
    _state: State<'_, AgentState>,
    workspace: String,
) -> Result<Option<WorkspaceMemory>, String> {
    let root = memory_root();
    let Some(dir) = find_workspace_memory_dir(&root, Path::new(&workspace)) else {
        return Ok(None);
    };
    let content = match std::fs::read_to_string(dir.join("MEMORY.md")) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("读取工作区记忆失败: {e}")),
    };
    let dir_name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(Some(WorkspaceMemory { dir_name, content }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_toggle_roundtrip() {
        let mut doc: toml_edit::DocumentMut = "[models]\ndefault = \"glm\"\n".parse().unwrap();
        assert!(!read_memory_enabled(&doc), "缺段默认 false");
        write_memory_enabled(&mut doc, true);
        assert!(read_memory_enabled(&doc));
        write_memory_enabled(&mut doc, false);
        assert!(!read_memory_enabled(&doc));
        // 不碰别的段
        assert!(doc.to_string().contains("default = \"glm\""));
    }

    #[test]
    fn slugify_common_forms() {
        assert_eq!(slugify_loose("wancode"), "wancode");
        assert_eq!(slugify_loose("My Project"), "my-project");
        assert_eq!(slugify_loose("WANCode_2.0"), "wancode-2-0");
        assert_eq!(slugify_loose("---"), "");
    }

    #[test]
    fn workspace_dir_discovery_requires_unique_prefix() {
        let root = std::env::temp_dir().join(format!("c3mem-{}", std::process::id()));
        std::fs::create_dir_all(root.join("wancode-a3f7b2c9")).unwrap();
        let ws = Path::new("D:\\code\\wancode");
        assert_eq!(
            find_workspace_memory_dir(&root, ws).unwrap().file_name().unwrap(),
            "wancode-a3f7b2c9"
        );
        // 多命中 → 不认（诚实优于猜测）
        std::fs::create_dir_all(root.join("wancode-bbbbbbbb")).unwrap();
        assert!(find_workspace_memory_dir(&root, ws).is_none());
        // 零命中 → None
        assert!(find_workspace_memory_dir(&root, Path::new("D:\\code\\nowhere")).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn append_global_creates_and_separates() {
        // 直接测文件层语义（不经 Tauri 命令边界——grok_home 在测试进程里
        // 指向真实用户目录，不能写）。这里用临时根复刻同一写入逻辑。
        let root = std::env::temp_dir().join(format!("c3app-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let p = root.join("MEMORY.md");
        let append = |text: &str| {
            let mut body = std::fs::read_to_string(&p).unwrap_or_default();
            if !body.trim().is_empty() {
                body.push_str("\n\n---\n\n");
            }
            body.push_str(text.trim());
            body.push('\n');
            std::fs::write(&p, body).unwrap();
        };
        append("第一条");
        append("第二条");
        let s = std::fs::read_to_string(&p).unwrap();
        assert_eq!(s.matches("---").count(), 1, "条目间恰好一个分隔");
        assert!(s.contains("第一条") && s.contains("第二条"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
