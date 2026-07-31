//! #127-2 能力快照与双数据链适配器。
//!
//! "只读一次"的准确定义（复核定案）：**每个配置世代**只读取一次
//! wancode.toml 生成共享快照——不是整个 App 生命周期永不重读。
//! 世代切换点：启动加载、模型保存/删除、显式刷新。切换 = 原子替换
//! （RwLock<Arc<...>> 整体换新），读者拿到的永远是完整一致的一代。
//!
//! 两条 UI 数据链（设置页 model_list / 聊天目录 caps map）各有自己的
//! 适配器，但都**只接收 &CapabilitySnapshot**——适配器内部禁止读文件，
//! 这是双链不漂移的结构保证（v0.18.7-B 教训）。
//!
//! 文件级 fail-visible：文件不存在 = 空快照零诊断（无覆盖是常态）；
//! 读取失败 / TOML 语法错误必须形成文件级诊断，绝不退化为"空配置"。

use std::sync::{Arc, RwLock};

use serde::Serialize;

use crate::model_caps::{
    resolve_caps, wancode_config_path, CapIssue, CapOverrides, ModelCaps, ParsedWanCodeConfig,
};

/// 文件级问题：整份 wancode.toml 不可用（区别于条目级 CapIssue）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileIssueKind {
    /// 文件存在但读不出来（权限/IO）。
    ReadError,
    /// 内容不是合法 TOML。
    ParseError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileIssue {
    pub kind: FileIssueKind,
    pub message: String,
}

/// 一个配置世代的只读快照。适配器共享同一份，禁止各自读文件。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapabilitySnapshot {
    parsed: ParsedWanCodeConfig,
    pub file_issue: Option<FileIssue>,
}

impl CapabilitySnapshot {
    /// 文件缺失时的形态：空快照、零诊断。
    pub fn empty() -> CapabilitySnapshot {
        CapabilitySnapshot::default()
    }

    /// 从文件文本构建。语法错误 → 文件级诊断 + 空覆盖（可见地空，
    /// 而不是静默地空）。
    pub fn from_text(text: &str) -> CapabilitySnapshot {
        match text.parse::<toml_edit::DocumentMut>() {
            Ok(doc) => CapabilitySnapshot {
                parsed: ParsedWanCodeConfig::parse(&doc),
                file_issue: None,
            },
            Err(e) => CapabilitySnapshot {
                parsed: ParsedWanCodeConfig::default(),
                file_issue: Some(FileIssue {
                    kind: FileIssueKind::ParseError,
                    message: e.to_string(),
                }),
            },
        }
    }

    /// 从磁盘加载一代。缺失 → empty；读取失败 → ReadError 诊断。
    pub fn load(path: &std::path::Path) -> CapabilitySnapshot {
        match std::fs::read_to_string(path) {
            Ok(text) => CapabilitySnapshot::from_text(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => CapabilitySnapshot::empty(),
            Err(e) => CapabilitySnapshot {
                parsed: ParsedWanCodeConfig::default(),
                file_issue: Some(FileIssue {
                    kind: FileIssueKind::ReadError,
                    message: e.to_string(),
                }),
            },
        }
    }

    pub fn overrides_for(&self, catalog_key: &str) -> CapOverrides {
        self.parsed.for_model(catalog_key)
    }

    /// 全量条目/顶层诊断（带归属）。
    pub fn issues(&self) -> &[CapIssue] {
        &self.parsed.issues
    }
}

/// 世代持有者：启动加载一次；模型保存/显式刷新时 reload() 原子替换。
/// 路径构造期注入：生产 init() 用 wancode_config_path()，测试用临时路径
/// ——热加载测试因此能走**同一个**生产 reload()，不是复制其实现。
pub struct CapsState {
    path: std::path::PathBuf,
    current: RwLock<Arc<CapabilitySnapshot>>,
}

impl CapsState {
    pub fn init() -> CapsState {
        CapsState::new_with_path(wancode_config_path())
    }

    pub fn new_with_path(path: std::path::PathBuf) -> CapsState {
        let first = Arc::new(CapabilitySnapshot::load(&path));
        CapsState {
            path,
            current: RwLock::new(first),
        }
    }

    /// 当前世代（Arc 克隆，读者持有的一代不受后续替换影响）。
    pub fn snapshot(&self) -> Arc<CapabilitySnapshot> {
        self.current.read().unwrap().clone()
    }

