//! v0.19-2a 启动迁移门（设计稿 §6 第 2 步之 a）。
//!
//! 职责边界：只做「应用启动时把存量会话迁入 SurfaceBinding sidecar，
//! 且所有会话启动入口等待同一个迁移结果」。不改 agent_start 的参数、
//! 不裁剪工具、不碰 UI——那些是 2b/2c/2d。
//!
//! 语义：
//! - 会话枚举走引擎公开的 `list_summaries(None)`（全量、无数量上限），
//!   不手扫 ~/.grok，也不用截断 30 条的 fetch_merged；
//! - 成功结果缓存终身（此后所有入口零开销放行）；失败**不**缓存——
//!   下一个调用者重试一次（migrate_legacy 幂等，磁盘故障修复后可自愈）；
//! - 并发调用被内部互斥串行化：迁移进行中到达的调用者在锁上等待，
//!   共享同一次执行的结果，绝不并行跑两个迁移；
//! - 另一进程持迁移锁（migration_locked）时有界重试；仍占用则如实
//!   上抛（结构化、可稍后重试）；
//! - 损坏标记 / 迁移不完整 / 存储 IO 等一律阻止会话启动，错误以
//!   `SURFACE_GATE_BLOCKED: {json}` 形态给前端（serde tag = code）。

use crate::surface::{SurfaceBinding, SurfaceBindingStore, SurfaceError, SurfaceKind};
use std::path::PathBuf;
use std::sync::Arc;

/// migration_locked 的有界重试：40 × 250ms = 最多等 10s。
const LOCKED_RETRIES: u32 = 40;
const LOCKED_BACKOFF_MS: u64 = 250;

pub struct SurfaceGate {
    store: SurfaceBindingStore,
    /// None = 尚未跑；Some(Ok) = 已完成（终身缓存）；Some(Err) = 上次
    /// 失败（不作数，下个调用者重试）。锁在整个迁移执行期间持有——
    /// 这就是「并发入口共享同一结果」的实现。
    slot: tokio::sync::Mutex<Option<Result<(), SurfaceError>>>,
}

impl SurfaceGate {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            store: SurfaceBindingStore::new(root),
            slot: tokio::sync::Mutex::new(None),
        }
    }

    /// 供 2b 起的后续接线读取 binding（本 PR 内测试也用）。
    pub fn store(&self) -> &SurfaceBindingStore {
        &self.store
    }

    /// 等待迁移门：成功即放行；失败返回结构化错误（调用方拒绝启动会话）。
    /// `enumerate` 提供全量存量会话 ID（生产 = 引擎 list_summaries；
    /// 测试注入）。
    pub async fn ensure_migrated<F, Fut>(&self, enumerate: F) -> Result<(), SurfaceError>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<String>, String>>,
    {
        let mut slot = self.slot.lock().await;
        if let Some(Ok(())) = *slot {
            return Ok(());
        }
        // 上次失败或从未跑过：本调用者执行（锁持有中，其余调用者排队）。
        let ids = match enumerate().await {
            Ok(ids) => ids,
            Err(reason) => {
                let e = SurfaceError::StoreIo {
                    session_id: String::new(),
                    reason: format!("会话枚举失败: {reason}"),
                };
                *slot = Some(Err(e.clone()));
                return Err(e);
            }
        };
        let mut result = self
            .store
            .migrate_legacy(ids.iter().map(|s| s.as_str()));
        // 另一进程正迁移：有界等待后重试（其完成后本次通常直接 no-op 过）。
        let mut tries = 0;
        while matches!(result, Err(SurfaceError::MigrationLocked { .. })) && tries < LOCKED_RETRIES
        {
            tries += 1;
            tokio::time::sleep(std::time::Duration::from_millis(LOCKED_BACKOFF_MS)).await;
            result = self
                .store
                .migrate_legacy(ids.iter().map(|s| s.as_str()));
        }
        // 门级裁决（复核 P0：孤儿不得全局拒启动）：标记后存在无归属会话
        // 是**个别会话**的病，不是门的病——store 层如实上报（复核三契约
        // 不变），门层降为诊断日志放行；这些会话在各自 resolve 时单独被
        // unbound_surface 拦住，走显式认领。若把它做成门级失败，一个
        // 崩溃孤儿就能把 235 个健康会话全部锁死（真实现场实测形态）。
        let result = match result {
            Err(SurfaceError::PostMarkerUnbound { session_ids }) => {
                tracing::warn!(
                    count = session_ids.len(),
                    ids = ?session_ids,
                    "标记后存在无归属会话：门放行，逐会话 resolve 时单独阻塞（待显式认领）"
                );
                Ok(())
            }
            other => other,
        };
        *slot = Some(result.clone());
        result
    }
}

