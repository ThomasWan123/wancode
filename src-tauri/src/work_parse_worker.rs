//! Work 解析的崩溃遏制外壳（设计 §1.1「崩溃遏制」）。
//!
//! 约束原文：「解析跑在独立工作进程（超时可杀），Pdfium 原生崩溃/panic
//! 不得带倒主应用；失败即整体拒收，暂存区无半成品。」
//!
//! 本模块**只做外壳**，不含任何解析逻辑——解析器随后续 PR 接入
//! `run_request` 的 dispatch。这样做是为了让「隔离是否真的成立」能被单独
//! 评审和单独证伪，不和解析器的正确性搅在一起。
//!
//! ## 为什么外壳能保证「暂存区无半成品」
//!
//! 不是靠 worker 自觉清理——崩溃的进程没有自觉。是靠**它根本没有写入路径**：
//! worker 从 stdin 收字节、往 stdout 吐字节，全程不碰暂存区，也不接收暂存区
//! 路径。父进程拿到一份**完整且合法**的响应后，才由自己写盘。worker 死在
//! 任何一步，暂存区都不曾被触碰过——零残留是结构性的，不是清理出来的。
//!
//! ## 进程树治理
//!
//! Windows 上 `Child::kill()` 只杀直接子进程，孙进程会变孤儿。W1 spike 里
//! 用的就是裸 kill，作为可行性探针够用，作为产品代码不够。这里每次调用建
//! 一个**独立 Job**，把 worker 塞进去，超时用 `TerminateJobObject` 整树清杀。
//!
//! 应用主 Job（`lib.rs::setup_job_object`）设了 8GB/进程——那是给主进程和
//! webview 留的余量，对**解析不受信文档**来说太松。嵌套 Job 的限额取交集，
//! 所以这里的小限额直接生效，不需要 breakaway（也**不应**breakaway：worker
//! 必须随应用退出一起死）。

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 触发 worker 模式的环境变量。父进程重入自身 exe 时设置。
pub const WORKER_ENV: &str = "WANCODE_PARSE_WORKER";

/// 解析请求（父 → 子，stdin 上一行 JSON）。
///
/// 注意这里传的是**原件路径**，不是暂存区路径：worker 只读原件，
/// 对暂存区一无所知。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ParseRequest {
    pub kind: DocKind,
    pub source_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocKind {
    Pdf,
    Docx,
}

/// 解析响应（子 → 父，stdout 上一行 JSON）。
///
/// `Err` 是 worker **有序**地拒收（例如格式不支持）；worker 崩溃/挂死不会
/// 产生任何响应，那走 [`ParseFailure`] 的另外几个分支。两者必须分开：
/// 有序拒收说明防线按预期工作，崩溃说明防线被打穿了但被壳兜住。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum ParseResponse {
    Ok { text: String },
    Err { reason: String },
}

/// 资源边界（设计 §1.1 安全面「资源边界实测并定档」）。
#[derive(Debug, Clone, Copy)]
pub struct ParseLimits {
    /// 输入体积上限：超限**根本不起 worker**。
    pub max_input_bytes: u64,
    /// 单文档解析墙钟时限：到点整树清杀。
    pub wall_clock: Duration,
    /// worker 进程内存上限（Job 强制）。
    pub max_process_bytes: u64,
    /// worker 响应体积上限：防止 worker 反过来把父进程撑爆。
    pub max_output_bytes: u64,
}

impl Default for ParseLimits {
    /// 初始档位。**这些数字是保守的起点，不是实测定档**——设计 §1.1 要求
    /// 「资源边界实测并定档」，实测随解析器接入那一 PR 做。
    fn default() -> Self {
        Self {
            max_input_bytes: 64 * 1024 * 1024,
            wall_clock: Duration::from_secs(30),
            max_process_bytes: 512 * 1024 * 1024,
            max_output_bytes: 32 * 1024 * 1024,
        }
    }
}

/// 整体拒收的原因。**没有"部分成功"这一档**——设计要求「失败即整体拒收」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseFailure {
    /// 输入超限，未起 worker。
    InputTooLarge { bytes: u64, cap: u64 },
    /// 原件不可读。
    SourceUnreadable(String),
    /// 起不来 worker（exe 定位失败/spawn 失败）。
    SpawnFailed(String),
    /// 超时，已整树清杀。
    Timeout { after: Duration },
    /// worker 非正常终止（原生崩溃/panic/被外部杀）。`code` 为退出码，
    /// Windows 上原生崩溃典型为 0xC0000005 一类。
    Crashed { code: Option<i32>, stderr: String },
    /// worker 正常退出但输出不是一份合法响应（协议被破坏）。
    BadOutput(String),
    /// 输出超限。
    OutputTooLarge { cap: u64 },
    /// worker 有序拒收。
    Rejected(String),
    /// **无法建立进程树遏制**（Job 不可用）。见 `job_for_child` 的说明：
    /// 这不是降级运行的理由，是拒绝解析的理由。
    ContainmentUnavailable(String),
}

