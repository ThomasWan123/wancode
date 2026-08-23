//! WanCode-owned authorization kernel.
//!
//! A capability lease is immutable, hash-addressed authority. Child authority
//! can only be the intersection of its parent and the target Surface policy.
//! Resource identifiers are never capabilities on their own.

use crate::execution_ledger::hex_sha256;
use crate::surface::SurfaceKind;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const CAPABILITY_LEASE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolRisk {
    ReadOnly,
    WorkspaceWrite,
    Process,
    Network,
    Privileged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpInheritance {
    None,
    Named,
    All,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityLease {
    pub schema_version: u32,
    pub lease_id: String,
    pub session_id: String,
    pub surface_kind: SurfaceKind,
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<String>,
    pub workspace_id_hash: String,
    pub provider_route_hash: String,
    pub model_caps_hash: String,
    pub visible_tools: BTreeMap<String, ToolRisk>,
    pub readable_roots: BTreeSet<String>,
    pub writable_roots: BTreeSet<String>,
    pub denied_roots: BTreeSet<String>,
    pub mcp_inheritance: McpInheritance,
    pub mcp_names: BTreeSet<String>,
    pub policy_version: u32,
    pub issued_unix_ms: u64,
}

#[derive(Debug, Clone)]
pub struct LeaseRequest {
    pub session_id: String,
    pub surface_kind: SurfaceKind,
    pub agent_id: String,
    pub parent_agent_id: Option<String>,
    pub workspace_id_hash: String,
    pub provider_route_hash: String,
    pub model_caps_hash: String,
    pub visible_tools: BTreeMap<String, ToolRisk>,
    pub readable_roots: Vec<PathBuf>,
    pub writable_roots: Vec<PathBuf>,
    pub denied_roots: Vec<PathBuf>,
    pub mcp_inheritance: McpInheritance,
    pub mcp_names: BTreeSet<String>,
    pub policy_version: u32,
}

#[derive(Debug, Clone)]
pub struct SurfaceCapabilityPolicy {
    pub surface_kind: SurfaceKind,
    pub visible_tools: BTreeMap<String, ToolRisk>,
    pub readable_roots: Vec<PathBuf>,
    pub writable_roots: Vec<PathBuf>,
    pub denied_roots: Vec<PathBuf>,
    pub mcp_inheritance: McpInheritance,
    pub mcp_names: BTreeSet<String>,
    pub policy_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityError {
    UnsupportedSchema(u32),
    CoworkGated,
    InvalidIdentity(&'static str),
    InvalidDigest(&'static str),
    InvalidRoot(String),
    ParentMismatch,
    ProviderRouteMismatch,
    ToolEscalation(String),
    RootEscalation(String),
    McpEscalation,
    DeniedPath(String),
    ToolDenied(String),
    ResourceAlreadyExists,
    ResourceNotFound,
    ResourceOwnerMismatch,
    ClockFailure,
}

impl fmt::Display for CapabilityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchema(version) => write!(f, "unsupported lease schema {version}"),
            Self::CoworkGated => write!(f, "Cowork remains gated until its release gate passes"),
            Self::InvalidIdentity(field) => write!(f, "invalid identity field {field}"),
            Self::InvalidDigest(field) => write!(f, "invalid digest field {field}"),
            Self::InvalidRoot(reason) => write!(f, "invalid capability root: {reason}"),
            Self::ParentMismatch => write!(f, "child lease parent identity mismatch"),
            Self::ProviderRouteMismatch => write!(f, "child provider route differs from parent"),
            Self::ToolEscalation(tool) => write!(f, "tool authority escalation: {tool}"),
            Self::RootEscalation(root) => write!(f, "root authority escalation: {root}"),
            Self::McpEscalation => write!(f, "MCP authority escalation"),
            Self::DeniedPath(path) => write!(f, "path is denied: {path}"),
            Self::ToolDenied(tool) => write!(f, "tool is denied: {tool}"),
            Self::ResourceAlreadyExists => write!(f, "resource already exists"),
            Self::ResourceNotFound => write!(f, "resource not found"),
            Self::ResourceOwnerMismatch => write!(f, "resource owner mismatch"),
            Self::ClockFailure => write!(f, "system clock cannot issue a lease"),
        }
    }
}

impl std::error::Error for CapabilityError {}

impl CapabilityLease {
    pub fn issue_root(request: LeaseRequest) -> Result<Self, CapabilityError> {
        if request.parent_agent_id.is_some() {
            return Err(CapabilityError::ParentMismatch);
        }
        issue(request)
    }

    pub fn derive_child(
        &self,
        request: LeaseRequest,
        policy: &SurfaceCapabilityPolicy,
    ) -> Result<Self, CapabilityError> {
        self.validate()?;
        if request.surface_kind == SurfaceKind::Cowork {
            return Err(CapabilityError::CoworkGated);
        }
        if request.parent_agent_id.as_deref() != Some(self.agent_id.as_str())
            || request.session_id != self.session_id
        {
            return Err(CapabilityError::ParentMismatch);
        }
        if request.provider_route_hash != self.provider_route_hash {
            return Err(CapabilityError::ProviderRouteMismatch);
        }
        if request.surface_kind != policy.surface_kind
            || request.policy_version != policy.policy_version
        {
            return Err(CapabilityError::ParentMismatch);
        }

        for (tool, risk) in &request.visible_tools {
            if self.visible_tools.get(tool) != Some(risk)
                || policy.visible_tools.get(tool) != Some(risk)
            {
                return Err(CapabilityError::ToolEscalation(tool.clone()));
            }
        }
        ensure_roots_are_subset(&request.readable_roots, &self.readable_roots)?;
        ensure_roots_are_subset_paths(&request.readable_roots, &policy.readable_roots)?;
        ensure_roots_are_subset(&request.writable_roots, &self.writable_roots)?;
        ensure_roots_are_subset_paths(&request.writable_roots, &policy.writable_roots)?;
        ensure_mcp_subset(
            request.mcp_inheritance,
            &request.mcp_names,
            self.mcp_inheritance,
            &self.mcp_names,
        )?;
        ensure_mcp_subset(
            request.mcp_inheritance,
            &request.mcp_names,
            policy.mcp_inheritance,
            &policy.mcp_names,
        )?;
        let mut child = issue(request)?;
        child.denied_roots.extend(self.denied_roots.iter().cloned());
        child
            .denied_roots
            .extend(normalize_roots(&policy.denied_roots)?);
        reseal(&mut child)?;
        Ok(child)
    }

    pub fn validate(&self) -> Result<(), CapabilityError> {
        if self.schema_version != CAPABILITY_LEASE_SCHEMA_VERSION {
            return Err(CapabilityError::UnsupportedSchema(self.schema_version));
        }
        validate_identity("lease_id", &self.lease_id)?;
        validate_identity("session_id", &self.session_id)?;
        validate_identity("agent_id", &self.agent_id)?;
        if let Some(parent) = &self.parent_agent_id {
            validate_identity("parent_agent_id", parent)?;
        }
        validate_digest("workspace_id_hash", &self.workspace_id_hash)?;
        validate_digest("provider_route_hash", &self.provider_route_hash)?;
        validate_digest("model_caps_hash", &self.model_caps_hash)?;
        Ok(())
    }

    pub fn authorize_tool(
        &self,
        tool: &str,
        maximum_risk: ToolRisk,
    ) -> Result<(), CapabilityError> {
        let risk = self
            .visible_tools
            .get(tool)
            .ok_or_else(|| CapabilityError::ToolDenied(tool.to_string()))?;
        if *risk > maximum_risk {
            return Err(CapabilityError::ToolDenied(tool.to_string()));
        }
        Ok(())
    }

    pub fn authorize_read(&self, path: &Path) -> Result<(), CapabilityError> {
        authorize_path(path, &self.readable_roots, &self.denied_roots)
    }

    pub fn authorize_write(&self, path: &Path) -> Result<(), CapabilityError> {
        authorize_path(path, &self.writable_roots, &self.denied_roots)
    }
}

fn issue(request: LeaseRequest) -> Result<CapabilityLease, CapabilityError> {
    if request.surface_kind == SurfaceKind::Cowork {
        return Err(CapabilityError::CoworkGated);
    }
    validate_identity("session_id", &request.session_id)?;
    validate_identity("agent_id", &request.agent_id)?;
    validate_digest("workspace_id_hash", &request.workspace_id_hash)?;
    validate_digest("provider_route_hash", &request.provider_route_hash)?;
    validate_digest("model_caps_hash", &request.model_caps_hash)?;
    let issued_unix_ms = u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| CapabilityError::ClockFailure)?
            .as_millis(),
    )
    .map_err(|_| CapabilityError::ClockFailure)?;
    let readable_roots = normalize_roots(&request.readable_roots)?;
    let writable_roots = normalize_roots(&request.writable_roots)?;
    let denied_roots = normalize_roots(&request.denied_roots)?;
    for root in &writable_roots {
        if !readable_roots
            .iter()
            .any(|readable| root_is_within(root, readable))
        {
            return Err(CapabilityError::RootEscalation(root.clone()));
        }
    }
    let mut lease = CapabilityLease {
        schema_version: CAPABILITY_LEASE_SCHEMA_VERSION,
        lease_id: "pending".to_string(),
        session_id: request.session_id,
        surface_kind: request.surface_kind,
        agent_id: request.agent_id,
        parent_agent_id: request.parent_agent_id,
        workspace_id_hash: request.workspace_id_hash,
        provider_route_hash: request.provider_route_hash,
        model_caps_hash: request.model_caps_hash,
        visible_tools: request.visible_tools,
        readable_roots,
        writable_roots,
        denied_roots,
        mcp_inheritance: request.mcp_inheritance,
        mcp_names: request.mcp_names,
        policy_version: request.policy_version,
        issued_unix_ms,
    };
    reseal(&mut lease)?;
    lease.validate()?;
    Ok(lease)
}

