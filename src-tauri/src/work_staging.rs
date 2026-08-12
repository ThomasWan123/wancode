//! Work 层暂存身份模型(v0.20 W2-a,设计稿 §1.4 + codex R3-F2)。
//!
//! 三层身份严格分离,均为**严格语法校验的新类型**:
//!   - `WorkspaceId`  —— 持久 Work 工作区标识,一个工作区可含多篇文档;
//!   - `ImportId`     —— 每篇文档导入铸造;
//!   - 原件 sha256    —— 与 import_id 联合定位文档。
//!
//! 本切片只做**后端身份基础**:单源路径解析、id 铸造+严格校验、清单读写
//! (原子)、fail-closed 版本门。**不含**前端切换、导入命令、SurfaceBinding
//! 扩展(归 W2-b/c)。范围诚实收窄,不宣称 W2 完成。
//!
//! 安全不变量(codex R2/R3):id 语法**固定为铸造格式**,自定义 Deserialize
//! 统一走校验 —— 篡改清单放入 `..`/路径分隔符/绝对路径一律拒绝,解析结果
//! 恒在 Work 根内。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Deserializer, Serialize};

/// Work 暂存根目录名。唯一字面量出处(PR #38 F2)。
const WORK_DIR_NAME: &str = "work";

/// 清单格式版本。加字段必须 bump,旧版本读到未知 schema 一律阻塞。
pub const CURRENT_MANIFEST_SCHEMA_VERSION: u32 = 1;

pub fn work_root_under(app_data_dir: PathBuf) -> PathBuf {
    app_data_dir.join(WORK_DIR_NAME)
}
pub fn workspace_dir_under(app_data_dir: PathBuf, ws: &WorkspaceId) -> PathBuf {
    work_root_under(app_data_dir).join(ws.as_str())
}
pub fn manifest_path_under(app_data_dir: PathBuf, ws: &WorkspaceId) -> PathBuf {
    workspace_dir_under(app_data_dir, ws).join("manifest.json")
}

// ── id 铸造与严格校验 ────────────────────────────────────────────────
//
// 铸造格式(两类 id 同构):`<prefix>-<ms:012x>-<seq:06x>-<pid:08x>`
// (seq 在 pid 之前:同机同毫秒字典序由铸造顺序决定,见 format_id)。
// 严格校验保证 id 里只可能出现 [0-9a-f-] 与固定前缀,绝无路径分隔符/`.`。

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 纯函数:把三段拼成固定形状 id。**seq 掩到 24 位**(codex R2-P2:
/// `{:06x}` 是最小宽度,计数到 0x1000000 会溢出 7 位使自己的 parser 拒绝)。
/// 掩码后 seq 永远 6 位 hex,与校验器精确对应;24 位 = 每进程 1670 万个/毫秒
/// 循环窗口,配合 ms 时间戳足够抗碰撞。
fn format_id(prefix: &str, ms: u64, seq_raw: u64, pid: u64) -> String {
    let ms = ms & 0xffff_ffff_ffff; // 48 位,对应 {:012x}
    let seq = seq_raw & 0xff_ffff; // 24 位,对应 {:06x}
    let pid = pid & 0xffff_ffff; // 32 位,对应 {:08x}
    format!("{prefix}-{ms:012x}-{seq:06x}-{pid:08x}")
}

fn mint_id(prefix: &str) -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    // seq 在 pid 之前:同机同毫秒的字典序由铸造顺序(而非 pid)决定。
    let seq = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id() as u64;
    format_id(prefix, ms, seq, pid)
}

/// 校验 `<prefix>-<12hex>-<6hex>-<8hex>` 精确形状。任何越界字符/长度即拒。
fn validate_id(s: &str, prefix: &str) -> bool {
    let rest = match s.strip_prefix(prefix).and_then(|r| r.strip_prefix('-')) {
        Some(r) => r,
        None => return false,
    };
    let parts: Vec<&str> = rest.split('-').collect();
    if parts.len() != 3 {
        return false;
    }
    let (a, b, c) = (parts[0], parts[1], parts[2]);
    a.len() == 12
        && b.len() == 6
        && c.len() == 8
        && [a, b, c]
            .iter()
            .all(|p| p.chars().all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase()))
}

macro_rules! strict_id_type {
    ($name:ident, $prefix:literal, $err:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn mint() -> Self {
                Self(mint_id($prefix))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
            /// 从字符串重建(读盘/前端回传),**严格校验固定语法**。
            pub fn parse(s: impl Into<String>) -> Result<Self, WorkStagingError> {
                let s = s.into();
                if validate_id(&s, $prefix) {
                    Ok(Self(s))
                } else {
                    Err(WorkStagingError::$err(s))
                }
            }
        }

        // 自定义 Deserialize:反序列化路径也强制走校验(codex R3-F2:
        // #[serde(transparent)] 的 derive Deserialize 会绕过校验)。
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                $name::parse(s).map_err(serde::de::Error::custom)
            }
        }
    };
}

