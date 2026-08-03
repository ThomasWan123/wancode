//! v0.19 分层基础层：SurfaceBinding sidecar 存储 + 有效策略派生
//! （docs/design/v0.19-layered-surfaces.md §0.1，四轮评审定稿）。
//!
//! 两层契约：**不可变的是会话层身份，不是权限规则的副本**。
//! - [`SurfaceBinding`]（持久化，不可变）：只有 session_id / surface_kind /
//!   created_policy_version，零策略内容。
//! - [`derive_effective_policy`]（纯函数，不持久化）：每次按**当前**代码
//!   规则派生，磁盘里永远没有可信任的权限列表——旧版本 binding 也用当前
//!   规则派生，杜绝陈旧权限快照。
//!
//! 存储：引擎 Summary 无扩展字段保留能力（重写即丢，探针结论 2026-08-03），
//! 故 sidecar 落 WanCode 自有目录：每 session 一个独立文件（避免全局 JSON
//! 并发覆盖）；文件名 = session ID 的 SHA-256 hex（原始 ID 不进路径，路径
//! 穿越型 ID 无法逃出目录）；文件内保存原始 ID，读取时交叉校验；写入 =
//! 同目录临时文件 + 原子替换（沿 write_config_atomic 惯例）。
//!
//! 迁移契约（binding 缺失**不能**永久解释成 legacy Code——否则「引擎建
//! 会话成功、sidecar 写入前崩溃」的新 Chat/Work 会话下次恢复会被错误提升
//! 为 Code）：首次升级枚举全部现存会话幂等回填 Code → 全部成功才原子写
//! `surface-binding-v1.complete` → 标记存在后缺 binding = 结构化
//! `unbound_surface` 阻塞，绝不默认 Code；新会话 sidecar 写失败 = 会话
//! 可留存但不可运行，等待显式恢复/认领。
//!
//! 威胁边界：不防拥有本机账户权限的恶意用户（无密钥/MAC）；目标是损坏、
//! 错配与状态漂移的 fail-closed。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// 当前策略规则代号。恢复时**始终**以此为准派生有效策略；binding 里的
/// [`SurfaceBinding::created_policy_version`] 只用于迁移判定与诊断。
pub const CURRENT_POLICY_VERSION: u32 = 1;

/// 迁移完成标记文件名（v1 命名空间，未来 schema 演进换新标记）。
pub const MIGRATION_MARKER: &str = "surface-binding-v1.complete";

/// 会话的层身份。创建时一次性确定，禁止跨层重新归属（换层 = 新开会话）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SurfaceKind {
    Chat,
    Code,
    Work,
    Cowork,
}

/// 持久化的层身份（sidecar 文件内容）。策略内容一概不入盘。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceBinding {
    /// 原始 session ID（文件名只有其 SHA-256，读取时与请求方交叉校验）。
    pub session_id: String,
    pub surface_kind: SurfaceKind,
    /// 创建时的规则代号。派生有效策略永远用 CURRENT_POLICY_VERSION 的
    /// 当前代码规则；此字段仅供迁移判定，磁盘值不构成权限来源。
    pub created_policy_version: u32,
}

/// 文件系统访问面（派生结果的一部分；G23/G25 的断言对象）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FsScope {
    /// 不访问用户/项目文件系统（应用私有会话资产目录除外）。
    None,
    /// 用户项目目录（现状 Code 语义）。
    Project,
    /// 仅 app_data_dir()/work/<workspace-id>/（原件只读）。
    WorkStaging,
    /// worktree cwd（档 A/档 B 语义由 Cowork 探针定，见设计稿 §1.1）。
    Worktree,
}

/// 工具面（派生结果；后续接线时映射到实际工具注入清单）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolProfile {
    /// 仅联网 MCP（web-search/reader），文件/执行工具不注入。
    WebOnly,
    /// 全量（现状）。
    Full,
    /// 读文档/检索/联网，零代码执行。
    DocReadOnly,
}

