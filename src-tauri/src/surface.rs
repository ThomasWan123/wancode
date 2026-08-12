//! v0.19 分层基础层：SurfaceBinding sidecar 存储 + 有效策略派生
//! （docs/design/v0.19-layered-surfaces.md §0.1，四轮评审定稿 + 基础层复核收口）。
//!
//! 两层契约：**不可变的是会话层身份，不是权限规则的副本**。
//! - [`SurfaceBinding`]（持久化，不可变）：session_id / surface_kind /
//!   版本号两枚，零策略内容。
//! - [`derive_effective_policy`]（纯函数，不持久化）：每次按**当前**代码
//!   规则派生，磁盘里永远没有可信任的权限列表——旧版本 binding 也用当前
//!   规则派生，杜绝陈旧权限快照。**来自未来的版本**（schema 或 policy
//!   代号大于当前程序）结构化阻塞：旧程序绝不按较旧规则运行新会话。
//!
//! 存储：引擎 Summary 无扩展字段保留能力（重写即丢，探针结论 2026-08-03），
//! 故 sidecar 落 WanCode 自有目录：每 session 一个独立文件（避免全局 JSON
//! 并发覆盖）；文件名 = session ID 的 SHA-256 hex（原始 ID 不进路径，路径
//! 穿越型 ID 无法逃出目录）；文件内保存原始 ID，读取时交叉校验；写入 =
//! 唯一临时文件（pid+序号）→ sync_all → **no-clobber 发布**（hard_link，
//! 目标已存在即失败）——同 session 两种 kind 的并发首写只能一个成功。
//!
//! 迁移契约（binding 缺失**不能**永久解释成 legacy Code——否则「引擎建
//! 会话成功、sidecar 写入前崩溃」的新 Chat/Work 会话下次恢复会被错误提升
//! 为 Code）：首次升级枚举全部现存会话幂等回填 Code → 全部成功才发布
//! `surface-binding-v1.complete`（有内容、有版本校验，同样 no-clobber）→
//! 标记有效后：缺 binding = `unbound_surface` 阻塞；**再次调用迁移是
//! no-op**，不得回填任何新 ID。标记前缺 binding = `migration_required`
//! 结构化返回，调用方不得自行解释为 Code。
//!
//! 威胁边界：不防拥有本机账户权限的恶意用户（无密钥/MAC）；目标是损坏、
//! 错配与状态漂移的 fail-closed。

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// 当前策略规则代号。恢复时**始终**以此为准派生有效策略；binding 里的
/// [`SurfaceBinding::created_policy_version`] 只用于迁移判定与诊断。
pub const CURRENT_POLICY_VERSION: u32 = 1;

/// sidecar 文件格式版本。读到更大的值 = 文件来自未来版本的 WanCode，
/// 结构化阻塞（不猜测字段语义）。
pub const CURRENT_BINDING_SCHEMA_VERSION: u32 = 2;
/// 认得的最旧 binding schema。v1 = Chat/Code 时代(无 workspace_id 字段);
/// v2 = 加了可选 workspace_id(W2-c)。读时显式接受 [v1..=CURRENT],未来阻塞。
pub const OLDEST_READABLE_BINDING_SCHEMA_VERSION: u32 = 1;

/// 迁移完成标记文件名（v1 命名空间，未来 schema 演进换新标记）。
pub const MIGRATION_MARKER: &str = "surface-binding-v1.complete";

/// 标记文件内容（首行）。读取时必须匹配，损坏/未知内容结构化阻塞。
const MARKER_CONTENT: &str = "surface-binding-migration v1";

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
/// `deny_unknown_fields`：未知字段 = 文件不是本程序认识的形状，阻塞
/// 而非静默忽略。
///
/// **关于 `workspace_id`（v0.20 W2-c,不 bump schema 的理由）**:一般"新增
/// 字段必须 bump schema version",因为 `deny_unknown_fields` 会让旧读者拒绝
/// 带新字段的文件。但 `workspace_id` 仅 Work 层携带,而 **Work 是 v0.20 全新
/// 层**——已发布版本从不创建 Work 绑定。配合 `skip_serializing_if=None`,
/// Chat/Code 绑定(workspace_id=None)序列化时**不含该字段**,与旧格式逐字节
/// 相同;旧读者只可能读到 Chat/Code 文件,永远读不到带 workspace_id 的 Work
/// 文件(那是升级后才产生的新数据)。因此现存数据零改动、零迁移,schema 保持 1。
/// (若未来给 Chat/Code 也加字段,那才必须 bump——因为会改动现存文件形状。)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceBinding {
    /// sidecar 文件格式版本（见 CURRENT_BINDING_SCHEMA_VERSION）。
    pub binding_schema_version: u32,
    /// 原始 session ID（文件名只有其 SHA-256，读取时与请求方交叉校验）。
    pub session_id: String,
    pub surface_kind: SurfaceKind,
    /// 创建时的规则代号。派生有效策略永远用 CURRENT_POLICY_VERSION 的
    /// 当前代码规则；此字段仅供迁移判定，磁盘值不构成权限来源。
    pub created_policy_version: u32,
    /// Work 层会话绑定的持久工作区身份(codex R3-F2 / W2-c)。仅 Work 层为
    /// Some;Chat/Code 为 None 且不序列化(见上文)。检索/会话据此绑定单一
    /// 工作区清单,文档不得跨工作区串。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<crate::work_staging::WorkspaceId>,
}

impl SurfaceBinding {
    /// 以当前版本号构造(Chat/Code:无工作区身份)。
    pub fn new(session_id: impl Into<String>, surface_kind: SurfaceKind) -> Self {
        Self {
            binding_schema_version: CURRENT_BINDING_SCHEMA_VERSION,
            session_id: session_id.into(),
            surface_kind,
            created_policy_version: CURRENT_POLICY_VERSION,
            workspace_id: None,
        }
    }

    /// 构造 Work 层绑定,携带其持久工作区身份。
    pub fn new_work(
        session_id: impl Into<String>,
        workspace_id: crate::work_staging::WorkspaceId,
    ) -> Self {
        Self {
            binding_schema_version: CURRENT_BINDING_SCHEMA_VERSION,
            session_id: session_id.into(),
            surface_kind: SurfaceKind::Work,
            created_policy_version: CURRENT_POLICY_VERSION,
            workspace_id: Some(workspace_id),
        }
    }

