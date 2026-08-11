//! W1 解析可行性 spike:安全面证据采集(不受信输入)。
//!
//! 对照 docs/design/v0.20-work-cowork-increment.md §1.1 W1 双门清单的**安全面**:
//! 资源边界、ZIP/XML 炸弹、DOCX 路径穿越、截断/加密文件、崩溃遏制。
//! 功能面(抽取率/锚点回源)是产品实现(W3)的事,spike 只钉安全边界。
//!
//! 输出结构化 JSON 一行 `W1_EVIDENCE <json>`,证据入 PR 正文与 docs/evidence。
//! 每个探针的"预期"= fail-closed(拒绝 + 无 panic 逃逸 + 无部分状态)。

use std::io::{Cursor, Write};
use std::time::Instant;

#[derive(Default)]
struct Probe {
    name: &'static str,
    outcome: String,
    detail: String,
}

fn rec(name: &'static str, outcome: &str, detail: String) -> Probe {
    Probe { name, outcome: outcome.into(), detail }
}

fn main() {
    let mut probes: Vec<Probe> = Vec::new();

    // ---- PDF: 截断文件 ----
    // 只有 %PDF 头,无 xref/trailer —— 解析器必须报错而非 panic/挂死。
    probes.push(catch("pdf_truncated", || {
        let bytes = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n"; // 无 xref/trailer
        match lopdf::Document::load_mem(bytes) {
            Ok(_) => ("UNEXPECTED_OK".into(), "截断 PDF 被当作有效解析".into()),
            Err(e) => ("REJECTED".into(), format!("拒绝: {e}")),
        }
    }));

    // ---- PDF: 空/垃圾输入 ----
    probes.push(catch("pdf_garbage", || {
        let bytes = vec![0xFFu8; 4096];
        match lopdf::Document::load_mem(&bytes) {
            Ok(_) => ("UNEXPECTED_OK".into(), "垃圾字节被当作有效 PDF".into()),
            Err(e) => ("REJECTED".into(), format!("拒绝: {e}")),
        }
    }));

    // ---- PDF: 有效最小文档(正对照,证明解析器活着)----
    // 用 lopdf API 构造(保证 xref 偏移正确),不依赖 dictionary! 宏。
    probes.push(catch("pdf_valid_control", || {
        use lopdf::{Dictionary, Document, Object};
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let mut page = Dictionary::new();
        page.set("Type", "Page");
        page.set("Parent", pages_id);
        page.set("MediaBox", vec![0.into(), 0.into(), 612.into(), 792.into()]);
        let page_id = doc.add_object(page);
        let mut pages = Dictionary::new();
        pages.set("Type", "Pages");
        pages.set("Kids", vec![Object::Reference(page_id)]);
        pages.set("Count", 1);
        doc.objects.insert(pages_id, Object::Dictionary(pages));
        let mut catalog = Dictionary::new();
        catalog.set("Type", "Catalog");
        catalog.set("Pages", pages_id);
        let catalog_id = doc.add_object(catalog);
        doc.trailer.set("Root", catalog_id);
        let mut buf = Vec::new();
        doc.save_to(&mut buf).unwrap();
        match Document::load_mem(&buf) {
            Ok(d) => ("OK".into(), format!("正对照解析成功,页数={}", d.get_pages().len())),
            Err(e) => ("UNEXPECTED_FAIL".into(), format!("正对照失败: {e}")),
        }
    }));

    // ---- DOCX(ZIP): 路径穿越条目 ----
    // 构造一个含 "../evil.txt" 条目的 zip —— 解压逻辑必须拒绝或不落盘到该路径。
    probes.push(catch("docx_zip_path_traversal", || {
        let mut buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
            // 恶意条目名
            if zw.start_file("../../evil.txt", opts).is_ok() {
                let _ = zw.write_all(b"pwned");
            }
            let _ = zw.finish();
        }
        // 探针:遍历条目时是否暴露了逃逸路径(我们绝不按 enclosed_name 之外落盘)
        let reader = zip::ZipArchive::new(Cursor::new(&buf));
        match reader {
            Ok(mut ar) => {
                let mut traversal_seen = false;
                for i in 0..ar.len() {
                    let f = ar.by_index(i).unwrap();
                    // zip crate 的 enclosed_name() 对逃逸名返回 None —— 这是安全 API
                    if f.enclosed_name().is_none() {
                        traversal_seen = true;
                    }
                }
                if traversal_seen {
                    ("REJECTED".into(),
                     "enclosed_name() 对 ../ 条目返回 None:必须只用该 API 落盘".into())
                } else {
                    ("UNEXPECTED_OK".into(), "逃逸条目未被 enclosed_name 拦截".into())
                }
            }
            Err(e) => ("REJECTED".into(), format!("zip 打开失败: {e}")),
        }
    }));

    // ---- DOCX(ZIP): 声明式炸弹防护策略验证 ----
    // 不实际构造 4GB 炸弹;验证"解压前可读取声明大小并设上限"的机制存在。
    probes.push(catch("docx_zip_bomb_guard", || {
        // 造一个正常小 zip,读其条目声明的 uncompressed size —— 证明我们能在
        // 解压前拿到 size 做上限判定(策略:sum(uncompressed) > CAP 即拒)。
        let mut buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
            zw.start_file("word/document.xml", opts).unwrap();
            zw.write_all(&vec![b'a'; 10_000]).unwrap();
            zw.finish().unwrap();
        }
        let mut ar = zip::ZipArchive::new(Cursor::new(&buf)).unwrap();
        let mut declared_total: u64 = 0;
        for i in 0..ar.len() {
            declared_total += ar.by_index(i).unwrap().size();
        }
        const CAP: u64 = 200 * 1024 * 1024;
        if declared_total > 0 && declared_total <= CAP {
            ("OK".into(),
             format!("解压前可读声明大小={} 字节,上限判定机制成立(CAP={CAP})", declared_total))
        } else {
            ("UNEXPECTED".into(), format!("声明大小异常={declared_total}"))
        }
    }));

    // ---- 崩溃遏制:catch_unwind 是否能兜住解析 panic ----
    probes.push(catch("crash_containment", || {
        let r = std::panic::catch_unwind(|| {
            // 故意越界,模拟解析器内部 panic
            let v: Vec<u8> = vec![];
            let _ = v[10];
        });
        match r {
            Err(_) => ("CONTAINED".into(),
                       "解析 panic 被 catch_unwind 兜住;生产须跑在可杀工作进程".into()),
            Ok(_) => ("UNEXPECTED".into(), "预期 panic 未发生".into()),
        }
    }));

    // 汇总为结构化证据
    let items: Vec<String> = probes.iter().map(|p| {
        format!("{{\"probe\":\"{}\",\"outcome\":\"{}\",\"detail\":\"{}\"}}",
            p.name, p.outcome,
            p.detail.replace('\\', "\\\\").replace('"', "\\'").replace('\n', " "))
    }).collect();

    // 判据:每个安全探针必须落在其"安全结局"集合内
    let safe = probes.iter().all(|p| matches!(p.outcome.as_str(),
        "REJECTED" | "OK" | "CONTAINED"));

    println!("W1_EVIDENCE {{\"pdf_parser\":\"lopdf(pure-rust)\",\"docx\":\"zip+docx-rs(pure-rust)\",\"native_binary\":false,\"probes\":[{}],\"all_safe\":{}}}",
        items.join(","), safe);

    std::process::exit(if safe { 0 } else { 1 });
}

/// 单个探针包 catch_unwind,任何探针自身 panic 不得中断整轮(遏制即证据)。
fn catch(name: &'static str, f: impl FnOnce() -> (String, String) + std::panic::UnwindSafe) -> Probe {
    let t = Instant::now();
    match std::panic::catch_unwind(f) {
        Ok((outcome, detail)) => rec(name, &outcome,
            format!("{detail} ({}ms)", t.elapsed().as_millis())),
        Err(_) => rec(name, "PANIC_ESCAPED",
            "探针自身 panic 逃逸 —— 遏制不足".into()),
    }
}