impl std::fmt::Display for ParseFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InputTooLarge { bytes, cap } => {
                write!(f, "输入 {bytes} 字节超过上限 {cap}，未起解析进程")
            }
            Self::SourceUnreadable(e) => write!(f, "原件不可读：{e}"),
            Self::SpawnFailed(e) => write!(f, "解析进程起不来：{e}"),
            Self::Timeout { after } => write!(f, "解析超时（{:?}），进程树已清杀", after),
            Self::Crashed { code, stderr } => {
                write!(f, "解析进程非正常终止（退出码 {code:?}）：{stderr}")
            }
            Self::BadOutput(e) => write!(f, "解析进程输出不合协议：{e}"),
            Self::OutputTooLarge { cap } => write!(f, "解析输出超过上限 {cap} 字节"),
            Self::Rejected(r) => write!(f, "解析拒收：{r}"),
            Self::ContainmentUnavailable(e) => {
                write!(f, "无法建立进程树遏制（{e}），拒绝解析不受信文档")
            }
        }
    }
}

// ───────────────────────── 子进程侧 ─────────────────────────

/// 若本进程是解析 worker，就跑解析并**退出进程**，永不返回。
///
/// 必须在 `run()` 最早处调用：worker 不该建应用 Job、不该起 tauri、不该碰
/// 任何应用状态。它是个哑管道。
pub fn run_as_worker_if_requested() {
    if std::env::var(WORKER_ENV).as_deref() != Ok("1") {
        return;
    }
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        std::process::exit(2);
    }
    let resp = match serde_json::from_str::<ParseRequest>(&input) {
        Ok(req) => run_request(&req),
        Err(e) => ParseResponse::Err {
            reason: format!("请求不合协议：{e}"),
        },
    };
    // 序列化失败不该发生；真发生了就以非零退出，让父进程走 BadOutput，
    // 而不是吐半截 JSON。
    match serde_json::to_string(&resp) {
        Ok(s) => {
            let mut out = std::io::stdout();
            if out.write_all(s.as_bytes()).is_err() || out.flush().is_err() {
                std::process::exit(3);
            }
            std::process::exit(0);
        }
        Err(_) => std::process::exit(3),
    }
}

/// 实际解析。**本 PR 尚未接入解析器**——见文件头。
fn run_request(req: &ParseRequest) -> ParseResponse {
    // 自测钩子：让隔离本身可被证伪（见 tests）。仅在 worker 进程内生效，
    // 不影响产品路径。
    if let Ok(mode) = std::env::var("WANCODE_PARSE_WORKER_SELFTEST") {
        match mode.as_str() {
            // 原生崩溃的替身：非正常终止，父进程必须报 Crashed 而非挂死。
            "abort" => std::process::abort(),
            "hang" => loop {
                std::thread::sleep(Duration::from_secs(3600));
            },
            "garbage" => {
                print!("这不是 JSON");
                let _ = std::io::stdout().flush();
                std::process::exit(0);
            }
            "flood" => {
                // 吐超量输出：验证父进程不是无脑读到内存耗尽。
                let chunk = "x".repeat(1024 * 1024);
                let mut out = std::io::stdout();
                loop {
                    if out.write_all(chunk.as_bytes()).is_err() || out.flush().is_err() {
                        std::process::exit(0);
                    }
                }
            }
            "echo" => {
                return ParseResponse::Ok {
                    text: format!("echo:{}", req.source_path),
                }
            }
            _ => {}
        }
    }
    ParseResponse::Err {
        reason: "解析器尚未接入（W3-P2 外壳 PR）".to_string(),
    }
}

// ───────────────────────── 父进程侧 ─────────────────────────