strict_id_type!(WorkspaceId, "ws", InvalidWorkspaceId);
strict_id_type!(ImportId, "imp", InvalidImportId);

// ── 清单 ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportRecord {
    pub import_id: ImportId,
    /// 完整原件 sha256(不截断),64 位小写 hex。
    pub source_sha256: String,
    pub display_name: String,
    /// 暂存副本相对工作区目录的路径(由生产导入逻辑构造,W2-b)。
    pub staging_rel_path: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkManifest {
    pub manifest_schema_version: u32,
    pub workspace_id: WorkspaceId,
    pub imports: Vec<ImportRecord>,
}

impl WorkManifest {
    pub fn new(workspace_id: WorkspaceId) -> Self {
        Self {
            manifest_schema_version: CURRENT_MANIFEST_SCHEMA_VERSION,
            workspace_id,
            imports: Vec::new(),
        }
    }

    pub fn to_json(&self) -> Result<String, WorkStagingError> {
        serde_json::to_string_pretty(self).map_err(|e| WorkStagingError::Serialize(e.to_string()))
    }

    /// 两阶段:先宽容探测版本,再严格全量解析(未来版本阻塞而非误报损坏)。
    pub fn from_json(s: &str) -> Result<Self, WorkStagingError> {
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

    /// 原子写盘:唯一临时文件 → flush + sync_all → 覆盖式 rename → sync 父目录。
    /// 唯一临时名避免并发写者互踩;失败清理临时文件(codex R3-F3)。
    pub fn write_atomic(&self, manifest_path: &Path) -> Result<(), WorkStagingError> {
        use std::io::Write;
        let json = self.to_json()?;
        let parent = manifest_path
            .parent()
            .ok_or_else(|| WorkStagingError::Io("manifest 无父目录".into()))?;
        std::fs::create_dir_all(parent).map_err(|e| WorkStagingError::Io(e.to_string()))?;

        // 唯一临时名:pid + 单调计数,避免并发写者共用同一 .tmp。
        let uniq = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = parent.join(format!(".manifest.{}.{}.tmp", std::process::id(), uniq));

        let write_and_sync = |tmp: &Path| -> std::io::Result<()> {
            let mut f = std::fs::File::create(tmp)?;
            f.write_all(json.as_bytes())?;
            f.flush()?;
            f.sync_all()?; // 数据落盘,而非仅进页缓存
            Ok(())
        };
        if let Err(e) = write_and_sync(&tmp) {
            let _ = std::fs::remove_file(&tmp);
            return Err(WorkStagingError::Io(e.to_string()));
        }
        // std::fs::rename 在 Windows 上走 MOVEFILE_REPLACE_EXISTING,可覆盖已存在目标。
        if let Err(e) = std::fs::rename(&tmp, manifest_path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(WorkStagingError::Io(e.to_string()));
        }
        // 尽力同步父目录(部分平台使 rename 持久;Windows 无对应语义时忽略错误)。
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    }

    pub fn read(manifest_path: &Path) -> Result<Self, WorkStagingError> {
        let s = std::fs::read_to_string(manifest_path)
            .map_err(|e| WorkStagingError::Io(e.to_string()))?;
        Self::from_json(&s)
    }
}

#[derive(Deserialize)]
struct SchemaProbe {
    manifest_schema_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkStagingError {
    InvalidWorkspaceId(String),
    InvalidImportId(String),
    UnsupportedSchema { found: u32, supported: u32 },
    Corrupt(String),
    Serialize(String),
    Io(String),
}

impl std::fmt::Display for WorkStagingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkStagingError::InvalidWorkspaceId(s) => write!(f, "非法 workspace_id: {s}"),
            WorkStagingError::InvalidImportId(s) => write!(f, "非法 import_id: {s}"),
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
        assert_eq!(work_root_under(base.clone()), base.join("work"));
        let ws = WorkspaceId::mint();
        assert_eq!(
            workspace_dir_under(base.clone(), &ws),
            base.join("work").join(ws.as_str())
        );
    }

    #[test]
    fn minted_ids_are_unique_valid_and_typed() {
        let a = WorkspaceId::mint();
        let b = WorkspaceId::mint();
        assert_ne!(a, b);
        assert!(WorkspaceId::parse(a.as_str()).is_ok());
        let i = ImportId::mint();
        assert!(ImportId::parse(i.as_str()).is_ok());
        // 两层身份类型隔离:workspace id 不是合法 import id,反之亦然。
        assert!(ImportId::parse(a.as_str()).is_err());
        assert!(WorkspaceId::parse(i.as_str()).is_err());
    }

    #[test]
    fn ids_reject_path_escape_and_bad_shape() {
        // codex R3-F2 反向用例:路径逃逸物一律拒绝。
        for bad in [
            "ws-../../etc",
            "ws-/abs/path",
            r"ws-..\..\host",
            "ws-",
            "ws-xyz",              // 非 hex
            "ws-000000000000-000000-0000000",   // 段长错(8→7)
            "ws-000000000000-000000-00000000-x", // 多段
            "C:/work/ws-x",
            "nope",
        ] {
            assert!(WorkspaceId::parse(bad).is_err(), "应拒: {bad}");
        }
        // 合法铸造格式接受。
        assert!(WorkspaceId::parse("ws-000000000000-000000-00000000").is_ok());
    }

