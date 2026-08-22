//! v0.19-2c 回归探针：process-wide fan-out 不得污染本地扩展已禁用的
//! 活跃 Chat 会话。
//!
//! 原诊断已证明 fan-out 缺口；引擎现有会话级硬门，本测试翻转为严格
//! 零贡献，并保留 Code 插件正常工作的正对照。
//!
//! 结构性防假绿（复核九）：
//! - 单次 `spawn_grok_shell`：两次 spawn = 两套
//!   SharedPluginRegistryHandle，证不了同进程 fan-out；
//! - 时序：空来源起 Chat（`/plugins` 被 BuiltinGate 隐藏是预期态）→
//!   夹具落插件 → 起 Code 并**先证 Code 侧激活** → 负对照 → 广播；
//! - 隔离四件套：GROK_HOME + USERPROFILE/HOME + cwd + 插件安装目录。
//!
//! 证据纪律（复核十一）：
//! - 无固定 sleep：全部有界轮询，超时信息打印 ACP 通知/哨兵/MCP 计数；
//! - 负对照先证 Code reload 真的执行过（否则「Chat 不变」是假绿）；
//! - MCP 流量分阶段记基线，Chat 污染最终由**带 Chat 会话特征的哨兵**
//!   直接归因，不靠总连接数猜测。
//!
//! **为何是 lib 内单测而非集成测试**（复核十八实测订正）：本仓确实
//! 有 5 个集成测试且都在 CI 跑，所以「集成测试不可用」是错的。真实
//! 约束是 **libtest harness 与 panic 策略**：workspace 根
//! `[profile.dev] panic = "abort"`（grok-build，改它=动 vendor=破
//! G26），而带标准 harness 的集成测试需要 unwind。
//! 仓内唯一 import wancode_lib 的集成测试 `job_breakaway` 正是靠
//! `harness = false`（自写 main）绕开的，代价是没有 `#[ignore]`、
//! 没有名称过滤。本探针需要这两者（防误运行护栏的一半），故选
//! lib 内 `#[cfg(test)]` 模块 + `--ignored` 过滤：进程内只跑本探针，
//! `grok_home()` 的 OnceLock 隔离前提同样成立。
//!
//! 运行（专用命令，五要素缺一不可）。**PowerShell（本项目默认环境）**
//! ——try/finally 保证开关不残留、不污染后续命令：
//!
//! ```text
//! $env:WANCODE_PLUGIN_FANOUT_PROBE = "1"
//! try {
//!   cargo test --locked -p wancode --lib -- --ignored `
//!     plugin_fanout_cannot_pollute_extensions_disabled_chat `
//!     --nocapture --test-threads=1
//! } finally {
//!   Remove-Item Env:\WANCODE_PLUGIN_FANOUT_PROBE -ErrorAction SilentlyContinue
//! }
//! ```
//!
//! Bash 备用（前缀式赋值天然只作用于该条命令）：
//!
//! ```text
//! WANCODE_PLUGIN_FANOUT_PROBE=1 cargo test --locked -p wancode --lib -- \\
//!   --ignored plugin_fanout_cannot_pollute_extensions_disabled_chat \\
//!   --nocapture --test-threads=1
//! ```
//!
//! 缺环境开关即在动夹具前失败。保留 `#[ignore]` 以确保独立进程与
//! OnceLock 隔离，由 CI 专用步骤常驻执行。

#[cfg(test)]
mod probe {

    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// 有界轮询：每 50ms 检查一次，超时返回 false（绝不固定 sleep 判定）。
    /// **必须 async**（复核二十七实测）：此前用 `std::thread::sleep` 会把
    /// tokio 执行器线程整个占死——期间 mock 模型 server 无法应答、ACP
    /// 通知泵无法回 ACK、tokio 定时器不推进，等于「堵着门等门里的人出
    /// 来」。实测表现为整轮挂死约 1h40m，且此前几轮「普通 prompt 没打
    /// 模型」的结论也可能是被该阻塞掩盖的假象，需在修复后重新验证。
    /// 配套：测试用 multi_thread 运行时——引擎在同进程内需并发推进。
    async fn wait_until(timeout: Duration, mut cond: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if cond() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        cond()
    }

    /// 隔离夹具：GROK_HOME / HOME / USERPROFILE / cwd / 插件安装目录全在
    /// 临时目录内，绝不触碰真实用户插件状态。
    struct Fixture {
        root: PathBuf,
        _guard: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let guard = tempfile::tempdir().expect("tempdir");
            let root = guard.path().to_path_buf();
            for sub in [
                "grok-home",
                "home",
                "chat-cwd",
                "code-cwd",
                "plugin-src",
                "sentinels",
            ] {
                std::fs::create_dir_all(root.join(sub)).expect("mkdir");
            }
            // 隔离环境变量：必须在任何引擎调用之前设置——grok_home() 是
            // OnceLock，进程内只解析一次。
            // SAFETY: 测试进程启动早期、单线程。
            unsafe {
                std::env::set_var("GROK_HOME", root.join("grok-home"));
                std::env::set_var("HOME", root.join("home"));
                std::env::set_var("USERPROFILE", root.join("home"));
            }
            Self { root, _guard: guard }
        }

        fn path(&self, sub: &str) -> PathBuf {
            self.root.join(sub)
        }

        /// hook 哨兵文件：每次 hook 触发追加一行（带会话特征），用于按
        /// 会话归因污染，而不是只看总数。
        fn sentinel(&self, tag: &str) -> PathBuf {
            self.path("sentinels").join(format!("{tag}.log"))
        }

        /// 清空哨兵（复核十四）：进入真实链前必须抹掉夹具自测产生的
        /// JSONL，否则「模拟运行的记录」会混进引擎证据里冒充命中。
        fn reset_sentinels(&self) {
            let _ = std::fs::remove_file(self.sentinel("hook"));
        }

        /// 解析 JSONL 哨兵（复核十三：cwd 含空格/特殊字符也不会解析走样）。
        fn hook_records(&self) -> Vec<serde_json::Value> {
            self.sentinel_lines("hook")
                .iter()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect()
        }

        /// 某会话 ID 的 hook 命中数——污染主证据按 session ID 精确归因
        /// （复核十二），不靠总行数、不做子串匹配。
        fn hook_hits_for_session(&self, session_id: &str) -> usize {
            self.hook_records()
                .iter()
                .filter(|r| r.get("session").and_then(|v| v.as_str()) == Some(session_id))
                .count()
        }

        fn sentinel_lines(&self, tag: &str) -> Vec<String> {
            std::fs::read_to_string(self.sentinel(tag))
                .map(|s| s.lines().map(str::to_owned).collect())
                .unwrap_or_default()
        }

        /// 写隔离 config.toml：模型指向 mock 端点，**不含任何 [plugins] 段**
    /// ——插件来源必须在 Chat 起来之后才出现（复核十一时序纪律）。
    fn write_base_config(&self, model_port: u16) {
        let cfg = self.path("grok-home").join("config.toml");
        std::fs::write(
            &cfg,
            format!(
                "[models]\ndefault = \"probe\"\n\n\
                 [model.probe]\n\
                 name = \"Probe Mock\"\n\
                 model = \"probe-model\"\n\
                 base_url = \"http://127.0.0.1:{model_port}/v1\"\n\
                 api_key = \"probe-key\"\n\
                 api_backend = \"chat_completions\"\n"
            ),
        )
        .expect("write base config");
    }

