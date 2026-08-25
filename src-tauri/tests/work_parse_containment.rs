//! W3-P2 崩溃遏制外壳的对抗测试。
//!
//! 关harness 自写 main 的原因和 `job_breakaway` 一样：worker 是**自我重入**
//! 的——父进程起的是 `current_exe()`，在测试里就是本测试二进制。所以 main
//! 必须最先分流 worker 分支，标准 harness 承载不了。
//!
//! 设计原则：每条断言都要能**失败**。
//!   - 正对照 `echo` 必须成功——没有它，「外壳把一切都判成失败」会看起来全绿；
//!   - 每条负例断言的是**具体那一种**失败（Crashed / Timeout / BadOutput /
//!     OutputTooLarge），不是「反正 is_err()」。笼统的 is_err() 会让超时被
//!     误判成崩溃、死锁被误判成超时，恰好放过这类外壳最容易出的错。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use wancode_lib::work_context::build_work_prompt;
use wancode_lib::work_import::import_document;
use wancode_lib::work_parse_worker::{
    parse_in_worker, run_as_worker_if_requested, DocKind, ParseFailure, ParseLimits, ParseRequest,
    ParsedDoc,
};
use wancode_lib::work_staging::{workspace_dir_under, WorkspaceId};

#[allow(clippy::permissions_set_readonly_false)] // Windows ACL semantics are not Unix world-write.
fn make_owner_writable(path: &Path, mut permissions: std::fs::Permissions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(permissions.mode() | 0o200);
    }
    #[cfg(not(unix))]
    permissions.set_readonly(false);
    let _ = std::fs::set_permissions(path, permissions);
}

fn req() -> ParseRequest {
    // 外壳不解析内容，但会 stat 原件，所以必须给一个真实存在的文件。
    ParseRequest {
        kind: DocKind::Pdf,
        source_path: std::env::current_exe().unwrap().display().to_string(),
    }
}

fn materialize_real_docx_fixture(dir: &Path) -> PathBuf {
    let encoded = include_str!("fixtures/project-orion.docx.b64").trim();
    let bytes = decode_base64(encoded).expect("committed DOCX fixture must be valid base64");
    assert_eq!(
        bytes.len(),
        39_603,
        "committed DOCX fixture changed unexpectedly"
    );
    let path = dir.join("Project-Orion-professional.docx");
    std::fs::write(&path, bytes).expect("write DOCX fixture");
    path
}

fn decode_base64(input: &str) -> Result<Vec<u8>, String> {
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut accumulator = 0u32;
    let mut bits = 0u8;
    for byte in input.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        if byte == b'=' {
            break;
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return Err(format!("invalid base64 byte {byte}")),
        };
        accumulator = (accumulator << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((accumulator >> bits) as u8);
            accumulator &= (1u32 << bits) - 1;
        }
    }
    Ok(output)
}

/// Build a standards-compliant one-page PDF without relying on a second PDF
/// library. PDFium must parse the resulting cross-reference table and extract
/// the text through the same production path used for user documents.
fn materialize_text_pdf_fixture(dir: &Path, name: &str, text: &str) -> PathBuf {
    let stream = format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>".to_string(),
        format!("<< /Length {} >>\nstream\n{}\nendstream", stream.len(), stream),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    let mut pdf = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", index + 1, object).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    let path = dir.join(name);
    std::fs::write(&path, pdf).expect("write PDF fixture");
    path
}

fn materialize_office_fixture(dir: &Path, name: &str, entries: &[(&str, &str)]) -> PathBuf {
    let path = dir.join(name);
    let file = std::fs::File::create(&path).expect("create Office fixture");
    let mut package = zip::ZipWriter::new(file);
    for (entry_name, body) in entries {
        package
            .start_file(*entry_name, zip::write::SimpleFileOptions::default())
            .expect("start Office fixture entry");
        package
            .write_all(body.as_bytes())
            .expect("write Office fixture entry");
    }
    package.finish().expect("finish Office fixture");
    path
}

