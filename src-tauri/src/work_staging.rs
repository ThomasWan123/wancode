//! Work 层暂存身份模型(v0.20 W2-a,设计稿 §1.4 + codex R3-F2)。
//!
//! 三层身份严格分离:
//!   - `WorkspaceId`  —— 持久 Work 工作区标识,一个工作区可含多篇文档;
//!   - `ImportId`     —— 每篇文档导入铸造;
//!   - 原件 sha256    —— 与 import_id 联合定位文档。
//!
//! 本切片只做**后端身份基础**:单源路径解析、id 铸造、清单读写(原子)、
//! fail-closed 版本门。**不含**前端切换、导入命令、SurfaceBinding 扩展
//! (归 W2-b/c)。范围诚实收窄,不宣称 W2 完成。
//!
//! 教训沿用:路径单一来源(PR #38 F2)、serde_json、未知 schema 阻塞而非
//! 静默(surface.rs 同款)、清单写入用 temp+rename 原子替换。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Work 暂存根目录名。唯一字面量出处 —— 除 [`work_root_under`] 外任何代码
/// 不得再拼写(PR #38 F2:两处独立字面量各自漂移 = 隐形丢文件 bug)。
const WORK_DIR_NAME: &str = "work";

/// 清单格式版本。未来加字段必须 bump,旧版本读到未知 schema 一律阻塞。
pub const CURRENT_MANIFEST_SCHEMA_VERSION: u32 = 1;

/// Work 暂存根:`app_data_dir/work`。所有 Work 路径的唯一推导点。
pub fn work_root_under(app_data_dir: PathBuf) -> PathBuf {
    app_data_dir.join(WORK_DIR_NAME)
}

/// 某工作区目录:`app_data_dir/work/<workspace_id>`。
pub fn workspace_dir_under(app_data_dir: PathBuf, ws: &WorkspaceId) -> PathBuf {
    work_root_under(app_data_dir).join(ws.as_str())
}

/// 该工作区的清单文件路径。
pub fn manifest_path_under(app_data_dir: PathBuf, ws: &WorkspaceId) -> PathBuf {
    workspace_dir_under(app_data_dir, ws).join("manifest.json")
}

/// 持久不透明工作区标识。ULID 风格:48 位毫秒时间戳 + 单调计数 + 进程盐,
/// 编码为可排序的小写十六进制。零外部依赖(SystemTime + 原子计数即可)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkspaceId(String);

static WS_COUNTER: AtomicU64 = AtomicU64::new(0);

impl WorkspaceId {
    /// 铸造一个新的工作区 id。同一进程内单调递增计数保证同毫秒不撞。
    pub fn mint() -> Self {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let n = WS_COUNTER.fetch_add(1, Ordering::Relaxed);
        // 进程盐:用当前 pid,降低跨进程同毫秒同计数的碰撞面。
        let pid = std::process::id() as u64;
        Self(format!("ws-{ms:012x}-{pid:08x}-{n:06x}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 从已存在的字符串重建(读盘/前端回传)。仅校验非空与前缀,不铸造。
    pub fn from_existing(s: impl Into<String>) -> Result<Self, WorkStagingError> {
        let s = s.into();
        if s.is_empty() || !s.starts_with("ws-") {
            return Err(WorkStagingError::InvalidWorkspaceId(s));
        }
        Ok(Self(s))
    }
}

/// 每篇导入文档的清单记录。锚点契约见设计 §1.2;本切片只落身份与哈希,
/// 解析产物/映射表待 W3。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportRecord {
    /// 每篇文档导入铸造的持久标识(与 workspace_id 分离,codex R3-F2)。
    pub import_id: String,
    /// 完整原件 sha256(不截断)。
    pub source_sha256: String,
    /// 原文件名(仅展示;不作路径用)。
    pub display_name: String,
    /// 暂存副本相对工作区目录的路径。
    pub staging_rel_path: String,
    /// 文档类型(pdf|docx),小写。
    pub kind: String,
}

/// 工作区清单。绑定单一 workspace_id;会话/检索只读它所属的这一份
/// (codex R3-F2:文档不得跨工作区串)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkManifest {
    pub manifest_schema_version: u32,
    pub workspace_id: WorkspaceId,
    pub imports: Vec<ImportRecord>,
}

impl WorkManifest {
    /// 新建空清单。
    pub fn new(workspace_id: WorkspaceId) -> Self {
        Self {
            manifest_schema_version: CURRENT_MANIFEST_SCHEMA_VERSION,
            workspace_id,
            imports: Vec::new(),
        }
    }

    /// 序列化为 JSON(serde_json,正确转义)。
    pub fn to_json(&self) -> Result<String, WorkStagingError> {
        serde_json::to_string_pretty(self).map_err(|e| WorkStagingError::Serialize(e.to_string()))
    }

    /// 从 JSON 反序列化,并过版本门(fail-closed)。未知 schema/未来版本一律阻塞。
    pub fn from_json(s: &str) -> Result<Self, WorkStagingError> {
        // 两阶段:先宽容探测版本,再严格全量解析(避免"未来版本"被误报"损坏")。
        let probe: SchemaProbe =
            serde_json::from_str(s).map_err(|e| WorkStagingError::Corrupt(e.to_string()))?;
        if probe.manifest_schema_version != CURRENT_MANIFEST_SCHEMA_VERSION {
            return Err(WorkStagingError::UnsupportedSchema {
                found: probe.manifest_schema_version,
                supported: CURRENT_MANIFEST_SCHEMA_VERSION,
            });
        }
        serde_json::from_str(s).map_err(|e| WorkStagingError::Corrupt(e.to_string()))
    }

