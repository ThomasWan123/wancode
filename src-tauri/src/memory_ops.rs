//! C3（v0.20）：项目记忆的设置面与文件层读写。
//!
//! 引擎方法 `memory/flush` / `memory/rewrite` 早已存在（见 engine_ops），
//! 本模块补上设置面需要的其余半边：记忆开关（`[memory].enabled`，引擎在
//! **会话启动时**解析——改动只对新会话生效）与 MEMORY.md 的读写。
//!
//! 文件层全部落在客户端（G26 零引擎改动）：
//! - 全局记忆 `~/.grok/memory/MEMORY.md`——路径确定，直接读写；
//! - 工作区记忆 `~/.grok/memory/{slug}-{hash8}/MEMORY.md`——直接复用引擎
//!   公开的 `MemoryStorage` 身份算法（git origin 优先、路径兜底），
//!   不做 basename 前缀猜测，避免同名仓库之间泄露记忆。

use std::io::Write;
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
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(format!("读取配置失败: {error}")),
    };
    let doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("配置解析失败: {e}"))?;
    Ok(read_memory_enabled(&doc))
}

/// 开关记忆。**只对新会话生效**（引擎在会话启动时解析配置）——前端必须
/// 把这个语义显示出来，否则用户会以为当前会话立即获得记忆。
#[tauri::command]
pub fn memory_config_set(enabled: bool) -> Result<(), String> {
    let path = crate::config_core::user_config_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("读取配置失败: {error}")),
    };
    let mut doc: toml_edit::DocumentMut =
        text.parse().map_err(|e| format!("配置解析失败: {e}"))?;
    write_memory_enabled(&mut doc, enabled);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败: {e}"))?;
    }
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
    append_memory_file(&root.join("MEMORY.md"), trimmed)
        .map_err(|e| format!("写入全局记忆失败: {e}"))
}

/// 单次 append 写入一整条记录；绝不把“读失败”当空文件后覆盖旧内容。
fn append_memory_file(path: &Path, trimmed: &str) -> std::io::Result<()> {
    let nonempty = match std::fs::metadata(path) {
        Ok(meta) => meta.len() > 0,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    let mut entry = String::new();
    if nonempty {
        entry.push_str("\n\n---\n\n");
    }
    entry.push_str(trimmed);
    entry.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(entry.as_bytes())?;
    file.sync_all()
}

#[derive(serde::Serialize)]
pub struct WorkspaceMemory {
    pub dir_name: String,
    pub content: String,
}

/// 与引擎完全相同的工作区身份解析；不自行复制 hash/remote 规则。
pub(crate) fn workspace_memory_dir(root: &Path, workspace: &Path) -> PathBuf {
    xai_grok_shell::session::memory::MemoryStorage::new(workspace, Some(root))
        .workspace_dir()
        .to_path_buf()
}

/// 读当前工作区的 MEMORY.md（best-effort：目录发现不了就返回 None，
/// 前端显示「尚无工作区记忆」，绝不猜写）。
#[tauri::command]
pub fn memory_read_workspace(
    _state: State<'_, AgentState>,
    workspace: String,
) -> Result<Option<WorkspaceMemory>, String> {
    let root = memory_root();
    let dir = workspace_memory_dir(&root, Path::new(&workspace));
    if !dir.is_dir() {
        return Ok(None);
    }
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
    fn workspace_dir_uses_full_identity_not_basename_prefix() {
        let root = std::env::temp_dir().join(format!("c3mem-{}", std::process::id()));
        let first = workspace_memory_dir(&root, Path::new("D:\\one\\same-name"));
        let second = workspace_memory_dir(&root, Path::new("D:\\two\\same-name"));
        assert_ne!(first, second, "同名但不同路径的非仓库不得共享记忆");
        assert!(first.starts_with(&root) && second.starts_with(&root));
    }

    #[test]
    fn append_global_creates_and_separates() {
        // 直接测文件层语义（不经 Tauri 命令边界——grok_home 在测试进程里
        // 指向真实用户目录，不能写）。这里用临时根复刻同一写入逻辑。
        let root = std::env::temp_dir().join(format!("c3app-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let p = root.join("MEMORY.md");
        append_memory_file(&p, "第一条").unwrap();
        append_memory_file(&p, "第二条").unwrap();
        let s = std::fs::read_to_string(&p).unwrap();
        assert_eq!(s.matches("---").count(), 1, "条目间恰好一个分隔");
        assert!(s.contains("第一条") && s.contains("第二条"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
