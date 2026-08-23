//! Provider-specific execution tuning without provider identity collapse.
//!
//! Profiles select safe defaults only. Code/hybrid tool presentation is gated
//! by model-bound benchmark evidence; unknown/custom routes stay native and
//! serial until explicitly proven otherwise.

use crate::execution_ledger::hex_sha256;
use serde::{Deserialize, Serialize};
use std::fmt;

pub const PROVIDER_PROFILE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderFamily {
    Glm,
    DeepSeek,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPresentation {
    Native,
    Code,
    Hybrid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderProfile {
    pub schema_version: u32,
    pub provider_catalog_key_hash: String,
    pub family: ProviderFamily,
    pub tool_presentation: ToolPresentation,
    pub max_concurrent_reads: u16,
    pub stable_prompt_prefix: bool,
    pub cache_read_telemetry: bool,
    pub reasoning_round_trip: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub benchmark_evidence_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolModeEvidence {
    pub provider_catalog_key: String,
    pub benchmark_id: String,
    pub requested_mode: ToolPresentation,
    pub trials: u32,
    pub baseline_correctness: f64,
    pub candidate_correctness: f64,
    pub median_latency_improvement: f64,
    pub write_order_drift_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderUsageFacts {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileError {
    InvalidCatalogKey,
    EvidenceIdentityMismatch,
    InsufficientTrials,
    CorrectnessRegression,
    NoMeasuredBenefit,
    WriteOrderDrift,
    CacheTelemetryUnsupported,
    InvalidUsage,
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCatalogKey => write!(f, "invalid provider catalog key"),
            Self::EvidenceIdentityMismatch => write!(f, "benchmark evidence targets another model"),
            Self::InsufficientTrials => write!(f, "benchmark evidence has fewer than 30 trials"),
            Self::CorrectnessRegression => write!(f, "candidate tool mode regresses correctness"),
            Self::NoMeasuredBenefit => {
                write!(f, "candidate tool mode has less than 10% median benefit")
            }
            Self::WriteOrderDrift => write!(f, "candidate tool mode changed write ordering"),
            Self::CacheTelemetryUnsupported => {
                write!(f, "provider does not declare cache-read telemetry")
            }
            Self::InvalidUsage => write!(f, "provider usage facts are inconsistent"),
        }
    }
}

impl std::error::Error for ProfileError {}

impl ProviderProfile {
    pub fn safe_default(
        provider_catalog_key: &str,
        family: ProviderFamily,
    ) -> Result<Self, ProfileError> {
        validate_catalog_key(provider_catalog_key)?;
        let (
            max_concurrent_reads,
            stable_prompt_prefix,
            cache_read_telemetry,
            reasoning_round_trip,
        ) = match family {
            ProviderFamily::Glm => (2, true, false, false),
            ProviderFamily::DeepSeek => (2, true, true, true),
            ProviderFamily::Custom => (1, true, false, false),
        };
        Ok(Self {
            schema_version: PROVIDER_PROFILE_SCHEMA_VERSION,
            provider_catalog_key_hash: hex_sha256(provider_catalog_key.as_bytes()),
            family,
            tool_presentation: ToolPresentation::Native,
            max_concurrent_reads,
            stable_prompt_prefix,
            cache_read_telemetry,
            reasoning_round_trip,
            benchmark_evidence_hash: None,
        })
    }

    pub fn apply_tool_mode_evidence(
        mut self,
        evidence: &ToolModeEvidence,
    ) -> Result<Self, ProfileError> {
        validate_catalog_key(&evidence.provider_catalog_key)?;
        if hex_sha256(evidence.provider_catalog_key.as_bytes()) != self.provider_catalog_key_hash {
            return Err(ProfileError::EvidenceIdentityMismatch);
        }
        if evidence.trials < 30 {
            return Err(ProfileError::InsufficientTrials);
        }
        if evidence.candidate_correctness < evidence.baseline_correctness {
            return Err(ProfileError::CorrectnessRegression);
        }
        if evidence.median_latency_improvement < 0.10 {
            return Err(ProfileError::NoMeasuredBenefit);
        }
        if evidence.write_order_drift_count != 0 {
            return Err(ProfileError::WriteOrderDrift);
        }
        let encoded = serde_json::json!({
            "provider_catalog_key_hash": self.provider_catalog_key_hash,
            "benchmark_id": evidence.benchmark_id,
            "requested_mode": evidence.requested_mode,
            "trials": evidence.trials,
            "baseline_correctness_ppm": (evidence.baseline_correctness * 1_000_000.0).round() as i64,
            "candidate_correctness_ppm": (evidence.candidate_correctness * 1_000_000.0).round() as i64,
            "latency_improvement_ppm": (evidence.median_latency_improvement * 1_000_000.0).round() as i64,
            "write_order_drift_count": evidence.write_order_drift_count
        });
        let bytes = serde_json::to_vec(&encoded).map_err(|_| ProfileError::InvalidUsage)?;
        self.tool_presentation = evidence.requested_mode;
        self.benchmark_evidence_hash = Some(hex_sha256(&bytes));
        Ok(self)
    }

    pub fn validate_usage(&self, usage: ProviderUsageFacts) -> Result<(), ProfileError> {
        if usage.cache_read_tokens.is_some() && !self.cache_read_telemetry {
            return Err(ProfileError::CacheTelemetryUnsupported);
        }
        if usage
            .cache_read_tokens
            .is_some_and(|cache_read| cache_read > usage.input_tokens)
        {
            return Err(ProfileError::InvalidUsage);
        }
        Ok(())
    }

    pub fn stable_prefix_fingerprint(
        &self,
        system_prompt_hash: &str,
        tool_schema_hash: &str,
        memory_prefix_hash: Option<&str>,
    ) -> String {
        let value = serde_json::json!({
            "profile": self.provider_catalog_key_hash,
            "tool_presentation": self.tool_presentation,
            "system_prompt_hash": system_prompt_hash,
            "tool_schema_hash": tool_schema_hash,
            "memory_prefix_hash": memory_prefix_hash
        });
        hex_sha256(&serde_json::to_vec(&value).expect("static JSON shape"))
    }
}

/// Infer `ProviderFamily` from a catalog key using the same slug/hostname
/// conventions as `model_caps::provider_of`. Returns `Custom` for anything
/// that does not match a known family — this is the fail-closed default.
pub fn infer_family(catalog_key: &str) -> ProviderFamily {
    let lower = catalog_key.to_ascii_lowercase();
    if lower.starts_with("glm") || lower.contains("bigmodel") || lower.contains("zhipu") {
        ProviderFamily::Glm
    } else if lower.starts_with("deepseek") {
        ProviderFamily::DeepSeek
    } else {
        ProviderFamily::Custom
    }
}

fn validate_catalog_key(value: &str) -> Result<(), ProfileError> {
    if value.is_empty()
        || value.len() > 256
        || value.chars().any(char::is_control)
        || value.contains("://")
        || value.contains('?')
    {
        return Err(ProfileError::InvalidCatalogKey);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(key: &str) -> ToolModeEvidence {
        ToolModeEvidence {
            provider_catalog_key: key.into(),
            benchmark_id: "B-tool-mode-2026-08".into(),
            requested_mode: ToolPresentation::Hybrid,
            trials: 30,
            baseline_correctness: 0.90,
            candidate_correctness: 0.91,
            median_latency_improvement: 0.20,
            write_order_drift_count: 0,
        }
    }

    #[test]
    fn unknown_custom_provider_is_native_serial_and_has_no_cache_claim() {
        let profile =
            ProviderProfile::safe_default("custom:model", ProviderFamily::Custom).unwrap();
        assert_eq!(profile.tool_presentation, ToolPresentation::Native);
        assert_eq!(profile.max_concurrent_reads, 1);
        assert!(!profile.cache_read_telemetry);
    }

    #[test]
    fn model_bound_evidence_is_required_before_hybrid_mode() {
        let profile =
            ProviderProfile::safe_default("deepseek:chat", ProviderFamily::DeepSeek).unwrap();
        assert_eq!(profile.tool_presentation, ToolPresentation::Native);
        let enabled = profile
            .clone()
            .apply_tool_mode_evidence(&evidence("deepseek:chat"))
            .unwrap();
        assert_eq!(enabled.tool_presentation, ToolPresentation::Hybrid);
        assert!(enabled.benchmark_evidence_hash.is_some());
        assert_eq!(
            profile.apply_tool_mode_evidence(&evidence("deepseek:reasoner")),
            Err(ProfileError::EvidenceIdentityMismatch)
        );
    }

    #[test]
    fn correctness_drift_and_weak_benchmarks_fail_closed() {
        let profile = ProviderProfile::safe_default("glm:chat", ProviderFamily::Glm).unwrap();
        let mut weak = evidence("glm:chat");
        weak.trials = 29;
        assert_eq!(
            profile.clone().apply_tool_mode_evidence(&weak),
            Err(ProfileError::InsufficientTrials)
        );
        weak.trials = 30;
        weak.candidate_correctness = 0.89;
        assert_eq!(
            profile.clone().apply_tool_mode_evidence(&weak),
            Err(ProfileError::CorrectnessRegression)
        );
        weak.candidate_correctness = 0.91;
        weak.write_order_drift_count = 1;
        assert_eq!(
            profile.apply_tool_mode_evidence(&weak),
            Err(ProfileError::WriteOrderDrift)
        );
    }

    #[test]
    fn deepseek_cache_usage_is_accounted_but_impossible_values_are_rejected() {
        let deepseek =
            ProviderProfile::safe_default("deepseek:chat", ProviderFamily::DeepSeek).unwrap();
        assert!(deepseek
            .validate_usage(ProviderUsageFacts {
                input_tokens: 100,
                output_tokens: 10,
                cache_read_tokens: Some(80),
            })
            .is_ok());
        assert_eq!(
            deepseek.validate_usage(ProviderUsageFacts {
                input_tokens: 100,
                output_tokens: 10,
                cache_read_tokens: Some(101),
            }),
            Err(ProfileError::InvalidUsage)
        );
        let custom = ProviderProfile::safe_default("custom:model", ProviderFamily::Custom).unwrap();
        assert_eq!(
            custom.validate_usage(ProviderUsageFacts {
                input_tokens: 100,
                output_tokens: 10,
                cache_read_tokens: Some(10),
            }),
            Err(ProfileError::CacheTelemetryUnsupported)
        );
    }

    #[test]
    fn infer_family_classifies_known_providers_and_fails_closed_on_unknown() {
        assert_eq!(infer_family("glm-4-flash"), ProviderFamily::Glm);
        assert_eq!(infer_family("GLM-5.2"), ProviderFamily::Glm);
        assert_eq!(infer_family("deepseek-chat"), ProviderFamily::DeepSeek);
        assert_eq!(infer_family("deepseek-reasoner"), ProviderFamily::DeepSeek);
        assert_eq!(infer_family("custom:my-model"), ProviderFamily::Custom);
        assert_eq!(infer_family("gpt-4o"), ProviderFamily::Custom);
        assert_eq!(infer_family("qwen2.5-coder"), ProviderFamily::Custom);
    }

    #[test]
    fn glm_deepseek_custom_get_native_serial_defaults_without_evidence() {
        let glm = ProviderProfile::safe_default("glm-4-flash", ProviderFamily::Glm).unwrap();
        assert_eq!(glm.tool_presentation, ToolPresentation::Native);
        assert_eq!(glm.max_concurrent_reads, 2);
        assert!(glm.stable_prompt_prefix);
        assert!(!glm.cache_read_telemetry);
        assert!(glm.benchmark_evidence_hash.is_none());

        let deepseek =
            ProviderProfile::safe_default("deepseek-chat", ProviderFamily::DeepSeek).unwrap();
        assert_eq!(deepseek.tool_presentation, ToolPresentation::Native);
        assert_eq!(deepseek.max_concurrent_reads, 2);
        assert!(deepseek.cache_read_telemetry);
        assert!(deepseek.benchmark_evidence_hash.is_none());

        let custom =
            ProviderProfile::safe_default("qwen2.5-coder", ProviderFamily::Custom).unwrap();
        assert_eq!(custom.tool_presentation, ToolPresentation::Native);
        assert_eq!(custom.max_concurrent_reads, 1);
        assert!(!custom.cache_read_telemetry);
        assert!(custom.benchmark_evidence_hash.is_none());
    }

    #[test]
    fn unknown_provider_fails_closed_on_production_path() {
        let family = infer_family("completely-unknown-model-xyz");
        assert_eq!(family, ProviderFamily::Custom);
        let profile =
            ProviderProfile::safe_default("completely-unknown-model-xyz", family).unwrap();
        assert_eq!(profile.tool_presentation, ToolPresentation::Native);
        assert_eq!(profile.max_concurrent_reads, 1);
        assert!(!profile.cache_read_telemetry);
        assert!(!profile.reasoning_round_trip);
        assert!(profile.benchmark_evidence_hash.is_none());

        let evidence = ToolModeEvidence {
            provider_catalog_key: "completely-unknown-model-xyz".into(),
            benchmark_id: "B-attempt".into(),
            requested_mode: ToolPresentation::Hybrid,
            trials: 30,
            baseline_correctness: 0.90,
            candidate_correctness: 0.91,
            median_latency_improvement: 0.20,
            write_order_drift_count: 0,
        };
        let upgraded = profile.apply_tool_mode_evidence(&evidence).unwrap();
        assert_eq!(upgraded.tool_presentation, ToolPresentation::Hybrid);
        assert!(upgraded.benchmark_evidence_hash.is_some());
    }

    #[test]
    fn stable_prefix_changes_when_schema_or_memory_changes() {
        let profile =
            ProviderProfile::safe_default("deepseek:chat", ProviderFamily::DeepSeek).unwrap();
        let a = profile.stable_prefix_fingerprint("system", "schema-a", Some("memory"));
        let b = profile.stable_prefix_fingerprint("system", "schema-b", Some("memory"));
        let c = profile.stable_prefix_fingerprint("system", "schema-a", None);
        assert_ne!(a, b);
        assert_ne!(a, c);
    }
}
