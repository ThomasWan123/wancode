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

use std::time::{Duration, Instant};
use wancode_lib::work_parse_worker::{
    parse_in_worker, run_as_worker_if_requested, DocKind, ParseFailure, ParseLimits,
    ParseRequest, ParsedDoc,
};

fn req() -> ParseRequest {
    // 外壳不解析内容，但会 stat 原件，所以必须给一个真实存在的文件。
    ParseRequest {
        kind: DocKind::Pdf,
        source_path: std::env::current_exe().unwrap().display().to_string(),
    }
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
    let r = with_mode(Some("echo"), || parse_in_worker(&req(), ParseLimits::default()));
    check(
        "positive_control_echo",
        matches!(&r, Ok(ParsedDoc::Echo { text }) if text.starts_with("echo:")),
        format!("{r:?}"),
    );

    // ② PDF 解析器尚未接入 → **有序拒收**，不是崩溃，证明 dispatch 走到了。
    let r = with_mode(None, || parse_in_worker(&req(), ParseLimits::default()));
    check(
        "unwired_kind_is_orderly_rejection",
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
        format!("{r:?} 用时={took:?}（必须远早于 60s 墙钟，否则说明是被超时兜住的，不是被上限挡住的）"),
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
    // ⑩ 端到端：真实 DOCX 走完整 worker 路径（隔离 + 解析 + 协议往返）。
    //    样本不入库，未设环境变量即跳过——CI 上这条永远 NOT-RUN。
    match std::env::var("WANCODE_DOCX_SAMPLE") {
        Ok(sample) => {
            let r = parse_in_worker(
                &ParseRequest {
                    kind: DocKind::Docx,
                    source_path: sample,
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
        }
        Err(_) => println!("SKIP real_docx_end_to_end_through_worker — 未设 WANCODE_DOCX_SAMPLE"),
    }

    // ⑩ try_wait 出错必须走同一清理出口（#56 R2-P1）。
    //    hang worker 会一直活着：旧实现直接 return SpawnFailed，不杀 Job、
    //    不 join 读线程，worker 残留。新实现必须 SpawnFailed **且** PID 已死，
    //    且远早于墙钟（证明不是被 Timeout 兜住的）。
    let pidfile = std::env::temp_dir().join(format!(
        "wancode-waiterr-{}.pid",
        std::process::id()
    ));
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
