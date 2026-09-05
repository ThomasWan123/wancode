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

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;

use crate::capability_broker::{
    CapabilityLease, LeaseRequest, McpInheritance, ResourceKind, ResourceRegistry, ToolRisk,
};
use crate::crash_recovery::write_session_marker;
use crate::execution_ledger::{
    hex_sha256, prompt_evidence, ApprovalDecision, EventContext, ExecutionEventKind,
    ExecutionLedger, FrozenRequestEvidence, LedgerDiagnostics, LedgerRedactor, SessionEndReason,
    TurnOutcome,
};
use crate::provider_profile::{infer_family, ProviderProfile, ProviderUsageFacts};
use crate::provider_ops::{inject_managed_keys};
use crate::config_core::{validate_startup_models, StartupModels};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

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

struct PendingPermission {
    sender: oneshot::Sender<Option<String>>,
    session_id: String,
    lease_id: String,
    call_id: String,
    action_fingerprint: String,
    option_ids: BTreeSet<String>,
}

fn validate_pending_permission(
    pending: &PendingPermission,
    live_session_id: &str,
    live_lease_id: &str,
    option_id: Option<&str>,
) -> Result<(), &'static str> {
    if pending.session_id != live_session_id || pending.lease_id != live_lease_id {
        return Err("stale_receipt");
    }
    if option_id.is_some_and(|selected| !pending.option_ids.contains(selected)) {
        return Err("invalid_option");
    }
    Ok(())
}