/// Tauri 托管态：启动时在 setup 注册（root = app_data_dir()/surface-bindings/）。
pub struct SurfaceState {
    gate: Arc<SurfaceGate>,
}

impl SurfaceState {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            gate: Arc::new(SurfaceGate::new(root)),
        }
    }

    pub fn gate(&self) -> Arc<SurfaceGate> {
        self.gate.clone()
    }

    /// 新会话身份写入（最低事务链：引擎返回 ID → 写 binding → 成功后
    /// 调用方才安装 handle/返回前端）。写失败调用方必须取消本次 Agent。
    pub fn bind_new_session(
        &self,
        session_id: &str,
        kind: SurfaceKind,
    ) -> Result<SurfaceBinding, SurfaceError> {
        // W2-c:Work 层需要 workspace_id,不能经本(仅 kind 的)入口创建——
        // 用 bind_new_work_session。生产新会话目前固定 Code(2d 评审前),
        // 故此约束不影响现有路径。
        if kind == SurfaceKind::Work {
            return Err(SurfaceError::CorruptBinding {
                session_id: session_id.to_string(),
                reason: "Work 层会话须经 bind_new_work_session 提供 workspace_id".into(),
            });
        }
        let b = SurfaceBinding::new(session_id, kind);
        self.gate.store().write(&b)?;
        Ok(b)
    }

    /// 创建 Work 层新会话,携带其持久工作区身份(W2-c / R3-F2)。
    pub fn bind_new_work_session(
        &self,
        session_id: &str,
        workspace_id: crate::work_staging::WorkspaceId,
    ) -> Result<SurfaceBinding, SurfaceError> {
        let b = SurfaceBinding::new_work(session_id, workspace_id);
        self.gate.store().write(&b)?;
        Ok(b)
    }

    /// 恢复会话身份解析：在启动引擎、加载会话之前调用；层身份只信
    /// sidecar，不信前端参数或 localStorage。
    pub fn resolve(&self, session_id: &str) -> Result<SurfaceBinding, SurfaceError> {
        self.gate.store().resolve(session_id)
    }

    /// 派生路径（fork / worktree resume 等）：新会话继承源会话的层身份。
    /// 源无归属即失败——派生不能凭空发明身份。
    pub fn inherit_binding(
        &self,
        source_session_id: &str,
        new_session_id: &str,
    ) -> Result<SurfaceBinding, SurfaceError> {
        let source = self.gate.store().resolve(source_session_id)?;
        // W2-c / R3-F2:fork **完整继承**源身份,含 workspace_id(Work 源
        // fork 出的会话必须留在同一工作区,而非丢掉工作区身份)。
        let inherited = SurfaceBinding {
            binding_schema_version: crate::surface::CURRENT_BINDING_SCHEMA_VERSION,
            session_id: new_session_id.to_string(),
            surface_kind: source.surface_kind,
            created_policy_version: crate::surface::CURRENT_POLICY_VERSION,
            workspace_id: source.workspace_id.clone(),
        };
        self.gate.store().write(&inherited)?;
        Ok(inherited)
    }

    /// 生产迁移门：枚举 = 引擎公开的全量会话列举（无数量上限）。
    pub async fn ensure_migrated(&self) -> Result<(), SurfaceError> {
        self.gate
            .ensure_migrated(|| async {
                xai_grok_shell::session::persistence::list_summaries(None)
                    .await
                    .map(|summaries| {
                        summaries
                            .into_iter()
                            .map(|s| s.info.id.0.to_string())
                            .collect()
                    })
                    .map_err(|e| e.to_string())
            })
            .await
    }
}

