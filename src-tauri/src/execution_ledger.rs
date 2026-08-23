//! WanCode Harness Kernel v1: append-only execution evidence.
//!
//! The ledger is deliberately a WanCode-owned sidecar instead of a replacement
//! for grok-build's session persistence.  It establishes a stable audit and
//! recovery boundary across ACP/provider/runtime changes.  Every record is a
//! strict, versioned JSON object on one line.  A crash-truncated final line is
//! preserved as evidence and removed before the next append; corruption in any
//! completed line remains fail-closed.

use crate::surface::SurfaceKind;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const EXECUTION_EVENT_SCHEMA_VERSION: u32 = 1;
pub const LEDGER_FILE_NAME: &str = "execution-events-v1.jsonl";
const RECOVERED_TAIL_DIR: &str = "recovered-tails";
const MAX_IDENTITY_BYTES: usize = 256;
const MAX_CONTENT_TYPES: usize = 16;

static RECOVERY_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventContext {
    pub session_id: String,
    pub surface_kind: SurfaceKind,
    pub policy_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_catalog_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
}

impl EventContext {
    pub fn session(
        session_id: impl Into<String>,
        surface_kind: SurfaceKind,
        policy_version: u32,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            surface_kind,
            policy_version,
            provider_catalog_key: None,
            turn_id: None,
            step_id: None,
            call_id: None,
            agent_id: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionEndReason {
    CleanExit,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approved,
    Denied,
    Cancelled,
}

/// Field-level redaction policy. Callers never receive a shortened copy of a
/// secret: unsafe diagnostics are classified into a fixed vocabulary and
/// unsafe content-type metadata is replaced with an inert value.
pub struct LedgerRedactor;

impl LedgerRedactor {
    pub fn error_code(raw: &str) -> &'static str {
        let normalized = raw.to_ascii_lowercase();
        if normalized.contains("429") || normalized.contains("rate limit") {
            "provider_rate_limited"
        } else if normalized.contains("401")
            || normalized.contains("403")
            || normalized.contains("unauthorized")
            || normalized.contains("forbidden")
        {
            "provider_auth_failed"
        } else if normalized.contains("timeout") || normalized.contains("timed out") {
            "provider_timeout"
        } else if normalized.contains("cancel") {
            "cancelled"
        } else if normalized.contains("connection") || normalized.contains("network") {
            "provider_transport_failed"
        } else {
            "unclassified_failure"
        }
    }

    fn content_type(raw: &str) -> String {
        let normalized = raw.trim().to_ascii_lowercase();
        let valid = !normalized.is_empty()
            && normalized.len() <= 127
            && normalized.contains('/')
            && normalized
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$&^_.+-/".contains(&byte));
        if valid {
            normalized
        } else {
            "application/octet-stream".to_string()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptEvidence {
    pub sha256: String,
    pub byte_len: u64,
    pub content_types: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenRequestEvidence<'a> {
    pub prompt_sha256: &'a str,
    pub tool_schema_sha256: &'a str,
    pub stable_prefix_sha256: &'a str,
    pub provider_catalog_key: &'a str,
    pub model_caps_sha256: &'a str,
    pub memory_context_sha256: Option<&'a str>,
}

impl FrozenRequestEvidence<'_> {
    /// Fingerprint the frozen, model-visible request components. A caller can
    /// rebuild the same structure from the session projection and compare the
    /// digest without storing prompt/schema/history bodies in the ledger.
    pub fn fingerprint(&self) -> Result<String, LedgerError> {
        validate_hash("request.prompt_sha256", self.prompt_sha256)
            .and_then(|_| validate_hash("request.tool_schema_sha256", self.tool_schema_sha256))
            .and_then(|_| validate_hash("request.stable_prefix_sha256", self.stable_prefix_sha256))
            .and_then(|_| validate_hash("request.model_caps_sha256", self.model_caps_sha256))
            .and_then(|_| {
                validate_optional_hash("request.memory_context_sha256", self.memory_context_sha256)
            })
            .and_then(|_| {
                validate_label(
                    "request.provider_catalog_key",
                    self.provider_catalog_key,
                    MAX_IDENTITY_BYTES,
                )
            })
            .map_err(|(field, reason)| LedgerError::RejectedEvent { field, reason })?;
        let encoded = serde_json::to_vec(self).map_err(io)?;
        Ok(hex_sha256(&encoded))
    }
}

/// Build stable evidence for the exact content sent to ACP without retaining
/// the prompt or image payload. Length prefixes make the digest unambiguous
/// across different text/image block boundaries.
pub fn prompt_evidence<'a>(
    text: &str,
    images: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> PromptEvidence {
    let mut digest = Sha256::new();
    let mut byte_len = 0u64;
    let mut content_types = vec!["text/plain".to_string()];

    update_evidence_part(&mut digest, b"text/plain", text.as_bytes());
    byte_len = byte_len.saturating_add(text.len() as u64);
    for (mime, data) in images {
        update_evidence_part(&mut digest, mime.as_bytes(), data.as_bytes());
        byte_len = byte_len.saturating_add(data.len() as u64);
        content_types.push(LedgerRedactor::content_type(mime));
    }

    PromptEvidence {
        sha256: format!("{:x}", digest.finalize()),
        byte_len,
        content_types,
    }
}

fn update_evidence_part(digest: &mut Sha256, label: &[u8], value: &[u8]) {
    digest.update((label.len() as u64).to_le_bytes());
    digest.update(label);
    digest.update((value.len() as u64).to_le_bytes());
    digest.update(value);
}

/// Typed event vocabulary.  Payloads intentionally contain identities,
/// fingerprints, counts and bounded diagnostics rather than raw prompts,
/// credentials, environment variables or arbitrary tool output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum ExecutionEventKind {
    SessionStarted {
        resumed: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace_fingerprint: Option<String>,
    },
    SessionEnded {
        reason: SessionEndReason,
    },
    TurnStarted,
    TurnEnded {
        outcome: TurnOutcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error_code: Option<String>,
    },
    PromptSubmitted {
        sha256: String,
        byte_len: u64,
        content_types: Vec<String>,
    },
    ProviderRequested {
        request_fingerprint: String,
    },
    ProviderCompleted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_tokens: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_tokens: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_read_tokens: Option<u64>,
    },
    ProviderFailed {
        error_code: String,
        retryable: bool,
    },
    ToolCalled {
        tool_name: String,
        arguments_fingerprint: String,
    },
    ToolCompleted {
        tool_name: String,
        result_fingerprint: String,
    },
    ToolFailed {
        tool_name: String,
        result_fingerprint: String,
        error_code: String,
    },
    ToolDenied {
        tool_name: String,
        reason_code: String,
    },
    ToolAborted {
        tool_name: String,
        before_dispatch: bool,
    },
    ApprovalRequested {
        approval_id: String,
        action_fingerprint: String,
    },
    ApprovalResolved {
        approval_id: String,
        decision: ApprovalDecision,
    },
    SurfaceBound,
    PolicyApplied,
    PolicyRejected {
        reason_code: String,
    },
    AgentSpawned {
        child_agent_id: String,
    },
    AgentCancelled {
        target_agent_id: String,
    },
    AgentCompleted {
        target_agent_id: String,
        outcome: TurnOutcome,
    },
    ResourceCreated {
        resource_kind: String,
        resource_id_hash: String,
    },
    ResourceReleased {
        resource_kind: String,
        resource_id_hash: String,
    },
    CheckpointCreated {
        checkpoint_id_hash: String,
    },
    RecoveryCompleted {
        recovered_event_count: u64,
        truncated_tail_preserved: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionEvent {
    pub schema_version: u32,
    pub seq: u64,
    pub event_id: String,
    pub time_unix_ms: u64,
    #[serde(flatten)]
    pub context: EventContext,
    #[serde(flatten)]
    pub event: ExecutionEventKind,
}

#[derive(Debug)]
pub enum LedgerError {
    Io(String),
    CorruptRecord {
        line: usize,
        reason: String,
    },
    UnsupportedSchema {
        line: usize,
        version: u32,
    },
    NonMonotonicSequence {
        line: usize,
        expected: u64,
        actual: u64,
    },
    SequenceExhausted,
    RejectedEvent {
        field: &'static str,
        reason: String,
    },
}

impl fmt::Display for LedgerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(reason) => write!(f, "execution ledger IO failure: {reason}"),
            Self::CorruptRecord { line, reason } => {
                write!(
                    f,
                    "execution ledger corrupt record at line {line}: {reason}"
                )
            }
            Self::UnsupportedSchema { line, version } => write!(
                f,
                "execution ledger schema {version} at line {line} is unsupported"
            ),
            Self::NonMonotonicSequence {
                line,
                expected,
                actual,
            } => write!(
                f,
                "execution ledger sequence at line {line}: expected {expected}, got {actual}"
            ),
            Self::SequenceExhausted => write!(f, "execution ledger sequence exhausted"),
            Self::RejectedEvent { field, reason } => {
                write!(f, "execution ledger rejected {field}: {reason}")
            }
        }
    }
}