/// 默认权限模式（派生结果；与既有 PermMode 前端语义对接）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultPermission {
    Manual,
    /// 沿用用户设置（Code/Cowork）。
    UserConfigured,
}

/// 每次打开会话时实时派生的有效策略。**不持久化**——磁盘里没有它。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EffectiveSurfacePolicy {
    pub surface_kind: SurfaceKind,
    pub fs_scope: FsScope,
    pub tool_profile: ToolProfile,
    pub default_permission: DefaultPermission,
    /// 派生所用规则代号 == CURRENT_POLICY_VERSION（诊断用）。
    pub policy_version: u32,
}

/// 纯函数：由层身份 + 当前代码规则派生有效策略。没有任何 IO、不读磁盘
/// 权限列表；旧 created_policy_version 的会话同样落到这里（G22⑥）。
pub fn derive_effective_policy(kind: SurfaceKind) -> EffectiveSurfacePolicy {
    let (fs_scope, tool_profile, default_permission) = match kind {
        SurfaceKind::Chat => (FsScope::None, ToolProfile::WebOnly, DefaultPermission::Manual),
        SurfaceKind::Code => (
            FsScope::Project,
            ToolProfile::Full,
            DefaultPermission::UserConfigured,
        ),
        SurfaceKind::Work => (
            FsScope::WorkStaging,
            ToolProfile::DocReadOnly,
            DefaultPermission::Manual,
        ),
        SurfaceKind::Cowork => (
            FsScope::Worktree,
            ToolProfile::Full,
            DefaultPermission::UserConfigured,
        ),
    };
    EffectiveSurfacePolicy {
        surface_kind: kind,
        fs_scope,
        tool_profile,
        default_permission,
        policy_version: CURRENT_POLICY_VERSION,
    }
}

/// 结构化错误：全部 fail-closed 场景的机器可读原因（G22②⑤、契约表）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum SurfaceError {
    /// 迁移标记已存在但该会话无 binding——绝不默认 Code。
    UnboundSurface { session_id: String },
    /// sidecar 文件损坏 / 未知 surface_kind / 字段缺失。
    CorruptBinding { session_id: String, reason: String },
    /// 文件内 session_id 与请求方不符（哈希碰撞或文件被错放）。
    SessionIdMismatch { requested: String, embedded: String },
    /// 既有 binding 的 surface_kind 与写入请求不同——身份不可变。
    ImmutableKindConflict {
        session_id: String,
        existing: SurfaceKind,
        requested: SurfaceKind,
    },
    /// 存储 IO 失败（新会话此态 = 可留存不可运行，等显式恢复/认领）。
    StoreIo { session_id: String, reason: String },
    /// 迁移未全部成功，完成标记未写。
    MigrationIncomplete { failed: Vec<String> },
}

impl std::fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SurfaceError::UnboundSurface { session_id } => {
                write!(f, "unbound_surface: 会话 {session_id} 无层归属（迁移已完成，不默认 Code）")
            }
            SurfaceError::CorruptBinding { session_id, reason } => {
                write!(f, "corrupt_binding: 会话 {session_id} 的 sidecar 损坏：{reason}")
            }
            SurfaceError::SessionIdMismatch { requested, embedded } => {
                write!(f, "session_id_mismatch: 请求 {requested} ≠ 文件内 {embedded}")
            }
            SurfaceError::ImmutableKindConflict { session_id, existing, requested } => write!(
                f,
                "immutable_kind_conflict: 会话 {session_id} 已归属 {existing:?}，拒绝改为 {requested:?}"
            ),
            SurfaceError::StoreIo { session_id, reason } => {
                write!(f, "store_io: 会话 {session_id} sidecar 写入失败：{reason}")
            }
            SurfaceError::MigrationIncomplete { failed } => {
                write!(f, "migration_incomplete: {} 个会话回填失败", failed.len())
            }
        }
    }
}

/// sidecar 存储：root 目录下每 session 一个 `<sha256(session_id)>.json`。
pub struct SurfaceBindingStore {
    root: PathBuf,
}