fn resource_kind_code(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Terminal => "terminal",
        ResourceKind::Job => "job",
        ResourceKind::Mcp => "mcp",
        ResourceKind::Worktree => "worktree",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalResourceAction {
    None,
    Create,
    List,
    Use,
    Release,
}

fn terminal_resource_action(method: &str) -> TerminalResourceAction {
    match method {
        "x.ai/terminal/create" | "x.ai/terminal/pty/create" => TerminalResourceAction::Create,
        "x.ai/terminal/list" => TerminalResourceAction::List,
        "x.ai/terminal/release" | "x.ai/terminal/kill" => TerminalResourceAction::Release,
        "x.ai/terminal/output"
        | "x.ai/terminal/background"
        | "x.ai/terminal/wait_for_exit"
        | "x.ai/terminal/pty/load"
        | "x.ai/terminal/pty/resize"
        | "x.ai/terminal/pty/input" => TerminalResourceAction::Use,
        _ => TerminalResourceAction::None,
    }
}

fn terminal_id_from_params(params: &serde_json::Value) -> Option<&str> {
    params
        .get("terminalId")
        .or_else(|| params.get("terminal_id"))
        .and_then(serde_json::Value::as_str)
}

fn terminal_id_from_response(response: &serde_json::Value) -> Option<&str> {
    let result = response.get("result").unwrap_or(response);
    result
        .get("terminalId")
        .or_else(|| result.get("terminal_id"))
        .and_then(serde_json::Value::as_str)
}

fn extension_response_succeeded(response: &serde_json::Value) -> bool {
    response
        .get("error")
        .map(serde_json::Value::is_null)
        .unwrap_or(true)
}

fn retain_owned_terminals(
    response: &mut serde_json::Value,
    registry: &ResourceRegistry,
    lease: &CapabilityLease,
) {
    let terminals = if response.get("result").is_some() {
        response
            .get_mut("result")
            .and_then(|result| result.get_mut("terminals"))
            .and_then(serde_json::Value::as_array_mut)
    } else {
        response
            .get_mut("terminals")
            .and_then(serde_json::Value::as_array_mut)
    };
    let Some(terminals) = terminals else { return };
    terminals.retain(|terminal| {
        terminal
            .get("terminalId")
            .or_else(|| terminal.get("terminal_id"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(|terminal_id| {
                registry
                    .authorize(lease, ResourceKind::Terminal, terminal_id)
                    .is_ok()
            })
    });
}

pub struct AgentHandle {
    pub(crate) acp_tx: AcpAgentTx,
    pub(crate) session_id: acp::SessionId,
    cancel: CancellationToken,
    /// 会话工作区。git 命令用它本地解析 gitRoot（见 session_git_root）。
    pub cwd: PathBuf,
    /// Sidecar 解析出的真实层身份；热切换等活跃策略门只信这里。
    pub(crate) surface_kind: crate::surface::SurfaceKind,
    /// Work workspace identity, copied from the durable surface binding.
    pub(crate) work_workspace_id: Option<crate::work_staging::WorkspaceId>,
    /// Catalog identity, never the provider-facing model id. The ledger and
    /// provider policy must not collapse same-named models across providers.
    pub(crate) provider_catalog_key: Option<String>,
    /// Provider-bound request identity. Tuning fields are evidence records;
    /// the external engine keeps its separately audited scheduling policy.
    pub(crate) provider_profile: ProviderProfile,
    /// Immutable authority snapshot issued before this handle becomes visible.
    /// Host dispatchers must authorize against this lease; model-provided tool
    /// metadata is presentation evidence only and cannot expand the lease.
    pub(crate) capability_lease: Arc<CapabilityLease>,
}

#[derive(Default)]
pub struct AgentState {
    pub(crate) handle: Mutex<Option<AgentHandle>>,
    pending_permissions: Mutex<HashMap<u64, PendingPermission>>,
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
    /// One model-visible turn per live session. Concurrent sends are rejected;
    /// this also gives ACP tool/approval notifications an unambiguous owner.
    active_turns: Mutex<HashMap<String, String>>,
    /// ACP updates can omit tool kind/title. Remember only the bounded generic
    /// category keyed by (session, call); never retain raw input or output.
    ledger_tool_calls: Mutex<HashMap<(String, String), String>>,
    /// Terminal calls are remembered for the process lifetime so duplicate or
    /// late ACP updates cannot create a second terminal fact.
    ledger_terminal_calls: Mutex<std::collections::HashSet<(String, String)>>,
    /// A corrupt completed ledger line is sticky for this process: execution
    /// stays blocked instead of silently starting an unrelated audit trail.
    execution_ledger: OnceLock<Result<Arc<ExecutionLedger>, String>>,
    /// Host resources are capabilities only when bound to the immutable live
    /// lease. ACP call IDs alone never confer ownership.
    resource_registry: ResourceRegistry,
    next_turn_id: AtomicU64,
}

impl AgentState {
    /// The cwd owned by the live engine handle. UI folders are presentation
    /// state (especially in Work) and must not select the session store.
    pub(crate) async fn live_session_cwd(&self) -> Result<String, String> {
        let guard = self.handle.lock().await;
        let handle = guard.as_ref().ok_or(SESSION_NOT_STARTED_ERROR)?;
        Ok(handle.cwd.to_string_lossy().into_owned())
    }

    fn execution_ledger(&self, app: &AppHandle) -> Result<Arc<ExecutionLedger>, String> {
        self.execution_ledger
            .get_or_init(|| {
                let root = if let Ok(workspace) = std::env::var("WANCODE_AUTOTEST") {
                    PathBuf::from(workspace).join("execution-ledger")
                } else {
                    app.path()
                        .app_data_dir()
                        .map_err(|error| {
                            format!("解析 execution ledger app_data_dir 失败: {error}")
                        })?
                        .join("execution-ledger")
                };
                ExecutionLedger::open(root)
                    .map(Arc::new)
                    .map_err(|error| format!("EXECUTION_LEDGER_BLOCKED: {error}"))
            })
            .clone()
    }

    fn next_turn_id(&self) -> Result<String, String> {
        let ordinal = self
            .next_turn_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .map_err(|_| "execution turn id exhausted".to_string())?;
        Ok(format!("wt-{:08x}-{ordinal:016x}", std::process::id()))
    }

    async fn live_event_context(
        &self,
        session_id: &str,
        call_id: Option<String>,
    ) -> Result<EventContext, String> {
        let (surface_kind, provider_catalog_key, agent_id) = {
            let guard = self.handle.lock().await;
            let handle = guard.as_ref().ok_or(SESSION_NOT_STARTED_ERROR)?;
            if handle.session_id.0.as_ref() != session_id {
                return Err("ACP event session does not own the live handle".to_string());
            }
            handle
                .capability_lease
                .validate()
                .map_err(|error| format!("CAPABILITY_LEASE_INVALID: {error}"))?;
            if handle.capability_lease.session_id != session_id
                || handle.capability_lease.surface_kind != handle.surface_kind
                || handle.capability_lease.policy_version
                    != crate::surface::CURRENT_POLICY_VERSION
            {
                return Err("CAPABILITY_LEASE_BINDING_MISMATCH".to_string());
            }
            let provider_key = handle
                .provider_catalog_key
                .as_deref()
                .unwrap_or("provider-route-unavailable");
            if handle.capability_lease.provider_route_hash != hex_sha256(provider_key.as_bytes()) {
                return Err("CAPABILITY_LEASE_PROVIDER_MISMATCH".to_string());
            }
            (
                handle.surface_kind,
                handle.provider_catalog_key.clone(),
                handle.capability_lease.agent_id.clone(),
            )
        };
        let turn_id = self.active_turns.lock().await.get(session_id).cloned();
        Ok(EventContext {
            session_id: session_id.to_string(),
            surface_kind,
            policy_version: crate::surface::CURRENT_POLICY_VERSION,
            provider_catalog_key,
            turn_id,
            step_id: None,
            call_id,
            agent_id: Some(agent_id),
        })
    }

    async fn append_live_event(
        &self,
        session_id: &str,
        call_id: Option<String>,
        event: ExecutionEventKind,
    ) -> Result<(), String> {
        let context = self.live_event_context(session_id, call_id).await?;
        let ledger = self
            .execution_ledger
            .get()
            .ok_or("execution ledger was not initialized")?
            .clone()?;
        ledger
            .append(context, event)
            .map(|_| ())
            .map_err(|error| format!("EXECUTION_LEDGER_APPEND_FAILED: {error}"))
    }

    async fn live_capability_lease(
        &self,
        session_id: &str,
    ) -> Result<Arc<CapabilityLease>, String> {
        let guard = self.handle.lock().await;
        let handle = guard.as_ref().ok_or(SESSION_NOT_STARTED_ERROR)?;
        if handle.session_id.0.as_ref() != session_id {
            return Err("ACP resource session does not own the live handle".to_string());
        }
        handle
            .capability_lease
            .validate()
            .map_err(|error| format!("CAPABILITY_LEASE_INVALID: {error}"))?;
        Ok(handle.capability_lease.clone())
    }

    async fn register_live_resource(
        &self,
        session_id: &str,
        lease: &CapabilityLease,
        kind: ResourceKind,
        resource_id: &str,
    ) -> Result<(), String> {
        let resource_id_hash = self.resource_registry
            .register(lease, kind, resource_id)
            .map_err(|error| format!("RESOURCE_OWNERSHIP_BLOCKED: {error}"))?;
        if let Err(error) = self.append_live_event(
            session_id,
            None,
            ExecutionEventKind::ResourceCreated {
                resource_kind: resource_kind_code(kind).to_string(),
                resource_id_hash,
            },
        ).await {
            let _ = self.resource_registry.release(lease, kind, resource_id);
            return Err(error);
        }
        Ok(())
    }

    fn authorize_live_resource(
        &self,
        lease: &CapabilityLease,
        kind: ResourceKind,
        resource_id: &str,
    ) -> Result<(), String> {
        self.resource_registry
            .authorize(lease, kind, resource_id)
            .map_err(|error| format!("RESOURCE_OWNERSHIP_BLOCKED: {error}"))
    }

    async fn release_live_resource(
        &self,
        session_id: &str,
        lease: &CapabilityLease,
        kind: ResourceKind,
        resource_id: &str,
    ) -> Result<(), String> {
        self.resource_registry
            .release(lease, kind, resource_id)
            .map_err(|error| format!("RESOURCE_RELEASE_FAILED: {error}"))?;
        self.append_live_event(
            session_id,
            None,
            ExecutionEventKind::ResourceReleased {
                resource_kind: resource_kind_code(kind).to_string(),
                resource_id_hash: hex_sha256(resource_id.as_bytes()),
            },
        ).await
    }

    /// A host resource that cannot be committed to the ownership registry must
    /// not leave a send-capable session behind. Abort only the exact session
    /// that created it and release every binding owned by its immutable lease.
    async fn abort_live_session_after_resource_failure(
        &self,
        session_id: &str,
    ) -> Result<usize, String> {
        let handle = {
            let mut guard = self.handle.lock().await;
            match guard.as_ref() {
                Some(handle) if handle.session_id.0.as_ref() == session_id => {
                    guard.take().expect("checked live handle")
                }
                Some(_) => return Err("RESOURCE_SESSION_REPLACED".to_string()),
                None => return Err("RESOURCE_SESSION_NOT_LIVE".to_string()),
            }
        };
        handle.cancel.cancel();
        self.active_turns.lock().await.remove(session_id);
        self.pending_permissions
            .lock()
            .await
            .retain(|_, pending| pending.session_id != session_id);
        self.resource_registry
            .release_all(&handle.capability_lease)
            .map(|released| released.len())
            .map_err(|error| format!("RESOURCE_RELEASE_FAILED: {error}"))
    }

    /// Window close cannot await Tokio locks. Best effort is explicit and
    /// bounded: cancel the live engine synchronously, then fsync a clean
    /// terminal event only when the handle and initialized ledger are available.
    pub(crate) fn close_active_session_now(&self) -> Result<(), String> {
        let mut guard = self
            .handle
            .try_lock()
            .map_err(|_| "active session is busy during window close".to_string())?;
        let Some(handle) = guard.take() else {
            return Ok(());
        };
        handle.cancel.cancel();
        let Some(Ok(ledger)) = self.execution_ledger.get() else {
            return Ok(());
        };
        let context = EventContext {
            session_id: handle.session_id.0.to_string(),
            surface_kind: handle.surface_kind,
            policy_version: crate::surface::CURRENT_POLICY_VERSION,
            provider_catalog_key: handle.provider_catalog_key,
            turn_id: None,
            step_id: None,
            call_id: None,
            agent_id: Some(handle.capability_lease.agent_id.clone()),
        };
        let released = self
            .resource_registry
            .release_all(&handle.capability_lease)
            .map_err(|error| format!("RESOURCE_RELEASE_FAILED: {error}"))?;
        for (kind, resource_id_hash) in released {
            ledger
                .append(
                    context.clone(),
                    ExecutionEventKind::ResourceReleased {
                        resource_kind: resource_kind_code(kind).to_string(),
                        resource_id_hash,
                    },
                )
                .map_err(|error| format!("EXECUTION_LEDGER_APPEND_FAILED: {error}"))?;
        }
        ledger
            .append(
                context,
                ExecutionEventKind::SessionEnded {
                    reason: SessionEndReason::CleanExit,
                },
            )
            .map(|_| ())
            .map_err(|error| format!("EXECUTION_LEDGER_APPEND_FAILED: {error}"))
    }
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
    /// W2-fe-b：Work 会话的持久工作区身份（来自 binding；非 Work 为 None）。
    /// 前端据此调 work_import 把文档导入本会话所属工作区。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    /// 当前策略规则代号（派生用，见 surface::CURRENT_POLICY_VERSION）。
    pub policy_version: u32,
    /// C2：当前模型的推理强度菜单（来自 `_meta["x.ai/sessionConfig"]` 的
    /// mode 条目）。空 = 当前模型不支持强度选择，前端不得显示选择器。
    pub effort_options: Vec<crate::effort::EffortChoice>,
    /// C2：当前选中的强度档 id（菜单里的 selected 项；引擎未下发为 None）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_effort: Option<String>,
}

#[derive(serde::Serialize, Clone, Default)]
pub struct ModelOption {
    pub id: String,
    pub name: String,
    pub endpoint_label: String,
    /// #127-2：能力 + 归属诊断（聊天目录链适配器产出；前端徽章在 PR 3）。
    pub caps: crate::caps_snapshot::ResolvedModelCaps,
    /// C2：该模型是否支持推理强度（引擎能力位；false 时下面两个字段恒为空）。
    pub supports_effort: bool,
    /// C2：该模型 catalog 声明的强度菜单（空 = 引擎回落 legacy 五档）。
    pub effort_options: Vec<crate::effort::EffortChoice>,
    /// C2：该模型 config.toml 里配置的默认强度档。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
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

/// 端到端**可启动**的层。W2-fe-b:Chat/Code/Work 已全链路打通(Work 有创建
/// 入口 bind_new_work_session + 前端 switcher/视图/导入 + WORK_UI_READY);
/// Cowork 待 Cowork 线。用于 agent_start 在发布 handle 之前 gate——不可启动
/// 的层绝不装 handle(否则留下前端无法显示、agent_cancel 无法拆除的孤儿会话)。
/// 本闸与前端 surface.ts 的 WORK_UI_READY **协同放行**:两处必须同版本一起改。
fn surface_launchable(kind: crate::surface::SurfaceKind) -> bool {
    use crate::surface::SurfaceKind::{Chat, Code, Work};
    matches!(kind, Chat | Code | Work)
}

/// 是否加载配置/继承的 MCP 服务器。**仅 Code**——Chat/Work 默认无联网/MCP
/// 能力面（codex W2-fe-b R1；设计 §1「默认 Work 会话不注入联网/MCP 工具」）。
fn surface_loads_configured_mcp(kind: crate::surface::SurfaceKind) -> bool {
    kind == crate::surface::SurfaceKind::Code
}

fn surface_visible_tools(
    kind: crate::surface::SurfaceKind,
) -> BTreeMap<String, ToolRisk> {
    use crate::surface::SurfaceKind::{Chat, Code, Cowork, Work};

    let entries: &[(&str, ToolRisk)] = match kind {
        Code => &[
            ("read", ToolRisk::ReadOnly),
            ("search", ToolRisk::ReadOnly),
            ("think", ToolRisk::ReadOnly),
            ("fetch", ToolRisk::Network),
            ("edit", ToolRisk::WorkspaceWrite),
            ("delete", ToolRisk::WorkspaceWrite),
            ("move", ToolRisk::WorkspaceWrite),
            ("execute", ToolRisk::Process),
            ("switch_mode", ToolRisk::Privileged),
            ("other", ToolRisk::Privileged),
        ],
        Chat => &[
            ("search", ToolRisk::ReadOnly),
            ("think", ToolRisk::ReadOnly),
            ("fetch", ToolRisk::Network),
        ],
        Work => &[
            ("read", ToolRisk::ReadOnly),
            ("search", ToolRisk::ReadOnly),
            ("think", ToolRisk::ReadOnly),
        ],
        // Cowork is release-gated in both surface_launchable and the broker.
        Cowork => &[],
    };
    entries
        .iter()
        .map(|(name, risk)| ((*name).to_string(), *risk))
        .collect()
}

#[cfg(test)]
mod surface_visible_tools_tests {
    use super::{surface_visible_tools, ToolRisk};
    use crate::surface::SurfaceKind::{Chat, Code, Cowork, Work};
    use std::collections::BTreeMap;

    fn tools(entries: &[(&str, ToolRisk)]) -> BTreeMap<String, ToolRisk> {
        entries
            .iter()
            .map(|(name, risk)| ((*name).to_owned(), *risk))
            .collect()
    }

    #[test]
    fn backend_surface_tool_contract_matches_the_frontend_mirror() {
        assert_eq!(
            surface_visible_tools(Chat),
            tools(&[
                ("search", ToolRisk::ReadOnly),
                ("think", ToolRisk::ReadOnly),
                ("fetch", ToolRisk::Network),
            ])
        );
        assert_eq!(
            surface_visible_tools(Code),
            tools(&[
                ("read", ToolRisk::ReadOnly),
                ("search", ToolRisk::ReadOnly),
                ("think", ToolRisk::ReadOnly),
                ("fetch", ToolRisk::Network),
                ("edit", ToolRisk::WorkspaceWrite),
                ("delete", ToolRisk::WorkspaceWrite),
                ("move", ToolRisk::WorkspaceWrite),
                ("execute", ToolRisk::Process),
                ("switch_mode", ToolRisk::Privileged),
                ("other", ToolRisk::Privileged),
            ])
        );
        assert_eq!(
            surface_visible_tools(Work),
            tools(&[
                ("read", ToolRisk::ReadOnly),
                ("search", ToolRisk::ReadOnly),
                ("think", ToolRisk::ReadOnly),
            ])
        );
        assert_eq!(surface_visible_tools(Cowork), BTreeMap::new());
    }
}

/// Authorization policy for a host-initiated ACP extension.
///
/// This is deliberately three-state: methods either consume a Surface
/// capability, are explicitly safe without one, or are denied.  Unknown
/// methods land in `Denied`, so adding a new extension can never silently
/// bypass the live lease.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExtMethodPolicy {
    Required(&'static str, ToolRisk),
    NoCapability(&'static str),
    Denied(&'static str),
}

const UNKNOWN_EXT_METHOD: &str = "unregistered extension method";

fn ext_method_policy(method: &str) -> ExtMethodPolicy {
    let read = || ExtMethodPolicy::Required("read", ToolRisk::ReadOnly);
    let write = || ExtMethodPolicy::Required("edit", ToolRisk::WorkspaceWrite);
    let privileged = || ExtMethodPolicy::Required("other", ToolRisk::Privileged);
    if method.starts_with("x.ai/terminal/") || method.starts_with("x.ai/task/") {
        return ExtMethodPolicy::Required("execute", ToolRisk::Process);
    }
    if method.starts_with("x.ai/mcp/")
        || method.starts_with("x.ai/subagent/")
        || method.starts_with("x.ai/hooks/")
        || method == "x.ai/scheduler/delete"
        || method == "x.ai/session/update_mcp_servers"
    {
        return privileged();
    }
    if matches!(
        method,
        "x.ai/fs/read_file"
            | "x.ai/fs/list"
            | "x.ai/fs/exists"
            | "x.ai/search/content"
            | "x.ai/git/branches"
            | "x.ai/git/current_commit"
            | "x.ai/git/diffs"
            | "x.ai/git/files"
            | "x.ai/git/git_repo_root"
            | "x.ai/git/info"
            | "x.ai/git/serialize_changes"
            | "x.ai/git/status"
            | "x.ai/git/worktree/list"
            | "x.ai/commands/list"
    ) {
        return read();
    }
    if matches!(
        method,
        "x.ai/fs/write_file"
            | "x.ai/fs/delete_file"
            | "x.ai/git/checkout"
            | "x.ai/git/checkout_commit"
            | "x.ai/git/checkout_session_head"
            | "x.ai/git/commit"
            | "x.ai/git/discard"
            | "x.ai/git/stage"
            | "x.ai/git/stash"
            | "x.ai/git/unstage"
            | "x.ai/git/worktree/apply"
            | "x.ai/git/worktree/remove"
            | "x.ai/rewind/execute"
    ) {
        return write();
    }

    if matches!(
        method,
        "x.ai/search/fuzzy/open"
            | "x.ai/search/fuzzy/change"
            | "x.ai/search/fuzzy/close"
            | "x.ai/skills/list"
            | "x.ai/skills/config"
            | "x.ai/plugins/list"
    ) {
        return read();
    }

    if matches!(
        method,
        "x.ai/git/worktree/resume_session"
            | "x.ai/skills/add"
            | "x.ai/skills/remove"
            | "x.ai/skills/reset"
            | "x.ai/skills/toggle"
            | "x.ai/skills/refresh-baseline"
            | "x.ai/plugins/action"
            | "x.ai/memory/flush"
            | "x.ai/memory/rewrite"
            | "x.ai/internal/reload_models"
    ) {
        return privileged();
    }

    // Explicit zero-capability allowlist.  Each entry carries its reason so a
    // reviewer can distinguish a deliberate protocol decision from omission.
    let no_capability = match method {
        "x.ai/compact_conversation" => Some("rewrites conversation context, not workspace resources"),
        "x.ai/interject" => Some("adds user text to the active turn only"),
        "x.ai/permissions/reset" => Some("only revokes remembered permission grants"),
        "x.ai/prompt_history" => Some("reads the user's local prompt history"),
        "x.ai/queue/clear" => Some("edits only this client's queued prompts"),
        "x.ai/queue/edit" => Some("edits only this client's queued prompts"),
        "x.ai/queue/interject" => Some("promotes only this client's queued prompt"),
        "x.ai/queue/remove" => Some("removes only this client's queued prompt"),
        "x.ai/queue/reorder" => Some("reorders only this client's queued prompts"),
        "x.ai/recap" => Some("returns conversation-derived recap metadata"),
        "x.ai/rewind/points" => Some("lists conversation rewind metadata without applying it"),
        "x.ai/session/close" => Some("closes a user-owned session record"),
        "x.ai/session/delete" => Some("deletes a user-owned session record, not workspace files"),
        "x.ai/session/fork" => Some("copies session history without changing workspace files"),
        "x.ai/session/info" => Some("reads metadata for the active session"),
        "x.ai/session/list" => Some("lists user-owned session metadata"),
        "x.ai/session/load_history" => Some("reads user-owned session history"),
        "x.ai/session/rename" => Some("renames a user-owned session record"),
        "x.ai/session/repair" => Some("repairs the user-owned session ledger"),
        "x.ai/session/search" => Some("searches user-owned session history"),
        "x.ai/session/updates" => Some("reads active-session update metadata"),
        "x.ai/session_summaries/session_list" => Some("lists user-owned session summaries"),
        "x.ai/session_summaries/workspace_list" => Some("lists workspace labels from session history"),
        "x.ai/session_summaries/workspace_list_recent" => Some("lists recent workspace labels from session history"),
        "x.ai/sessions/list" => Some("lists user-owned session metadata"),
        "x.ai/suggest" => Some("returns conversation-derived prompt suggestions"),
        "x.ai/toggle_plan_mode" => Some("narrows or restores the active session mode"),
        "x.ai/workspaces/list" => Some("lists workspace labels from session history"),
        "x.ai/yolo_mode_changed" => Some("syncs UI policy but cannot expand the Surface lease"),
        _ => None,
    };
    if let Some(reason) = no_capability {
        return ExtMethodPolicy::NoCapability(reason);
    }

    // Engine-originated requests/notifications and ACP metadata keys are
    // deliberately not callable through either host extension entrance.
    let denied = match method {
        "x.ai/ask_user_question" => Some("engine-to-client request"),
        "x.ai/debug" => Some("engine-to-client diagnostic request"),
        "x.ai/exit_plan_mode" => Some("engine-to-client request"),
        "x.ai/folder_trust/request" => Some("engine-to-client trust request"),
        "x.ai/folderTrust" => Some("ACP capability metadata key"),
        "x.ai/localExtensionsDisabled" => Some("ACP capability metadata key"),
        "x.ai/modelBlock" => Some("ACP session metadata key"),
        "x.ai/rewind" => Some("engine-to-client rewind notification"),
        "x.ai/session_notification" => Some("engine-to-client session notification"),
        "x.ai/sessionConfig" => Some("ACP session metadata key"),
        _ => None,
    };
    ExtMethodPolicy::Denied(denied.unwrap_or(UNKNOWN_EXT_METHOD))
}

pub(crate) fn provider_profile_for_catalog_key(
    provider_catalog_key: Option<&str>,
) -> Result<ProviderProfile, String> {
    let key = provider_catalog_key.unwrap_or("provider-route-unavailable");
    ProviderProfile::safe_default(key, infer_family(key))
        .map_err(|error| format!("PROVIDER_PROFILE_BLOCKED: {error}"))
}

pub(crate) const SESSION_NOT_STARTED_ERROR: &str = "SESSION_NOT_STARTED: 会话未启动";

fn ensure_execution_integrity(diagnostics: &LedgerDiagnostics) -> Result<(), String> {
    if diagnostics.duplicate_event_ids.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "EXECUTION_INTEGRITY_BLOCKED: duplicate event ids: {}",
            diagnostics
                .duplicate_event_ids
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(",")
        ))
    }
}

fn issue_session_capability_lease(
    session_id: &str,
    surface_kind: crate::surface::SurfaceKind,
    cwd: &std::path::Path,
    work_workspace_id: Option<&crate::work_staging::WorkspaceId>,
    provider_catalog_key: Option<&str>,
    model_options: &[ModelOption],
) -> Result<CapabilityLease> {
    let provider_key = provider_catalog_key.unwrap_or("provider-route-unavailable");
    let model_caps = provider_catalog_key
        .and_then(|key| model_options.iter().find(|option| option.id == key))
        .and_then(|option| serde_json::to_vec(&option.caps).ok())
        .unwrap_or_else(|| b"model-caps-unavailable".to_vec());
    let workspace_identity = work_workspace_id
        .map(|id| id.as_str().as_bytes().to_vec())
        .unwrap_or_else(|| cwd.to_string_lossy().as_bytes().to_vec());
    let readable_roots = match surface_kind {
        crate::surface::SurfaceKind::Code | crate::surface::SurfaceKind::Work => {
            vec![cwd.to_path_buf()]
        }
        crate::surface::SurfaceKind::Chat | crate::surface::SurfaceKind::Cowork => Vec::new(),
    };
    let writable_roots = if surface_kind == crate::surface::SurfaceKind::Code {
        vec![cwd.to_path_buf()]
    } else {
        Vec::new()
    };

    CapabilityLease::issue_root(LeaseRequest {
        session_id: session_id.to_string(),
        surface_kind,
        agent_id: "main".to_string(),
        parent_agent_id: None,
        workspace_id_hash: hex_sha256(&workspace_identity),
        provider_route_hash: hex_sha256(provider_key.as_bytes()),
        model_caps_hash: hex_sha256(&model_caps),
        visible_tools: surface_visible_tools(surface_kind),
        readable_roots,
        writable_roots,
        denied_roots: Vec::new(),
        mcp_inheritance: if surface_loads_configured_mcp(surface_kind) {
            McpInheritance::All
        } else {
            McpInheritance::None
        },
        mcp_names: BTreeSet::new(),
        policy_version: crate::surface::CURRENT_POLICY_VERSION,
    })
    .map_err(|error| anyhow!("CAPABILITY_LEASE_BLOCKED: {error}"))
}

/// 该层是否**要求引擎确认已应用**本地扩展隔离（`localExtensionsDisabled`）。
///
/// 请求该策略与**验证它被应用**是两回事（codex W2-fe-b R3）：curated profile
/// 与空 ACP `mcp_servers` 并不能独立压制插件/managed MCP 来源——真正压制它们
/// 的是引擎应用了该策略。因此凡是宣称「零 MCP/零本地扩展」的层（Chat、Work），
/// 都必须在**新建与恢复**两条路径上要求 `localExtensionsDisabledApplied`，
/// 否则引擎漂移/旧引擎/忽略该请求的路径会让边界 fail-open。Code 无此要求
/// （它本就允许配置 MCP 与本地扩展）。
fn surface_requires_local_extension_isolation(kind: crate::surface::SurfaceKind) -> bool {
    use crate::surface::SurfaceKind::{Chat, Work};
    matches!(kind, Chat | Work)
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
    work_workspace_id: Option<String>,
) -> Result<StartResult, String> {
    // smoke 模式：前端不许动会话。debug 构建的 webview 若碰到活着的 dev
    // server 会加载完整前端并自动启动会话，把 autotest 的 handle 换成
    // localStorage 工作区（宿主仓库！）——run3 的 stash 事故 + S2/S4 全部
    // 抖动皆源于此。autotest 走 start_inner 内部路径，不经过这里。
    if std::env::var("WANCODE_AUTOTEST").is_ok() {
        return Err("AUTOTEST 模式：前端会话启动被禁用".into());
    }
    let intent = crate::surface_policy::NewSurfaceIntent::from_wire(surface.as_deref())
        .map_err(|e| crate::surface_policy::policy_blocked_message(&e))?;
    let requested_work_workspace = work_workspace_id
        .map(crate::work_staging::WorkspaceId::parse)
        .transpose()
        .map_err(|e| e.to_string())?;
    if requested_work_workspace.is_some()
        && intent.surface_kind() != crate::surface::SurfaceKind::Work
    {
        return Err("work_workspace_id 只能用于 Work 层".into());
    }
    let result = start_inner_with_intent_and_workspace(
        app,
        &state,
        workspace,
        model,
        resume,
        intent,
        requested_work_workspace,
    )
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
    start_inner_with_intent_and_workspace(
        app,
        state,
        workspace,
        model,
        resume,
        new_intent,
        None,
    )
    .await
}

fn select_work_workspace(
    bound: Option<&crate::work_staging::WorkspaceId>,
    requested: Option<crate::work_staging::WorkspaceId>,
) -> Result<crate::work_staging::WorkspaceId> {
    match bound {
        Some(bound) => {
            if let Some(requested) = requested.as_ref() {
                if requested != bound {
                    return Err(anyhow!(
                        "WORKSPACE_IDENTITY_CONFLICT: 恢复会话绑定 {}，请求却为 {}",
                        bound.as_str(),
                        requested.as_str()
                    ));
                }
            }
            Ok(bound.clone())
        }
        None => Ok(requested.unwrap_or_else(crate::work_staging::WorkspaceId::mint)),
    }
}

async fn start_inner_with_intent_and_workspace(
    app: AppHandle,
    state: &State<'_, AgentState>,
    workspace: String,
    model: Option<String>,
    resume: Option<String>,
    new_intent: crate::surface_policy::NewSurfaceIntent,
    requested_work_workspace: Option<crate::work_staging::WorkspaceId>,
) -> Result<StartResult> {
    let was_resumed = resume.is_some();
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
    // Ledger integrity is an execution prerequisite. Open it before spawning
    // the engine so a corrupt completed record cannot leave an unaudited live
    // process or session behind.
    let execution_ledger = state
        .execution_ledger(&app)
        .map_err(|error| anyhow!(error))?;
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
    let is_work = surface_kind == crate::surface::SurfaceKind::Work;
    // W2-fe-b:Work 会话的 workspace_id 需在 cwd 之前确定——Work 的 cwd 就是
    // 该工作区的暂存目录(app_data_dir/work/<workspace_id>)。resumed Work 用
    // 绑定里的 id(不变量保证 Some);fresh Work 现铸造(下面写入 Work 绑定,
    // 同一 id 复用,不重复铸造)。
    let work_workspace_id: Option<crate::work_staging::WorkspaceId> = if is_work {
        let bound = resumed_binding
            .as_ref()
            .map(|b| {
                b.workspace_id
                    .as_ref()
                    .ok_or_else(|| anyhow!("Work 绑定缺 workspace_id（身份不变量被破坏）"))
            })
            .transpose()?;
        Some(select_work_workspace(bound, requested_work_workspace)?)
    } else {
        None
    };
    let cwd = if is_chat {
        // 路径必须经 resolve_chat_runtime_dir 单一来源（PR #38 F2）。
        let path = resolve_chat_runtime_dir(&app).map_err(|e| anyhow!(e))?;
        std::fs::create_dir_all(&path)
            .with_context(|| format!("创建 Chat 私有运行目录失败: {}", path.display()))?;
        path
    } else if is_work {
        // Work cwd = 该工作区暂存目录(原件只读、DocReadOnly,不碰用户项目)。
        let app_data = app
            .path()
            .app_data_dir()
            .map_err(|e| anyhow!("解析 app_data_dir 失败: {e}"))?;
        let ws = work_workspace_id
            .as_ref()
            .expect("is_work 分支必有 workspace_id");
        let path = crate::work_staging::workspace_dir_under(app_data, ws);
        std::fs::create_dir_all(&path)
            .with_context(|| format!("创建 Work 暂存目录失败: {}", path.display()))?;
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
    // 在那个状态下 stash/丢弃会打错目标）。失败宁可 `SESSION_NOT_STARTED:`。
    if let Some(old) = state.handle.lock().await.take() {
        old.cancel.cancel();
        let pending = std::mem::take(&mut *state.pending_permissions.lock().await);
        for (_, receipt) in pending {
            let _ = receipt.sender.send(None);
        }
        let context = EventContext {
            session_id: old.session_id.0.to_string(),
            surface_kind: old.surface_kind,
            policy_version: crate::surface::CURRENT_POLICY_VERSION,
            provider_catalog_key: old.provider_catalog_key,
            turn_id: None,
            step_id: None,
            call_id: None,
            agent_id: Some(old.capability_lease.agent_id.clone()),
        };
        for (kind, resource_id_hash) in state
            .resource_registry
            .release_all(&old.capability_lease)
            .map_err(|error| anyhow!("RESOURCE_RELEASE_FAILED: {error}"))?
        {
            execution_ledger
                .append(
                    context.clone(),
                    ExecutionEventKind::ResourceReleased {
                        resource_kind: resource_kind_code(kind).to_string(),
                        resource_id_hash,
                    },
                )
                .map_err(|error| anyhow!("EXECUTION_LEDGER_APPEND_FAILED: {error}"))?;
        }
        execution_ledger
            .append(
                context,
                ExecutionEventKind::SessionEnded {
                    reason: SessionEndReason::Cancelled,
                },
            )
            .map_err(|error| anyhow!("EXECUTION_LEDGER_APPEND_FAILED: {error}"))?;
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
    // codex W2-fe-b R1:Work 也要落 AgentConfig 覆盖（关 managed MCP）与禁
    // 自动模式——否则 Work 会继承 Code 的配置档。工具/MCP 主防线是上面的
    // work_agent_profile（schema 缺席）+ mcp_servers 置空。
    if is_work {
        crate::surface_policy::apply_work_agent_config_overrides(&mut agent_config);
        agent_config.default_auto_mode = false;
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
    } else if is_work {
        // Work cwd 是暂存目录，非用户项目——跳过 git 状态/项目布局注入。
        let mut hints = crate::surface_policy::work_startup_hints();
        hints.as_object_mut().expect("static Work hints")
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
    let init_resp: acp::InitializeResponse =
        match bounded_acp_request(acp_send(init_req, &acp_tx), SESSION_HANDSHAKE_TIMEOUT).await {
            BoundedAcpOutcome::Completed(resp) => resp,
            BoundedAcpOutcome::Failed(error) => {
                cancel.cancel();
                return Err(anyhow!("ACP initialize 失败: {error}"));
            }
            BoundedAcpOutcome::TimedOut => {
                cancel.cancel();
                return Err(anyhow!(
                    "SESSION_HANDSHAKE_TIMEOUT: ACP initialize 在 {} 秒内未完成，请重试",
                    SESSION_HANDSHAKE_TIMEOUT.as_secs()
                ));
            }
        };

    // ── Authenticate (non-interactive methods only) ─────────────────
    let method_id = init_resp
        .auth_methods
        .iter()
        .find(|m| !AuthMethodKind::from_id(m.id()).needs_interactive_login())
        .map(|m| m.id().clone())
        .context("没有可用的非交互认证方式（请在 ~/.grok/config.toml 配置模型 API Key）")?;
    let auth_request = acp_send(
        acp::AuthenticateRequest::new(method_id)
            .meta(serde_json::json!({"headless": true}).as_object().cloned()),
        &acp_tx,
    );
    match bounded_acp_request(auth_request, SESSION_HANDSHAKE_TIMEOUT).await {
        BoundedAcpOutcome::Completed(_response) => {}
        BoundedAcpOutcome::Failed(error) => {
            cancel.cancel();
            return Err(anyhow!("认证失败: {error}"));
        }
        BoundedAcpOutcome::TimedOut => {
            cancel.cancel();
            return Err(anyhow!(
                "SESSION_HANDSHAKE_TIMEOUT: 认证在 {} 秒内未完成，请重试",
                SESSION_HANDSHAKE_TIMEOUT.as_secs()
            ));
        }
    }

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
                        let Some(msg) = msg else {
                            on_engine_channel_closed(&app, &pump_cancel).await;
                            break;
                        };
                        handle_acp_message(&app, msg).await;
                    }
                }
            }
        });
    }

    // ── Open session (new or resume-with-replay) ───────────────────
    // codex W2-fe-b R1:Work 与 Chat 同样**零配置/继承 MCP**（默认 Work 无
    // 联网/MCP 能力面）。只有 Code 加载配置 MCP。
    let mcp_servers = if surface_loads_configured_mcp(surface_kind) {
        xai_grok_shell::util::config::load_mcp_servers(
            &cwd,
            &xai_grok_tools::types::compat::CompatConfig::default(),
        )
    } else {
        Vec::new()
    };
    // 会话级 agentProfile：Chat / Work 各自的受限档；Code 无（继承默认）。
    let session_meta = if is_chat {
        Some(
            serde_json::json!({
                "agentProfile": crate::surface_policy::chat_agent_profile(),
                "x.ai/localExtensionsDisabled": true,
            })
            .as_object()
            .cloned()
            .expect("static Chat session meta"),
        )
    } else if is_work {
        Some(
            serde_json::json!({
                "agentProfile": crate::surface_policy::work_agent_profile(),
                "x.ai/localExtensionsDisabled": true,
            })
            .as_object()
            .cloned()
            .expect("static Work session meta"),
        )
    } else {
        None
    };
    let mut model_block: Option<serde_json::Value> = None;
    // C2：两条路径（新建/恢复）都带 `x.ai/sessionConfig` meta——强度菜单与
    // 当前档在会话打开时下发，热切换后的菜单由前端按 ModelOption 能力位推导。
    let (session_id, session_models, session_config_meta) = if let Some(sid) = resume {
        let mut req = acp::LoadSessionRequest::new(acp::SessionId::new(sid.clone()), cwd.clone())
            .mcp_servers(mcp_servers);
        if let Some(meta) = session_meta.clone() {
            req = req.meta(Some(meta));
        }
        let resp: acp::LoadSessionResponse =
            match bounded_acp_request(acp_send(req, &acp_tx), SESSION_OPEN_TIMEOUT).await {
                BoundedAcpOutcome::Completed(resp) => resp,
                BoundedAcpOutcome::Failed(error) => {
                    cancel.cancel();
                    return Err(anyhow!("恢复会话失败: {error}"));
                }
                BoundedAcpOutcome::TimedOut => {
                    cancel.cancel();
                    return Err(anyhow!(
                        "SESSION_START_TIMEOUT: 引擎在 {} 秒内未完成会话恢复，请重试",
                        SESSION_OPEN_TIMEOUT.as_secs()
                    ));
                }
            };
        model_block = resp
            .meta
            .as_ref()
            .and_then(|m| m.get("x.ai/modelBlock"))
            .cloned();
        if surface_requires_local_extension_isolation(surface_kind)
            && !local_extensions_policy_applied(resp.meta.as_ref())
        {
            cancel.cancel();
            return Err(anyhow!("{}", crate::surface_policy::policy_blocked_message(
                &crate::surface_policy::SurfacePolicyError::LocalExtensionsPolicyNotApplied)));
        }
        (acp::SessionId::new(sid), resp.models, resp.meta)
    } else {
        let mut req = acp::NewSessionRequest::new(cwd.clone()).mcp_servers(mcp_servers);
        if let Some(meta) = session_meta {
            req = req.meta(Some(meta));
        }
        let resp: acp::NewSessionResponse =
            match bounded_acp_request(acp_send(req, &acp_tx), SESSION_OPEN_TIMEOUT).await {
                BoundedAcpOutcome::Completed(resp) => resp,
                BoundedAcpOutcome::Failed(error) => {
                    cancel.cancel();
                    return Err(anyhow!("创建会话失败: {error}"));
                }
                BoundedAcpOutcome::TimedOut => {
                    cancel.cancel();
                    return Err(anyhow!(
                        "SESSION_START_TIMEOUT: 引擎在 {} 秒内未完成会话创建，请重试",
                        SESSION_OPEN_TIMEOUT.as_secs()
                    ));
                }
            };
        if surface_requires_local_extension_isolation(surface_kind)
            && !local_extensions_policy_applied(resp.meta.as_ref())
        {
            cancel.cancel();
            return Err(anyhow!("{}", crate::surface_policy::policy_blocked_message(
                &crate::surface_policy::SurfacePolicyError::LocalExtensionsPolicyNotApplied)));
        }
        (resp.session_id, resp.models, resp.meta)
    };
    // ── v0.19-2a 最低身份事务链：引擎返回 ID → 写 binding → 成功后才
    // 安装 handle/返回前端。写失败即取消本次 Agent——绝不暴露可发送的
    // handle；引擎可能留下孤立会话，恢复时会被 unbound_surface 拦住，
    // 走显式恢复/认领，不会静默升 Code。
    let surface_binding = match resumed_binding {
        Some(b) => b,
        None => {
            let surface = app.state::<crate::surface_gate::SurfaceState>();
            // W2-fe-b:新 Work 会话写 Work 绑定,复用上面(cwd 之前)铸造的
            // **同一** workspace_id(身份不变量要求 Work⟺workspace_id;
            // bind_new_session 拒 Work)。workspace_id 随绑定持久化,经
            // StartResult.workspace_id 回传前端用于导入。
            let bound = if is_work {
                let ws = work_workspace_id
                    .clone()
                    .expect("is_work 分支必有 workspace_id");
                surface.bind_new_work_session(&session_id.0, ws)
            } else {
                surface.bind_new_session(&session_id.0, surface_kind)
            };
            match bound {
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
                        let (supports_effort, effort_options, default_effort) =
                            crate::effort::parse_model_effort_meta(am.meta.as_ref());
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
                            supports_effort,
                            effort_options,
                            default_effort,
                        }
                    })
                    .collect(),
            )
        })
        .unwrap_or_default();

    let provider_profile = provider_profile_for_catalog_key(current_model_id.as_deref())
        .inspect_err(|_| cancel.cancel())
        .map_err(anyhow::Error::msg)?;
    let capability_lease = Arc::new(
        issue_session_capability_lease(
            session_id.0.as_ref(),
            surface_binding.surface_kind,
            &cwd,
            surface_binding.workspace_id.as_ref(),
            current_model_id.as_deref(),
            &model_options,
        )
        .inspect_err(|_error| {
            cancel.cancel();
        })?,
    );

    // Binding + crash marker + ledger are one publication boundary: only after
    // the immutable lease and all durable records succeed may a send-capable
    // handle reach the UI.
    let ledger_context = EventContext {
        session_id: session_id.0.to_string(),
        surface_kind: surface_binding.surface_kind,
        policy_version: crate::surface::CURRENT_POLICY_VERSION,
        provider_catalog_key: current_model_id.clone(),
        turn_id: None,
        step_id: None,
        call_id: None,
        agent_id: Some(capability_lease.agent_id.clone()),
    };
    let workspace_fingerprint = Some(hex_sha256(cwd.to_string_lossy().as_bytes()));
    for event in [
        ExecutionEventKind::SessionStarted {
            resumed: was_resumed,
            workspace_fingerprint,
        },
        ExecutionEventKind::SurfaceBound,
        ExecutionEventKind::PolicyApplied,
    ] {
        if let Err(error) = execution_ledger.append(ledger_context.clone(), event) {
            cancel.cancel();
            return Err(anyhow!("EXECUTION_LEDGER_APPEND_FAILED: {error}"));
        }
    }

    *state.handle.lock().await = Some(AgentHandle {
        acp_tx: acp_tx.clone(),
        session_id: session_id.clone(),
        cancel,
        cwd: cwd.clone(),
        surface_kind: surface_binding.surface_kind,
        work_workspace_id: surface_binding.workspace_id.clone(),
        provider_catalog_key: current_model_id.clone(),
        provider_profile,
        capability_lease,
    });

    // 新会话的技能来自 agent 启动时的内存快照（self.cfg.skills），运行期改
    // 的 [skills].disabled 它看不见——引擎没有任何回灌路径。刷新只是
    // best-effort 的后置维护，绝不能卡住已经完成的会话发布：真实 Windows
    // 运行曾在会话已落盘、MCP 全健康后永久停在 UI "Starting…"，因为这里
    // 同步等待一个未回包的 ext 请求。后台任务自身仍有硬超时，避免泄漏。
    if !is_chat {
        schedule_skill_baseline_refresh(acp_tx.clone());
    }

    let (mut effort_options, current_effort) =
        crate::effort::parse_session_config_effort(session_config_meta.as_ref());
    // sessionConfig 的 mode 条目只有展示 id；自定义菜单真正发往引擎的
    // canonical value 在当前 ModelInfo.meta.reasoningEfforts。按 id 合并，
    // 否则 `deep -> xhigh` 会错误发送 deep 并被引擎静默忽略。
    if let Some(current_model) = current_model_id.as_deref().and_then(|id| {
        model_options.iter().find(|option| option.id == id)
    }) {
        crate::effort::reconcile_session_effort_values(
            &mut effort_options,
            &current_model.effort_options,
        );
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
        workspace_id: surface_binding
            .workspace_id
            .as_ref()
            .map(|w| w.as_str().to_string()),
        policy_version: crate::surface::derive_effective_policy(surface_binding.surface_kind)
            .policy_version,
        effort_options,
        current_effort,
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
    use super::{
        ext_method_policy, issue_session_capability_lease, surface_launchable, ExtMethodPolicy,
        UNKNOWN_EXT_METHOD,
        surface_loads_configured_mcp, surface_requires_local_extension_isolation,
        terminal_id_from_response, terminal_resource_action, retain_owned_terminals,
        validate_pending_permission, PendingPermission, TerminalResourceAction,
    };
    use crate::capability_broker::{McpInheritance, ResourceKind, ResourceRegistry, ToolRisk};
    use crate::surface::SurfaceKind;
    use std::collections::BTreeSet;

    #[test]
    fn chat_code_work_launchable_cowork_gated() {
        // W2-fe-b:Work 端到端打通 → 可启动。Cowork 线未落地 → agent_start
        // 仍在装 handle 前拦截,绝不发布孤儿 handle。
        assert!(surface_launchable(SurfaceKind::Chat));
        assert!(surface_launchable(SurfaceKind::Code));
        assert!(surface_launchable(SurfaceKind::Work));
        assert!(!surface_launchable(SurfaceKind::Cowork));
    }

    // codex W2-fe-b R3:宣称零 MCP/零本地扩展的层(Chat、Work)必须在新建与
    // 恢复两条路径上**要求引擎确认**已应用隔离;缺失/false 一律 fail-closed。
    // Code 是正对照(不受限,不要求确认)。
    #[test]
    fn work_and_chat_require_local_extension_ack_code_does_not() {
        use super::local_extensions_policy_applied;
        assert!(surface_requires_local_extension_isolation(SurfaceKind::Work));
        assert!(surface_requires_local_extension_isolation(SurfaceKind::Chat));
        assert!(!surface_requires_local_extension_isolation(SurfaceKind::Code));

        // 组合判定 = 门的真实逻辑:要求隔离 且 未确认 → 阻塞。
        let blocks = |kind, meta: Option<serde_json::Value>| {
            let map = meta.map(|v| v.as_object().cloned().expect("object meta"));
            surface_requires_local_extension_isolation(kind)
                && !local_extensions_policy_applied(map.as_ref())
        };
        // Work:确认缺失 / 显式 false → 阻塞;true → 放行。
        assert!(blocks(SurfaceKind::Work, None), "Work 缺确认必须阻塞");
        assert!(
            blocks(SurfaceKind::Work, Some(serde_json::json!({}))),
            "Work 元数据无该字段必须阻塞"
        );
        assert!(
            blocks(
                SurfaceKind::Work,
                Some(serde_json::json!({"localExtensionsDisabledApplied": false}))
            ),
            "Work 显式 false 必须阻塞"
        );
        assert!(
            !blocks(
                SurfaceKind::Work,
                Some(serde_json::json!({"localExtensionsDisabledApplied": true}))
            ),
            "Work 已确认应放行"
        );
        // Code 正对照:无论确认与否都不因此阻塞。
        assert!(!blocks(SurfaceKind::Code, None));
        assert!(!blocks(
            SurfaceKind::Code,
            Some(serde_json::json!({"localExtensionsDisabledApplied": false}))
        ));
    }

    #[test]
    fn only_code_loads_configured_mcp() {
        // codex W2-fe-b R1:默认 Work 无 MCP 能力面(与 Chat 同);正对照 Code
        // 仍加载配置 MCP。
        assert!(surface_loads_configured_mcp(SurfaceKind::Code));
        assert!(!surface_loads_configured_mcp(SurfaceKind::Work));
        assert!(!surface_loads_configured_mcp(SurfaceKind::Chat));
        assert!(!surface_loads_configured_mcp(SurfaceKind::Cowork));
    }

    #[test]
    fn extension_resource_commands_cannot_bypass_surface_capabilities() {
        assert_eq!(
            ext_method_policy("x.ai/terminal/pty/create"),
            ExtMethodPolicy::Required("execute", ToolRisk::Process)
        );
        assert_eq!(
            ext_method_policy("x.ai/mcp/toggle_tool"),
            ExtMethodPolicy::Required("other", ToolRisk::Privileged)
        );
        assert_eq!(
            ext_method_policy("x.ai/fs/read_file"),
            ExtMethodPolicy::Required("read", ToolRisk::ReadOnly)
        );
        assert_eq!(
            ext_method_policy("x.ai/commands/list"),
            ExtMethodPolicy::Required("read", ToolRisk::ReadOnly)
        );
        assert_eq!(
            ext_method_policy("x.ai/git/stage"),
            ExtMethodPolicy::Required("edit", ToolRisk::WorkspaceWrite)
        );
        assert!(matches!(
            ext_method_policy("x.ai/session/info"),
            ExtMethodPolicy::NoCapability(_)
        ));
        assert_eq!(
            ext_method_policy(concat!("x.ai/", "not-registered")),
            ExtMethodPolicy::Denied(UNKNOWN_EXT_METHOD)
        );

        let root = tempfile::tempdir().unwrap();
        let code = issue_session_capability_lease(
            "code-extension",
            SurfaceKind::Code,
            root.path(),
            None,
            Some("deepseek:chat"),
            &[],
        )
        .unwrap();
        let work = issue_session_capability_lease(
            "work-extension",
            SurfaceKind::Work,
            root.path(),
            None,
            Some("glm:work"),
            &[],
        )
        .unwrap();
        let ExtMethodPolicy::Required(tool, risk) = ext_method_policy("x.ai/terminal/create") else {
            panic!("terminal create must require a capability")
        };
        assert!(code.authorize_tool(tool, risk).is_ok());
        assert!(work.authorize_tool(tool, risk).is_err());
        let ExtMethodPolicy::Required(tool, risk) = ext_method_policy("x.ai/fs/read_file") else {
            panic!("filesystem read must require a capability")
        };
        assert!(work.authorize_tool(tool, risk).is_ok());
    }

    #[test]
    fn every_extension_literal_in_rust_sources_has_an_explicit_policy() {
        fn rust_files_below(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).expect("read Rust source directory") {
                let path = entry.expect("read source entry").path();
                if path.is_dir() {
                    rust_files_below(&path, out);
                } else if path.extension().and_then(|value| value.to_str()) == Some("rs") {
                    out.push(path);
                }
            }
        }

        fn extension_literals(source: &str, out: &mut BTreeSet<String>) {
            let mut rest = source;
            while let Some(start) = rest.find("\"x.ai/") {
                rest = &rest[start + 1..];
                let Some(end) = rest.find('"') else { break };
                let candidate = &rest[..end];
                if candidate
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._/-".contains(&byte))
                {
                    out.insert(candidate.to_owned());
                }
                rest = &rest[end + 1..];
            }
        }

        let mut files = Vec::new();
        rust_files_below(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut files,
        );
        let mut methods = BTreeSet::new();
        for file in files {
            extension_literals(
                &std::fs::read_to_string(&file).expect("read Rust source"),
                &mut methods,
            );
        }
        // These are match prefixes/metadata namespaces, not callable methods.
        for prefix in [
            "x.ai/",
            "x.ai/fs/",
            "x.ai/git/",
            "x.ai/git/worktree",
            "x.ai/hooks/",
            "x.ai/mcp/",
            "x.ai/subagent/",
            "x.ai/task/",
            "x.ai/terminal/",
        ] {
            methods.remove(prefix);
        }
        let unknown: Vec<_> = methods
            .iter()
            .filter(|method| {
                ext_method_policy(method) == ExtMethodPolicy::Denied(UNKNOWN_EXT_METHOD)
            })
            .cloned()
            .collect();
        assert!(
            unknown.is_empty(),
            "new x.ai method literals need an explicit Required/NoCapability/Denied policy: {unknown:?}"
        );

        for method in methods {
            match ext_method_policy(&method) {
                ExtMethodPolicy::NoCapability(reason) | ExtMethodPolicy::Denied(reason) => {
                    assert!(!reason.trim().is_empty(), "{method} needs a reviewable rationale");
                }
                ExtMethodPolicy::Required(_, _) => {}
            }
        }
    }

    #[test]
    fn request_and_notification_entrances_share_one_authorization_preflight() {
        let source = include_str!("agent.rs");
        let call = concat!("authorize_ext_method(state, method, &params)", ".await?");
        assert_eq!(
            source.matches(call).count(),
            2,
            "ext_call and ext_notify must both use the shared preflight"
        );
    }

    #[test]
    fn terminal_handles_are_classified_and_filtered_by_lease_owner() {
        assert_eq!(terminal_resource_action("x.ai/terminal/pty/create"), TerminalResourceAction::Create);
        assert_eq!(terminal_resource_action("x.ai/terminal/pty/input"), TerminalResourceAction::Use);
        assert_eq!(terminal_resource_action("x.ai/terminal/kill"), TerminalResourceAction::Release);

        let root = tempfile::tempdir().unwrap();
        let owner = issue_session_capability_lease(
            "terminal-owner", SurfaceKind::Code, root.path(), None,
            Some("deepseek:chat"), &[],
        ).unwrap();
        let sibling = issue_session_capability_lease(
            "terminal-sibling", SurfaceKind::Code, root.path(), None,
            Some("deepseek:chat"), &[],
        ).unwrap();
        let registry = ResourceRegistry::default();
        registry.register(&owner, ResourceKind::Terminal, "terminal-owned").unwrap();
        registry.register(&sibling, ResourceKind::Terminal, "terminal-sibling").unwrap();

        let mut response = serde_json::json!({
            "result": {"terminals": [
                {"terminalId": "terminal-owned"},
                {"terminalId": "terminal-sibling"},
                {"terminalId": "terminal-unregistered"}
            ]},
            "error": null
        });
        retain_owned_terminals(&mut response, &registry, &owner);
        assert_eq!(response["result"]["terminals"], serde_json::json!([
            {"terminalId": "terminal-owned"}
        ]));
        assert_eq!(
            terminal_id_from_response(&serde_json::json!({
                "result": {"terminalId": "terminal-new"}
            })),
            Some("terminal-new")
        );
    }

    #[test]
    fn approval_receipt_is_single_binding_and_option_scoped() {
        let (sender, _receiver) = tokio::sync::oneshot::channel();
        let pending = PendingPermission {
            sender,
            session_id: "session-a".into(),
            lease_id: "lease-a".into(),
            call_id: "call-a".into(),
            action_fingerprint: "f".repeat(64),
            option_ids: BTreeSet::from(["allow-once".into(), "deny".into()]),
        };
        assert_eq!(
            validate_pending_permission(
                &pending,
                "session-a",
                "lease-a",
                Some("allow-once"),
            ),
            Ok(())
        );
        assert_eq!(
            validate_pending_permission(&pending, "session-b", "lease-a", Some("allow-once")),
            Err("stale_receipt")
        );
        assert_eq!(
            validate_pending_permission(&pending, "session-a", "lease-b", Some("allow-once")),
            Err("stale_receipt")
        );
        assert_eq!(
            validate_pending_permission(&pending, "session-a", "lease-a", Some("forged")),
            Err("invalid_option")
        );
    }

    #[test]
    fn session_lease_matches_surface_roots_and_mcp_boundary() {
        let root = tempfile::tempdir().unwrap();
        let code = issue_session_capability_lease(
            "code-session",
            SurfaceKind::Code,
            root.path(),
            None,
            Some("deepseek:chat"),
            &[],
        )
        .unwrap();
        assert_eq!(code.readable_roots, code.writable_roots);
        assert_eq!(code.mcp_inheritance, McpInheritance::All);
        assert!(code.visible_tools.contains_key("execute"));

        let work = issue_session_capability_lease(
            "work-session",
            SurfaceKind::Work,
            root.path(),
            None,
            Some("glm:work"),
            &[],
        )
        .unwrap();
        assert!(!work.readable_roots.is_empty());
        assert!(work.writable_roots.is_empty());
        assert_eq!(work.mcp_inheritance, McpInheritance::None);
        assert!(!work.visible_tools.contains_key("execute"));

        let chat = issue_session_capability_lease(
            "chat-session",
            SurfaceKind::Chat,
            root.path(),
            None,
            Some("glm:chat"),
            &[],
        )
        .unwrap();
        assert!(chat.readable_roots.is_empty());
        assert!(chat.writable_roots.is_empty());
        assert_eq!(chat.mcp_inheritance, McpInheritance::None);

        assert!(
            issue_session_capability_lease(
                "cowork-session",
                SurfaceKind::Cowork,
                root.path(),
                None,
                Some("glm:cowork"),
                &[],
            )
            .is_err()
        );
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

#[cfg(test)]
mod map_acp_send_error_tests {
    use super::{acp, map_acp_send_error};
    use xai_acp_lib::{AcpAgentMessage, acp_send};
    fn ext_request() -> acp::ExtRequest {
        acp::ExtRequest::new(
            "x.ai/git/worktree/list",
            serde_json::value::to_raw_value(&serde_json::json!({}))
                .unwrap()
                .into(),
        )
    }
    // 正例：receiver 先 drop（引擎线程已退出）→ send_failed。
    // 输入不用手造错误，而是走真实 acp_send 失败路径——与线上弹窗同源。
    #[tokio::test]
    async fn send_failure_gets_engine_dead_prefix() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<AcpAgentMessage>();
        drop(rx);
        let err = acp_send(ext_request(), &tx).await.unwrap_err();
        let mapped = map_acp_send_error(&err);
        assert!(
            mapped.starts_with("ENGINE_DEAD: "),
            "channel failure must carry the structured prefix, got: {mapped}"
        );
        assert!(
            mapped.contains("channel closed"),
            "raw diagnostics must survive the mapping, got: {mapped}"
        );
    }
    // 负例 1：普通内部错误（无 data 判别符）必须原样透传——前缀只属于
    // 引擎死亡，不属于一切错误。
    #[test]
    fn plain_internal_error_passes_through_untouched() {
        let err = xai_acp_lib::acp_internal_error("boom");
        let mapped = map_acp_send_error(&err);
        assert!(!mapped.contains("ENGINE_DEAD"), "got: {mapped}");
        assert!(mapped.contains("boom"), "got: {mapped}");
        assert_eq!(mapped, err.to_string());
    }
    // 负例 2：带**其它** data 的错误不算通道死亡——证明触发条件是
    // xaiAcpChannelFailure 判别符本身，不是「有 data 就前缀」。
    #[test]
    fn foreign_error_data_is_not_treated_as_channel_failure() {
        let err = xai_acp_lib::acp_internal_error("invalid session id")
            .data(serde_json::json!({ "requestId": "abc" }));
        let mapped = map_acp_send_error(&err);
        assert!(!mapped.contains("ENGINE_DEAD"), "got: {mapped}");
        assert_eq!(mapped, err.to_string());
    }
}
#[cfg(test)]
mod clear_dead_handle_tests {
    use super::{
        AgentHandle, AgentState, PendingPermission, ResourceKind, clear_dead_handle,
        issue_session_capability_lease, provider_profile_for_catalog_key,
    };
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;
    async fn state_with_handle(cancel: CancellationToken) -> (AgentState, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let capability_lease = Arc::new(
            issue_session_capability_lease(
                "s1",
                crate::surface::SurfaceKind::Code,
                dir.path(),
                None,
                Some("deepseek:chat"),
                &[],
            )
            .unwrap(),
        );
        let (tx, _rx): (super::AcpAgentTx, _) = tokio::sync::mpsc::unbounded_channel();
        let state = AgentState::default();
        *state.handle.lock().await = Some(AgentHandle {
            acp_tx: tx,
            session_id: super::acp::SessionId::new("s1"),
            cancel,
            cwd: dir.path().to_path_buf(),
            surface_kind: crate::surface::SurfaceKind::Code,
            work_workspace_id: None,
            provider_catalog_key: Some("deepseek:chat".into()),
            provider_profile: provider_profile_for_catalog_key(Some("deepseek:chat")).unwrap(),
            capability_lease,
        });
        (state, dir)
    }
    // 正方向：未取消（= 引擎意外死亡）→ handle 被摘除并返回身份。
    #[tokio::test]
    async fn unexpected_death_clears_handle_and_reports_identity() {
        let (state, _dir) = state_with_handle(CancellationToken::new()).await;
        let cleanup = clear_dead_handle(&state, &CancellationToken::new())
            .await
            .expect("unexpected death must report identity");
        assert_eq!(cleanup.session_id, "s1");
        assert!(!cleanup.cwd.is_empty());
        assert_eq!(cleanup.released_resources.unwrap(), 0);
        assert!(
            state.handle.lock().await.is_none(),
            "dead engine's handle must not survive"
        );
    }
    // 反方向：正常拆除（token 已取消）绝不能摘 handle——旧泵的退出路径，
    // 误摘会把用户正在切换的新会话拆掉。
    #[tokio::test]
    async fn teardown_cancelled_token_leaves_handle_untouched() {
        let cancel = CancellationToken::new();
        let (state, _dir) = state_with_handle(cancel.clone()).await;
        cancel.cancel();
        assert!(
            clear_dead_handle(&state, &cancel).await.is_none(),
            "teardown must not report engine death"
        );
        assert!(
            state.handle.lock().await.is_some(),
            "handle must survive normal teardown classification"
        );
    }
    // 反方向的并发窗口：旧泵已开始清理、但仍在等待 handle 锁时，会话切换
    // 取消旧 token。取消判定必须发生在取得锁之后，否则旧泵会误摘仍存活的
    // handle。current_thread + yield 让清理任务确定先运行并阻塞在锁上。
    #[tokio::test(flavor = "current_thread")]
    async fn cancellation_while_waiting_for_handle_lock_preserves_live_handle() {
        let cancel = CancellationToken::new();
        let (state, _dir) = state_with_handle(cancel.clone()).await;
        let state = Arc::new(state);
        let mut held = state.handle.lock().await;
        let cleanup_state = state.clone();
        let cleanup_cancel = cancel.clone();
        let cleanup = tokio::spawn(async move {
            clear_dead_handle(&cleanup_state, &cleanup_cancel).await
        });

        tokio::task::yield_now().await;
        cancel.cancel();
        // Simulate the session switch installing a replacement while it owns
        // the mutex. The waiting old pump must observe cancellation only after
        // this guard is released, and must leave the replacement untouched.
        held.as_mut().unwrap().cancel = CancellationToken::new();
        drop(held);

        assert!(
            cleanup.await.unwrap().is_none(),
            "a pump cancelled during lock contention is normal teardown"
        );
        assert!(
            state.handle.lock().await.is_some(),
            "the old pump must not remove the replacement handle after cancellation"
        );
    }
    // 启动窗口：handle 尚未装上时死亡 → 无事可做，不 panic。
    #[tokio::test]
    async fn death_before_handle_install_is_a_noop() {
        let state = AgentState::default();
        assert!(
            clear_dead_handle(&state, &CancellationToken::new())
                .await
                .is_none()
        );
    }