    /// 落地测试插件：inline hooks（写哨兵）+ inline MCP（指向 mock）。
        /// 在 **Chat 已存活之后**调用——复核九时序修正的关键。
        fn write_plugin(&self, mcp_port: u16) {
            let dir = self.path("plugin-src").join("probe-plugin");
            std::fs::create_dir_all(&dir).expect("mkdir plugin");
            // hook 命令：写「session ID + workspace root」双身份字段。
            //
            // **不用 %CD%**（复核十二/十三）：Unix 侧固定 `sh -c`
            // （runner/command.rs:117）；Windows 侧走 `shell_command_argv`，
            // 可能选中 pwsh / PowerShell / Git Bash / cmd——即
            // **不能保证运行在 cmd 语义下**，`%CD%` 有很大概率被原样写入 =
            // 假证据。改为读 runner **在 spawn 时最后注入**的身份变量
            // （command.rs:172-179，注入顺序刻意排在 extra_env 之后，
            // 插件/用户 JSON 无法伪造）：GROK_SESSION_ID / GROK_WORKSPACE_ROOT。
            //
            // 加固（复核十三）：
            // - 显式 `powershell.exe -NoProfile -NonInteractive
            //   -ExecutionPolicy Bypass -File <script>`：不受用户执行策略
            //   与交互式 profile 影响，CI 可重复；
            // - 哨兵写 **JSONL**：cwd 里的空格/引号/反斜杠不会破坏解析，
            //   测试侧反序列化后按 session ID 精确归因。
            let sentinel = self.sentinel("hook");
            let script = self.path("plugin-src").join("probe-hook.ps1");
        // 硬化（复核三十一）：Add-Content 的非终止错误语义会让写失败被
        // 静默吞掉（实测 status=success 却无哨兵）。改为：
        //   - StrictMode + ErrorActionPreference=Stop 让任何问题终止；
        //   - [System.IO.File]::AppendAllText 直写绝对路径，绕开
        //     provider/相对路径解析；
        //   - 写后立即验证文件存在，异常/未生成 → exit 41（可被
        //     HookExecution.status=failed 捕获）；
        //   - 成功显式 exit 0。
        // 注：引擎构造 HookExecution 时 output 恒为 None
        // （hook_dispatch.rs:162），故错误只能靠 exit code 体现。
        let sentinel_str = sentinel.display().to_string().replace('\'', "''");
        std::fs::write(
            &script,
            format!(
                "$ErrorActionPreference = 'Stop'\n\
                 Set-StrictMode -Version Latest\n\
                 try {{\n\
                 $obj = [ordered]@{{ session = $env:GROK_SESSION_ID; \
                 cwd = $env:GROK_WORKSPACE_ROOT; event = $env:GROK_HOOK_EVENT; \
                 hook = $env:GROK_HOOK_NAME }}\n\
                 $line = ($obj | ConvertTo-Json -Compress) + [Environment]::NewLine\n\
                 $path = '{sentinel_str}'\n\
                 [System.IO.File]::AppendAllText($path, $line, \
                 [System.Text.UTF8Encoding]::new($false))\n\
                 if (-not (Test-Path -LiteralPath $path)) {{ exit 41 }}\n\
                 exit 0\n\
                 }} catch {{ exit 41 }}\n"
            ),
        )
        .expect("write hook script");
        let cmd = format!(
                "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File {}",
                script.display()
            );
            let manifest = serde_json::json!({
                "name": "probe-plugin",
                "version": "0.0.1",
                "description": "v0.19-2c fan-out 诊断探针插件",
                "hooks": {
                    "hooks": {
                        "UserPromptSubmit": [
                            // A clean Windows runner can spend more than five seconds on the
                            // first PowerShell start. This changes only the fixture budget.
                            { "hooks": [ { "type": "command", "command": cmd, "timeout": 20 } ] }
                        ]
                    }
                },
                "mcpServers": {
                    "probe-mcp": {
                        "type": "http",
                        "url": format!("http://127.0.0.1:{mcp_port}/mcp")
                    }
                }
            });
            std::fs::write(
                dir.join("plugin.json"),
                serde_json::to_string_pretty(&manifest).unwrap(),
            )
            .expect("write manifest");
            // 经 config [plugins].paths 让引擎发现它（有效配置层，与
            // resolve_effective_plugins_config 同源）。
            let cfg = self.path("grok-home").join("config.toml");
            let mut text = std::fs::read_to_string(&cfg).unwrap_or_default();
            text.push_str(&format!(
                "\n[plugins]\npaths = [\"{}\"]\n",
                dir.display().to_string().replace('\\', "/")
            ));
            std::fs::write(&cfg, text).expect("write config");
        }
    }

    /// Mock 模型端点（内容寻址，复核十九）：**绝不按请求下标判定归属**
/// ——标题生成、建议等辅助请求会抢序。每条请求整体留存，测试按
/// 「请求体是否含某个探针标记词」来识别是哪一轮主回合，另留 tools
/// 形状供 2c 后续的工具集断言复用。
#[derive(Clone, Default)]
struct ModelMock {
    requests: Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
}

/// 主回合的预期工具形状（判据③）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpectTools {
    /// Chat 主回合：函数工具集**精确等于** {web_search, web_fetch}。
    ChatExact,
    /// Code 主回合：工具声明非空且含编码工具判别项。
    CodeCoding,
}

