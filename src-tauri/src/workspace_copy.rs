//! Copy a user-chosen file into an opened folder (Work surface).
//!
//! Destination name is only the source basename — no caller-controlled path
//! segments — so this cannot escape the folder. Files already inside the
//! folder are left in place (no extra copy).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Copy `source` into `workspace`, returning the workspace-relative path
/// (forward slashes). No-ops when the file is already inside the folder.
pub fn copy_into_workspace_dir(workspace: &Path, source: &Path) -> Result<String, String> {
    if !workspace.is_dir() {
        return Err("工作区文件夹不存在".into());
    }
    if !source.is_file() {
        return Err("源文件不存在".into());
    }
    let name = source
        .file_name()
        .ok_or_else(|| "源文件名无效".to_string())?;
    if name == "." || name == ".." {
        return Err("源文件名无效".into());
    }

    let ws = workspace
        .canonicalize()
        .map_err(|e| format!("工作区路径无效: {e}"))?;

    if let Ok(src) = source.canonicalize() {
        if src.starts_with(&ws) {
            return src
                .strip_prefix(&ws)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .map_err(|_| "目标路径逃逸".to_string());
        }
    }

    let dest = unique_dest(&ws, name);
    if !dest.starts_with(&ws) {
        return Err("目标路径逃逸".into());
    }
    std::fs::copy(source, &dest).map_err(|e| format!("复制失败: {e}"))?;
    dest.strip_prefix(&ws)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .map_err(|_| "目标路径逃逸".into())
}

fn unique_dest(dir: &Path, name: &std::ffi::OsStr) -> PathBuf {
    let path = dir.join(name);
    if !path.exists() {
        return path;
    }
    let path_name = Path::new(name);
    let stem = path_name.file_stem().unwrap_or(name);
    let ext = path_name.extension();
    for i in 1..1000 {
        let mut candidate = OsString::from(stem);
        candidate.push(format!(" ({i})"));
        if let Some(ext) = ext {
            candidate.push(".");
            candidate.push(ext);
        }
        let p = dir.join(&candidate);
        if !p.exists() {
            return p;
        }
    }
    dir.join(name)
}

/// Place a file into the opened workspace folder. Returns the relative path.
#[tauri::command]
pub fn copy_into_workspace(workspace: String, source_path: String) -> Result<String, String> {
    copy_into_workspace_dir(Path::new(&workspace), Path::new(&source_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn copies_basename_into_the_folder() {
        let dir = tmp();
        let src_dir = tmp();
        let src = src_dir.path().join("brief.pdf");
        std::fs::write(&src, b"%PDF").unwrap();
        let rel = copy_into_workspace_dir(dir.path(), &src).unwrap();
        assert_eq!(rel, "brief.pdf");
        assert_eq!(std::fs::read(dir.path().join("brief.pdf")).unwrap(), b"%PDF");
    }

    #[test]
    fn copies_xlsx_as_an_ordinary_file() {
        let dir = tmp();
        let src_dir = tmp();
        let src = src_dir.path().join("budget.xlsx");
        std::fs::write(&src, b"PK").unwrap();
        let rel = copy_into_workspace_dir(dir.path(), &src).unwrap();
        assert_eq!(rel, "budget.xlsx");
        assert!(dir.path().join("budget.xlsx").is_file());
    }

    #[test]
    fn ignores_source_directory_components() {
        let dir = tmp();
        let nested = tmp();
        let src = nested.path().join("nested").join("notes.docx");
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, b"DOCX").unwrap();
        let rel = copy_into_workspace_dir(dir.path(), &src).unwrap();
        assert_eq!(rel, "notes.docx");
        assert!(dir.path().join("notes.docx").is_file());
        assert!(!dir.path().join("nested").exists());
    }

    #[test]
    fn already_in_folder_is_a_noop() {
        let dir = tmp();
        let src = dir.path().join("brief.pdf");
        std::fs::write(&src, b"%PDF").unwrap();
        let rel = copy_into_workspace_dir(dir.path(), &src).unwrap();
        assert_eq!(rel, "brief.pdf");
        assert_eq!(std::fs::read(&src).unwrap(), b"%PDF");
    }

    #[test]
    fn collision_gets_a_numeric_suffix() {
        let dir = tmp();
        std::fs::write(dir.path().join("brief.pdf"), b"old").unwrap();
        let src_dir = tmp();
        let src = src_dir.path().join("brief.pdf");
        std::fs::write(&src, b"new").unwrap();
        let rel = copy_into_workspace_dir(dir.path(), &src).unwrap();
        assert_eq!(rel, "brief (1).pdf");
        assert_eq!(std::fs::read(dir.path().join("brief.pdf")).unwrap(), b"old");
        assert_eq!(
            std::fs::read(dir.path().join("brief (1).pdf")).unwrap(),
            b"new"
        );
    }

    #[test]
    fn rejects_missing_folder_or_file() {
        let dir = tmp();
        assert!(copy_into_workspace_dir(&dir.path().join("missing"), Path::new("x")).is_err());
        assert!(copy_into_workspace_dir(dir.path(), &dir.path().join("nope.pdf")).is_err());
    }
}