    /// 身份不变量:Work ⟺ 有 workspace_id;非 Work ⟺ 无。违反 = 结构不一致。
    pub fn workspace_invariant_holds(&self) -> bool {
        matches!(
            (self.surface_kind, self.workspace_id.is_some()),
            (SurfaceKind::Work, true) | (SurfaceKind::Chat, false)
                | (SurfaceKind::Code, false)
                | (SurfaceKind::Cowork, false)
        )
    }
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

/// 结构化错误：全部 fail-closed 场景的机器可读原因。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum SurfaceError {
    /// 迁移标记已有效但该会话无 binding——绝不默认 Code。
    UnboundSurface { session_id: String },
    /// 迁移尚未完成，无法裁决该会话的层归属——调用方必须先跑迁移，
    /// 不得自行解释为 Code。
    MigrationRequired { session_id: String },
    /// sidecar 文件损坏 / 未知 surface_kind / 未知字段 / 字段缺失。
    CorruptBinding { session_id: String, reason: String },
    /// binding 来自未来版本（schema 或 policy 代号大于当前程序）——
    /// 旧程序不得按较旧规则运行新会话。
    UnsupportedBindingVersion {
        session_id: String,
        binding_schema_version: u32,
        created_policy_version: u32,
    },
    /// 文件内 session_id 与请求方不符（哈希碰撞或文件被错放）。
    SessionIdMismatch { requested: String, embedded: String },
    /// 既有 binding 的 surface_kind 与写入请求不同——身份不可变。
    ImmutableKindConflict {
        session_id: String,
        existing: SurfaceKind,
        requested: SurfaceKind,
    },
    /// 既有 Work binding 的 workspace_id 与写入请求不同——工作区身份不可变
    /// (codex R1-F2:不许把已绑定会话悄悄重绑到别的工作区)。
    WorkspaceIdentityConflict {
        session_id: String,
        existing: String,
        requested: String,
    },
    /// 存储 IO 失败（新会话此态 = 可留存不可运行，等显式恢复/认领）。
    StoreIo { session_id: String, reason: String },
    /// 迁移未全部成功，完成标记未写。
    MigrationIncomplete { failed: Vec<String> },
    /// 另一进程/线程正持有迁移排他锁——稍后重试，不得绕过。
    MigrationLocked { reason: String },
    /// 迁移窗口已关闭，但本次快照中仍有无归属会话——既不回填（可能是
    /// 崩溃期新层会话）也不静默成功，交显式恢复/认领裁决。
    PostMarkerUnbound { session_ids: Vec<String> },
    /// 迁移标记存在但内容损坏/版本未知——不可信亦不可忽略。
    CorruptMigrationMarker { reason: String },
}

impl std::fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SurfaceError::UnboundSurface { session_id } => {
                write!(f, "unbound_surface: 会话 {session_id} 无层归属（迁移已完成，不默认 Code）")
            }
            SurfaceError::MigrationRequired { session_id } => {
                write!(f, "migration_required: 会话 {session_id} 归属待迁移裁决，先运行迁移")
            }
            SurfaceError::CorruptBinding { session_id, reason } => {
                write!(f, "corrupt_binding: 会话 {session_id} 的 sidecar 损坏：{reason}")
            }
            SurfaceError::UnsupportedBindingVersion {
                session_id,
                binding_schema_version,
                created_policy_version,
            } => write!(
                f,
                "unsupported_binding_version: 会话 {session_id} 的 binding 版本不受当前程序支持（schema {binding_schema_version}, policy {created_policy_version}）"
            ),
            SurfaceError::SessionIdMismatch { requested, embedded } => {
                write!(f, "session_id_mismatch: 请求 {requested} ≠ 文件内 {embedded}")
            }
            SurfaceError::ImmutableKindConflict { session_id, existing, requested } => write!(
                f,
                "immutable_kind_conflict: 会话 {session_id} 已归属 {existing:?}，拒绝改为 {requested:?}"
            ),
            SurfaceError::WorkspaceIdentityConflict { session_id, existing, requested } => write!(
                f,
                "workspace_identity_conflict: 会话 {session_id} 已绑定工作区 {existing}，拒绝改为 {requested}"
            ),
            SurfaceError::StoreIo { session_id, reason } => {
                write!(f, "store_io: 会话 {session_id} sidecar 写入失败：{reason}")
            }
            SurfaceError::MigrationIncomplete { failed } => {
                write!(f, "migration_incomplete: {} 个会话回填失败", failed.len())
            }
            SurfaceError::MigrationLocked { reason } => {
                write!(f, "migration_locked: 迁移排他锁被占用：{reason}")
            }
            SurfaceError::PostMarkerUnbound { session_ids } => write!(
                f,
                "post_marker_unbound: 迁移已完成但 {} 个会话无归属（不回填不吞错，需显式认领）",
                session_ids.len()
            ),
            SurfaceError::CorruptMigrationMarker { reason } => {
                write!(f, "corrupt_migration_marker: 迁移标记损坏或版本未知：{reason}")
            }
        }
    }
}