impl std::error::Error for LedgerError {}

fn io(error: impl fmt::Display) -> LedgerError {
    LedgerError::Io(error.to_string())
}

#[derive(Debug)]
struct LedgerWriter {
    file: File,
    next_seq: u64,
}

/// Process-local serialized writer.  WanCode currently has a single desktop
/// process owning this file; cross-process writers are outside the v1 contract.
#[derive(Debug)]
pub struct ExecutionLedger {
    path: PathBuf,
    writer: Mutex<LedgerWriter>,
    recovered_tail: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LedgerDiagnostics {
    pub schema_version: u32,
    pub event_count: u64,
    pub ledger_sha256: String,
    pub session_ids: BTreeSet<String>,
    pub open_turns: BTreeSet<String>,
    pub duplicate_event_ids: BTreeSet<String>,
}

impl ExecutionLedger {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, LedgerError> {
        let root = root.as_ref();
        std::fs::create_dir_all(root).map_err(io)?;
        let path = root.join(LEDGER_FILE_NAME);
        if !path.exists() {
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(io)?;
            file.sync_all().map_err(io)?;
        }

        let bytes = std::fs::read(&path).map_err(io)?;
        let scan = scan_records(&bytes)?;
        let recovered_tail = match scan.invalid_final_fragment {
            Some(fragment) => {
                let backup = preserve_truncated_tail(root, fragment)?;
                let file = OpenOptions::new().write(true).open(&path).map_err(io)?;
                file.set_len(scan.valid_bytes as u64).map_err(io)?;
                file.sync_all().map_err(io)?;
                Some(backup)
            }
            None => {
                if scan.needs_newline {
                    let mut file = OpenOptions::new().append(true).open(&path).map_err(io)?;
                    file.write_all(b"\n").map_err(io)?;
                    file.sync_all().map_err(io)?;
                }
                None
            }
        };

