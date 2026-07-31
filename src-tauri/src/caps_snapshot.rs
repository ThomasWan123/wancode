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
pub struct CapsState(RwLock<Arc<CapabilitySnapshot>>);

impl CapsState {
    pub fn init() -> CapsState {
        CapsState(RwLock::new(Arc::new(CapabilitySnapshot::load(
            &wancode_config_path(),
        ))))
    }

    /// 当前世代（Arc 克隆，读者持有的一代不受后续替换影响）。
    pub fn snapshot(&self) -> Arc<CapabilitySnapshot> {
        self.0.read().unwrap().clone()
    }

    /// 重读文件、原子替换为新一代。
    pub fn reload(&self) {
        let fresh = Arc::new(CapabilitySnapshot::load(&wancode_config_path()));
        *self.0.write().unwrap() = fresh;
    }
}

/// 一个模型在某条 UI 链上的最终能力视图：能力 + 该条目归属的诊断。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

/// 聊天目录链适配器：对整份目录（key, slug, base_url）批量出能力视图，
/// 供聊天模型下拉一次取全。独立于设置页适配器实现——两条真实链的
/// 一致性由测试与"共享同一快照"共同保证，而非同一段代码。
pub fn chat_caps_map(
    snapshot: &CapabilitySnapshot,
    catalog: &[(String, String, String)],
) -> std::collections::BTreeMap<String, ResolvedModelCaps> {
    let mut out = std::collections::BTreeMap::new();
    for (key, slug, base_url) in catalog {
        let ov = snapshot.overrides_for(key);
        out.insert(
            key.clone(),
            ResolvedModelCaps {
                caps: resolve_caps(slug, base_url, &ov),
                issues: entry_issues(snapshot, key),
            },
        );
    }
    out
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

    /// 双链一致性门槛：**两个不同适配器**（设置页 settings_caps_for /
    /// 聊天目录 chat_caps_map）从同一快照对同一 key 必须得到完全相同的
    /// 能力与诊断归属——含正确条目、覆盖条目、带诊断条目三种形态。
    #[test]
    fn both_adapters_agree_on_caps_and_issue_attribution() {
        let snap = CapabilitySnapshot::from_text(
            r#"
[model_capabilities.my-eyes]
vision_input = false

[model_capabilities.broken]
vision_input = "yes"
"#,
        );
        let catalog: Vec<(String, String, String)> = vec![
            ("my-eyes".into(), "glm-4v-flash".into(), ZHIPU_OPEN.into()),
            ("plain".into(), "glm-5.2".into(), ZHIPU_OPEN.into()),
            ("broken".into(), "glm-4v-flash".into(), ZHIPU_OPEN.into()),
        ];
        let chat = chat_caps_map(&snap, &catalog);
        for (key, slug, base_url) in &catalog {
            let settings = settings_caps_for(&snap, key, slug, base_url);
            assert_eq!(
                Some(&settings),
                chat.get(key),
                "key={key}: 两条链的能力/诊断必须逐字相同"
            );
        }
        // 语义抽查：覆盖生效（my-eyes 的内置 true 被显式 false 压掉）
        assert_eq!(
            chat["my-eyes"].caps.vision_input.state,
            CapState::Unsupported
        );
        assert_eq!(
            chat["my-eyes"].caps.vision_input.source,
            CapSource::Config
        );
        // broken 的诊断归属到 broken，plain 零诊断
        assert_eq!(chat["broken"].issues.len(), 1);
        assert_eq!(chat["broken"].issues[0].catalog_key.as_deref(), Some("broken"));
        assert!(chat["plain"].issues.is_empty());
    }

    /// 热加载：更新 wancode.toml → reload() → 无需重启即得新能力；
    /// 旧世代的持有者不受影响（原子替换语义）。
    #[test]
    fn hot_reload_swaps_generation_without_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wancode.toml");

        // 世代 1：无文件 → 空
        let state = CapsState(RwLock::new(Arc::new(CapabilitySnapshot::load(&path))));
        let gen1 = state.snapshot();
        assert_eq!(gen1.overrides_for("m").vision_input, None);

        // 用户写入新配置，reload 换代
        std::fs::write(&path, "[model_capabilities.m]\nvision_input = true\n").unwrap();
        let fresh = Arc::new(CapabilitySnapshot::load(&path));
        *state.0.write().unwrap() = fresh;

        // 新读者立即看到新能力——无需重启
        let gen2 = state.snapshot();
        assert_eq!(gen2.overrides_for("m").vision_input, Some(true));
        // 旧世代持有者仍是完整一致的旧一代（原子替换，不是原地修改）
        assert_eq!(gen1.overrides_for("m").vision_input, None);
    }
}
