//! Safe, read-only text extraction for modern Office containers used by Work.
//!
//! XLSX and PPTX are ZIP packages. We never extract entries to disk, execute
//! macros, follow external relationships, or load embedded objects. Only the
//! worksheet/shared-string XML and slide XML are read, under explicit entry,
//! uncompressed-byte, block-count, and block-size caps.

use std::io::Read;
use std::path::Path;

use crate::work_anchor::utf16_len;
use crate::work_blocks::WorkBlock;

#[derive(Debug, Clone, Copy)]
pub struct OfficeLimits {
    pub max_entries: usize,
    pub max_total_uncompressed: u64,
    pub max_xml_bytes: u64,
    pub max_blocks: usize,
    pub max_block_utf16: usize,
}

impl Default for OfficeLimits {
    fn default() -> Self {
        Self {
            max_entries: 10_000,
            max_total_uncompressed: 128 * 1024 * 1024,
            max_xml_bytes: 32 * 1024 * 1024,
            max_blocks: 200_000,
            max_block_utf16: 256 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfficeError {
    NotAZip(String),
    TooManyEntries { entries: usize, cap: usize },
    DeclaredSizeOverCap { declared: u64, cap: u64 },
    EntryTooLarge { name: String, cap: u64 },
    UnsafeEntryName(String),
    MissingWorkbook,
    MissingSlides,
    Xml(String),
    DoctypeRejected,
    InvalidCellReference(String),
    InvalidSharedStringIndex(String),
    TooManyBlocks { cap: usize },
    BlockTooLong { path: String, cap: usize },
    NoExtractableText,
}

impl std::fmt::Display for OfficeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAZip(e) => write!(f, "不是合法 Office ZIP：{e}"),
            Self::TooManyEntries { entries, cap } => {
                write!(f, "Office 包含 {entries} 个条目，超过上限 {cap}")
            }
            Self::DeclaredSizeOverCap { declared, cap } => {
                write!(f, "Office 声明解压后体积 {declared} 超过上限 {cap}")
            }
            Self::EntryTooLarge { name, cap } => write!(f, "Office 条目 {name} 超过上限 {cap}"),
            Self::UnsafeEntryName(name) => write!(f, "Office 条目路径不安全：{name}"),
            Self::MissingWorkbook => write!(f, "XLSX 不含任何工作表"),
            Self::MissingSlides => write!(f, "PPTX 不含任何幻灯片"),
            Self::Xml(e) => write!(f, "Office XML 解析失败：{e}"),
            Self::DoctypeRejected => write!(f, "Office XML 含 DOCTYPE，拒收"),
            Self::InvalidCellReference(r) => write!(f, "XLSX 单元格坐标非法：{r}"),
            Self::InvalidSharedStringIndex(v) => write!(f, "XLSX 共享字符串索引非法：{v}"),
            Self::TooManyBlocks { cap } => write!(f, "Office 文本块超过上限 {cap}"),
            Self::BlockTooLong { path, cap } => write!(f, "Office 文本块 {path} 超过上限 {cap}"),
            Self::NoExtractableText => write!(f, "Office 文件没有可提取文字"),
        }
    }
}

impl std::error::Error for OfficeError {}

fn open_package(
    path: &Path,
    limits: OfficeLimits,
) -> Result<zip::ZipArchive<std::fs::File>, OfficeError> {
    let file = std::fs::File::open(path).map_err(|e| OfficeError::NotAZip(e.to_string()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| OfficeError::NotAZip(e.to_string()))?;
    if archive.len() > limits.max_entries {
        return Err(OfficeError::TooManyEntries {
            entries: archive.len(),
            cap: limits.max_entries,
        });
    }
    let mut declared = 0u64;
    for entry in archive.file_names() {
        if entry.split('/').any(|part| part == "..") || entry.contains('\\') {
            return Err(OfficeError::UnsafeEntryName(entry.to_string()));
        }
    }
    for index in 0..archive.len() {
        declared = declared.saturating_add(
            archive
                .by_index_raw(index)
                .map_err(|e| OfficeError::NotAZip(e.to_string()))?
                .size(),
        );
    }
    if declared > limits.max_total_uncompressed {
        return Err(OfficeError::DeclaredSizeOverCap {
            declared,
            cap: limits.max_total_uncompressed,
        });
    }
    Ok(archive)
}

fn read_xml(
    archive: &mut zip::ZipArchive<std::fs::File>,
    name: &str,
    cap: u64,
) -> Result<Option<String>, OfficeError> {
    let mut entry = match archive.by_name(name) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(OfficeError::NotAZip(e.to_string())),
    };
    if entry.size() > cap {
        return Err(OfficeError::EntryTooLarge {
            name: name.to_string(),
            cap,
        });
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry
        .by_ref()
        .take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| OfficeError::NotAZip(e.to_string()))?;
    if bytes.len() as u64 > cap {
        return Err(OfficeError::EntryTooLarge {
            name: name.to_string(),
            cap,
        });
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|e| OfficeError::Xml(format!("{name} 非 UTF-8：{e}")))
}

