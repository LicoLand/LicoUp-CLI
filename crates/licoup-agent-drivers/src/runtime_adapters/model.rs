use licoup_agent_adapter_sdk::Transition;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct RuntimeDriverProfile {
    pub driver_status: String,
    pub readiness: String,
    pub protocol: String,
    pub blocker: Option<String>,
    pub capability_matrix: Option<Value>,
    pub summary_codes: Vec<String>,
    pub consecutive_passes: usize,
    pub evidence_age_class: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DriverInventoryDocument {
    pub schema_version: String,
    pub contract_version: String,
    pub evidence_contract: DriverEvidenceContract,
    pub drivers: Vec<DriverInventoryEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DriverEvidenceContract {
    pub minimum_consecutive_passes: usize,
    pub core_checks: Vec<String>,
    pub conditional_checks: Vec<String>,
    pub required_booleans: Vec<String>,
    pub required_counts: Vec<String>,
    pub required_digests: Vec<String>,
    pub required_bindings: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DriverInventoryEntry {
    pub agent_id: String,
    pub driver_id: String,
    pub runtime_protocol: String,
    pub official_native_lane_kind: String,
    pub history_readable: bool,
    pub driver_mode: String,
    pub blocker_codes: Vec<String>,
    #[serde(default)]
    pub capability_matrix: Option<Value>,
    // Consumed by the support-matrix projection
    // (`tools/scripts/client-support-matrix.mjs`); the runtime only enforces
    // the contract shape so unobservable stages stay explicitly declared.
    #[allow(dead_code)]
    pub lifecycle_evidence: LifecycleEvidence,
}

/// Per-driver declaration of which conversation lifecycle stages carry native
/// evidence.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(dead_code)]
pub struct LifecycleEvidence {
    pub accepted: bool,
    pub processing: bool,
    pub responding: bool,
    pub completed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeCapabilityDocument {
    pub schema_version: String,
    pub agents: Vec<NativeCapabilityEntry>,
    /// Quota-capability flags naming the agents that have a provider quota
    /// source. Additive metadata consumed by the provider-quota domain; the
    /// capability-kind allowlist is unchanged by these flags.
    #[serde(default)]
    #[allow(dead_code)]
    pub quota_sources: Vec<NativeCapabilityQuotaSource>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(dead_code)]
pub struct NativeCapabilityQuotaSource {
    pub agent_id: String,
    pub provider: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeCapabilityEntry {
    pub agent_id: String,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadinessDocument {
    pub schema_version: String,
    pub contract_version: String,
    pub minimum_consecutive_passes: usize,
    pub summary: ReadinessSummary,
    pub adapters: Vec<ReadinessEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadinessSummary {
    pub total: usize,
    pub ready: usize,
    pub partial: usize,
    pub failed: usize,
    pub blocked: usize,
    pub unverified: usize,
    pub history_only: usize,
    pub send_enabled: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadinessEntry {
    pub agent_id: String,
    pub status: String,
    pub send_enabled: bool,
    pub official_native_lane_proven: bool,
    pub conversation_gate_passed: bool,
    pub cleanup_passed: bool,
    pub privacy_passed: bool,
    pub consecutive_passes: usize,
    pub core_checks: CoreReadinessCounts,
    pub conditional_checks: ConditionalReadinessCounts,
    pub evidence_binding: Option<ReadinessEvidenceBinding>,
    pub summary_codes: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadinessEvidenceBinding {
    pub agent_id: String,
    pub driver_id: String,
    pub runtime_protocol: String,
    pub harness_version: String,
    pub runtime_version_class: String,
    pub runtime_version_digest: String,
    pub capability_snapshot_digest: String,
    pub adapter_manifest_digest: String,
    pub release_artifact_digest: String,
    pub release_sidecar_digest: String,
    pub product_continuity_binding_digest: String,
    pub runtime_source_class: String,
    pub registry_digest: String,
    pub driver_inventory_digest: String,
    pub evidence_digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreReadinessCounts {
    pub required: usize,
    pub passed: usize,
    pub failed: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConditionalReadinessCounts {
    pub total: usize,
    pub native_supported: usize,
    pub passed: usize,
    pub gaps: usize,
    pub failed: usize,
}

#[derive(Debug)]
pub struct RuntimeDriverRegistry {
    pub drivers: BTreeMap<String, DriverInventoryEntry>,
    pub readiness: BTreeMap<String, ReadinessEntry>,
}

#[derive(Clone, Debug, Default)]
pub struct NormalizedEffectiveSettings {
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub permission_mode: Option<String>,
    pub mode: Option<String>,
    pub runtime_agent: Option<String>,
    pub allow_all: Option<bool>,
    pub sandbox: Option<Value>,
    pub approval_policy: Option<Value>,
}

#[derive(Clone, Debug)]
pub struct NormalizedFailure {
    pub code: String,
    pub message: String,
    pub stage: String,
    pub component: Option<String>,
    pub retryable: Option<bool>,
    pub recovery: Option<String>,
    pub user_interaction_required: bool,
    pub request_method: Option<String>,
    pub session_id: Option<String>,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub turn_status: Option<String>,
}

#[derive(Debug)]
pub struct NormalizedExecution {
    pub ok: bool,
    pub output: String,
    pub transitions: Vec<Transition>,
    pub capabilities: Value,
    pub error: Option<NormalizedFailure>,
    pub session_id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub turn_status: String,
    pub effective: NormalizedEffectiveSettings,
    pub status_code: Option<i32>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub started_at: String,
    pub runtime_protocol: &'static str,
    pub driver_id: &'static str,
}