    // Recovery contract: clearing a dead engine must release every capability
    // binding owned by its lease. Otherwise a restarted session cannot claim
    // the same terminal/worktree/MCP/job identity until the app is restarted.
    #[tokio::test]
    async fn engine_death_releases_resource_for_a_new_lease() {
        let (state, dir) = state_with_handle(CancellationToken::new()).await;
        let old_lease = state
            .handle
            .lock()
            .await
            .as_ref()
            .unwrap()
            .capability_lease
            .clone();
        state
            .resource_registry
            .register(&old_lease, ResourceKind::Terminal, "terminal-reused")
            .unwrap();
        state
            .active_turns
            .lock()
            .await
            .insert("s1".into(), "turn-before-death".into());
        let (permission_tx, permission_rx) = tokio::sync::oneshot::channel();
        state.pending_permissions.lock().await.insert(
            7,
            PendingPermission {
                sender: permission_tx,
                session_id: "s1".into(),
                lease_id: old_lease.lease_id.clone(),
                call_id: "call-before-death".into(),
                action_fingerprint: "fingerprint-before-death".into(),
                option_ids: std::collections::BTreeSet::new(),
            },
        );

        let cleanup = clear_dead_handle(&state, &CancellationToken::new())
            .await
            .expect("unexpected death must clear the live handle");
        assert_eq!(cleanup.released_resources.unwrap(), 1);
        assert_eq!(permission_rx.await.unwrap(), None);
        assert!(state.active_turns.lock().await.get("s1").is_none());
        assert!(state.pending_permissions.lock().await.is_empty());

        let replacement = issue_session_capability_lease(
            "s2",
            crate::surface::SurfaceKind::Code,
            dir.path(),
            None,
            Some("deepseek:chat"),
            &[],
        )
        .unwrap();
        assert!(
            state
                .resource_registry
                .register(
                    &replacement,
                    ResourceKind::Terminal,
                    "terminal-reused"
                )
                .is_ok(),
            "a replacement lease must be able to reclaim the released resource id"
        );
    }

