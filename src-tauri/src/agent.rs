//! Embedded grok-build agent session for the WanCode GUI.
//!
//! Mirrors the lifecycle used by `xai-grok-pager`'s headless mode
//! (init → authenticate → new session → prompt), but pumps every ACP
//! notification to the frontend as Tauri events instead of stdout:
//!
//! - `agent://update`      — session updates (message/thought/tool chunks)
//! - `agent://permission`  — tool-call approval requests (answered via
//!   the `agent_permission_respond` command)
//! - `agent://turn-end`    — a prompt turn finished (with stop reason or error)

use std::collections::HashMap;
use std::path::PathBuf;

use crate::crash_recovery::write_session_marker;
use crate::provider_ops::{inject_managed_keys};
use crate::config_core::{validate_startup_models, StartupModels};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, anyhow};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::{Mutex, oneshot};
use tokio_util::sync::CancellationToken;

use agent_client_protocol as acp;
use xai_acp_lib::{AcpAgentTx, AcpClientMessage, acp_send};
use xai_grok_pager::acp::spawn::spawn_grok_shell;
use xai_grok_shell::agent::auth_method::AuthMethodKind;
use xai_grok_shell::agent::config::Config as AgentConfig;

/// 计划审批回包：(outcome, feedback)。
type PlanReply = (String, Option<String>);
/// 提问回包：问题文本 → 选中项列表；None = 用户取消。
type QuestionReply = Option<HashMap<String, Vec<String>>>;

pub struct AgentHandle {
    pub(crate) acp_tx: AcpAgentTx,
    pub(crate) session_id: acp::SessionId,
    cancel: CancellationToken,
    /// 会话工作区。git 命令用它本地解析 gitRoot（见 session_git_root）。
    pub cwd: PathBuf,
    /// Sidecar 解析出的真实层身份；热切换等活跃策略门只信这里。
    pub(crate) surface_kind: crate::surface::SurfaceKind,
}

#[derive(Default)]
pub struct AgentState {
    pub(crate) handle: Mutex<Option<AgentHandle>>,
    pending_permissions: Mutex<HashMap<u64, oneshot::Sender<Option<String>>>>,
    next_permission_id: AtomicU64,
    /// Pending `x.ai/exit_plan_mode` approvals → (outcome, feedback).
    pending_plans: Mutex<HashMap<u64, oneshot::Sender<PlanReply>>>,
    /// Pending `x.ai/ask_user_question` requests: answers keyed by question text.
    pending_questions: Mutex<HashMap<u64, oneshot::Sender<QuestionReply>>>,
    /// Pending `x.ai/folder_trust/request` prompts → true = trust.
    pending_trust: Mutex<HashMap<u64, oneshot::Sender<bool>>>,
    /// 后台工作会话（Review 等）：通知泵对这些会话不发 agent://update，
    /// 权限请求一律自动取消——它们绝不能污染主聊天或卡在前端审批上。
    pub(crate) background_sessions: Mutex<std::collections::HashSet<String>>,
}

#[derive(Serialize, Clone)]
pub struct StartResult {
    pub session_id: String,
    pub models: Vec<String>,
    /// 引擎实际选中的 catalog key。恢复会话时它可能与前端上一个会话的
    /// 下拉值不同；不透传会造成界面显示 glm-open、请求实际走 glm-coding。
    pub current_model_id: Option<String>,
    /// 会话真实 cwd——前端必须用它当工作区标签（#83：标签来自
    /// localStorage 而会话另有其主时，面板显示的是别的仓库）。
    pub cwd: String,
    /// Why this session cannot prompt yet, if it cannot — carried from
    /// `LoadSessionResponse.meta["x.ai/modelBlock"]`.
    ///
    /// A blocked session loads fine and its history is readable; only sending
    /// is held. Without this the client's first hint is an empty `EndTurn`,
    /// which it cannot explain or act on. Ambiguity in particular is only
    /// resolvable by the user, so it has to travel with the load result.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_block: Option<serde_json::Value>,
    /// Structured dropdown options: value is ALWAYS the catalog key; display
    /// name + sanitized endpoint host come along so two same-named models are
    /// distinguishable in the UI (v0.18.7-B).
    pub model_options: Vec<ModelOption>,
    /// #127-2：config.toml 读取/解析失败时的文件级诊断——能力元数据问题
    /// 不阻止会话启动，但绝不静默（全员 unknown 必须有可见原因）。
    pub caps_config_issue: Option<crate::caps_snapshot::FileIssue>,
    /// v0.19-2a：会话的真实层身份（来自 sidecar，不是前端猜测）。
    pub surface_kind: crate::surface::SurfaceKind,
    /// 当前策略规则代号（派生用，见 surface::CURRENT_POLICY_VERSION）。
    pub policy_version: u32,
}

#[derive(serde::Serialize, Clone, Default)]
pub struct ModelOption {
    pub id: String,
    pub name: String,
    pub endpoint_label: String,
    /// #127-2：能力 + 归属诊断（聊天目录链适配器产出；前端徽章在 PR 3）。
    pub caps: crate::caps_snapshot::ResolvedModelCaps,
}

#[derive(Serialize, Clone)]
pub struct SessionEntry {
    pub session_id: String,
    pub title: String,
    pub updated_at: String,
    pub num_messages: usize,
    pub model_id: Option<String>,
}

/// List locally stored sessions for a workspace (newest first).
#[tauri::command]
pub async fn agent_list_sessions(workspace: String) -> Result<Vec<SessionEntry>, String> {
    let sessions =
        xai_grok_shell::session::merge::fetch_merged(None, Some(&workspace), None, 30).await;
    Ok(sessions
        .into_iter()
        .map(|s| SessionEntry {
            title: if s.summary.is_empty() {
                s.first_prompt.clone().unwrap_or_else(|| "(未命名会话)".into())
            } else {
                s.summary.clone()
            },
            session_id: s.session_id,
            updated_at: s.updated_at,
            num_messages: s.num_messages,
            model_id: s.model_id,
        })
        .collect())
}

/// List MCP servers configured for a workspace (from config.toml / .mcp.json).
#[tauri::command]
pub async fn agent_list_mcp(workspace: String) -> Result<Vec<String>, String> {
    let cwd = PathBuf::from(&workspace);
    let servers = xai_grok_shell::util::config::load_mcp_servers(
        &cwd,
        &xai_grok_tools::types::compat::CompatConfig::default(),
    );
    Ok(servers
        .iter()
        .map(|s| {
            serde_json::to_value(s)
                .ok()
                .and_then(|v| v.get("name").and_then(|n| n.as_str()).map(String::from))
                .unwrap_or_else(|| "(unnamed)".into())
        })
        .collect())
}

/// 端到端**可启动**的层。W2-fe-a:仅 Chat/Code 已全链路打通(创建+显示+
/// 生命周期);Work 待 W2-fe-b、Cowork 待 Cowork 线。用于 agent_start 在发布
/// handle 之前 gate——不可启动的层绝不装 handle(否则留下前端无法显示、
/// agent_cancel 无法拆除的孤儿会话)。放行条件与前端 WORK_UI_READY 协同解除。
fn surface_launchable(kind: crate::surface::SurfaceKind) -> bool {
    use crate::surface::SurfaceKind::{Chat, Code};
    matches!(kind, Chat | Code)
}