/// 迁移门错误 → 前端契约字符串（沿 MODEL_REQUIRED 前缀式错误码惯例，
/// 负载为结构化 JSON，serde tag = code）。
pub fn gate_blocked_message(e: &SurfaceError) -> String {
    format!(
        "SURFACE_GATE_BLOCKED: {}",
        serde_json::to_string(e).unwrap_or_else(|_| e.to_string())
    )
}

/// 身份链错误（resolve/写 binding 失败）→ 前端契约字符串。
pub fn binding_blocked_message(e: &SurfaceError) -> String {
    format!(
        "SURFACE_BINDING_BLOCKED: {}",
        serde_json::to_string(e).unwrap_or_else(|_| e.to_string())
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::surface::SurfaceKind;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn gate() -> (tempfile::TempDir, Arc<SurfaceGate>) {
        let dir = tempfile::tempdir().unwrap();
        let g = Arc::new(SurfaceGate::new(dir.path().join("surface-bindings")));
        (dir, g)
    }

    // 2a-1：超过 30 个会话全量迁移（防再次引入 fetch_merged 式截断）。
    #[tokio::test]
    async fn migrates_more_than_thirty_sessions() {
        let (_d, g) = gate();
        let ids: Vec<String> = (0..37).map(|i| format!("legacy-{i}")).collect();
        let ids2 = ids.clone();
        g.ensure_migrated(move || {
            let ids = ids2.clone();
            async move { Ok(ids) }
        })
        .await
        .expect("37 会话迁移须成功");
        assert!(g.store().migration_complete().unwrap());
        for id in &ids {
            assert_eq!(
                g.store().resolve(id).unwrap().surface_kind,
                SurfaceKind::Code,
                "{id} 必须回填为 Code"
            );
        }
    }

    // 2a-2：迁移失败 → 门返回结构化错误（调用方禁止启动会话）；
    // 故障修复后下一个调用者重试成功（失败不被缓存）。
    #[tokio::test]
    async fn migration_failure_blocks_then_retry_succeeds() {
        let (_d, g) = gate();
        let victim = "broken";
        std::fs::create_dir_all(g.store().path_for(victim)).unwrap();
        let enumerate = {
            let victim = victim.to_string();
            move || {
                let v = victim.clone();
                async move { Ok(vec!["ok-1".to_string(), v]) }
            }
        };
        match g.ensure_migrated(enumerate.clone()).await {
            Err(SurfaceError::MigrationIncomplete { failed }) => {
                assert_eq!(failed, vec![victim.to_string()])
            }
            other => panic!("期望 MigrationIncomplete，得到 {other:?}"),
        }
        assert!(!g.store().migration_complete().unwrap());
        // 修复后重试：同一个门自愈。
        std::fs::remove_dir(g.store().path_for(victim)).unwrap();
        g.ensure_migrated(enumerate).await.expect("修复后须成功");
        assert!(g.store().migration_complete().unwrap());
    }

    // 2a-3：并发入口共享同一结果——迁移只执行一次（枚举计数 == 1），
    // 其余调用者在门上等待后直接放行。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_entries_share_single_run() {
        let (_d, g) = gate();
        let calls = Arc::new(AtomicU32::new(0));
        let mk = |g: Arc<SurfaceGate>, calls: Arc<AtomicU32>| {
            tokio::spawn(async move {
                g.ensure_migrated(move || {
                    let calls = calls.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        // 放大执行窗口，让其余调用者真的撞上进行中的迁移。
                        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                        Ok(vec!["s1".to_string(), "s2".to_string()])
                    }
                })
                .await
            })
        };
        let handles: Vec<_> = (0..8).map(|_| mk(g.clone(), calls.clone())).collect();
        for h in handles {
            h.await.unwrap().expect("全部调用者须放行");
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1, "迁移只执行一次");
        assert!(g.store().migration_complete().unwrap());
    }

    // 2a-4：另一进程持迁移锁 → 有界重试；释放后本门放行。
    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn locked_by_other_process_retries_until_released() {
        let (_d, g) = gate();
        let root = g.store().path_for("x");
        let root = root.parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&root).unwrap();
        // 模拟他进程：**先**在主线程同步取得独占锁（时序确定，门必然
        // 撞锁），再由后台线程 600ms 后释放。
        let lock_path = root.join("migration.lock");
        let held = {
            use std::os::windows::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .share_mode(0)
                .open(lock_path)
                .unwrap()
        };
        let holder = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(600));
            drop(held);
        });
        g.ensure_migrated(|| async { Ok(vec!["s1".to_string()]) })
            .await
            .expect("锁释放后须放行");
        holder.join().unwrap();
        assert!(g.store().migration_complete().unwrap());
    }

    // 2a-6：生产链——迁移 → 新建（写 binding）→ 进程重启（新实例）→
    // 恢复 resolve 成功且身份保持；未写 binding 的「崩溃孤儿」则被拦。
    #[tokio::test]
    async fn full_chain_migrate_create_restart_restore() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("surface-bindings");
        // 第一进程：迁移 + 新建会话写 binding。
        {
            let s = SurfaceState::new(root.clone());
            s.gate()
                .ensure_migrated(|| async { Ok(vec!["legacy-1".to_string()]) })
                .await
                .unwrap();
            s.bind_new_session("fresh-chat", SurfaceKind::Chat).unwrap();
            // 模拟崩溃孤儿：引擎已建会话但 binding 未写成。
            // （什么都不写就是这个状态。）
        }
        // 第二进程（重启）：全新实例。
        let s2 = SurfaceState::new(root);
        s2.gate()
            .ensure_migrated(|| async { Ok(vec!["legacy-1".to_string()]) })
            .await
            .expect("重启后门幂等放行");
        assert_eq!(s2.resolve("legacy-1").unwrap().surface_kind, SurfaceKind::Code);
        assert_eq!(
            s2.resolve("fresh-chat").unwrap().surface_kind,
            SurfaceKind::Chat,
            "新建会话重启后必须恢复出同一层身份"
        );
        assert!(matches!(
            s2.resolve("orphan-crashed"),
            Err(SurfaceError::UnboundSurface { .. })
        ));
    }

    // 2a-7：派生路径继承——fork/worktree 的新会话拿源会话的层身份；
    // 源无归属时派生失败，不发明身份。
    #[tokio::test]
    async fn inherit_binding_copies_source_kind() {
        let dir = tempfile::tempdir().unwrap();
        let s = SurfaceState::new(dir.path().join("surface-bindings"));
        s.gate()
            .ensure_migrated(|| async { Ok(vec![]) })
            .await
            .unwrap();
        // W2-c:Work 源经 bind_new_work_session 建,fork 必须**继承 workspace_id**。
        let ws = crate::work_staging::WorkspaceId::mint();
        s.bind_new_work_session("src-work", ws.clone()).unwrap();
        let b = s.inherit_binding("src-work", "child-1").unwrap();
        assert_eq!(b.surface_kind, SurfaceKind::Work);
        assert_eq!(b.workspace_id, Some(ws.clone()), "fork 必须继承源的 workspace_id");
        let child = s.resolve("child-1").unwrap();
        assert_eq!(child.surface_kind, SurfaceKind::Work);
        assert_eq!(child.workspace_id, Some(ws), "child 持久绑定同一工作区");
        // 源无归属：拒绝派生。
        assert!(matches!(
            s.inherit_binding("ghost-src", "child-2"),
            Err(SurfaceError::UnboundSurface { .. })
        ));
        assert!(matches!(
            s.resolve("child-2"),
            Err(SurfaceError::UnboundSurface { .. }),

        ));
    }

    // 2a-9（review 身份 P0 判别证据）：review 临时会话按 review_run 的
    // 代码路径（活跃会话 → inherit_binding）派生。判别点：源是非 Code
    // 层时，派生结果必须是源的层——旧实现硬编码 bind(Code) 在本测试
    // 必红。源无归属时 review 派生同样被拒。
    #[tokio::test]
    async fn review_style_derivation_inherits_source_never_invents_code() {
        let dir = tempfile::tempdir().unwrap();
        let s = SurfaceState::new(dir.path().join("surface-bindings"));
        s.gate()
            .ensure_migrated(|| async { Ok(vec![]) })
            .await
            .unwrap();
        // 活跃会话是 Chat（非 Code）——审查派生必须继承 Chat。
        s.bind_new_session("active-chat", SurfaceKind::Chat).unwrap();
        let review = s.inherit_binding("active-chat", "review-temp-1").unwrap();
        assert_eq!(review.surface_kind, SurfaceKind::Chat);
        assert_ne!(
            review.surface_kind,
            SurfaceKind::Code,
            "review 派生绝不发明 Code 身份"
        );
        assert_eq!(
            s.resolve("review-temp-1").unwrap().surface_kind,
            SurfaceKind::Chat,
            "崩溃孤儿复活时也按源会话层恢复"
        );
        // Code 源正常继承 Code（现状主路径）。
        s.bind_new_session("active-code", SurfaceKind::Code).unwrap();
        assert_eq!(
            s.inherit_binding("active-code", "review-temp-2").unwrap().surface_kind,
            SurfaceKind::Code
        );
        // 源无归属（孤儿会话上触发审查）：拒绝派生，不发明身份。
        assert!(matches!(
            s.inherit_binding("orphan-active", "review-temp-3"),
            Err(SurfaceError::UnboundSurface { .. })
        ));
        assert!(matches!(
            s.resolve("review-temp-3"),
            Err(SurfaceError::UnboundSurface { .. })
        ));
    }

    // 2a-8（复核 P0 验收样本形状）：标记后存在孤儿 → 门必须放行、
    // 健康会话正常打开，仅孤儿在 resolve 时单独 unbound_surface。
    #[tokio::test]
    async fn gate_passes_with_orphans_and_blocks_them_individually() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("surface-bindings");
        // 第一进程：迁移完成 + 一个健康新会话。
        {
            let s = SurfaceState::new(root.clone());
            s.gate()
                .ensure_migrated(|| async { Ok(vec!["healthy-legacy".to_string()]) })
                .await
                .unwrap();
            s.bind_new_session("healthy-new", SurfaceKind::Chat).unwrap();
            // 孤儿 = 引擎建会话成功、binding 写入前崩溃：什么都不写。
        }
        // 冷启动（新实例）：枚举包含孤儿——门必须放行。
        let s2 = SurfaceState::new(root);
        s2.gate()
            .ensure_migrated(|| async {
                Ok(vec![
                    "healthy-legacy".to_string(),
                    "healthy-new".to_string(),
                    "orphan-crashed".to_string(),
                ])
            })
            .await
            .expect("孤儿不得把门打死（235 个健康会话不能陪葬）");
        // 健康会话正常打开。
        assert_eq!(
            s2.resolve("healthy-legacy").unwrap().surface_kind,
            SurfaceKind::Code
        );
        assert_eq!(
            s2.resolve("healthy-new").unwrap().surface_kind,
            SurfaceKind::Chat
        );
        // 仅孤儿单独被拦。
        assert!(matches!(
            s2.resolve("orphan-crashed"),
            Err(SurfaceError::UnboundSurface { .. })
        ));
    }

    // 2a-5：损坏标记 → 门阻塞且错误结构化（前端契约串可解析）。
    #[tokio::test]
    async fn corrupt_marker_blocks_gate_with_structured_error() {
        let (_d, g) = gate();
        g.ensure_migrated(|| async { Ok(vec![]) }).await.unwrap();
        std::fs::write(
            g.store().path_for("x").parent().unwrap().join("surface-binding-v1.complete"),
            "garbage",
        )
        .unwrap();
        // 新门实例（模拟重启后）：损坏标记必须阻塞。
        let g2 = SurfaceGate::new(g.store().path_for("x").parent().unwrap());
        let err = g2
            .ensure_migrated(|| async { Ok(vec![]) })
            .await
            .expect_err("损坏标记必须阻塞");
        assert!(matches!(err, SurfaceError::CorruptMigrationMarker { .. }));
        let msg = gate_blocked_message(&err);
        assert!(msg.starts_with("SURFACE_GATE_BLOCKED: {"));
        assert!(msg.contains("\"code\":\"corrupt_migration_marker\""));
    }
}