/// 在隔离 worker 里解析一份文档。
///
/// 无论 worker 怎么死，本函数都返回；调用方**永远**只能拿到「完整成功」或
/// 「整体拒收」，没有中间态。
pub fn parse_in_worker(req: &ParseRequest, limits: ParseLimits) -> Result<String, ParseFailure> {
    let meta = std::fs::metadata(&req.source_path)
        .map_err(|e| ParseFailure::SourceUnreadable(e.to_string()))?;
    if meta.len() > limits.max_input_bytes {
        return Err(ParseFailure::InputTooLarge {
            bytes: meta.len(),
            cap: limits.max_input_bytes,
        });
    }

    let exe = std::env::current_exe().map_err(|e| ParseFailure::SpawnFailed(e.to_string()))?;
    let payload =
        serde_json::to_string(req).map_err(|e| ParseFailure::SpawnFailed(e.to_string()))?;

    let mut cmd = Command::new(exe);
    cmd.env(WORKER_ENV, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // 自测模式透传，产品路径下该变量不存在。
    if let Ok(v) = std::env::var("WANCODE_PARSE_WORKER_SELFTEST") {
        cmd.env("WANCODE_PARSE_WORKER_SELFTEST", v);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| ParseFailure::SpawnFailed(e.to_string()))?;

    // 进程树治理：建独立 Job 并纳入 worker。失败不阻断解析——退化为
    // 「只能杀直接子进程」，比不解析强；但必须留痕，否则退化会静默发生。
    // 拿不到 Job 就**不解析**。z-code #56 R1-P2-1 建议「至少打日志」，这里
    // 修得更硬：`eprintln!` 在 GUI 进程里无人可见，而失去整树清杀意味着
    // 设计 §1.1 的「超时可杀」不再成立——对不受信文档降级运行，等于把遏制
    // 承诺悄悄变成尽力而为。宁可拒绝解析。
    #[cfg(windows)]
    let job = match job_for_child(&child, limits.max_process_bytes) {
        Ok(j) => j,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ParseFailure::ContainmentUnavailable(e));
        }
    };

    // stdin 必须**在独立线程里写**：worker 可能不读 stdin 就崩（abort 自测
    // 就是这种），此时 write 会因管道断裂阻塞或报错。放主线程会让超时形同虚设。
    if let Some(mut si) = child.stdin.take() {
        std::thread::spawn(move || {
            let _ = si.write_all(payload.as_bytes());
            // drop 关闭管道 → worker 的 read_to_string 才会返回
        });
    }

    // stdout/stderr 也必须**边等边读**：管道缓冲区满了 worker 就会阻塞在
    // write 上，看起来像挂死，实际是我们没读。这是这类外壳最常见的死锁。
    let out_cap = limits.max_output_bytes;
    // 超限标志必须**能中断等待**。第一版让读取线程超限后继续把管道读干
    // （怕不读会死锁），结果 worker 可以无限吐、父进程一直陪到墙钟，最后
    // 报的是 Timeout 而不是 OutputTooLarge——上限形同虚设。对抗测试
    // `output_flood_is_capped_not_deadlocked` 就是抓这个的：它同时断言
    // 失败**种类**和**用时上界**，只断言 is_err() 的话这条会显示通过。
    //
    // 正解：超限是**终止理由**，不是继续读的理由。读取线程置标志即返回，
    // 等待循环看到标志立刻整树清杀——既不死锁，也不陪跑。
    let over_cap = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut so = child.stdout.take();
    let flag = over_cap.clone();
    let out_handle = std::thread::spawn(move || read_capped(&mut so, out_cap, Some(flag)));
    let mut se = child.stderr.take();
    let err_handle = std::thread::spawn(move || read_capped(&mut se, 64 * 1024, None));

    enum Ended {
        Exited(std::process::ExitStatus),
        TimedOut,
        OutputOverCap,
    }

    let start = Instant::now();
    let ended = loop {
        if over_cap.load(std::sync::atomic::Ordering::Relaxed) {
            #[cfg(windows)]
            terminate_job(job);
            let _ = child.kill();
            let _ = child.wait();
            break Ended::OutputOverCap;
        }
        match child.try_wait() {
            Ok(Some(st)) => break Ended::Exited(st),
            Ok(None) => {}
            Err(e) => return Err(ParseFailure::SpawnFailed(e.to_string())),
        }
        if start.elapsed() >= limits.wall_clock {
            #[cfg(windows)]
            terminate_job(job);
            // 无论 Job 是否可用都补一刀，保证直接子进程必死。
            let _ = child.kill();
            let _ = child.wait();
            break Ended::TimedOut;
        }
        std::thread::sleep(Duration::from_millis(10));
    };

    // Job 句柄必须关：每次解析泄一个句柄是真泄漏。此时 worker 已终止，
    // 关闭触发 KILL_ON_JOB_CLOSE，顺带清掉可能残留的孙进程。
    #[cfg(windows)]
    close_job(job);

    let stdout = out_handle.join().unwrap_or(Err("stdout 读取线程 panic".into()));
    let stderr = err_handle
        .join()
        .unwrap_or(Err("stderr 读取线程 panic".into()))
        .unwrap_or_default();

    let status = match ended {
        Ended::OutputOverCap => return Err(ParseFailure::OutputTooLarge { cap: out_cap }),
        Ended::TimedOut => {
            return Err(ParseFailure::Timeout {
                after: limits.wall_clock,
            })
        }
        Ended::Exited(st) => st,
    };

    let stdout = match stdout {
        Ok(s) => s,
        // worker 已正常退出但输出仍超限（吐完就走，没触发上面的清杀分支）。
        Err(e) if e == "OVER_CAP" => return Err(ParseFailure::OutputTooLarge { cap: out_cap }),
        Err(e) => return Err(ParseFailure::BadOutput(e)),
    };

    if !status.success() {
        return Err(ParseFailure::Crashed {
            code: status.code(),
            stderr: stderr.chars().take(2000).collect(),
        });
    }

    match serde_json::from_str::<ParseResponse>(&stdout) {
        Ok(ParseResponse::Ok { text }) => Ok(text),
        Ok(ParseResponse::Err { reason }) => Err(ParseFailure::Rejected(reason)),
        Err(e) => Err(ParseFailure::BadOutput(format!(
            "{e}（前 200 字节：{}）",
            stdout.chars().take(200).collect::<String>()
        ))),
    }
}

