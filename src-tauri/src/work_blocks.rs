//! 解析器输出契约 + 锚点铸造（v0.20 W3-b，DOCX only）。
//!
//! # 为什么先定契约再写解析器
//!
//! DOCX 解析 crate 要等那**一次** vendored-lock 审计才能进产品代码（裁断：
//! NFC + DOCX crate + PDF 栈一起加）。但「解析器该产出什么」与「怎么由它
//! 铸锚点」跟具体 crate 无关——先把这层定死并验证，等 crate 落地时解析器
//! 只需填这个契约，锚点这一层不必重测。
//!
//! # 契约
//!
//! 解析器把文档拍平成 [`WorkBlock`] 序列。每块携带：
//!   - `path`：块路径（DOCX 形如 `body/p[41]`），锚点定位子直接用它；
//!   - `raw`：**原始抽取文本**（不归一化——归一只用于摘录相等性，见
//!     `work_anchor` 模块头）；
//!   - `runs`：块内各 run 覆盖 `raw` 的 UTF-16 半开区间。DOCX 常把一句话
//!     拆进多个 run，`run_ordinals` 就是据此算出来的。
//!
//! # 铸造的不变量
//!
//! 铸出的锚点**必须能被 `resolve_anchor` 解回同一段文本**——铸造与解析是
//! 一对，任何一侧漂移都在单测里炸掉。越界/劈开代理对/块不存在一律
//! fail-closed，绝不"就近取整"。
//!
//! **本切片不含任何文件解析**（无 zip / 无 XML）。PDF 亦不在范围内：
//! Work PDF 未完成。

use serde::{Deserialize, Serialize};

use crate::work_anchor::{
    utf16_len, utf16_slice, Anchor, AnchorError, DocKind, Locator,
};
use crate::work_staging::ImportId;

/// 解析器产出的一个块（DOCX 段落）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkBlock {
    /// 块路径，锚点 `block_path` 直接取它。
    pub path: String,
    /// 原始抽取文本（不归一化）。
    pub raw: String,
    /// 各 run 覆盖 `raw` 的 UTF-16 半开区间，按出现顺序。
    pub runs: Vec<[usize; 2]>,
}

impl WorkBlock {
    /// 该块的 UTF-16 长度。
    pub fn len_utf16(&self) -> usize {
        utf16_len(&self.raw)
    }

    /// 块自身是否自洽：run 区间递增、不重叠、不越界。
    ///
    /// 解析器有 bug 时这里要能拦住——否则错误的 run 边界会被铸进锚点，
    /// 之后再排查就要跨解析器/锚点两层。
    pub fn is_well_formed(&self) -> bool {
        let len = self.len_utf16();
        let mut prev_end = 0usize;
        for &[s, e] in &self.runs {
            if s > e || e > len || s < prev_end {
                return false;
            }
            prev_end = e;
        }
        true
    }
}

/// 铸造失败的原因（与 [`AnchorError`] 分开：这些是**铸造期**的输入问题，
/// 不是解析期的失配）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum MintError {
    /// 找不到该 `block_path`。
    BlockNotFound { path: String },
    /// 块自身不自洽（run 区间乱序/重叠/越界）——解析器 bug。
    MalformedBlock { path: String },
    /// 请求的区间在块内非法（越界、start > end、劈开代理对）。
    BadRange { range: [usize; 2], block_len: usize },
    /// 区间未覆盖任何 run——锚点必须落在实际文本上。
    RangeCoversNoRun { range: [usize; 2] },
}

impl std::fmt::Display for MintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MintError::BlockNotFound { path } => write!(f, "块不存在: {path}"),
            MintError::MalformedBlock { path } => write!(f, "块结构不自洽（解析器 bug）: {path}"),
            MintError::BadRange { range, block_len } => {
                write!(f, "区间 [{}, {}) 在长度 {block_len} 的块内非法", range[0], range[1])
            }
            MintError::RangeCoversNoRun { range } => {
                write!(f, "区间 [{}, {}) 未覆盖任何 run", range[0], range[1])
            }
        }
    }
}
impl std::error::Error for MintError {}

/// 区间覆盖到的 run 序号（半开区间相交即算覆盖；零长区间不覆盖任何 run）。
fn covered_run_ordinals(block: &WorkBlock, [start, end]: [usize; 2]) -> Vec<u32> {
    if start >= end {
        return Vec::new();
    }
    block
        .runs
        .iter()
        .enumerate()
        .filter(|(_, &[s, e])| s < end && start < e)
        .map(|(i, _)| i as u32)
        .collect()
}