        let next_seq = scan
            .last_seq
            .checked_add(1)
            .ok_or(LedgerError::SequenceExhausted)?;
        let open_turns = find_open_turns(&scan.records);
        let file = OpenOptions::new().append(true).open(&path).map_err(io)?;
        let ledger = Self {
            path,
            writer: Mutex::new(LedgerWriter { file, next_seq }),
            recovered_tail,
        };

        let recovered_turn_count = open_turns.len() as u64;
        for context in open_turns.into_values() {
            ledger.append(
                context,
                ExecutionEventKind::TurnEnded {
                    outcome: TurnOutcome::Cancelled,
                    error_code: Some("process_restarted".to_string()),
                },
            )?;
        }
        if ledger.recovered_tail.is_some() || recovered_turn_count > 0 {
            ledger.append(
                EventContext::session(
                    "system:execution-ledger",
                    SurfaceKind::Code,
                    crate::surface::CURRENT_POLICY_VERSION,
                ),
                ExecutionEventKind::RecoveryCompleted {
                    recovered_event_count: recovered_turn_count,
                    truncated_tail_preserved: ledger.recovered_tail.is_some(),
                },
            )?;
        }
        Ok(ledger)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn recovered_tail(&self) -> Option<&Path> {
        self.recovered_tail.as_deref()
    }

    pub fn append(
        &self,
        context: EventContext,
        event: ExecutionEventKind,
    ) -> Result<ExecutionEvent, LedgerError> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| io("execution ledger writer lock poisoned"))?;
        let seq = writer.next_seq;
        let time_unix_ms = now_unix_ms()?;
        let record = ExecutionEvent {
            schema_version: EXECUTION_EVENT_SCHEMA_VERSION,
            seq,
            event_id: format!(
                "wl-{seq:016x}-{time_unix_ms:016x}-{:08x}",
                std::process::id()
            ),
            time_unix_ms,
            context,
            event,
        };
        validate_record(&record)
            .map_err(|(field, reason)| LedgerError::RejectedEvent { field, reason })?;
        let mut encoded = serde_json::to_vec(&record).map_err(io)?;
        encoded.push(b'\n');
        writer.file.write_all(&encoded).map_err(io)?;
        writer.file.flush().map_err(io)?;
        writer.file.sync_data().map_err(io)?;
        writer.next_seq = seq.checked_add(1).ok_or(LedgerError::SequenceExhausted)?;
        Ok(record)
    }

    pub fn read_all(&self) -> Result<Vec<ExecutionEvent>, LedgerError> {
        // Do not observe a record between write_all and sync_data.  This lock is
        // also the positive guarantee behind the concurrent-reader tests added
        // when the first event producers are wired.
        let _writer = self
            .writer
            .lock()
            .map_err(|_| io("execution ledger writer lock poisoned"))?;
        let bytes = std::fs::read(&self.path).map_err(io)?;
        let scan = scan_records(&bytes)?;
        if scan.invalid_final_fragment.is_some() {
            return Err(LedgerError::CorruptRecord {
                line: scan.records.len() + 1,
                reason: "truncated final fragment appeared while ledger is open".into(),
            });
        }
        Ok(scan.records)
    }

    /// Redacted diagnostic projection suitable for the UI/export layer. It
    /// reports identities and integrity only; event payload bodies stay in the
    /// validated JSONL evidence file.
    pub fn diagnostics(&self) -> Result<LedgerDiagnostics, LedgerError> {
        let _writer = self
            .writer
            .lock()
            .map_err(|_| io("execution ledger writer lock poisoned"))?;
        let bytes = std::fs::read(&self.path).map_err(io)?;
        let scan = scan_records(&bytes)?;
        if scan.invalid_final_fragment.is_some() {
            return Err(LedgerError::CorruptRecord {
                line: scan.records.len() + 1,
                reason: "truncated final fragment appeared while ledger is open".into(),
            });
        }
        let records = scan.records;
        let mut session_ids = BTreeSet::new();
        let mut event_ids = HashSet::new();
        let mut duplicate_event_ids = BTreeSet::new();
        for record in &records {
            session_ids.insert(record.context.session_id.clone());
            if !event_ids.insert(record.event_id.clone()) {
                duplicate_event_ids.insert(record.event_id.clone());
            }
        }
        let open_turns = find_open_turns(&records)
            .into_keys()
            .map(|(session_id, turn_id)| format!("{session_id}:{turn_id}"))
            .collect();
        Ok(LedgerDiagnostics {
            schema_version: EXECUTION_EVENT_SCHEMA_VERSION,
            event_count: records.len() as u64,
            ledger_sha256: hex_sha256(&bytes),
            session_ids,
            open_turns,
            duplicate_event_ids,
        })
    }
}

