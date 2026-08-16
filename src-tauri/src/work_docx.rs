//! DOCX 抽取（产品代码）。W3 spike 的 `extract_docx` productionize。
//!
//! DOCX = zip；正文在 `word/document.xml`。段落 `<w:p>`，其中 `<w:r>` 是
//! run、`<w:t>` 是文本。一句话常被拆进多个 run（格式一变就断开）——这正是
//! 锚点要记 `run_ordinals` 的原因。
//!
//! ## 相对 spike 补上的三件事
//!
//! **① run 之外的文本。** spike 无条件把 `<w:t>` 的文本追加进 `raw`，不管
//! 它是否在某个 `<w:r>` 里。真遇到这种结构时 `runs` 会**留缺口**，而
//! [`WorkBlock::is_well_formed`] 要求 runs 铺满 `[0, len)`——缺口意味着落在
//! 缺口里的区间会算出空的 `run_ordinals`，等于铸出一个指不到 run 的锚点。
//! 这里的处理是：只在 run 内累计文本，并**统计被丢弃的字符**；一旦丢过，
//! 整篇拒收（`TextOutsideRun`）。既不静默丢正文，也不产出破的 tiling。
//!
//! **② XML 实体展开炸弹。** 见到 `DOCTYPE` 即拒——W1 安全面把这条列为
//! REJECTED，但那是 spike 里的独立探针；产品读取路径必须自己拦。
//!
//! **③ zip 炸弹。** 解压**前**先看声明的解压后体积，超限即拒；解压时再按
//! 上限截断读取，双保险（声明值是攻击者可控的）。
//!
//! ## 路径穿越为何不适用
//!
//! 我们**从不落盘**：只按精确名 `word/document.xml` 取一个条目读进内存。
//! 没有写路径，`../` 无处施展。这是结构性的，不是靠校验挡的。

use crate::work_blocks::WorkBlock;
use std::io::Read;

/// 抽取资源边界。
///
/// **这些数字是保守起点，不是实测定档**——设计 §1.1 要求「资源边界实测并
/// 定档」，实测另行安排。
#[derive(Debug, Clone, Copy)]
pub struct DocxLimits {
    /// `word/document.xml` 解压后体积上限（zip 炸弹）。
    pub max_document_xml_bytes: u64,
    /// 块数上限。
    pub max_blocks: usize,
    /// 单块 UTF-16 长度上限。
    pub max_block_utf16: usize,
}

impl Default for DocxLimits {
    fn default() -> Self {
        Self {
            max_document_xml_bytes: 64 * 1024 * 1024,
            max_blocks: 200_000,
            max_block_utf16: 1_000_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocxError {
    NotAZip(String),
    MissingDocumentXml(String),
    /// 声明的解压后体积超限——**解压前**就拒了。
    DeclaredSizeOverCap { declared: u64, cap: u64 },
    /// 实际读取时超限（声明值不可信，这是第二道）。
    UncompressedOverCap { cap: u64 },
    NotUtf8(String),
    /// 见到 DOCTYPE：实体展开炸弹的前提，一律拒。
    DoctypeRejected,
    XmlError(String),
    /// 有正文落在任何 run 之外——解析器无法为它给出 run 归属，
    /// 铸出的锚点会指不到 run，故整篇拒收。
    TextOutsideRun { dropped_chars: usize },
    /// 块自身不自洽（runs 未铺满）。解析器 bug 必须当场拦，
    /// 不能漏进锚点层。
    MalformedBlock { path: String },
    TooManyBlocks { cap: usize },
    BlockTooLong { path: String, cap: usize },
}

impl std::fmt::Display for DocxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAZip(e) => write!(f, "不是合法 zip：{e}"),
            Self::MissingDocumentXml(e) => write!(f, "缺 word/document.xml：{e}"),
            Self::DeclaredSizeOverCap { declared, cap } => {
                write!(f, "声明解压后体积 {declared} 超过上限 {cap}，解压前拒收")
            }
            Self::UncompressedOverCap { cap } => write!(f, "解压后体积超过上限 {cap}"),
            Self::NotUtf8(e) => write!(f, "document.xml 非 UTF-8：{e}"),
            Self::DoctypeRejected => {
                write!(f, "document.xml 含 DOCTYPE——实体展开风险，拒收")
            }
            Self::XmlError(e) => write!(f, "XML 解析失败：{e}"),
            Self::TextOutsideRun { dropped_chars } => write!(
                f,
                "有 {dropped_chars} 个字符落在 run 之外，无法给出 run 归属，整篇拒收"
            ),
            Self::MalformedBlock { path } => {
                write!(f, "块 {path} 的 runs 未铺满其文本（解析器 bug）")
            }
            Self::TooManyBlocks { cap } => write!(f, "块数超过上限 {cap}"),
            Self::BlockTooLong { path, cap } => {
                write!(f, "块 {path} 长度超过上限 {cap}")
            }
        }
    }
}

