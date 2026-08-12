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

use std::path::Path;

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
    /// 现有清单归属另一工作区(codex R2-F1)。
    WorkspaceMismatch { requested: String, found: String },
    Io(String),
}

impl std::fmt::Display for WorkImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkImportError::UnsupportedKind(k) => write!(f, "不支持的文档类型: {k}(仅 pdf/docx)"),
            WorkImportError::SourceUnreadable(s) => write!(f, "源文件不可读: {s}"),
            WorkImportError::Staging(e) => write!(f, "暂存失败: {e}"),
            WorkImportError::WorkspaceMismatch { requested, found } => write!(
                f,
                "工作区身份不符: 请求 {requested},清单声称 {found}"
            ),
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
/// 事务性(codex R1):整个 stage + 清单 read-modify-write 收进**每工作区
/// 排他锁**内,使并发导入串行化(不丢记录);锁内任一步失败则**清理新建
/// 的导入目录**并保留原始错误(不留只读孤儿)。返回新建的 ImportRecord。
pub fn import_document(
    app_data_dir: &Path,
    workspace_id: &WorkspaceId,
    source: &Path,
) -> Result<ImportRecord, WorkImportError> {
    let kind = kind_from_extension(source)?;

    // 读原件 + 算完整 sha256(纯读,无落盘)。
    let bytes = std::fs::read(source)
        .map_err(|e| WorkImportError::SourceUnreadable(format!("{}: {e}", source.display())))?;
    let sha256 = hex_lower(&Sha256::digest(&bytes));

    let ws_dir = workspace_dir_under(app_data_dir.to_path_buf(), workspace_id);
    std::fs::create_dir_all(&ws_dir).map_err(|e| WorkImportError::Io(e.to_string()))?;

    // 每工作区排他锁:持锁期间的整段事务对并发导入串行。
    let _lock = acquire_workspace_lock(&ws_dir)?;

    let import_id = ImportId::mint();
    let import_dir = ws_dir.join(import_id.as_str());
    let manifest_path = manifest_path_under(app_data_dir.to_path_buf(), workspace_id);

    // 事务体:任一步 Err 都清理 import_dir 并回传原错。
    let txn = || -> Result<ImportRecord, WorkImportError> {
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
        let record = ImportRecord {
            import_id: import_id.clone(),
            source_sha256: sha256.clone(),
            display_name,
            staging_rel_path: format!("{}/{}", import_id.as_str(), file_name),
            kind: kind.to_string(),
        };

        let mut manifest = if manifest_path.exists() {
            let m = WorkManifest::read(&manifest_path)?; // 损坏/未来版本在此 fail-closed
            // codex R2-F1:现有清单的 workspace_id 必须**精确等于**请求的 id。
            // 否则(拷贝/篡改/恢复错位)会把导入错误归属到别的工作区。
            if &m.workspace_id != workspace_id {
                return Err(WorkImportError::WorkspaceMismatch {
                    requested: workspace_id.as_str().to_string(),
                    found: m.workspace_id.as_str().to_string(),
                });
            }
            m
        } else {
            WorkManifest::new(workspace_id.clone())
        };
        manifest.imports.push(record.clone());
        manifest.write_atomic(&manifest_path)?;
        Ok(record)
    };

    match txn() {
        Ok(rec) => Ok(rec),
        Err(e) => {
            // 清理新建的导入目录(先清只读,再删),保留原始错误。
            cleanup_import_dir(&import_dir);
            Err(e)
        }
    }
}

/// 每工作区排他锁:`<ws_dir>/.import.lock`,Windows 独占共享模式;
/// 被占则短暂重试(阻塞式串行,而非立即报错),最多约 5 秒。
fn acquire_workspace_lock(ws_dir: &Path) -> Result<std::fs::File, WorkImportError> {
    let path = ws_dir.join(".import.lock");
    for _ in 0..250 {
        let mut opts = std::fs::OpenOptions::new();
        opts.read(true).write(true).create(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            opts.share_mode(0); // 独占;他人 open → ERROR_SHARING_VIOLATION(32)
        }
        match opts.open(&path) {
            Ok(f) => return Ok(f),
            Err(e) if e.raw_os_error() == Some(32) => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => return Err(WorkImportError::Io(format!("获取工作区锁失败: {e}"))),
        }
    }
    Err(WorkImportError::Io("工作区锁长时间被占,放弃".into()))
}

/// 清理导入目录:递归清只读后删除。best-effort(错误不覆盖原始失败)。
fn cleanup_import_dir(import_dir: &Path) {
    clear_readonly_recursive(import_dir);
    let _ = std::fs::remove_dir_all(import_dir);
}