fn append_event_text(
    raw: &mut String,
    event: quick_xml::events::Event<'_>,
) -> Result<(), OfficeError> {
    use quick_xml::events::Event;
    match event {
        Event::Text(t) => raw.push_str(
            &t.xml10_content()
                .map_err(|e| OfficeError::Xml(e.to_string()))?,
        ),
        Event::CData(t) => raw.push_str(&t.decode().map_err(|e| OfficeError::Xml(e.to_string()))?),
        Event::GeneralRef(r) => {
            let name = r
                .xml10_content()
                .map_err(|e| OfficeError::Xml(e.to_string()))?;
            raw.push_str(
                &quick_xml::escape::unescape(&format!("&{name};"))
                    .map_err(|e| OfficeError::Xml(e.to_string()))?,
            );
        }
        _ => {}
    }
    Ok(())
}

fn push_block(
    blocks: &mut Vec<WorkBlock>,
    path: String,
    raw: String,
    limits: OfficeLimits,
) -> Result<(), OfficeError> {
    let raw = raw.trim().to_string();
    if raw.is_empty() {
        return Ok(());
    }
    if blocks.len() >= limits.max_blocks {
        return Err(OfficeError::TooManyBlocks {
            cap: limits.max_blocks,
        });
    }
    let len = utf16_len(&raw);
    if len > limits.max_block_utf16 {
        return Err(OfficeError::BlockTooLong {
            path,
            cap: limits.max_block_utf16,
        });
    }
    blocks.push(WorkBlock {
        path,
        raw,
        runs: vec![[0, len]],
    });
    Ok(())
}

fn parse_shared_strings(xml: &str) -> Result<Vec<String>, OfficeError> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut strings = Vec::new();
    let mut current: Option<String> = None;
    let mut in_text = false;
    loop {
        match reader.read_event() {
            Ok(Event::DocType(_)) => return Err(OfficeError::DoctypeRejected),
            Ok(Event::Start(e)) if e.local_name().as_ref() == b"si" => {
                current = Some(String::new())
            }
            Ok(Event::Start(e)) if e.local_name().as_ref() == b"t" && current.is_some() => {
                in_text = true
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == b"t" => in_text = false,
            Ok(Event::End(e)) if e.local_name().as_ref() == b"si" => {
                strings.push(current.take().unwrap_or_default())
            }
            Ok(event @ (Event::Text(_) | Event::CData(_) | Event::GeneralRef(_))) if in_text => {
                append_event_text(current.as_mut().expect("in_text requires current"), event)?;
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(OfficeError::Xml(e.to_string())),
            _ => {}
        }
    }
    Ok(strings)
}

#[derive(Default)]
struct Cell {
    reference: String,
    kind: String,
    value: String,
    inline: String,
    formula: String,
}

fn valid_cell_ref(reference: &str) -> bool {
    !reference.is_empty()
        && reference
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'$')
}