/// Start (or restart) an embedded agent session rooted at `workspace`.
#[tauri::command]
pub async fn agent_start(
    app: AppHandle,
    state: State<'_, AgentState>,
    workspace: String,
    model: Option<String>,
    resume: Option<String>,
    surface: Option<String>,
) -> Result<StartResult, String> {
    // smoke 模式：前端不许动会话。debug 构建的 webview 若碰到活着的 dev
    // server 会加载完整前端并自动启动会话，把 autotest 的 handle 换成
    // localStorage 工作区（宿主仓库！）——run3 的 stash 事故 + S2/S4 全部
    // 抖动皆源于此。autotest 走 start_inner 内部路径，不经过这里。
    if std::env::var("WANCODE_AUTOTEST").is_ok() {
        return Err("AUTOTEST 模式：前端会话启动被禁用".into());
    }
    // Tear down any previous session first.
    if let Some(old) = state.handle.lock().await.take() {
        old.cancel.cancel();
    }

    let intent = crate::surface_policy::NewSurfaceIntent::from_wire(surface.as_deref())
        .map_err(|e| crate::surface_policy::policy_blocked_message(&e))?;
    let result = start_inner_with_intent(app, &state, workspace, model, resume, intent)
        .await
        .map_err(|e| format!("{e:#}"))?;
    Ok(result)
}

pub(crate) async fn start_inner(
    app: AppHandle,
    state: &State<'_, AgentState>,
    workspace: String,
    model: Option<String>,
    resume: Option<String>,
) -> Result<StartResult> {
    start_inner_with_intent(
        app,
        state,
        workspace,
        model,
        resume,
        crate::surface_policy::NewSurfaceIntent::Code,
    )
    .await
}