    /// 重读文件、原子替换为新一代。
    pub fn reload(&self) {
        let fresh = Arc::new(CapabilitySnapshot::load(&self.path));
        *self.current.write().unwrap() = fresh;
    }
}

/// 生产 config.toml 文档加载：聊天链取 slug/base_url 专用。
/// 缺失 = 空文档零诊断（全新安装的常态）；读取失败 / 解析失败 →
/// 文件级诊断 + 空文档——所有 option 会落 unknown，但**必须**伴随可见
/// 诊断，绝不允许 unwrap_or_default 式的静默降级（复核 P1）。
pub fn load_config_doc(path: &std::path::Path) -> (toml_edit::DocumentMut, Option<FileIssue>) {
    match std::fs::read_to_string(path) {
        Ok(text) => match text.parse::<toml_edit::DocumentMut>() {
            Ok(doc) => (doc, None),
            Err(e) => (
                toml_edit::DocumentMut::default(),
                Some(FileIssue {
                    kind: FileIssueKind::ParseError,
                    message: format!("config.toml: {e}"),
                }),
            ),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            (toml_edit::DocumentMut::default(), None)
        }
        Err(e) => (
            toml_edit::DocumentMut::default(),
            Some(FileIssue {
                kind: FileIssueKind::ReadError,
                message: format!("config.toml: {e}"),
            }),
        ),
    }
}

/// 一个模型在某条 UI 链上的最终能力视图：能力 + 该条目归属的诊断。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ResolvedModelCaps {
    pub caps: ModelCaps,
    pub issues: Vec<CapIssue>,
}

fn entry_issues(snapshot: &CapabilitySnapshot, catalog_key: &str) -> Vec<CapIssue> {
    snapshot
        .issues()
        .iter()
        .filter(|i| i.catalog_key.as_deref() == Some(catalog_key))
        .cloned()
        .collect()
}

/// 设置页链适配器：model_list 逐条目调用。
/// 只接收快照引用——内部不读文件。
pub fn settings_caps_for(
    snapshot: &CapabilitySnapshot,
    catalog_key: &str,
    slug: &str,
    base_url: &str,
) -> ResolvedModelCaps {
    let ov = snapshot.overrides_for(catalog_key);
    ResolvedModelCaps {
        caps: resolve_caps(slug, base_url, &ov),
        issues: entry_issues(snapshot, catalog_key),
    }
}