/// 有上限地读一个管道。
///
/// 超限时：置 `over_flag`（若给了）并**立即停止读取**返回 `Err("OVER_CAP")`。
/// 停止读取本身会让 worker 阻塞在 write 上——这没关系，因为置了标志的那一刻
/// 等待循环就会把它整树清杀。**前提是调用方真的看这个标志**；不看就会退化成
/// 死锁。stderr 那一路不给标志（64KB 上限，且 stderr 只是诊断信息），
/// 它超限后照旧读干，不影响主判定。
fn read_capped<R: Read>(
    src: &mut Option<R>,
    cap: u64,
    over_flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<String, String> {
    let Some(r) = src.as_mut() else {
        return Ok(String::new());
    };
    let mut buf = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    let mut over = false;
    loop {
        match r.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if !over {
                    if buf.len() as u64 + n as u64 > cap {
                        over = true;
                        buf.clear();
                        buf.shrink_to_fit();
                        if let Some(f) = &over_flag {
                            f.store(true, std::sync::atomic::Ordering::Relaxed);
                            break; // 交给等待循环去杀，不再陪读
                        }
                    } else {
                        buf.extend_from_slice(&chunk[..n]);
                    }
                }
            }
            // 进程被我们杀掉时管道会断，这是预期路径，不当成读错误。
            Err(_) if over => break,
            Err(e) => return Err(e.to_string()),
        }
    }
    if over {
        return Err("OVER_CAP".into());
    }
    String::from_utf8(buf).map_err(|e| format!("输出非 UTF-8：{e}"))
}

#[cfg(windows)]
fn job_for_child(
    child: &std::process::Child,
    mem_cap: u64,
) -> Result<*mut core::ffi::c_void, String> {
    // 注入点：让「Job 不可用」这条分支可被证伪。没有它，
    // ContainmentUnavailable 就是一条永远跑不到、无人验证的死代码。
    if std::env::var("WANCODE_PARSE_WORKER_SELFTEST").as_deref() == Ok("nojob") {
        return Err("selftest 注入：CreateJobObject 不可用".into());
    }
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    };
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return Err("CreateJobObject 失败".into());
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        // 不加 BREAKAWAY_OK：解析 worker **必须**随应用一起死，没有任何
        // 正当理由脱离（与更新安装器相反，那个必须活过应用退出）。
        info.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        info.ProcessMemoryLimit = mem_cap as usize;
        if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) == 0
        {
            close_job(job);
            return Err("SetInformationJobObject 失败".into());
        }
        if AssignProcessToJobObject(job, child.as_raw_handle() as _) == 0 {
            close_job(job);
            return Err("AssignProcessToJobObject 失败（可能在禁嵌套 Job 的环境）".into());
        }
        Ok(job)
    }
}

#[cfg(windows)]
fn terminate_job(job: *mut core::ffi::c_void) {
    if job.is_null() {
        return;
    }
    use windows_sys::Win32::System::JobObjects::TerminateJobObject;
    // 1 = 退出码，仅用于区分「被我们杀的」。
    unsafe { TerminateJobObject(job, 1) };
}

#[cfg(windows)]
fn close_job(job: *mut core::ffi::c_void) {
    if job.is_null() {
        return;
    }
    use windows_sys::Win32::Foundation::CloseHandle;
    unsafe { CloseHandle(job) };
}