    #[test]
    fn deserialize_enforces_id_validation() {
        // #[serde(transparent)] 的默认 Deserialize 会绕过校验 —— 自定义实现堵死。
        let evil = r#"{"manifest_schema_version":1,"workspace_id":"ws-../../escape","imports":[]}"#;
        assert!(matches!(
            WorkManifest::from_json(evil),
            Err(WorkStagingError::Corrupt(_)) // 校验失败经 serde custom error 传出
        ));
        // import_id 逃逸同样被拒。
        let ws = WorkspaceId::mint();
        let evil2 = format!(
            r#"{{"manifest_schema_version":1,"workspace_id":"{}","imports":[{{"import_id":"imp-/../x","source_sha256":"{}","display_name":"x","staging_rel_path":"x","kind":"pdf"}}]}}"#,
            ws.as_str(), "a".repeat(64)
        );
        assert!(WorkManifest::from_json(&evil2).is_err());
    }

    #[test]
    fn seq_overflow_still_produces_valid_ids() {
        // codex R2-P2:计数器越过 0xffffff 后掩码回绕,id 仍是合法 6 位段。
        for seq in [0x00_0000u64, 0xff_ffff, 0x100_0000, 0x100_0001, u64::MAX] {
            let ws = format_id("ws", 1, seq, 7);
            assert!(WorkspaceId::parse(&ws).is_ok(), "seq={seq:#x} 产出非法 id: {ws}");
            let imp = format_id("imp", 1, seq, 7);
            assert!(ImportId::parse(&imp).is_ok(), "seq={seq:#x} 产出非法 import id: {imp}");
        }
        // 掩码前后不同 seq 在同一 6 位窗口内映射一致(0x1000000 回绕到 0)。
        assert_eq!(format_id("ws", 1, 0x100_0000, 7), format_id("ws", 1, 0, 7));
    }

    #[test]
    fn manifest_round_trips_through_json() {
        let mut m = WorkManifest::new(WorkspaceId::mint());
        m.imports.push(ImportRecord {
            import_id: ImportId::mint(),
            source_sha256: "a".repeat(64),
            display_name: "报告.pdf".into(),
            staging_rel_path: "imp-x/original.pdf".into(),
            kind: "pdf".into(),
        });
        let back = WorkManifest::from_json(&m.to_json().unwrap()).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn future_schema_version_is_blocked_not_corrupt() {
        let ws = WorkspaceId::mint();
        let future = format!(
            r#"{{"manifest_schema_version":2,"workspace_id":"{}","imports":[]}}"#,
            ws.as_str()
        );
        match WorkManifest::from_json(&future) {
            Err(WorkStagingError::UnsupportedSchema { found: 2, supported: 1 }) => {}
            other => panic!("期望 UnsupportedSchema,实得 {other:?}"),
        }
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let ws = WorkspaceId::mint();
        let bad = format!(
            r#"{{"manifest_schema_version":1,"workspace_id":"{}","imports":[],"evil":true}}"#,
            ws.as_str()
        );
        assert!(matches!(
            WorkManifest::from_json(&bad),
            Err(WorkStagingError::Corrupt(_))
        ));
    }

    #[test]
    fn atomic_write_survives_multiple_updates() {
        // codex R3-F3:连续两次写入现有 manifest.json,读回**第二份**状态,无残留。
        let dir = std::env::temp_dir().join(format!("w2a-{}-{}", std::process::id(),
            ID_COUNTER.fetch_add(1, Ordering::Relaxed)));
        let ws = WorkspaceId::mint();
        let mp = manifest_path_under(dir.clone(), &ws);

        let m1 = WorkManifest::new(ws.clone());
        m1.write_atomic(&mp).unwrap();
        // 断言覆盖语义:第二次写入前目标**已存在**(证明这是 replace 而非 create;
        // rust CI job = windows-latest,故此路径在 Windows 上被 CI 实测)。
        assert!(mp.exists(), "第二次写入前 manifest.json 必须已存在");

        let mut m2 = WorkManifest::new(ws);
        m2.imports.push(ImportRecord {
            import_id: ImportId::mint(),
            source_sha256: "b".repeat(64),
            display_name: "second.pdf".into(),
            staging_rel_path: "imp-y/original.pdf".into(),
            kind: "pdf".into(),
        });
        // 覆盖已存在目标(Windows rename 覆盖语义)。
        m2.write_atomic(&mp).unwrap();

        let back = WorkManifest::read(&mp).unwrap();
        assert_eq!(back, m2, "读回必须是第二份状态");
        assert_eq!(back.imports.len(), 1);

        // 无 .tmp 残留
        let leftover: Vec<_> = std::fs::read_dir(mp.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftover.is_empty(), "不得残留临时文件");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