fn with_mode<T>(mode: Option<&str>, f: impl FnOnce() -> T) -> T {
    match mode {
        // SAFETY: 本测试 main 串行执行，无并发线程读环境变量
        Some(m) => unsafe { std::env::set_var("WANCODE_PARSE_WORKER_SELFTEST", m) },
        None => unsafe { std::env::remove_var("WANCODE_PARSE_WORKER_SELFTEST") },
    }
    let out = f();
    unsafe { std::env::remove_var("WANCODE_PARSE_WORKER_SELFTEST") };
    out
}

#[cfg(windows)]
fn process_still_running(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    const STILL_ACTIVE: u32 = 259;
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        ok != 0 && code == STILL_ACTIVE
    }
}

#[cfg(not(windows))]
fn process_still_running(_pid: u32) -> bool {
    false
}

fn main() {
    // 必须最先：本进程可能是被父进程重入起来的 worker。
    run_as_worker_if_requested();

    // Fixtures are materialized only in the parent. The worker sees ordinary
    // files and therefore exercises the exact production parser boundary.
    let fixtures = tempfile::tempdir().expect("fixture tempdir");
    let docx_sample = materialize_real_docx_fixture(fixtures.path());
    let pdf_sample = materialize_text_pdf_fixture(
        fixtures.path(),
        "project-orion-text.pdf",
        "Project Orion budget 128400 owner Lin Mei deadline 2026-09-30 risk AMBER",
    );
    let blank_pdf = materialize_text_pdf_fixture(fixtures.path(), "image-only.pdf", "");
    let xlsx_sample = materialize_office_fixture(
        fixtures.path(),
        "project-orion.xlsx",
        &[
            ("xl/sharedStrings.xml", r#"<sst><si><t>Budget</t></si></sst>"#),
            ("xl/worksheets/sheet1.xml", r#"<worksheet><c r="A1" t="s"><v>0</v></c><c r="B1"><v>128400</v></c></worksheet>"#),
        ],
    );
    let pptx_sample = materialize_office_fixture(
        fixtures.path(),
        "project-orion.pptx",
        &[(
            "ppt/slides/slide1.xml",
            r#"<p:sld xmlns:p="p" xmlns:a="a"><a:t>Project Orion</a:t><a:t>Risk AMBER</a:t></p:sld>"#,
        )],
    );

    let mut pass = 0usize;
    let mut fail = 0usize;
    let mut check = |name: &str, ok: bool, detail: String| {
        if ok {
            pass += 1;
            println!("PASS {name} — {detail}");
        } else {
            fail += 1;
            println!("FAIL {name} — {detail}");
        }
    };

    // ① 正对照：worker 正常跑通。没有这条，全部失败也会"全绿"。
    let r = with_mode(Some("echo"), || {
        parse_in_worker(&req(), ParseLimits::default())
    });
    check(
        "positive_control_echo",
        matches!(&r, Ok(ParsedDoc::Echo { text }) if text.starts_with("echo:")),
        format!("{r:?}"),
    );

    // ② 畸形 PDF → **有序拒收**，不是崩溃，证明 dispatch 走到了。
    let r = with_mode(None, || parse_in_worker(&req(), ParseLimits::default()));
    check(
        "malformed_pdf_is_orderly_rejection",
        matches!(&r, Err(ParseFailure::Rejected(_))),
        format!("{r:?}"),
    );

    // ③ 原生崩溃替身：worker abort → 父进程必须报 Crashed 且自己活着。
    let r = with_mode(Some("abort"), || {
        parse_in_worker(&req(), ParseLimits::default())
    });
    check(
        "abort_is_contained_as_crashed",
        matches!(&r, Err(ParseFailure::Crashed { .. })),
        format!("{r:?}"),
    );

    // ④ 挂死：必须在时限附近返回 Timeout，而不是永远等下去。
    //    同时断言**用时上界**——只断言 Err 的话，一个立刻返回的 bug
    //    也会"通过"，而那恰恰说明超时机制没跑。
    let limits = ParseLimits {
        wall_clock: Duration::from_secs(2),
        ..ParseLimits::default()
    };
    let t0 = Instant::now();
    let r = with_mode(Some("hang"), || parse_in_worker(&req(), limits));
    let took = t0.elapsed();
    check(
        "hang_is_killed_at_deadline",
        matches!(&r, Err(ParseFailure::Timeout { .. }))
            && took >= Duration::from_secs(2)
            && took < Duration::from_secs(15),
        format!("{r:?} 用时={took:?}（必须 ≥2s 证明真等到了时限，<15s 证明没挂死）"),
    );

    // ⑤ 协议被破坏：正常退出但输出不是 JSON → BadOutput，不能当成成功。
    let r = with_mode(Some("garbage"), || {
        parse_in_worker(&req(), ParseLimits::default())
    });
    check(
        "non_json_output_is_bad_output",
        matches!(&r, Err(ParseFailure::BadOutput(_))),
        format!("{r:?}"),
    );

    // ⑥ 输出洪水：这是本外壳最容易出的 bug——父进程不读管道 → worker 阻塞
    //    在 write 上 → 看起来像挂死。必须报 OutputTooLarge（而非 Timeout），
    //    且远早于墙钟时限返回。
    let limits = ParseLimits {
        max_output_bytes: 4 * 1024 * 1024,
        wall_clock: Duration::from_secs(60),
        ..ParseLimits::default()
    };
    let t0 = Instant::now();
    let r = with_mode(Some("flood"), || parse_in_worker(&req(), limits));
    let took = t0.elapsed();
    check(
        "output_flood_is_capped_not_deadlocked",
        matches!(&r, Err(ParseFailure::OutputTooLarge { .. })) && took < Duration::from_secs(60),
        format!(
            "{r:?} 用时={took:?}（必须远早于 60s 墙钟，否则说明是被超时兜住的，不是被上限挡住的）"
        ),
    );

    // ⑦ 输入超限：**根本不起 worker**（省掉一次进程创建，也不让超大文件
    //    进入解析路径）。
    let limits = ParseLimits {
        max_input_bytes: 1,
        ..ParseLimits::default()
    };
    let r = parse_in_worker(&req(), limits);
    check(
        "oversize_input_rejected_before_spawn",
        matches!(&r, Err(ParseFailure::InputTooLarge { cap: 1, .. })),
        format!("{r:?}"),
    );

    // ⑧ 原件不存在。
    let bad = ParseRequest {
        kind: DocKind::Docx,
        source_path: "Z:\\definitely\\missing\\file.docx".into(),
    };
    let r = parse_in_worker(&bad, ParseLimits::default());
    check(
        "missing_source_is_unreadable",
        matches!(&r, Err(ParseFailure::SourceUnreadable(_))),
        format!("{r:?}"),
    );

    // ⑨ Job 不可用 → **拒绝解析**，不降级运行（z-code #56 R1-P2-1）。
    //    这条存在的意义是让 ContainmentUnavailable 可被证伪——没有注入点，
    //    那条分支就是永远跑不到、无人验证的死代码。
    let r = with_mode(Some("nojob"), || {
        parse_in_worker(&req(), ParseLimits::default())
    });
    check(
        "no_job_means_refuse_not_degrade",
        matches!(&r, Err(ParseFailure::ContainmentUnavailable(_))),
        format!("{r:?}"),
    );
    // ⑩ 端到端：提交的真实 DOCX 固定样本走完整 worker 路径。该样本不再
    //    依赖环境变量，因此本地和 CI 都不允许 SKIP。
    {
        let sample_path = docx_sample.clone();
        let r = parse_in_worker(
            &ParseRequest {
                kind: DocKind::Docx,
                source_path: sample_path.to_string_lossy().into_owned(),
            },
            ParseLimits::default(),
        );
        check(
            "real_docx_end_to_end_through_worker",
            matches!(&r, Ok(ParsedDoc::Docx { blocks })
                    if !blocks.is_empty() && blocks.iter().all(|b| b.is_well_formed())),
            match &r {
                Ok(ParsedDoc::Docx { blocks }) => {
                    format!("块={}，全部 well-formed", blocks.len())
                }
                other => format!("{other:?}"),
            },
        );

        // ⑪ 产品链正/负对照：真实导入清单 → 身份复核 → worker 解析 →
        // JSONL 上下文；随后篡改暂存副本必须在送模前被 SHA 门拒绝。
        let tmp = tempfile::tempdir().expect("work context tempdir");
        let ws = WorkspaceId::mint();
        let imported = import_document(tmp.path(), &ws, &sample_path);
        let prompt = imported
            .as_ref()
            .map_err(|e| e.to_string())
            .and_then(|_| build_work_prompt(tmp.path(), &ws, "给出运营摘要"));
        check(
            "real_docx_import_to_model_context",
            matches!(&prompt, Ok(text)
                    if text.contains("UNTRUSTED DATA")
                        && text.contains("\"block_path\"")
                        && text.ends_with("给出运营摘要")),
            prompt
                .as_ref()
                .map(|p| format!("context_utf16={}", p.encode_utf16().count()))
                .unwrap_or_else(|e| e.clone()),
        );
        if let Ok(record) = imported {
            let staged =
                workspace_dir_under(tmp.path().to_path_buf(), &ws).join(record.staging_rel_path);
            if let Ok(meta) = std::fs::metadata(&staged) {
                make_owner_writable(&staged, meta.permissions());
                let _ = std::fs::write(&staged, b"tampered");
            }
            let tampered = build_work_prompt(tmp.path(), &ws, "给出运营摘要");
            check(
                "tampered_staged_doc_is_rejected_before_model",
                matches!(&tampered, Err(message) if message.contains("SHA-256")),
                format!("{tampered:?}"),
            );
        }
    }

    // ⑫ PDF 产品链：有效 PDF → worker/PDFium → 导入清单 → SHA 复核 →
    // 模型上下文。页路径必须稳定且正文必须真的进入上下文。
    let r = parse_in_worker(
        &ParseRequest {
            kind: DocKind::Pdf,
            source_path: pdf_sample.to_string_lossy().into_owned(),
        },
        ParseLimits::default(),
    );
    check(
        "pdf_end_to_end_through_worker",
        matches!(&r, Ok(ParsedDoc::Pdf { blocks })
            if blocks.len() == 1
                && blocks[0].path == "page[1]/chunk[0]"
                && blocks[0].raw.contains("128400")
                && blocks[0].is_well_formed()),
        format!("{r:?}"),
    );
    // Each parser limit gets a positive document and a deliberately tiny cap.
    // The worker must reject for the named limit rather than crash or time out.
    for (mode, name, needle) in [
        ("pdf-page-cap", "pdf_page_count_cap", "超过上限 0"),
        ("pdf-page-text-cap", "pdf_per_page_text_cap", "第 1 页文本"),
        ("pdf-total-text-cap", "pdf_total_text_cap", "PDF 文本累计"),
    ] {
        let r = with_mode(Some(mode), || {
            parse_in_worker(
                &ParseRequest {
                    kind: DocKind::Pdf,
                    source_path: pdf_sample.to_string_lossy().into_owned(),
                },
                ParseLimits::default(),
            )
        });
        check(
            name,
            matches!(&r, Err(ParseFailure::Rejected(reason)) if reason.contains(needle)),
            format!("{r:?}"),
        );
    }
    let tmp = tempfile::tempdir().expect("PDF work context tempdir");
    let ws = WorkspaceId::mint();
    let imported = import_document(tmp.path(), &ws, &pdf_sample);
    let prompt = imported
        .as_ref()
        .map_err(|e| e.to_string())
        .and_then(|_| build_work_prompt(tmp.path(), &ws, "What is the budget?"));
    check(
        "pdf_import_to_model_context",
        matches!(&prompt, Ok(text)
            if text.contains("UNTRUSTED DATA")
                && text.contains("page[1]/chunk[0]")
                && text.contains("128400")
                && text.ends_with("What is the budget?")),
        format!("{prompt:?}"),
    );
    if let Ok(record) = imported {
        let staged =
            workspace_dir_under(tmp.path().to_path_buf(), &ws).join(record.staging_rel_path);
        if let Ok(meta) = std::fs::metadata(&staged) {
            make_owner_writable(&staged, meta.permissions());
            let _ = std::fs::write(&staged, b"tampered pdf");
        }
        let tampered = build_work_prompt(tmp.path(), &ws, "What is the budget?");
        check(
            "tampered_pdf_is_rejected_before_model",
            matches!(&tampered, Err(message) if message.contains("SHA-256")),
            format!("{tampered:?}"),
        );
    }

    let blank = parse_in_worker(
        &ParseRequest {
            kind: DocKind::Pdf,
            source_path: blank_pdf.to_string_lossy().into_owned(),
        },
        ParseLimits::default(),
    );
    check(
        "image_only_pdf_fails_with_truthful_ocr_boundary",
        matches!(&blank, Err(ParseFailure::Rejected(reason))
            if reason.contains("OCR") && reason.contains("没有可提取文字")),
        format!("{blank:?}"),
    );

    let mixed = tempfile::tempdir().expect("mixed document context tempdir");
    let mixed_ws = WorkspaceId::mint();
    let mixed_docx = import_document(mixed.path(), &mixed_ws, &docx_sample);
    let mixed_pdf = import_document(mixed.path(), &mixed_ws, &pdf_sample);
    let mixed_prompt = mixed_docx
        .as_ref()
        .map_err(|e| e.to_string())
        .and_then(|_| mixed_pdf.as_ref().map_err(|e| e.to_string()))
        .and_then(|_| build_work_prompt(mixed.path(), &mixed_ws, "Summarize both documents"));
    check(
        "mixed_pdf_docx_context_is_complete",
        matches!(&mixed_prompt, Ok(text)
            if text.contains("Project-Orion-professional.docx")
                && text.contains("project-orion-text.pdf")
                && text.contains("body/p[")
                && text.contains("page[1]/chunk[0]")),
        mixed_prompt
            .as_ref()
            .map(|text| format!("context_utf16={}", text.encode_utf16().count()))
            .unwrap_or_else(|error| error.clone()),
    );

    // Modern Office formats must cross the same worker, staging, hash, and
    // untrusted-context boundaries as PDF/DOCX. These are mandatory fixtures,
    // not environment-variable breadth probes.
    for (kind, source, expected_path, expected_text, name) in [
        (
            DocKind::Xlsx,
            &xlsx_sample,
            "workbook/sheet[1]/cell[B1]",
            "128400",
            "xlsx_end_to_end_through_worker",
        ),
        (
            DocKind::Pptx,
            &pptx_sample,
            "slides/slide[1]/text[1]",
            "Risk AMBER",
            "pptx_end_to_end_through_worker",
        ),
    ] {
        let parsed = parse_in_worker(
            &ParseRequest {
                kind,
                source_path: source.to_string_lossy().into_owned(),
            },
            ParseLimits::default(),
        );
        let parsed_ok = match (&kind, &parsed) {
            (DocKind::Xlsx, Ok(ParsedDoc::Xlsx { blocks }))
            | (DocKind::Pptx, Ok(ParsedDoc::Pptx { blocks })) => blocks.iter().any(|block| {
                block.path == expected_path
                    && block.raw.contains(expected_text)
                    && block.is_well_formed()
            }),
            _ => false,
        };
        check(name, parsed_ok, format!("{parsed:?}"));

        let tmp = tempfile::tempdir().expect("Office work context tempdir");
        let ws = WorkspaceId::mint();
        let imported = import_document(tmp.path(), &ws, source);
        let prompt = imported
            .as_ref()
            .map_err(|error| error.to_string())
            .and_then(|_| build_work_prompt(tmp.path(), &ws, "Summarize this file"));
        check(
            &format!("{name}_to_model_context"),
            matches!(&prompt, Ok(text)
                if text.contains(expected_path)
                    && text.contains(expected_text)
                    && text.ends_with("Summarize this file")),
            format!("{prompt:?}"),
        );
    }

    // Optional local breadth probe against a real-world PDF. CI correctness
    // does not depend on private files; when supplied, it still uses the exact
    // same contained production parser and contributes a named PASS/FAIL.
    if let Ok(real_pdf) = std::env::var("WANCODE_PDF_SAMPLE") {
        let real = parse_in_worker(
            &ParseRequest {
                kind: DocKind::Pdf,
                source_path: real_pdf,
            },
            ParseLimits::default(),
        );
        check(
            "real_world_pdf_sample_through_worker",
            matches!(&real, Ok(ParsedDoc::Pdf { blocks })
                if !blocks.is_empty() && blocks.iter().all(|block| block.is_well_formed())),
            match &real {
                Ok(ParsedDoc::Pdf { blocks }) => format!("pages_with_text={}", blocks.len()),
                other => format!("{other:?}"),
            },
        );
    }

    // ⑩ try_wait 出错必须走同一清理出口（#56 R2-P1）。
    //    hang worker 会一直活着：旧实现直接 return SpawnFailed，不杀 Job、
    //    不 join 读线程，worker 残留。新实现必须 SpawnFailed **且** PID 已死，
    //    且远早于墙钟（证明不是被 Timeout 兜住的）。
    let pidfile = std::env::temp_dir().join(format!("wancode-waiterr-{}.pid", std::process::id()));
    let _ = std::fs::remove_file(&pidfile);
    let limits = ParseLimits {
        wall_clock: Duration::from_secs(30),
        ..ParseLimits::default()
    };
    let t0 = Instant::now();
    let r = {
        unsafe {
            std::env::set_var("WANCODE_PARSE_WORKER_SELFTEST", "hang");
            std::env::set_var("WANCODE_PARSE_PARENT_SELFTEST", "waiterr");
            std::env::set_var("WANCODE_PARSE_WORKER_PIDFILE", &pidfile);
        }
        let out = parse_in_worker(&req(), limits);
        unsafe {
            std::env::remove_var("WANCODE_PARSE_WORKER_SELFTEST");
            std::env::remove_var("WANCODE_PARSE_PARENT_SELFTEST");
            std::env::remove_var("WANCODE_PARSE_WORKER_PIDFILE");
        }
        out
    };
    let took = t0.elapsed();
    let pid = std::fs::read_to_string(&pidfile)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok());
    let _ = std::fs::remove_file(&pidfile);
    let worker_gone = match pid {
        Some(p) => !process_still_running(p),
        None => false,
    };
    check(
        "wait_error_still_reaps_worker",
        matches!(&r, Err(ParseFailure::SpawnFailed(e)) if e.contains("try_wait"))
            && took < Duration::from_secs(5)
            && worker_gone,
        format!("{r:?} 用时={took:?} pid={pid:?} worker_gone={worker_gone}（必须 SpawnFailed、远早于 30s 墙钟、且 hang worker PID 已死）"),
    );

    println!("\nCONTAINMENT DONE pass={pass} fail={fail}");
    if fail > 0 {
        std::process::exit(1);
    }
}
