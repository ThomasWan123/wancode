//! W3 功能面 spike：DOCX 抽取 + 锚点可回源（真实样本）。
//!
//! W1 只证了**安全面**（结构对抗输入 fail-closed），并把「真实样本抽取率、
//! DOCX 段落锚点 / run 拆分、良性 DOCX 正对照」明确列为 NOT-RUN，且说
//! 「pdfium→纯 Rust 选型是**建议**，待功能面 spike 后落地」。本 spike 就是
//! 那个功能面：用**真实 .docx** 回答两个问题——
//!
//!   ① 抽取率：能否拿到正文文本（非空、段落数合理、中文不乱码）；
//!   ② 锚点可回源：按 `block_path` + `run_ordinals` + `raw_range` 取回的
//!      文本是否与原文**逐字一致**（这是 W3 验收的硬指标）。
//!
//! 坐标系与 wancode 的 `work_anchor` 一致：UTF-16 code unit、半开 0 基、
//! 寻址 raw 抽取文本。
//!
//! 样本**只在本地读取**，内容绝不写进证据文件（隐私）：输出只含计数、
//! 长度、是否逐字一致等指标。

use std::io::Read;

/// 一个块（段落）的抽取结果：raw 文本 + 每个 run 在 raw 中的 UTF-16 区间。
#[derive(Debug)]
struct Block {
    /// 形如 `body/p[3]`（0 基序号，与 wancode 锚点契约同形）。
    path: String,
    /// 该段落拼接后的 raw 文本。
    raw: String,
    /// 第 i 个 run 覆盖 raw 的 [start, end)（UTF-16 单元）。
    runs: Vec<(usize, usize)>,
}

fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// 按 UTF-16 半开区间切片（与 work_anchor::utf16_slice 同语义，代理对不劈开）。
fn utf16_slice(s: &str, start: usize, end: usize) -> Option<String> {
    if start > end || end > utf16_len(s) {
        return None;
    }
    let mut units = 0usize;
    let mut out = String::new();
    let mut started = false;
    for ch in s.chars() {
        let w = ch.len_utf16();
        if !started {
            if units == start {
                started = true;
            } else if units < start && start < units + w {
                return None; // 劈开代理对
            }
        }
        if started {
            if units == end {
                return Some(out);
            }
            if units < end && end < units + w {
                return None;
            }
            out.push(ch);
        }
        units += w;
    }
    if started || start == utf16_len(s) {
        Some(out)
    } else {
        None
    }
}