/// 新会话的内部层意图入口。恢复会话刻意忽略 `new_intent`，只从
/// sidecar 派生身份；公开 Tauri 命令在 2d 评审前仍固定 Code。
pub(crate) async fn start_inner_with_intent(
    app: AppHandle,
    state: &State<'_, AgentState>,
    workspace: String,
    model: Option<String>,
    resume: Option<String>,
    new_intent: crate::surface_policy::NewSurfaceIntent,
) -> Result<StartResult> {
    // Make WanCode-managed API keys (stored in the OS keyring) visible to the
    // engine's `env_key` resolution for this process.
    // ── 启动不变量（v0.12.2）：零模型绝不进入引擎 ─────────────────
    // 引擎在零模型状态下启动即 panic（capacity overflow / RefCell 双崩，
    // 实测）。此前的门控只在前端——恢复会话/切工作区/删最后一个模型后
    // 继续操作都可能绕过它直达这里。校验必须住在所有入口的必经之路上。
    // 错误码是前端契约：MODEL_REQUIRED → 重开向导；MODEL_CONFIG_INVALID
    // → 提示修配置。改动前先跑 config 单测。
    // ── v0.19-2a 迁移门（必经之路，与模型门同级）：层归属迁移完成前
    // 不启动任何会话。所有入口（前端 agent_start、autotest）都过这里；
    // 门内部并发共享同一结果、migration_locked 有界重试；损坏标记/迁移
    // 不完整等一律结构化阻塞（SURFACE_GATE_BLOCKED: {json}）。
    let resumed_binding = {
        let surface = app.state::<crate::surface_gate::SurfaceState>();
        if let Err(e) = surface.ensure_migrated().await {
            return Err(anyhow!("{}", crate::surface_gate::gate_blocked_message(&e)));
        }
        // 恢复会话：启动引擎、加载会话之前先 resolve——层身份只信 sidecar，
        // 不信前端参数或 localStorage。无归属/损坏/版本不支持一律在
        // 引擎起来之前拒绝。
        match resume.as_ref() {
            Some(sid) => Some(surface.resolve(sid).map_err(|e| {
                anyhow!("{}", crate::surface_gate::binding_blocked_message(&e))
            })?),
            None => None,
        }
    };
    match validate_startup_models() {
        StartupModels::Ok => {}
        StartupModels::NoModels => {
            return Err(anyhow!("MODEL_REQUIRED: 尚未配置任何模型"));
        }
        StartupModels::RepairedDefault(fixed) => {
            tracing::warn!("[models].default 悬空，已自动修复为 {fixed}");
        }
        StartupModels::Invalid(reason) => {
            return Err(anyhow!("MODEL_CONFIG_INVALID: {reason}"));
        }
    }

    inject_managed_keys();
    let surface_kind = resumed_binding
        .as_ref()
        .map(|b| b.surface_kind)
        .unwrap_or_else(|| new_intent.surface_kind());
    let is_chat = surface_kind == crate::surface::SurfaceKind::Chat;
    let cwd = if is_chat {
        // 路径必须经 resolve_chat_runtime_dir 单一来源（PR #38 F2）。
        let path = resolve_chat_runtime_dir(&app).map_err(|e| anyhow!(e))?;
        std::fs::create_dir_all(&path)
            .with_context(|| format!("创建 Chat 私有运行目录失败: {}", path.display()))?;
        path
    } else {
        let path = PathBuf::from(&workspace);
        if !path.is_dir() {
            return Err(anyhow!("工作区目录不存在: {workspace}"));
        }
        path
    };

    // 先拆掉旧会话。此前旧 handle 一直留到函数末尾才被替换——本次启动
    // 半路失败时它就成了僵尸：前端以为没会话/换了工作区，ext 调用却仍
    // 注入旧 sessionId，git 面板显示的是**另一个仓库**的改动（#83，
    // 在那个状态下 stash/丢弃会打错目标）。失败宁可「会话未启动」。
    if let Some(old) = state.handle.lock().await.take() {
        old.cancel.cancel();
    }

    // ── Config (mirrors headless.rs) ────────────────────────────────
    let raw_config =
        xai_grok_shell::config::load_effective_config().map_err(|e| anyhow!("加载配置失败: {e}"))?;
    let mut agent_config =
        AgentConfig::new_from_toml_cfg(&raw_config).map_err(|e| anyhow!("解析配置失败: {e}"))?;
    if let Some(ref m) = model {
        agent_config.default_model_override = Some(m.clone());
    }
    agent_config.resolve_runtime_fields(&xai_grok_shell::agent::config::RuntimeResolutionContext {
        raw_config: &raw_config,
        remote_settings: None,
        cwd: Some(&cwd),
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
    });
    agent_config.mode = xai_grok_shell::agent::config::AgentMode::Headless;
    // GUI answers permission requests explicitly — never yolo.
    agent_config.default_yolo_mode = false;
    agent_config.default_auto_mode =
        xai_grok_shell::util::config::effective_auto_for_launch(false, None, None);
    if is_chat {
        // 在任何引擎进程启动前确定 catalog key 并执行 agent_type 门。
        let selected_model = if let Some(sid) = resume.as_deref() {
            let summaries = xai_grok_shell::session::persistence::list_summaries(None)
                .await
                .map_err(|e| anyhow!("读取恢复会话模型失败: {e}"))?;
            let summary = summaries.into_iter()
                .find(|s| s.info.id.0.as_ref() == sid)
                .ok_or_else(|| anyhow!("恢复会话不存在: {sid}"))?;
            summary.catalog_model_id.map(|id| id.0.to_string()).ok_or_else(|| {
                anyhow!("{}", crate::surface_policy::policy_blocked_message(
                    &crate::surface_policy::SurfacePolicyError::ModelUnresolvable {
                        model_id: summary.current_model_id.0.to_string(),
                        reason: "会话缺少 catalog_model_id，不能安全判定 agent_type".into(),
                    }))
            })?
        } else {
            model.clone().or_else(|| agent_config.models.default.clone())
                .ok_or_else(|| anyhow!("Chat 无法确定启动模型"))?
        };
        let (doc, issue) =
            crate::caps_snapshot::load_config_doc(&crate::config_core::user_config_path());
        if let Some(issue) = issue {
            return Err(anyhow!("{}", crate::surface_policy::policy_blocked_message(
                &crate::surface_policy::SurfacePolicyError::ModelUnresolvable {
                    model_id: selected_model,
                    reason: format!("config.toml 不可判定：{}", issue.message),
                })));
        }
        crate::surface_policy::ensure_chat_model_allowed(&doc, &selected_model)
            .map_err(|e| anyhow!("{}", crate::surface_policy::policy_blocked_message(&e)))?;
        crate::surface_policy::apply_chat_agent_config_overrides(&mut agent_config);
        agent_config.default_auto_mode = false;

        // 引擎硬门是权威边界；旧扫描器只作可见诊断，不再全局拒绝会话。
        if let Err(e) = crate::surface_policy::enforce_chat_plugin_preflight(&cwd) {
            tracing::warn!(error = %e, "Chat 发现本地插件来源；由引擎会话硬门隔离");
        }
        if let Err(e) = crate::surface_policy::ensure_no_disk_global_hooks() {
            tracing::warn!(error = %e, "Chat 发现磁盘 hooks；由引擎会话硬门隔离");
        }
    }

    // NOTE: we deliberately do NOT grant_folder_trust() here.
    //
    // That blanket grant was written when opening a workspace always meant the
    // user had just picked it in the folder dialog. Since 0.8.2 WanCode
    // auto-opens the last-used folder (or the home directory on first run), so
    // the grant was trusting folders the user never approved — and folder trust
    // is what gates repo-local MCP servers and LSP, i.e. config a cloned repo
    // can ship to make the agent run things.
    //
    // Instead we advertise `x.ai/folderTrust.interactive` below and let the
    // engine prompt through `x.ai/folder_trust/request`. The engine keeps
    // project-scoped config gated until an explicit grant, and treats any
    // undecodable answer as reject.

    let cancel = CancellationToken::new();
    let memory_config = agent_config.memory_config.clone();
    let spawned = spawn_grok_shell(agent_config, &cancel, memory_config)
        .await
        .map_err(|e| anyhow!("启动 Agent 失败: {e}"))?;
    let acp_tx = spawned.channel.tx;
    let mut acp_rx = spawned.channel.rx;

    // ── Initialize ─────────────────────────────────────────────────
    // The trust capability is read from `client_capabilities.meta`, NOT the
    // request meta — putting it on the request silently does nothing.
    let mut caps = acp::ClientCapabilities::new()
        .fs(acp::FileSystemCapabilities::new())
        .terminal(false);
    caps.meta = serde_json::json!({ "x.ai/folderTrust": { "interactive": true } })
        .as_object()
        .cloned();

    let startup_hints = if is_chat {
        let mut hints = crate::surface_policy::chat_startup_hints();
        hints.as_object_mut().expect("static Chat hints")
            .insert("nonInteractive".into(), serde_json::Value::Bool(true));
        hints
    } else {
        serde_json::json!({
            "nonInteractive": true,
            "skipGitStatus": false,
            "skipProjectLayout": false,
        })
    };
    let init_req = acp::InitializeRequest::new(acp::ProtocolVersion::V1)
        .client_capabilities(caps)
        .meta(
            serde_json::json!({
                "clientType": "wancode",
                "clientVersion": env!("CARGO_PKG_VERSION"),
                "startupHints": startup_hints,
            })
            .as_object()
            .cloned(),
        );
    let init_resp: acp::InitializeResponse = acp_send(init_req, &acp_tx)
        .await
        .map_err(|e| anyhow!("ACP initialize 失败: {e}"))?;

    // ── Authenticate (non-interactive methods only) ─────────────────
    let method_id = init_resp
        .auth_methods
        .iter()
        .find(|m| !AuthMethodKind::from_id(m.id()).needs_interactive_login())
        .map(|m| m.id().clone())
        .context("没有可用的非交互认证方式（请在 ~/.grok/config.toml 配置模型 API Key）")?;
    let _: acp::AuthenticateResponse = acp_send(
        acp::AuthenticateRequest::new(method_id)
            .meta(serde_json::json!({"headless": true}).as_object().cloned()),
        &acp_tx,
    )
    .await
    .map_err(|e| anyhow!("认证失败: {e}"))?;

    // ── Event pump: ACP notifications → Tauri events ───────────────
    // Must start BEFORE the session opens: resuming a session replays
    // history notifications during LoadSession, and each notification
    // waits for a response — with no consumer that deadlocks.
    {
        let app = app.clone();
        let pump_cancel = cancel.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::select! {
                    _ = pump_cancel.cancelled() => break,
                    msg = acp_rx.recv() => {
                        let Some(msg) = msg else { break };
                        handle_acp_message(&app, msg).await;
                    }
                }
            }
        });
    }

    // ── Open session (new or resume-with-replay) ───────────────────
    let mcp_servers = if is_chat {
        Vec::new()
    } else {
        xai_grok_shell::util::config::load_mcp_servers(
            &cwd,
            &xai_grok_tools::types::compat::CompatConfig::default(),
        )
    };
    let session_meta = is_chat.then(|| serde_json::json!({
        "agentProfile": crate::surface_policy::chat_agent_profile(),
        "x.ai/localExtensionsDisabled": true,
    }).as_object().cloned().expect("static Chat session meta"));
    let mut model_block: Option<serde_json::Value> = None;
    let (session_id, session_models) = if let Some(sid) = resume {
        let mut req = acp::LoadSessionRequest::new(acp::SessionId::new(sid.clone()), cwd.clone())
            .mcp_servers(mcp_servers);
        if let Some(meta) = session_meta.clone() {
            req = req.meta(Some(meta));
        }
        let resp: acp::LoadSessionResponse = acp_send(
            req,
            &acp_tx,
        )
        .await
        .map_err(|e| anyhow!("恢复会话失败: {e}"))?;
        model_block = resp
            .meta
            .as_ref()
            .and_then(|m| m.get("x.ai/modelBlock"))
            .cloned();
        if is_chat && !local_extensions_policy_applied(resp.meta.as_ref()) {
            cancel.cancel();
            return Err(anyhow!("{}", crate::surface_policy::policy_blocked_message(
                &crate::surface_policy::SurfacePolicyError::LocalExtensionsPolicyNotApplied)));
        }
        (acp::SessionId::new(sid), resp.models)
    } else {
        let mut req = acp::NewSessionRequest::new(cwd.clone()).mcp_servers(mcp_servers);
        if let Some(meta) = session_meta {
            req = req.meta(Some(meta));
        }
        let resp: acp::NewSessionResponse = acp_send(
            req,
            &acp_tx,
        )
        .await
        .map_err(|e| anyhow!("创建会话失败: {e}"))?;
        if is_chat && !local_extensions_policy_applied(resp.meta.as_ref()) {
            cancel.cancel();
            return Err(anyhow!("{}", crate::surface_policy::policy_blocked_message(
                &crate::surface_policy::SurfacePolicyError::LocalExtensionsPolicyNotApplied)));
        }
        (resp.session_id, resp.models)
    };
    // ── v0.19-2a 最低身份事务链：引擎返回 ID → 写 binding → 成功后才
    // 安装 handle/返回前端。写失败即取消本次 Agent——绝不暴露可发送的
    // handle；引擎可能留下孤立会话，恢复时会被 unbound_surface 拦住，
    // 走显式恢复/认领，不会静默升 Code。
    let surface_binding = match resumed_binding {
        Some(b) => b,
        None => {
            let surface = app.state::<crate::surface_gate::SurfaceState>();
            match surface
                .bind_new_session(&session_id.0, surface_kind)
            {
                Ok(b) => b,
                Err(e) => {
                    cancel.cancel();
                    return Err(anyhow!(
                        "{}",
                        crate::surface_gate::binding_blocked_message(&e)
                    ));
                }
            }
        }
    };
    // W2-fe-a(codex R3):端到端未打通的层不得启动。Work/Cowork 在本版本没有
    // 完整链路(无创建入口、前端无显示),若放行会**在此处之下装出一个活 handle**
    // (line ~574),而前端无法显示、agent_cancel 又只取消回合不拆 handle —— 留下
    // 一个隐藏的孤儿会话。gate 在 handle 发布**之前**(与紧邻的崩溃标记 gate 同
    // 一发布事务):不可启动的层直接取消并结构化报错,绝不发布 handle。W2-fe-b
    // 打通 Work 端到端后 surface_launchable 放行 Work(Cowork 随 Cowork 线)。
    if !surface_launchable(surface_binding.surface_kind) {
        cancel.cancel();
        return Err(anyhow!(
            "SURFACE_NOT_LAUNCHABLE: {:?} 层会话在本版本尚不可启动",
            surface_binding.surface_kind
        ));
    }
    // Crash recovery is part of the same publication transaction as the
    // immutable surface binding. A session without a durable dirty marker must
    // never become the active, send-capable handle: otherwise a crash can make
    // the only recovery pointer disappear while the UI reported a valid start.
    if let Err(error) = write_session_marker(&session_id.0, &cwd.to_string_lossy(), false) {
        cancel.cancel();
        return Err(anyhow!("CRASH_RECOVERY_MARKER_FAILED: {error}"));
    }
    // #127-2 聊天目录链：同一世代快照 + config 文档，逐 option 出能力。
    // 配置读取/解析失败不阻止会话启动，但必须作为结构化诊断随
    // StartResult 返回——禁止 unwrap_or_default 静默降级为全员 unknown。
    let caps_snapshot = app.state::<crate::caps_snapshot::CapsState>().snapshot();
    let (caps_config_doc, caps_config_issue) =
        crate::caps_snapshot::load_config_doc(&crate::config_core::user_config_path());
    let (model_ids, current_model_id, model_options): (
        Vec<String>,
        Option<String>,
        Vec<ModelOption>,
    ) = session_models
        .map(|m| {
            (
                m.available_models
                    .iter()
                    .map(|am| am.model_id.0.to_string())
                    .collect(),
                Some(m.current_model_id.0.to_string()),
                m.available_models
                    .iter()
                    .map(|am| {
                        let id = am.model_id.0.to_string();
                        let caps = crate::caps_snapshot::model_option_caps(
                            &caps_snapshot,
                            &id,
                            &caps_config_doc,
                        );
                        ModelOption {
                            name: am.name.clone(),
                            endpoint_label: am
                                .meta
                                .as_ref()
                                .and_then(|meta| meta.get("endpointLabel"))
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            caps,
                            id,
                        }
                    })
                    .collect(),
            )
        })
        .unwrap_or_default();

    *state.handle.lock().await = Some(AgentHandle {
        acp_tx: acp_tx.clone(),
        session_id: session_id.clone(),
        cancel,
        cwd: cwd.clone(),
        surface_kind: surface_binding.surface_kind,
    });

    // 新会话的技能来自 agent 启动时的内存快照（self.cfg.skills），运行期改
    // 的 [skills].disabled 它看不见——引擎没有任何回灌路径。刷新只是
    // best-effort 的后置维护，绝不能卡住已经完成的会话发布：真实 Windows
    // 运行曾在会话已落盘、MCP 全健康后永久停在 UI "Starting…"，因为这里
    // 同步等待一个未回包的 ext 请求。后台任务自身仍有硬超时，避免泄漏。
    if !is_chat {
        schedule_skill_baseline_refresh(acp_tx.clone());
    }

    Ok(StartResult {
        session_id: session_id.0.to_string(),
        models: model_ids,
        current_model_id,
        cwd: cwd.to_string_lossy().into_owned(),
        model_block,
        model_options,
        caps_config_issue,
        surface_kind: surface_binding.surface_kind,
        policy_version: crate::surface::derive_effective_policy(surface_binding.surface_kind)
            .policy_version,
    })
}

