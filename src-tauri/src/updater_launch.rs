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

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, GetExitCodeProcess, OpenProcess, CREATE_BREAKAWAY_FROM_JOB,
        CREATE_NEW_PROCESS_GROUP, PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
        STARTUPINFOW,
    };

    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    /// 已启动的 breakaway 进程。持有 CreateProcessW 返回的**原始句柄**做
    /// 存活检查——按 pid 重开句柄有 PID 复用竞态（安装器速死后 pid 被
    /// 别的进程占用会误判"还活着"），原始句柄永远指向那一个进程。
    pub struct Spawned {
        pid: u32,
        handle: HANDLE,
    }

    impl Spawned {
        pub fn pid(&self) -> u32 {
            self.pid
        }

        /// 经原始句柄查询：进程仍在运行（STILL_ACTIVE）。
        pub fn alive(&self) -> bool {
            let mut code: u32 = 0;
            let ok = unsafe { GetExitCodeProcess(self.handle, &mut code) };
            ok != 0 && code == STILL_ACTIVE as u32
        }
    }

    impl Drop for Spawned {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }

    /// 以 CREATE_BREAKAWAY_FROM_JOB 启动 `exe args...`。
    ///
    /// 命令行手工引号包裹：安装器路径在 %TEMP% 下必含空格概率低但不赌。
    /// args 由调用方按 NSIS 规则预先转义（见 escape_nsis_current_exe_arg）。
    pub fn spawn_breakaway(exe: &Path, args: &[&str]) -> std::io::Result<Spawned> {
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
        unsafe {
            CloseHandle(pi.hThread);
        }
        Ok(Spawned {
            pid: pi.dwProcessId,
            handle: pi.hProcess,
        })
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

    /// 启动并确认存活：spawn → 等 `settle_ms` → 原始句柄仍 STILL_ACTIVE
    /// 才算成功。
    ///
    /// "短暂检查"不能证明安装一定成功（那由下次启动的版本对账负责，#121），
    /// 但能把这次事故的形状——**起来即死/根本没起来**——当场变成可见错误。
    pub fn spawn_breakaway_verified(
        exe: &Path,
        args: &[&str],
        settle_ms: u64,
    ) -> std::io::Result<u32> {
        let child = spawn_breakaway(exe, args)?;
        std::thread::sleep(std::time::Duration::from_millis(settle_ms));
        if !child.alive() {
            return Err(std::io::Error::other(format!(
                "installer (pid {}) exited within {settle_ms}ms of launch",
                child.pid()
            )));
        }
        Ok(child.pid())
    }

    /// 复刻 tauri-plugin-updater 2.10.1 的 NSIS 参数转义（其 fn 为私有，
    /// 无法直接引用）。用于 `/ARGS` 后透传应用当前启动参数——安装器 `/R`
    /// 重启应用时以此恢复启动上下文。相比 std 的规则额外转义 `/`。
    #[allow(dead_code)] // Kept as a parity-tested contract for the updater handoff.
    pub fn escape_nsis_current_exe_arg(arg: &OsStr) -> String {
        let arg = arg.to_string_lossy();
        let mut cmd: Vec<char> = Vec::new();

        let quote = arg.chars().any(|c| c == ' ' || c == '\t' || c == '/') || arg.is_empty();
        if quote {
            cmd.push('"');
        }
        let mut backslashes: usize = 0;
        for x in arg.chars() {
            if x == '\\' {
                backslashes += 1;
            } else {
                if x == '"' {
                    cmd.extend((0..=backslashes).map(|_| '\\'));
                }
                backslashes = 0;
            }
            cmd.push(x);
        }
        if quote {
            cmd.extend((0..backslashes).map(|_| '\\'));
            cmd.push('"');
        }
        cmd.into_iter().collect()
    }

    #[cfg(test)]
    mod tests {
        /// 用例逐字取自插件自带测试 it_escapes_correctly_for_nsis——
        /// 保证与被复刻实现行为一致。
        #[test]
        fn escape_matches_plugin_behavior() {
            use std::ffi::OsStr;

            let cases = [
                ("something", "something"),
                ("--flag", "--flag"),
                ("--empty=", "--empty="),
                ("--arg=value", "--arg=value"),
                ("some space", "\"some space\""),
                ("--arg value", "\"--arg value\""),
                ("--arg=unwrapped space", "\"--arg=unwrapped space\""),
                ("--arg=\"wrapped\"", "--arg=\\\"wrapped\\\""),
                ("--arg=\"wrapped space\"", "\"--arg=\\\"wrapped space\\\"\""),
                (
                    "--arg=midword\"wrapped space\"",
                    "\"--arg=midword\\\"wrapped space\\\"\"",
                ),
                ("", "\"\""),
            ];
            for (orig, escaped) in cases {
                assert_eq!(super::escape_nsis_current_exe_arg(OsStr::new(orig)), escaped);
            }
        }
    }
}