    // Recovery visibility must not depend on capability-registry bookkeeping.
    // An invalid lease gives release_all a deterministic failure without
    // poisoning a process-global mutex: cleanup must still detach the dead
    // handle and return the identity needed for agent://engine-dead.
    #[tokio::test]
    async fn resource_release_failure_still_clears_handle_and_reports_identity() {
        let (state, _dir) = state_with_handle(CancellationToken::new()).await;
        {
            let mut handle = state.handle.lock().await;
            let live = handle.as_mut().unwrap();
            let mut invalid_lease = (*live.capability_lease).clone();
            invalid_lease.schema_version = 0;
            live.capability_lease = Arc::new(invalid_lease);
        }

        let cleanup = clear_dead_handle(&state, &CancellationToken::new())
            .await
            .expect("bookkeeping failure must not suppress engine-dead identity");
        assert_eq!(cleanup.session_id, "s1");
        assert!(!cleanup.cwd.is_empty());
        assert!(
            cleanup
                .released_resources
                .unwrap_err()
                .starts_with("RESOURCE_RELEASE_FAILED:"),
        );
        assert!(
            state.handle.lock().await.is_none(),
            "dead handle must be removed even when resource release fails"
        );
    }
}
fn acp_tool_category(kind: acp::ToolKind) -> String {
    let wire = serde_json::to_value(kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "other".to_string());
    format!("acp_{wire}")
}

