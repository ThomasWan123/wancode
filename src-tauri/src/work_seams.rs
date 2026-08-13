//! v0.20 W2.5：Work 身份事务的**跨 PR 接缝**测试（codex issue #47 ②③④⑦）。
//!
//! 放在 lib 单测而非 tests/：本文件引用 work_import(含 #[tauri::command]),
//! 在集成测试 harness 下会把 tauri 插件链进来并与 panic=abort 冲突;
//! 而 CI 的 `cargo test -p wancode --lib` 同样覆盖这里。真正需要外部
//! 引擎的那部分在 tests/work_surface_engine.rs。

#![cfg(test)]

// v0.20 W2.5：Work 身份事务的**跨 PR 接缝**验证（codex issue #47 required
// scope ②③④⑦）。
//
// 单测各自覆盖了 W2-a/b/c 的内部（id 校验、并发清单、schema 门……），但
// **接缝只有合起来才有意义**：铸造的 workspace_id 是否真落进持久 binding、
// 该 id 是否能驱动导入、恢复时 binding 是否压过对立意图、失败启动是否
// 不留身份残留。本文件只测这些接缝，不重做已覆盖的内部。
//
// 与 `work_surface_engine.rs` 分工：那条验证**引擎侧**会话构造（profile
// 能否 build、注册表能否解析）；本条验证 **wancode 自有层**的身份事务，
// 不需要引擎，因而快且确定。

use crate::surface::{SurfaceBinding, SurfaceBindingStore, SurfaceError, SurfaceKind};
use crate::work_import::{import_document, WorkImportError};
use crate::work_staging::{manifest_path_under, workspace_dir_under, WorkManifest, WorkspaceId};

const SESSION: &str = "w25-work-session";

/// ② 铸造的 workspace_id 必须真正落进**持久 binding**，读回后逐字相等。
/// 这是 W2-c 引入的事务边界：返回值对不算数，盘上对才算。
#[test]
fn minted_workspace_id_is_persisted_in_the_binding() {
    let tmp = tempfile::tempdir().unwrap();
    let store = SurfaceBindingStore::new(tmp.path().join("surface-bindings"));
    let ws = WorkspaceId::mint();

    store
        .write(&SurfaceBinding::new_work(SESSION, ws.clone()))
        .expect("Work binding 应可写入");

    // 读回持久化的那一份（不是我们手里的对象）。
    let back = store.resolve(SESSION).expect("Work binding 应可解析");
    assert_eq!(back.session_id, SESSION);
    assert_eq!(back.surface_kind, SurfaceKind::Work);
    assert_eq!(
        back.workspace_id.as_ref().map(|w| w.as_str()),
        Some(ws.as_str()),
        "持久 binding 必须携带铸造时的同一 workspace_id"
    );
}

/// ③ 由 binding 读回的 workspace_id 必须能驱动**生产导入路径**，且原件
/// 字节/权限不被改动、暂存副本只读、清单读回含且仅含该记录。
#[test]
fn workspace_id_from_binding_drives_import_without_touching_the_source() {
    let tmp = tempfile::tempdir().unwrap();
    let app_data = tmp.path().join("app-data");
    let store = SurfaceBindingStore::new(tmp.path().join("surface-bindings"));
    let ws = WorkspaceId::mint();
    store
        .write(&SurfaceBinding::new_work(SESSION, ws.clone()))
        .unwrap();

    // 原件放在**工作区之外**，模拟用户从任意位置选文件。
    let src_dir = tmp.path().join("user-docs");
    std::fs::create_dir_all(&src_dir).unwrap();
    let src = src_dir.join("报告.pdf");
    let content = b"%PDF-1.7 w2.5 seam fixture";
    std::fs::write(&src, content).unwrap();
    let src_ro_before = std::fs::metadata(&src).unwrap().permissions().readonly();

    // 用**从 binding 读回**的 id 导入（而不是手里那个），走生产函数。
    let ws_from_binding = store
        .resolve(SESSION)
        .unwrap()
        .workspace_id
        .expect("Work binding 必有 workspace_id");
    let rec = import_document(&app_data, &ws_from_binding, &src).expect("导入应成功");

    // 原件字节与权限未变（Work 底线：原件全程只读、不被搬走）。
    assert_eq!(std::fs::read(&src).unwrap(), content, "原件字节不得改动");
    assert_eq!(
        std::fs::metadata(&src).unwrap().permissions().readonly(),
        src_ro_before,
        "原件权限不得改动"
    );

    // 暂存副本：字节相同、只读。
    let staged = workspace_dir_under(app_data.clone(), &ws_from_binding)
        .join(rec.import_id.as_str())
        .join("original.pdf");
    assert_eq!(std::fs::read(&staged).unwrap(), content, "暂存副本字节必须与原件一致");
    assert!(
        std::fs::metadata(&staged).unwrap().permissions().readonly(),
        "暂存副本必须只读"
    );

    // 清单读回：在**该 workspace 名下**，且恰好这一条。
    let manifest = WorkManifest::read(&manifest_path_under(app_data, &ws_from_binding)).unwrap();
    assert_eq!(manifest.workspace_id, ws_from_binding);
    assert_eq!(manifest.imports, vec![rec], "清单必须含且仅含返回的那条记录");
}

