//! DOCX 抽取（产品代码）。W3 spike 的 `extract_docx` productionize。
//!
//! DOCX = zip；正文在 `word/document.xml`。段落 `<w:p>`，其中 `<w:r>` 是
//! run、`<w:t>` 是文本。一句话常被拆进多个 run（格式一变就断开）——这正是
//! 锚点要记 `run_ordinals` 的原因。
//!
//! ## 相对 spike 补上的事
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
//! **④ 按命名空间 URI 认元素，而不是字面前缀 `w:`。** XML 前缀是别名。
//! 合法 WordprocessingML 可以把标准 Word NS 绑到 `word:`、默认 xmlns、或
//! 任何前缀；只匹配 `w:p`/`w:r`/`w:t` 会把整篇抽成 `Ok([])`，worker 再
//! 序列化成空 `ParsedDoc::Docx`。CDATA 同理：只收 `Event::Text` 会把
//! `w:t` 里的 CDATA 丢掉。未见过 Word NS 元素时有序拒收
//! （`UnrecognizedWordprocessing`），绝不把「没认出来」当成空文档。
//!
//! **⑤ 嵌套段落不得覆盖外层 `cur`。** Word 文本框/绘图里的 `<w:p>` 会挂在
//! 外层段落下面；直接 `cur = Some(...)` 会丢掉已经抽出的外层正文，成功返回
//! 只剩内层。开始新段落前先收口当前段；内层结束后若还在外层里，再开一段
//! 承接段尾文本。正文按出现顺序落成块，不静默缺字。
//!
//! **⑥ 实体引用是独立事件，不能被 `_ => {}` 吃掉。** quick-xml 0.41
//! （为修 RUSTSEC-2026-0194/0195 从 0.37 升上来）改了事件模型：`&amp;`、
//! `&#x41;` 这类引用不再由 `Event::Text` 内联展开，而是各发一个
//! `Event::GeneralRef`，`BytesText::unescape` 随之移除。只把 `unescape`
//! 换成 `xml10_content` 而不接 `GeneralRef`，「甲&amp;乙」就会静默抽成
//! 「甲乙」——和 ①/④ 要挡的静默丢正文是同一类事故，只是换了个入口。
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
    /// `document.xml` 里从未出现绑定到 WordprocessingML 标准命名空间的元素。
    /// 常见原因：前缀是别名但解析器按字面 `w:*` 匹配、或根本不是 Word 文档。
    /// 绝不能当成「空文档」成功返回——那会把合法正文静默吃掉。
    UnrecognizedWordprocessing,
    /// XML 在开始标签仍未闭合时耗尽。已经抽出的前缀也不可信，因为文件可能
    /// 缺少任意后续正文，必须整篇拒收。
    TruncatedDocument { pending_chars: usize, open_paragraphs: usize, open_elements: usize },
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
            Self::UnrecognizedWordprocessing => write!(
                f,
                "document.xml 不含 WordprocessingML 标准命名空间元素，拒收（避免把合法正文当成空文档）"
            ),
            Self::TruncatedDocument { pending_chars, open_paragraphs, open_elements } => write!(
                f,
                "document.xml 在元素闭合前结束：未闭合元素 {open_elements} 个、段落 {open_paragraphs} 个、待定正文 {pending_chars} 字符"
            ),
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

/// WordprocessingML 主命名空间。前缀是别名，匹配必须看 URI。
const WORD_NS: &[u8] = b"http://schemas.openxmlformats.org/wordprocessingml/2006/main";

fn bound_wml(ns: &quick_xml::name::ResolveResult<'_>) -> bool {
    matches!(ns, quick_xml::name::ResolveResult::Bound(n) if n.0 == WORD_NS)
}

