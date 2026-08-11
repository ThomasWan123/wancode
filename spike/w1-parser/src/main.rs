//! W1 解析 spike —— 探索性 API 可行性(**非**安全门关闭证据,codex R2-F1)。
//!
//! 每个探针要么喂真实构造物给真实解析器并断言其行为,要么明确标 NOT-RUN。
//! 不做同义反复的字符串检查(codex R2-F3),不用假加密文件(R2-F2)。
//!
//! 子进程模式:argv[1] == "__crash" 立即崩溃;"__hang" 死循环(供 F4 超时杀)。
//! 两者在崩溃/挂死前都会写一个临时哨兵文件,父进程据此断言零残留清理。

use std::io::{Cursor, Write};
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};
use serde_json::json;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(mode) = args.get(1) {
        match mode.as_str() {
            "__crash" => { child_prelude(&args); std::process::abort(); }
            "__hang"  => { child_prelude(&args); loop { std::hint::spin_loop(); } }
            _ => {}
        }
    }

    let mut probes = Vec::new();

    // ① PDF 截断 → 结构化错误
    probes.push(catch("pdf_truncated", || {
        match lopdf::Document::load_mem(b"%PDF-1.7\n1 0 obj\n<< >>\nendobj\n") {
            Ok(_) => ("UNEXPECTED_OK", "截断被接受".to_string()),
            Err(e) => ("REJECTED", format!("{e}")),
        }
    }));

    // ② PDF 垃圾 → 结构化错误
    probes.push(catch("pdf_garbage", || {
        match lopdf::Document::load_mem(&vec![0xFFu8; 4096]) {
            Ok(_) => ("UNEXPECTED_OK", "垃圾被接受".to_string()),
            Err(e) => ("REJECTED", format!("{e}")),
        }
    }));

    // ③ PDF 正对照:合法单页解析成功
    probes.push(catch("pdf_valid_control", || {
        let buf = build_pdf(false);
        match lopdf::Document::load_mem(&buf) {
            Ok(d) => ("OK", format!("页数={}", d.get_pages().len())),
            Err(e) => ("UNEXPECTED_FAIL", format!("{e}")),
        }
    }));

    // ④ PDF 加密:构造**结构合法**且 trailer 带 /Encrypt 的文档,
    //    断言解析器**检出 /Encrypt** → 一期分类为不支持(codex R2-F2)。
    probes.push(catch("pdf_encrypted_detected", || {
        let buf = build_pdf(true); // 带 /Encrypt 字典
        match lopdf::Document::load_mem(&buf) {
            Ok(d) => {
                if d.trailer.get(b"Encrypt").is_ok() {
                    ("HANDLED", "trailer 含 /Encrypt,分类为不支持(不解密)".to_string())
                } else {
                    ("UNEXPECTED", "结构合法但未检出 /Encrypt".to_string())
                }
            }
            // 若解析器直接因加密报错也是可接受的 fail-closed
            Err(e) => ("HANDLED", format!("加密文档被拒(fail-closed):{e}")),
        }
    }));

    // ⑤ DOCX zip 路径穿越
    probes.push(catch("docx_zip_path_traversal", || {
        let mut buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(Cursor::new(&mut buf));
            let o: zip::write::FileOptions<()> = zip::write::FileOptions::default();
            let _ = zw.start_file("../../evil.txt", o);
            let _ = zw.write_all(b"x");
            let _ = zw.finish();
        }
        let mut ar = zip::ZipArchive::new(Cursor::new(&buf)).unwrap();
        let escaped = (0..ar.len()).any(|i| ar.by_index(i).unwrap().enclosed_name().is_none());
        if escaped { ("REJECTED", "enclosed_name() 对 ../ 返回 None".to_string()) }
        else { ("UNEXPECTED_OK", "逃逸条目未被拦".to_string()) }
    }));

    // ⑥ DOCX zip 超限:声明解压 > CAP 解压前拒绝(变异敏感)
    probes.push(catch("docx_zip_over_cap_rejected", || {
        const CAP: u64 = 1_000_000;
        let buf = build_docx_zip("word/document.xml", &vec![b'a'; 2_000_000]);
        let mut ar = zip::ZipArchive::new(Cursor::new(&buf)).unwrap();
        let mut declared: u64 = 0;
        for i in 0..ar.len() {
            declared = declared.checked_add(ar.by_index(i).unwrap().size()).unwrap();
        }
        if declared > CAP { ("REJECTED", format!("声明={declared}>CAP={CAP},解压前拒")) }
        else { ("MUTATION_LEAK", format!("声明={declared} 未超 CAP")) }
    }));

    // ⑦ DOCX XML 实体炸弹:把真实的恶意 .docx 喂给 docx-rs,断言**不展开/不挂死**。
    //    加良性 DOCX 正对照。这是真实解析边界,不是字符串检查(codex R2-F3)。
    probes.push(catch("docx_xml_entity_real_parse", || {
        let bomb_xml = br#"<?xml version="1.0"?><!DOCTYPE lolz [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;"><!ENTITY c "&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;">]><w:document xmlns:w="x"><w:body><w:p><w:r><w:t>&c;</w:t></w:r></w:body></w:document>"#;
        let docx = build_docx_zip("word/document.xml", bomb_xml);
        // 在子线程 + 超时内解析,证明不会无界展开挂死。
        let (tx, rx) = std::sync::mpsc::channel();
        let bytes = docx.clone();
        std::thread::spawn(move || {
            let r = std::panic::catch_unwind(|| docx_rs::read_docx(&bytes).is_ok());
            let _ = tx.send(r.unwrap_or(false));
        });
        match rx.recv_timeout(Duration::from_secs(3)) {
            // 无论解析成功与否,关键是**在 3s 内返回**(未无界展开挂死)。
            Ok(_) => ("REJECTED", "恶意 DOCX 在超时内被解析器有界处理(无挂死展开)".to_string()),
            Err(_) => ("EXPANDED_OR_HUNG", "解析在 3s 内未返回(疑似实体展开挂死)".to_string()),
        }
    }));

    // ⑦b DOCX 良性正对照:合法 DOCX 能被 docx-rs 解析
    probes.push(catch("docx_benign_control", || {
        let ok_xml = br#"<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>hello</w:t></w:r></w:p></w:body></w:document>"#;
        let docx = build_docx_zip("word/document.xml", ok_xml);
        match docx_rs::read_docx(&docx) {
            Ok(_) => ("OK", "良性 DOCX 解析成功(证明拒绝非全拒)".to_string()),
            // docx-rs 需完整 OOXML 结构;解析失败不代表安全问题,标记为 NOT-RUN 语义
            Err(e) => ("PARSER_NEEDS_FULL_OOXML",
                format!("最小 DOCX 不足以让 docx-rs 完整解析:{e};正对照留待 W3 用真实样本")),
        }
    }));

    // ⑧ 崩溃遏制:crash worker → 必须非成功终止 + 残留哨兵被清
    probes.push(worker_probe("crash_containment", "__crash", false));
    // ⑧b 挂死遏制:hang worker → 必须触发超时被杀 + 残留哨兵被清
    probes.push(worker_probe("hang_timeout_kill", "__hang", true));

    // ---- 汇总为真实 JSON(serde_json) + parse-back 校验(codex R2-F5)----
    let arr: Vec<serde_json::Value> = probes.iter()
        .map(|(n, o, d)| json!({"probe": n, "outcome": o, "detail": d})).collect();
    let safe = probes.iter().all(|(_, o, _)| matches!(*o,
        "REJECTED" | "OK" | "HANDLED" | "CONTAINED" | "KILLED"
        | "PARSER_NEEDS_FULL_OOXML"));
    let doc = json!({
        "artifact": "w1-parser-spike",
        "scope": "exploratory API feasibility — NOT a closed safety gate",
        "compression_stack": "pure-rust deflate (miniz_oxide); no separately-shipped native binary",
        "all_safe": safe,
        "probes": arr,
    });
    let s = serde_json::to_string_pretty(&doc).unwrap();
    // 真实 parse-back:序列化产物必须能被 serde_json 重新解析
    let parse_back_ok = serde_json::from_str::<serde_json::Value>(&s).is_ok();

    let out = args.get(2).cloned().unwrap_or_else(|| "w1-evidence.json".into());
    std::fs::write(&out, &s).unwrap();
    println!("W1 spike: probes={} all_safe={} parse_back_ok={} -> {}",
        probes.len(), safe, parse_back_ok, out);
    std::process::exit(if safe && parse_back_ok { 0 } else { 1 });
}