/// 进程内唯一序号：并发写同一 session 时临时文件互不相扰。
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

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

    /// 迁移完成标记状态：`Ok(true)` 有效存在 / `Ok(false)` 不存在 /
    /// `Err(CorruptMigrationMarker)` 存在但内容损坏或版本未知。
    /// 不用裸 `is_file()`——空文件、半写、未来版本的标记都不可信。
    pub fn migration_complete(&self) -> Result<bool, SurfaceError> {
        let text = match std::fs::read_to_string(self.marker_path()) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => {
                return Err(SurfaceError::CorruptMigrationMarker {
                    reason: format!("读取失败: {e}"),
                })
            }
        };
        if text.trim() == MARKER_CONTENT {
            Ok(true)
        } else {
            Err(SurfaceError::CorruptMigrationMarker {
                reason: format!("内容不符（读到 {:?}）", text.trim().chars().take(64).collect::<String>()),
            })
        }
    }

    /// 唯一临时文件（pid+进程内序号）→ 写入 → sync_all → no-clobber 发布
    /// （hard_link：目标已存在即失败，绝不覆盖既有文件）。
    fn publish_no_clobber(
        &self,
        session_id: &str,
        target: &PathBuf,
        content: &str,
    ) -> Result<bool, SurfaceError> {
        let io = |reason: String| SurfaceError::StoreIo {
            session_id: session_id.to_string(),
            reason,
        };
        std::fs::create_dir_all(&self.root).map_err(|e| io(format!("创建 sidecar 目录失败: {e}")))?;
        let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = target.with_extension(format!("tmp-{}-{seq}", std::process::id()));
        {
            let mut f = std::fs::File::create(&tmp).map_err(|e| io(format!("建临时文件失败: {e}")))?;
            f.write_all(content.as_bytes())
                .map_err(|e| io(format!("写临时文件失败: {e}")))?;
            // 崩溃窗口收口：发布前落盘。发布后 target 要么不存在、要么内容完整。
            f.sync_all().map_err(|e| io(format!("sync 失败: {e}")))?;
        }
        let published = match std::fs::hard_link(&tmp, target) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
            // 目标被目录占位等异常路径归 IO；调用方按可留存不可运行处理。
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(io(format!("no-clobber 发布失败: {e}")));
            }
        };
        let _ = std::fs::remove_file(&tmp);
        Ok(published)
    }

    /// 写一个 binding：no-clobber 首写 + 幂等重写（同 kind）。
    /// 并发首写同一 session：恰有一个发布成功，落败方按既有文件裁决——
    /// 同 kind 幂等 Ok，异 kind ImmutableKindConflict。
    ///
    /// 传入对象先验证再落盘（复核三）：写入永远发生在「现在」，版本字段
    /// 必须等于当前程序常量；空 session_id 拒绝。垃圾进不了磁盘，而不是
    /// 靠下次读取才发现。
    pub fn write(&self, binding: &SurfaceBinding) -> Result<(), SurfaceError> {
        if binding.session_id.is_empty() {
            return Err(SurfaceError::CorruptBinding {
                session_id: String::new(),
                reason: "session_id 为空".into(),
            });
        }
        if binding.binding_schema_version != CURRENT_BINDING_SCHEMA_VERSION
            || binding.created_policy_version != CURRENT_POLICY_VERSION
        {
            return Err(SurfaceError::UnsupportedBindingVersion {
                session_id: binding.session_id.clone(),
                binding_schema_version: binding.binding_schema_version,
                created_policy_version: binding.created_policy_version,
            });
        }
        // 写前也强制身份不变量(codex R1-F1):不允许把 Work+None 或
        // 非 Work+Some 这种不一致身份落盘。
        if !binding.workspace_invariant_holds() {
            return Err(SurfaceError::CorruptBinding {
                session_id: binding.session_id.clone(),
                reason: "写入的 binding 违反 Work⟺workspace_id 身份不变量".into(),
            });
        }
        let judge_existing = |existing: SurfaceBinding| {
            // 完整不可变身份比对(codex R1-F2):kind **与 workspace_id** 都须一致,
            // 否则重绑到别的工作区会假报成功。
            if existing.surface_kind != binding.surface_kind {
                Err(SurfaceError::ImmutableKindConflict {
                    session_id: binding.session_id.clone(),
                    existing: existing.surface_kind,
                    requested: binding.surface_kind,
                })
            } else if existing.workspace_id != binding.workspace_id {
                Err(SurfaceError::WorkspaceIdentityConflict {
                    session_id: binding.session_id.clone(),
                    existing: existing
                        .workspace_id
                        .map(|w| w.as_str().to_string())
                        .unwrap_or_default(),
                    requested: binding
                        .workspace_id
                        .clone()
                        .map(|w| w.as_str().to_string())
                        .unwrap_or_default(),
                })
            } else {
                Ok(())
            }
        };
        // 快路径：已有文件直接裁决（读错误如实上抛）。
        if let Some(existing) = self.try_read_raw(&binding.session_id)? {
            return judge_existing(existing);
        }
        let content = serde_json::to_string_pretty(binding).map_err(|e| SurfaceError::StoreIo {
            session_id: binding.session_id.clone(),
            reason: format!("序列化失败: {e}"),
        })?;
        let target = self.path_for(&binding.session_id);
        if self.publish_no_clobber(&binding.session_id, &target, &content)? {
            return Ok(());
        }
        // 发布被抢先：按赢家文件裁决。
        match self.try_read_raw(&binding.session_id)? {
            Some(existing) => judge_existing(existing),
            None => Err(SurfaceError::StoreIo {
                session_id: binding.session_id.clone(),
                reason: "发布竞争后文件不可读".into(),
            }),
        }
    }

    /// 读原始文件（存在性 + 解析 + 版本门 + 交叉校验），不涉及迁移语义。
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
        // 两阶段解析（终核 P1）：非当前 schema 的文件形状未知——先用
        // 宽容探针只取版本字段过版本门，否则「未来 schema + 新字段」会
        // 在 deny_unknown_fields 全量解析里先炸成 CorruptBinding，把
        // 「版本不支持」误报成「损坏」。
        // 阶段一：宽容探针（未知字段放行；缺版本字段 = 不是本格式）。
        #[derive(Deserialize)]
        struct SchemaProbe {
            binding_schema_version: u32,
            #[serde(default)]
            created_policy_version: u32,
        }
        let probe: SchemaProbe =
            serde_json::from_str(&text).map_err(|e| SurfaceError::CorruptBinding {
                session_id: session_id.to_string(),
                reason: format!("版本探针解析失败: {e}"),
            })?;
        // 版本门(W2-c 修订):schema 显式接受 [OLDEST..=CURRENT]——v1 是
        // Chat/Code 时代的已知旧格式(无 workspace_id),v2 当前。**过去的
        // 已知版本不再一律阻塞**,因为 v1→v2 是纯附加(v1 语义 = workspace_id
        // None),读作 None 是精确而非有损兼容,这是显式处理不是静默。未来
        // (> CURRENT)仍阻塞;更旧(< OLDEST)也阻塞。policy 代号仍仅未来阻塞。
        // 关键(codex R1-F3):旧的 schema-1 二进制读到 v2 文件时,其严格
        // `!= 1` 探针门会先报 UnsupportedBindingVersion(而非 deny_unknown_fields
        // 的 CorruptBinding)——两阶段探针在旧读者上把降级场景归类正确。
        if probe.binding_schema_version > CURRENT_BINDING_SCHEMA_VERSION
            || probe.binding_schema_version < OLDEST_READABLE_BINDING_SCHEMA_VERSION
            || probe.created_policy_version > CURRENT_POLICY_VERSION
        {
            return Err(SurfaceError::UnsupportedBindingVersion {
                session_id: session_id.to_string(),
                binding_schema_version: probe.binding_schema_version,
                created_policy_version: probe.created_policy_version,
            });
        }
        // 阶段二:严格全量解析。v1/v2 都能解进 v2 结构体(workspace_id 有
        // serde default);v1 文件无该字段 → None。
        let mut binding: SurfaceBinding =
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
        // v1 legacy 显式规范化为 v2:v1 必是 Chat/Code + None;若 v1 文件竟带
        // workspace_id 或为 Work,即形状不一致 → CorruptBinding。
        if binding.binding_schema_version < CURRENT_BINDING_SCHEMA_VERSION {
            if binding.workspace_id.is_some() || binding.surface_kind == SurfaceKind::Work {
                return Err(SurfaceError::CorruptBinding {
                    session_id: session_id.to_string(),
                    reason: "v1 binding 不得携带 workspace_id 或为 Work 层".into(),
                });
            }
            binding.binding_schema_version = CURRENT_BINDING_SCHEMA_VERSION;
        }
        // 信任边界强制身份不变量(codex R1-F1):Work⟺Some / 非 Work⟺None,
        // 违反即 fail-closed,不把不一致的持久身份返给调用方。
        if !binding.workspace_invariant_holds() {
            return Err(SurfaceError::CorruptBinding {
                session_id: session_id.to_string(),
                reason: "binding 违反 Work⟺workspace_id 身份不变量".into(),
            });
        }
        Ok(Some(binding))
    }

    /// 恢复会话时解析层归属（契约表）：
    /// - binding 存在（版本门 + 校验过）→ 返回；
    /// - 缺失且迁移标记**有效** → `UnboundSurface`（含 sidecar 写失败后的
    ///   新会话：可留存不可运行，等显式恢复/认领）；
    /// - 缺失且标记**不存在** → `MigrationRequired`，调用方先跑迁移，
    ///   不得自行解释为 Code；
    /// - 标记损坏 → `CorruptMigrationMarker` 上抛。
    pub fn resolve(&self, session_id: &str) -> Result<SurfaceBinding, SurfaceError> {
        match self.try_read_raw(session_id)? {
            Some(b) => Ok(b),
            None => {
                if self.migration_complete()? {
                    Err(SurfaceError::UnboundSurface {
                        session_id: session_id.to_string(),
                    })
                } else {
                    Err(SurfaceError::MigrationRequired {
                        session_id: session_id.to_string(),
                    })
                }
            }
        }
    }

    /// 迁移排他锁：`migration.lock` 以 Windows 独占共享模式（share_mode 0）
    /// 打开，句柄存活期间任何其他进程/线程都打不开——进程崩溃即释放，
    /// 无陈旧锁问题。占用中返回结构化 MigrationLocked（调用方稍后重试）。
    /// 目的（复核 P0）：封死「A/B 同时通过标记缺失检查，A 发布标记后
    /// B 的回填失败被后续 no-op 吞掉」的交错——标记检查与发布全程在锁内。
    fn acquire_migration_lock(&self) -> Result<std::fs::File, SurfaceError> {
        std::fs::create_dir_all(&self.root).map_err(|e| SurfaceError::StoreIo {
            session_id: String::new(),
            reason: format!("创建 sidecar 目录失败: {e}"),
        })?;
        let path = self.root.join("migration.lock");
        let mut opts = std::fs::OpenOptions::new();
        opts.read(true).write(true).create(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            opts.share_mode(0); // 独占：他人 open 即 ERROR_SHARING_VIOLATION
        }
        opts.open(&path).map_err(|e| {
            // Windows 共享冲突 = 锁被占（raw 32）；其余按 IO 上报。
            if e.raw_os_error() == Some(32) {
                SurfaceError::MigrationLocked {
                    reason: "另一迁移正在进行".into(),
                }
            } else {
                SurfaceError::StoreIo {
                    session_id: String::new(),
                    reason: format!("迁移锁获取失败: {e}"),
                }
            }
        })
    }

    /// 首次升级迁移：把现存会话幂等回填为 Code；**全部成功**才发布完成
    /// 标记（有内容、no-clobber），任一失败返回 MigrationIncomplete。
    /// **标记已有效时为 no-op**——迁移窗口已关闭，绝不回填新 ID（否则
    /// 崩溃期的新层会话会被错误提升为 Code）。标记损坏则上抛。
    ///
    /// 全程持迁移排他锁（复核 P0）：标记复查、回填、标记发布是一个
    /// 排他临界区，两个迁移不可能交错；锁被占返回 MigrationLocked。
    /// 胜者快照之外的 legacy 会话（理论上不应存在——快照后新建的会话
    /// 自带 binding）最终表现为可见的 unbound_surface，走显式恢复/认领，
    /// 永不静默升 Code。
    /// 会话枚举由调用方提供（App 层扫 ~/.grok sessions），本层不做 IO 发现。
    pub fn migrate_legacy<'a>(
        &self,
        existing_session_ids: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), SurfaceError> {
        let _lock = self.acquire_migration_lock()?;
        if self.migration_complete()? {
            // 窗口已关闭：绝不回填任何 ID，但也绝不静默成功——快照内仍
            // 无归属的会话（真 legacy 被早先快照漏掉，或崩溃期新层会话）
            // 结构化上报，交显式恢复/认领裁决（复核三：不允许「b1 被
            // 漏掉后仍报成功」的终态）。
            let mut unbound = Vec::new();
            for sid in existing_session_ids {
                // 此处只判定「有无有效归属」：缺失、损坏、读不出的一律
                // 进清单（具体病因由后续 resolve/认领路径给出），本检查
                // 不因单个坏文件中断整体上报。
                match self.try_read_raw(sid) {
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => unbound.push(sid.to_string()),
                }
            }
            if unbound.is_empty() {
                return Ok(()); // 全部已有归属：幂等重跑。
            }
            return Err(SurfaceError::PostMarkerUnbound { session_ids: unbound });
        }
        let mut failed = Vec::new();
        for sid in existing_session_ids {
            match self.write(&SurfaceBinding::new(sid, SurfaceKind::Code)) {
                Ok(()) => {}
                // 已有非 Code binding 的会话不是 legacy，跳过即幂等安全。
                Err(SurfaceError::ImmutableKindConflict { .. }) => {}
                Err(_) => failed.push(sid.to_string()),
            }
        }
        if !failed.is_empty() {
            return Err(SurfaceError::MigrationIncomplete { failed });
        }
        // 标记发布：同一套唯一临时文件 + sync + no-clobber。持锁下无并发
        // 发布者，若仍撞到已存在文件说明状态异常——回读校验兜底。
        self.publish_no_clobber("<marker>", &self.marker_path(), MARKER_CONTENT)?;
        // 发布后回读校验：确认落盘的标记确实有效（内容/版本匹配），
        // 不把「发布调用返回」当成「标记有效」。
        if !self.migration_complete()? {
            return Err(SurfaceError::StoreIo {
                session_id: String::new(),
                reason: "标记发布后回读缺失".into(),
            });
        }
        Ok(())
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
        // W2-c:Work 必须携带 workspace_id(身份不变量),其余走普通构造。
        if kind == SurfaceKind::Work {
            SurfaceBinding::new_work(sid, crate::work_staging::WorkspaceId::mint())
        } else {
            SurfaceBinding::new(sid, kind)
        }
    }

    // W2-c:Chat/Code 绑定序列化**不含** workspace_id 字段(与旧格式逐字节兼容)。
    #[test]
    fn chat_code_binding_omits_workspace_id_field() {
        for kind in [SurfaceKind::Chat, SurfaceKind::Code] {
            let json = serde_json::to_string(&SurfaceBinding::new("s", kind)).unwrap();
            assert!(
                !json.contains("workspace_id"),
                "{kind:?} 绑定不得含 workspace_id 字段: {json}"
            );
        }
    }

    // W2-c:旧格式文件(无 workspace_id 字段)仍能反序列化(default None)。
    #[test]
    fn legacy_binding_without_workspace_id_deserializes() {
        let legacy = r#"{"binding_schema_version":1,"session_id":"old","surface_kind":"code","created_policy_version":1}"#;
        let b: SurfaceBinding = serde_json::from_str(legacy).unwrap();
        assert_eq!(b.workspace_id, None);
        assert_eq!(b.surface_kind, SurfaceKind::Code);
    }

    // W2-c:Work 绑定携带 workspace_id 且 round-trip;身份不变量成立。
    #[test]
    fn work_binding_carries_workspace_id_and_round_trips() {
        let ws = crate::work_staging::WorkspaceId::mint();
        let b = SurfaceBinding::new_work("ws-sess", ws.clone());
        assert_eq!(b.surface_kind, SurfaceKind::Work);
        assert_eq!(b.workspace_id, Some(ws));
        assert!(b.workspace_invariant_holds());
        let json = serde_json::to_string(&b).unwrap();
        assert!(json.contains("workspace_id"));
        let back: SurfaceBinding = serde_json::from_str(&json).unwrap();
        assert_eq!(b, back);
    }

    // W2-c:身份不变量——Work 无 id / 非 Work 有 id 都算违反。
    #[test]
    fn workspace_invariant_rejects_mismatched_shapes() {
        // Work 但无 workspace_id
        let bad1 = SurfaceBinding::new("s", SurfaceKind::Work);
        assert!(!bad1.workspace_invariant_holds());
        // Code 但有 workspace_id(手工构造违反)
        let mut bad2 = SurfaceBinding::new("s", SurfaceKind::Code);
        bad2.workspace_id = Some(crate::work_staging::WorkspaceId::mint());
        assert!(!bad2.workspace_invariant_holds());
    }

    // W2-c:带 workspace_id 的**篡改** Work 文件里,逃逸 id 被 WorkspaceId 的
    // 严格 Deserialize 拒绝(与 W2-a 同源防线,不因进了 binding 而失效)。
    #[test]
    fn tampered_workspace_id_in_binding_is_rejected() {
        let evil = r#"{"binding_schema_version":2,"session_id":"s","surface_kind":"work","created_policy_version":1,"workspace_id":"ws-../../escape"}"#;
        assert!(serde_json::from_str::<SurfaceBinding>(evil).is_err());
    }

    // W2-c F1:store 在**信任边界**强制身份不变量——Work+None / 非 Work+Some
    // 都不能落盘也不能解析返回。
    #[test]
    fn store_rejects_invariant_violating_bindings() {
        let (_g, s) = store();
        // Work + None(手工构造违反)→ write 拒。
        let bad_work = SurfaceBinding::new("bw", SurfaceKind::Work);
        assert!(matches!(s.write(&bad_work), Err(SurfaceError::CorruptBinding { .. })));
        // Code + Some(手工构造违反)→ write 拒。
        let mut bad_code = SurfaceBinding::new("bc", SurfaceKind::Code);
        bad_code.workspace_id = Some(crate::work_staging::WorkspaceId::mint());
        assert!(matches!(s.write(&bad_code), Err(SurfaceError::CorruptBinding { .. })));
        // 直接把违反不变量的文件写到盘上,resolve 也 fail-closed。
        std::fs::create_dir_all(&s.root).unwrap();
        std::fs::write(
            s.path_for("disk-bad"),
            r#"{"binding_schema_version":2,"session_id":"disk-bad","surface_kind":"work","created_policy_version":1}"#,
        )
        .unwrap();
        assert!(matches!(s.resolve("disk-bad"), Err(SurfaceError::CorruptBinding { .. })));
    }

    // W2-c F2:已绑定某工作区的 Work 会话,重绑到**别的**工作区必须显式冲突,
    // 而非假报成功。
    #[test]
    fn rebind_to_different_workspace_conflicts() {
        let (_g, s) = store();
        let ws_a = crate::work_staging::WorkspaceId::mint();
        let ws_b = crate::work_staging::WorkspaceId::mint();
        s.write(&SurfaceBinding::new_work("s", ws_a.clone())).unwrap();
        // 同工作区重写 = 幂等 Ok。
        s.write(&SurfaceBinding::new_work("s", ws_a)).unwrap();
        // 换工作区 = WorkspaceIdentityConflict。
        assert!(matches!(
            s.write(&SurfaceBinding::new_work("s", ws_b)),
            Err(SurfaceError::WorkspaceIdentityConflict { .. })
        ));
    }

    // W2-c F3:旧 v1 文件(无 workspace_id 字段)被**显式接受**为 Chat/Code,
    // 规范化为当前 schema,workspace_id=None——不误报 Corrupt/UnsupportedVersion。
    #[test]
    fn legacy_v1_file_accepted_as_none() {
        let (_g, s) = store();
        std::fs::create_dir_all(&s.root).unwrap();
        std::fs::write(
            s.path_for("legacy"),
            r#"{"binding_schema_version":1,"session_id":"legacy","surface_kind":"code","created_policy_version":1}"#,
        )
        .unwrap();
        let b = s.resolve("legacy").unwrap();
        assert_eq!(b.surface_kind, SurfaceKind::Code);
        assert_eq!(b.workspace_id, None);
        assert_eq!(b.binding_schema_version, CURRENT_BINDING_SCHEMA_VERSION);
    }

    // W2-c F3:v1 文件若竟带 workspace_id 或为 Work(形状不一致)→ CorruptBinding。
    #[test]
    fn legacy_v1_with_workspace_or_work_is_corrupt() {
        let (_g, s) = store();
        std::fs::create_dir_all(&s.root).unwrap();
        std::fs::write(
            s.path_for("v1work"),
            r#"{"binding_schema_version":1,"session_id":"v1work","surface_kind":"work","created_policy_version":1}"#,
        )
        .unwrap();
        assert!(matches!(s.resolve("v1work"), Err(SurfaceError::CorruptBinding { .. })));
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
            let back = s.resolve(&sid).expect("resolve");
            assert_eq!(back.surface_kind, kind);
            assert_eq!(back.session_id, sid);
            assert_eq!(back.binding_schema_version, CURRENT_BINDING_SCHEMA_VERSION);
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
            r#"{"binding_schema_version":1,"session_id":"sess-y","surface_kind":"root","created_policy_version":1}"#,
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
        s.write(&binding("sess-a", SurfaceKind::Code)).unwrap();
        std::fs::write(
            s.path_for("sess-a"),
            serde_json::to_string(&binding("sess-b", SurfaceKind::Code)).unwrap(),
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

    // 复核-6a：未知字段 → CorruptBinding（deny_unknown_fields）。
    #[test]
    fn unknown_field_blocks_structured() {
        let (_g, s) = store();
        s.write(&binding("sess-z", SurfaceKind::Code)).unwrap();
        std::fs::write(
            s.path_for("sess-z"),
            r#"{"binding_schema_version":1,"session_id":"sess-z","surface_kind":"code","created_policy_version":1,"granted_tools":["shell"]}"#,
        )
        .unwrap();
        assert!(matches!(
            s.resolve("sess-z"),
            Err(SurfaceError::CorruptBinding { .. })
        ));
    }

    // 复核-6b：未来 binding schema → UnsupportedBindingVersion。
    #[test]
    fn future_schema_version_blocks() {
        let (_g, s) = store();
        s.write(&binding("fut-s", SurfaceKind::Chat)).unwrap();
        std::fs::write(
            s.path_for("fut-s"),
            r#"{"binding_schema_version":99,"session_id":"fut-s","surface_kind":"chat","created_policy_version":1}"#,
        )
        .unwrap();
        match s.resolve("fut-s") {
            Err(SurfaceError::UnsupportedBindingVersion { binding_schema_version, .. }) => {
                assert_eq!(binding_schema_version, 99)
            }
            other => panic!("期望 UnsupportedBindingVersion，得到 {other:?}"),
        }
    }

    // 复核-6c：未来 policy version → UnsupportedBindingVersion（旧程序
    // 不得按较旧规则运行新会话）。
    #[test]
    fn future_policy_version_blocks() {
        let (_g, s) = store();
        s.write(&binding("fut-p", SurfaceKind::Chat)).unwrap();
        std::fs::write(
            s.path_for("fut-p"),
            r#"{"binding_schema_version":1,"session_id":"fut-p","surface_kind":"chat","created_policy_version":99}"#,
        )
        .unwrap();
        assert!(matches!(
            s.resolve("fut-p"),
            Err(SurfaceError::UnsupportedBindingVersion { created_policy_version: 99, .. })
        ));
    }

    // RED-3：首次迁移回填 Code，重复执行幂等（已归属会话不被改写）。
    #[test]
    fn legacy_migration_backfills_code_idempotently() {
        let (_g, s) = store();
        s.write(&binding("sess-chat", SurfaceKind::Chat)).unwrap();
        let legacy = ["old-1", "old-2", "sess-chat"];
        s.migrate_legacy(legacy.iter().copied()).expect("first run");
        assert!(s.migration_complete().unwrap());
        s.migrate_legacy(legacy.iter().copied()).expect("second run");
        assert_eq!(
            s.resolve("old-1").unwrap().surface_kind,
            SurfaceKind::Code
        );
        assert_eq!(
            s.resolve("sess-chat").unwrap().surface_kind,
            SurfaceKind::Chat,
            "迁移不得把已归属会话改写为 Code"
        );
    }

    // RED-4：任一回填失败 → MigrationIncomplete 且不写完成标记。
    #[test]
    fn migration_failure_blocks_marker() {
        let (_g, s) = store();
        let victim = "will-fail";
        std::fs::create_dir_all(s.path_for(victim)).unwrap();
        let err = s
            .migrate_legacy(["ok-1", victim, "ok-2"])
            .expect_err("必须失败");
        assert!(matches!(
            &err,
            SurfaceError::MigrationIncomplete { failed } if failed == &vec![victim.to_string()]
        ));
        assert!(!s.migration_complete().unwrap(), "任一失败不得写完成标记");
    }

    // 复核-4：部分失败后修复重试 → 幂等完成。
    #[test]
    fn migration_retry_after_partial_failure_completes() {
        let (_g, s) = store();
        let victim = "late-ok";
        std::fs::create_dir_all(s.path_for(victim)).unwrap();
        assert!(s.migrate_legacy(["ok-1", victim]).is_err());
        std::fs::remove_dir(s.path_for(victim)).unwrap(); // 修复故障
        s.migrate_legacy(["ok-1", victim]).expect("重试须成功");
        assert!(s.migration_complete().unwrap());
        assert_eq!(s.resolve("ok-1").unwrap().surface_kind, SurfaceKind::Code);
        assert_eq!(s.resolve(victim).unwrap().surface_kind, SurfaceKind::Code);
    }

    // RED-5：标记完成后缺 binding → UnboundSurface，绝不默认 Code。
    #[test]
    fn unbound_after_marker_blocks() {
        let (_g, s) = store();
        s.migrate_legacy(std::iter::empty()).unwrap();
        assert!(s.migration_complete().unwrap());
        match s.resolve("ghost") {
            Err(SurfaceError::UnboundSurface { session_id }) => assert_eq!(session_id, "ghost"),
            other => panic!("期望 UnboundSurface，得到 {other:?}"),
        }
    }

    // 复核-1（复核三修订）：标记完成后迁移绝不回填新 ID，且遇到快照内
    // 无归属会话时**不得静默成功**——结构化 PostMarkerUnbound 上报。
    #[test]
    fn migration_after_marker_reports_unbound_never_backfills() {
        let (_g, s) = store();
        s.migrate_legacy(std::iter::empty()).unwrap();
        // 「引擎建会话成功、sidecar 写入前崩溃」的新 Chat 会话，
        // 若启动时被当作 legacy 传给迁移——不回填、不吞错。
        match s.migrate_legacy(["crashed-chat"]) {
            Err(SurfaceError::PostMarkerUnbound { session_ids }) => {
                assert_eq!(session_ids, vec!["crashed-chat".to_string()]);
            }
            other => panic!("期望 PostMarkerUnbound，得到 {other:?}"),
        }
        match s.resolve("crashed-chat") {
            Err(SurfaceError::UnboundSurface { .. }) => {}
            other => panic!("标记后迁移不得回填新 ID，得到 {other:?}"),
        }
        // 快照内全部已有归属时才是幂等 Ok。
        s.write(&binding("bound", SurfaceKind::Code)).unwrap();
        s.migrate_legacy(["bound"]).expect("全绑定快照幂等成功");
    }

    // 复核-3：标记前缺 binding → MigrationRequired（不是 None、不是 Code）。
    #[test]
    fn missing_before_marker_requires_migration() {
        let (_g, s) = store();
        match s.resolve("early") {
            Err(SurfaceError::MigrationRequired { session_id }) => {
                assert_eq!(session_id, "early")
            }
            other => panic!("期望 MigrationRequired，得到 {other:?}"),
        }
    }

    // 复核-5：损坏/未知版本标记 → CorruptMigrationMarker 可见阻塞。
    #[test]
    fn corrupt_marker_blocks_visibly() {
        let (_g, s) = store();
        s.migrate_legacy(std::iter::empty()).unwrap();
        std::fs::write(s.root.join(MIGRATION_MARKER), "surface-binding-migration v99").unwrap();
        assert!(matches!(
            s.migration_complete(),
            Err(SurfaceError::CorruptMigrationMarker { .. })
        ));
        // resolve 与 migrate 都不得把损坏标记当有效或当不存在。
        assert!(matches!(
            s.resolve("anyone"),
            Err(SurfaceError::CorruptMigrationMarker { .. })
        ));
        assert!(matches!(
            s.migrate_legacy(["x"]),
            Err(SurfaceError::CorruptMigrationMarker { .. })
        ));
    }

    // RED-6：新会话创建后 sidecar 写失败 → 恢复不得升为 Code。
    #[test]
    fn sidecar_write_failure_never_promotes_to_code() {
        let (_g, s) = store();
        s.migrate_legacy(std::iter::empty()).unwrap();
        let sid = "chat-crashed";
        std::fs::create_dir_all(s.path_for(sid)).unwrap(); // 制造写失败
        assert!(matches!(
            s.write(&binding(sid, SurfaceKind::Chat)),
            Err(SurfaceError::StoreIo { .. })
        ));
        std::fs::remove_dir(s.path_for(sid)).unwrap(); // 崩溃后重启
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
                    s.write(&SurfaceBinding::new(sid.clone(), kind)).expect("并行写失败");
                }
            })
        };
        let a = mk("par-a".into(), SurfaceKind::Chat);
        let b = mk("par-b".into(), SurfaceKind::Code);
        a.join().unwrap();
        b.join().unwrap();
        assert_eq!(s.resolve("par-a").unwrap().surface_kind, SurfaceKind::Chat);
        assert_eq!(s.resolve("par-b").unwrap().surface_kind, SurfaceKind::Code);
    }

    // 复核-2：同 session 两种 kind 并发首写恰一个成功（no-clobber）。
    #[test]
    fn concurrent_first_write_same_session_single_winner() {
        for round in 0..20 {
            let (_g, s) = store();
            let root = s.root.clone();
            let sid = format!("race-{round}");
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let mk = |kind: SurfaceKind| {
                let root = root.clone();
                let sid = sid.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let s = SurfaceBindingStore::new(root);
                    barrier.wait();
                    s.write(&SurfaceBinding::new(sid, kind))
                })
            };
            let ha = mk(SurfaceKind::Chat);
            let hb = mk(SurfaceKind::Code);
            let a = ha.join().unwrap();
            let b = hb.join().unwrap();
            let oks = [&a, &b].iter().filter(|r| r.is_ok()).count();
            assert_eq!(oks, 1, "恰一个成功，实际 a={a:?} b={b:?}");
            let loser = if a.is_ok() { &b } else { &a };
            assert!(
                matches!(loser, Err(SurfaceError::ImmutableKindConflict { .. })),
                "落败方必须 ImmutableKindConflict，实际 {loser:?}"
            );
            // 磁盘上的 kind == 赢家的 kind。
            let winner_kind = if a.is_ok() { SurfaceKind::Chat } else { SurfaceKind::Code };
            assert_eq!(
                s.try_read_raw(&sid).unwrap().unwrap().surface_kind,
                winner_kind
            );
        }
    }

    // 复核二-P0a：迁移排他锁被占 → MigrationLocked，不得绕过。
    #[cfg(windows)]
    #[test]
    fn migration_lock_excludes_concurrent_holder() {
        let (_g, s) = store();
        std::fs::create_dir_all(&s.root).unwrap();
        // 手工独占持有锁文件，模拟另一进程的迁移进行中。
        let _held = {
            use std::os::windows::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .share_mode(0)
                .open(s.root.join("migration.lock"))
                .unwrap()
        };
        match s.migrate_legacy(["x"]) {
            Err(SurfaceError::MigrationLocked { .. }) => {}
            other => panic!("期望 MigrationLocked，得到 {other:?}"),
        }
        drop(_held);
        s.migrate_legacy(["x"]).expect("锁释放后须成功");
        assert!(s.migration_complete().unwrap());
    }

    // 复核二-P0b：不同枚举快照 + 一方失败的并发迁移——排他锁下两个迁移
    // 不可能交错：绝不出现「B 部分回填失败被 A 的标记静默吞掉且 B 上报
    // 成功」。合法终态只有两类，且失败方要么拿到 MigrationIncomplete、
    // 要么拿到的是标记后 no-op（其快照外会话表现为可见 unbound_surface）。
    #[test]
    fn concurrent_migrations_different_snapshots_one_failing() {
        for round in 0..10 {
            let (_g, s) = store();
            let victim = format!("victim-{round}");
            std::fs::create_dir_all(s.path_for(&victim)).unwrap(); // B 必失败项
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
            let run = |ids: Vec<String>| {
                let root = s.root.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let s = SurfaceBindingStore::new(root);
                    barrier.wait();
                    // 锁被占就重试——模拟真实调用方行为。
                    loop {
                        match s.migrate_legacy(ids.iter().map(|x| x.as_str())) {
                            Err(SurfaceError::MigrationLocked { .. }) => {
                                std::thread::yield_now();
                                continue;
                            }
                            other => return other,
                        }
                    }
                })
            };
            let ha = run(vec!["a1".into()]);
            let hb = run(vec!["b1".into(), victim.clone()]);
            let ra = ha.join().unwrap();
            let rb = hb.join().unwrap();
            // A 的快照无失败项且全在 A 自己跑内可绑定：必成功。
            assert!(ra.is_ok(), "A 必成功，得到 {ra:?}");
            assert!(s.migration_complete().unwrap());
            // 复核三：B **永远不得报成功**——它的快照里 victim 必然无归属
            // （b1 视交错可能已绑或未绑）。合法结果只有两种可见失败：
            // 实跑 MigrationIncomplete，或标记后 PostMarkerUnbound。
            match &rb {
                Err(SurfaceError::MigrationIncomplete { failed }) => {
                    assert_eq!(failed, &vec![victim.clone()]);
                }
                Err(SurfaceError::PostMarkerUnbound { session_ids }) => {
                    assert!(session_ids.contains(&victim), "victim 必在上报清单");
                }
                other => panic!("B 不得报成功/其他，得到 {other:?}"),
            }
            // 「b1 被漏掉但整体报成功」被禁止：若 b1 无归属，B 的错误
            // 必须点名 b1。
            let b1_bound = s.try_read_raw("b1").unwrap().is_some();
            if !b1_bound {
                match &rb {
                    Err(SurfaceError::PostMarkerUnbound { session_ids }) => {
                        assert!(session_ids.contains(&"b1".to_string()), "漏掉的 b1 必须被点名");
                    }
                    other => panic!("b1 未绑定时 B 必须 PostMarkerUnbound 点名，得到 {other:?}"),
                }
            }
            // 无论哪种交错：victim 绝不被升为 Code；结局是可见阻塞。
            std::fs::remove_dir(s.path_for(&victim)).unwrap();
            match s.resolve(&victim) {
                Err(SurfaceError::UnboundSurface { .. }) => {}
                other => panic!("victim 必须 unbound_surface，得到 {other:?}"),
            }
            // A 快照内的会话必已绑定。
            assert_eq!(s.resolve("a1").unwrap().surface_kind, SurfaceKind::Code);
        }
    }

    // 复核三-P1a：过去版本 schema 同样结构化阻塞——旧格式必须经显式
    // 迁移，不做静默兼容读取（schema 门为严格等值）。
    #[test]
    fn past_schema_version_blocks() {
        let (_g, s) = store();
        s.write(&binding("past", SurfaceKind::Code)).unwrap();
        std::fs::write(
            s.path_for("past"),
            r#"{"binding_schema_version":0,"session_id":"past","surface_kind":"work","created_policy_version":0}"#,
        )
        .unwrap();
        assert!(matches!(
            s.resolve("past"),
            Err(SurfaceError::UnsupportedBindingVersion { binding_schema_version: 0, .. })
        ));
    }

    // 复核三-P1b：write() 验证**传入对象**本身——版本字段必须等于当前
    // 程序常量、session_id 非空；垃圾对象拒收且不落盘。
    #[test]
    fn write_validates_argument_object() {
        let (_g, s) = store();
        // 未来 schema 的传入对象。
        let mut b1 = binding("arg-a", SurfaceKind::Chat);
        b1.binding_schema_version = 99;
        assert!(matches!(
            s.write(&b1),
            Err(SurfaceError::UnsupportedBindingVersion { binding_schema_version: 99, .. })
        ));
        assert!(!s.path_for("arg-a").exists(), "拒收对象不得落盘");
        // 非当前 policy 代号的传入对象（写入永远发生在「现在」）。
        let mut b2 = binding("arg-b", SurfaceKind::Chat);
        b2.created_policy_version = 0;
        assert!(matches!(
            s.write(&b2),
            Err(SurfaceError::UnsupportedBindingVersion { created_policy_version: 0, .. })
        ));
        assert!(!s.path_for("arg-b").exists());
        // 空 session_id。
        let b3 = SurfaceBinding::new("", SurfaceKind::Code);
        assert!(matches!(s.write(&b3), Err(SurfaceError::CorruptBinding { .. })));
        // 既有未来版本文件上写入也被版本门拒（读取路径的门）。
        s.write(&binding("fut-w", SurfaceKind::Chat)).unwrap();
        std::fs::write(
            s.path_for("fut-w"),
            r#"{"binding_schema_version":99,"session_id":"fut-w","surface_kind":"chat","created_policy_version":1}"#,
        )
        .unwrap();
        assert!(matches!(
            s.write(&binding("fut-w", SurfaceKind::Chat)),
            Err(SurfaceError::UnsupportedBindingVersion { .. })
        ));
    }

    // 终核-P1a：未来 schema + 未知字段 → 必须报版本不支持，不得误报损坏
    // （两阶段解析的判别性测试：单次严格解析会先炸 CorruptBinding）。
    #[test]
    fn future_schema_with_unknown_fields_reports_version_not_corrupt() {
        let (_g, s) = store();
        std::fs::create_dir_all(&s.root).unwrap();
        std::fs::write(
            s.path_for("fut-shape"),
            r#"{"binding_schema_version":3,"session_id":"fut-shape","surface_kind":"quantum","brand_new_field":{"nested":true}}"#,
        )
        .unwrap();
        match s.resolve("fut-shape") {
            Err(SurfaceError::UnsupportedBindingVersion { binding_schema_version: 3, .. }) => {}
            other => panic!("期望 UnsupportedBindingVersion(schema=3)，得到 {other:?}"),
        }
    }

    // 终核-P1b：缺版本字段 = 不是本格式 → 探针阶段即 CorruptBinding。
    #[test]
    fn missing_schema_field_is_corrupt() {
        let (_g, s) = store();
        std::fs::create_dir_all(&s.root).unwrap();
        std::fs::write(
            s.path_for("no-ver"),
            r#"{"session_id":"no-ver","surface_kind":"chat"}"#,
        )
        .unwrap();
        assert!(matches!(
            s.resolve("no-ver"),
            Err(SurfaceError::CorruptBinding { .. })
        ));
    }

    // RED-8：旧 created_policy_version 按当前规则派生，不能恢复旧权限。
    // （旧 binding 由历史版本程序写下——直接落盘模拟，不走 write()：
    // write() 的参数验证只收当前版本。）
    #[test]
    fn old_policy_version_derives_with_current_rules() {
        let (_g, s) = store();
        std::fs::create_dir_all(&s.root).unwrap();
        std::fs::write(
            s.path_for("vintage"),
            r#"{"binding_schema_version":1,"session_id":"vintage","surface_kind":"chat","created_policy_version":0}"#,
        )
        .unwrap();
        let back = s.resolve("vintage").unwrap();
        let policy = derive_effective_policy(back.surface_kind);
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
            s.write(&binding(evil, SurfaceKind::Code)).unwrap();
            assert!(p.is_file());
        }
    }

    // RED-附：身份不可变——同一会话换 kind 写入被拒，同 kind 幂等。
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
        s.write(&binding("fixed", SurfaceKind::Chat)).expect("幂等");
    }
}
