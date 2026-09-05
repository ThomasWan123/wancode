//! Safe, read-only text extraction for modern Office containers used by Work.
//!
//! XLSX and PPTX are ZIP packages. We never extract entries to disk, execute
//! macros, follow external relationships, or load embedded objects. Only the
//! worksheet/shared-string XML and slide XML are read, under explicit entry,
//! uncompressed-byte, block-count, and block-size caps.

use std::collections::HashMap;
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
    TooManyEntries {
        entries: usize,
        cap: usize,
    },
    DeclaredSizeOverCap {
        declared: u64,
        cap: u64,
    },
    EntryTooLarge {
        name: String,
        cap: u64,
    },
    UnsafeEntryName(String),
    MissingWorkbook,
    MissingSlides,
    MissingRelationship(String),
    Xml(String),
    DoctypeRejected,
    InvalidCellReference(String),
    InvalidSharedStringIndex(String),
    TooManyBlocks {
        cap: usize,
    },
    BlockTooLong {
        path: String,
        cap: usize,
    },
    TruncatedXml {
        part: &'static str,
        open_elements: usize,
    },
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
            Self::MissingRelationship(id) => write!(f, "Office 关系 {id} 缺失或不安全"),
            Self::Xml(e) => write!(f, "Office XML 解析失败：{e}"),
            Self::DoctypeRejected => write!(f, "Office XML 含 DOCTYPE，拒收"),
            Self::InvalidCellReference(r) => write!(f, "XLSX 单元格坐标非法：{r}"),
            Self::InvalidSharedStringIndex(v) => write!(f, "XLSX 共享字符串索引非法：{v}"),
            Self::TooManyBlocks { cap } => write!(f, "Office 文本块超过上限 {cap}"),
            Self::BlockTooLong { path, cap } => write!(f, "Office 文本块 {path} 超过上限 {cap}"),
            Self::TruncatedXml {
                part,
                open_elements,
            } => write!(
                f,
                "Office XML {part} 在元素闭合前结束（未闭合元素 {open_elements} 个）"
            ),
            Self::NoExtractableText => write!(f, "Office 文件没有可提取文字"),
        }
    }
}

impl std::error::Error for OfficeError {}

fn track_xml_depth(
    event: &Result<quick_xml::events::Event<'_>, quick_xml::Error>,
    open_elements: &mut usize,
) -> Result<(), OfficeError> {
    use quick_xml::events::Event;
    match event {
        Ok(Event::Start(_)) => *open_elements += 1,
        Ok(Event::End(_)) => {
            *open_elements = open_elements
                .checked_sub(1)
                .ok_or_else(|| OfficeError::Xml("结束标签没有对应的开始标签".into()))?;
        }
        _ => {}
    }
    Ok(())
}

fn reject_truncated_xml(part: &'static str, open_elements: usize) -> Result<(), OfficeError> {
    if open_elements > 0 {
        Err(OfficeError::TruncatedXml {
            part,
            open_elements,
        })
    } else {
        Ok(())
    }
}

fn open_package(
    path: &Path,
    limits: OfficeLimits,
) -> Result<zip::ZipArchive<std::fs::File>, OfficeError> {
    let file = std::fs::File::open(path).map_err(|e| OfficeError::NotAZip(e.to_string()))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| OfficeError::NotAZip(e.to_string()))?;
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

fn decoded_attributes(
    reader: &quick_xml::Reader<&[u8]>,
    element: &quick_xml::events::BytesStart<'_>,
) -> Result<Vec<(Vec<u8>, String)>, OfficeError> {
    element
        .attributes()
        .with_checks(false)
        .map(|attribute| {
            let attribute = attribute.map_err(|error| OfficeError::Xml(error.to_string()))?;
            let value = attribute
                .decoded_and_normalized_value(quick_xml::XmlVersion::Implicit1_0, reader.decoder())
                .map_err(|error| OfficeError::Xml(error.to_string()))?
                .into_owned();
            Ok((attribute.key.as_ref().to_vec(), value))
        })
        .collect()
}