/// 聊天目录链适配器：agent_start 对每个 available model 调用，产出
/// ModelOption.caps。slug/base_url 从 config.toml 的 [model.<key>] 条目
/// 取（引擎的 catalog key 即 config key）；条目缺失时以 key 当 slug、
/// 空 base_url 解析（自然落 unknown——fail-visible 而非猜测）。
/// 独立于设置页适配器实现——两条真实链的一致性由双链测试锁住。
pub fn model_option_caps(
    snapshot: &CapabilitySnapshot,
    catalog_key: &str,
    config_doc: &toml_edit::DocumentMut,
) -> ResolvedModelCaps {
    let entry = config_doc
        .get("model")
        .and_then(|m| m.as_table_like())
        .and_then(|t| t.get(catalog_key))
        .and_then(|v| v.as_table_like());
    let get = |k: &str| {
        entry
            .and_then(|t| t.get(k))
            .and_then(|v| v.as_str())
            .map(String::from)
    };
    let slug = get("model").unwrap_or_else(|| catalog_key.to_string());
    let base_url = get("base_url").unwrap_or_default();
    let ov = snapshot.overrides_for(catalog_key);
    ResolvedModelCaps {
        caps: resolve_caps(&slug, &base_url, &ov),
        issues: entry_issues(snapshot, catalog_key),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_caps::{CapSource, CapState};

    const ZHIPU_OPEN: &str = "https://open.bigmodel.cn/api/paas/v4";

    /// 文件不存在 = 空快照、零错误。
    #[test]
    fn missing_file_is_empty_snapshot_no_issues() {
        let dir = tempfile::tempdir().unwrap();
        let snap = CapabilitySnapshot::load(&dir.path().join("wancode.toml"));
        assert_eq!(snap, CapabilitySnapshot::empty());
        assert!(snap.file_issue.is_none());
        assert!(snap.issues().is_empty());
    }

    /// TOML 语法错误必须形成可见的文件级诊断——不许退化为静默空配置。
    #[test]
    fn syntax_error_yields_file_issue_not_silent_empty() {
        let snap = CapabilitySnapshot::from_text("[model_capabilities\nbroken");
        let issue = snap.file_issue.as_ref().expect("必须有文件级诊断");
        assert_eq!(issue.kind, FileIssueKind::ParseError);
        assert!(!issue.message.is_empty());
        // 覆盖为空是"可见地空"
        assert_eq!(
            snap.overrides_for("any"),
            crate::model_caps::CapOverrides::default()
        );
    }

    /// 读取失败（存在但不可读）→ ReadError。Windows 上以目录冒充文件模拟。
    #[test]
    fn unreadable_file_yields_read_error() {
        let dir = tempfile::tempdir().unwrap();
        let as_dir = dir.path().join("wancode.toml");
        std::fs::create_dir(&as_dir).unwrap();
        let snap = CapabilitySnapshot::load(&as_dir);
        assert_eq!(
            snap.file_issue.as_ref().map(|i| i.kind.clone()),
            Some(FileIssueKind::ReadError)
        );
    }

    /// 双链一致性门槛：**两个生产适配器**——settings_caps_for（model_list
    /// 在用）与 model_option_caps（agent_start 在用）——从同一快照对同一
    /// key 必须得到完全相同的能力与诊断归属——含正确、覆盖、带诊断三形态。
    #[test]
    fn both_production_adapters_agree_on_caps_and_issue_attribution() {
        let snap = CapabilitySnapshot::from_text(
            r#"
[model_capabilities.my-eyes]
vision_input = false

[model_capabilities.broken]
vision_input = "yes"
"#,
        );
        // ModelOption 链的 slug/base_url 来源：config.toml 文档
        let config_doc: toml_edit::DocumentMut = format!(
            r#"
[model.my-eyes]
model = "glm-4v-flash"
base_url = "{ZHIPU_OPEN}"

[model.plain]
model = "glm-5.2"
base_url = "{ZHIPU_OPEN}"

[model.broken]
model = "glm-4v-flash"
base_url = "{ZHIPU_OPEN}"
"#
        )
        .parse()
        .unwrap();
        for (key, slug) in [
            ("my-eyes", "glm-4v-flash"),
            ("plain", "glm-5.2"),
            ("broken", "glm-4v-flash"),
        ] {
            let settings = settings_caps_for(&snap, key, slug, ZHIPU_OPEN);
            let chat = model_option_caps(&snap, key, &config_doc);
            assert_eq!(
                settings, chat,
                "key={key}: 两条生产链的能力/诊断必须逐字相同"
            );
        }
        // 语义抽查：覆盖生效（my-eyes 的内置 true 被显式 false 压掉）
        let eyes = model_option_caps(&snap, "my-eyes", &config_doc);
        assert_eq!(eyes.caps.vision_input.state, CapState::Unsupported);
        assert_eq!(eyes.caps.vision_input.source, CapSource::Config);
        // broken 的诊断归属到 broken，plain 零诊断
        let broken = model_option_caps(&snap, "broken", &config_doc);
        assert_eq!(broken.issues.len(), 1);
        assert_eq!(broken.issues[0].catalog_key.as_deref(), Some("broken"));
        assert!(model_option_caps(&snap, "plain", &config_doc)
            .issues
            .is_empty());
        // config.toml 无条目的 key：落 unknown（不猜测）
        let ghost = model_option_caps(&snap, "ghost", &config_doc);
        assert_eq!(ghost.caps.vision_input.state, CapState::Unknown);
    }

    /// 损坏的 config.toml：聊天链加载必须出文件级诊断——空文档导致的
    /// 全员 unknown 只有在伴随可见诊断时才合法（复核 P1：禁静默降级）。
    #[test]
    fn corrupted_config_doc_yields_visible_issue_not_silent_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[model.broken
not toml").unwrap();
        let (doc, issue) = load_config_doc(&path);
        let issue = issue.expect("解析失败必须出文件级诊断");
        assert_eq!(issue.kind, FileIssueKind::ParseError);
        assert!(issue.message.contains("config.toml"));
        // 空文档下 option 落 unknown——但调用方持有 issue，非静默
        let snap = CapabilitySnapshot::empty();
        let r = model_option_caps(&snap, "any", &doc);
        assert_eq!(r.caps.vision_input.state, CapState::Unknown);
        // 缺失（全新安装）才是零诊断
        let (_, issue) = load_config_doc(&dir.path().join("absent.toml"));
        assert!(issue.is_none());
        // 读取失败（目录冒充文件）→ ReadError
        let as_dir = dir.path().join("dir.toml");
        std::fs::create_dir(&as_dir).unwrap();
        let (_, issue) = load_config_doc(&as_dir);
        assert_eq!(issue.map(|i| i.kind), Some(FileIssueKind::ReadError));
    }

    /// 热加载：更新 wancode.toml → reload() → 无需重启即得新能力；
    /// 旧世代的持有者不受影响（原子替换语义）。
    #[test]
    fn hot_reload_swaps_generation_without_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wancode.toml");

        // 世代 1：无文件 → 空（路径注入，后续 reload 走生产实现）
        let state = CapsState::new_with_path(path.clone());
        let gen1 = state.snapshot();
        assert_eq!(gen1.overrides_for("m").vision_input, None);

        // 用户写入新配置 → **生产 reload()** 换代
        std::fs::write(&path, "[model_capabilities.m]\nvision_input = true\n").unwrap();
        state.reload();

        // 新读者立即看到新能力——无需重启
        let gen2 = state.snapshot();
        assert_eq!(gen2.overrides_for("m").vision_input, Some(true));
        // 旧世代持有者仍是完整一致的旧一代（原子替换，不是原地修改）
        assert_eq!(gen1.overrides_for("m").vision_input, None);
    }
}