impl ModelMock {
    /// 从一条请求里取「最后一条 user 消息」的纯文本（content 可能是
    /// 字符串，也可能是 [{type:text,text:..}] 分段数组，两种都归一）。
    fn last_user_text(req: &serde_json::Value) -> Option<String> {
        let msgs = req.get("messages")?.as_array()?;
        let m = msgs
            .iter()
            .rev()
            .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))?;
        let c = m.get("content")?;
        if let Some(t) = c.as_str() {
            return Some(t.to_string());
        }
        let parts = c.as_array()?;
        let mut out = String::new();
        for part in parts {
            if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                out.push_str(t);
            }
        }
        Some(out)
    }

    /// 请求里声明的函数工具名（扁平化，去掉命名空间前缀便于比较）。
    fn tool_names(req: &serde_json::Value) -> Vec<String> {
        req.get("tools")
            .and_then(|t| t.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| {
                        t.get("function")
                            .and_then(|f| f.get("name"))
                            .or_else(|| t.get("name"))
                            .and_then(|n| n.as_str())
                            .map(|n| n.rsplit(':').next().unwrap_or(n).to_string())
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn tools_match(req: &serde_json::Value, expect: ExpectTools) -> bool {
        let mut names = Self::tool_names(req);
        names.sort();
        names.dedup();
        match expect {
            ExpectTools::ChatExact => {
                names == vec!["web_fetch".to_string(), "web_search".to_string()]
            }
            ExpectTools::CodeCoding => {
                const CODING: &[&str] = &[
                    "run_terminal_cmd",
                    "read_file",
                    "write_file",
                    "search_replace",
                    "list_dir",
                    "grep",
                ];
                !names.is_empty() && names.iter().any(|n| CODING.contains(&n.as_str()))
            }
        }
    }

    /// 引擎发给模型的用户文本包装（实测）：`<user_query>\n{prompt}\n</user_query>`。
    /// 写进契约而非用 trim/substring 兜——包装格式一旦变化必须显式红，
    /// 逼迫重新审计请求形状（复核二十八 C 方案）。
    const USER_WRAP_OPEN: &'static str = "<user_query>\n";
    const USER_WRAP_CLOSE: &'static str = "\n</user_query>";

    /// 精确剥壳：要求**存在且仅一层**包装，剥完与原 prompt 字节级相等。
    /// 无包装 / 前后有额外文本 / 重复包装 一律判否。
    fn unwrap_user_query(text: &str) -> Option<&str> {
        let inner = text
            .strip_prefix(Self::USER_WRAP_OPEN)?
            .strip_suffix(Self::USER_WRAP_CLOSE)?;
        // 仅一层：内文不得再含包装标记（重复包装判否）。
        if inner.contains("<user_query>") || inner.contains("</user_query>") {
            return None;
        }
        Some(inner)
    }

    /// **主回合判定**（复核二十八：C 方案七条）。
    ///
    /// 顺序刻意如此：先按工具形状筛掉辅助请求（实测同一 prompt 会另发
    /// 一条 `tools=1` 的标题/摘要类请求），再对唯一候选做包装与内文的
    /// 字节级校验——既不退化成 substring，也不会被辅助请求冒名。
    ///
    /// 返回 Err(诊断串) 覆盖：候选数 != 1、缺包装、额外前后文本、
    /// 重复包装、内文不等。
    fn assert_single_main_turn(
        &self,
        prompt: &str,
        expect: ExpectTools,
    ) -> Result<(), String> {
        let reqs = self.requests.lock().unwrap();
        let candidates: Vec<&serde_json::Value> = reqs
            .iter()
            .filter(|r| Self::tools_match(r, expect))
            .collect();
        if candidates.len() != 1 {
            return Err(format!(
                "工具形状候选请求应恰好 1 条，实得 {}（expect={expect:?}）",
                candidates.len()
            ));
        }
        let text = Self::last_user_text(candidates[0])
            .ok_or_else(|| "候选请求无 role==user 消息".to_string())?;
        let inner = Self::unwrap_user_query(&text).ok_or_else(|| {
            format!("用户消息不是恰好一层 <user_query> 包装：{text:?}")
        })?;
        if inner != prompt {
            return Err(format!("剥壳后内文与 prompt 不等：inner={inner:?} prompt={prompt:?}"));
        }
        Ok(())
    }

    /// 请求摘要（诊断用）：每条取「工具数 + 最后一条 user 文本前 40 字」。
    fn summary(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| {
                let text = Self::last_user_text(r).unwrap_or_default();
                let head: String = text.chars().take(40).collect();
                format!("[tools={} user={head:?}]", Self::tool_names(r).len())
            })
            .collect()
    }

    fn total(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    /// 最近一条主回合请求的工具名（供工具集精确断言复用）。
    #[allow(dead_code)]
    fn tools_of_main_turn(&self, prompt: &str, expect: ExpectTools) -> Option<Vec<String>> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|r| {
                Self::last_user_text(r).as_deref() == Some(prompt)
                    && Self::tools_match(r, expect)
            })
            .map(Self::tool_names)
    }
}
/// 起 mock 模型端点：OpenAI 兼容 `/v1/chat/completions`，整条请求体
/// 留存供结构化寻址；stream 与非 stream 两种响应都给（引擎按 backend
/// 选择，不能只支持一种）。
/// 独立审计（复核三十）：直接读该会话持久化的 `updates.jsonl`，找
/// `hook_execution` 行。`send_xai_notification` 是**先写持久化、再发 ACP
/// 通知**（updates.rs:724 一带），所以 JSONL 与泵的四种组合可判别：
///   JSONL 有 / 泵无        → 泵解析错误；
///   两处都有 Failed        → runner/PowerShell 问题；
///   两处都无（注册态与主回合成立）→ 引擎 dispatch 路径缺口；
///   JSONL Success / 哨兵无 → 脚本落盘问题。
fn jsonl_hook_lines(cwd: &std::path::Path, session_id: &str) -> Vec<String> {
    let dir = xai_grok_shell::util::grok_home::sessions_cwd_dir(&cwd.to_string_lossy())
        .join(session_id);
    let path = dir.join("updates.jsonl");
    std::fs::read_to_string(&path)
        .map(|t| {
            t.lines()
                .filter(|l| l.contains("hook_execution") || l.contains("hook"))
                .map(|l| l.chars().take(200).collect::<String>())
                .collect()
        })
        .unwrap_or_else(|e| vec![format!("<读 {} 失败: {e}>", path.display())])
}

async fn serve_model_mock(mock: ModelMock) -> u16 {
    use axum::{extract::State, response::Response, routing::post, Router};
    async fn handler(
        State(mock): State<ModelMock>,
        body: axum::extract::Json<serde_json::Value>,
    ) -> Response {
        let req = body.0;
        let streaming = req.get("stream").and_then(|v| v.as_bool()).unwrap_or(false);
        mock.requests.lock().unwrap().push(req);
        if streaming {
            let sse = concat!(
                "data: {\"id\":\"p\",\"object\":\"chat.completion.chunk\",\"created\":0,",
                "\"model\":\"probe-model\",\"choices\":[{\"index\":0,\"delta\":",
                "{\"role\":\"assistant\",\"content\":\"ok\"},\"finish_reason\":null}]}\n\n",
                "data: {\"id\":\"p\",\"object\":\"chat.completion.chunk\",\"created\":0,",
                "\"model\":\"probe-model\",\"choices\":[{\"index\":0,\"delta\":{},",
                "\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"
            );
            Response::builder()
                .header("content-type", "text/event-stream")
                .body(axum::body::Body::from(sse))
                .unwrap()
        } else {
            let json = serde_json::json!({
                "id": "p", "object": "chat.completion", "created": 0,
                "model": "probe-model",
                "choices": [{ "index": 0, "finish_reason": "stop",
                    "message": { "role": "assistant", "content": "ok" } }],
            });
            Response::builder()
                .header("content-type", "application/json")
                .body(axum::body::Body::from(json.to_string()))
                .unwrap()
        }
    }
    let app = Router::new()
        .route("/v1/chat/completions", post(handler))
        .with_state(mock);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    port
}