fn find_open_turns(records: &[ExecutionEvent]) -> BTreeMap<(String, String), EventContext> {
    let mut open = BTreeMap::new();
    for record in records {
        let Some(turn_id) = record.context.turn_id.as_ref() else {
            continue;
        };
        let key = (record.context.session_id.clone(), turn_id.clone());
        match record.event {
            ExecutionEventKind::TurnStarted => {
                open.insert(key, record.context.clone());
            }
            ExecutionEventKind::TurnEnded { .. } => {
                open.remove(&key);
            }
            _ => {}
        }
    }
    open
}

#[derive(Debug)]
struct ScanResult<'a> {
    records: Vec<ExecutionEvent>,
    last_seq: u64,
    valid_bytes: usize,
    invalid_final_fragment: Option<&'a [u8]>,
    needs_newline: bool,
}

fn scan_records(bytes: &[u8]) -> Result<ScanResult<'_>, LedgerError> {
    let mut records = Vec::new();
    let mut expected_seq = 1u64;
    let mut valid_bytes = 0usize;
    let mut cursor = 0usize;
    let mut line_number = 0usize;

    while cursor < bytes.len() {
        line_number += 1;
        let remainder = &bytes[cursor..];
        let newline_offset = remainder.iter().position(|byte| *byte == b'\n');
        let (line, consumed, complete) = match newline_offset {
            Some(offset) => (&remainder[..offset], offset + 1, true),
            None => (remainder, remainder.len(), false),
        };
        if line.is_empty() {
            return Err(LedgerError::CorruptRecord {
                line: line_number,
                reason: "blank records are forbidden".into(),
            });
        }
        match parse_record(line, line_number, expected_seq) {
            Ok(record) => {
                expected_seq = expected_seq
                    .checked_add(1)
                    .ok_or(LedgerError::SequenceExhausted)?;
                records.push(record);
                cursor += consumed;
                valid_bytes = cursor;
                if !complete {
                    return Ok(ScanResult {
                        last_seq: expected_seq - 1,
                        records,
                        valid_bytes,
                        invalid_final_fragment: None,
                        needs_newline: true,
                    });
                }
            }
            Err(_error) if !complete => {
                return Ok(ScanResult {
                    last_seq: expected_seq - 1,
                    records,
                    valid_bytes,
                    invalid_final_fragment: Some(line),
                    needs_newline: false,
                });
            }
            Err(error) => return Err(error),
        }
    }

    Ok(ScanResult {
        last_seq: expected_seq - 1,
        records,
        valid_bytes,
        invalid_final_fragment: None,
        needs_newline: false,
    })
}

fn parse_record(
    line: &[u8],
    line_number: usize,
    expected_seq: u64,
) -> Result<ExecutionEvent, LedgerError> {
    let record: ExecutionEvent =
        serde_json::from_slice(line).map_err(|error| LedgerError::CorruptRecord {
            line: line_number,
            reason: error.to_string(),
        })?;
    if record.schema_version != EXECUTION_EVENT_SCHEMA_VERSION {
        return Err(LedgerError::UnsupportedSchema {
            line: line_number,
            version: record.schema_version,
        });
    }
    if record.seq != expected_seq {
        return Err(LedgerError::NonMonotonicSequence {
            line: line_number,
            expected: expected_seq,
            actual: record.seq,
        });
    }
    validate_record(&record).map_err(|(field, reason)| LedgerError::CorruptRecord {
        line: line_number,
        reason: format!("unsafe {field}: {reason}"),
    })?;
    Ok(record)
}