/// 从 .docx 抽出块与 run 结构。
///
/// DOCX = zip；正文在 `word/document.xml`。段落 `<w:p>`，其中 `<w:r>` 是
/// run、`<w:t>` 是文本。一句话常被拆进多个 run（格式变化即断开）——这正是
/// 锚点要用 `run_ordinals` 记录的原因。
fn extract_docx(path: &std::path::Path) -> Result<Vec<Block>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("打开失败: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("非法 zip: {e}"))?;
    let mut xml = String::new();
    {
        let mut entry = zip
            .by_name("word/document.xml")
            .map_err(|e| format!("缺 word/document.xml: {e}"))?;
        entry
            .read_to_string(&mut xml)
            .map_err(|e| format!("读取 document.xml 失败: {e}"))?;
    }

    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(&xml);
    let mut buf = Vec::new();
    let mut blocks: Vec<Block> = Vec::new();
    let mut para_idx = 0usize;
    let mut cur: Option<Block> = None;
    let mut in_text = false;
    let mut run_start: Option<usize> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match e.name().as_ref() {
                b"w:p" => {
                    cur = Some(Block {
                        path: format!("body/p[{para_idx}]"),
                        raw: String::new(),
                        runs: Vec::new(),
                    });
                }
                b"w:r" => {
                    if let Some(b) = &cur {
                        run_start = Some(utf16_len(&b.raw));
                    }
                }
                b"w:t" => in_text = true,
                _ => {}
            },
            Ok(Event::Text(t)) => {
                if in_text {
                    if let Some(b) = &mut cur {
                        let s = t.unescape().map_err(|e| format!("解码失败: {e}"))?;
                        b.raw.push_str(&s);
                    }
                }
            }
            Ok(Event::End(e)) => match e.name().as_ref() {
                b"w:t" => in_text = false,
                b"w:r" => {
                    if let (Some(b), Some(st)) = (&mut cur, run_start.take()) {
                        let en = utf16_len(&b.raw);
                        if en > st {
                            b.runs.push((st, en));
                        }
                    }
                }
                b"w:p" => {
                    if let Some(b) = cur.take() {
                        if !b.raw.trim().is_empty() {
                            blocks.push(b);
                        }
                        para_idx += 1;
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("XML 解析失败: {e}")),
            _ => {}
        }
        buf.clear();
    }
    Ok(blocks)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(path) = args.get(1) else {
        eprintln!("用法: w3_docx_probe <sample.docx>");
        std::process::exit(2);
    };
    let path = std::path::Path::new(path);

    let blocks = match extract_docx(path) {
        Ok(b) => b,
        Err(e) => {
            println!(
                "{}",
                serde_json::json!({ "artifact": "w3-docx-functional", "ok": false, "error": e })
            );
            std::process::exit(1);
        }
    };

    // ① 抽取指标（不含任何正文内容）。
    let total_utf16: usize = blocks.iter().map(|b| utf16_len(&b.raw)).sum();
    let total_runs: usize = blocks.iter().map(|b| b.runs.len()).sum();
    let multi_run_blocks = blocks.iter().filter(|b| b.runs.len() > 1).count();
    let has_cjk = blocks
        .iter()
        .any(|b| b.raw.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)));

    // ② 锚点回源逐字一致 —— **必须跨一次独立重解析**。
    //
    // 起初我写成「同一函数同一输入算两遍再比较」，那是恒等式，什么也证明不了。
    // 真正要证的是：把锚点（block_path + raw_range + excerpt）落下来之后，
    // **重新解析同一文件**，仍能按它取回**逐字相同**的文本。这同时覆盖了
    // 「解析是否确定性」——若两次抽取有任何漂移，这里会失败。
    let anchors: Vec<(String, usize, usize, String)> = blocks
        .iter()
        .flat_map(|b| {
            b.runs.iter().filter_map(move |&(st, en)| {
                utf16_slice(&b.raw, st, en).map(|ex| (b.path.clone(), st, en, ex))
            })
        })
        .filter(|(_, _, _, ex)| !ex.is_empty())
        .collect();
    // 跨 run 的整段锚点（一句话被拆开的情形）一并纳入。
    let span_anchors: Vec<(String, usize, usize, String)> = blocks
        .iter()
        .filter(|b| b.runs.len() > 1)
        .map(|b| {
            let st = b.runs.first().unwrap().0;
            let en = b.runs.last().unwrap().1;
            (b.path.clone(), st, en, utf16_slice(&b.raw, st, en).unwrap_or_default())
        })
        .collect();

    // —— 独立第二次解析 ——
    let reparsed = match extract_docx(path) {
        Ok(b) => b,
        Err(e) => {
            println!(
                "{}",
                serde_json::json!({ "artifact": "w3-docx-functional", "ok": false, "error": format!("重解析失败: {e}") })
            );
            std::process::exit(1);
        }
    };
    let resolve = |path_key: &str, st: usize, en: usize| -> Option<String> {
        reparsed
            .iter()
            .find(|b| b.path == path_key)
            .and_then(|b| utf16_slice(&b.raw, st, en))
    };
    let anchor_total = anchors.len();
    let anchor_verbatim = anchors
        .iter()
        .filter(|(pk, st, en, ex)| resolve(pk, *st, *en).as_deref() == Some(ex.as_str()))
        .count();
    let span_total = span_anchors.len();
    let span_verbatim = span_anchors
        .iter()
        .filter(|(pk, st, en, ex)| resolve(pk, *st, *en).as_deref() == Some(ex.as_str()))
        .count();
    // 解析确定性：两次抽取的块数与文本必须完全一致。
    let deterministic = reparsed.len() == blocks.len()
        && reparsed
            .iter()
            .zip(blocks.iter())
            .all(|(a, b)| a.path == b.path && a.raw == b.raw && a.runs == b.runs);

    let out = serde_json::json!({
        "artifact": "w3-docx-functional",
        "ok": true,
        "stack": "pure-rust (zip deflate + quick-xml)",
        "blocks": blocks.len(),
        "total_text_utf16": total_utf16,
        "runs": total_runs,
        "multi_run_blocks": multi_run_blocks,
        "contains_cjk": has_cjk,
        "anchor_roundtrip": {
            "run_anchors": anchor_total,
            "verbatim": anchor_verbatim,
        },
        "cross_run_span_anchors": {
            "total": span_total,
            "verbatim": span_verbatim,
        },
        "reparse_deterministic": deterministic,
        "method": "锚点在第一次抽取时落定，再对**独立第二次解析**的结果取回并逐字比对（非同一次结果自比）",
        "note": "指标不含正文内容；样本仅本地读取"
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
}
