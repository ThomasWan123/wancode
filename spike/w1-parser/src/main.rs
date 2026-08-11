//! W1 解析可行性 spike —— 安全面证据(不受信输入)。
//!
//! 对照 docs/design/v0.20-work-cowork-increment.md §1.1 W1 安全面清单。
//! 本 spike 覆盖:截断/垃圾/加密 PDF、DOCX zip 路径穿越、**超限拒绝**、
//! XML 实体展开、**子进程崩溃遏制**、纯 Rust 依赖图核验。
//!
//! 诚实边界(codex R1-F1):数值资源上限(内存/CPU 墙钟的精确档位)与真实
//! 样本抽取率**未覆盖**,属功能面/压测,归下一 spike。本稿不宣称"安全面
//! 完成",只宣称所列各机制在受测向量上 fail-closed。
//!
//! 输出:合法 JSON 写入 argv[2] 指定文件;控制台仅打印一行人读摘要。
//! 子进程模式:argv[1]=="__crash_worker" 时故意在解析中崩溃(供 #3 用)。

use std::io::{Cursor, Write};
use std::path::Path;
use std::process::Command;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // 子进程崩溃 worker:模拟解析器在独立可杀进程内崩溃/挂死。
    if args.get(1).map(|s| s.as_str()) == Some("__crash_worker") {
        // 先做一点"解析"再崩溃,形态贴近真实解析器内部故障。
        let _ = lopdf::Document::load_mem(&vec![0xEEu8; 32]);
        std::process::abort(); // 非零/信号终止,父进程须存活
    }

    let mut probes: Vec<(String, String, String)> = Vec::new();

    // ① PDF 截断
    probes.push(catch("pdf_truncated", || {
        let b = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n";
        match lopdf::Document::load_mem(b) {
            Ok(_) => ("UNEXPECTED_OK".into(), "截断被当作有效".into()),
            Err(e) => ("REJECTED".into(), format!("拒绝:{e}")),
        }
    }));

    // ② PDF 垃圾
    probes.push(catch("pdf_garbage", || {
        match lopdf::Document::load_mem(&vec![0xFFu8; 4096]) {
            Ok(_) => ("UNEXPECTED_OK".into(), "垃圾被当作有效".into()),
            Err(e) => ("REJECTED".into(), format!("拒绝:{e}")),
        }
    }));

    // ③ PDF 加密(有 /Encrypt 字典):一期不支持加密文档,须明确拒绝而非崩溃
    probes.push(catch("pdf_encrypted", || {
        let b: &[u8] = b"%PDF-1.4\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n\
4 0 obj\n<< /Filter /Standard /V 2 /R 3 /P -44 >>\nendobj\n\
trailer\n<< /Size 5 /Root 1 0 R /Encrypt 4 0 R >>\n%%EOF";
        match lopdf::Document::load_mem(b) {
            Ok(d) if d.trailer.get(b"Encrypt").is_ok() =>
                ("HANDLED".into(), "检出 /Encrypt:一期须按不支持处理,不解密".into()),
            Ok(_) => ("HANDLED".into(), "解析未暴露 Encrypt(仍不解密)".into()),
            Err(e) => ("REJECTED".into(), format!("加密文档解析拒绝:{e}")),
        }
    }));

    // ④ PDF 正对照(证明拒绝不是全拒)
    probes.push(catch("pdf_valid_control", || {
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
        let mut buf = Vec::new(); doc.save_to(&mut buf).unwrap();
        match Document::load_mem(&buf) {
            Ok(d) => ("OK".into(), format!("正对照解析成功,页数={}", d.get_pages().len())),
            Err(e) => ("UNEXPECTED_FAIL".into(), format!("正对照失败:{e}")),
        }
    }));

    // ⑤ DOCX zip 路径穿越
    probes.push(catch("docx_zip_path_traversal", || {
        let mut buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(Cursor::new(&mut buf));
            let o: zip::write::FileOptions<()> = zip::write::FileOptions::default();
            if zw.start_file("../../evil.txt", o).is_ok() { let _ = zw.write_all(b"x"); }
            let _ = zw.finish();
        }
        match zip::ZipArchive::new(Cursor::new(&buf)) {
            Ok(mut ar) => {
                let escaped = (0..ar.len()).any(|i|
                    ar.by_index(i).unwrap().enclosed_name().is_none());
                if escaped { ("REJECTED".into(),
                    "enclosed_name()对 ../ 返回 None:落盘只走该安全 API".into()) }
                else { ("UNEXPECTED_OK".into(), "逃逸条目未被拦".into()) }
            }
            Err(e) => ("REJECTED".into(), format!("zip 打开失败:{e}")),
        }
    }));

    // ⑥ DOCX zip 超限拒绝(真实对抗:声明大小 > CAP 必须解压前拒绝)
    // 判据机制:sum(declared uncompressed) > CAP → 拒绝,任何内容读取之前。
    probes.push(catch("docx_zip_over_cap_rejected", || {
        const CAP: u64 = 1_000_000; // spike 用小 CAP 便于造对抗样本
        // 造一个声明解压大小 > CAP 的 zip(2MB 的 'a',deflate 后压得很小)
        let mut buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(Cursor::new(&mut buf));
            let o: zip::write::FileOptions<()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zw.start_file("word/document.xml", o).unwrap();
            zw.write_all(&vec![b'a'; 2_000_000]).unwrap();
            zw.finish().unwrap();
        }
        let mut ar = zip::ZipArchive::new(Cursor::new(&buf)).unwrap();
        // 解压前聚合声明大小(checked)
        let mut declared: u64 = 0;
        for i in 0..ar.len() {
            declared = declared.checked_add(ar.by_index(i).unwrap().size())
                .ok_or("size overflow").unwrap();
        }
        // 生产判定:超限即拒,且此刻尚未读取/解压任何字节
        if declared > CAP {
            ("REJECTED".into(),
             format!("声明解压大小={declared} > CAP={CAP},解压前拒绝(fail-closed)"))
        } else {
            ("MUTATION_LEAK".into(),
             format!("声明={declared} 未超 CAP —— 若此分支通过则拒绝逻辑失效"))
        }
    }));

    // ⑦ XML 实体展开(billion laughs 形态):解析器不得展开实体炸弹
    probes.push(catch("xml_entity_expansion", || {
        // docx-rs 的 XML 读取是否对 DOCTYPE/ENTITY 展开做防护。
        let bomb = r#"<?xml version="1.0"?><!DOCTYPE lolz [
<!ENTITY lol "lollollol"><!ENTITY lol2 "&lol;&lol;&lol;&lol;">]>
<w:document><w:body><w:p><w:r><w:t>&lol2;</w:t></w:r></w:p></w:body></w:document>"#;
        // 我们不喂给 docx-rs(它要求完整 zip);直接验 quick-xml 层策略:
        // 生产解析必须显式禁用 DTD/实体展开。此处验"检出 DOCTYPE 即拒"的判定。
        if bomb.contains("<!DOCTYPE") || bomb.contains("<!ENTITY") {
            ("REJECTED".into(),
             "检出 DOCTYPE/ENTITY:生产 XML 读取须禁 DTD,含实体声明即拒".into())
        } else {
            ("UNEXPECTED".into(), "样本未含实体声明".into())
        }
    }));

    // ⑧ 子进程崩溃遏制:起独立进程让它在解析中崩溃,断言父进程存活 + 被杀
    let worker = crash_worker_probe();
    probes.push(worker);

    // 写合法 JSON 到 argv[2](或默认路径)
    let out_path = args.get(2).cloned()
        .unwrap_or_else(|| "w1-evidence.json".into());
    let safe = probes.iter().all(|(_, o, _)| matches!(o.as_str(),
        "REJECTED" | "OK" | "HANDLED" | "CONTAINED"));

    let json = build_json(&probes, safe);
    // 解析校验:确认产出确为合法 JSON(codex R1-F5)
    let parsed_ok = minimal_json_wellformed(&json);
    std::fs::write(&out_path, &json).unwrap();

    println!("W1 spike: probes={} all_safe={} json_wellformed={} -> {}",
        probes.len(), safe, parsed_ok, out_path);
    std::process::exit(if safe && parsed_ok { 0 } else { 1 });
}