fn ingest_text(
    s: &str,
    in_text: bool,
    cur: &mut Option<WorkBlock>,
    run_start: Option<usize>,
    dropped_outside_run: &mut usize,
) {
    if in_text {
        match (cur.as_mut(), run_start) {
            // 只在 run 内累计：这样 runs 天然铺满 raw。
            (Some(b), Some(_)) => b.raw.push_str(s),
            // 段落内、run 外，或段落外的 w:t：记账，稍后整篇拒收。
            (Some(_), None) | (None, _) => *dropped_outside_run += s.chars().count(),
        }
    } else if s.chars().any(|c| !c.is_whitespace()) {
        // 不在 w:t 里的非空白文本/CDATA：不是「标签间空白」，不能静默丢掉。
        *dropped_outside_run += s.chars().count();
    }
}

fn close_open_run(cur: &mut Option<WorkBlock>, run_start: &mut Option<usize>) {
    if let (Some(b), Some(st)) = (cur.as_mut(), run_start.take()) {
        let en = b.len_utf16();
        if en > st {
            b.runs.push([st, en]);
        }
    }
}

fn finish_paragraph(
    cur: &mut Option<WorkBlock>,
    para_idx: &mut usize,
    blocks: &mut Vec<WorkBlock>,
    limits: &DocxLimits,
) -> Result<(), DocxError> {
    if let Some(b) = cur.take() {
        if !b.raw.trim().is_empty() {
            if b.len_utf16() > limits.max_block_utf16 {
                return Err(DocxError::BlockTooLong {
                    path: b.path,
                    cap: limits.max_block_utf16,
                });
            }
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
        *para_idx += 1;
    }
    Ok(())
}

/// 与 IO 分离，便于直接喂构造出的 XML 做对抗测试。
pub fn parse_document_xml(xml: &str, limits: DocxLimits) -> Result<Vec<WorkBlock>, DocxError> {
    use quick_xml::events::Event;

    let mut reader = quick_xml::NsReader::from_str(xml);
    let mut buf = Vec::new();
    let mut blocks: Vec<WorkBlock> = Vec::new();
    let mut para_idx = 0usize;
    let mut cur: Option<WorkBlock> = None;
    let mut in_text = false;
    let mut run_start: Option<usize> = None;
    // 嵌套 <w:p>（文本框等）的深度。>0 表示仍在外层段里，结束内层后要
    // 再开一块承接段尾，不能把外层上下文丢掉。
    let mut p_depth = 0usize;
    // run 之外被丢弃的字符数——非零即整篇拒收（见文件头 ①）。
    let mut dropped_outside_run = 0usize;
    // 是否见过绑定到 Word NS 的元素。未见过却返回 Ok([]) 就是把合法正文
    // 静默吃掉（#57 R1-P1）。
    let mut saw_word_ns = false;
    // quick-xml 的 EOF 不等于 XML 结构完整；显式跟踪所有开始/结束元素，
    // 防止截断文件把已解析前缀伪装成完整结果。
    let mut open_elements = 0usize;

    loop {
        match reader.read_resolved_event_into(&mut buf) {
            Ok((_, Event::DocType(_))) => return Err(DocxError::DoctypeRejected),
            Ok((ns, Event::Start(e))) => {
                open_elements += 1;
                let name = e.local_name();
                let local = name.as_ref();
                if bound_wml(&ns) {
                    saw_word_ns = true;
                    match local {
                        b"p" => {
                            if cur.is_some() {
                                close_open_run(&mut cur, &mut run_start);
                                finish_paragraph(
                                    &mut cur,
                                    &mut para_idx,
                                    &mut blocks,
                                    &limits,
                                )?;
                            }
                            cur = Some(WorkBlock {
                                path: format!("body/p[{para_idx}]"),
                                raw: String::new(),
                                runs: Vec::new(),
                            });
                            p_depth += 1;
                        }
                        b"r" => {
                            if let Some(b) = &cur {
                                run_start = Some(b.len_utf16());
                            }
                        }
                        b"t" => in_text = true,
                        _ => {}
                    }
                }
            }
            Ok((ns, Event::Empty(e))) => {
                let name = e.local_name();
                let local = name.as_ref();
                if bound_wml(&ns) {
                    saw_word_ns = true;
                    if local == b"p" {
                        para_idx += 1;
                    }
                }
            }
            Ok((_, Event::Text(t))) => {
                // `xml10_content` = 解码 + XML 1.0 换行规范化。实体展开不在这里
                // 做了，见下面的 `GeneralRef` 分支（文件头 ⑥）。
                let s = t
                    .xml10_content()
                    .map_err(|e| DocxError::XmlError(e.to_string()))?;
                ingest_text(
                    &s,
                    in_text,
                    &mut cur,
                    run_start,
                    &mut dropped_outside_run,
                );
            }
            Ok((_, Event::GeneralRef(r))) => {
                // 事件里装的是引用名（`amp` / `#x41`），不含 `&` 和 `;`。补回定界
                // 符再交给 `escape::unescape`：数值引用它自己算，五个预定义实体
                // 它自己认，其余一律 `UnrecognizedEntity` → 整篇拒收。DOCTYPE 已在
                // 上面拒掉，所以这里不存在自定义实体，fail-closed 是对的口径。
                let name = r
                    .xml10_content()
                    .map_err(|e| DocxError::XmlError(e.to_string()))?;
                let s = quick_xml::escape::unescape(&format!("&{name};"))
                    .map_err(|e| DocxError::XmlError(e.to_string()))?
                    .into_owned();
                ingest_text(
                    &s,
                    in_text,
                    &mut cur,
                    run_start,
                    &mut dropped_outside_run,
                );
            }
            Ok((_, Event::CData(t))) => {
                let s = t
                    .decode()
                    .map_err(|e| DocxError::XmlError(e.to_string()))?;
                ingest_text(
                    &s,
                    in_text,
                    &mut cur,
                    run_start,
                    &mut dropped_outside_run,
                );
            }
            Ok((ns, Event::End(e))) => {
                open_elements = open_elements
                    .checked_sub(1)
                    .ok_or_else(|| DocxError::XmlError("结束标签没有对应的开始标签".into()))?;
                let name = e.local_name();
                let local = name.as_ref();
                if bound_wml(&ns) {
                    match local {
                        b"t" => in_text = false,
                        b"r" => close_open_run(&mut cur, &mut run_start),
                        b"p" => {
                            close_open_run(&mut cur, &mut run_start);
                            finish_paragraph(
                                &mut cur,
                                &mut para_idx,
                                &mut blocks,
                                &limits,
                            )?;
                            p_depth = p_depth.saturating_sub(1);
                            if p_depth > 0 {
                                cur = Some(WorkBlock {
                                    path: format!("body/p[{para_idx}]"),
                                    raw: String::new(),
                                    runs: Vec::new(),
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
            Ok((_, Event::Eof)) => break,
            Err(e) => return Err(DocxError::XmlError(e.to_string())),
            _ => {}
        }
        buf.clear();
    }

    if !saw_word_ns {
        return Err(DocxError::UnrecognizedWordprocessing);
    }
    if open_elements > 0 {
        return Err(DocxError::TruncatedDocument {
            pending_chars: cur.as_ref().map_or(0, |block| block.raw.chars().count()),
            open_paragraphs: p_depth,
            open_elements,
        });
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
    use std::io::Write;

    fn p(inner: &str) -> String {
        format!(
            r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{inner}</w:body></w:document>"#
        )
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

    fn docx_package(document_xml: &str) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut zip = zip::ZipWriter::new(file.reopen().unwrap());
        zip.start_file(
            "word/document.xml",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(document_xml.as_bytes()).unwrap();
        zip.finish().unwrap();
        file
    }

    #[test]
    fn truncated_second_paragraph_is_rejected_instead_of_returning_prefix() {
        let xml = format!(
            r#"<w:document xmlns:w="{WML_NS}"><w:body><w:p><w:r><w:t>完整段</w:t></w:r></w:p><w:p><w:r><w:t>尾段</w:t></w:r>"#
        );
        assert!(matches!(
            parse_document_xml(&xml, DocxLimits::default()),
            Err(DocxError::TruncatedDocument {
                pending_chars: 2,
                open_paragraphs: 1,
                open_elements: 3,
            })
        ));
    }

    #[test]
    fn truncated_only_paragraph_is_rejected_instead_of_empty_ok() {
        let xml = format!(
            r#"<w:document xmlns:w="{WML_NS}"><w:body><w:p><w:r><w:t>全文唯一一段</w:t></w:r>"#
        );
        assert!(matches!(
            parse_document_xml(&xml, DocxLimits::default()),
            Err(DocxError::TruncatedDocument {
                pending_chars: 6,
                open_paragraphs: 1,
                ..
            })
        ));
    }

    #[test]
    fn truncated_docx_package_is_rejected_instead_of_returning_prefix() {
        let file = docx_package(&format!(
            r#"<w:document xmlns:w="{WML_NS}"><w:body><w:p><w:r><w:t>完整段</w:t></w:r></w:p><w:p><w:r><w:t>丢失尾段</w:t></w:r>"#
        ));
        assert!(matches!(
            parse_docx(file.path(), DocxLimits::default()),
            Err(DocxError::TruncatedDocument { .. })
        ));
    }

    #[test]
    fn two_closed_paragraphs_remain_extractable() {
        let xml = p(r#"<w:p><w:r><w:t>第一段</w:t></w:r></w:p><w:p><w:r><w:t>第二段</w:t></w:r></w:p>"#);
        let blocks = parse_document_xml(&xml, DocxLimits::default()).unwrap();
        assert_eq!(blocks.iter().map(|b| b.raw.as_str()).collect::<Vec<_>>(), ["第一段", "第二段"]);
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

    const WML_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

    /// 前缀是别名：`word:` 绑定到标准 Word NS。按字面匹配 `w:p` 会抽成空文档。
    #[test]
    fn alternate_prefix_bound_to_word_ns_is_parsed() {
        let xml = format!(
            r#"<word:document xmlns:word="{WML_NS}"><word:body><word:p><word:r><word:t>你好</word:t></word:r></word:p></word:body></word:document>"#
        );
        let b = parse_document_xml(&xml, DocxLimits::default()).unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].raw, "你好");
        assert_eq!(b[0].runs, vec![[0, 2]]);
        assert!(b[0].is_well_formed());
    }

    /// 默认命名空间绑定到 Word NS、元素无前缀。同样是合法 WordprocessingML。
    #[test]
    fn default_xmlns_word_ns_is_parsed() {
        let xml = format!(
            r#"<document xmlns="{WML_NS}"><body><p><r><t>世界</t></r></p></body></document>"#
        );
        let b = parse_document_xml(&xml, DocxLimits::default()).unwrap();
        assert_eq!(b[0].raw, "世界");
        assert!(b[0].is_well_formed());
    }

    /// CDATA 在 `w:t` 内是正文，不是「不支持的事件」该被丢掉。
    #[test]
    fn cdata_inside_t_is_text_not_dropped() {
        let xml = p(r#"<w:p><w:r><w:t><![CDATA[cdata正文]]></w:t></w:r></w:p>"#);
        let b = parse_document_xml(&xml, DocxLimits::default()).unwrap();
        assert_eq!(b[0].raw, "cdata正文");
        assert_eq!(b[0].runs, vec![[0, 7]]);
        assert!(b[0].is_well_formed());
    }

    /// 不含 Word NS 的 XML 必须有序拒收，绝不能 `Ok([])`。
    #[test]
    fn no_word_namespace_is_rejected_not_empty_ok() {
        let xml = r#"<root><p><r><t>hello</t></r></p></root>"#;
        assert_eq!(
            parse_document_xml(xml, DocxLimits::default()),
            Err(DocxError::UnrecognizedWordprocessing)
        );
    }

    /// `w` 前缀绑到别的 NS：字面 `w:p` 会误抽，按 URI 则应拒收。
    #[test]
    fn wrong_namespace_on_w_prefix_is_rejected() {
        let xml = r#"<w:document xmlns:w="http://example.com/not-word"><w:body><w:p><w:r><w:t>hi</w:t></w:r></w:p></w:body></w:document>"#;
        assert_eq!(
            parse_document_xml(xml, DocxLimits::default()),
            Err(DocxError::UnrecognizedWordprocessing)
        );
    }

    /// Codex #65 R1-P1：嵌套 `<w:p>` 不得覆盖外层段、丢掉已抽出的正文。
    /// 外层段首「甲」、嵌套「乙」、段尾「丙」必须都还在；允许整篇有序拒收，
    /// 禁止 `Ok` 且缺字。
    #[test]
    fn nested_paragraph_does_not_drop_surrounding_text() {
        let before_only = p(
            r#"<w:p><w:r><w:t>甲</w:t></w:r><w:p><w:r><w:t>乙</w:t></w:r></w:p></w:p>"#,
        );
        if let Ok(b) = parse_document_xml(&before_only, DocxLimits::default()) {
            let joined: String = b.iter().map(|x| x.raw.as_str()).collect();
            assert!(
                joined.contains('甲'),
                "嵌套 <p> 覆盖 cur 会丢掉外层段首，得 {joined:?}"
            );
            assert!(joined.contains('乙'), "嵌套段正文必须在，得 {joined:?}");
            assert!(b.iter().all(|x| x.is_well_formed()));
        }
        let before_and_after = p(
            r#"<w:p><w:r><w:t>甲</w:t></w:r><w:p><w:r><w:t>乙</w:t></w:r></w:p><w:r><w:t>丙</w:t></w:r></w:p>"#,
        );
        if let Ok(b) = parse_document_xml(&before_and_after, DocxLimits::default()) {
            let joined: String = b.iter().map(|x| x.raw.as_str()).collect();
            assert!(joined.contains('甲'), "段首不得丢，得 {joined:?}");
            assert!(joined.contains('乙'), "嵌套段不得丢，得 {joined:?}");
            assert!(joined.contains('丙'), "段尾不得丢，得 {joined:?}");
            assert!(b.iter().all(|x| x.is_well_formed()));
        }
    }

    /// quick-xml 0.41 把 `&amp;` 拆成独立的 `GeneralRef` 事件。不接这个事件，
    /// 本例会静默抽成「甲乙」并且 runs 仍然自洽——测不出来。断言落在 raw 上。
    #[test]
    fn predefined_entity_inside_run_is_expanded_not_dropped() {
        let xml = p(r#"<w:p><w:r><w:t>甲&amp;乙</w:t></w:r></w:p>"#);
        let b = parse_document_xml(&xml, DocxLimits::default()).unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].raw, "甲&乙");
        assert_eq!(b[0].runs, vec![[0, 3]], "展开出的字符必须算进 run 区间");
        assert!(b[0].is_well_formed());
    }

    /// 数值字符引用同样走 `GeneralRef`；十进制与十六进制都要认。
    #[test]
    fn numeric_character_references_are_expanded() {
        let xml = p(r#"<w:p><w:r><w:t>&#x41;&#66;</w:t></w:r></w:p>"#);
        let b = parse_document_xml(&xml, DocxLimits::default()).unwrap();
        assert_eq!(b[0].raw, "AB");
        assert_eq!(b[0].runs, vec![[0, 2]]);
    }

    /// 未知实体 → 整篇拒收。DOCTYPE 已被拒，合法 DOCX 里不存在自定义实体；
    /// 静默丢掉一个认不出的引用等于静默改正文。
    #[test]
    fn unknown_entity_is_rejected_not_silently_dropped() {
        let xml = p(r#"<w:p><w:r><w:t>甲&nbsp;乙</w:t></w:r></w:p>"#);
        match parse_document_xml(&xml, DocxLimits::default()) {
            Err(DocxError::XmlError(_)) => {}
            other => panic!("未知实体应拒收，实得 {other:?}"),
        }
    }

    /// run 之外的实体和 run 之外的裸文本同一口径：记账并整篇拒收。
    #[test]
    fn entity_outside_run_is_counted_as_dropped() {
        let xml = p(r#"<w:p><w:t>&amp;</w:t><w:r><w:t>正常</w:t></w:r></w:p>"#);
        match parse_document_xml(&xml, DocxLimits::default()) {
            Err(DocxError::TextOutsideRun { dropped_chars }) => assert_eq!(dropped_chars, 1),
            other => panic!("应拒收 TextOutsideRun，实得 {other:?}"),
        }
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