fn reseal(lease: &mut CapabilityLease) -> Result<(), CapabilityError> {
    lease.lease_id = "pending".to_string();
    let claims =
        serde_json::to_vec(lease).map_err(|_| CapabilityError::InvalidIdentity("lease"))?;
    lease.lease_id = format!("cl-{}", &hex_sha256(&claims)[..32]);
    Ok(())
}

fn validate_identity(field: &'static str, value: &str) -> Result<(), CapabilityError> {
    if value.is_empty()
        || value.len() > 256
        || value.chars().any(char::is_control)
        || value.contains("://")
        || value.contains('?')
    {
        return Err(CapabilityError::InvalidIdentity(field));
    }
    Ok(())
}

fn validate_digest(field: &'static str, value: &str) -> Result<(), CapabilityError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CapabilityError::InvalidDigest(field));
    }
    Ok(())
}

fn normalize_roots(roots: &[PathBuf]) -> Result<BTreeSet<String>, CapabilityError> {
    roots
        .iter()
        .map(|root| normalize_existing_root(root))
        .collect()
}

fn normalize_existing_root(path: &Path) -> Result<String, CapabilityError> {
    let canonical = std::fs::canonicalize(path)
        .map_err(|error| CapabilityError::InvalidRoot(format!("{}: {error}", path.display())))?;
    normalize_absolute(&canonical)
}