impl SurfaceBindingStore {
    /// root 通常 = app_data_dir()/surface-bindings/。目录按需创建。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 文件路径 = root/<sha256 hex>.json。原始 ID 绝不进路径——路径穿越
    /// 型 session ID（`../x`、绝对路径、盘符）哈希后只是 64 个 hex 字符。
    pub fn path_for(&self, session_id: &str) -> PathBuf {
        let hash = Sha256::digest(session_id.as_bytes());
        let mut name = String::with_capacity(69);
        for b in hash {
            use std::fmt::Write;
            let _ = write!(name, "{b:02x}");
        }
        name.push_str(".json");
        self.root.join(name)
    }

    fn marker_path(&self) -> PathBuf {
        self.root.join(MIGRATION_MARKER)
    }

    /// 迁移完成标记是否存在。
    pub fn migration_complete(&self) -> bool {
        self.marker_path().is_file()
    }

    /// 原子写一个 binding。身份不可变：既有文件的 surface_kind 不同即拒绝
    /// （相同则幂等成功——迁移重复执行靠这个）。
    pub fn write(&self, binding: &SurfaceBinding) -> Result<(), SurfaceError> {
        if let Some(existing) = self.try_read_raw(&binding.session_id)? {
            if existing.surface_kind != binding.surface_kind {
                return Err(SurfaceError::ImmutableKindConflict {
                    session_id: binding.session_id.clone(),
                    existing: existing.surface_kind,
                    requested: binding.surface_kind,
                });
            }
            return Ok(()); // 幂等
        }
        std::fs::create_dir_all(&self.root).map_err(|e| SurfaceError::StoreIo {
            session_id: binding.session_id.clone(),
            reason: format!("创建 sidecar 目录失败: {e}"),
        })?;
        let content = serde_json::to_string_pretty(binding).map_err(|e| SurfaceError::StoreIo {
            session_id: binding.session_id.clone(),
            reason: format!("序列化失败: {e}"),
        })?;
        let path = self.path_for(&binding.session_id);
        // 同目录临时文件 + rename 原子替换（Windows MOVEFILE_REPLACE_EXISTING）。
        // 临时名带 pid + 会话哈希前缀，两个 session 并行写互不相扰。
        let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
        std::fs::write(&tmp, &content).map_err(|e| SurfaceError::StoreIo {
            session_id: binding.session_id.clone(),
            reason: format!("写临时文件失败: {e}"),
        })?;
        std::fs::rename(&tmp, &path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            SurfaceError::StoreIo {
                session_id: binding.session_id.clone(),
                reason: format!("原子替换失败: {e}"),
            }
        })
    }

    /// 读原始文件（存在性 + 解析 + 交叉校验），不涉及迁移语义。
    fn try_read_raw(&self, session_id: &str) -> Result<Option<SurfaceBinding>, SurfaceError> {
        let path = self.path_for(session_id);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(SurfaceError::StoreIo {
                    session_id: session_id.to_string(),
                    reason: format!("读取失败: {e}"),
                })
            }
        };
        let binding: SurfaceBinding =
            serde_json::from_str(&text).map_err(|e| SurfaceError::CorruptBinding {
                session_id: session_id.to_string(),
                reason: e.to_string(),
            })?;
        if binding.session_id != session_id {
            return Err(SurfaceError::SessionIdMismatch {
                requested: session_id.to_string(),
                embedded: binding.session_id,
            });
        }
        Ok(Some(binding))
    }

    /// 恢复会话时解析层归属（契约表第 3/4 行）：
    /// - binding 存在 → 返回；
    /// - 缺失且迁移**已**完成 → `UnboundSurface` 阻塞（含 sidecar 写失败
    ///   后的新会话：可留存不可运行，等显式恢复/认领）；
    /// - 缺失且迁移**未**完成 → `Ok(None)`，调用方应先跑迁移。
    pub fn resolve(&self, session_id: &str) -> Result<Option<SurfaceBinding>, SurfaceError> {
        match self.try_read_raw(session_id)? {
            Some(b) => Ok(Some(b)),
            None if self.migration_complete() => Err(SurfaceError::UnboundSurface {
                session_id: session_id.to_string(),
            }),
            None => Ok(None),
        }
    }

    /// 首次升级迁移：把现存会话幂等回填为 Code；**全部成功**才原子写
    /// 完成标记，任一失败返回 MigrationIncomplete 且不写标记。
    /// 会话枚举由调用方提供（App 层扫 ~/.grok sessions），本层不做 IO 发现。
    pub fn migrate_legacy<'a>(
        &self,
        existing_session_ids: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), SurfaceError> {
        let mut failed = Vec::new();
        for sid in existing_session_ids {
            let binding = SurfaceBinding {
                session_id: sid.to_string(),
                surface_kind: SurfaceKind::Code,
                created_policy_version: CURRENT_POLICY_VERSION,
            };
            match self.write(&binding) {
                Ok(()) => {}
                // 已有非 Code binding 的会话不是 legacy，跳过即幂等安全。
                Err(SurfaceError::ImmutableKindConflict { .. }) => {}
                Err(_) => failed.push(sid.to_string()),
            }
        }
        if !failed.is_empty() {
            return Err(SurfaceError::MigrationIncomplete { failed });
        }
        std::fs::create_dir_all(&self.root).map_err(|e| SurfaceError::StoreIo {
            session_id: String::new(),
            reason: format!("创建 sidecar 目录失败: {e}"),
        })?;
        let tmp = self.marker_path().with_extension("tmp");
        std::fs::write(&tmp, CURRENT_POLICY_VERSION.to_string()).map_err(|e| {
            SurfaceError::StoreIo {
                session_id: String::new(),
                reason: format!("写迁移标记临时文件失败: {e}"),
            }
        })?;
        std::fs::rename(&tmp, self.marker_path()).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            SurfaceError::StoreIo {
                session_id: String::new(),
                reason: format!("迁移标记原子替换失败: {e}"),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, SurfaceBindingStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = SurfaceBindingStore::new(dir.path().join("surface-bindings"));
        (dir, store)
    }

    fn binding(sid: &str, kind: SurfaceKind) -> SurfaceBinding {
        SurfaceBinding {
            session_id: sid.to_string(),
            surface_kind: kind,
            created_policy_version: CURRENT_POLICY_VERSION,
        }
    }

    // RED-1：四种 SurfaceKind round-trip。
    #[test]
    fn all_four_kinds_round_trip() {
        let (_g, s) = store();
        for (i, kind) in [
            SurfaceKind::Chat,
            SurfaceKind::Code,
            SurfaceKind::Work,
            SurfaceKind::Cowork,
        ]
        .into_iter()
        .enumerate()
        {
            let sid = format!("sess-{i}");
            s.write(&binding(&sid, kind)).expect("write");
            let back = s.resolve(&sid).expect("resolve").expect("some");
            assert_eq!(back.surface_kind, kind);
            assert_eq!(back.session_id, sid);
            assert_eq!(back.created_policy_version, CURRENT_POLICY_VERSION);
        }
    }

    // RED-2a：损坏 JSON → CorruptBinding。
    #[test]
    fn corrupt_json_blocks_structured() {
        let (_g, s) = store();
        s.write(&binding("sess-x", SurfaceKind::Chat)).unwrap();
        std::fs::write(s.path_for("sess-x"), "{not json").unwrap();
        match s.resolve("sess-x") {
            Err(SurfaceError::CorruptBinding { session_id, .. }) => {
                assert_eq!(session_id, "sess-x")
            }
            other => panic!("期望 CorruptBinding，得到 {other:?}"),
        }
    }

    // RED-2b：未知 surface_kind → CorruptBinding（枚举严格反序列化）。
    #[test]
    fn unknown_kind_blocks_structured() {
        let (_g, s) = store();
        s.write(&binding("sess-y", SurfaceKind::Code)).unwrap();
        std::fs::write(
            s.path_for("sess-y"),
            r#"{"session_id":"sess-y","surface_kind":"root","created_policy_version":1}"#,
        )
        .unwrap();
        assert!(matches!(
            s.resolve("sess-y"),
            Err(SurfaceError::CorruptBinding { .. })
        ));
    }

    // RED-2c：文件名与内部 session ID 不符 → SessionIdMismatch。
    #[test]
    fn embedded_id_mismatch_blocks() {
        let (_g, s) = store();
        s.write(&binding("sess-a", SurfaceKind::Work)).unwrap();
        // 把 sess-a 的文件内容改成宣称自己是 sess-b（文件错放/篡改漂移）。
        std::fs::write(
            s.path_for("sess-a"),
            serde_json::to_string(&binding("sess-b", SurfaceKind::Work)).unwrap(),
        )
        .unwrap();
        match s.resolve("sess-a") {
            Err(SurfaceError::SessionIdMismatch { requested, embedded }) => {
                assert_eq!(requested, "sess-a");
                assert_eq!(embedded, "sess-b");
            }
            other => panic!("期望 SessionIdMismatch，得到 {other:?}"),
        }
    }

    // RED-3：首次迁移回填 Code，重复执行幂等（含已有非 Code binding 不被改写）。
    #[test]
    fn legacy_migration_backfills_code_idempotently() {
        let (_g, s) = store();
        // 预置一个新层会话：迁移不得动它。
        s.write(&binding("sess-chat", SurfaceKind::Chat)).unwrap();
        let legacy = ["old-1", "old-2", "sess-chat"];
        s.migrate_legacy(legacy.iter().copied()).expect("first run");
        assert!(s.migration_complete());
        // 第二次执行：幂等，无错误，归属不变。
        s.migrate_legacy(legacy.iter().copied()).expect("second run");
        assert_eq!(
            s.resolve("old-1").unwrap().unwrap().surface_kind,
            SurfaceKind::Code
        );
        assert_eq!(
            s.resolve("sess-chat").unwrap().unwrap().surface_kind,
            SurfaceKind::Chat,
            "迁移不得把已归属会话改写为 Code"
        );
    }

    // RED-4：任一回填失败 → MigrationIncomplete 且不写完成标记。
    #[test]
    fn migration_failure_blocks_marker() {
        let (_g, s) = store();
        // 制造一个必然写失败的会话：其 sidecar 路径被占为「目录」。
        let victim = "will-fail";
        std::fs::create_dir_all(s.path_for(victim)).unwrap();
        let err = s
            .migrate_legacy(["ok-1", victim, "ok-2"])
            .expect_err("必须失败");
        assert!(matches!(
            &err,
            SurfaceError::MigrationIncomplete { failed } if failed == &vec![victim.to_string()]
        ));
        assert!(!s.migration_complete(), "任一失败不得写完成标记");
    }

    // RED-5：标记完成后缺 binding → UnboundSurface，绝不默认 Code。
    #[test]
    fn unbound_after_marker_blocks() {
        let (_g, s) = store();
        s.migrate_legacy(std::iter::empty()).unwrap();
        assert!(s.migration_complete());
        match s.resolve("ghost") {
            Err(SurfaceError::UnboundSurface { session_id }) => assert_eq!(session_id, "ghost"),
            other => panic!("期望 UnboundSurface，得到 {other:?}"),
        }
    }

    // RED-6：新会话创建后 sidecar 写失败 → 恢复不得升为 Code。
    // （模拟：写失败 = 磁盘上无 binding；迁移标记已完成 → 必须 UnboundSurface。）
    #[test]
    fn sidecar_write_failure_never_promotes_to_code() {
        let (_g, s) = store();
        s.migrate_legacy(std::iter::empty()).unwrap();
        // 模拟「引擎建会话成功、sidecar 写入前崩溃」：制造写失败。
        let sid = "chat-crashed";
        std::fs::create_dir_all(s.path_for(sid)).unwrap(); // 路径被目录占位 → write 必失败
        assert!(matches!(
            s.write(&binding(sid, SurfaceKind::Chat)),
            Err(SurfaceError::StoreIo { .. })
        ));
        std::fs::remove_dir(s.path_for(sid)).unwrap(); // 崩溃后重启：占位不再，binding 仍缺
        match s.resolve(sid) {
            Err(SurfaceError::UnboundSurface { .. }) => {}
            other => panic!("写失败的会话必须 UnboundSurface（可留存不可运行），得到 {other:?}"),
        }
    }

    // RED-7：两个 session 并行写入互不覆盖。
    #[test]
    fn concurrent_writes_do_not_clobber() {
        let (_g, s) = store();
        let root = s.root.clone();
        let mk = |sid: String, kind: SurfaceKind| {
            let root = root.clone();
            std::thread::spawn(move || {
                let s = SurfaceBindingStore::new(root);
                for _ in 0..50 {
                    s.write(&SurfaceBinding {
                        session_id: sid.clone(),
                        surface_kind: kind,
                        created_policy_version: CURRENT_POLICY_VERSION,
                    })
                    .expect("并行写失败");
                }
            })
        };
        let a = mk("par-a".into(), SurfaceKind::Chat);
        let b = mk("par-b".into(), SurfaceKind::Work);
        a.join().unwrap();
        b.join().unwrap();
        assert_eq!(
            s.resolve("par-a").unwrap().unwrap().surface_kind,
            SurfaceKind::Chat
        );
        assert_eq!(
            s.resolve("par-b").unwrap().unwrap().surface_kind,
            SurfaceKind::Work
        );
    }

    // RED-8：旧 created_policy_version 按当前规则派生，不能恢复旧权限。
    #[test]
    fn old_policy_version_derives_with_current_rules() {
        let (_g, s) = store();
        let old = SurfaceBinding {
            session_id: "vintage".into(),
            surface_kind: SurfaceKind::Chat,
            created_policy_version: 0, // 早于当前代号
        };
        s.write(&old).unwrap();
        let back = s.resolve("vintage").unwrap().unwrap();
        let policy = derive_effective_policy(back.surface_kind);
        // 派生结果与「今天新建的同层会话」逐字段一致——磁盘版本号不影响权限。
        assert_eq!(policy, derive_effective_policy(SurfaceKind::Chat));
        assert_eq!(policy.policy_version, CURRENT_POLICY_VERSION);
        assert_eq!(policy.fs_scope, FsScope::None);
        assert_eq!(policy.tool_profile, ToolProfile::WebOnly);
    }

    // RED-9：路径穿越型 session ID 无法逃出 sidecar 目录。
    #[test]
    fn path_traversal_ids_stay_inside_root() {
        let (_g, s) = store();
        for evil in [
            "../escape",
            "..\\escape",
            "/etc/passwd",
            "C:\\Windows\\system32\\cfg",
            "a/../../b",
            "..",
        ] {
            let p = s.path_for(evil);
            assert_eq!(
                p.parent().unwrap(),
                s.root.as_path(),
                "{evil} 的路径必须直接位于 root 下"
            );
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            assert!(
                name.len() == 69 && name.ends_with(".json")
                    && name[..64].bytes().all(|b| b.is_ascii_hexdigit()),
                "{evil} 的文件名必须是 64 hex + .json，实际 {name}"
            );
            // 写入+读回全链路也不逃逸。
            s.write(&binding(evil, SurfaceKind::Code)).unwrap();
            assert!(p.is_file());
        }
    }

    // RED-附：身份不可变——同一会话换 kind 写入被拒。
    #[test]
    fn surface_kind_is_immutable() {
        let (_g, s) = store();
        s.write(&binding("fixed", SurfaceKind::Chat)).unwrap();
        match s.write(&binding("fixed", SurfaceKind::Code)) {
            Err(SurfaceError::ImmutableKindConflict { existing, requested, .. }) => {
                assert_eq!(existing, SurfaceKind::Chat);
                assert_eq!(requested, SurfaceKind::Code);
            }
            other => panic!("期望 ImmutableKindConflict，得到 {other:?}"),
        }
        // 同 kind 重写幂等成功。
        s.write(&binding("fixed", SurfaceKind::Chat)).expect("幂等");
    }
}