#[allow(clippy::permissions_set_readonly_false)] // 清理孤儿时有意清只读
fn clear_readonly_recursive(dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                clear_readonly_recursive(&p);
            } else if let Ok(meta) = std::fs::metadata(&p) {
                let mut perms = meta.permissions();
                perms.set_readonly(false);
                let _ = std::fs::set_permissions(&p, perms);
            }
        }
    }
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
    use std::path::PathBuf;

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
    fn concurrent_imports_both_persist_no_lost_record() {
        // codex R1-F1:两个线程从同一起点并发导入,锁串行化后**两条记录都在**。
        use std::sync::{Arc, Barrier};
        let app = Arc::new(tmp_dir("app"));
        let src_dir = Arc::new(tmp_dir("src"));
        let ws = Arc::new(WorkspaceId::mint());
        // 预建空清单,制造"两者读到同一起点"的条件。
        WorkManifest::new((*ws).clone())
            .write_atomic(&manifest_path_under((*app).clone(), &ws))
            .unwrap();

        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = ["c1.pdf", "c2.docx"]
            .into_iter()
            .map(|name| {
                let (app, src_dir, ws, barrier) =
                    (app.clone(), src_dir.clone(), ws.clone(), barrier.clone());
                let src = write_source(&src_dir, name, name.as_bytes());
                std::thread::spawn(move || {
                    barrier.wait(); // 对齐起跑,最大化重叠
                    import_document(&app, &ws, &src).unwrap()
                })
            })
            .collect();
        let recs: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        let m = WorkManifest::read(&manifest_path_under((*app).clone(), &ws)).unwrap();
        assert_eq!(m.imports.len(), 2, "并发两次导入都必须落盘,零丢失");
        for r in &recs {
            assert!(m.imports.contains(r), "返回的记录必须在持久清单里: {:?}", r.import_id);
        }
        let _ = std::fs::remove_dir_all(&*app);
        let _ = std::fs::remove_dir_all(&*src_dir);
    }

    #[test]
    fn manifest_failure_after_staging_leaves_no_orphan() {
        // codex R1-F2:staging 成功后清单读取失败(预置损坏清单),必须
        // 清理新建的导入目录,且不改动清单。
        let app = tmp_dir("app");
        let src_dir = tmp_dir("src");
        let ws = WorkspaceId::mint();
        // 预置**损坏**清单 → 事务里 read 会 Err(在 staging 之后)。
        let mp = manifest_path_under(app.clone(), &ws);
        std::fs::create_dir_all(mp.parent().unwrap()).unwrap();
        std::fs::write(&mp, b"{ this is not json").unwrap();

        let before = std::fs::read(&mp).unwrap();
        let src = write_source(&src_dir, "x.pdf", b"data");
        let err = import_document(&app, &ws, &src).unwrap_err();
        assert!(matches!(err, WorkImportError::Staging(_)), "应报暂存层错误,实得 {err:?}");

        // 清单未被改动
        assert_eq!(std::fs::read(&mp).unwrap(), before, "失败不得改动清单");
        // 无孤儿导入目录:工作区下除 manifest.json / .import.lock 外无 imp-* 目录
        let ws_dir = workspace_dir_under(app.clone(), &ws);
        let orphans: Vec<_> = std::fs::read_dir(&ws_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("imp-"))
            .collect();
        assert!(orphans.is_empty(), "staging 后失败必须清理导入目录,零孤儿");

        // 清掉锁文件后可删(锁文件非只读)
        let _ = std::fs::remove_dir_all(&app);
        let _ = std::fs::remove_dir_all(&src_dir);
    }

    #[test]
    fn foreign_workspace_manifest_is_rejected_no_orphan() {
        // codex R2-F1:工作区 A 目录下放着工作区 B 的清单,导入 A 必须拒绝,
        // 清单字节不变,无 imp-* 孤儿。
        let app = tmp_dir("app");
        let src_dir = tmp_dir("src");
        let ws_a = WorkspaceId::mint();
        let ws_b = WorkspaceId::mint();
        // 在 A 的目录下写入 B 的(语法合法的)清单。
        let mp_a = manifest_path_under(app.clone(), &ws_a);
        std::fs::create_dir_all(mp_a.parent().unwrap()).unwrap();
        WorkManifest::new(ws_b.clone()).write_atomic(&mp_a).unwrap();
        let before = std::fs::read(&mp_a).unwrap();

        let src = write_source(&src_dir, "x.pdf", b"data");
        let err = import_document(&app, &ws_a, &src).unwrap_err();
        assert!(
            matches!(err, WorkImportError::WorkspaceMismatch { .. }),
            "应报工作区身份不符,实得 {err:?}"
        );
        assert_eq!(std::fs::read(&mp_a).unwrap(), before, "拒绝不得改动清单");
        let ws_dir = workspace_dir_under(app.clone(), &ws_a);
        let orphans: Vec<_> = std::fs::read_dir(&ws_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("imp-"))
            .collect();
        assert!(orphans.is_empty(), "身份不符拒绝后零 imp-* 孤儿");
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