#[derive(Debug, PartialEq, Eq)]
enum SkillBaselineRefreshOutcome {
    Succeeded,
    Failed(String),
    TimedOut,
}

async fn refresh_skill_baseline_with_timeout(
    acp_tx: AcpAgentTx,
    timeout: std::time::Duration,
) -> SkillBaselineRefreshOutcome {
    let raw = serde_json::value::to_raw_value(&serde_json::json!({})).expect("static json");
    match tokio::time::timeout(
        timeout,
        acp_send(
            acp::ExtRequest::new("x.ai/skills/refresh-baseline".to_string(), raw.into()),
            &acp_tx,
        ),
    )
    .await
    {
        Ok(Ok(_)) => SkillBaselineRefreshOutcome::Succeeded,
        Ok(Err(error)) => SkillBaselineRefreshOutcome::Failed(error.to_string()),
        Err(_) => SkillBaselineRefreshOutcome::TimedOut,
    }
}

fn schedule_skill_baseline_refresh(acp_tx: AcpAgentTx) {
    tauri::async_runtime::spawn(async move {
        match refresh_skill_baseline_with_timeout(acp_tx, std::time::Duration::from_secs(5)).await {
            SkillBaselineRefreshOutcome::Succeeded => {}
            SkillBaselineRefreshOutcome::Failed(error) => {
                tracing::warn!(%error, "post-start skill baseline refresh failed");
            }
            SkillBaselineRefreshOutcome::TimedOut => {
                tracing::warn!("post-start skill baseline refresh timed out");
            }
        }
    });
}

fn local_extensions_policy_applied(meta: Option<&serde_json::Map<String, serde_json::Value>>) -> bool {
    meta.and_then(|m| m.get("localExtensionsDisabledApplied"))
        .and_then(serde_json::Value::as_bool) == Some(true)
}

#[cfg(test)]
mod surface_launchable_tests {
    use super::surface_launchable;
    use crate::surface::SurfaceKind;

    #[test]
    fn only_chat_and_code_are_launchable_pre_w2fe_b() {
        // codex W2-fe-a R3:Work/Cowork 端到端未打通 → agent_start 在装 handle
        // 前据此拦截,绝不为它们发布孤儿 handle。
        assert!(surface_launchable(SurfaceKind::Chat));
        assert!(surface_launchable(SurfaceKind::Code));
        assert!(!surface_launchable(SurfaceKind::Work));
        assert!(!surface_launchable(SurfaceKind::Cowork));
    }
}