/// ④ 反向控制（最可能抓到身份回归的一条）：恢复既有 Work 会话时，即便
/// 调用方带着**对立意图**（Code），持久 binding 必须权威——层不变、
/// workspace_id 不变、**不铸第二个工作区**。
#[test]
fn resume_with_opposing_intent_keeps_the_persisted_work_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let store = SurfaceBindingStore::new(tmp.path().join("surface-bindings"));
    let ws = WorkspaceId::mint();
    store
        .write(&SurfaceBinding::new_work(SESSION, ws.clone()))
        .unwrap();

    // 生产恢复路径读的就是 resolve()——它不接受任何"意图"参数,这正是
    // 设计要点：层身份只信 sidecar。
    let resumed = store.resolve(SESSION).unwrap();
    assert_eq!(resumed.surface_kind, SurfaceKind::Work);
    assert_eq!(resumed.workspace_id.as_ref(), Some(&ws), "恢复不得换工作区");

    // 显式带对立意图去写：必须被身份不可变门拒绝，而不是悄悄改写。
    let opposing = store.write(&SurfaceBinding::new(SESSION, SurfaceKind::Code));
    assert!(
        matches!(opposing, Err(SurfaceError::ImmutableKindConflict { .. })),
        "对立意图必须显式冲突,实得 {opposing:?}"
    );

    // 盘上身份未被污染，且没有第二个工作区被铸出来。
    let after = store.resolve(SESSION).unwrap();
    assert_eq!(after.surface_kind, SurfaceKind::Work);
    assert_eq!(after.workspace_id.as_ref(), Some(&ws), "冲突后身份必须原样");
}

/// ⑦ 失败清理：导入失败不得留下暂存残留，也不得改动清单——即"失败启动
/// 不留身份/资源残留"这条在导入侧的对应物。
#[test]
fn failed_import_leaves_no_identity_or_staging_residue() {
    let tmp = tempfile::tempdir().unwrap();
    let app_data = tmp.path().join("app-data");
    let ws = WorkspaceId::mint();

    // 先成功导入一份，建立"已知良好"的清单基线。
    let src_dir = tmp.path().join("user-docs");
    std::fs::create_dir_all(&src_dir).unwrap();
    let good = src_dir.join("good.pdf");
    std::fs::write(&good, b"good").unwrap();
    import_document(&app_data, &ws, &good).expect("首次导入应成功");
    let manifest_path = manifest_path_under(app_data.clone(), &ws);
    let baseline = std::fs::read(&manifest_path).unwrap();
    let dirs_before = staged_import_dirs(&workspace_dir_under(app_data.clone(), &ws));

    // 不支持的类型：必须拒绝。
    let bad = src_dir.join("note.txt");
    std::fs::write(&bad, b"nope").unwrap();
    assert!(matches!(
        import_document(&app_data, &ws, &bad),
        Err(WorkImportError::UnsupportedKind(_))
    ));

    // 缺失源：必须拒绝。
    assert!(matches!(
        import_document(&app_data, &ws, &src_dir.join("missing.pdf")),
        Err(WorkImportError::SourceUnreadable(_))
    ));

    // 两次失败后：清单字节不变，暂存目录集合不变（零残留）。
    assert_eq!(
        std::fs::read(&manifest_path).unwrap(),
        baseline,
        "失败导入不得改动清单"
    );
    assert_eq!(
        staged_import_dirs(&workspace_dir_under(app_data, &ws)),
        dirs_before,
        "失败导入不得留下暂存目录残留"
    );
}

/// 工作区下的 `imp-*` 暂存目录名集合（排序后可比）。
fn staged_import_dirs(ws_dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(ws_dir)
        .map(|it| {
            it.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("imp-"))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}