/// 阶段基线快照（复核十四）：每阶段独立记 hook/MCP 计数，断言只看
    /// **相对上一阶段的增量**，杜绝跨阶段串味与「总数够了就算」。
    #[derive(Debug, Clone, Copy)]
    struct StageBaseline {
        chat_hits: usize,
        code_hits: usize,
        mcp_hits: usize,
    }

    impl StageBaseline {
        fn take(fx: &Fixture, mock: &McpMock, chat_sid: &str, code_sid: &str) -> Self {
            Self {
                chat_hits: fx.hook_hits_for_session(chat_sid),
                code_hits: fx.hook_hits_for_session(code_sid),
                mcp_hits: mock.hits(),
            }
        }
    }

    /// MCP mock：记录每次连接/请求，分阶段取基线用。
    #[derive(Clone, Default)]
    struct McpMock {
        hits: Arc<AtomicUsize>,
    }

    impl McpMock {
        fn hits(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "独立进程回归门：由 CI 专用命令设置环境开关运行"]
    async fn plugin_fanout_cannot_pollute_extensions_disabled_chat() {
        // 防误运行护栏（复核十五）：名称过滤只是操作约定——将来有人跑
        // 全量 `--ignored` 会把本探针和其他 ignored 测试一起拉起，
        // 静默改写环境变量（GROK_HOME/HOME/USERPROFILE）并起真引擎。
        // 显式开关只由专用命令设置，别的路径一律在动任何夹具之前失败。
        assert_eq!(
            std::env::var("WANCODE_PLUGIN_FANOUT_PROBE").as_deref(),
            Ok("1"),
            "诊断探针只能用专用命令独立运行（见文件头运行说明）"
        );

        let fx = Fixture::new();

        // ── 阶段 0：mock MCP server（记录命中）───────────────────────
        let mock = McpMock::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mcp mock");
        let mcp_port = listener.local_addr().unwrap().port();
        {
            let hits = mock.hits.clone();
            tokio::spawn(async move {
                loop {
                    match listener.accept().await {
                        Ok(_) => {
                            hits.fetch_add(1, Ordering::SeqCst);
                        }
                        Err(_) => break,
                    }
                }
            });
        }

        // ── 阶段 1：起 mock + 落真实插件来源 ──────────────────────
        let model_mock = ModelMock::default();
        let model_port = serve_model_mock(model_mock.clone()).await;
        fx.write_base_config(model_port);
        fx.write_plugin(mcp_port);
        fx.reset_sentinels(); // 清掉夹具自测记录，再取阶段基线

        // 旧 preflight 只作诊断；必须看见来源，防零贡献因无插件而假绿。
        assert!(
            crate::surface_policy::enforce_chat_plugin_preflight(&fx.path("chat-cwd")).is_err(),
            "生产 preflight 没看见探针插件来源"
        );

        let mut stages: Vec<serde_json::Value> = Vec::new();
        let base = StageBaseline::take(&fx, &mock, "", "");
        stages.push(serde_json::json!({
            "stage": "baseline_after_reset",
            "chat_hits": base.chat_hits,
            "code_hits": base.code_hits,
            "mcp_hits": base.mcp_hits,
        }));

        // ── 阶段 2：单次 spawn + initialize/authenticate + Chat 会话 ──
        // **单次 spawn_grok_shell**（复核九）：两次 = 两套
        // SharedPluginRegistryHandle，证不了同进程 fan-out。
        let chat_cwd = fx.path("chat-cwd");
        let raw_config = xai_grok_shell::config::load_effective_config()
            .expect("加载隔离配置失败");
        let mut agent_config =
            xai_grok_shell::agent::config::Config::new_from_toml_cfg(&raw_config)
                .expect("解析隔离配置失败");
        agent_config.resolve_runtime_fields(
            &xai_grok_shell::agent::config::RuntimeResolutionContext {
                raw_config: &raw_config,
                remote_settings: None,
                cwd: Some(&chat_cwd),
                is_headless: true,
                cli_subagents: None,
                cli_web_search_model: None,
                cli_session_summary_model: None,
                cli_experimental_memory: false,
                cli_no_memory: false,
                disable_web_search: false,
                todo_gate: false,
                laziness_debug_log: None,
                storage_mode: None,
            },
        );
        // 全局 reload 从 AgentConfig 重建 registry；显式指向同一真实夹具，
        // 避免会话 list 看得到、广播源却因启动快照为空而重建成空。
        agent_config.plugins.cli_plugin_dirs =
            vec![fx.path("plugin-src").join("probe-plugin")];
        agent_config.mode = xai_grok_shell::agent::config::AgentMode::Headless;
        agent_config.default_yolo_mode = false;
        let memory_config = agent_config.memory_config.clone();
        let cancel = tokio_util::sync::CancellationToken::new();
        let spawned = xai_grok_pager::acp::spawn::spawn_grok_shell(
            agent_config,
            &cancel,
            memory_config,
        )
        .await
        .expect("spawn_grok_shell 失败");
        let acp_tx = spawned.channel.tx;
        let mut acp_rx = spawned.channel.rx;
        // 通知泵：引擎的 SessionNotification 必须有人消费，否则 resume/
        // 命令列表等路径会阻塞。全部留存供断言（available commands 等）。
        // 通知泵（复核二十二 P0-1）：**必须 ACK**——引擎的 update 发送
        // 在等 response_tx，不回就会卡住后续命令列表 / reload / 会话流程。
        // 结构化分派：可用命令更新单独收集（正事件断言用），文件/终端
        // 类请求记为 unexpected 让测试可见地失败（Chat 层本不该出现）。
        #[derive(Default)]
        struct Pumped {
            /// (session_id, 命令名列表)
            commands: Vec<(String, Vec<String>)>,
            /// x.ai 扩展通知原文（HooksChanged / HookExecution 等）——
            /// hook「派发态 + 执行结果」两层证据来源（复核二十九）。
            xai_notes: Vec<String>,
            /// 所有到达的 update 变体名 (session_id, variant)——诊断用：
            /// 「没等到 X」必须能区分「什么都没来」与「来的是别的」。
            all_updates: Vec<(String, String)>,
            /// 不该出现的客户端请求（文件读写、终端）——Chat 零文件面的
            /// 直接反证；记录后测试断言为空。
            unexpected: Vec<String>,
        }
        impl Pumped {
            /// **唯一允许的读取方式**（复核二十八收口）：单次 lock 后
            /// clone 出所需字段，失败信息只读这份快照。
            ///
            /// 背景：此前四处 assert! 的格式参数里连续两次
            /// `pumped.lock()`——std::sync::Mutex 不可重入，两个临时
            /// guard 在整条表达式结束前都活着，第二次 lock 自我死锁。
            /// 断言通过时格式参数不求值，故只在**失败瞬间**暴露；实测
            /// 挂死 7.5 小时、CPU 静止。规则：任何失败信息一律先
            /// `snapshot()`，禁止在同一表达式里出现两次 `.lock()`。
            fn snapshot(m: &std::sync::Mutex<Self>) -> String {
                let g = m.lock().unwrap();
                format!(
                    "all_updates={:?} unexpected={:?} commands={:?} xai_notes={:?}",
                    g.all_updates, g.unexpected, g.commands, g.xai_notes
                )
            }
        }
        let pumped: Arc<std::sync::Mutex<Pumped>> = Arc::new(std::sync::Mutex::new(Pumped::default()));
        {
            let pumped = pumped.clone();
            tokio::spawn(async move {
                use xai_acp_lib::AcpClientMessage as M;
                while let Some(msg) = acp_rx.recv().await {
                    match msg {
                        M::SessionNotification(a) => {
                            let sid = a.request.session_id.0.to_string();
                            {
                                let variant = format!("{:?}", a.request.update);
                                let head: String =
                                    variant.chars().take(40).collect();
                                pumped
                                    .lock()
                                    .unwrap()
                                    .all_updates
                                    .push((sid.clone(), head));
                            }
                            if let agent_client_protocol::SessionUpdate::AvailableCommandsUpdate(
                                u,
                            ) = &a.request.update
                            {
                                let names: Vec<String> = u
                                    .available_commands
                                    .iter()
                                    .map(|c| c.name.clone())
                                    .collect();
                                pumped.lock().unwrap().commands.push((sid, names));
                            }
                            let _ = a.response_tx.send(Ok(()));
                        }
                        M::ExtNotification(a) => {
                            // **结构化解析**（复核三十）：不再用 Debug +
                            // contains("hook") 猜——method 必须精确等于
                            // x.ai/session_notification，params 反序列化后
                            // 按 update 判别 HooksChanged / HookExecution，
                            // 逐条留存 session/event/promptId 与每个 run 的
                            // Success/Failed/Skipped。
                            if a.request.method.as_ref() == "x.ai/session_notification"
                                && serde_json::from_str::<serde_json::Value>(
                                    a.request.params.get(),
                                )
                                .is_ok()
                            {
                                let v = serde_json::from_str::<serde_json::Value>(
                                    a.request.params.get(),
                                )
                                .expect("已验证可解析");
                                let sid = v
                                    .get("sessionId")
                                    .and_then(|x| x.as_str())
                                    .unwrap_or("?")
                                    .to_string();
                                let upd = v.get("update");
                                let kind = upd
                                    .and_then(|u| u.get("sessionUpdate"))
                                    .and_then(|x| x.as_str())
                                    .unwrap_or("");
                                if kind == "hook_execution" || kind == "hooks_changed" {
                                    let runs: Vec<String> = upd
                                        .and_then(|u| u.get("runs"))
                                        .and_then(|r| r.as_array())
                                        .map(|arr| {
                                            arr.iter()
                                                .map(|r| {
                                                    // HookRunStatusDto 用 tag="status"
                                                    // 内部标签（notification.rs:342），
                                                    // status/error/elapsedMs 扁平在 run 对象里。
                                                    // 直接留存整条原文，不再逐字段猜。
                                                    r.to_string().chars().take(300)
                                                        .collect::<String>()
                                                })
                                                .collect()
                                        })
                                        .unwrap_or_default();
                                    pumped.lock().unwrap().xai_notes.push(format!(
                                        "kind={kind} sid={sid} event={:?} promptId={:?} runs={runs:?}",
                                        upd.and_then(|u| u.get("eventName"))
                                            .and_then(|x| x.as_str()),
                                        upd.and_then(|u| u.get("promptId"))
                                            .and_then(|x| x.as_str()),
                                    ));
                                }
                            }
                            let _ = a.response_tx.send(Ok(()));
                        }
                        M::RequestPermission(a) => {
                            // 明确取消：探针不批准任何权限请求。
                            pumped
                                .lock()
                                .unwrap()
                                .unexpected
                                .push("RequestPermission".into());
                            let _ = a.response_tx.send(Ok(
                                agent_client_protocol::RequestPermissionResponse::new(
                                    agent_client_protocol::RequestPermissionOutcome::Cancelled,
                                ),
                            ));
                        }
                        M::ReadTextFile(a) => {
                            pumped.lock().unwrap().unexpected.push("ReadTextFile".into());
                            let _ = a.response_tx.send(Err(
                                agent_client_protocol::Error::method_not_found(),
                            ));
                        }
                        M::WriteTextFile(a) => {
                            pumped.lock().unwrap().unexpected.push("WriteTextFile".into());
                            let _ = a.response_tx.send(Err(
                                agent_client_protocol::Error::method_not_found(),
                            ));
                        }
                        M::CreateTerminal(a) => {
                            pumped.lock().unwrap().unexpected.push("CreateTerminal".into());
                            let _ = a.response_tx.send(Err(
                                agent_client_protocol::Error::method_not_found(),
                            ));
                        }
                        M::TerminalOutput(a) => {
                            pumped.lock().unwrap().unexpected.push("TerminalOutput".into());
                            let _ = a.response_tx.send(Err(
                                agent_client_protocol::Error::method_not_found(),
                            ));
                        }
                        M::ReleaseTerminal(a) => {
                            pumped.lock().unwrap().unexpected.push("ReleaseTerminal".into());
                            let _ = a.response_tx.send(Err(
                                agent_client_protocol::Error::method_not_found(),
                            ));
                        }
                        M::WaitForTerminalExit(a) => {
                            pumped.lock().unwrap().unexpected.push("WaitForTerminalExit".into());
                            let _ = a.response_tx.send(Err(
                                agent_client_protocol::Error::method_not_found(),
                            ));
                        }
                        M::KillTerminalCommand(a) => {
                            pumped.lock().unwrap().unexpected.push("KillTerminalCommand".into());
                            let _ = a.response_tx.send(Err(
                                agent_client_protocol::Error::method_not_found(),
                            ));
                        }
                        M::ExtMethod(a) => {
                            let _ = a.response_tx.send(Err(
                                agent_client_protocol::Error::method_not_found(),
                            ));
                        }
                    }
                }
            });
        }

        let caps = agent_client_protocol::ClientCapabilities::new()
            .fs(agent_client_protocol::FileSystemCapabilities::new())
            .terminal(false);
        let init_resp: agent_client_protocol::InitializeResponse = xai_acp_lib::acp_send(
            agent_client_protocol::InitializeRequest::new(
                agent_client_protocol::ProtocolVersion::V1,
            )
            .client_capabilities(caps)
            .meta(
                serde_json::json!({
                    "clientType": "wancode-probe",
                    "startupHints": { "nonInteractive": true },
                })
                .as_object()
                .cloned(),
            ),
            &acp_tx,
        )
        .await
        .expect("ACP initialize 失败");
        if let Some(m) = init_resp.auth_methods.first() {
            let _: agent_client_protocol::AuthenticateResponse = xai_acp_lib::acp_send(
                agent_client_protocol::AuthenticateRequest::new(m.id().clone()),
                &acp_tx,
            )
            .await
            .expect("ACP authenticate 失败");
        }

        // **真实 Chat 会话**（复核二十二 P0-2）：不注入 agentProfile 的
        // 话，这只是「私有 cwd 下的默认 Code 会话」，不能作为 Chat 生命
        // 周期探针。注入生产同款 profile 与 startup hints。
        let chat_resp: agent_client_protocol::NewSessionResponse = xai_acp_lib::acp_send(
            agent_client_protocol::NewSessionRequest::new(chat_cwd.clone()).meta(
                serde_json::json!({
                    "agentProfile": crate::surface_policy::chat_agent_profile(),
                    "startupHints": crate::surface_policy::chat_startup_hints(),
                    "x.ai/localExtensionsDisabled": true,
                })
                .as_object()
                .cloned()
                .unwrap(),
            ),
            &acp_tx,
        )
        .await
        .expect("创建 Chat 会话失败");
        let chat_sid = chat_resp.session_id.0.to_string();
        assert_eq!(
            chat_resp
                .meta
                .as_ref()
                .and_then(|m| m.get("localExtensionsDisabledApplied"))
                .and_then(|v| v.as_bool()),
            Some(true),
            "引擎未回显 localExtensionsDisabledApplied=true"
        );

        // ── chat_started_clean 的六条断言（复核二十一）──────────────
        // 全部通过之后才追加阶段记录——阶段名本身就是「该段证据已完成」
        // 的凭据，不是控制流到过的标记。
        assert!(!chat_sid.is_empty(), "Chat session ID 为空");
        assert!(
            fx.path("plugin-src").join("probe-plugin").join("plugin.json").is_file(),
            "预置插件夹具不存在，fan-out 回归测试失去正向对照"
        );
        assert!(
            crate::surface_policy::enforce_chat_plugin_preflight(&chat_cwd).is_err(),
            "诊断 preflight 应看见插件来源；真正隔离必须由引擎策略位完成"
        );
        assert_eq!(fx.hook_hits_for_session(&chat_sid), 0, "Chat 起始 hook 命中非 0");
        assert_eq!(mock.hits(), base.mcp_hits, "Chat 起始 MCP 命中非基线");
        // Chat cwd 必须是隔离私有目录（不是宿主/项目目录）。
        // cwd 精确判据（复核二十三）：不看 meta 回显（缺字段就退化成
        // 自比恒真），而看**引擎按 cwd 实际落盘的会话目录**——
        // $GROK_HOME/sessions/<encode_cwd_dirname(cwd)>/<session_id>/
        // 存在，才证明这个会话真的 root 在预定私有目录上。
        let expect_dir = xai_grok_shell::util::grok_home::sessions_cwd_dir(
            &chat_cwd.to_string_lossy(),
        )
        .join(&chat_sid);
        assert!(
            wait_until(Duration::from_secs(10), || expect_dir.is_dir()).await,
            "Chat 会话未落在预定私有 cwd 目录：期望 {}",
            expect_dir.display()
        );
        // 初始 registry 为空（复核二十四定案：**x.ai/plugins/list 正证据**）。
        //
        // 为什么不用 AvailableCommandsUpdate 作门槛：实测该通知在本场景
        // 20s 内根本没到（all_updates=[]），而「命令集为空被静默吞掉」
        // 的假设不成立——session-info 等 AlwaysOn 命令保证列表非空，
        // 故缺通知是独立的通知行为问题，不该拿来当 registry 判据。
        // 改用 x.ai/plugins/list：对**已知 session** 它查该会话自己的
        // registry（不回退共享快照，extensions/plugins.rs:135），且经
        // session actor 的 plugins_list() 返回——顺带证明 actor 正常
        // 处理请求。model_requests=0 在未发 prompt 前属正常，不作判据。
        let list_raw = serde_json::value::to_raw_value(&serde_json::json!({
            "sessionId": chat_sid,
        }))
        .expect("static json");
        let list_resp = tokio::time::timeout(
            Duration::from_secs(20),
            xai_acp_lib::acp_send(
                agent_client_protocol::ExtRequest::new(
                    "x.ai/plugins/list".to_string(),
                    list_raw.into(),
                ),
                &acp_tx,
            ),
        )
        .await;
        let list_resp: agent_client_protocol::ExtResponse = match list_resp {
            Err(_) => {
                let dbg = Pumped::snapshot(&pumped);
                panic!(
                    "x.ai/plugins/list 超时：session actor / 初始化链有问题。{dbg} model_requests={}",
                    model_mock.total()
                )
            }
            Ok(r) => r.expect("x.ai/plugins/list 调用失败"),
        };
        let list_json: serde_json::Value =
            serde_json::from_str(list_resp.0.get()).expect("plugins/list 响应非 JSON");
        // ext 响应带 {result:...} 信封（实测），兼容裸形状。
        let plugins = list_json
            .get("result")
            .unwrap_or(&list_json)
            .get("plugins")
            .and_then(|v| v.as_array())
            .unwrap_or_else(|| panic!("plugins/list 响应缺 plugins 数组：{list_json}"));
        assert!(
            plugins.is_empty(),
            "Chat 初始 registry 非空（隔离失败）：{plugins:?}"
        );
        {
            let snapshot = pumped.lock().unwrap();
            assert!(
                snapshot.unexpected.is_empty(),
                "Chat 启动期出现不该有的客户端请求：{:?}",
                snapshot.unexpected
            );
        }
        stages.push(serde_json::json!({
            "stage": "chat_started_clean",
            "chat_session": chat_sid,
            "chat_cwd": chat_cwd.display().to_string(),
            "hook_hits": 0,
            "mcp_hits": mock.hits(),
            "plugins_list": "empty",
            // 诊断字段（不参与判定）：该通知在本场景未到达，见复核二十四。
            "available_commands_seen": pumped.lock().unwrap().commands.len(),
        }));

        // ── 夹具自测（复核三十一）：**必须与 runner 同语义** ────────
        // 直接起 powershell.exe 证明不了 runner 路径可行——runner 走
        // shell_command_argv（可能选中 pwsh/PowerShell/Git Bash/cmd），
        // 且在 workspace_root 下注入五个身份变量。自测用同一套包装、
        // 同 cwd、同变量；通过后清空哨兵再进真实链。
        {
            let script_path = fx.path("plugin-src").join("probe-hook.ps1");
            let cmd = format!(
                "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File {}",
                script_path.display()
            );
            // G26 记录：runner 用 xai_grok_config::shell::shell_command_argv
            // 选 shell，但该 crate 不是 wancode 的依赖（加依赖=改
            // Cargo.lock=破 G26 清单哈希）。自测退一步用 cmd.exe /C 执行
            // 同一条 command 串——**这是已知偏差**：它验证的是「命令串本身
            // 可执行且能写出哨兵」，不验证 shell 选择逻辑。shell 选择的正确
            // 性由真实链的 HookExecution.status 兜底（选错 shell 会失败）。
            let mut c = std::process::Command::new("cmd.exe");
            c.arg("/C").arg(&cmd)
                .current_dir(fx.path("code-cwd"))
                .env("GROK_HOOK_EVENT", "user_prompt_submit")
                .env("GROK_HOOK_NAME", "selftest")
                .env("GROK_SESSION_ID", "selftest-session")
                .env("GROK_WORKSPACE_ROOT", fx.path("code-cwd"))
                .env("CLAUDE_PROJECT_DIR", fx.path("code-cwd"));
            let out = c.output().expect("夹具自测启动失败");
            assert!(
                out.status.success(),
                "夹具自测失败：exit={:?} stdout={} stderr={}",
                out.status.code(),
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            let recs = fx.hook_records();
            assert!(
                recs.iter().any(|r| r.get("session").and_then(|v| v.as_str())
                    == Some("selftest-session")),
                "夹具自测未产生哨兵记录（脚本本身写不出文件）：{recs:?}"
            );
            fx.reset_sentinels(); // 自测记录绝不混入引擎证据
        }

        let after_gate =
            crate::surface_policy::enforce_chat_plugin_preflight(&chat_cwd);
        assert!(
            after_gate.is_err(),
            "落插件后生产 preflight 仍放行：门看不见这批来源，后续污染判定失去意义"
        );
        // 引擎自己的有效配置解析必须也看得见（辅助证据；主证据仍是
        // Code 会话真的激活了 hook/MCP）。
        let eff_now =
            xai_grok_shell::config::resolve_effective_plugins_config(&fx.path("code-cwd"));
        assert!(
            !eff_now.paths.is_empty(),
            "引擎有效配置未看到后写入的 [plugins].paths：{eff_now:?}"
        );

        // ── 阶段 4：同一连接建 Code 会话，先证插件真的激活 ─────────
        let code_cwd = fx.path("code-cwd");
        let code_resp: agent_client_protocol::NewSessionResponse = xai_acp_lib::acp_send(
            agent_client_protocol::NewSessionRequest::new(code_cwd.clone()),
            &acp_tx,
        )
        .await
        .expect("创建 Code 会话失败");
        let code_sid = code_resp.session_id.0.to_string();
        assert_ne!(code_sid, chat_sid, "Code 与 Chat 会话 ID 相同，拓扑错误");

        // Code registry 必须看到插件（证明会话启动时重读了磁盘配置，
        // 用的不是 AgentConfig 创建时的启动快照）。
        let code_list_raw = serde_json::value::to_raw_value(&serde_json::json!({
            "sessionId": code_sid,
        }))
        .expect("static json");
        let code_list: agent_client_protocol::ExtResponse = tokio::time::timeout(
            Duration::from_secs(20),
            xai_acp_lib::acp_send(
                agent_client_protocol::ExtRequest::new(
                    "x.ai/plugins/list".to_string(),
                    code_list_raw.into(),
                ),
                &acp_tx,
            ),
        )
        .await
        .expect("Code plugins/list 超时")
        .expect("Code plugins/list 调用失败");
        let code_json: serde_json::Value =
            serde_json::from_str(code_list.0.get()).expect("非 JSON");
        let code_plugins = code_json
            .get("result")
            .unwrap_or(&code_json)
            .get("plugins")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        assert!(
            !code_plugins.is_empty(),
            "Code registry 未加载探针插件（会话启动未重读磁盘配置？）：{code_json}"
        );

        // ── 由 Code 会话触发真正的全局 reload/broadcast ───────────
        let pre_reload = StageBaseline::take(&fx, &mock, &chat_sid, &code_sid);
        // send_prompt（复核二十六）：**不再吞错**——返回结构化结果，
        // 外层有界超时。此前 `let _ = ... as Result<_,_>` 同时吞掉了
        // ACP 错误、提前 EndTurn 与取消结果，导致「没打模型」无法归因。
        let send_prompt = |sid: agent_client_protocol::SessionId, text: &str| {
            let tx = acp_tx.clone();
            let text = text.to_string();
            async move {
                match tokio::time::timeout(
                    Duration::from_secs(120),
                    xai_acp_lib::acp_send(
                        agent_client_protocol::PromptRequest::new(
                            sid,
                            vec![agent_client_protocol::ContentBlock::from(text.as_str())],
                        ),
                        &tx,
                    ),
                )
                .await
                {
                    Err(_) => Err("prompt 超时（120s）".to_string()),
                    Ok(Err(e)) => Err(format!("ACP 错误：{e:?}")),
                    Ok(Ok(r)) => Ok(r),
                }
            }
        };

        let action_raw = serde_json::value::to_raw_value(&serde_json::json!({
            "sessionId": code_sid,
            "action": { "type": "reload" },
        }))
        .expect("static json");
        let action_resp: agent_client_protocol::ExtResponse = tokio::time::timeout(
            Duration::from_secs(60),
            xai_acp_lib::acp_send(
                agent_client_protocol::ExtRequest::new(
                    "x.ai/plugins/action".to_string(),
                    action_raw.into(),
                ),
                &acp_tx,
            ),
        )
        .await
        .expect("x.ai/plugins/action 超时")
        .expect("x.ai/plugins/action 调用失败");
        let action_dbg = action_resp.0.get().chars().take(300).collect::<String>();

        // ── hook 注册态实证（复核二十九）：x.ai/hooks/list ──────────
        // 直接读该 session 的 hook_registry（run_loop.rs:354），不再靠
        // PluginInfo.hookCount 推断运行态——后者只证 manifest 有效。
        let hooks_raw = serde_json::value::to_raw_value(&serde_json::json!({
            "sessionId": code_sid,
        }))
        .expect("static json");
        let hooks_resp: agent_client_protocol::ExtResponse = tokio::time::timeout(
            Duration::from_secs(20),
            xai_acp_lib::acp_send(
                agent_client_protocol::ExtRequest::new(
                    "x.ai/hooks/list".to_string(),
                    hooks_raw.into(),
                ),
                &acp_tx,
            ),
        )
        .await
        .expect("x.ai/hooks/list 超时")
        .expect("x.ai/hooks/list 调用失败");
        let hooks_json: serde_json::Value =
            serde_json::from_str(hooks_resp.0.get()).expect("hooks/list 非 JSON");
        let hooks_body = hooks_json.get("result").unwrap_or(&hooks_json);
        let hooks_list = hooks_body
            .get("hooks")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let load_errors = hooks_body
            .get("loadErrors")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let script_path = fx.path("plugin-src").join("probe-hook.ps1");
        let target: Option<&serde_json::Value> = hooks_list.iter().find(|h| {
            // **只认精确值**（复核三十）：内部解析与派发共用同一个
            // HookEventName::UserPromptSubmit 枚举，`user_prompt_submit`
            // 只是 x.ai/hooks/list 的固定 wire 表示——不存在命名空间
            // 不一致，故不接受两种写法，wire 形状变化必须显式红。
            h.get("event").and_then(|v| v.as_str()) == Some("user_prompt_submit")
        });
        assert!(
            !hooks_list.is_empty()
                && load_errors.is_empty()
                && target.is_some_and(|h| {
                    h.get("handlerType").and_then(|v| v.as_str()) == Some("command")
                        && h.get("command").and_then(|v| v.as_str()).is_some_and(|c| {
                            c.contains(&script_path.display().to_string())
                        })
                        && h.get("disabled").and_then(|v| v.as_bool()) == Some(false)
                }),
            "hook 注册态不合预期（问题在 reload 的解析/安装阶段）：\
             hooks={hooks_body} script={} pump={}",
            script_path.display(),
            Pumped::snapshot(&pumped)
        );

        // ── 普通 Code 回合：**先证进了 turn，再谈 hook** ─────────────
        // 判定分层（复核二十六）：
        //   Err            → 修 ACP/会话错误；
        //   Ok 但主回合未增 → 命中模型阻塞等提前返回路径，先修那条路；
        //   主回合增 1 但 hook 不触发 → 才去查 hook registry/runner。
        const CODE_PROMPT: &str = "probe-code-turn-1";
        let code_prompt_resp = send_prompt(code_resp.session_id.clone(), CODE_PROMPT).await;
        let code_prompt_dbg = format!("{code_prompt_resp:?}");
        let pump_dbg2 = Pumped::snapshot(&pumped);
        assert!(
            code_prompt_resp.is_ok(),
            "Code prompt 未成功：{code_prompt_dbg} {pump_dbg2} model_summary={:?}",
            model_mock.summary()
        );
        let pump_dbg3 = Pumped::snapshot(&pumped);

        // 主回合前置门（复核二十八 C）：先等候选出现，再做七条校验。
        let turn_seen = wait_until(Duration::from_secs(60), || {
            model_mock
                .assert_single_main_turn(CODE_PROMPT, ExpectTools::CodeCoding)
                .is_ok()
        })
        .await;
        let turn_verdict = model_mock.assert_single_main_turn(CODE_PROMPT, ExpectTools::CodeCoding);
        assert!(
            turn_seen && turn_verdict.is_ok(),
            "Code 普通回合未成立：{:?} resp={code_prompt_dbg} model_summary={:?} \
             {pump_dbg3} hook_records={:?} mcp_hits={}",
            turn_verdict,
            model_mock.summary(),
            fx.hook_records(),
            mock.hits()
        );

        // 会话串线排除（复核三十）：普通 prompt 的响应 meta.sessionId
        // 必须就是 code_sid，否则后续一切归因都不成立。
        let resp_sid = code_prompt_resp
            .as_ref()
            .ok()
            .and_then(|r| r.meta.as_ref())
            .and_then(|m| m.get("sessionId"))
            .and_then(|v| v.as_str())
            .unwrap_or("<none>")
            .to_string();
        assert_eq!(
            resp_sid, code_sid,
            "PromptResponse.meta.sessionId 与 code_sid 不符（会话串线）"
        );

        // 前置门通过后才等 hook 哨兵。
        let code_hook_fired = wait_until(Duration::from_secs(60), || {
            fx.hook_hits_for_session(&code_sid) > 0
        }).await;
        assert!(
            code_hook_fired,
            "主回合已成立、hook 注册态正常，但哨兵为空——分层判定：\
             xai_notes 有 HookExecution.Failed ⇒ runner/命令执行问题；\
             无对应 HookExecution ⇒ 事件匹配/dispatch 问题；\
             有 Success 却无哨兵 ⇒ 脚本自身落盘问题。\
             records={:?} mcp_hits={} model_summary={:?} resp={code_prompt_dbg} \
             pump={} jsonl_hook_lines={:?}",
            fx.hook_records(),
            mock.hits(),
            model_mock.summary(),
            Pumped::snapshot(&pumped),
            jsonl_hook_lines(&code_cwd, &code_sid)
        );
        // 归因交叉验证：该记录的 cwd 必须是 Code 的 cwd。
        assert!(
            fx.hook_records().iter().any(|r| {
                r.get("session").and_then(|v| v.as_str()) == Some(code_sid.as_str())
                    && r.get("cwd")
                        .and_then(|v| v.as_str())
                        .map(|c| std::path::Path::new(c) == code_cwd)
                        .unwrap_or(false)
            }),
            "Code hook 记录的 cwd 不匹配：{:?}",
            fx.hook_records()
        );
        let after_code = StageBaseline::take(&fx, &mock, &chat_sid, &code_sid);
        stages.push(serde_json::json!({
            "stage": "code_plugin_active",
            "code_session": code_sid,
            "code_hook_hits": after_code.code_hits,
            "chat_hook_hits": after_code.chat_hits,
            "mcp_hits_delta": after_code.mcp_hits.saturating_sub(pre_reload.mcp_hits),
            "code_plugins": code_plugins.len(),
        }));

        // 全局广播完成后，Chat actor 的 registry 必须仍为空。
        let pre_broadcast = pre_reload;
        let raw = serde_json::value::to_raw_value(&serde_json::json!({
            "sessionId": chat_sid,
        }))
        .expect("static json");
        let list_after: agent_client_protocol::ExtResponse = tokio::time::timeout(
            Duration::from_secs(20),
            xai_acp_lib::acp_send(
                agent_client_protocol::ExtRequest::new(
                    "x.ai/plugins/list".to_string(),
                    raw.into(),
                ),
                &acp_tx,
            ),
        )
        .await
        .expect("Chat plugins/list（广播后）超时")
        .expect("Chat plugins/list（广播后）失败");
        let list_json: serde_json::Value =
            serde_json::from_str(list_after.0.get()).expect("非 JSON");
        let chat_plugins_after = list_json
            .get("result")
            .unwrap_or(&list_json)
            .get("plugins")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let fanout_observed = !chat_plugins_after.is_empty();
        stages.push(serde_json::json!({
            "stage": "acp_reload_broadcast",
            "action_resp": action_dbg,
            "chat_plugins_after_broadcast": chat_plugins_after.len(),
            "fanout_observed": fanout_observed,
            "mcp_hits": mock.hits(),
            "mcp_delta_since_broadcast":
                mock.hits().saturating_sub(pre_broadcast.mcp_hits),
        }));

        // ── 阶段 6：Chat 侧归因（主证据 = 带 chat_sid 的 hook 记录）──
        const CHAT_PROMPT: &str = "probe-chat-turn-1";
        let chat_prompt_resp = send_prompt(chat_resp.session_id.clone(), CHAT_PROMPT).await;
        let chat_prompt_dbg = format!("{chat_prompt_resp:?}");
        assert!(chat_prompt_resp.is_ok(), "Chat prompt 未成功：{chat_prompt_dbg}");
        model_mock
            .assert_single_main_turn(CHAT_PROMPT, ExpectTools::ChatExact)
            .expect("Chat provider 请求工具集不精确");
        let chat_hook_polluted = fx.hook_hits_for_session(&chat_sid) > 0;
        let chat_recs: Vec<serde_json::Value> = fx
            .hook_records()
            .into_iter()
            .filter(|r| r.get("session").and_then(|v| v.as_str()) == Some(chat_sid.as_str()))
            .collect();
        // 运行时 cwd 交叉验证（复核二十三）：Chat 记录的 cwd 必须是
        // Chat 的私有 cwd，排除「记成 Code 会话」的误判。
        let chat_cwd_ok = chat_recs.iter().all(|r| {
            r.get("cwd")
                .and_then(|v| v.as_str())
                .map(|c| std::path::Path::new(c) == chat_cwd)
                .unwrap_or(false)
        });
        stages.push(serde_json::json!({
            "stage": "chat_post_broadcast_attribution",
            "chat_prompt_resp": chat_prompt_dbg,
            "chat_hook_hits": fx.hook_hits_for_session(&chat_sid),
            "chat_hook_records": chat_recs,
            "chat_cwd_attribution_ok": chat_cwd_ok,
            "chat_hook_polluted": chat_hook_polluted,
            "mcp_hits_total": mock.hits(),
            "mcp_delta_since_broadcast":
                mock.hits().saturating_sub(pre_broadcast.mcp_hits),
        }));

        assert!(
            !fanout_observed,
            "全局 fan-out 污染了禁用会话：chat_plugins={} action_resp={action_dbg}",
            chat_plugins_after.len(),
        );
        assert!(
            !chat_hook_polluted && chat_recs.is_empty(),
            "全局 fan-out 后 Chat 执行了本地 hook：chat_hooks={} \
             action_resp={action_dbg} pump={}",
            fx.hook_hits_for_session(&chat_sid),
            Pumped::snapshot(&pumped)
        );
        assert!(chat_cwd_ok, "Chat cwd 归因异常：{chat_recs:?}");

        let chat_sid = chat_sid.as_str();
        let code_sid = code_sid.as_str();
        let baseline_mcp = base.mcp_hits;
        let baseline_hook = fx.sentinel_lines("hook").len();
        eprintln!(
            "[probe] 基线：mcp_hits={baseline_mcp} hook_lines={baseline_hook} \
             mcp_port={mcp_port} root={}",
            fx.root.display()
        );

        // ── 结构化总结（复核十五）：审计用，**不替代断言** ──────────
        // 通过与否以上面的断言为准；本段只是让人一眼看清各阶段发生了
        // 什么，便于把结论贴进 PR/设计稿。真实链接入后 stages 会逐段
        // 填充（每阶段 hook/MCP 的相对增量 + 是否观察到 fan-out）。
        // `executed` 一致性守卫（复核十七：终态契约一次写死）。
        //
        // REQUIRED_STAGES 是完整契约——不是
        // 「有多少写多少」：否则将来接主体时忘记扩这个常量，只要填上
        // 两个会话 ID 就会错误转成 executed，一份半截报告被当完整证据。
        // 断言用**顺序完全相等**而非「包含全部」：缺失、重复、乱序、
        // 多余阶段四种情况一次全抓。
        const REQUIRED_STAGES: &[&str] = &[
            "baseline_after_reset",
            "chat_started_clean",
            "code_plugin_active",
            "acp_reload_broadcast",
            "chat_post_broadcast_attribution",
        ];
        let actual_stages: Vec<&str> = stages
            .iter()
            .filter_map(|st| st.get("stage").and_then(|v| v.as_str()))
            .collect();
        let stages_exact = actual_stages.as_slice() == REQUIRED_STAGES;
        // ID 守卫：非空、无 <pending>、且两者互异（同一 ID 说明会话
        // 创建或记录串了，归因失去意义）。
        let ids_resolved = !chat_sid.is_empty()
            && !code_sid.is_empty()
            && !chat_sid.contains("<pending")
            && !code_sid.contains("<pending")
            && chat_sid != code_sid;
        let status = if ids_resolved && stages_exact {
            "executed"
        } else {
            "skeleton"
        };
        assert!(
            status == "skeleton" || (ids_resolved && stages_exact),
            "状态一致性破坏：executed 要求 ID 已解析且互异、阶段序列与契约完全相等"
        );
        let summary = serde_json::json!({
            "probe": "plugin_fanout_regression",
            "status": status,
            "sessions": { "chat": chat_sid, "code": code_sid },
            "gate": {
                "preflight_with_plugin": "diagnostic(plugins.paths)",
                "engine_policy_applied": true
            },
            "stages": stages,
            "fanout_observed": fanout_observed,
            "root": fx.root.display().to_string(),
        });
        eprintln!(
            "[probe][summary] {}",
            serde_json::to_string_pretty(&summary).unwrap()
        );
    }


}