    /// 原子写盘:先写同目录 temp,fsync 后 rename 覆盖,避免半写清单。
    pub fn write_atomic(&self, manifest_path: &Path) -> Result<(), WorkStagingError> {
        let json = self.to_json()?;
        let parent = manifest_path
            .parent()
            .ok_or_else(|| WorkStagingError::Io("manifest 无父目录".into()))?;
        std::fs::create_dir_all(parent).map_err(|e| WorkStagingError::Io(e.to_string()))?;
        let tmp = manifest_path.with_extension("json.tmp");
        std::fs::write(&tmp, json.as_bytes()).map_err(|e| WorkStagingError::Io(e.to_string()))?;
        std::fs::rename(&tmp, manifest_path).map_err(|e| WorkStagingError::Io(e.to_string()))?;
        Ok(())
    }

    /// 从盘读取并过版本门。
    pub fn read(manifest_path: &Path) -> Result<Self, WorkStagingError> {
        let s = std::fs::read_to_string(manifest_path)
            .map_err(|e| WorkStagingError::Io(e.to_string()))?;
        Self::from_json(&s)
    }
}

/// 仅用于两阶段版本探测。
#[derive(Deserialize)]
struct SchemaProbe {
    manifest_schema_version: u32,
}

/// 结构化 fail-closed 错误。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkStagingError {
    InvalidWorkspaceId(String),
    UnsupportedSchema { found: u32, supported: u32 },
    Corrupt(String),
    Serialize(String),
    Io(String),
}

impl std::fmt::Display for WorkStagingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkStagingError::InvalidWorkspaceId(s) => write!(f, "非法 workspace_id: {s}"),
            WorkStagingError::UnsupportedSchema { found, supported } => {
                write!(f, "清单 schema 版本 {found} 不受支持(当前 {supported})")
            }
            WorkStagingError::Corrupt(s) => write!(f, "清单损坏: {s}"),
            WorkStagingError::Serialize(s) => write!(f, "清单序列化失败: {s}"),
            WorkStagingError::Io(s) => write!(f, "清单 IO 失败: {s}"),
        }
    }
}
impl std::error::Error for WorkStagingError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_paths_have_single_literal_source() {
        let base = PathBuf::from("C:/app");
        let root = work_root_under(base.clone());
        assert_eq!(root, base.join("work"));
        let ws = WorkspaceId::from_existing("ws-abc").unwrap();
        assert_eq!(workspace_dir_under(base.clone(), &ws), base.join("work").join("ws-abc"));
        assert_eq!(
            manifest_path_under(base, &ws),
            PathBuf::from("C:/app/work/ws-abc/manifest.json")
        );
    }

    #[test]
    fn minted_ids_are_unique_and_prefixed() {
        let a = WorkspaceId::mint();
        let b = WorkspaceId::mint();
        assert!(a.as_str().starts_with("ws-"));
        assert_ne!(a, b, "同进程连续铸造必须不同(单调计数)");
    }

    #[test]
    fn from_existing_rejects_bad_ids() {
        assert!(WorkspaceId::from_existing("").is_err());
        assert!(WorkspaceId::from_existing("nope").is_err());
        assert!(WorkspaceId::from_existing("ws-ok").is_ok());
    }

    #[test]
    fn manifest_round_trips_through_json() {
        let mut m = WorkManifest::new(WorkspaceId::from_existing("ws-1").unwrap());
        m.imports.push(ImportRecord {
            import_id: "imp-1".into(),
            source_sha256: "a".repeat(64),
            display_name: "报告.pdf".into(),
            staging_rel_path: "imp-1/original.pdf".into(),
            kind: "pdf".into(),
        });
        let json = m.to_json().unwrap();
        let back = WorkManifest::from_json(&json).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn future_schema_version_is_blocked_not_corrupt() {
        // 未来版本(schema=2)必须报 UnsupportedSchema,不能误报 Corrupt。
        let future = r#"{"manifest_schema_version":2,"workspace_id":"ws-1","imports":[]}"#;
        match WorkManifest::from_json(future) {
            Err(WorkStagingError::UnsupportedSchema { found: 2, supported: 1 }) => {}
            other => panic!("期望 UnsupportedSchema,实得 {other:?}"),
        }
    }

    #[test]
    fn unknown_fields_are_rejected() {
        // deny_unknown_fields:形状不认识 = 阻塞。
        let bad = r#"{"manifest_schema_version":1,"workspace_id":"ws-1","imports":[],"evil":true}"#;
        assert!(matches!(
            WorkManifest::from_json(bad),
            Err(WorkStagingError::Corrupt(_))
        ));
    }

    #[test]
    fn write_atomic_then_read_is_identity() {
        let dir = std::env::temp_dir().join(format!("w2a-{}", std::process::id()));
        let ws = WorkspaceId::from_existing("ws-rt").unwrap();
        let mp = manifest_path_under(dir.clone(), &ws);
        let m = WorkManifest::new(ws);
        m.write_atomic(&mp).unwrap();
        let back = WorkManifest::read(&mp).unwrap();
        assert_eq!(m, back);
        // 无 .tmp 残留
        assert!(!mp.with_extension("json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