fn normalize_candidate(path: &Path) -> Result<String, CapabilityError> {
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(CapabilityError::InvalidRoot(path.display().to_string()));
    }
    if path.exists() {
        return normalize_existing_root(path);
    }
    let mut missing = Vec::new();
    let mut ancestor = path;
    while !ancestor.exists() {
        let name = ancestor
            .file_name()
            .ok_or_else(|| CapabilityError::InvalidRoot(path.display().to_string()))?;
        missing.push(name.to_os_string());
        ancestor = ancestor
            .parent()
            .ok_or_else(|| CapabilityError::InvalidRoot(path.display().to_string()))?;
    }
    let mut canonical = std::fs::canonicalize(ancestor)
        .map_err(|error| CapabilityError::InvalidRoot(error.to_string()))?;
    for part in missing.iter().rev() {
        canonical.push(part);
    }
    normalize_absolute(&canonical)
}

fn normalize_absolute(path: &Path) -> Result<String, CapabilityError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(CapabilityError::InvalidRoot(path.display().to_string()));
    }
    let normalized = path.to_string_lossy().replace('/', "\\");
    Ok(normalized.trim_end_matches('\\').to_ascii_lowercase())
}

fn root_is_within(candidate: &str, root: &str) -> bool {
    candidate == root
        || candidate
            .strip_prefix(root)
            .is_some_and(|suffix| suffix.starts_with('\\'))
}

fn authorize_path(
    path: &Path,
    allowed: &BTreeSet<String>,
    denied: &BTreeSet<String>,
) -> Result<(), CapabilityError> {
    let candidate = normalize_candidate(path)?;
    if denied.iter().any(|root| root_is_within(&candidate, root)) {
        return Err(CapabilityError::DeniedPath(candidate));
    }
    if allowed.iter().any(|root| root_is_within(&candidate, root)) {
        Ok(())
    } else {
        Err(CapabilityError::RootEscalation(candidate))
    }
}