fn normalize_relationship_target(base_dir: &str, target: &str) -> Option<String> {
    if target.contains('\\') || target.contains(':') {
        return None;
    }
    let joined = if let Some(absolute) = target.strip_prefix('/') {
        absolute.to_string()
    } else {
        format!("{base_dir}/{target}")
    };
    let mut parts = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            normal => parts.push(normal),
        }
    }
    Some(parts.join("/"))
}

fn parse_relationships(
    xml: &str,
    base_dir: &str,
    allowed_prefix: &str,
    relationship_type_suffix: &str,
) -> Result<HashMap<String, String>, OfficeError> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut relationships = HashMap::new();
    let mut open_elements = 0usize;
    loop {
        let event = reader.read_event();
        track_xml_depth(&event, &mut open_elements)?;
        match event {
            Ok(Event::DocType(_)) => return Err(OfficeError::DoctypeRejected),
            Ok(Event::Start(element) | Event::Empty(element))
                if element.local_name().as_ref() == b"Relationship" =>
            {
                let mut id = None;
                let mut target = None;
                let mut kind = None;
                let mut external = false;
                for (key, value) in decoded_attributes(&reader, &element)? {
                    match key.as_slice() {
                        b"Id" => id = Some(value),
                        b"Target" => target = Some(value),
                        b"Type" => kind = Some(value),
                        b"TargetMode" if value.eq_ignore_ascii_case("external") => external = true,
                        _ => {}
                    }
                }
                if external
                    || !kind
                        .as_deref()
                        .is_some_and(|value| value.ends_with(relationship_type_suffix))
                {
                    continue;
                }
                let id = id.ok_or_else(|| OfficeError::Xml("Office 关系缺少 Id".into()))?;
                let target = target
                    .and_then(|value| normalize_relationship_target(base_dir, &value))
                    .filter(|value| value.starts_with(allowed_prefix) && value.ends_with(".xml"))
                    .ok_or_else(|| OfficeError::MissingRelationship(id.clone()))?;
                if relationships.insert(id.clone(), target).is_some() {
                    return Err(OfficeError::Xml(format!("Office 关系 Id 重复：{id}")));
                }
            }
            Ok(Event::Eof) => {
                reject_truncated_xml("relationships", open_elements)?;
                break;
            }
            Err(error) => return Err(OfficeError::Xml(error.to_string())),
            _ => {}
        }
    }
    Ok(relationships)
}

fn parse_workbook_sheet_refs(xml: &str) -> Result<Vec<(String, String)>, OfficeError> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut sheets = Vec::new();
    let mut open_elements = 0usize;
    loop {
        let event = reader.read_event();
        track_xml_depth(&event, &mut open_elements)?;
        match event {
            Ok(Event::DocType(_)) => return Err(OfficeError::DoctypeRejected),
            Ok(Event::Start(element) | Event::Empty(element))
                if element.local_name().as_ref() == b"sheet" =>
            {
                let mut name = None;
                let mut relationship_id = None;
                for (key, value) in decoded_attributes(&reader, &element)? {
                    if key == b"name" {
                        name = Some(value);
                    } else if key == b"r:id" || key.ends_with(b":id") {
                        relationship_id = Some(value);
                    }
                }
                sheets.push((
                    name.ok_or_else(|| OfficeError::Xml("XLSX sheet 缺少 name".into()))?,
                    relationship_id
                        .ok_or_else(|| OfficeError::Xml("XLSX sheet 缺少 r:id".into()))?,
                ));
            }
            Ok(Event::Eof) => {
                reject_truncated_xml("workbook", open_elements)?;
                break;
            }
            Err(error) => return Err(OfficeError::Xml(error.to_string())),
            _ => {}
        }
    }
    Ok(sheets)
}

