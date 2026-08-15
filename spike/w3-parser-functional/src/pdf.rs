//! W3 功能面 spike：PDF 抽取 + 锚点可回源（真实样本）。
//!
//! 与 DOCX 面同一问题、同一方法论：
//!   ① 抽取率：每页能否拿到文本（非空、中文不乱码）；
//!   ② 锚点可回源：按 `page` + `raw_range` 取回的文本，在**独立第二次解析**
//!      后是否仍与落定时**逐字一致**（不是同一次结果自比——那是恒等式）。
//!
//! 这正是 W1 列为 NOT-RUN 的功能面：W1 只证了安全面（畸形/截断/加密 PDF
//! fail-closed），并说「pdfium→纯 Rust 选型是**建议**，待功能面 spike 后
//! 落地」。本 spike 用真实样本回答"lopdf 的文本抽取够不够用"。
//!
//! 样本只在本地读取；输出只含指标，不含任何正文内容。

use std::collections::BTreeMap;

fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

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
                return None;
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

/// 逐页抽取文本：page_number → raw text。
fn extract_pdf(path: &std::path::Path) -> Result<BTreeMap<u32, String>, String> {
    let doc = lopdf::Document::load(path).map_err(|e| format!("加载失败: {e}"))?;
    let mut out = BTreeMap::new();
    for (&page_no, _) in doc.get_pages().iter() {
        // lopdf 的 extract_text 对单页取文本；失败的页记空串（抽取率会体现）。
        let text = doc.extract_text(&[page_no]).unwrap_or_default();
        out.insert(page_no, text);
    }
    Ok(out)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(path) = args.get(1) else {
        eprintln!("用法: w3_pdf_probe <sample.pdf>");
        std::process::exit(2);
    };
    let path = std::path::Path::new(path);
    let label = args.get(2).cloned().unwrap_or_else(|| "sample".to_string());

    let t0 = std::time::Instant::now();
    let pages = match extract_pdf(path) {
        Ok(p) => p,
        Err(e) => {
            println!(
                "{}",
                serde_json::json!({ "artifact": "w3-pdf-functional", "sample": label, "ok": false, "error": e })
            );
            std::process::exit(1);
        }
    };
    let parse_ms = t0.elapsed().as_millis();

    // ① 抽取指标。
    let total_pages = pages.len();
    let nonempty_pages = pages.values().filter(|t| !t.trim().is_empty()).count();
    let total_utf16: usize = pages.values().map(|t| utf16_len(t)).sum();
    let has_cjk = pages
        .values()
        .any(|t| t.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)));
    // 抽取率 = 有文本的页 / 总页数。扫描件（纯图像页）会显著偏低——这正是
    // 我们要量的东西。
    let extraction_rate = if total_pages == 0 {
        0.0
    } else {
        nonempty_pages as f64 / total_pages as f64
    };

    // ② 锚点回源：每个非空页取若干区间，跨独立第二次解析比对。
    let mut anchors: Vec<(u32, usize, usize, String)> = Vec::new();
    for (&pg, text) in pages.iter() {
        let len = utf16_len(text);
        if len == 0 {
            continue;
        }
        // 页首、页中、页尾三段（各 ≤80 单元），避开代理对由 utf16_slice 保证。
        for (st, en) in [
            (0usize, len.min(80)),
            (len / 2, (len / 2 + 80).min(len)),
            (len.saturating_sub(80), len),
        ] {
            if st < en {
                if let Some(ex) = utf16_slice(text, st, en) {
                    if !ex.trim().is_empty() {
                        anchors.push((pg, st, en, ex));
                    }
                }
            }
        }
    }

    let reparsed = match extract_pdf(path) {
        Ok(p) => p,
        Err(e) => {
            println!(
                "{}",
                serde_json::json!({ "artifact": "w3-pdf-functional", "sample": label, "ok": false, "error": format!("重解析失败: {e}") })
            );
            std::process::exit(1);
        }
    };
    let anchor_total = anchors.len();
    let anchor_verbatim = anchors
        .iter()
        .filter(|(pg, st, en, ex)| {
            reparsed
                .get(pg)
                .and_then(|t| utf16_slice(t, *st, *en))
                .as_deref()
                == Some(ex.as_str())
        })
        .count();
    let deterministic = reparsed.len() == pages.len()
        && reparsed.iter().zip(pages.iter()).all(|((k1, v1), (k2, v2))| k1 == k2 && v1 == v2);

    let out = serde_json::json!({
        "artifact": "w3-pdf-functional",
        "sample": label,
        "ok": true,
        "stack": "pure-rust (lopdf)",
        "pages": total_pages,
        "pages_with_text": nonempty_pages,
        "extraction_rate": (extraction_rate * 1000.0).round() / 1000.0,
        "total_text_utf16": total_utf16,
        "contains_cjk": has_cjk,
        "parse_ms": parse_ms,
        "anchor_roundtrip": { "total": anchor_total, "verbatim": anchor_verbatim },
        "reparse_deterministic": deterministic,
        "method": "锚点第一次解析时落定，对**独立第二次解析**结果取回并逐字比对",
        "note": "指标不含正文内容；样本仅本地读取"
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
}
