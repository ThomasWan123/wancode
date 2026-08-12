//! Work 文档导入(v0.20 W2-b,设计稿 §1.4)。
//!
//! 把一份不受信文档导入某 Work 工作区的暂存区:
//!   1. 读原件、算完整 sha256(与 import_id 联合定位,设计 §1.2);
//!   2. 按扩展名判类型(仅 pdf/docx,其余拒绝);
//!   3. 铸造 import_id,建 `work/<ws>/<import_id>/`,原件复制进去;
//!   4. 暂存副本置**只读**(设计底线:原件全程只读);
//!   5. 原子更新工作区清单(复用 W2-a 的 write_atomic);
//!   6. 返回 ImportRecord。
//!
//! 安全不变量:暂存路径由 workspace_id/import_id(严格校验的新类型)拼出,
//! 不含调用方可控的路径段 —— 原始文件名只作 display_name,绝不入路径
//! (防路径穿越,与 W2-a 的 id 逃逸防线同源)。**不含**解析/锚点(W3)、
//! 前端(W2-c)。

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::work_staging::{
    manifest_path_under, workspace_dir_under, ImportId, ImportRecord, WorkManifest,
    WorkStagingError, WorkspaceId,
};

/// 导入结果错误(含底层暂存错误)。
#[derive(Debug)]
pub enum WorkImportError {
    /// 不支持的文档类型(仅 pdf/docx)。
    UnsupportedKind(String),
    /// 源文件不存在或不可读。
    SourceUnreadable(String),
    /// 暂存/清单层错误。
    Staging(WorkStagingError),
    Io(String),
}

impl std::fmt::Display for WorkImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkImportError::UnsupportedKind(k) => write!(f, "不支持的文档类型: {k}(仅 pdf/docx)"),
            WorkImportError::SourceUnreadable(s) => write!(f, "源文件不可读: {s}"),
            WorkImportError::Staging(e) => write!(f, "暂存失败: {e}"),
            WorkImportError::Io(s) => write!(f, "IO 失败: {s}"),
        }
    }
}
impl std::error::Error for WorkImportError {}
impl From<WorkStagingError> for WorkImportError {
    fn from(e: WorkStagingError) -> Self {
        WorkImportError::Staging(e)
    }
}

/// 由扩展名判文档类型。仅 pdf/docx;大小写不敏感。
fn kind_from_extension(source: &Path) -> Result<&'static str, WorkImportError> {
    let ext = source
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "pdf" => Ok("pdf"),
        "docx" => Ok("docx"),
        other => Err(WorkImportError::UnsupportedKind(other.to_string())),
    }
}

/// 暂存副本文件名:固定 `original.<ext>`,不含原始文件名(防路径穿越)。
fn staged_file_name(kind: &str) -> String {
    format!("original.{kind}")
}

/// 核心导入逻辑(可测,不依赖 Tauri)。app_data_dir 由调用方给出。
///
/// 返回新建的 ImportRecord;清单已原子更新到盘。
pub fn import_document(
    app_data_dir: &Path,
    workspace_id: &WorkspaceId,
    source: &Path,
) -> Result<ImportRecord, WorkImportError> {
    let kind = kind_from_extension(source)?;

    // 读原件 + 算完整 sha256。
    let bytes = std::fs::read(source)
        .map_err(|e| WorkImportError::SourceUnreadable(format!("{}: {e}", source.display())))?;
    let sha256 = hex_lower(&Sha256::digest(&bytes));

    let import_id = ImportId::mint();
    let ws_dir = workspace_dir_under(app_data_dir.to_path_buf(), workspace_id);
    let import_dir = ws_dir.join(import_id.as_str());
    std::fs::create_dir_all(&import_dir).map_err(|e| WorkImportError::Io(e.to_string()))?;

    let file_name = staged_file_name(kind);
    let staged_path = import_dir.join(&file_name);
    std::fs::write(&staged_path, &bytes).map_err(|e| WorkImportError::Io(e.to_string()))?;
    set_read_only(&staged_path).map_err(|e| WorkImportError::Io(e.to_string()))?;

    let display_name = source
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("document")
        .to_string();
    let staging_rel_path = format!("{}/{}", import_id.as_str(), file_name);

    let record = ImportRecord {
        import_id,
        source_sha256: sha256,
        display_name,
        staging_rel_path,
        kind: kind.to_string(),
    };

    // 读现有清单(不存在则新建),追加记录,原子写回。
    let manifest_path = manifest_path_under(app_data_dir.to_path_buf(), workspace_id);
    let mut manifest = if manifest_path.exists() {
        WorkManifest::read(&manifest_path)?
    } else {
        WorkManifest::new(workspace_id.clone())
    };
    manifest.imports.push(record.clone());
    manifest.write_atomic(&manifest_path)?;

    Ok(record)
}