#[cfg(test)]
mod local_extensions_handshake_tests {
    use super::local_extensions_policy_applied;

    #[test]
    fn chat_requires_an_explicit_true_engine_acknowledgement() {
        assert!(!local_extensions_policy_applied(None));

        for value in [
            serde_json::Value::Null,
            serde_json::Value::Bool(false),
            serde_json::Value::String("true".to_string()),
        ] {
            let mut meta = serde_json::Map::new();
            meta.insert("localExtensionsDisabledApplied".to_string(), value);
            assert!(!local_extensions_policy_applied(Some(&meta)));
        }

        let mut meta = serde_json::Map::new();
        meta.insert(
            "localExtensionsDisabledApplied".to_string(),
            serde_json::Value::Bool(true),
        );
        assert!(local_extensions_policy_applied(Some(&meta)));
    }
}

#[cfg(test)]
mod post_start_refresh_tests {
    use super::{
        SkillBaselineRefreshOutcome, refresh_skill_baseline_with_timeout,
    };
    use xai_acp_lib::AcpAgentTx;

    #[tokio::test]
    async fn a_nonresponsive_refresh_is_bounded() {
        let (tx, _receiver): (AcpAgentTx, _) = tokio::sync::mpsc::unbounded_channel();
        let started = tokio::time::Instant::now();

        let outcome = refresh_skill_baseline_with_timeout(
            tx,
            std::time::Duration::from_millis(25),
        )
        .await;

        assert_eq!(outcome, SkillBaselineRefreshOutcome::TimedOut);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }
}

