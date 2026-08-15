//! W3 选项 2 证据 spike：`pdf-extract` 与 `pdf` **并列**评测（唯一一轮）。
//!
//! 裁断给的边界：同一份证据、同一对样本、同一套指标，两者并列跑；
//! **杀死条件** = 在 Type0/CID + 全 ToUnicode 的样本上两者抽取率都为 0
//! → 选项 2 死，下一跳 pdfium，不再试第三个纯 Rust crate。
//! **通过条件** = 至少一方抽取率 > 0，并写清锚点还缺什么。
//! 通过也不等于选型落地，只表示「纯 Rust 仍可能」。
//!
//! 指标与前一轮 lopdf 完全一致，便于横向对照：
//!   - 抽取率 = 有文本的页 / 总页数（`pdf-extract` 只给整篇文本时，按
//!     「整篇是否非空」记 1/0 并在输出里标明粒度，不伪造逐页数据）；
//!   - 字体类型普查 + ToUnicode 是否存在（由 lopdf 侧诊断提供，不重复实现）；
//!   - 中文是否可读（CJK 出现且非乱码替代符）。
//!
//! 样本只在本地读取；输出只含指标，不含正文内容。

use std::io::Read;

fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

fn cjk_count(s: &str) -> usize {
    s.chars().filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c)).count()
}

/// 替代符/控制符比例——抽出来但全是 U+FFFD 或问号，等于没抽出来。
fn junk_ratio(s: &str) -> f64 {
    let total = s.chars().filter(|c| !c.is_whitespace()).count();
    if total == 0 {
        return 0.0;
    }
    let junk = s
        .chars()
        .filter(|&c| c == '\u{fffd}' || c == '\u{0}' || (c.is_control() && c != '\n' && c != '\r' && c != '\t'))
        .count();
    junk as f64 / total as f64
}

/// 候选一：pdf-extract（自带 Type0/CID + ToUnicode 解码）。整篇粒度。
fn try_pdf_extract(path: &std::path::Path) -> Result<String, String> {
    // 该 crate 会 panic 于部分畸形文件，spike 里捕获以免整轮中断。
    let p = path.to_path_buf();
    std::panic::catch_unwind(move || pdf_extract::extract_text(&p))
        .map_err(|_| "panic in pdf-extract".to_string())?
        .map_err(|e| format!("{e}"))
}

/// 候选二：pdf crate。逐页取文本。
fn try_pdf_crate(path: &std::path::Path) -> Result<Vec<String>, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| format!("打开失败: {e}"))?
        .read_to_end(&mut bytes)
        .map_err(|e| format!("读取失败: {e}"))?;
    let p = bytes.clone();
    let out = std::panic::catch_unwind(move || -> Result<Vec<String>, String> {
        let file = pdf::file::FileOptions::cached()
            .load(&p[..])
            .map_err(|e| format!("加载失败: {e}"))?;
        let resolver = file.resolver();
        let mut pages = Vec::new();
        for page in file.pages() {
            let page = page.map_err(|e| format!("页错误: {e}"))?;
            let flow = pdf_text_of_page(&page, &resolver).unwrap_or_default();
            pages.push(flow);
        }
        Ok(pages)
    })
    .map_err(|_| "panic in pdf crate".to_string())?;
    out
}

/// pdf crate 没有开箱的 extract_text；用其内容流算子取 Tj/TJ 的字符串。
/// 这里只做**存在性**判断（能否拿到字符），不追求排版还原。
fn pdf_text_of_page(
    page: &pdf::object::PageRc,
    resolver: &impl pdf::object::Resolve,
) -> Option<String> {
    use pdf::content::Op;
    let contents = page.contents.as_ref()?;
    let ops = contents.operations(resolver).ok()?;
    let mut out = String::new();
    for op in ops {
        match op {
            Op::TextDraw { text } => {
                if let Ok(s) = std::str::from_utf8(text.as_bytes()) {
                    out.push_str(s);
                }
            }
            Op::TextDrawAdjusted { array } => {
                for item in array {
                    if let pdf::content::TextDrawAdjusted::Text(t) = item {
                        if let Ok(s) = std::str::from_utf8(t.as_bytes()) {
                            out.push_str(s);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Some(out)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(path) = args.get(1) else {
        eprintln!("用法: w3_opt2 <sample.pdf> [label]");
        std::process::exit(2);
    };
    let path = std::path::Path::new(path);
    let label = args.get(2).cloned().unwrap_or_else(|| "sample".into());

    // —— 候选一 ——
    let t0 = std::time::Instant::now();
    let a = try_pdf_extract(path);
    let a_ms = t0.elapsed().as_millis();
    let a_json = match &a {
        Ok(text) => serde_json::json!({
            "ok": true,
            "granularity": "whole-document",
            "nonempty": !text.trim().is_empty(),
            "text_utf16": utf16_len(text),
            "cjk_chars": cjk_count(text),
            "junk_ratio": (junk_ratio(text) * 1000.0).round() / 1000.0,
            "ms": a_ms,
        }),
        Err(e) => serde_json::json!({ "ok": false, "error": e, "ms": a_ms }),
    };

    // —— 候选二 ——
    let t1 = std::time::Instant::now();
    let b = try_pdf_crate(path);
    let b_ms = t1.elapsed().as_millis();
    let b_json = match &b {
        Ok(pages) => {
            let nonempty = pages.iter().filter(|t| !t.trim().is_empty()).count();
            let all: String = pages.concat();
            serde_json::json!({
                "ok": true,
                "granularity": "per-page",
                "pages": pages.len(),
                "pages_with_text": nonempty,
                "extraction_rate": if pages.is_empty() { 0.0 }
                    else { ((nonempty as f64 / pages.len() as f64) * 1000.0).round() / 1000.0 },
                "text_utf16": utf16_len(&all),
                "cjk_chars": cjk_count(&all),
                "junk_ratio": (junk_ratio(&all) * 1000.0).round() / 1000.0,
                "ms": b_ms,
            })
        }
        Err(e) => serde_json::json!({ "ok": false, "error": e, "ms": b_ms }),
    };

    let a_alive = a.as_ref().map(|t| !t.trim().is_empty()).unwrap_or(false);
    let b_alive = b
        .as_ref()
        .map(|p| p.iter().any(|t| !t.trim().is_empty()))
        .unwrap_or(false);

    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "artifact": "w3-opt2-pure-rust-pdf",
            "sample": label,
            "pdf_extract": a_json,
            "pdf_crate": b_json,
            "either_extracted": a_alive || b_alive,
            "note": "指标不含正文内容；样本仅本地读取。杀死条件见文件头。"
        }))
        .unwrap()
    );
}