/// Tauri 命令:把一份文档导入指定 Work 工作区。
///
/// `workspace_id` 从前端回传,经 [`WorkspaceId::parse`] 严格校验(拒绝路径逃逸)。
/// app_data_dir 由 AppHandle 解析,与 W2-a 的路径根同源。返回 JSON 化的
/// ImportRecord;任何失败返回结构化错误字符串(fail-closed,前端可读)。
#[tauri::command]
pub fn work_import(
    app: tauri::AppHandle,
    workspace_id: String,
    source_path: String,
) -> Result<ImportRecord, String> {
    use tauri::Manager;
    let ws = WorkspaceId::parse(workspace_id).map_err(|e| e.to_string())?;
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("解析 app_data_dir 失败: {e}"))?;
    import_document(&app_data, &ws, Path::new(&source_path)).map_err(|e| e.to_string())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// 置只读。原件全程只读是 Work 层底线(设计 §1.4)。
fn set_read_only(path: &Path) -> std::io::Result<()> {
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(path, perms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "w2b-{}-{}-{}",
            tag,
            std::process::id(),
            crate::work_staging::WorkspaceId::mint().as_str()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_source(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn imports_pdf_records_hash_and_sets_readonly() {
        let app = tmp_dir("app");
        let src_dir = tmp_dir("src");
        let content = b"%PDF-1.7 fake body";
        let src = write_source(&src_dir, "report.pdf", content);
        let ws = WorkspaceId::mint();

        let rec = import_document(&app, &ws, &src).unwrap();
        assert_eq!(rec.kind, "pdf");
        assert_eq!(rec.display_name, "report.pdf");
        // sha256 正确
        assert_eq!(rec.source_sha256, hex_lower(&Sha256::digest(content)));
        // 暂存副本存在且只读
        let staged = workspace_dir_under(app.clone(), &ws)
            .join(rec.import_id.as_str())
            .join("original.pdf");
        assert!(staged.exists());
        assert!(std::fs::metadata(&staged).unwrap().permissions().readonly());
        // 清单含该记录
        let mp = manifest_path_under(app.clone(), &ws);
        let m = WorkManifest::read(&mp).unwrap();
        assert_eq!(m.imports.len(), 1);
        assert_eq!(m.imports[0], rec);

        let _ = std::fs::remove_dir_all(&app);
        let _ = std::fs::remove_dir_all(&src_dir);
    }

    #[test]
    fn rejects_unsupported_kind() {
        let app = tmp_dir("app");
        let src_dir = tmp_dir("src");
        let src = write_source(&src_dir, "note.txt", b"hi");
        let ws = WorkspaceId::mint();
        assert!(matches!(
            import_document(&app, &ws, &src),
            Err(WorkImportError::UnsupportedKind(_))
        ));
        // 拒绝时不建暂存目录、不写清单
        assert!(!manifest_path_under(app.clone(), &ws).exists());
        let _ = std::fs::remove_dir_all(&app);
        let _ = std::fs::remove_dir_all(&src_dir);
    }

    #[test]
    fn two_imports_into_one_workspace_both_listed() {
        let app = tmp_dir("app");
        let src_dir = tmp_dir("src");
        let ws = WorkspaceId::mint();
        let a = import_document(&app, &ws, &write_source(&src_dir, "a.pdf", b"AAAA")).unwrap();
        let b = import_document(&app, &ws, &write_source(&src_dir, "b.docx", b"BBBB")).unwrap();
        assert_ne!(a.import_id, b.import_id);
        let m = WorkManifest::read(&manifest_path_under(app.clone(), &ws)).unwrap();
        assert_eq!(m.imports.len(), 2, "同一工作区两次导入都在清单里");
        let _ = std::fs::remove_dir_all(&app);
        let _ = std::fs::remove_dir_all(&src_dir);
    }

    #[test]
    fn staged_path_ignores_hostile_source_filename() {
        // 恶意源文件名不得进暂存路径(路径穿越防线)。暂存文件名恒为 original.<ext>。
        let app = tmp_dir("app");
        let src_dir = tmp_dir("src");
        // 源文件名本身合法(OS 不允许 / 在名字里),用带点的名字模拟:
        let src = write_source(&src_dir, "..evil.pdf", b"x");
        let ws = WorkspaceId::mint();
        let rec = import_document(&app, &ws, &src).unwrap();
        // 暂存相对路径只含 import_id + original.pdf,不含源文件名
        assert!(rec.staging_rel_path.ends_with("/original.pdf"));
        assert!(rec.staging_rel_path.starts_with(rec.import_id.as_str()));
        assert!(!rec.staging_rel_path.contains("evil"));
        let _ = std::fs::remove_dir_all(&app);
        let _ = std::fs::remove_dir_all(&src_dir);
    }

    #[test]
    fn missing_source_is_reported() {
        let app = tmp_dir("app");
        let ws = WorkspaceId::mint();
        let missing = app.join("nope.pdf");
        assert!(matches!(
            import_document(&app, &ws, &missing),
            Err(WorkImportError::SourceUnreadable(_))
        ));
        let _ = std::fs::remove_dir_all(&app);
    }
}