/// 读出 `word/document.xml`，两道体积上限。
fn read_document_xml(path: &std::path::Path, limits: &DocxLimits) -> Result<String, DocxError> {
    let file = std::fs::File::open(path).map_err(|e| DocxError::NotAZip(e.to_string()))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| DocxError::NotAZip(e.to_string()))?;
    let mut entry = zip
        .by_name("word/document.xml")
        .map_err(|e| DocxError::MissingDocumentXml(e.to_string()))?;

    // 第一道：声明值。攻击者可控，但便宜——能挡住的先挡掉，不浪费解压。
    let declared = entry.size();
    if declared > limits.max_document_xml_bytes {
        return Err(DocxError::DeclaredSizeOverCap {
            declared,
            cap: limits.max_document_xml_bytes,
        });
    }

    // 第二道：实际读取时按上限截断。声明小、实际大的 zip 炸弹在这里死。
    // 读 cap+1 字节：读满即说明超限（而不是「恰好等于上限」）。
    let cap = limits.max_document_xml_bytes;
    let mut buf = Vec::new();
    let n = entry
        .by_ref()
        .take(cap + 1)
        .read_to_end(&mut buf)
        .map_err(|e| DocxError::MissingDocumentXml(e.to_string()))?;
    if n as u64 > cap {
        return Err(DocxError::UncompressedOverCap { cap });
    }
    String::from_utf8(buf).map_err(|e| DocxError::NotUtf8(e.to_string()))
}

/// 抽取为块序列。空段落被跳过（无正文的段落不构成可锚定的块）。
pub fn parse_docx(
    path: &std::path::Path,
    limits: DocxLimits,
) -> Result<Vec<WorkBlock>, DocxError> {
    let xml = read_document_xml(path, &limits)?;
    parse_document_xml(&xml, limits)
}