async fn handle_acp_message(app: &AppHandle, msg: AcpClientMessage) {
    match msg {
        AcpClientMessage::SessionNotification(boxed) => {
            // 后台会话（Review 等）的更新不进主聊天流
            {
                let state: State<'_, AgentState> = app.state();
                let bg = state.background_sessions.lock().await;
                if bg.contains(boxed.request.session_id.0.as_ref()) {
                    let _ = boxed.response_tx.send(Ok(()));
                    return;
                }
            }
            let payload =
                serde_json::to_value(&boxed.request.update).unwrap_or(serde_json::Value::Null);
            if std::env::var("WANCODE_AUTOTEST").is_ok() {
                use std::io::Write;
                let kind = payload
                    .get("sessionUpdate")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
                    .to_string();
                let log = std::env::temp_dir().join("wancode-autotest.log");
                if let Ok(mut f) =
                    std::fs::OpenOptions::new().create(true).append(true).open(&log)
                {
                    let _ = writeln!(f, "update: {kind}");
                }
            }
            let _ = app.emit("agent://update", payload);
            let _ = boxed.response_tx.send(Ok(()));
        }
        AcpClientMessage::RequestPermission(req) => {
            // 无头 smoke：自动选第一个选项（引擎约定首项为放行），否则
            // S3/S4 的命令权限会等前端 600 秒。仅 AUTOTEST 模式生效。
            if std::env::var("WANCODE_AUTOTEST").is_ok() {
                let first = req.request.options.first().map(|o| o.option_id.clone());
                let outcome = match first {
                    Some(id) => acp::RequestPermissionOutcome::Selected(
                        acp::SelectedPermissionOutcome::new(id),
                    ),
                    None => acp::RequestPermissionOutcome::Cancelled,
                };
                let _ = req
                    .response_tx
                    .send(Ok(acp::RequestPermissionResponse::new(outcome)));
                return;
            }
            let state: State<'_, AgentState> = app.state();
            // 后台会话理论上是只读（plan）模式；万一有工具越权申请，
            // 直接取消而不是等前端 600 秒——前端根本看不见这个会话。
            if state
                .background_sessions
                .lock()
                .await
                .contains(req.request.session_id.0.as_ref())
            {
                let _ = req.response_tx.send(Ok(acp::RequestPermissionResponse::new(
                    acp::RequestPermissionOutcome::Cancelled,
                )));
                return;
            }
            let id = state.next_permission_id.fetch_add(1, Ordering::Relaxed);
            let (tx, rx) = oneshot::channel::<Option<String>>();
            state.pending_permissions.lock().await.insert(id, tx);

            let payload = serde_json::json!({
                "id": id,
                "request": serde_json::to_value(&req.request).unwrap_or(serde_json::Value::Null),
            });
            let _ = app.emit("agent://permission", payload);

            // Wait for the frontend's decision (10 min timeout → cancel).
            tauri::async_runtime::spawn(async move {
                let decision =
                    tokio::time::timeout(std::time::Duration::from_secs(600), rx).await;
                let outcome = match decision {
                    Ok(Ok(Some(option_id))) => acp::RequestPermissionOutcome::Selected(
                        acp::SelectedPermissionOutcome::new(acp::PermissionOptionId::new(
                            option_id,
                        )),
                    ),
                    _ => acp::RequestPermissionOutcome::Cancelled,
                };
                let _ = req
                    .response_tx
                    .send(Ok(acp::RequestPermissionResponse::new(outcome)));
            });
        }
        AcpClientMessage::ExtNotification(notif) => {
            let payload = serde_json::json!({
                "method": notif.request.method.to_string(),
                "params": serde_json::to_value(&notif.request.params).unwrap_or(serde_json::Value::Null),
            });
            let _ = app.emit("agent://ext", payload);
            let _ = notif.response_tx.send(Ok(()));
        }
        AcpClientMessage::ExtMethod(args) => {
            // 后台会话的交互型 ext 请求（exit_plan_mode / ask_user_question /
            // folder_trust）绝不能弹到前端——用户根本看不见那个会话。
            // 统一自动应答：计划直接放行、提问回空、信任拒绝。
            // （实测教训：Review 子会话在 plan 模式收尾时，把审查 JSON 当
            // "计划"弹进了主 UI 的审批框。）
            {
                let params: serde_json::Value = serde_json::from_str(args.request.params.get())
                    .unwrap_or(serde_json::Value::Null);
                let sid = params
                    .get("sessionId")
                    .or_else(|| params.get("session_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let state: State<'_, AgentState> = app.state();
                if !sid.is_empty() && state.background_sessions.lock().await.contains(sid) {
                    let resp = match args.request.method.as_ref() {
                        "x.ai/exit_plan_mode" => {
                            serde_json::json!({ "outcome": "approved", "feedback": null })
                        }
                        _ => serde_json::json!({}),
                    };
                    let raw = serde_json::value::to_raw_value(&resp).unwrap();
                    let _ = args.response_tx.send(Ok(acp::ExtResponse::new(raw.into())));
                    return;
                }
            }
            if args.request.method.as_ref() == "x.ai/exit_plan_mode" {
                let params: serde_json::Value =
                    serde_json::from_str(args.request.params.get()).unwrap_or(serde_json::Value::Null);
                let plan = params
                    .get("planContent")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let state: State<'_, AgentState> = app.state();
                let id = state.next_permission_id.fetch_add(1, Ordering::Relaxed);
                let (tx, rx) = oneshot::channel::<(String, Option<String>)>();
                state.pending_plans.lock().await.insert(id, tx);
                let _ = app.emit(
                    "agent://plan-approval",
                    serde_json::json!({ "id": id, "planContent": plan }),
                );
                tauri::async_runtime::spawn(async move {
                    let (outcome, feedback) =
                        match tokio::time::timeout(std::time::Duration::from_secs(600), rx).await {
                            Ok(Ok(v)) => v,
                            _ => ("cancelled".to_string(), None),
                        };
                    let resp = serde_json::json!({ "outcome": outcome, "feedback": feedback });
                    let raw = serde_json::value::to_raw_value(&resp).unwrap();
                    let _ = args.response_tx.send(Ok(acp::ExtResponse::new(raw.into())));
                });
            } else if args.request.method.as_ref() == "x.ai/ask_user_question" {
                // The agent is asking the user something. Previously this fell
                // into the catch-all below and got answered with `{}` — the
                // question never reached the user and the model saw a blank.
                let params: serde_json::Value =
                    serde_json::from_str(args.request.params.get()).unwrap_or(serde_json::Value::Null);
                let questions = params
                    .get("questions")
                    .cloned()
                    .unwrap_or(serde_json::Value::Array(vec![]));
                let state: State<'_, AgentState> = app.state();
                let id = state.next_permission_id.fetch_add(1, Ordering::Relaxed);
                let (tx, rx) =
                    oneshot::channel::<Option<HashMap<String, Vec<String>>>>();
                state.pending_questions.lock().await.insert(id, tx);
                let _ = app.emit(
                    "agent://ask-question",
                    serde_json::json!({ "id": id, "questions": questions }),
                );
                tauri::async_runtime::spawn(async move {
                    let answered =
                        match tokio::time::timeout(std::time::Duration::from_secs(600), rx).await {
                            Ok(Ok(v)) => v,
                            _ => None,
                        };
                    // Tagged on "outcome" — see AskUserQuestionExtResponse.
                    let resp = match answered {
                        Some(answers) => {
                            serde_json::json!({ "outcome": "accepted", "answers": answers })
                        }
                        None => serde_json::json!({ "outcome": "cancelled" }),
                    };
                    let raw = serde_json::value::to_raw_value(&resp).unwrap();
                    let _ = args.response_tx.send(Ok(acp::ExtResponse::new(raw.into())));
                });
            } else if args.request.method.as_ref() == "x.ai/folder_trust/request" {
                // 引擎问：这个工作区里有 repo 自带的 MCP/hooks/LSP 配置，
                // 要不要信任？未信任前引擎已把这些配置挡住了。
                let params: serde_json::Value =
                    serde_json::from_str(args.request.params.get()).unwrap_or(serde_json::Value::Null);
                let state: State<'_, AgentState> = app.state();
                let id = state.next_permission_id.fetch_add(1, Ordering::Relaxed);
                let (tx, rx) = oneshot::channel::<bool>();
                state.pending_trust.lock().await.insert(id, tx);
                let _ = app.emit(
                    "agent://folder-trust",
                    serde_json::json!({
                        "id": id,
                        "workspace": params.get("workspace").and_then(|v| v.as_str()).unwrap_or(""),
                        "cwd": params.get("cwd").and_then(|v| v.as_str()).unwrap_or(""),
                        "configKinds": params.get("configKinds").cloned()
                            .unwrap_or(serde_json::Value::Array(vec![])),
                    }),
                );
                tauri::async_runtime::spawn(async move {
                    // 超时/关闭一律按拒绝——引擎也把任何无法解码的回复当拒绝。
                    let trusted =
                        matches!(tokio::time::timeout(std::time::Duration::from_secs(600), rx).await,
                            Ok(Ok(true)));
                    let resp = serde_json::json!({
                        "outcome": if trusted { "trust" } else { "reject" }
                    });
                    let raw = serde_json::value::to_raw_value(&resp).unwrap();
                    let _ = args.response_tx.send(Ok(acp::ExtResponse::new(raw.into())));
                });
            } else {
                // Unknown reverse ext-request: answer with empty ok so the
                // agent-side tool call doesn't hang/fail.
                let raw = serde_json::value::to_raw_value(&serde_json::json!({})).unwrap();
                let _ = args.response_tx.send(Ok(acp::ExtResponse::new(raw.into())));
            }
        }
        _ => {}
    }
}

/// Answer a pending plan-mode approval (`x.ai/exit_plan_mode`).
/// `outcome`: "approved" | "cancelled" | "abandoned".
#[tauri::command]
pub async fn agent_plan_respond(
    state: State<'_, AgentState>,
    id: u64,
    outcome: String,
    feedback: Option<String>,
) -> Result<(), String> {
    let sender = state.pending_plans.lock().await.remove(&id);
    match sender {
        Some(tx) => {
            let _ = tx.send((outcome, feedback));
            Ok(())
        }
        None => Err(format!("没有待处理的计划审批 #{id}")),
    }
}


/// A pasted image: base64 data + mime type.
#[derive(serde::Deserialize)]
pub struct PromptImage {
    pub data: String,
    pub mime: String,
}

/// Send one user prompt (optionally with pasted images for vision models);
/// resolves when the turn completes.
#[tauri::command]
pub async fn agent_prompt(
    app: AppHandle,
    state: State<'_, AgentState>,
    text: String,
    images: Option<Vec<PromptImage>>,
) -> Result<(), String> {
    let (acp_tx, session_id) = {
        let guard = state.handle.lock().await;
        let h = guard.as_ref().ok_or("会话未启动")?;
        (h.acp_tx.clone(), h.session_id.clone())
    };
    let mut blocks = vec![acp::ContentBlock::Text(acp::TextContent::new(text))];
    for img in images.unwrap_or_default() {
        blocks.push(acp::ContentBlock::Image(acp::ImageContent::new(img.data, img.mime)));
    }
    let request = acp::PromptRequest::new(session_id, blocks);
    let result: Result<acp::PromptResponse, _> = acp_send(request, &acp_tx).await;
    let payload = match &result {
        Ok(resp) => serde_json::json!({
            "ok": true,
            "stopReason": serde_json::to_value(resp.stop_reason).unwrap_or(serde_json::Value::Null),
        }),
        Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
    };
    let _ = app.emit("agent://turn-end", payload);
    result.map(|_| ()).map_err(|e| e.to_string())
}

/// Answer a pending permission request. `option_id = None` cancels/denies.
#[tauri::command]
pub async fn agent_permission_respond(
    state: State<'_, AgentState>,
    id: u64,
    option_id: Option<String>,
) -> Result<(), String> {
    let sender = state.pending_permissions.lock().await.remove(&id);
    match sender {
        Some(tx) => {
            let _ = tx.send(option_id);
            Ok(())
        }
        None => Err(format!("没有待处理的权限请求 #{id}")),
    }
}



/// Call an `x.ai/*` ACP extension method against the live session and
/// return the raw JSON response.
pub(crate) async fn ext_call(
    state: &State<'_, AgentState>,
    method: &str,
    mut params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let (acp_tx, session_id) = {
        let guard = state.handle.lock().await;
        let h = guard.as_ref().ok_or("会话未启动")?;
        (h.acp_tx.clone(), h.session_id.clone())
    };
    if let Some(obj) = params.as_object_mut() {
        // 引擎里同级方法的命名并不统一：mcp/list 用 camelCase 的 sessionId，
        // 而 mcp/toggle / toggle_tool / auth_trigger 用 snake_case 的
        // session_id。两个都塞进去——没有 deny_unknown_fields，多余的键会被
        // 忽略，但少一个就是静默的 missing field 失败。
        //
        // 例外：参数结构体上带 #[serde(alias)] 的方法，两个键会映射到同一
        // 字段，serde 直接报 duplicate field。目前引擎里只有 rewind/*
        // （snake 为主名）和 debug/*（camel 为主名）用 alias——这两族只塞一个。
        let sid = serde_json::Value::String(session_id.0.to_string());
        if method.starts_with("x.ai/rewind") {
            obj.entry("session_id").or_insert(sid);
        } else if method.starts_with("x.ai/debug") {
            obj.entry("sessionId").or_insert(sid);
        } else {
            obj.entry("sessionId").or_insert(sid.clone());
            obj.entry("session_id").or_insert(sid);
        }
    }
    // #83：git/*（worktree 除外）一律显式带 gitRoot。引擎在会话目录不是
    // 仓库时会静默回退到 workspace-hub 根——嵌入式场景那是本应用自己的
    // 仓库。客户端解析不出仓库就本地拒绝，绝不触发那个回退。
    if method.starts_with("x.ai/git/") && !method.starts_with("x.ai/git/worktree") {
        if let Some(obj) = params.as_object_mut() {
            if !obj.contains_key("gitRoot") && !obj.contains_key("git_root") {
                let root = {
                    let guard = state.handle.lock().await;
                    let h = guard.as_ref().ok_or("会话未启动")?;
                    git2::Repository::discover(&h.cwd)
                        .ok()
                        .and_then(|r| r.workdir().map(|p| p.to_string_lossy().into_owned()))
                };
                let Some(root) = root else {
                    return Err("当前工作区不是 git 仓库".into());
                };
                obj.insert("gitRoot".into(), serde_json::Value::String(root));
            }
        }
    }
    let raw = serde_json::value::to_raw_value(&params).map_err(|e| e.to_string())?;
    let resp: acp::ExtResponse =
        acp_send(acp::ExtRequest::new(method.to_string(), raw.into()), &acp_tx)
            .await
            .map_err(|e| e.to_string())?;
    serde_json::from_str(resp.0.get()).map_err(|e| e.to_string())
}

/// Fire-and-forget ext *notification* (no response), e.g. the `x.ai/queue/*`
/// edit operations — the engine handles those on the notification path, not
/// as requests.
pub(crate) async fn ext_notify(
    state: &State<'_, AgentState>,
    method: &str,
    mut params: serde_json::Value,
) -> Result<(), String> {
    let (acp_tx, session_id) = {
        let guard = state.handle.lock().await;
        let h = guard.as_ref().ok_or("会话未启动")?;
        (h.acp_tx.clone(), h.session_id.clone())
    };
    if let Some(obj) = params.as_object_mut() {
        obj.entry("sessionId")
            .or_insert(serde_json::Value::String(session_id.0.to_string()));
        // 不注入 owner：排队条目经标准 ACP prompt 入队，owner=None（我们从未
        // 声明 origin client），而 remove/interject/clear 的守卫要求请求 owner
        // 与条目 owner 精确匹配——注入 "wancode" 会永远匹配不上，整族操作
        // 静默 no-op（用户实报"按钮没反应"）。与 yolo_mode_changed 同一教训：
        // 单客户端应用不传标识（None=匹配全部）才是正确姿势。
    }
    let raw = serde_json::value::to_raw_value(&params).map_err(|e| e.to_string())?;
    let _: () = acp_send(
        acp::ExtNotification::new(method.to_string(), raw.into()),
        &acp_tx,
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Mid-turn interjection (`x.ai/interject`): steer the agent WITHOUT waiting
/// for the turn to finish and without cancelling it. The session actor drains
/// it at the next safe point. Distinct from queueing (runs after the turn).
///
/// The engine broadcasts `x.ai/session/interjection` to every attached pane;
/// we mint `interjectionId` so the frontend can dedup its own optimistic echo.
#[tauri::command]
pub async fn agent_interject(
    state: State<'_, AgentState>,
    text: String,
    interjection_id: String,
) -> Result<serde_json::Value, String> {
    ext_call(
        &state,
        "x.ai/interject",
        serde_json::json!({ "text": text, "interjectionId": interjection_id }),
    )
    .await
}

/// Edit a queued prompt in place (`x.ai/queue/edit`, notification path).
#[tauri::command]
pub async fn agent_queue_edit(
    state: State<'_, AgentState>,
    id: String,
    new_text: String,
) -> Result<(), String> {
    ext_notify(
        &state,
        "x.ai/queue/edit",
        serde_json::json!({ "id": id, "newText": new_text }),
    )
    .await
}

/// Reorder the queue (`x.ai/queue/reorder`). Full ordered id list wins.
#[tauri::command]
pub async fn agent_queue_reorder(
    state: State<'_, AgentState>,
    ordered_ids: Vec<String>,
) -> Result<(), String> {
    ext_notify(
        &state,
        "x.ai/queue/reorder",
        serde_json::json!({ "orderedIds": ordered_ids }),
    )
    .await
}

/// Promote a queued prompt to a mid-turn interjection (`x.ai/queue/interject`):
/// it runs NOW instead of waiting its turn. Version-guarded like remove.
#[tauri::command]
pub async fn agent_queue_interject(
    state: State<'_, AgentState>,
    id: String,
    expected_version: u64,
) -> Result<(), String> {
    ext_notify(
        &state,
        "x.ai/queue/interject",
        serde_json::json!({ "id": id, "expectedVersion": expected_version }),
    )
    .await
}

/// Toggle plan mode (`x.ai/toggle_plan_mode`, notification path). The engine
/// flips plan⇄default and emits `current_mode_update`, which the UI already
/// follows — so this needs no response handling. Bound to Shift+Tab.
#[tauri::command]
pub async fn agent_toggle_plan_mode(state: State<'_, AgentState>) -> Result<(), String> {
    ext_notify(&state, "x.ai/toggle_plan_mode", serde_json::json!({})).await
}

/// Forget all "always allow" tool-permission grants (`x.ai/permissions/reset`).
#[tauri::command]
pub async fn permissions_reset(state: State<'_, AgentState>) -> Result<(), String> {
    ext_notify(&state, "x.ai/permissions/reset", serde_json::json!({})).await
}

/// Sync the client-side permission mode to the engine
/// (`x.ai/yolo_mode_changed`). Until now bypass/auto were client-side only —
/// the engine still raised permission requests and we auto-answered them.
/// With this the engine skips the round-trip entirely.
///
/// Key casing is the engine's, verbatim: `clientIdentifier` is camelCase,
/// `yolo_mode` / `auto_mode` / `permission_mode` are snake_case.
#[tauri::command]
pub async fn agent_sync_permission_mode(
    state: State<'_, AgentState>,
    yolo: bool,
    auto: bool,
) -> Result<(), String> {
    ext_notify(
        &state,
        "x.ai/yolo_mode_changed",
        // 不传 clientIdentifier：引擎按 origin_client.product == sender 匹配
        // 会话，而我们从未在 initialize meta 里声明过 origin client（= None），
        // 传了标识就永远匹配不上——同步变成静默 no-op（实测踩过：切了自动
        // 模式引擎照样发权限请求）。单客户端应用走 sender_id.is_none() 分支
        // 匹配全部会话即可。
        serde_json::json!({
            "yolo_mode": yolo,
            "auto_mode": auto,
            "permission_mode": if yolo { "yolo" } else if auto { "auto" } else { "default" },
        }),
    )
    .await
}

/// Drop one queued prompt. `expected_version` guards against acting on a stale
/// view (mismatch = benign no-op + the engine rebroadcasts the queue).
#[tauri::command]
pub async fn agent_queue_remove(
    state: State<'_, AgentState>,
    id: String,
    expected_version: u64,
) -> Result<(), String> {
    ext_notify(
        &state,
        "x.ai/queue/remove",
        serde_json::json!({ "id": id, "expectedVersion": expected_version }),
    )
    .await
}

/// Drop every prompt this client queued.
#[tauri::command]
pub async fn agent_queue_clear(state: State<'_, AgentState>) -> Result<(), String> {
    ext_notify(&state, "x.ai/queue/clear", serde_json::json!({})).await
}

/// Compact the conversation to reclaim context (`x.ai/compact_conversation`).
#[tauri::command]
pub async fn agent_compact(
    state: State<'_, AgentState>,
    user_context: Option<String>,
) -> Result<serde_json::Value, String> {
    ext_call(
        &state,
        "x.ai/compact_conversation",
        serde_json::json!({ "userContext": user_context }),
    )
    .await
}

/// Flatten `session_summaries/workspace_list` into a cross-workspace "recent
/// sessions" list for the home screen, newest first.
///
/// The engine groups summaries by cwd; the home screen wants the opposite view
/// — the last N sessions regardless of which project they belong to — so the
/// regrouping happens here rather than in the UI.
#[tauri::command]
pub async fn recent_sessions(
    state: State<'_, AgentState>,
    limit: Option<usize>,
) -> Result<Vec<serde_json::Value>, String> {
    let v = ext_call(
        &state,
        "x.ai/session_summaries/workspace_list",
        serde_json::json!({}),
    )
    .await?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    let map = v
        .get("result")
        .and_then(|r| r.get("all_sessions"))
        .or_else(|| v.get("all_sessions"))
        .and_then(|m| m.as_object())
        .cloned()
        .unwrap_or_default();

    let mut out: Vec<serde_json::Value> = map
        .into_iter()
        .flat_map(|(path, sessions)| {
            let path = path.clone();
            sessions
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(move |s| {
                    let get = |k: &str| s.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
                    serde_json::json!({
                        "path": path,
                        "sessionId": s.get("info").and_then(|i| i.get("id"))
                            .and_then(|x| x.as_str()).unwrap_or("").to_string(),
                        "title": get("session_summary"),
                        "updatedAt": get("updated_at"),
                        "branch": get("head_branch"),
                        "messages": s.get("num_chat_messages")
                            .and_then(|x| x.as_u64()).unwrap_or(0),
                    })
                })
                .collect::<Vec<_>>()
        })
        // 空会话（一条消息都没有）对首页没有意义
        .filter(|s| s.get("messages").and_then(|m| m.as_u64()).unwrap_or(0) > 0)
        .collect();

    out.sort_by(|a, b| {
        b.get("updatedAt")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .cmp(a.get("updatedAt").and_then(|v| v.as_str()).unwrap_or(""))
    });
    out.truncate(limit.unwrap_or(8));
    Ok(out)
}

/// Enveloped ext call: unwrap `{result, error}` — Err on engine error, else
/// the inner result. 90% of the P2 surface is exactly this shape.
pub(crate) async fn ext_ok(
    state: &State<'_, AgentState>,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let v = ext_call(state, method, params).await?;
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        return Err(e.to_string());
    }
    Ok(v.get("result").cloned().unwrap_or(v))
}



/// Answer a pending folder-trust prompt. Anything but an explicit `true`
/// leaves repo-local MCP/hooks/LSP gated.
#[tauri::command]
pub async fn agent_trust_respond(
    state: State<'_, AgentState>,
    id: u64,
    trust: bool,
) -> Result<(), String> {
    let sender = state.pending_trust.lock().await.remove(&id);
    sender
        .ok_or("该信任请求已失效")?
        .send(trust)
        .map_err(|_| "回传信任决定失败".to_string())
}

/// Answer a pending `x.ai/ask_user_question`. `answers` maps each question's
/// text to the chosen option labels; `None` = the user dismissed it.
#[tauri::command]
pub async fn agent_question_respond(
    state: State<'_, AgentState>,
    id: u64,
    answers: Option<HashMap<String, Vec<String>>>,
) -> Result<(), String> {
    let sender = state.pending_questions.lock().await.remove(&id);
    sender
        .ok_or("该提问已失效")?
        .send(answers)
        .map_err(|_| "回传答案失败".to_string())
}





/// Best-effort default working directory when the user hasn't picked one yet,
/// so the composer is usable immediately (Claude Code / Codex launch in cwd).
#[tauri::command]
pub fn default_workspace() -> String {
    std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string())
}

/// Chat 私有运行目录名。唯一字面量出处——除 [`chat_runtime_dir_under`]
/// 外任何代码不得再拼写它（PR #38 F2：两处独立字面量可各自漂移，
/// 重现"侧栏查的目录 ≠ 引擎写的目录"的隐形丢历史 bug）。
const CHAT_RUNTIME_DIR_NAME: &str = "chat-runtime";

/// Chat 私有运行目录的唯一推导点（纯函数，可测）。
pub(crate) fn chat_runtime_dir_under(app_data_dir: PathBuf) -> PathBuf {
    app_data_dir.join(CHAT_RUNTIME_DIR_NAME)
}

/// 解析当前应用的 Chat 私有运行目录。`start_inner` 的 is_chat 分支与
/// `chat_workspace` 命令都必须经由此函数取路径。
pub(crate) fn resolve_chat_runtime_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(chat_runtime_dir_under(
        app.path()
            .app_data_dir()
            .map_err(|e| format!("解析 Chat 私有运行目录失败: {e}"))?,
    ))
}

/// Chat 界面的私有工作区路径（app_data_dir/chat-runtime）。
/// 侧栏用它列出 Chat 会话——此前切到 Chat 直接清空列表，已存在的 Chat
/// 会话永不显示（v0.19 Chat 分层漏环）。与 agent_start 的 is_chat 分支
/// 共用 [`resolve_chat_runtime_dir`]，单一来源保证查询目录即写入目录。
/// 本命令只读——目录创建的副作用归会话启动所有。
#[tauri::command]
pub fn chat_workspace(app: AppHandle) -> Result<String, String> {
    let path = resolve_chat_runtime_dir(&app)?;
    Ok(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod chat_runtime_dir_tests {
    use super::*;

    #[test]
    fn chat_runtime_dir_is_app_data_joined_with_the_single_literal() {
        let base = PathBuf::from("C:/Users/x/AppData/Roaming/wancode");
        let dir = chat_runtime_dir_under(base.clone());
        assert_eq!(dir, base.join("chat-runtime"));
        assert_eq!(
            dir.file_name().and_then(|n| n.to_str()),
            Some(CHAT_RUNTIME_DIR_NAME)
        );
    }
}

/// Interrupt the current turn.
#[tauri::command]
pub async fn agent_cancel(state: State<'_, AgentState>) -> Result<(), String> {
    let (acp_tx, session_id) = {
        let guard = state.handle.lock().await;
        let h = guard.as_ref().ok_or("会话未启动")?;
        (h.acp_tx.clone(), h.session_id.clone())
    };
    acp_send(acp::CancelNotification::new(session_id), &acp_tx)
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}