/// 在指定块的指定 UTF-16 区间上铸造 DOCX 锚点。
///
/// 摘录取自 `raw` 的该区间（`Anchor::new` 内部做归一化 + 截断）。
pub fn mint_docx_anchor(
    blocks: &[WorkBlock],
    block_path: &str,
    raw_range: [usize; 2],
    import_id: ImportId,
    source_sha256: &str,
) -> Result<Anchor, MintError> {
    let block = blocks
        .iter()
        .find(|b| b.path == block_path)
        .ok_or_else(|| MintError::BlockNotFound {
            path: block_path.to_string(),
        })?;
    if !block.is_well_formed() {
        return Err(MintError::MalformedBlock {
            path: block_path.to_string(),
        });
    }
    // 区间合法性交给 utf16_slice——它同时管越界与代理对，语义与解析侧同源。
    let excerpt_raw =
        utf16_slice(&block.raw, raw_range[0], raw_range[1]).map_err(|_| MintError::BadRange {
            range: raw_range,
            block_len: block.len_utf16(),
        })?;
    let run_ordinals = covered_run_ordinals(block, raw_range);
    if run_ordinals.is_empty() {
        return Err(MintError::RangeCoversNoRun { range: raw_range });
    }
    Ok(Anchor::new(
        import_id,
        source_sha256,
        DocKind::Docx,
        Locator::Docx {
            block_path: block.path.clone(),
            run_ordinals,
            raw_range,
        },
        &excerpt_raw,
    ))
}

/// 解析一个锚点回到它所指的块文本。
///
/// 与 `work_anchor::resolve_anchor` 的区别：那个对着**一段** raw 文本解析，
/// 这个先按 `block_path` 在块序列里定位。fail-closed 语义一致。
pub fn resolve_docx_anchor(
    anchor: &Anchor,
    blocks: &[WorkBlock],
    doc_sha256: &str,
) -> Result<String, AnchorErrorOrMissing> {
    let path = match &anchor.locator {
        Locator::Docx { block_path, .. } => block_path.as_str(),
        Locator::Pdf { .. } => return Err(AnchorErrorOrMissing::KindMismatch),
    };
    let block = blocks
        .iter()
        .find(|b| b.path == path)
        .ok_or_else(|| AnchorErrorOrMissing::BlockMissing {
            path: path.to_string(),
        })?;
    crate::work_anchor::resolve_anchor(anchor, doc_sha256, &block.raw)
        .map_err(AnchorErrorOrMissing::Anchor)
}

/// 块级解析的失败原因：锚点本身的失配，或块已不存在（文档被替换/重解析后
/// 结构变了）——两者都让引用**不可点击**，但原因要能区分给用户看。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AnchorErrorOrMissing {
    Anchor(AnchorError),
    BlockMissing { path: String },
    KindMismatch,
}