fn validate_record(record: &ExecutionEvent) -> Result<(), (&'static str, String)> {
    validate_label("session_id", &record.context.session_id, MAX_IDENTITY_BYTES)?;
    validate_optional_label(
        "provider_catalog_key",
        record.context.provider_catalog_key.as_deref(),
        MAX_IDENTITY_BYTES,
    )?;
    validate_optional_label(
        "turn_id",
        record.context.turn_id.as_deref(),
        MAX_IDENTITY_BYTES,
    )?;
    validate_optional_label(
        "step_id",
        record.context.step_id.as_deref(),
        MAX_IDENTITY_BYTES,
    )?;
    validate_optional_label(
        "call_id",
        record.context.call_id.as_deref(),
        MAX_IDENTITY_BYTES,
    )?;
    validate_optional_label(
        "agent_id",
        record.context.agent_id.as_deref(),
        MAX_IDENTITY_BYTES,
    )?;

    match &record.event {
        ExecutionEventKind::SessionStarted {
            workspace_fingerprint,
            ..
        } => validate_optional_hash("workspace_fingerprint", workspace_fingerprint.as_deref())?,
        ExecutionEventKind::TurnEnded { error_code, .. } => {
            validate_optional_code("error_code", error_code.as_deref())?
        }
        ExecutionEventKind::PromptSubmitted {
            sha256,
            content_types,
            ..
        } => {
            validate_hash("prompt.sha256", sha256)?;
            if content_types.is_empty() || content_types.len() > MAX_CONTENT_TYPES {
                return Err((
                    "prompt.content_types",
                    format!("must contain 1..={MAX_CONTENT_TYPES} entries"),
                ));
            }
            for content_type in content_types {
                if LedgerRedactor::content_type(content_type) != *content_type {
                    return Err((
                        "prompt.content_types",
                        "contains an unsafe content type".to_string(),
                    ));
                }
            }
        }
        ExecutionEventKind::ProviderRequested {
            request_fingerprint,
        } => validate_hash("provider.request_fingerprint", request_fingerprint)?,
        ExecutionEventKind::ProviderFailed { error_code, .. } => {
            validate_code("provider.error_code", error_code)?
        }
        ExecutionEventKind::ToolCalled {
            tool_name,
            arguments_fingerprint,
        } => {
            validate_label("tool_name", tool_name, MAX_IDENTITY_BYTES)?;
            validate_hash("tool.arguments_fingerprint", arguments_fingerprint)?;
        }
        ExecutionEventKind::ToolCompleted {
            tool_name,
            result_fingerprint,
        } => {
            validate_label("tool_name", tool_name, MAX_IDENTITY_BYTES)?;
            validate_hash("tool.result_fingerprint", result_fingerprint)?;
        }
        ExecutionEventKind::ToolFailed {
            tool_name,
            result_fingerprint,
            error_code,
        } => {
            validate_label("tool_name", tool_name, MAX_IDENTITY_BYTES)?;
            validate_hash("tool.result_fingerprint", result_fingerprint)?;
            validate_code("tool.error_code", error_code)?;
        }
        ExecutionEventKind::ToolDenied {
            tool_name,
            reason_code,
        } => {
            validate_label("tool_name", tool_name, MAX_IDENTITY_BYTES)?;
            validate_code("tool.reason_code", reason_code)?;
        }
        ExecutionEventKind::ToolAborted { tool_name, .. } => {
            validate_label("tool_name", tool_name, MAX_IDENTITY_BYTES)?
        }
        ExecutionEventKind::ApprovalRequested {
            approval_id,
            action_fingerprint,
        } => {
            validate_label("approval_id", approval_id, MAX_IDENTITY_BYTES)?;
            validate_hash("approval.action_fingerprint", action_fingerprint)?;
        }
        ExecutionEventKind::ApprovalResolved { approval_id, .. } => {
            validate_label("approval_id", approval_id, MAX_IDENTITY_BYTES)?
        }
        ExecutionEventKind::PolicyRejected { reason_code } => {
            validate_code("policy.reason_code", reason_code)?
        }
        ExecutionEventKind::AgentSpawned { child_agent_id } => {
            validate_label("child_agent_id", child_agent_id, MAX_IDENTITY_BYTES)?
        }
        ExecutionEventKind::AgentCancelled { target_agent_id }
        | ExecutionEventKind::AgentCompleted {
            target_agent_id, ..
        } => validate_label("target_agent_id", target_agent_id, MAX_IDENTITY_BYTES)?,
        ExecutionEventKind::ResourceCreated {
            resource_kind,
            resource_id_hash,
        }
        | ExecutionEventKind::ResourceReleased {
            resource_kind,
            resource_id_hash,
        } => {
            validate_code("resource_kind", resource_kind)?;
            validate_hash("resource_id_hash", resource_id_hash)?;
        }
        ExecutionEventKind::CheckpointCreated { checkpoint_id_hash } => {
            validate_hash("checkpoint_id_hash", checkpoint_id_hash)?
        }
        ExecutionEventKind::SessionEnded { .. }
        | ExecutionEventKind::TurnStarted
        | ExecutionEventKind::ProviderCompleted { .. }
        | ExecutionEventKind::SurfaceBound
        | ExecutionEventKind::PolicyApplied
        | ExecutionEventKind::RecoveryCompleted { .. } => {}
    }
    Ok(())
}

fn validate_optional_label(
    field: &'static str,
    value: Option<&str>,
    max_bytes: usize,
) -> Result<(), (&'static str, String)> {
    if let Some(value) = value {
        validate_label(field, value, max_bytes)?;
    }
    Ok(())
}

fn validate_label(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), (&'static str, String)> {
    if value.is_empty() || value.len() > max_bytes {
        return Err((field, format!("length must be 1..={max_bytes} bytes")));
    }
    if value.chars().any(char::is_control) {
        return Err((field, "control characters are forbidden".to_string()));
    }
    let lower = value.to_ascii_lowercase();
    let secret_marker = lower.starts_with("sk-")
        || lower.contains("authorization")
        || lower.contains("bearer ")
        || lower.contains("api_key")
        || lower.contains("apikey")
        || lower.contains("access_token")
        || lower.contains("refresh_token")
        || lower.contains("token=");
    if secret_marker {
        return Err((field, "secret-like content is forbidden".to_string()));
    }
    if lower.contains("://") || lower.contains('?') {
        return Err((field, "URLs and query strings are forbidden".to_string()));
    }
    Ok(())
}

fn validate_code(field: &'static str, value: &str) -> Result<(), (&'static str, String)> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err((
            field,
            "must be a lowercase snake_case code up to 64 bytes".to_string(),
        ));
    }
    Ok(())
}

fn validate_optional_code(
    field: &'static str,
    value: Option<&str>,
) -> Result<(), (&'static str, String)> {
    if let Some(value) = value {
        validate_code(field, value)?;
    }
    Ok(())
}

fn validate_hash(field: &'static str, value: &str) -> Result<(), (&'static str, String)> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err((
            field,
            "must be a 64-character hexadecimal digest".to_string(),
        ));
    }
    Ok(())
}