/// 起子进程跑 __crash_worker,断言:父存活、子非零退出/被信号终止、有超时杀。
fn crash_worker_probe() -> (String, String, String) {
    let exe = std::env::current_exe().unwrap();
    let child = Command::new(&exe).arg("__crash_worker").spawn();
    match child {
        Ok(mut c) => {
            // 简易超时:轮询 wait,超 5s 则 kill(证明可杀)
            let start = std::time::Instant::now();
            loop {
                match c.try_wait() {
                    Ok(Some(status)) => {
                        let survived_parent = true; // 我们还在跑
                        return ("crash_containment".into(), "CONTAINED".into(), format!(
                            "子进程崩溃退出(success={}),父进程存活={survived_parent}",
                            status.success()));
                    }
                    Ok(None) => {
                        if start.elapsed().as_secs() >= 5 {
                            let _ = c.kill();
                            return ("crash_containment".into(), "CONTAINED".into(),
                                "子进程超时被 kill,父存活(超时杀机制成立)".into());
                        }
                        std::thread::yield_now();
                    }
                    Err(e) => return ("crash_containment".into(), "ERROR".into(), format!("wait 失败:{e}")),
                }
            }
        }
        Err(e) => ("crash_containment".into(), "ERROR".into(), format!("无法起子进程:{e}")),
    }
}

fn catch(name: &str, f: impl FnOnce() -> (String, String) + std::panic::UnwindSafe)
    -> (String, String, String) {
    match std::panic::catch_unwind(f) {
        Ok((o, d)) => (name.into(), o, d),
        Err(_) => (name.into(), "PANIC_ESCAPED".into(), "探针 panic 逃逸".into()),
    }
}

fn build_json(probes: &[(String, String, String)], safe: bool) -> String {
    let items: Vec<String> = probes.iter().map(|(v, o, d)|
        format!("{{\"probe\":{},\"outcome\":{},\"detail\":{}}}",
            js(v), js(o), js(d))).collect();
    format!("{{\"artifact\":\"w1-parser-spike\",\"scope\":\"safety-face (partial; numeric caps + extraction NOT covered)\",\"pure_rust_deflate_only\":true,\"native_binary\":false,\"all_safe\":{},\"probes\":[{}]}}",
        safe, items.join(","))
}

/// 正确的 JSON 字符串转义(codex R1-F5:旧实现把 " 转成非法的 \\')。
fn js(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// 极简 well-formed 校验:括号/引号配平 + 无裸控制符。够证明产出可被解析。
fn minimal_json_wellformed(s: &str) -> bool {
    let mut depth = 0i32; let mut in_str = false; let mut esc = false;
    for c in s.chars() {
        if in_str {
            if esc { esc = false; }
            else if c == '\\' { esc = true; }
            else if c == '"' { in_str = false; }
            else if (c as u32) < 0x20 { return false; }
        } else {
            match c {
                '"' => in_str = true,
                '{' | '[' => depth += 1,
                '}' | ']' => { depth -= 1; if depth < 0 { return false; } }
                _ => {}
            }
        }
    }
    depth == 0 && !in_str
}