fn permission_decision(kind: &acp::PermissionOptionKind) -> ApprovalDecision {
    match kind {
        acp::PermissionOptionKind::AllowOnce | acp::PermissionOptionKind::AllowAlways => {
            ApprovalDecision::Approved
        }
        acp::PermissionOptionKind::RejectOnce | acp::PermissionOptionKind::RejectAlways => {
            ApprovalDecision::Denied
        }
        _ => ApprovalDecision::Cancelled,
    }
}

fn json_fingerprint(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(value) => serde_json::to_vec(value)
            .map(|bytes| hex_sha256(&bytes))
            .unwrap_or_else(|_| hex_sha256(b"serialization_failed")),
        None => hex_sha256(b"null"),
    }
}

fn tool_resource_id(session_id: &str, call_id: &str) -> String {
    format!("{session_id}\u{1f}{call_id}")
}

async fn record_tool_started(
    state: &AgentState,
    session_id: &str,
    call_id: &str,
    tool_name: String,
    raw_input: Option<&serde_json::Value>,
) -> Result<(), String> {
    let key = (session_id.to_string(), call_id.to_string());
    if state.ledger_terminal_calls.lock().await.contains(&key) {
        return Ok(());
    }
    {
        let mut calls = state.ledger_tool_calls.lock().await;
        if calls.contains_key(&key) {
            return Ok(());
        }
        calls.insert(key.clone(), tool_name.clone());
    }
    let lease = state.live_capability_lease(session_id).await?;
    let resource_id = tool_resource_id(session_id, call_id);
    let resource_id_hash = match state
        .resource_registry
        .register(&lease, ResourceKind::Job, &resource_id)
    {
        Ok(hash) => hash,
        Err(error) => {
            state.ledger_tool_calls.lock().await.remove(&key);
            return Err(format!("RESOURCE_OWNERSHIP_BLOCKED: {error}"));
        }
    };
    if let Err(error) = state
        .append_live_event(
            session_id,
            Some(call_id.to_string()),
            ExecutionEventKind::ResourceCreated {
                resource_kind: "job".to_string(),
                resource_id_hash,
            },
        )
        .await
    {
        let _ = state
            .resource_registry
            .release(&lease, ResourceKind::Job, &resource_id);
        state.ledger_tool_calls.lock().await.remove(&key);
        return Err(error);
    }
    if let Err(error) = state
        .append_live_event(
            session_id,
            Some(call_id.to_string()),
            ExecutionEventKind::ToolCalled {
                tool_name,
                arguments_fingerprint: json_fingerprint(raw_input),
            },
        )
        .await
    {
        let _ = state
            .resource_registry
            .release(&lease, ResourceKind::Job, &resource_id);
        state.ledger_tool_calls.lock().await.remove(&key);
        return Err(error);
    }
    Ok(())
}

