//! #129：从 WanCode 的 Job Object 里安全拉起更新安装器。
//!
//! 事故链（2026-07-30 真机实锤）：插件 `install()` 用 `ShellExecuteW` 拉起
//! NSIS 后立即 `exit(0)`；安装器继承本进程的 Job（lib.rs 的进程树治理，
//! KILL_ON_JOB_CLOSE）；应用一退出 Job 关闭，安装器被瞬杀——版本不变、
//! 零反馈。应用内自动更新自 v0.17.0（Job 引入）起全坏。
//!
//! 修复三要素，缺一不可：
//!   1. Job 加 `JOB_OBJECT_LIMIT_BREAKAWAY_OK`（lib.rs）——只是"允许"，
//!      不是"自动"；绝不用 SILENT_BREAKAWAY_OK，否则 AI 子进程也会
//!      逃出治理。
//!   2. 安装器必须以 `CREATE_BREAKAWAY_FROM_JOB` **显式**创建（本模块）。
//!      继续用 ShellExecuteW 无法传该标志，孩子照样进 Job。
//!   3. 启动后短暂确认进程仍存活才允许退出应用；起不来必须留在应用内
//!      报错——"安装器没起来"与"安装成功"绝不能再在退出瞬间不可区分。

#[cfg(windows)]
pub mod win {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, GetExitCodeProcess, OpenProcess, CREATE_BREAKAWAY_FROM_JOB,
        CREATE_NEW_PROCESS_GROUP, PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
        STARTUPINFOW,
    };

    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    /// 以 CREATE_BREAKAWAY_FROM_JOB 启动 `exe args...`，返回 pid。
    ///
    /// 命令行手工引号包裹：安装器路径在 %TEMP% 下必含空格概率低但不赌。
    /// NSIS 参数（/P /R /UPDATE）与 tauri-plugin-updater 2.10.1 完全一致，
    /// 这是 E2E 已验证过的参数组合。
    pub fn spawn_breakaway(exe: &Path, args: &[&str]) -> std::io::Result<u32> {
        let exe_w = wide(exe.as_os_str());
        let mut cmdline = format!("\"{}\"", exe.display());
        for a in args {
            cmdline.push(' ');
            cmdline.push_str(a);
        }
        let mut cmdline_w: Vec<u16> = OsStr::new(&cmdline)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

        let ok = unsafe {
            CreateProcessW(
                exe_w.as_ptr(),
                cmdline_w.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                CREATE_BREAKAWAY_FROM_JOB | CREATE_NEW_PROCESS_GROUP,
                std::ptr::null(),
                std::ptr::null(),
                &si,
                &mut pi,
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error());
        }
        let pid = pi.dwProcessId;
        unsafe {
            CloseHandle(pi.hThread);
            CloseHandle(pi.hProcess);
        }
        Ok(pid)
    }

    /// 进程是否仍存活（STILL_ACTIVE）。打不开句柄（已退出被回收）视为死。
    pub fn process_alive(pid: u32) -> bool {
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return false;
            }
            let mut code: u32 = 0;
            let ok = GetExitCodeProcess(h, &mut code);
            CloseHandle(h);
            ok != 0 && code == STILL_ACTIVE as u32
        }
    }

    /// 启动并确认存活：spawn → 等 `settle_ms` → 仍在跑才算成功。
    ///
    /// "短暂检查"不能证明安装一定成功（那由下次启动的版本对账负责，#121），
    /// 但能把这次事故的形状——**起来即死/根本没起来**——当场变成可见错误。
    pub fn spawn_breakaway_verified(
        exe: &Path,
        args: &[&str],
        settle_ms: u64,
    ) -> std::io::Result<u32> {
        let pid = spawn_breakaway(exe, args)?;
        std::thread::sleep(std::time::Duration::from_millis(settle_ms));
        if !process_alive(pid) {
            return Err(std::io::Error::other(format!(
                "installer (pid {pid}) exited within {settle_ms}ms of launch"
            )));
        }
        Ok(pid)
    }
}
