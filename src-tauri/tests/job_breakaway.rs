//! #129：Job Object 与更新安装器的生死关系——事故复现 + 修复验证。
//!
//! 场景 A（事故留档，RED 形状）：KILL_ON_JOB_CLOSE 的 Job 里，helper 用
//! **普通方式**起孙进程 → 关 Job → 孙进程死。这就是 2026-07-30 真机验收
//! 抓到的事故：插件 ShellExecuteW 起的安装器随应用退出被瞬杀。
//!
//! 场景 B（修复验证）：Job 加 BREAKAWAY_OK，helper 改走生产入口
//! `updater_launch::win::spawn_breakaway_verified`（spawn + 原始句柄存活
//! 确认）→ 关 Job → 孙进程**存活**。
//!
//! harness=false：helper 模式需要本 exe 自我重入（breakaway 必须由 Job 内
//! 的进程发起，只有我们自己的代码能带 CREATE_BREAKAWAY_FROM_JOB 标志）。
//!
//! 不链接 wancode_lib，而是 #[path] 直接把同一份源文件编进本 crate：引擎
//! workspace 的 dev profile 是 panic=abort，而 cargo 强制测试目标 unwind，
//! 链 lib 会因 panic 策略不兼容在 CI 上编译失败——测的仍是逐字同一实现。
#![cfg(windows)]

#[path = "../src/updater_launch.rs"]
mod updater_launch;

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};

const GRANDCHILD: &str = "C:\\Windows\\System32\\PING.EXE";
const GRANDCHILD_ARGS: [&str; 3] = ["-n", "60", "127.0.0.1"];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 4 && args[1] == "__job_helper" {
        helper(&args[2], PathBuf::from(&args[3]));
        return;
    }

    let mut failed = 0;
    failed += scenario(
        "A 事故留档：普通 spawn 的孙进程随 Job 关闭被杀",
        false,
        false, // 期望：死
    );
    failed += scenario(
        "B 修复验证：spawn_breakaway 的孙进程活过 Job 关闭",
        true,
        true, // 期望：活
    );
    if failed > 0 {
        eprintln!("job_breakaway: {failed} scenario(s) FAILED");
        std::process::exit(1);
    }
    println!("job_breakaway: all scenarios PASS");
}

/// helper：在 Job 内运行。等 go 文件（父进程完成 Assign 后写入，消除
/// "spawn 早于入 Job"的竞态）→ 起孙进程 → 写 pid 文件 → 常驻等死。
fn helper(mode: &str, dir: PathBuf) {
    let go = dir.join("go");
    for _ in 0..200 {
        if go.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let pid = match mode {
        // 走生产真实使用的 verified 入口（spawn + 原始句柄存活确认），
        // 把这条链整段锁进测试。孙进程 ping 常驻 60s，300ms 存活检查必过。
        "breakaway" => updater_launch::win::spawn_breakaway_verified(
            std::path::Path::new(GRANDCHILD),
            &GRANDCHILD_ARGS,
            300,
        )
        .expect("spawn_breakaway_verified"),
        _ => std::process::Command::new(GRANDCHILD)
            .args(GRANDCHILD_ARGS)
            .spawn()
            .expect("plain spawn")
            .id(),
    };
    let mut f = std::fs::File::create(dir.join("pid")).unwrap();
    write!(f, "{pid}").unwrap();
    // 常驻到被 Job 收割——helper 本身必须死于关 Job，这是场景前提。
    std::thread::sleep(Duration::from_secs(120));
}

fn scenario(name: &str, breakaway_ok: bool, expect_alive: bool) -> u32 {
    let dir = std::env::temp_dir().join(format!(
        "wancode-jobtest-{}-{}",
        if breakaway_ok { "b" } else { "p" },
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        assert!(!job.is_null(), "CreateJobObject");
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | if breakaway_ok {
                JOB_OBJECT_LIMIT_BREAKAWAY_OK
            } else {
                0
            };
        assert_ne!(
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ),
            0,
            "SetInformationJobObject"
        );

        let mode = if breakaway_ok { "breakaway" } else { "plain" };
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["__job_helper", mode, dir.to_str().unwrap()])
            .spawn()
            .expect("spawn helper");
        // 先入 Job 再放行（go 文件），杜绝孙进程赶在 Assign 前出生。
        use std::os::windows::io::AsRawHandle;
        assert_ne!(
            AssignProcessToJobObject(job, child.as_raw_handle()),
            0,
            "AssignProcessToJobObject（若本测试自身已在禁嵌套 Job 中会失败）"
        );
        std::fs::write(dir.join("go"), b"1").unwrap();

        // 等孙进程 pid 落盘
        let pid_file = dir.join("pid");
        let mut pid: u32 = 0;
        for _ in 0..200 {
            if let Ok(s) = std::fs::read_to_string(&pid_file) {
                pid = s.trim().parse().unwrap();
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert_ne!(pid, 0, "grandchild pid");
        assert!(
            updater_launch::win::process_alive(pid),
            "孙进程应已在跑"
        );

        // 关 Job（KILL_ON_JOB_CLOSE 生效瞬间）
        CloseHandle(job);
        std::thread::sleep(Duration::from_millis(400));

        let alive = updater_launch::win::process_alive(pid);
        // 清理孙进程（若存活）
        if alive {
            let h = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if !h.is_null() {
                TerminateProcess(h, 0);
                CloseHandle(h);
            }
        }
        let _ = std::fs::remove_dir_all(&dir);

        if alive == expect_alive {
            println!("[PASS] {name} (alive={alive})");
            0
        } else {
            eprintln!("[FAIL] {name}: alive={alive}, expected {expect_alive}");
            1
        }
    }
}