fn parse_sheet(
    xml: &str,
    sheet_number: usize,
    shared: &[String],
    blocks: &mut Vec<WorkBlock>,
    limits: OfficeLimits,
) -> Result<(), OfficeError> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut cell: Option<Cell> = None;
    let mut field: Option<&'static str> = None;
    let mut ordinal = 0usize;
    loop {
        match reader.read_event() {
            Ok(Event::DocType(_)) => return Err(OfficeError::DoctypeRejected),
            Ok(Event::Start(e)) if e.local_name().as_ref() == b"c" => {
                let mut next = Cell::default();
                for attr in e.attributes().with_checks(false) {
                    let attr = attr.map_err(|e| OfficeError::Xml(e.to_string()))?;
                    let value = attr
                        .decoded_and_normalized_value(
                            quick_xml::XmlVersion::Implicit1_0,
                            reader.decoder(),
                        )
                        .map_err(|e| OfficeError::Xml(e.to_string()))?
                        .into_owned();
                    match attr.key.as_ref() {
                        b"r" => next.reference = value,
                        b"t" => next.kind = value,
                        _ => {}
                    }
                }
                ordinal += 1;
                if next.reference.is_empty() {
                    next.reference = format!("ordinal-{ordinal}");
                }
                if !valid_cell_ref(&next.reference) && !next.reference.starts_with("ordinal-") {
                    return Err(OfficeError::InvalidCellReference(next.reference));
                }
                cell = Some(next);
            }
            Ok(Event::Start(e)) if cell.is_some() => {
                field = match e.local_name().as_ref() {
                    b"v" => Some("value"),
                    b"t" => Some("inline"),
                    b"f" => Some("formula"),
                    _ => field,
                };
            }
            Ok(Event::End(e))
                if e.local_name().as_ref() == b"v"
                    || e.local_name().as_ref() == b"t"
                    || e.local_name().as_ref() == b"f" =>
            {
                field = None
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == b"c" => {
                let current = cell.take().unwrap_or_default();
                let value = if current.kind == "s" {
                    let index = current.value.trim().parse::<usize>().map_err(|_| {
                        OfficeError::InvalidSharedStringIndex(current.value.clone())
                    })?;
                    shared.get(index).cloned().ok_or_else(|| {
                        OfficeError::InvalidSharedStringIndex(current.value.clone())
                    })?
                } else if !current.inline.is_empty() {
                    current.inline
                } else {
                    current.value
                };
                let raw = if current.formula.trim().is_empty() {
                    value
                } else if value.trim().is_empty() {
                    format!("={}", current.formula.trim())
                } else {
                    format!("={} -> {}", current.formula.trim(), value.trim())
                };
                push_block(
                    blocks,
                    format!("workbook/sheet[{sheet_number}]/cell[{}]", current.reference),
                    raw,
                    limits,
                )?;
                field = None;
            }
            Ok(event @ (Event::Text(_) | Event::CData(_) | Event::GeneralRef(_))) => {
                if let (Some(current), Some(target)) = (cell.as_mut(), field) {
                    match target {
                        "value" => append_event_text(&mut current.value, event)?,
                        "inline" => append_event_text(&mut current.inline, event)?,
                        "formula" => append_event_text(&mut current.formula, event)?,
                        _ => unreachable!(),
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(OfficeError::Xml(e.to_string())),
            _ => {}
        }
    }
    Ok(())
}

fn numbered_entries(
    names: impl Iterator<Item = String>,
    prefix: &str,
    suffix: &str,
) -> Vec<(usize, String)> {
    let mut entries: Vec<_> = names
        .filter_map(|name| {
            let number = name
                .strip_prefix(prefix)?
                .strip_suffix(suffix)?
                .parse::<usize>()
                .ok()?;
            Some((number, name))
        })
        .collect();
    entries.sort_by_key(|(number, _)| *number);
    entries
}

pub fn parse_xlsx(path: &Path, limits: OfficeLimits) -> Result<Vec<WorkBlock>, OfficeError> {
    let mut archive = open_package(path, limits)?;
    let shared = match read_xml(&mut archive, "xl/sharedStrings.xml", limits.max_xml_bytes)? {
        Some(xml) => parse_shared_strings(&xml)?,
        None => Vec::new(),
    };
    let sheets = numbered_entries(
        archive.file_names().map(str::to_string),
        "xl/worksheets/sheet",
        ".xml",
    );
    if sheets.is_empty() {
        return Err(OfficeError::MissingWorkbook);
    }
    let mut blocks = Vec::new();
    for (number, name) in sheets {
        let xml = read_xml(&mut archive, &name, limits.max_xml_bytes)?
            .ok_or(OfficeError::MissingWorkbook)?;
        parse_sheet(&xml, number, &shared, &mut blocks, limits)?;
    }
    if blocks.is_empty() {
        Err(OfficeError::NoExtractableText)
    } else {
        Ok(blocks)
    }
}

fn parse_slide(
    xml: &str,
    slide_number: usize,
    blocks: &mut Vec<WorkBlock>,
    limits: OfficeLimits,
) -> Result<(), OfficeError> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut in_text = false;
    let mut current = String::new();
    let mut text_index = 0usize;
    loop {
        match reader.read_event() {
            Ok(Event::DocType(_)) => return Err(OfficeError::DoctypeRejected),
            Ok(Event::Start(e)) if e.local_name().as_ref() == b"t" => {
                in_text = true;
                current.clear();
            }
            Ok(Event::End(e)) if e.local_name().as_ref() == b"t" => {
                in_text = false;
                push_block(
                    blocks,
                    format!("slides/slide[{slide_number}]/text[{text_index}]"),
                    std::mem::take(&mut current),
                    limits,
                )?;
                text_index += 1;
            }
            Ok(event @ (Event::Text(_) | Event::CData(_) | Event::GeneralRef(_))) if in_text => {
                append_event_text(&mut current, event)?
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(OfficeError::Xml(e.to_string())),
            _ => {}
        }
    }
    Ok(())
}

pub fn parse_pptx(path: &Path, limits: OfficeLimits) -> Result<Vec<WorkBlock>, OfficeError> {
    let mut archive = open_package(path, limits)?;
    let slides = numbered_entries(
        archive.file_names().map(str::to_string),
        "ppt/slides/slide",
        ".xml",
    );
    if slides.is_empty() {
        return Err(OfficeError::MissingSlides);
    }
    let mut blocks = Vec::new();
    for (number, name) in slides {
        let xml = read_xml(&mut archive, &name, limits.max_xml_bytes)?
            .ok_or(OfficeError::MissingSlides)?;
        parse_slide(&xml, number, &mut blocks, limits)?;
    }
    if blocks.is_empty() {
        Err(OfficeError::NoExtractableText)
    } else {
        Ok(blocks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn package(entries: &[(&str, &str)]) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut zip = zip::ZipWriter::new(file.reopen().unwrap());
        for (name, body) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        file
    }

    #[test]
    fn xlsx_extracts_shared_inline_numeric_and_formula_cells() {
        let file = package(&[
            (
                "xl/sharedStrings.xml",
                r#"<sst><si><t>项目甲</t></si></sst>"#,
            ),
            (
                "xl/worksheets/sheet1.xml",
                r#"<worksheet><sheetData><row><c r="A1" t="s"><v>0</v></c><c r="B1" t="inlineStr"><is><t>状态</t></is></c><c r="C1"><f>1+1</f><v>2</v></c></row></sheetData></worksheet>"#,
            ),
        ]);
        let blocks = parse_xlsx(file.path(), OfficeLimits::default()).unwrap();
        assert_eq!(
            blocks.iter().map(|b| b.raw.as_str()).collect::<Vec<_>>(),
            ["项目甲", "状态", "=1+1 -> 2"]
        );
        assert!(blocks.iter().all(WorkBlock::is_well_formed));
    }

    #[test]
    fn pptx_extracts_slide_text_in_numeric_slide_order() {
        let file = package(&[
            (
                "ppt/slides/slide10.xml",
                r#"<p:sld xmlns:p="p" xmlns:a="a"><a:t>十</a:t></p:sld>"#,
            ),
            (
                "ppt/slides/slide2.xml",
                r#"<p:sld xmlns:p="p" xmlns:a="a"><a:t>二</a:t></p:sld>"#,
            ),
        ]);
        let blocks = parse_pptx(file.path(), OfficeLimits::default()).unwrap();
        assert_eq!(
            blocks.iter().map(|b| b.raw.as_str()).collect::<Vec<_>>(),
            ["二", "十"]
        );
        assert_eq!(blocks[0].path, "slides/slide[2]/text[0]");
    }

    #[test]
    fn office_doctype_is_rejected() {
        let file = package(&[(
            "ppt/slides/slide1.xml",
            "<!DOCTYPE x><p:sld><a:t>x</a:t></p:sld>",
        )]);
        assert_eq!(
            parse_pptx(file.path(), OfficeLimits::default()),
            Err(OfficeError::DoctypeRejected)
        );
    }

    #[test]
    fn package_resource_caps_and_missing_content_fail_closed() {
        let two_entries = package(&[("a.xml", "a"), ("b.xml", "b")]);
        let limits = OfficeLimits {
            max_entries: 1,
            ..OfficeLimits::default()
        };
        assert!(matches!(
            open_package(two_entries.path(), limits),
            Err(OfficeError::TooManyEntries { .. })
        ));

        let declared = package(&[("xl/worksheets/sheet1.xml", "123456")]);
        let limits = OfficeLimits {
            max_total_uncompressed: 5,
            ..OfficeLimits::default()
        };
        assert!(matches!(
            parse_xlsx(declared.path(), limits),
            Err(OfficeError::DeclaredSizeOverCap { .. })
        ));

        let no_sheets = package(&[("xl/workbook.xml", "<workbook/>")]);
        assert_eq!(
            parse_xlsx(no_sheets.path(), OfficeLimits::default()),
            Err(OfficeError::MissingWorkbook)
        );

        let no_slide_text = package(&[("ppt/slides/slide1.xml", "<p:sld/>")]);
        assert_eq!(
            parse_pptx(no_slide_text.path(), OfficeLimits::default()),
            Err(OfficeError::NoExtractableText)
        );
    }

    #[test]
    fn malformed_shared_string_and_block_flood_are_rejected() {
        let bad_shared_index = package(&[(
            "xl/worksheets/sheet1.xml",
            r#"<worksheet><c r="A1" t="s"><v>9</v></c></worksheet>"#,
        )]);
        assert!(matches!(
            parse_xlsx(bad_shared_index.path(), OfficeLimits::default()),
            Err(OfficeError::InvalidSharedStringIndex(_))
        ));

        let two_cells = package(&[(
            "xl/worksheets/sheet1.xml",
            r#"<worksheet><c r="A1"><v>1</v></c><c r="A2"><v>2</v></c></worksheet>"#,
        )]);
        let limits = OfficeLimits {
            max_blocks: 1,
            ..OfficeLimits::default()
        };
        assert!(matches!(
            parse_xlsx(two_cells.path(), limits),
            Err(OfficeError::TooManyBlocks { .. })
        ));
    }
}