fn validate_optional_hash(
    field: &'static str,
    value: Option<&str>,
) -> Result<(), (&'static str, String)> {
    if let Some(value) = value {
        validate_hash(field, value)?;
    }
    Ok(())
}

fn preserve_truncated_tail(root: &Path, fragment: &[u8]) -> Result<PathBuf, LedgerError> {
    let dir = root.join(RECOVERED_TAIL_DIR);
    std::fs::create_dir_all(&dir).map_err(io)?;
    let digest = hex_sha256(fragment);
    for _ in 0..32 {
        let ordinal = RECOVERY_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(
            "tail-{:08x}-{ordinal:016x}-{}.bin",
            std::process::id(),
            &digest[..16]
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(fragment).map_err(io)?;
                file.flush().map_err(io)?;
                file.sync_all().map_err(io)?;
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io(error)),
        }
    }
    Err(io("could not allocate a unique recovered-tail file"))
}

fn now_unix_ms() -> Result<u64, LedgerError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io)?
        .as_millis();
    u64::try_from(millis).map_err(|_| io("system time does not fit u64 milliseconds"))
}

pub fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn context(session_id: &str) -> EventContext {
        EventContext::session(session_id, SurfaceKind::Code, 1)
    }

    #[test]
    fn prompt_evidence_is_stable_but_changes_at_block_boundaries() {
        let first = prompt_evidence("hello", [("image/png", "abc")]);
        let same = prompt_evidence("hello", [("image/png", "abc")]);
        let different_boundary = prompt_evidence("helloa", [("image/png", "bc")]);

        assert_eq!(first, same);
        assert_ne!(first.sha256, different_boundary.sha256);
        assert_eq!(first.byte_len, 8);
        assert_eq!(first.content_types, ["text/plain", "image/png"]);
    }

    #[test]
    fn prompt_event_serialization_never_contains_raw_content() {
        let secret = "sk-test-do-not-persist";
        let evidence = prompt_evidence(secret, [("image/png", "private-image-data")]);
        let event = ExecutionEvent {
            schema_version: 1,
            seq: 1,
            event_id: "e1".into(),
            time_unix_ms: 1,
            context: context("s1"),
            event: ExecutionEventKind::PromptSubmitted {
                sha256: evidence.sha256,
                byte_len: evidence.byte_len,
                content_types: evidence.content_types,
            },
        };

        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains(secret));
        assert!(!json.contains("private-image-data"));
    }

    #[test]
    fn redactor_classifies_errors_without_copying_credentials_or_urls() {
        let credential = "Authorization: Bearer sk-live-secret";
        let url = "https://provider.example/v1?access_token=secret";

        assert_eq!(
            LedgerRedactor::error_code(credential),
            "unclassified_failure"
        );
        assert_eq!(LedgerRedactor::error_code(url), "unclassified_failure");
        assert_eq!(
            LedgerRedactor::error_code("HTTP 429"),
            "provider_rate_limited"
        );
        assert_eq!(
            LedgerRedactor::error_code("request timed out"),
            "provider_timeout"
        );
        assert!(!LedgerRedactor::error_code(credential).contains("secret"));
    }

    #[test]
    fn append_rejects_secret_like_identity_and_url_tool_name() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = ExecutionLedger::open(dir.path()).unwrap();
        let secret_context = context("sk-live-do-not-store");
        let secret_error = ledger
            .append(secret_context, ExecutionEventKind::TurnStarted)
            .unwrap_err();
        assert!(matches!(
            secret_error,
            LedgerError::RejectedEvent {
                field: "session_id",
                ..
            }
        ));

        let url_error = ledger
            .append(
                context("s1"),
                ExecutionEventKind::ToolCalled {
                    tool_name: "https://tool.invalid/run?token=secret".into(),
                    arguments_fingerprint: hex_sha256(b"{}"),
                },
            )
            .unwrap_err();
        assert!(matches!(
            url_error,
            LedgerError::RejectedEvent {
                field: "tool_name",
                ..
            }
        ));
        assert!(ledger.read_all().unwrap().is_empty());
    }

    #[test]
    fn concurrent_appends_are_strictly_sequenced_without_duplicate_ids() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = Arc::new(ExecutionLedger::open(dir.path()).unwrap());
        let mut workers = Vec::new();
        for worker in 0..8 {
            let ledger = ledger.clone();
            workers.push(std::thread::spawn(move || {
                for ordinal in 0..50 {
                    ledger
                        .append(
                            context(&format!("session-{worker}")),
                            if ordinal % 2 == 0 {
                                ExecutionEventKind::SurfaceBound
                            } else {
                                ExecutionEventKind::PolicyApplied
                            },
                        )
                        .unwrap();
                }
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }

        let records = ledger.read_all().unwrap();
        assert_eq!(records.len(), 400);
        assert!(records
            .iter()
            .enumerate()
            .all(|(index, record)| record.seq == index as u64 + 1));
        let event_ids = records
            .iter()
            .map(|record| record.event_id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(event_ids.len(), records.len());
        assert!(ledger.diagnostics().unwrap().duplicate_event_ids.is_empty());
    }

    #[test]
    fn unsafe_image_mime_is_replaced_before_persistence() {
        let evidence = prompt_evidence(
            "hello",
            [("image/png\r\nAuthorization: secret", "image-data")],
        );
        assert_eq!(
            evidence.content_types,
            ["text/plain", "application/octet-stream"]
        );
    }

    #[test]
    fn frozen_request_fingerprint_is_stable_and_sensitive_to_every_component() {
        let prompt = hex_sha256(b"prompt");
        let schema = hex_sha256(b"schema");
        let caps = hex_sha256(b"caps");
        let memory = hex_sha256(b"memory");
        let stable_prefix = hex_sha256(b"stable-prefix");
        let baseline = FrozenRequestEvidence {
            prompt_sha256: &prompt,
            tool_schema_sha256: &schema,
            stable_prefix_sha256: &stable_prefix,
            provider_catalog_key: "deepseek:chat",
            model_caps_sha256: &caps,
            memory_context_sha256: Some(&memory),
        };
        assert_eq!(
            baseline.fingerprint().unwrap(),
            baseline.fingerprint().unwrap()
        );
        let prompt_2 = hex_sha256(b"prompt-2");
        let schema_2 = hex_sha256(b"schema-2");
        let caps_2 = hex_sha256(b"caps-2");
        let stable_prefix_2 = hex_sha256(b"stable-prefix-2");

        for changed in [
            FrozenRequestEvidence {
                prompt_sha256: &prompt_2,
                ..baseline.clone()
            },
            FrozenRequestEvidence {
                tool_schema_sha256: &schema_2,
                ..baseline.clone()
            },
            FrozenRequestEvidence {
                stable_prefix_sha256: &stable_prefix_2,
                ..baseline.clone()
            },
            FrozenRequestEvidence {
                provider_catalog_key: "glm:chat",
                ..baseline.clone()
            },
            FrozenRequestEvidence {
                model_caps_sha256: &caps_2,
                ..baseline.clone()
            },
            FrozenRequestEvidence {
                memory_context_sha256: None,
                ..baseline.clone()
            },
        ] {
            assert_ne!(
                baseline.fingerprint().unwrap(),
                changed.fingerprint().unwrap()
            );
        }
    }

    #[test]
    fn diagnostics_report_integrity_without_prompt_bodies() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = ExecutionLedger::open(dir.path()).unwrap();
        let secret = "private prompt body";
        let evidence = prompt_evidence(secret, std::iter::empty::<(&str, &str)>());
        ledger
            .append(
                context("s1"),
                ExecutionEventKind::PromptSubmitted {
                    sha256: evidence.sha256,
                    byte_len: evidence.byte_len,
                    content_types: evidence.content_types,
                },
            )
            .unwrap();

        let diagnostics = ledger.diagnostics().unwrap();
        let json = serde_json::to_string(&diagnostics).unwrap();
        assert_eq!(diagnostics.event_count, 1);
        assert!(diagnostics.open_turns.is_empty());
        assert!(!json.contains(secret));
        assert_eq!(diagnostics.ledger_sha256.len(), 64);
    }

    #[test]
    fn multiple_appends_roundtrip_final_persisted_state() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = ExecutionLedger::open(dir.path()).unwrap();

        let first = ledger
            .append(
                context("s1"),
                ExecutionEventKind::SessionStarted {
                    resumed: false,
                    workspace_fingerprint: Some(hex_sha256(b"D:/repo")),
                },
            )
            .unwrap();
        let second = ledger
            .append(context("s1"), ExecutionEventKind::TurnStarted)
            .unwrap();
        let third = ledger
            .append(
                context("s1"),
                ExecutionEventKind::TurnEnded {
                    outcome: TurnOutcome::Completed,
                    error_code: None,
                },
            )
            .unwrap();

        assert_eq!((first.seq, second.seq, third.seq), (1, 2, 3));
        drop(ledger);

        let reopened = ExecutionLedger::open(dir.path()).unwrap();
        let records = reopened.read_all().unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[2], third);
        assert!(std::fs::read(reopened.path()).unwrap().ends_with(b"\n"));
    }

    #[test]
    fn valid_final_record_without_newline_is_kept_and_normalized() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = ExecutionLedger::open(dir.path()).unwrap();
        ledger
            .append(context("s1"), ExecutionEventKind::TurnStarted)
            .unwrap();
        let path = ledger.path().to_path_buf();
        drop(ledger);
        let mut bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.pop(), Some(b'\n'));
        std::fs::write(&path, bytes).unwrap();

        let reopened = ExecutionLedger::open(dir.path()).unwrap();

        assert_eq!(reopened.read_all().unwrap().len(), 1);
        assert!(std::fs::read(path).unwrap().ends_with(b"\n"));
        assert!(reopened.recovered_tail().is_none());
    }

    #[test]
    fn truncated_final_record_is_preserved_then_next_append_continues_sequence() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = ExecutionLedger::open(dir.path()).unwrap();
        ledger
            .append(context("s1"), ExecutionEventKind::TurnStarted)
            .unwrap();
        let path = ledger.path().to_path_buf();
        drop(ledger);
        let fragment = br#"{"schema_version":1,"seq":2,"event_id":"cut""#;
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(fragment).unwrap();
        drop(file);

        let reopened = ExecutionLedger::open(dir.path()).unwrap();
        let recovery_path = reopened.recovered_tail().unwrap();
        assert_eq!(std::fs::read(recovery_path).unwrap(), fragment);
        let second = reopened
            .append(context("s1"), ExecutionEventKind::SurfaceBound)
            .unwrap();

        assert_eq!(second.seq, 3);
        let records = reopened.read_all().unwrap();
        assert_eq!(records.len(), 3);
        assert!(matches!(
            &records[1].event,
            ExecutionEventKind::RecoveryCompleted {
                recovered_event_count: 0,
                truncated_tail_preserved: true
            }
        ));
    }

    #[test]
    fn reopen_synthesizes_terminal_event_for_interrupted_turn() {
        let dir = tempfile::tempdir().unwrap();
        let ledger = ExecutionLedger::open(dir.path()).unwrap();
        let mut turn_context = context("s1");
        turn_context.turn_id = Some("t1".into());
        ledger
            .append(turn_context.clone(), ExecutionEventKind::TurnStarted)
            .unwrap();
        drop(ledger);

        let reopened = ExecutionLedger::open(dir.path()).unwrap();
        let records = reopened.read_all().unwrap();

        assert_eq!(records.len(), 3);
        assert!(matches!(
            &records[1].event,
            ExecutionEventKind::TurnEnded {
                outcome: TurnOutcome::Cancelled,
                error_code
            } if error_code.as_deref() == Some("process_restarted")
        ));
        assert!(matches!(
            &records[2].event,
            ExecutionEventKind::RecoveryCompleted {
                recovered_event_count: 1,
                truncated_tail_preserved: false
            }
        ));
    }

    #[test]
    fn corruption_in_completed_line_fails_closed_and_is_not_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(LEDGER_FILE_NAME);
        let corrupt = b"{not-json}\n";
        std::fs::write(&path, corrupt).unwrap();

        let error = ExecutionLedger::open(dir.path()).unwrap_err();

        assert!(matches!(error, LedgerError::CorruptRecord { line: 1, .. }));
        assert_eq!(std::fs::read(path).unwrap(), corrupt);
    }

    #[test]
    fn unknown_field_in_completed_record_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let event = ExecutionEvent {
            schema_version: 1,
            seq: 1,
            event_id: "e1".into(),
            time_unix_ms: 1,
            context: context("s1"),
            event: ExecutionEventKind::TurnStarted,
        };
        let mut value = serde_json::to_value(event).unwrap();
        value["future_authority"] = serde_json::json!(true);
        std::fs::write(
            dir.path().join(LEDGER_FILE_NAME),
            format!("{}\n", serde_json::to_string(&value).unwrap()),
        )
        .unwrap();

        let error = ExecutionLedger::open(dir.path()).unwrap_err();

        assert!(matches!(error, LedgerError::CorruptRecord { line: 1, .. }));
    }

    #[test]
    fn seeded_duplicate_event_ids_are_detected_by_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(LEDGER_FILE_NAME);
        let event_a = ExecutionEvent {
            schema_version: EXECUTION_EVENT_SCHEMA_VERSION,
            seq: 1,
            event_id: "duplicate-id".into(),
            time_unix_ms: 1000,
            context: context("s1"),
            event: ExecutionEventKind::SurfaceBound,
        };
        let event_b = ExecutionEvent {
            schema_version: EXECUTION_EVENT_SCHEMA_VERSION,
            seq: 2,
            event_id: "duplicate-id".into(),
            time_unix_ms: 1001,
            context: context("s1"),
            event: ExecutionEventKind::PolicyApplied,
        };
        let content = format!(
            "{}\n{}\n",
            serde_json::to_string(&event_a).unwrap(),
            serde_json::to_string(&event_b).unwrap()
        );
        std::fs::write(&path, content).unwrap();

        let ledger = ExecutionLedger::open(dir.path()).unwrap();
        let diagnostics = ledger.diagnostics().unwrap();
        assert_eq!(
            diagnostics.duplicate_event_ids,
            BTreeSet::from(["duplicate-id".to_string()])
        );
    }

    #[test]
    fn future_schema_and_non_monotonic_sequence_are_distinct_failures() {
        let dir = tempfile::tempdir().unwrap();
        let event = ExecutionEvent {
            schema_version: 99,
            seq: 1,
            event_id: "e1".into(),
            time_unix_ms: 1,
            context: context("s1"),
            event: ExecutionEventKind::TurnStarted,
        };
        std::fs::write(
            dir.path().join(LEDGER_FILE_NAME),
            format!("{}\n", serde_json::to_string(&event).unwrap()),
        )
        .unwrap();
        assert!(matches!(
            ExecutionLedger::open(dir.path()).unwrap_err(),
            LedgerError::UnsupportedSchema {
                line: 1,
                version: 99
            }
        ));

        let second_dir = tempfile::tempdir().unwrap();
        let mut non_monotonic = event;
        non_monotonic.schema_version = 1;
        non_monotonic.seq = 2;
        std::fs::write(
            second_dir.path().join(LEDGER_FILE_NAME),
            format!("{}\n", serde_json::to_string(&non_monotonic).unwrap()),
        )
        .unwrap();
        assert!(matches!(
            ExecutionLedger::open(second_dir.path()).unwrap_err(),
            LedgerError::NonMonotonicSequence {
                line: 1,
                expected: 1,
                actual: 2
            }
        ));
    }
}