fn ensure_roots_are_subset(
    requested: &[PathBuf],
    parent: &BTreeSet<String>,
) -> Result<(), CapabilityError> {
    for path in requested {
        let normalized = normalize_existing_root(path)?;
        if !parent.iter().any(|root| root_is_within(&normalized, root)) {
            return Err(CapabilityError::RootEscalation(normalized));
        }
    }
    Ok(())
}

fn ensure_roots_are_subset_paths(
    requested: &[PathBuf],
    parent: &[PathBuf],
) -> Result<(), CapabilityError> {
    let parent = normalize_roots(parent)?;
    ensure_roots_are_subset(requested, &parent)
}

fn ensure_mcp_subset(
    requested_mode: McpInheritance,
    requested_names: &BTreeSet<String>,
    parent_mode: McpInheritance,
    parent_names: &BTreeSet<String>,
) -> Result<(), CapabilityError> {
    let permitted = match (requested_mode, parent_mode) {
        (McpInheritance::None, _) => true,
        (McpInheritance::Named, McpInheritance::Named) => requested_names.is_subset(parent_names),
        (McpInheritance::Named, McpInheritance::All) => true,
        (McpInheritance::All, McpInheritance::All) => true,
        _ => false,
    };
    permitted
        .then_some(())
        .ok_or(CapabilityError::McpEscalation)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PolicyDecision {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyVerdict {
    pub decision: PolicyDecision,
    pub reason_codes: BTreeSet<String>,
}

impl PolicyVerdict {
    pub fn allow() -> Self {
        Self {
            decision: PolicyDecision::Allow,
            reason_codes: BTreeSet::new(),
        }
    }

    /// Monotonic composition: later stages can only preserve or strengthen a
    /// decision. Deny therefore cannot be changed back to ask/allow.
    pub fn restrict(mut self, next: PolicyDecision, reason_code: impl Into<String>) -> Self {
        self.decision = self.decision.max(next);
        self.reason_codes.insert(reason_code.into());
        self
    }
}

/// Return the model-visible subset of an object schema. Host-only fields are
/// dropped from properties and required, and additional properties are denied.
pub fn model_visible_schema(
    schema: &serde_json::Value,
    allowlist: &BTreeSet<String>,
) -> Result<serde_json::Value, CapabilityError> {
    let object = schema
        .as_object()
        .ok_or(CapabilityError::InvalidIdentity("tool_schema"))?;
    let properties = object
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .ok_or(CapabilityError::InvalidIdentity("tool_schema.properties"))?;
    let filtered_properties = properties
        .iter()
        .filter(|(name, _)| allowlist.contains(*name))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect::<serde_json::Map<_, _>>();
    let required = object
        .get("required")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .filter(|name| allowlist.contains(*name))
        .map(|name| serde_json::Value::String(name.to_string()))
        .collect::<Vec<_>>();
    Ok(serde_json::json!({
        "type": "object",
        "properties": filtered_properties,
        "required": required,
        "additionalProperties": false
    }))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Terminal,
    Job,
    Mcp,
    Worktree,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResourceOwner {
    lease_id: String,
    session_id: String,
    agent_id: String,
    kind: ResourceKind,
}

#[derive(Default)]
pub struct ResourceRegistry {
    owners: Mutex<BTreeMap<String, ResourceOwner>>,
}

impl ResourceRegistry {
    pub fn register(
        &self,
        lease: &CapabilityLease,
        kind: ResourceKind,
        resource_id: &str,
    ) -> Result<String, CapabilityError> {
        lease.validate()?;
        let resource_hash = hex_sha256(resource_id.as_bytes());
        let owner = ResourceOwner {
            lease_id: lease.lease_id.clone(),
            session_id: lease.session_id.clone(),
            agent_id: lease.agent_id.clone(),
            kind,
        };
        let mut owners = self
            .owners
            .lock()
            .map_err(|_| CapabilityError::ResourceOwnerMismatch)?;
        if owners.contains_key(&resource_hash) {
            return Err(CapabilityError::ResourceAlreadyExists);
        }
        owners.insert(resource_hash.clone(), owner);
        Ok(resource_hash)
    }

    pub fn authorize(
        &self,
        lease: &CapabilityLease,
        kind: ResourceKind,
        resource_id: &str,
    ) -> Result<(), CapabilityError> {
        let resource_hash = hex_sha256(resource_id.as_bytes());
        let owners = self
            .owners
            .lock()
            .map_err(|_| CapabilityError::ResourceOwnerMismatch)?;
        let owner = owners
            .get(&resource_hash)
            .ok_or(CapabilityError::ResourceNotFound)?;
        if owner.lease_id == lease.lease_id
            && owner.session_id == lease.session_id
            && owner.agent_id == lease.agent_id
            && owner.kind == kind
        {
            Ok(())
        } else {
            Err(CapabilityError::ResourceOwnerMismatch)
        }
    }

    pub fn release(
        &self,
        lease: &CapabilityLease,
        kind: ResourceKind,
        resource_id: &str,
    ) -> Result<(), CapabilityError> {
        self.authorize(lease, kind, resource_id)?;
        self.owners
            .lock()
            .map_err(|_| CapabilityError::ResourceOwnerMismatch)?
            .remove(&hex_sha256(resource_id.as_bytes()));
        Ok(())
    }

    /// Release every resource owned by one immutable lease. Returned IDs are
    /// hashes, so cancellation/recovery can audit them without retaining raw
    /// terminal, process or MCP identifiers.
    pub fn release_all(
        &self,
        lease: &CapabilityLease,
    ) -> Result<Vec<(ResourceKind, String)>, CapabilityError> {
        lease.validate()?;
        let mut owners = self
            .owners
            .lock()
            .map_err(|_| CapabilityError::ResourceOwnerMismatch)?;
        let released = owners
            .iter()
            .filter(|(_, owner)| {
                owner.lease_id == lease.lease_id
                    && owner.session_id == lease.session_id
                    && owner.agent_id == lease.agent_id
            })
            .map(|(hash, owner)| (owner.kind, hash.clone()))
            .collect::<Vec<_>>();
        for (_, hash) in &released {
            owners.remove(hash);
        }
        Ok(released)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(root: &Path, agent: &str) -> LeaseRequest {
        LeaseRequest {
            session_id: "s1".into(),
            surface_kind: SurfaceKind::Code,
            agent_id: agent.into(),
            parent_agent_id: None,
            workspace_id_hash: hex_sha256(b"workspace"),
            provider_route_hash: hex_sha256(b"deepseek:chat"),
            model_caps_hash: hex_sha256(b"caps"),
            visible_tools: BTreeMap::from([
                ("read_file".into(), ToolRisk::ReadOnly),
                ("write_file".into(), ToolRisk::WorkspaceWrite),
            ]),
            readable_roots: vec![root.to_path_buf()],
            writable_roots: vec![root.to_path_buf()],
            denied_roots: vec![],
            mcp_inheritance: McpInheritance::All,
            mcp_names: BTreeSet::new(),
            policy_version: 1,
        }
    }

    #[test]
    fn child_lease_can_shrink_but_cannot_add_tool_root_or_provider() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let parent = CapabilityLease::issue_root(request(root.path(), "parent")).unwrap();
        let policy = SurfaceCapabilityPolicy {
            surface_kind: SurfaceKind::Code,
            visible_tools: parent.visible_tools.clone(),
            readable_roots: vec![root.path().to_path_buf()],
            writable_roots: vec![root.path().to_path_buf()],
            denied_roots: vec![],
            mcp_inheritance: McpInheritance::All,
            mcp_names: BTreeSet::new(),
            policy_version: 1,
        };
        let mut child = request(root.path(), "child");
        child.parent_agent_id = Some("parent".into());
        child.visible_tools.remove("write_file");
        assert!(parent.derive_child(child.clone(), &policy).is_ok());

        child
            .visible_tools
            .insert("shell".into(), ToolRisk::Process);
        assert!(matches!(
            parent.derive_child(child.clone(), &policy),
            Err(CapabilityError::ToolEscalation(_))
        ));
        child.visible_tools.remove("shell");
        child.readable_roots = vec![outside.path().to_path_buf()];
        assert!(matches!(
            parent.derive_child(child.clone(), &policy),
            Err(CapabilityError::RootEscalation(_))
        ));
        child.readable_roots = vec![root.path().to_path_buf()];
        child.provider_route_hash = hex_sha256(b"other-provider");
        assert!(matches!(
            parent.derive_child(child, &policy),
            Err(CapabilityError::ProviderRouteMismatch)
        ));
    }

    #[test]
    fn denied_root_wins_and_nonexistent_child_is_resolved_via_existing_ancestor() {
        let root = tempfile::tempdir().unwrap();
        let denied = root.path().join("secret");
        std::fs::create_dir(&denied).unwrap();
        let mut req = request(root.path(), "parent");
        req.denied_roots = vec![denied.clone()];
        let lease = CapabilityLease::issue_root(req).unwrap();

        assert!(lease
            .authorize_read(&root.path().join("new/file.txt"))
            .is_ok());
        assert!(matches!(
            lease.authorize_read(&denied.join("file.txt")),
            Err(CapabilityError::DeniedPath(_))
        ));
    }

    #[test]
    fn monotonic_policy_never_undoes_deny() {
        let verdict = PolicyVerdict::allow()
            .restrict(PolicyDecision::Deny, "surface_denied")
            .restrict(PolicyDecision::Allow, "plugin_allow");
        assert_eq!(verdict.decision, PolicyDecision::Deny);
        assert!(verdict.reason_codes.contains("surface_denied"));
    }

    #[test]
    fn model_schema_drops_host_only_fields_and_required_entries() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "host_owner_token": {"type": "string"}
            },
            "required": ["path", "host_owner_token"]
        });
        let visible = model_visible_schema(&schema, &BTreeSet::from(["path".into()])).unwrap();
        let json = serde_json::to_string(&visible).unwrap();
        assert!(json.contains("path"));
        assert!(!json.contains("host_owner_token"));
        assert_eq!(visible["additionalProperties"], false);
    }

    #[test]
    fn resource_id_alone_cannot_cross_lease_owner() {
        let root = tempfile::tempdir().unwrap();
        let parent = CapabilityLease::issue_root(request(root.path(), "parent")).unwrap();
        let sibling = CapabilityLease::issue_root(request(root.path(), "sibling")).unwrap();
        let registry = ResourceRegistry::default();
        registry
            .register(&parent, ResourceKind::Terminal, "terminal-1")
            .unwrap();
        assert!(registry
            .authorize(&parent, ResourceKind::Terminal, "terminal-1")
            .is_ok());
        assert_eq!(
            registry.authorize(&sibling, ResourceKind::Terminal, "terminal-1"),
            Err(CapabilityError::ResourceOwnerMismatch)
        );
        let released = registry.release_all(&parent).unwrap();
        assert_eq!(released.len(), 1);
        assert_eq!(released[0].0, ResourceKind::Terminal);
        assert_eq!(
            registry.authorize(&parent, ResourceKind::Terminal, "terminal-1"),
            Err(CapabilityError::ResourceNotFound)
        );
    }

    #[test]
    fn concurrent_resource_claim_has_exactly_one_owner() {
        let root = tempfile::tempdir().unwrap();
        let lease = std::sync::Arc::new(
            CapabilityLease::issue_root(request(root.path(), "parent")).unwrap(),
        );
        let registry = std::sync::Arc::new(ResourceRegistry::default());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let workers = (0..8)
            .map(|_| {
                let lease = lease.clone();
                let registry = registry.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    registry.register(&lease, ResourceKind::Job, "shared-job")
                })
            })
            .collect::<Vec<_>>();
        let results = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| { matches!(result, Err(CapabilityError::ResourceAlreadyExists)) })
                .count(),
            7
        );
        assert_eq!(registry.release_all(&lease).unwrap().len(), 1);
    }

    #[test]
    fn register_failure_after_create_leaves_no_orphan_in_registry() {
        let root = tempfile::tempdir().unwrap();
        let lease = CapabilityLease::issue_root(request(root.path(), "parent")).unwrap();
        let registry = ResourceRegistry::default();

        registry
            .register(&lease, ResourceKind::Terminal, "terminal-orphan")
            .unwrap();

        let second_register =
            registry.register(&lease, ResourceKind::Terminal, "terminal-orphan");
        assert!(
            matches!(second_register, Err(CapabilityError::ResourceAlreadyExists)),
            "duplicate register must fail (simulates ledger/registry failure)"
        );

        registry
            .release(&lease, ResourceKind::Terminal, "terminal-orphan")
            .unwrap();
        let remaining = registry.release_all(&lease).unwrap();
        assert!(
            remaining.is_empty(),
            "after compensating release, no orphan terminal should remain"
        );
        assert_eq!(
            registry.authorize(&lease, ResourceKind::Terminal, "terminal-orphan"),
            Err(CapabilityError::ResourceNotFound),
            "orphan must be fully cleaned up and invisible to release_all"
        );
    }
}