async fn record_tool_terminal(
    state: &AgentState,
    session_id: &str,
    call_id: &str,
    status: acp::ToolCallStatus,
    kind: Option<acp::ToolKind>,
    raw_input: Option<&serde_json::Value>,
    raw_output: Option<&serde_json::Value>,
) -> Result<(), String> {
    use acp::ToolCallStatus::{Completed, Failed};
    if !matches!(status, Completed | Failed) {
        return Ok(());
    }
    let key = (session_id.to_string(), call_id.to_string());
    if state.ledger_terminal_calls.lock().await.contains(&key) {
        return Ok(());
    }
    let fallback_name = acp_tool_category(kind.unwrap_or_default());
    record_tool_started(
        state,
        session_id,
        call_id,
        fallback_name.clone(),
        raw_input,
    )
    .await?;
    let tool_name = state
        .ledger_tool_calls
        .lock()
        .await
        .remove(&key)
        .unwrap_or(fallback_name);
    let lease = state.live_capability_lease(session_id).await?;
    let resource_id = tool_resource_id(session_id, call_id);
    state
        .resource_registry
        .authorize(&lease, ResourceKind::Job, &resource_id)
        .map_err(|error| format!("RESOURCE_OWNERSHIP_BLOCKED: {error}"))?;
    let event = match status {
        Completed => ExecutionEventKind::ToolCompleted {
            tool_name,
            result_fingerprint: json_fingerprint(raw_output),
        },
        Failed => ExecutionEventKind::ToolFailed {
            tool_name,
            result_fingerprint: json_fingerprint(raw_output),
            error_code: "tool_execution_failed".to_string(),
        },
        _ => return Ok(()),
    };
    state
        .append_live_event(session_id, Some(call_id.to_string()), event)
        .await?;
    // The tool terminal fact is durable now. Mark it before resource cleanup
    // so a cleanup/audit failure cannot cause ACP retries to append a second
    // terminal result.
    state.ledger_terminal_calls.lock().await.insert(key);
    state
        .resource_registry
        .release(&lease, ResourceKind::Job, &resource_id)
        .map_err(|error| format!("RESOURCE_RELEASE_FAILED: {error}"))?;
    state
        .append_live_event(
            session_id,
            Some(call_id.to_string()),
            ExecutionEventKind::ResourceReleased {
                resource_kind: "job".to_string(),
                resource_id_hash: hex_sha256(resource_id.as_bytes()),
            },
        )
        .await?;
    Ok(())
}

async fn record_acp_session_update(
    state: &AgentState,
    session_id: &str,
    update: &acp::SessionUpdate,
) -> Result<(), String> {
    match update {
        acp::SessionUpdate::ToolCall(call) => {
            let call_id = call.tool_call_id.0.as_ref();
            record_tool_started(
                state,
                session_id,
                call_id,
                acp_tool_category(call.kind),
                call.raw_input.as_ref(),
            )
            .await?;
            record_tool_terminal(
                state,
                session_id,
                call_id,
                call.status,
                Some(call.kind),
                call.raw_input.as_ref(),
                call.raw_output.as_ref(),
            )
            .await
        }
        acp::SessionUpdate::ToolCallUpdate(update) => {
            record_tool_terminal(
                state,
                session_id,
                update.tool_call_id.0.as_ref(),
                update.fields.status.unwrap_or_default(),
                update.fields.kind,
                update.fields.raw_input.as_ref(),
                update.fields.raw_output.as_ref(),
            )
            .await
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod execution_ledger_projection_tests {
    use super::*;

    async fn ledger_state() -> (AgentState, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(ExecutionLedger::open(dir.path()).unwrap());
        let capability_lease = Arc::new(
            issue_session_capability_lease(
                "s1",
                crate::surface::SurfaceKind::Code,
                dir.path(),
                None,
                Some("deepseek:chat"),
                &[],
            )
            .unwrap(),
        );
        let state = AgentState::default();
        state.execution_ledger.set(Ok(ledger)).unwrap();
        let (tx, _rx): (AcpAgentTx, _) = tokio::sync::mpsc::unbounded_channel();
        *state.handle.lock().await = Some(AgentHandle {
            acp_tx: tx,
            session_id: acp::SessionId::new("s1"),
            cancel: CancellationToken::new(),
            cwd: dir.path().to_path_buf(),
            surface_kind: crate::surface::SurfaceKind::Code,
            work_workspace_id: None,
            provider_catalog_key: Some("deepseek:chat".into()),
            provider_profile: provider_profile_for_catalog_key(Some("deepseek:chat")).unwrap(),
            capability_lease,
        });
        state
            .active_turns
            .lock()
            .await
            .insert("s1".into(), "t1".into());
        (state, dir)
    }

    #[test]
    fn permission_option_kind_maps_allow_reject_and_future_fail_closed() {
        assert_eq!(
            permission_decision(&acp::PermissionOptionKind::AllowOnce),
            ApprovalDecision::Approved
        );
        assert_eq!(
            permission_decision(&acp::PermissionOptionKind::RejectAlways),
            ApprovalDecision::Denied
        );
    }

    #[test]
    fn prompt_integrity_gate_has_positive_control_and_blocks_duplicates() {
        let mut diagnostics = LedgerDiagnostics {
            schema_version: 1,
            event_count: 0,
            ledger_sha256: hex_sha256(b"ledger"),
            session_ids: BTreeSet::new(),
            open_turns: BTreeSet::new(),
            duplicate_event_ids: BTreeSet::new(),
        };
        assert!(ensure_execution_integrity(&diagnostics).is_ok());
        diagnostics.duplicate_event_ids.insert("evt-duplicate".into());
        assert!(
            ensure_execution_integrity(&diagnostics)
                .unwrap_err()
                .contains("EXECUTION_INTEGRITY_BLOCKED")
        );
    }

    #[tokio::test]
    async fn resource_commit_failure_aborts_session_and_releases_all_bindings() {
        let (state, _dir) = ledger_state().await;
        let (lease, cancel) = {
            let guard = state.handle.lock().await;
            let handle = guard.as_ref().unwrap();
            (handle.capability_lease.clone(), handle.cancel.clone())
        };
        state
            .resource_registry
            .register(&lease, ResourceKind::Terminal, "terminal-orphan")
            .unwrap();

        let released = state
            .abort_live_session_after_resource_failure("s1")
            .await
            .unwrap();

        assert_eq!(released, 1);
        assert!(cancel.is_cancelled());
        assert!(state.handle.lock().await.is_none());
        assert!(state.active_turns.lock().await.get("s1").is_none());
        assert!(
            state
                .resource_registry
                .authorize(&lease, ResourceKind::Terminal, "terminal-orphan")
                .is_err()
        );
    }

    #[test]
    fn duplicate_terminal_id_preserves_the_existing_owner() {
        let root = tempfile::tempdir().unwrap();
        let existing = issue_session_capability_lease(
            "session-existing",
            crate::surface::SurfaceKind::Code,
            root.path(),
            None,
            Some("deepseek:chat"),
            &[],
        )
        .unwrap();
        let incoming = issue_session_capability_lease(
            "session-incoming",
            crate::surface::SurfaceKind::Code,
            root.path(),
            None,
            Some("deepseek:chat"),
            &[],
        )
        .unwrap();
        let registry = ResourceRegistry::default();
        registry
            .register(&existing, ResourceKind::Terminal, "terminal-duplicate")
            .unwrap();

        assert!(
            registry
                .register(&incoming, ResourceKind::Terminal, "terminal-duplicate")
                .is_err()
        );
        assert!(
            registry
                .authorize(&incoming, ResourceKind::Terminal, "terminal-duplicate")
                .is_err()
        );
        assert!(
            registry
                .authorize(&existing, ResourceKind::Terminal, "terminal-duplicate")
                .is_ok()
        );
    }

    #[tokio::test]
    async fn tool_projection_has_one_start_and_one_terminal_without_raw_payloads() {
        let (state, dir) = ledger_state().await;
        let raw_input = serde_json::json!({"command": "secret-command"});
        let start = acp::SessionUpdate::ToolCall(
            acp::ToolCall::new("c1", "human title with secret")
                .kind(acp::ToolKind::Execute)
                .raw_input(Some(raw_input)),
        );
        record_acp_session_update(&state, "s1", &start)
            .await
            .unwrap();
        let terminal = acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
            "c1",
            acp::ToolCallUpdateFields::new()
                .status(Some(acp::ToolCallStatus::Completed))
                .raw_output(Some(serde_json::json!({"stdout": "private-output"}))),
        ));
        record_acp_session_update(&state, "s1", &terminal)
            .await
            .unwrap();
        record_acp_session_update(&state, "s1", &terminal)
            .await
            .unwrap();

        let records = state
            .execution_ledger
            .get()
            .unwrap()
            .as_ref()
            .unwrap()
            .read_all()
            .unwrap();
        assert_eq!(records.len(), 4);
        assert!(matches!(
            &records[0].event,
            ExecutionEventKind::ResourceCreated { .. }
        ));
        assert!(matches!(
            &records[1].event,
            ExecutionEventKind::ToolCalled { .. }
        ));
        assert!(matches!(
            &records[2].event,
            ExecutionEventKind::ToolCompleted { .. }
        ));
        assert!(matches!(
            &records[3].event,
            ExecutionEventKind::ResourceReleased { .. }
        ));
        let persisted = std::fs::read_to_string(dir.path().join(crate::execution_ledger::LEDGER_FILE_NAME))
            .unwrap();
        assert!(!persisted.contains("secret-command"));
        assert!(!persisted.contains("human title"));
        assert!(!persisted.contains("private-output"));
    }

    #[tokio::test]
    async fn terminal_update_before_base_is_synthesized_once_and_late_base_is_ignored() {
        let (state, _dir) = ledger_state().await;
        let terminal = acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
            "c-late",
            acp::ToolCallUpdateFields::new()
                .kind(Some(acp::ToolKind::Read))
                .status(Some(acp::ToolCallStatus::Failed)),
        ));
        record_acp_session_update(&state, "s1", &terminal)
            .await
            .unwrap();
        let late_base = acp::SessionUpdate::ToolCall(
            acp::ToolCall::new("c-late", "late title").kind(acp::ToolKind::Read),
        );
        record_acp_session_update(&state, "s1", &late_base)
            .await
            .unwrap();

        let records = state
            .execution_ledger
            .get()
            .unwrap()
            .as_ref()
            .unwrap()
            .read_all()
            .unwrap();
        assert_eq!(records.len(), 4);
        assert!(matches!(
            &records[2].event,
            ExecutionEventKind::ToolFailed { .. }
        ));
    }
}
/// 事件泵 `recv()` 返回 None：引擎 drop 了它的 client 通道 = 引擎线程退出。
///
/// 此前这里只是静默 break——handle 仍指向死引擎，之后每个 ext 调用
/// （worktree_list 等自动刷新）都以
/// `unable to send 'ext_method' request, channel closed` 弹给用户。
/// 现在 fail-fast：摘掉 handle + 广播 `agent://engine-dead`，前端换成
/// 一条可理解的「引擎已退出」提示。
///
/// 状态操作收在 [`clear_dead_handle`]（可直接单测）；本函数只补日志与事件。
async fn on_engine_channel_closed(app: &AppHandle, pump_cancel: &CancellationToken) {
    let state: State<'_, AgentState> = app.state();
    let Some(cleanup) = clear_dead_handle(&state, pump_cancel).await else {
        return;
    };
    let released_resources = match cleanup.released_resources {
        Ok(count) => count,
        Err(ref error) => {
            tracing::error!(error, "engine channel closed; resource release bookkeeping failed");
            0
        }
    };
    tracing::error!(
        session_id = cleanup.session_id,
        cwd = cleanup.cwd,
        released_resources,
        "engine channel closed without teardown; handle cleared"
    );
    let _ = app.emit(
        "agent://engine-dead",
        serde_json::json!({ "sessionId": cleanup.session_id, "cwd": cleanup.cwd }),
    );
}