/// 与 IO 分离，便于直接喂构造出的 XML 做对抗测试。
pub fn parse_document_xml(xml: &str, limits: DocxLimits) -> Result<Vec<WorkBlock>, DocxError> {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut blocks: Vec<WorkBlock> = Vec::new();
    let mut para_idx = 0usize;
    let mut cur: Option<WorkBlock> = None;
    let mut in_text = false;
    let mut run_start: Option<usize> = None;
    // run 之外被丢弃的字符数——非零即整篇拒收（见文件头 ①）。
    let mut dropped_outside_run = 0usize;

    loop {
        match reader.read_event_into(&mut buf) {
            // 实体展开炸弹的前提是 DTD。见到就拒，不试图「安全地」处理它。
            Ok(Event::DocType(_)) => return Err(DocxError::DoctypeRejected),
            Ok(Event::Start(e)) => match e.name().as_ref() {
                b"w:p" => {
                    cur = Some(WorkBlock {
                        path: format!("body/p[{para_idx}]"),
                        raw: String::new(),
                        runs: Vec::new(),
                    });
                }
                b"w:r" => {
                    if let Some(b) = &cur {
                        run_start = Some(b.len_utf16());
                    }
                }
                b"w:t" => in_text = true,
                _ => {}
            },
            Ok(Event::Text(t)) => {
                if in_text {
                    let s = t
                        .unescape()
                        .map_err(|e| DocxError::XmlError(e.to_string()))?;
                    match (&mut cur, run_start) {
                        // 只在 run 内累计：这样 runs 天然铺满 raw。
                        (Some(b), Some(_)) => b.raw.push_str(&s),
                        // 段落内、run 外的正文：记账，稍后整篇拒收。
                        (Some(_), None) => dropped_outside_run += s.chars().count(),
                        // 段落外的 w:t：同样记账，不静默吞掉。
                        (None, _) => dropped_outside_run += s.chars().count(),
                    }
                }
            }
            Ok(Event::End(e)) => match e.name().as_ref() {
                b"w:t" => in_text = false,
                b"w:r" => {
                    if let (Some(b), Some(st)) = (&mut cur, run_start.take()) {
                        let en = b.len_utf16();
                        // 零宽 run（无文本）不记：记了会破坏「每条非空」。
                        if en > st {
                            b.runs.push([st, en]);
                        }
                    }
                }
                b"w:p" => {
                    if let Some(b) = cur.take() {
                        if !b.raw.trim().is_empty() {
                            if b.len_utf16() > limits.max_block_utf16 {
                                return Err(DocxError::BlockTooLong {
                                    path: b.path,
                                    cap: limits.max_block_utf16,
                                });
                            }
                            // 解析器 bug 当场拦，不许漏进锚点层。
                            if !b.is_well_formed() {
                                return Err(DocxError::MalformedBlock { path: b.path });
                            }
                            if blocks.len() >= limits.max_blocks {
                                return Err(DocxError::TooManyBlocks {
                                    cap: limits.max_blocks,
                                });
                            }
                            blocks.push(b);
                        }
                        para_idx += 1;
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(DocxError::XmlError(e.to_string())),
            _ => {}
        }
        buf.clear();
    }

    if dropped_outside_run > 0 {
        return Err(DocxError::TextOutsideRun {
            dropped_chars: dropped_outside_run,
        });
    }
    Ok(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(inner: &str) -> String {
        format!(r#"<w:document><w:body>{inner}</w:body></w:document>"#)
    }

    /// 正对照：两个 run 拼成一段，runs 必须**铺满**。
    /// 没有这条，「一律拒收」的实现会让所有负例都通过。
    #[test]
    fn two_runs_tile_the_paragraph() {
        let xml = p(r#"<w:p><w:r><w:t>你好</w:t></w:r><w:r><w:t>世界</w:t></w:r></w:p>"#);
        let b = parse_document_xml(&xml, DocxLimits::default()).unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].raw, "你好世界");
        assert_eq!(b[0].runs, vec![[0, 2], [2, 4]]);
        assert!(b[0].is_well_formed(), "runs 必须铺满 [0,len)");
    }

    /// 零宽 run 不该被记成 run（记了会破坏「每条非空」），但也不该造成缺口。
    #[test]
    fn empty_run_is_skipped_without_creating_a_gap() {
        let xml = p(r#"<w:p><w:r><w:t>甲</w:t></w:r><w:r></w:r><w:r><w:t>乙</w:t></w:r></w:p>"#);
        let b = parse_document_xml(&xml, DocxLimits::default()).unwrap();
        assert_eq!(b[0].runs, vec![[0, 1], [1, 2]]);
        assert!(b[0].is_well_formed());
    }

    /// run 之外的正文 → 整篇拒收，而不是静默丢弃或留缺口。
    #[test]
    fn text_outside_run_is_rejected_not_silently_dropped() {
        let xml = p(r#"<w:p><w:t>裸文本</w:t><w:r><w:t>正常</w:t></w:r></w:p>"#);
        match parse_document_xml(&xml, DocxLimits::default()) {
            Err(DocxError::TextOutsideRun { dropped_chars }) => assert_eq!(dropped_chars, 3),
            other => panic!("应拒收 TextOutsideRun，实得 {other:?}"),
        }
    }

    /// DOCTYPE 一律拒——实体展开炸弹的前提。
    #[test]
    fn doctype_is_rejected() {
        let xml = format!(
            r#"<!DOCTYPE d [<!ENTITY x "boom">]>{}"#,
            p(r#"<w:p><w:r><w:t>a</w:t></w:r></w:p>"#)
        );
        assert_eq!(
            parse_document_xml(&xml, DocxLimits::default()),
            Err(DocxError::DoctypeRejected)
        );
    }

    /// 空段落不产块（无正文不可锚定），且不影响后续段落编号。
    #[test]
    fn blank_paragraph_skipped_but_index_advances() {
        let xml = p(r#"<w:p></w:p><w:p><w:r><w:t>甲</w:t></w:r></w:p>"#);
        let b = parse_document_xml(&xml, DocxLimits::default()).unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].path, "body/p[1]", "编号须反映真实段落序号，否则锚点指错段");
    }

    #[test]
    fn block_cap_is_enforced() {
        let one = r#"<w:p><w:r><w:t>x</w:t></w:r></w:p>"#;
        let xml = p(&one.repeat(3));
        let limits = DocxLimits { max_blocks: 2, ..DocxLimits::default() };
        assert_eq!(
            parse_document_xml(&xml, limits),
            Err(DocxError::TooManyBlocks { cap: 2 })
        );
    }

    /// 代理对（BMP 外字符）的 UTF-16 计数：一个 emoji = 2 个 UTF-16 单元。
    /// 记这条是因为锚点全用 UTF-16 计量，按 char 计会整体错位。
    #[test]
    fn surrogate_pair_counts_as_two_utf16_units() {
        let xml = p(r#"<w:p><w:r><w:t>😀</w:t></w:r><w:r><w:t>a</w:t></w:r></w:p>"#);
        let b = parse_document_xml(&xml, DocxLimits::default()).unwrap();
        assert_eq!(b[0].runs, vec![[0, 2], [2, 3]]);
        assert!(b[0].is_well_formed());
    }
}

/// 真实样本探针：合成 XML 证不了真实 Word 产物的结构（真 .docx 里满是
/// `w:pPr`/`w:rPr`/`w:tab`/`w:bookmarkStart`，以及被格式切碎的 run）。
///
/// 样本**不入库**（个人文档），故用环境变量传路径；未设即跳过，所以 CI 上
/// 不会跑——这一条永远是 NOT-RUN in CI，本地实证结果写进证据档。
///
/// ```text
/// WANCODE_DOCX_SAMPLE="C:\path\to\real.docx" cargo test -p wancode --lib docx_real_sample -- --nocapture
/// ```
#[cfg(test)]
mod real_sample {
    use super::*;

    #[test]
    fn docx_real_sample_probe() {
        let Ok(path) = std::env::var("WANCODE_DOCX_SAMPLE") else {
            eprintln!("SKIP：未设 WANCODE_DOCX_SAMPLE");
            return;
        };
        let path = std::path::PathBuf::from(path);
        let blocks = match parse_docx(&path, DocxLimits::default()) {
            Ok(b) => b,
            Err(e) => panic!("真实样本解析失败：{e}"),
        };
        assert!(!blocks.is_empty(), "真实样本必须抽出块");
        // 每一块都必须自洽——这是锚点能不能铸的前提。
        for b in &blocks {
            assert!(b.is_well_formed(), "块 {} runs 未铺满", b.path);
        }
        let total: usize = blocks.iter().map(|b| b.len_utf16()).sum();
        let runs: usize = blocks.iter().map(|b| b.runs.len()).sum();
        let cjk = blocks
            .iter()
            .flat_map(|b| b.raw.chars())
            .filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c))
            .count();
        // 只输出指标，不输出正文。
        eprintln!(
            "REAL DOCX：块={} 总UTF16={} run总数={} 中文字={} 平均run/块={:.2}",
            blocks.len(),
            total,
            runs,
            cjk,
            runs as f64 / blocks.len() as f64
        );
        assert!(cjk > 0, "中文样本必须抽出中文（乱码/未解码会让这条挂掉）");
    }
}

/// 真实样本上的**锚点回源**：在第一次解析上铸锚点，对**独立第二次解析**
/// 的结果取回，逐字比对。
///
/// 跨独立再解析是关键——拿同一次结果自比是恒等式，证不了任何东西
/// （本仓栽过这个跟头）。
#[cfg(test)]
mod real_sample_anchor {
    use super::*;
    use crate::work_blocks::{mint_docx_anchor, resolve_docx_anchor};
    use crate::work_staging::ImportId;

    #[test]
    fn docx_real_sample_anchor_roundtrip() {
        let Ok(path) = std::env::var("WANCODE_DOCX_SAMPLE") else {
            eprintln!("SKIP：未设 WANCODE_DOCX_SAMPLE");
            return;
        };
        let path = std::path::PathBuf::from(path);
        let first = parse_docx(&path, DocxLimits::default()).expect("首次解析");
        let second = parse_docx(&path, DocxLimits::default()).expect("独立第二次解析");
        assert_eq!(first, second, "两次解析必须逐字一致，否则锚点无从谈起");

        let import = ImportId::mint();
        let sha = "0".repeat(64);
        let mut minted = 0usize;
        let mut verbatim = 0usize;
        for b in &first {
            let len = b.len_utf16();
            // 每块取块首、块中、块尾三段（各 ≤40 单元）。
            for (st, en) in [
                (0usize, len.min(40)),
                (len / 2, (len / 2 + 40).min(len)),
                (len.saturating_sub(40), len),
            ] {
                if st >= en {
                    continue;
                }
                let Ok(anchor) = mint_docx_anchor(&first, &b.path, [st, en], import.clone(), &sha)
                else {
                    continue;
                };
                minted += 1;
                // 对**第二次**解析取回。
                if resolve_docx_anchor(&anchor, &second, &sha).is_ok() {
                    verbatim += 1;
                }
            }
        }
        eprintln!("REAL DOCX 锚点：铸造={minted} 跨独立再解析取回成功={verbatim}");
        assert!(minted > 0, "必须铸出锚点，否则这条测试是空转");
        assert_eq!(minted, verbatim, "锚点必须 100% 可回源");
    }
}