fn parse_presentation_slide_refs(xml: &str) -> Result<Vec<String>, OfficeError> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut slides = Vec::new();
    let mut open_elements = 0usize;
    loop {
        let event = reader.read_event();
        track_xml_depth(&event, &mut open_elements)?;
        match event {
            Ok(Event::DocType(_)) => return Err(OfficeError::DoctypeRejected),
            Ok(Event::Start(element) | Event::Empty(element))
                if element.local_name().as_ref() == b"sldId" =>
            {
                let relationship_id = decoded_attributes(&reader, &element)?
                    .into_iter()
                    .find_map(|(key, value)| {
                        (key == b"r:id" || key.ends_with(b":id")).then_some(value)
                    })
                    .ok_or_else(|| OfficeError::Xml("PPTX sldId 缺少 r:id".into()))?;
                slides.push(relationship_id);
            }
            Ok(Event::Eof) => {
                reject_truncated_xml("presentation", open_elements)?;
                break;
            }
            Err(error) => return Err(OfficeError::Xml(error.to_string())),
            _ => {}
        }
    }
    Ok(slides)
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
    let mut open_elements = 0usize;
    loop {
        let event = reader.read_event();
        track_xml_depth(&event, &mut open_elements)?;
        match event {
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
            Ok(Event::Eof) => {
                reject_truncated_xml("sharedStrings", open_elements)?;
                break;
            }
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
    sheet_name: &str,
    shared: &[String],
    blocks: &mut Vec<WorkBlock>,
    limits: OfficeLimits,
) -> Result<(), OfficeError> {
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut cell: Option<Cell> = None;
    let mut field: Option<&'static str> = None;
    let mut ordinal = 0usize;
    let mut open_elements = 0usize;
    loop {
        let event = reader.read_event();
        track_xml_depth(&event, &mut open_elements)?;
        match event {
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
                    format!(
                        "workbook/sheet[{sheet_number}:{}]/cell[{}]",
                        serde_json::to_string(sheet_name)
                            .map_err(|error| OfficeError::Xml(error.to_string()))?,
                        current.reference
                    ),
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
            Ok(Event::Eof) => {
                reject_truncated_xml("worksheet", open_elements)?;
                break;
            }
            Err(e) => return Err(OfficeError::Xml(e.to_string())),
            _ => {}
        }
    }
    Ok(())
}

pub fn parse_xlsx(path: &Path, limits: OfficeLimits) -> Result<Vec<WorkBlock>, OfficeError> {
    let mut archive = open_package(path, limits)?;
    let shared = match read_xml(&mut archive, "xl/sharedStrings.xml", limits.max_xml_bytes)? {
        Some(xml) => parse_shared_strings(&xml)?,
        None => Vec::new(),
    };
    let workbook = read_xml(&mut archive, "xl/workbook.xml", limits.max_xml_bytes)?
        .ok_or(OfficeError::MissingWorkbook)?;
    let sheets = parse_workbook_sheet_refs(&workbook)?;
    if sheets.is_empty() {
        return Err(OfficeError::MissingWorkbook);
    }
    let relationships = read_xml(
        &mut archive,
        "xl/_rels/workbook.xml.rels",
        limits.max_xml_bytes,
    )?
    .ok_or_else(|| OfficeError::MissingRelationship("workbook relationships".into()))?;
    let relationships = parse_relationships(&relationships, "xl", "xl/worksheets/", "/worksheet")?;
    let mut blocks = Vec::new();
    for (index, (sheet_name, relationship_id)) in sheets.into_iter().enumerate() {
        let target = relationships
            .get(&relationship_id)
            .ok_or_else(|| OfficeError::MissingRelationship(relationship_id.clone()))?;
        let xml = read_xml(&mut archive, target, limits.max_xml_bytes)?
            .ok_or(OfficeError::MissingWorkbook)?;
        parse_sheet(&xml, index + 1, &sheet_name, &shared, &mut blocks, limits)?;
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
    let mut open_elements = 0usize;
    loop {
        let event = reader.read_event();
        track_xml_depth(&event, &mut open_elements)?;
        match event {
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
            Ok(Event::Eof) => {
                reject_truncated_xml("slide", open_elements)?;
                break;
            }
            Err(e) => return Err(OfficeError::Xml(e.to_string())),
            _ => {}
        }
    }
    Ok(())
}

pub fn parse_pptx(path: &Path, limits: OfficeLimits) -> Result<Vec<WorkBlock>, OfficeError> {
    let mut archive = open_package(path, limits)?;
    let presentation = read_xml(&mut archive, "ppt/presentation.xml", limits.max_xml_bytes)?
        .ok_or(OfficeError::MissingSlides)?;
    let slides = parse_presentation_slide_refs(&presentation)?;
    if slides.is_empty() {
        return Err(OfficeError::MissingSlides);
    }
    let relationships = read_xml(
        &mut archive,
        "ppt/_rels/presentation.xml.rels",
        limits.max_xml_bytes,
    )?
    .ok_or_else(|| OfficeError::MissingRelationship("presentation relationships".into()))?;
    let relationships = parse_relationships(&relationships, "ppt", "ppt/slides/", "/slide")?;
    let mut blocks = Vec::new();
    for (index, relationship_id) in slides.into_iter().enumerate() {
        let target = relationships
            .get(&relationship_id)
            .ok_or_else(|| OfficeError::MissingRelationship(relationship_id.clone()))?;
        let xml = read_xml(&mut archive, target, limits.max_xml_bytes)?
            .ok_or(OfficeError::MissingSlides)?;
        parse_slide(&xml, index + 1, &mut blocks, limits)?;
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

    fn xlsx_package(
        sheets: &[(&str, &str, &str)],
        parts: &[(&str, &str)],
    ) -> tempfile::NamedTempFile {
        let workbook_sheets = sheets
            .iter()
            .map(|(name, id, _)| format!(r#"<sheet name="{name}" r:id="{id}"/>"#))
            .collect::<String>();
        let relationships = sheets
            .iter()
            .map(|(_, id, target)| {
                format!(
                    r#"<Relationship Id="{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="{target}"/>"#
                )
            })
            .collect::<String>();
        let workbook =
            format!(r#"<workbook xmlns:r="r"><sheets>{workbook_sheets}</sheets></workbook>"#);
        let rels = format!(r#"<Relationships>{relationships}</Relationships>"#);
        let mut entries = vec![
            ("xl/workbook.xml", workbook.as_str()),
            ("xl/_rels/workbook.xml.rels", rels.as_str()),
        ];
        entries.extend_from_slice(parts);
        package(&entries)
    }

    fn pptx_package(slides: &[(&str, &str)], parts: &[(&str, &str)]) -> tempfile::NamedTempFile {
        let slide_ids = slides
            .iter()
            .map(|(id, _)| format!(r#"<p:sldId r:id="{id}"/>"#))
            .collect::<String>();
        let relationships = slides
            .iter()
            .map(|(id, target)| {
                format!(
                    r#"<Relationship Id="{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="{target}"/>"#
                )
            })
            .collect::<String>();
        let presentation = format!(
            r#"<p:presentation xmlns:p="p" xmlns:r="r"><p:sldIdLst>{slide_ids}</p:sldIdLst></p:presentation>"#
        );
        let rels = format!(r#"<Relationships>{relationships}</Relationships>"#);
        let mut entries = vec![
            ("ppt/presentation.xml", presentation.as_str()),
            ("ppt/_rels/presentation.xml.rels", rels.as_str()),
        ];
        entries.extend_from_slice(parts);
        package(&entries)
    }

    #[test]
    fn xlsx_extracts_shared_inline_numeric_and_formula_cells() {
        let file = xlsx_package(
            &[("Budget", "rId1", "worksheets/sheet1.xml")],
            &[
                (
                    "xl/sharedStrings.xml",
                    r#"<sst><si><t>项目甲</t></si></sst>"#,
                ),
                (
                    "xl/worksheets/sheet1.xml",
                    r#"<worksheet><sheetData><row><c r="A1" t="s"><v>0</v></c><c r="B1" t="inlineStr"><is><t>状态</t></is></c><c r="C1"><f>1+1</f><v>2</v></c></row></sheetData></worksheet>"#,
                ),
            ],
        );
        let blocks = parse_xlsx(file.path(), OfficeLimits::default()).unwrap();
        assert_eq!(
            blocks.iter().map(|b| b.raw.as_str()).collect::<Vec<_>>(),
            ["项目甲", "状态", "=1+1 -> 2"]
        );
        assert!(blocks.iter().all(WorkBlock::is_well_formed));
        assert!(blocks[0].path.contains("Budget"));
    }

    #[test]
    fn pptx_uses_presentation_order_and_ignores_orphan_parts() {
        let file = pptx_package(
            &[
                ("rId10", "slides/slide10.xml"),
                ("rId2", "slides/slide2.xml"),
            ],
            &[
                (
                    "ppt/slides/slide10.xml",
                    r#"<p:sld xmlns:p="p" xmlns:a="a"><a:t>十</a:t></p:sld>"#,
                ),
                (
                    "ppt/slides/slide2.xml",
                    r#"<p:sld xmlns:p="p" xmlns:a="a"><a:t>二</a:t></p:sld>"#,
                ),
                (
                    "ppt/slides/slide999.xml",
                    r#"<p:sld xmlns:p="p" xmlns:a="a"><a:t>孤儿内容</a:t></p:sld>"#,
                ),
            ],
        );
        let blocks = parse_pptx(file.path(), OfficeLimits::default()).unwrap();
        assert_eq!(
            blocks.iter().map(|b| b.raw.as_str()).collect::<Vec<_>>(),
            ["十", "二"]
        );
        assert_eq!(blocks[0].path, "slides/slide[1]/text[0]");
        assert!(blocks.iter().all(|block| !block.raw.contains("孤儿")));
    }

    #[test]
    fn truncated_xlsx_cell_rejects_the_whole_document() {
        let file = xlsx_package(
            &[("Budget", "rId1", "worksheets/sheet1.xml")],
            &[(
                "xl/worksheets/sheet1.xml",
                r#"<worksheet><c r="A1"><v>kept-prefix</v></c><c r="B1"><v>lost-tail</v>"#,
            )],
        );
        assert!(matches!(
            parse_xlsx(file.path(), OfficeLimits::default()),
            Err(OfficeError::TruncatedXml {
                part: "worksheet",
                ..
            })
        ));
    }

    #[test]
    fn truncated_pptx_text_rejects_the_whole_document() {
        let file = pptx_package(
            &[("rId1", "slides/slide1.xml")],
            &[(
                "ppt/slides/slide1.xml",
                r#"<p:sld><a:t>kept-prefix</a:t><a:t>lost-tail"#,
            )],
        );
        assert!(matches!(
            parse_pptx(file.path(), OfficeLimits::default()),
            Err(OfficeError::TruncatedXml { part: "slide", .. })
        ));
    }

    #[test]
    fn truncated_shared_string_table_is_rejected() {
        assert!(matches!(
            parse_shared_strings("<sst><si><t>lost-tail</t>"),
            Err(OfficeError::TruncatedXml {
                part: "sharedStrings",
                ..
            })
        ));
    }

    #[test]
    fn truncated_workbook_index_is_rejected() {
        assert!(matches!(
            parse_workbook_sheet_refs(
                r#"<workbook xmlns:r="r"><sheets><sheet name="kept" r:id="rId1"/><sheet name="lost" r:id="rId2">"#,
            ),
            Err(OfficeError::TruncatedXml {
                part: "workbook",
                ..
            })
        ));
    }

    #[test]
    fn truncated_presentation_index_is_rejected() {
        assert!(matches!(
            parse_presentation_slide_refs(
                r#"<p:presentation xmlns:p="p" xmlns:r="r"><p:sldIdLst><p:sldId r:id="rId1"/><p:sldId r:id="rId2">"#,
            ),
            Err(OfficeError::TruncatedXml {
                part: "presentation",
                ..
            })
        ));
    }

    #[test]
    fn truncated_relationship_index_is_rejected() {
        assert!(matches!(
            parse_relationships(
                r#"<Relationships><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml">"#,
                "xl",
                "xl/worksheets/",
                "/worksheet",
            ),
            Err(OfficeError::TruncatedXml {
                part: "relationships",
                ..
            })
        ));
    }

    #[test]
    fn unmatched_end_tag_remains_fail_closed() {
        assert!(matches!(
            parse_workbook_sheet_refs("</workbook>"),
            Err(OfficeError::Xml(_))
        ));
    }

    #[test]
    fn xlsx_uses_workbook_order_names_and_ignores_orphan_parts() {
        let file = xlsx_package(
            &[
                ("Second", "rId2", "worksheets/sheet2.xml"),
                ("First", "rId1", "worksheets/sheet1.xml"),
            ],
            &[
                (
                    "xl/worksheets/sheet1.xml",
                    r#"<worksheet><c r="A1"><v>first</v></c></worksheet>"#,
                ),
                (
                    "xl/worksheets/sheet2.xml",
                    r#"<worksheet><c r="A1"><v>second</v></c></worksheet>"#,
                ),
                (
                    "xl/worksheets/sheet999.xml",
                    r#"<worksheet><c r="A1"><v>orphan</v></c></worksheet>"#,
                ),
            ],
        );
        let blocks = parse_xlsx(file.path(), OfficeLimits::default()).unwrap();
        assert_eq!(
            blocks
                .iter()
                .map(|block| block.raw.as_str())
                .collect::<Vec<_>>(),
            ["second", "first"]
        );
        assert!(blocks[0].path.contains("Second"));
        assert!(blocks[1].path.contains("First"));
        assert!(blocks.iter().all(|block| block.raw != "orphan"));
    }

    #[test]
    fn office_doctype_is_rejected() {
        let file = pptx_package(
            &[("rId1", "slides/slide1.xml")],
            &[(
                "ppt/slides/slide1.xml",
                "<!DOCTYPE x><p:sld><a:t>x</a:t></p:sld>",
            )],
        );
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

        let no_slide_text = pptx_package(
            &[("rId1", "slides/slide1.xml")],
            &[("ppt/slides/slide1.xml", "<p:sld/>")],
        );
        assert_eq!(
            parse_pptx(no_slide_text.path(), OfficeLimits::default()),
            Err(OfficeError::NoExtractableText)
        );
    }

    #[test]
    fn malformed_shared_string_and_block_flood_are_rejected() {
        let bad_shared_index = xlsx_package(
            &[("Sheet 1", "rId1", "worksheets/sheet1.xml")],
            &[(
                "xl/worksheets/sheet1.xml",
                r#"<worksheet><c r="A1" t="s"><v>9</v></c></worksheet>"#,
            )],
        );
        assert!(matches!(
            parse_xlsx(bad_shared_index.path(), OfficeLimits::default()),
            Err(OfficeError::InvalidSharedStringIndex(_))
        ));

        let two_cells = xlsx_package(
            &[("Sheet 1", "rId1", "worksheets/sheet1.xml")],
            &[(
                "xl/worksheets/sheet1.xml",
                r#"<worksheet><c r="A1"><v>1</v></c><c r="A2"><v>2</v></c></worksheet>"#,
            )],
        );
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