#[derive(Debug)]
struct DeadHandleCleanup {
    session_id: String,
    cwd: String,
    released_resources: Result<usize, String>,
}
/// 引擎通道关闭后的 handle 处置（pump 观察到 recv → None 时调用）。
///
/// 定性：pump token 已取消 = **正常拆除**（agent_start 换会话、启动失败
/// 路径都会先 cancel 再摘 handle），引擎随之退出是预期，不算死亡，handle
/// 保持不动。未取消 = 意外退出 → 摘掉 handle。取消检查必须在持有 handle
/// 锁时完成：否则会话切换可能在「检查 token」与「取得锁」之间安装新 handle，
/// 随后旧泵会误摘新会话。
///
/// 摘除前尝试释放旧租约拥有的资源；否则下次启动因 handle 已不存在而跳过
/// 正常拆除路径，同一资源身份会永久报 `RESOURCE_OWNERSHIP_BLOCKED`。记账失败
/// 会随清理结果返回用于日志，但不能阻断摘 handle 或 `agent://engine-dead`。
/// 返回被摘除会话的身份与资源释放结果供事件广播与日志；
/// 无事可做（正常拆除 / 启动窗口内死亡且 handle 尚未装上——那条路
/// agent_start 自己的 acp_send 会拿到同族错误并回报）返回 None。
async fn clear_dead_handle(
    state: &AgentState,
    pump_cancel: &CancellationToken,
) -> Option<DeadHandleCleanup> {
    let mut handle = state.handle.lock().await;
    if pump_cancel.is_cancelled() {
        return None;
    }
    let live = handle.as_ref()?;
    let session_id = live.session_id.0.to_string();
    let released_resources = state
        .resource_registry
        .release_all(&live.capability_lease)
        .map(|released| released.len())
        .map_err(|error| format!("RESOURCE_RELEASE_FAILED: {error}"));
    state.active_turns.lock().await.remove(&session_id);
    let cancelled_permissions = {
        let mut pending = state.pending_permissions.lock().await;
        let ids = pending
            .iter()
            .filter(|(_, permission)| permission.session_id == session_id)
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        ids.into_iter()
            .filter_map(|id| pending.remove(&id))
            .collect::<Vec<_>>()
    };
    // Keep the handle mutex until all session-owned state is detached. A
    // concurrent restart can only install its replacement after this point,
    // so cleanup for a reused session id cannot erase the new session's turn
    // or permission state.
    let dead = handle.take().expect("live handle checked above");
    drop(handle);

    for permission in cancelled_permissions {
        let _ = permission.sender.send(None);
    }

    Some(DeadHandleCleanup {
        session_id,
        cwd: dead.cwd.to_string_lossy().into_owned(),
        released_resources,
    })
}
/// `acp::Error` → 用户可见错误串。通道断开（send/recv 对端已亡 = 引擎线程
/// 退出）时加结构化前缀 `ENGINE_DEAD`，前端据此展示「引擎已退出」而不是
/// channel 天书。判别用 `xai_acp_lib::acp_channel_failure`（错误 data 上的
/// 类型化判别符），不做子串匹配。纯函数，见 map_acp_send_error_tests。
pub(crate) fn map_acp_send_error(err: &acp::Error) -> String {
    if xai_acp_lib::acp_channel_failure(err).is_some() {
        format!("ENGINE_DEAD: {err}")
    } else {
        err.to_string()
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
            let session_id = boxed.request.session_id.0.to_string();
            {
                let state: State<'_, AgentState> = app.state();
                if let Err(error) = record_acp_session_update(
                    &state,
                    &session_id,
                    &boxed.request.update,
                )
                .await
                {
                    tracing::error!("{error}");
                    let guard = state.handle.lock().await;
                    if let Some(handle) = guard.as_ref() {
                        if handle.session_id.0.as_ref() == session_id {
                            handle.cancel.cancel();
                        }
                    }
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
            let approval_id = format!("ap-{id:016x}");
            let session_id = req.request.session_id.0.to_string();
            let call_id = req.request.tool_call.tool_call_id.0.to_string();
            let action_fingerprint = serde_json::to_vec(&req.request.tool_call)
                .map(|bytes| hex_sha256(&bytes))
                .unwrap_or_else(|_| hex_sha256(b"serialization_failed"));
            let lease_id = match state.live_capability_lease(&session_id).await {
                Ok(lease) => lease.lease_id.clone(),
                Err(error) => {
                    tracing::error!("stale permission request rejected: {error}");
                    let _ = req.response_tx.send(Ok(acp::RequestPermissionResponse::new(
                        acp::RequestPermissionOutcome::Cancelled,
                    )));
                    return;
                }
            };
            if let Err(error) = state
                .append_live_event(
                    &session_id,
                    Some(call_id.clone()),
                    ExecutionEventKind::ApprovalRequested {
                        approval_id: approval_id.clone(),
                        action_fingerprint: action_fingerprint.clone(),
                    },
                )
                .await
            {
                tracing::error!("{error}");
                if let Some(handle) = state.handle.lock().await.as_ref() {
                    handle.cancel.cancel();
                }
                let _ = req.response_tx.send(Ok(acp::RequestPermissionResponse::new(
                    acp::RequestPermissionOutcome::Cancelled,
                )));
                return;
            }

            let option_kinds: HashMap<String, acp::PermissionOptionKind> = req
                .request
                .options
                .iter()
                .map(|option| (option.option_id.0.to_string(), option.kind))
                .collect();
            // 无头 smoke：自动选第一个选项（引擎约定首项为放行），否则
            // S3/S4 的命令权限会等前端 600 秒。仅 AUTOTEST 模式生效。
            if std::env::var("WANCODE_AUTOTEST").is_ok() {
                let selected = req.request.options.first().map(|option| option.option_id.clone());
                let decision = selected
                    .as_ref()
                    .and_then(|option_id| option_kinds.get(option_id.0.as_ref()))
                    .map(permission_decision)
                    .unwrap_or(ApprovalDecision::Cancelled);
                let persisted = state
                    .append_live_event(
                        &session_id,
                        Some(call_id),
                        ExecutionEventKind::ApprovalResolved {
                            approval_id,
                            decision,
                        },
                    )
                    .await;
                let outcome = match (persisted, selected) {
                    (Ok(()), Some(option_id)) => acp::RequestPermissionOutcome::Selected(
                        acp::SelectedPermissionOutcome::new(option_id),
                    ),
                    (Err(error), _) => {
                        tracing::error!("{error}");
                        acp::RequestPermissionOutcome::Cancelled
                    }
                    (_, None) => acp::RequestPermissionOutcome::Cancelled,
                };
                let _ = req
                    .response_tx
                    .send(Ok(acp::RequestPermissionResponse::new(outcome)));
                return;
            }

            let (tx, rx) = oneshot::channel::<Option<String>>();
            state.pending_permissions.lock().await.insert(id, PendingPermission {
                sender: tx,
                session_id: session_id.clone(),
                lease_id,
                call_id: call_id.clone(),
                action_fingerprint: action_fingerprint.clone(),
                option_ids: option_kinds.keys().cloned().collect(),
            });

            let payload = serde_json::json!({
                "id": id,
                "request": serde_json::to_value(&req.request).unwrap_or(serde_json::Value::Null),
            });
            let _ = app.emit("agent://permission", payload);

            // Wait for the frontend's decision (10 min timeout → cancel).
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                let decision =
                    tokio::time::timeout(std::time::Duration::from_secs(600), rx).await;
                let selected = match decision {
                    Ok(Ok(Some(option_id))) => Some(option_id),
                    _ => None,
                };
                let audit_decision = selected
                    .as_ref()
                    .and_then(|option_id| option_kinds.get(option_id))
                    .map(permission_decision)
                    .unwrap_or(ApprovalDecision::Cancelled);
                let state: State<'_, AgentState> = app.state();
                let persisted = state
                    .append_live_event(
                        &session_id,
                        Some(call_id),
                        ExecutionEventKind::ApprovalResolved {
                            approval_id,
                            decision: audit_decision,
                        },
                    )
                    .await;
                let outcome = match (persisted, selected) {
                    (Ok(()), Some(option_id)) => acp::RequestPermissionOutcome::Selected(
                        acp::SelectedPermissionOutcome::new(acp::PermissionOptionId::new(
                            option_id,
                        )),
                    ),
                    (Err(error), _) => {
                        tracing::error!("{error}");
                        if let Some(handle) = state.handle.lock().await.as_ref() {
                            handle.cancel.cancel();
                        }
                        acp::RequestPermissionOutcome::Cancelled
                    }
                    (_, None) => acp::RequestPermissionOutcome::Cancelled,
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

/// Completion metadata belongs to the exact prompt invocation. In Work this
/// carries the verified block catalog built from the same immutable snapshot
/// that was sent to the provider, so a queued later prompt cannot replace the
/// evidence used to verify an earlier answer.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptCompletion {
    pub work_citation_sources: Vec<crate::work_context::WorkCitationSource>,
}

/// Read-only, already-redacted execution diagnostics for the timeline/export
/// UI. The command never returns prompt bodies, tool arguments or tool output.
#[tauri::command]
pub fn agent_execution_diagnostics(
    app: AppHandle,
    state: State<'_, AgentState>,
) -> Result<crate::execution_ledger::LedgerDiagnostics, String> {
    state
        .execution_ledger(&app)?
        .diagnostics()
        .map_err(|error| error.to_string())
}

/// Send one user prompt (optionally with pasted images for vision models);
/// resolves when the turn completes.
#[tauri::command]
pub async fn agent_prompt(
    app: AppHandle,
    state: State<'_, AgentState>,
    text: String,
    images: Option<Vec<PromptImage>>,
) -> Result<PromptCompletion, String> {
    let (
        acp_tx,
        session_id,
        surface_kind,
        work_workspace_id,
        provider_catalog_key,
        provider_profile,
        agent_id,
        capability_lease,
    ) = {
        let guard = state.handle.lock().await;
        let h = guard.as_ref().ok_or(SESSION_NOT_STARTED_ERROR)?;
        h.capability_lease
            .validate()
            .map_err(|error| format!("CAPABILITY_LEASE_INVALID: {error}"))?;
        if h.capability_lease.session_id != h.session_id.0.as_ref()
            || h.capability_lease.surface_kind != h.surface_kind
            || h.capability_lease.policy_version != crate::surface::CURRENT_POLICY_VERSION
        {
            return Err("CAPABILITY_LEASE_BINDING_MISMATCH".to_string());
        }
        (
            h.acp_tx.clone(),
            h.session_id.clone(),
            h.surface_kind,
            h.work_workspace_id.clone(),
            h.provider_catalog_key.clone(),
            h.provider_profile.clone(),
            h.capability_lease.agent_id.clone(),
            h.capability_lease.clone(),
        )
    };
    let mut images = images.unwrap_or_default();
    let mut work_citation_sources = Vec::new();
    let text = if surface_kind == crate::surface::SurfaceKind::Work {
        let workspace_id = work_workspace_id.ok_or("Work 会话缺少 workspace_id")?;
        let app_data = app
            .path()
            .app_data_dir()
            .map_err(|e| format!("解析 app_data_dir 失败: {e}"))?;
        let context = tokio::task::spawn_blocking(move || {
            crate::work_context::build_work_context(&app_data, &workspace_id, &text)
        })
        .await
        .map_err(|e| format!("Work 上下文任务失败: {e}"))??;
        images.extend(context.images.into_iter().map(|image| PromptImage {
            data: image.data,
            mime: image.mime,
        }));
        work_citation_sources = context.citation_sources;
        context.text
    } else {
        text
    };
    let evidence = prompt_evidence(
        &text,
        images
            .iter()
            .map(|image| (image.mime.as_str(), image.data.as_str())),
    );
    let tool_schema_sha256 = serde_json::to_vec(&capability_lease.visible_tools)
        .map(|bytes| hex_sha256(&bytes))
        .map_err(|error| format!("TOOL_SCHEMA_FINGERPRINT_FAILED: {error}"))?;
    let provider_key = provider_catalog_key
        .as_deref()
        .unwrap_or("provider-route-unavailable");
    let host_policy_prefix_sha256 = hex_sha256(
        format!(
            "surface={surface_kind:?};policy={};caps={}",
            crate::surface::CURRENT_POLICY_VERSION,
            capability_lease.model_caps_hash
        )
        .as_bytes(),
    );
    let stable_prefix_sha256 = provider_profile.stable_prefix_fingerprint(
        &host_policy_prefix_sha256,
        &tool_schema_sha256,
        None,
    );
    let request_fingerprint = FrozenRequestEvidence {
        prompt_sha256: &evidence.sha256,
        tool_schema_sha256: &tool_schema_sha256,
        stable_prefix_sha256: &stable_prefix_sha256,
        provider_catalog_key: provider_key,
        model_caps_sha256: &capability_lease.model_caps_hash,
        memory_context_sha256: None,
    }
    .fingerprint()
    .map_err(|error| format!("REQUEST_FINGERPRINT_FAILED: {error}"))?;
    let turn_id = state.next_turn_id()?;
    let session_id_string = session_id.0.to_string();
    let execution_ledger = state.execution_ledger(&app)?;
    let diagnostics = execution_ledger
        .diagnostics()
        .map_err(|error| format!("EXECUTION_DIAGNOSTICS_FAILED: {error}"))?;
    ensure_execution_integrity(&diagnostics)?;
    {
        let mut active_turns = state.active_turns.lock().await;
        if active_turns.contains_key(&session_id_string) {
            return Err("TURN_ALREADY_ACTIVE: 当前会话已有未结束回合".to_string());
        }
        active_turns.insert(session_id_string.clone(), turn_id.clone());
    }
    let ledger_context = EventContext {
        session_id: session_id_string.clone(),
        surface_kind,
        policy_version: crate::surface::CURRENT_POLICY_VERSION,
        provider_catalog_key,
        turn_id: Some(turn_id),
        step_id: None,
        call_id: None,
        agent_id: Some(agent_id),
    };
    if let Err(error) = execution_ledger.append(
            ledger_context.clone(),
            ExecutionEventKind::PromptSubmitted {
                sha256: evidence.sha256,
                byte_len: evidence.byte_len,
                content_types: evidence.content_types,
            },
        ) {
        state.active_turns.lock().await.remove(&session_id_string);
        return Err(format!("EXECUTION_LEDGER_APPEND_FAILED: {error}"));
    }
    if let Err(error) =
        execution_ledger.append(ledger_context.clone(), ExecutionEventKind::TurnStarted)
    {
        state.active_turns.lock().await.remove(&session_id_string);
        return Err(format!("EXECUTION_LEDGER_APPEND_FAILED: {error}"));
    }
    if let Err(error) = execution_ledger.append(
        ledger_context.clone(),
        ExecutionEventKind::ProviderRequested {
            request_fingerprint,
        },
    ) {
        state.active_turns.lock().await.remove(&session_id_string);
        return Err(format!("EXECUTION_LEDGER_APPEND_FAILED: {error}"));
    }

    let mut blocks = vec![acp::ContentBlock::Text(acp::TextContent::new(text))];
    for img in images {
        blocks.push(acp::ContentBlock::Image(acp::ImageContent::new(img.data, img.mime)));
    }
    let request = acp::PromptRequest::new(session_id, blocks);
    let result: Result<acp::PromptResponse, _> = acp_send(request, &acp_tx).await;
    let mut usage_validation_error = None;
    let (provider_event, terminal_event) = match &result {
        Ok(response) => {
            let usage = response.usage.as_ref();
            let usage_validation = usage.map(|usage| {
                provider_profile.validate_usage(ProviderUsageFacts {
                    input_tokens: usage.input_tokens,
                    output_tokens: usage.output_tokens,
                    cache_read_tokens: usage.cached_read_tokens,
                })
            });
            if let Some(Err(error)) = usage_validation {
                let error_code = "provider_usage_invalid".to_string();
                usage_validation_error = Some(format!("PROVIDER_USAGE_BLOCKED: {error}"));
                (
                    ExecutionEventKind::ProviderFailed {
                        error_code: error_code.clone(),
                        retryable: false,
                    },
                    ExecutionEventKind::TurnEnded {
                        outcome: TurnOutcome::Failed,
                        error_code: Some(error_code),
                    },
                )
            } else {
                (
                    ExecutionEventKind::ProviderCompleted {
                        input_tokens: usage.map(|usage| usage.input_tokens),
                        output_tokens: usage.map(|usage| usage.output_tokens),
                        cache_read_tokens: usage.and_then(|usage| usage.cached_read_tokens),
                    },
                    ExecutionEventKind::TurnEnded {
                        outcome: TurnOutcome::Completed,
                        error_code: None,
                    },
                )
            }
        }
        Err(error) => {
            let error_code = LedgerRedactor::error_code(&error.to_string()).to_string();
            (
                ExecutionEventKind::ProviderFailed {
                    error_code: error_code.clone(),
                    retryable: false,
                },
                ExecutionEventKind::TurnEnded {
                    outcome: TurnOutcome::Failed,
                    error_code: Some(error_code),
                },
            )
        }
    };
    let terminal_ledger_result = execution_ledger
        .append(ledger_context.clone(), provider_event)
        .and_then(|_| execution_ledger.append(ledger_context, terminal_event));
    state.active_turns.lock().await.remove(&session_id_string);
    let payload = match (&result, &usage_validation_error) {
        (_, Some(error)) => serde_json::json!({
            "ok": false,
            "sessionId": session_id_string,
            "error": error,
            "workCitationSources": &work_citation_sources,
        }),
        (Ok(resp), None) => serde_json::json!({
            "ok": true,
            "sessionId": session_id_string,
            "stopReason": serde_json::to_value(resp.stop_reason).unwrap_or(serde_json::Value::Null),
            "workCitationSources": &work_citation_sources,
        }),
        (Err(e), None) => serde_json::json!({
            "ok": false,
            "sessionId": session_id_string,
            "error": map_acp_send_error(e),
            "workCitationSources": &work_citation_sources,
        }),
    };
    let _ = app.emit("agent://turn-end", payload);
    terminal_ledger_result
        .map_err(|error| format!("EXECUTION_LEDGER_APPEND_FAILED: {error}"))?;
    if let Some(error) = usage_validation_error {
        return Err(error);
    }
    result
        .map(|_| PromptCompletion {
            work_citation_sources: work_citation_sources.clone(),
        })
        .map_err(|e| map_acp_send_error(&e))
}

/// Answer a pending permission request. `option_id = None` cancels/denies.
#[tauri::command]
pub async fn agent_permission_respond(
    state: State<'_, AgentState>,
    id: u64,
    option_id: Option<String>,
) -> Result<(), String> {
    let pending = state.pending_permissions.lock().await.remove(&id)
        .ok_or_else(|| format!("没有待处理的权限请求 #{id}"))?;
    let live_binding = {
        let guard = state.handle.lock().await;
        guard.as_ref().map(|handle| (
            handle.session_id.0.to_string(),
            handle.capability_lease.lease_id.clone(),
        ))
    };
    let validation = live_binding.as_ref().ok_or("stale_receipt").and_then(
        |(session_id, lease_id)| validate_pending_permission(
            &pending,
            session_id,
            lease_id,
            option_id.as_deref(),
        ),
    );
    if validation == Err("stale_receipt") {
        let _ = pending.sender.send(None);
        return Err(format!(
            "APPROVAL_RECEIPT_STALE: #{id} session={} call={} action={}",
            pending.session_id, pending.call_id, pending.action_fingerprint
        ));
    }
    if validation == Err("invalid_option") {
        let _ = pending.sender.send(None);
        return Err(format!("APPROVAL_OPTION_INVALID: #{id}"));
    }
    let _ = pending.sender.send(option_id);
    Ok(())
}



/// Call an `x.ai/*` ACP extension method against the live session and
/// return the raw JSON response.
fn bind_ext_session_params(method: &str, params: &mut serde_json::Value, session_id: &str) {
    let Some(obj) = params.as_object_mut() else {
        return;
    };
    // Target-session operations already carry the selected stored session in
    // `sessionId`. Adding the live handle again as `session_id` makes the
    // request ambiguous and can rename/delete the wrong identity (or report
    // the selected session as missing).
    if matches!(method, "x.ai/session/rename" | "x.ai/session/delete")
        && obj.contains_key("sessionId")
    {
        return;
    }
    // 引擎里同级方法的命名并不统一：mcp/list 用 camelCase 的 sessionId，
    // 而 mcp/toggle / toggle_tool / auth_trigger 用 snake_case 的
    // session_id。两个都塞进去——没有 deny_unknown_fields，多余的键会被
    // 忽略，但少一个就是静默的 missing field 失败。
    //
    // 例外：参数结构体上带 #[serde(alias)] 的方法，两个键会映射到同一
    // 字段，serde 直接报 duplicate field。目前 rewind/* 与 compact
    // 使用 snake 主名，debug/* 使用 camel 主名——这些方法只塞一个。
    let sid = serde_json::Value::String(session_id.to_string());
    if method.starts_with("x.ai/rewind") || method == "x.ai/compact_conversation" {
        obj.entry("session_id").or_insert(sid);
    } else if method.starts_with("x.ai/debug") {
        obj.entry("sessionId").or_insert(sid);
    } else {
        obj.entry("sessionId").or_insert(sid.clone());
        obj.entry("session_id").or_insert(sid);
    }
}

/// An ACP session open is an external actor boundary: the engine can create
/// its session and then fail to deliver the final response. Never let that
/// leave the desktop permanently disabled behind `Starting...`.
const SESSION_OPEN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
const SESSION_HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

#[derive(Debug, PartialEq, Eq)]
enum BoundedAcpOutcome<T> {
    Completed(T),
    Failed(String),
    TimedOut,
}

async fn bounded_acp_request<F, T, E>(
    future: F,
    timeout: std::time::Duration,
) -> BoundedAcpOutcome<T>
where
    F: std::future::Future<Output = std::result::Result<T, E>>,
    E: std::fmt::Display,
{
    match tokio::time::timeout(timeout, future).await {
        Ok(Ok(value)) => BoundedAcpOutcome::Completed(value),
        Ok(Err(error)) => BoundedAcpOutcome::Failed(error.to_string()),
        Err(_) => BoundedAcpOutcome::TimedOut,
    }
}

#[cfg(test)]
mod bounded_acp_request_tests {
    use super::{bounded_acp_request, BoundedAcpOutcome};
    use std::time::Duration;

    #[tokio::test]
    async fn pending_session_open_is_bounded_instead_of_hanging_forever() {
        let outcome = bounded_acp_request(
            std::future::pending::<Result<(), &'static str>>(),
            Duration::from_millis(10),
        )
        .await;
        assert_eq!(outcome, BoundedAcpOutcome::TimedOut);
    }

    #[tokio::test]
    async fn successful_session_open_keeps_its_response() {
        let outcome = bounded_acp_request(
            std::future::ready::<Result<&'static str, &'static str>>(Ok("session")),
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(outcome, BoundedAcpOutcome::Completed("session"));
    }

    #[tokio::test]
    async fn failed_session_open_keeps_the_engine_error() {
        let outcome = bounded_acp_request(
            std::future::ready::<Result<(), &'static str>>(Err("engine closed")),
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(
            outcome,
            BoundedAcpOutcome::Failed("engine closed".to_string())
        );
    }
}

#[cfg(test)]
mod ext_session_param_tests {
    use super::bind_ext_session_params;

    #[test]
    fn target_session_methods_keep_only_the_explicit_selected_identity() {
        for method in ["x.ai/session/rename", "x.ai/session/delete"] {
            let mut params = serde_json::json!({ "sessionId": "stored-target" });
            bind_ext_session_params(method, &mut params, "live-handle");
            assert_eq!(params["sessionId"], "stored-target");
            assert!(params.get("session_id").is_none());
        }
    }

    #[test]
    fn compact_uses_one_canonical_session_field() {
        let mut params = serde_json::json!({ "userContext": null });
        bind_ext_session_params("x.ai/compact_conversation", &mut params, "live-handle");
        assert_eq!(params["session_id"], "live-handle");
        assert!(params.get("sessionId").is_none());
    }

    #[test]
    fn ordinary_extension_calls_still_receive_the_live_identity() {
        let mut params = serde_json::json!({});
        bind_ext_session_params("x.ai/session/search", &mut params, "live-handle");
        assert_eq!(params["sessionId"], "live-handle");
        assert_eq!(params["session_id"], "live-handle");
    }
}

/// One authorization preflight shared by request and notification entrances.
/// Keeping the policy and path checks here makes it impossible for a future
/// `ext_notify` call to gain weaker treatment than the equivalent `ext_call`.
async fn authorize_ext_method(
    state: &State<'_, AgentState>,
    method: &str,
    params: &serde_json::Value,
) -> Result<(), String> {
    let (tool, maximum_risk) = match ext_method_policy(method) {
        ExtMethodPolicy::Required(tool, risk) => (tool, risk),
        ExtMethodPolicy::NoCapability(_reason) => return Ok(()),
        ExtMethodPolicy::Denied(reason) => {
            return Err(format!(
                "CAPABILITY_EXTENSION_BLOCKED: {method}: {reason}"
            ));
        }
    };
    let (lease, cwd) = {
        let guard = state.handle.lock().await;
        let handle = guard.as_ref().ok_or(SESSION_NOT_STARTED_ERROR)?;
        (handle.capability_lease.clone(), handle.cwd.clone())
    };
    lease
        .authorize_tool(tool, maximum_risk)
        .map_err(|error| format!("CAPABILITY_EXTENSION_BLOCKED: {method}: {error}"))?;
    if method.starts_with("x.ai/fs/") {
        let raw_path = params
            .get("path")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("CAPABILITY_EXTENSION_BLOCKED: {method}: missing path"))?;
        let raw_path = std::path::Path::new(raw_path);
        let target = if raw_path.is_absolute() {
            raw_path.to_path_buf()
        } else {
            cwd.join(raw_path)
        };
        let authorization = if maximum_risk == ToolRisk::ReadOnly {
            lease.authorize_read(&target)
        } else {
            lease.authorize_write(&target)
        };
        authorization.map_err(|error| format!("CAPABILITY_PATH_BLOCKED: {method}: {error}"))?;
    }
    Ok(())
}

pub(crate) async fn ext_call(
    state: &State<'_, AgentState>,
    method: &str,
    mut params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let (acp_tx, session_id) = {
        let guard = state.handle.lock().await;
        let h = guard.as_ref().ok_or(SESSION_NOT_STARTED_ERROR)?;
        (h.acp_tx.clone(), h.session_id.clone())
    };
    bind_ext_session_params(method, &mut params, session_id.0.as_ref());
    // #83：git/*（worktree 除外）一律显式带 gitRoot。引擎在会话目录不是
    // 仓库时会静默回退到 workspace-hub 根——嵌入式场景那是本应用自己的
    // 仓库。客户端解析不出仓库就本地拒绝，绝不触发那个回退。
    if method.starts_with("x.ai/git/") && !method.starts_with("x.ai/git/worktree") {
        if let Some(obj) = params.as_object_mut() {
            if !obj.contains_key("gitRoot") && !obj.contains_key("git_root") {
                let root = {
                    let guard = state.handle.lock().await;
                    let h = guard.as_ref().ok_or(SESSION_NOT_STARTED_ERROR)?;
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
    authorize_ext_method(state, method, &params).await?;
    let terminal_action = terminal_resource_action(method);
    let terminal_binding = if terminal_action == TerminalResourceAction::None {
        None
    } else {
        let guard = state.handle.lock().await;
        let handle = guard.as_ref().ok_or(SESSION_NOT_STARTED_ERROR)?;
        handle.capability_lease.validate()
            .map_err(|error| format!("CAPABILITY_LEASE_INVALID: {error}"))?;
        Some((handle.capability_lease.clone(), handle.session_id.0.to_string()))
    };
    if matches!(terminal_action, TerminalResourceAction::Use | TerminalResourceAction::Release) {
        let terminal_id = terminal_id_from_params(&params).ok_or_else(|| {
            format!("RESOURCE_OWNERSHIP_BLOCKED: {method}: missing terminalId")
        })?;
        let (lease, _) = terminal_binding.as_ref().expect("terminal action must carry a lease");
        state.authorize_live_resource(lease, ResourceKind::Terminal, terminal_id)?;
    }
    let raw = serde_json::value::to_raw_value(&params).map_err(|e| e.to_string())?;
    let resp: acp::ExtResponse =
        acp_send(acp::ExtRequest::new(method.to_string(), raw.into()), &acp_tx)
            .await
            .map_err(|e| map_acp_send_error(&e))?;
    let mut response: serde_json::Value =
        serde_json::from_str(resp.0.get()).map_err(|e| e.to_string())?;
    if let Some((lease, lease_session_id)) = terminal_binding.as_ref() {
        if extension_response_succeeded(&response) {
            match terminal_action {
                TerminalResourceAction::Create => {
                    let terminal_id = terminal_id_from_response(&response)
                        .ok_or_else(|| format!("RESOURCE_OWNERSHIP_BLOCKED: {method}: missing terminalId response"))?
                        .to_string();
                    let register_result = state.register_live_resource(
                        lease_session_id,
                        lease,
                        ResourceKind::Terminal,
                        &terminal_id,
                    ).await;
                    if let Err(register_error) = register_result {
                        let conflicts_with_existing_owner = register_error
                            .contains("resource already exists")
                            && state
                                .resource_registry
                                .authorize(lease, ResourceKind::Terminal, &terminal_id)
                                .is_err();
                        if conflicts_with_existing_owner {
                            let released = state
                                .abort_live_session_after_resource_failure(lease_session_id)
                                .await?;
                            return Err(format!(
                                "TERMINAL_CREATE_BLOCKED: {terminal_id}: duplicate ID conflicts with an existing owner; compensating kill skipped; new session aborted; released bindings={released}; {register_error}"
                            ));
                        }
                        let kill_result: Result<bool, String> = async {
                            let kill_params = serde_json::json!({
                                "sessionId": lease_session_id,
                                "terminalId": terminal_id,
                            });
                            let raw = serde_json::value::to_raw_value(&kill_params)
                                .map_err(|error| error.to_string())?;
                            let kill_response: acp::ExtResponse = acp_send(
                                acp::ExtRequest::new("x.ai/terminal/kill".to_string(), raw.into()),
                                &acp_tx,
                            )
                            .await
                            .map_err(|error| error.to_string())?;
                            let value: serde_json::Value = serde_json::from_str(kill_response.0.get())
                                .map_err(|error| error.to_string())?;
                            Ok(extension_response_succeeded(&value))
                        }
                        .await;
                        let released = state
                            .abort_live_session_after_resource_failure(lease_session_id)
                            .await?;
                        return Err(format!(
                            "{register_error}; terminal compensation={kill_result:?}; session aborted; released bindings={released}"
                        ));
                    }
                }
                TerminalResourceAction::List => {
                    retain_owned_terminals(&mut response, &state.resource_registry, lease);
                }
                TerminalResourceAction::Release => {
                    let terminal_id = terminal_id_from_params(&params)
                        .expect("authorized terminal release must include terminalId");
                    state.release_live_resource(
                        lease_session_id,
                        lease,
                        ResourceKind::Terminal,
                        terminal_id,
                    ).await?;
                }
                TerminalResourceAction::Use | TerminalResourceAction::None => {}
            }
        }
    }
    Ok(response)
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
        let h = guard.as_ref().ok_or(SESSION_NOT_STARTED_ERROR)?;
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
    authorize_ext_method(state, method, &params).await?;
    if terminal_resource_action(method) == TerminalResourceAction::Use {
        let terminal_id = terminal_id_from_params(&params).ok_or_else(|| {
            format!("RESOURCE_OWNERSHIP_BLOCKED: {method}: missing terminalId")
        })?;
        let lease = {
            let guard = state.handle.lock().await;
            guard
                .as_ref()
                .ok_or(SESSION_NOT_STARTED_ERROR)?
                .capability_lease
                .clone()
        };
        state.authorize_live_resource(&lease, ResourceKind::Terminal, terminal_id)?;
    }
    let raw = serde_json::value::to_raw_value(&params).map_err(|e| e.to_string())?;
    let _: () = acp_send(
        acp::ExtNotification::new(method.to_string(), raw.into()),
        &acp_tx,
    )
    .await
    .map_err(|e| map_acp_send_error(&e))?;
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

#[cfg(test)]
mod work_workspace_resume_tests {
    use super::*;

    #[test]
    fn fresh_engine_session_can_reuse_the_durable_work_workspace() {
        let workspace = crate::work_staging::WorkspaceId::parse(
            "ws-000000000001-000000-00000001",
        )
        .unwrap();
        assert_eq!(
            select_work_workspace(None, Some(workspace.clone())).unwrap(),
            workspace
        );
    }

    #[test]
    fn resumed_session_rejects_a_conflicting_workspace_pointer() {
        let bound = crate::work_staging::WorkspaceId::parse(
            "ws-000000000001-000000-00000001",
        )
        .unwrap();
        let requested = crate::work_staging::WorkspaceId::parse(
            "ws-000000000002-000000-00000002",
        )
        .unwrap();
        let error = select_work_workspace(Some(&bound), Some(requested)).unwrap_err();
        assert!(error.to_string().contains("WORKSPACE_IDENTITY_CONFLICT"));
    }
}

/// Interrupt the current turn.
#[tauri::command]
pub async fn agent_cancel(state: State<'_, AgentState>) -> Result<(), String> {
    let (acp_tx, session_id) = {
        let guard = state.handle.lock().await;
        let h = guard.as_ref().ok_or(SESSION_NOT_STARTED_ERROR)?;
        (h.acp_tx.clone(), h.session_id.clone())
    };
    acp_send(acp::CancelNotification::new(session_id), &acp_tx)
        .await
        .map(|_| ())
        .map_err(|e| map_acp_send_error(&e))
}