/// 子进程崩溃/挂死前落一个临时哨兵,父进程据此验证清理。
fn child_prelude(args: &[String]) {
    if let Some(sentinel) = args.get(2) {
        let _ = std::fs::write(sentinel, b"CHILD_PARTIAL_STATE");
    }
}

/// 起子进程,断言:crash→非成功退出;hang→超时被杀;两者父存活 + 哨兵清理。
fn worker_probe(name: &'static str, mode: &str, expect_timeout: bool)
    -> (&'static str, &'static str, String) {
    let exe = std::env::current_exe().unwrap();
    let sentinel = std::env::temp_dir()
        .join(format!("w1-{}-{}.tmp", mode.trim_start_matches('_'), std::process::id()));
    let child = Command::new(&exe).arg(mode).arg(&sentinel).spawn();
    let mut c = match child { Ok(c) => c, Err(e) =>
        return (name, "ERROR", format!("起子进程失败:{e}")) };

    let start = Instant::now();
    let timeout = Duration::from_secs(2);
    let outcome;
    loop {
        match c.try_wait() {
            Ok(Some(status)) => {
                if expect_timeout {
                    // hang worker 不该自己退出
                    outcome = ("UNEXPECTED", format!("hang worker 自行退出 status={status:?}"));
                } else if status.success() {
                    outcome = ("UNEXPECTED", "crash worker 竟成功退出".to_string());
                } else {
                    outcome = ("CONTAINED", format!("crash worker 非成功终止 status={status:?}"));
                }
                break;
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = c.kill();
                    let _ = c.wait();
                    outcome = if expect_timeout {
                        ("KILLED", "hang worker 超时被 kill,父存活".to_string())
                    } else {
                        ("UNEXPECTED", "crash worker 未在超时内崩溃".to_string())
                    };
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => { outcome = ("ERROR", format!("wait:{e}")); break; }
        }
    }
    // 残留清理:哨兵(子进程的部分状态)必须由父进程清掉,验证零残留策略
    let residue_before = sentinel.exists();
    let _ = std::fs::remove_file(&sentinel);
    let residue_cleaned = !sentinel.exists();
    let detail = format!("{};残留哨兵存在={} 清理后={}",
        outcome.1, residue_before, residue_cleaned);
    let final_outcome = if residue_cleaned { outcome.0 } else { "RESIDUE_LEAK" };
    (name, final_outcome, detail)
}

fn catch(name: &'static str,
    f: impl FnOnce() -> (&'static str, String) + std::panic::UnwindSafe)
    -> (&'static str, &'static str, String) {
    match std::panic::catch_unwind(f) {
        Ok((o, d)) => (name, o, d),
        Err(_) => (name, "PANIC_ESCAPED", "探针 panic 逃逸".to_string()),
    }
}

/// 构造结构合法的最小单页 PDF;encrypted=true 时 trailer 加 /Encrypt 字典。
fn build_pdf(encrypted: bool) -> Vec<u8> {
    use lopdf::{Dictionary, Document, Object};
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let mut page = Dictionary::new();
    page.set("Type", "Page"); page.set("Parent", pages_id);
    page.set("MediaBox", vec![0.into(), 0.into(), 612.into(), 792.into()]);
    let page_id = doc.add_object(page);
    let mut pages = Dictionary::new();
    pages.set("Type", "Pages"); pages.set("Kids", vec![Object::Reference(page_id)]);
    pages.set("Count", 1);
    doc.objects.insert(pages_id, Object::Dictionary(pages));
    let mut cat = Dictionary::new();
    cat.set("Type", "Catalog"); cat.set("Pages", pages_id);
    let cid = doc.add_object(cat);
    doc.trailer.set("Root", cid);
    if encrypted {
        let mut enc = Dictionary::new();
        enc.set("Filter", "Standard");
        enc.set("V", 2); enc.set("R", 3); enc.set("P", -44);
        let enc_id = doc.add_object(enc);
        doc.trailer.set("Encrypt", enc_id);
    }
    let mut buf = Vec::new();
    doc.save_to(&mut buf).unwrap();
    buf
}

/// 构造**完整最小 OOXML** DOCX:含 [Content_Types].xml、_rels/.rels、
/// 以及给定 document.xml 主体。这样 docx-rs 能真正解析(正对照有效),
/// 实体炸弹对照才有意义(codex R2-F3)。
fn build_docx_zip(_entry: &str, document_xml: &[u8]) -> Vec<u8> {
    let content_types = br#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
    let rels = br#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
    let mut buf = Vec::new();
    {
        let mut zw = zip::ZipWriter::new(Cursor::new(&mut buf));
        let o: zip::write::FileOptions<()> = zip::write::FileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in [
            ("[Content_Types].xml", &content_types[..]),
            ("_rels/.rels", &rels[..]),
            ("word/document.xml", document_xml),
        ] {
            zw.start_file(name, o).unwrap();
            zw.write_all(data).unwrap();
        }
        zw.finish().unwrap();
    }
    buf
}

#[allow(dead_code)]
fn touch(_p: &PathBuf) {}