impl std::fmt::Display for AnchorErrorOrMissing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AnchorErrorOrMissing::Anchor(e) => write!(f, "{e}"),
            AnchorErrorOrMissing::BlockMissing { path } => {
                write!(f, "来源已失效：块 {path} 不存在")
            }
            AnchorErrorOrMissing::KindMismatch => write!(f, "来源已失效：定位子类型不符"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work_anchor::MAX_EXCERPT_UTF16;

    fn sha() -> String {
        "ab".repeat(32)
    }

    /// 一句话被拆成三个 run 的段落（DOCX 的常态）。
    fn split_block() -> WorkBlock {
        let raw = "这句话被拆成三段。".to_string();
        let n = utf16_len(&raw);
        WorkBlock {
            path: "body/p[41]".into(),
            raw,
            runs: vec![[0, 3], [3, 6], [6, n]],
        }
    }

    fn blocks() -> Vec<WorkBlock> {
        vec![
            WorkBlock {
                path: "body/p[0]".into(),
                raw: "第一段。".into(),
                runs: vec![[0, 4]],
            },
            split_block(),
        ]
    }

    // ── 铸造 ↔ 解析是一对：铸出来的必须解得回同一段文本 ────────────────
    #[test]
    fn minted_anchor_resolves_back_to_the_same_text() {
        let bs = blocks();
        let a = mint_docx_anchor(&bs, "body/p[41]", [3, 6], ImportId::mint(), &sha()).unwrap();
        let got = resolve_docx_anchor(&a, &bs, &sha()).unwrap();
        assert_eq!(got, utf16_slice(&bs[1].raw, 3, 6).unwrap());
    }

    #[test]
    fn cross_run_range_records_every_covered_run() {
        let bs = blocks();
        let n = bs[1].len_utf16();
        // 覆盖整段 → 三个 run 全部记录。
        let a = mint_docx_anchor(&bs, "body/p[41]", [0, n], ImportId::mint(), &sha()).unwrap();
        match &a.locator {
            Locator::Docx { run_ordinals, block_path, raw_range } => {
                assert_eq!(run_ordinals, &vec![0, 1, 2], "跨 run 必须记全");
                assert_eq!(block_path, "body/p[41]");
                assert_eq!(raw_range, &[0, n]);
            }
            _ => panic!("应为 DOCX 定位子"),
        }
        assert_eq!(resolve_docx_anchor(&a, &bs, &sha()).unwrap(), bs[1].raw);
    }

    #[test]
    fn partial_range_records_only_the_runs_it_touches() {
        let bs = blocks();
        // [2,4) 跨 run0([0,3)) 与 run1([3,6)) 的边界。
        let a = mint_docx_anchor(&bs, "body/p[41]", [2, 4], ImportId::mint(), &sha()).unwrap();
        match &a.locator {
            Locator::Docx { run_ordinals, .. } => assert_eq!(run_ordinals, &vec![0, 1]),
            _ => panic!("应为 DOCX 定位子"),
        }
    }

    // ── fail-closed：铸造期 ──────────────────────────────────────────
    #[test]
    fn unknown_block_is_rejected() {
        let bs = blocks();
        assert!(matches!(
            mint_docx_anchor(&bs, "body/p[999]", [0, 1], ImportId::mint(), &sha()),
            Err(MintError::BlockNotFound { .. })
        ));
    }

    #[test]
    fn out_of_range_is_rejected() {
        let bs = blocks();
        assert!(matches!(
            mint_docx_anchor(&bs, "body/p[0]", [0, 999], ImportId::mint(), &sha()),
            Err(MintError::BadRange { .. })
        ));
    }

    #[test]
    fn empty_range_covers_no_run_and_is_rejected() {
        let bs = blocks();
        // 锚点必须落在实际文本上；零长区间不构成引用。
        assert!(matches!(
            mint_docx_anchor(&bs, "body/p[0]", [2, 2], ImportId::mint(), &sha()),
            Err(MintError::RangeCoversNoRun { .. })
        ));
    }

    #[test]
    fn malformed_block_from_a_buggy_parser_is_rejected() {
        // run 区间重叠/乱序 = 解析器 bug，必须在铸造前拦住。
        let bad = vec![WorkBlock {
            path: "body/p[0]".into(),
            raw: "abcdef".into(),
            runs: vec![[0, 4], [2, 6]], // 重叠
        }];
        assert!(!bad[0].is_well_formed());
        assert!(matches!(
            mint_docx_anchor(&bad, "body/p[0]", [0, 3], ImportId::mint(), &sha()),
            Err(MintError::MalformedBlock { .. })
        ));
    }

    #[test]
    fn range_splitting_a_surrogate_pair_is_rejected_at_mint_time() {
        // 𝄞 占 2 个 UTF-16 单元；[1,2) 劈开它。
        let bs = vec![WorkBlock {
            path: "body/p[0]".into(),
            raw: "a𝄞b".into(),
            runs: vec![[0, 4]],
        }];
        assert!(matches!(
            mint_docx_anchor(&bs, "body/p[0]", [1, 2], ImportId::mint(), &sha()),
            Err(MintError::BadRange { .. })
        ));
    }

    // ── fail-closed：解析期（引用不可点击的三条来源）────────────────
    #[test]
    fn resolving_against_a_replaced_document_fails_closed() {
        let bs = blocks();
        let a = mint_docx_anchor(&bs, "body/p[41]", [0, 3], ImportId::mint(), &sha()).unwrap();
        // 文档换了（哈希不同）→ 不得近似指向。
        let other = "cd".repeat(32);
        assert!(matches!(
            resolve_docx_anchor(&a, &bs, &other),
            Err(AnchorErrorOrMissing::Anchor(_))
        ));
    }

    #[test]
    fn resolving_when_the_block_disappeared_fails_closed() {
        let bs = blocks();
        let a = mint_docx_anchor(&bs, "body/p[41]", [0, 3], ImportId::mint(), &sha()).unwrap();
        // 重解析后结构变了，该块没了。
        let shrunk = vec![bs[0].clone()];
        assert!(matches!(
            resolve_docx_anchor(&a, &shrunk, &sha()),
            Err(AnchorErrorOrMissing::BlockMissing { .. })
        ));
    }

    #[test]
    fn resolving_a_pdf_anchor_against_docx_blocks_fails_closed() {
        let bs = blocks();
        let pdf = Anchor::new(
            ImportId::mint(),
            sha(),
            DocKind::Pdf,
            Locator::Pdf { page: 1, chunk: 0, raw_range: [0, 3] },
            "abc",
        );
        assert!(matches!(
            resolve_docx_anchor(&pdf, &bs, &sha()),
            Err(AnchorErrorOrMissing::KindMismatch)
        ));
    }

    // ── 摘录截断后仍能解析（长段落）────────────────────────────────
    #[test]
    fn long_range_truncates_excerpt_but_still_resolves() {
        let raw: String = std::iter::repeat_n('字', 800).collect();
        let n = utf16_len(&raw);
        let bs = vec![WorkBlock {
            path: "body/p[0]".into(),
            raw,
            runs: vec![[0, n]],
        }];
        let a = mint_docx_anchor(&bs, "body/p[0]", [0, n], ImportId::mint(), &sha()).unwrap();
        assert!(utf16_len(&a.excerpt) <= MAX_EXCERPT_UTF16);
        assert!(resolve_docx_anchor(&a, &bs, &sha()).is_ok(), "截断的摘录仍须解析成功");
    }

    #[test]
    fn block_contract_round_trips_through_json() {
        let b = split_block();
        let back: WorkBlock = serde_json::from_str(&serde_json::to_string(&b).unwrap()).unwrap();
        assert_eq!(b, back);
    }
}
