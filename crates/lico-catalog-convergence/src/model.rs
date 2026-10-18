use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const CATALOG_CONVERGENCE_SCHEMA: &str = "v0.0.1:licoup:catalog-convergence-1";
pub const OFFICIAL_CLIENT_RECEIPT_SCHEMA: &str =
    "v0.0.1:upstream-gateway:official-client-receipt-1";

pub const ALLOWED_CLIENT_TARGETS: &[&str] = &["macos", "linux", "windows", "ios", "android"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClientTarget {
    Macos,
    Linux,
    Windows,
    Ios,
    Android,
}

impl ClientTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Macos => "macos",
            Self::Linux => "linux",
            Self::Windows => "windows",
            Self::Ios => "ios",
            Self::Android => "android",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "macos" => Some(Self::Macos),
            "linux" => Some(Self::Linux),
            "windows" => Some(Self::Windows),
            "ios" => Some(Self::Ios),
            "android" => Some(Self::Android),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogToolEntry {
    pub name: String,
    #[serde(default, flatten)]
    pub descriptor: BTreeMap<String, Value>,
}

impl CatalogToolEntry {
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            descriptor: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogFetchedSnapshot {
    pub source_revision: i64,
    pub catalog_revision: String,
    pub audience_revision: i64,
    pub tools: Vec<CatalogToolEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogSnapshot {
    pub partition_key: String,
    pub source_revision: i64,
    pub catalog_revision: String,
    pub audience_revision: i64,
    pub tools: Vec<CatalogToolEntry>,
    pub tool_count: usize,
    pub fetched_at: String,
    pub digest: String,
}

impl CatalogSnapshot {
    pub fn from_fetched(
        partition_key: &str,
        fetched: &CatalogFetchedSnapshot,
        fetched_at: &str,
    ) -> Self {
        let partition_key = partition_key.trim().to_string();
        let catalog_revision = fetched.catalog_revision.trim().to_string();
        let tools = fetched.tools.clone();
        let tool_count = tools.len();
        let digest = digest_catalog_snapshot(
            &partition_key,
            fetched.source_revision,
            &catalog_revision,
            fetched.audience_revision,
            &tools,
        );
        Self {
            partition_key,
            source_revision: fetched.source_revision,
            catalog_revision,
            audience_revision: fetched.audience_revision,
            tools,
            tool_count,
            fetched_at: fetched_at.to_string(),
            digest,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogPullContext {
    pub partition_key: String,
    pub pending_invalidation: Option<PendingInvalidation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingInvalidation {
    pub partition_key: String,
    pub audience_revision: i64,
    pub catalog_revision: String,
    pub source_revision: i64,
    pub reason_code: String,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InvalidationNotification {
    #[serde(default)]
    pub affected_partitions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition_key: Option<String>,
    pub source_revision: i64,
    pub catalog_revision: String,
    pub audience_revision: i64,
    #[serde(default)]
    pub reason_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InvalidationResult {
    pub accepted_partition_keys: Vec<String>,
    pub pending_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RefreshOutcomeKind {
    Replaced,
    Unchanged,
    FetchFailed,
    RejectedStale,
    RejectedConflict,
    RejectedCapacity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RefreshResult {
    pub outcome: RefreshOutcomeKind,
    pub partition_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<CatalogSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retained: Option<CatalogSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CohortOutcome {
    Pending,
    Applied,
    Disconnected,
    Fenced,
}

impl CohortOutcome {
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Applied => "applied",
            Self::Disconnected => "disconnected",
            Self::Fenced => "fenced",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CohortEntry {
    pub partition_key: String,
    pub outcome: CohortOutcome,
    pub audience_revision: i64,
    pub catalog_revision: String,
    pub source_revision: i64,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiscoveryResult {
    pub ok: bool,
    pub reason_code: String,
    pub tools: Vec<CatalogToolEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_revision: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audience_revision: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutcomeRecord {
    pub ok: bool,
    pub reason_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OfficialClientReceipt {
    pub schema_version: String,
    pub target: String,
    pub platform: String,
    pub runtime: String,
    pub source_digest: String,
    pub negotiated_capability: String,
    pub opaque_partition_key: String,
    pub source_revision: i64,
    pub catalog_revision: String,
    pub audience_revision: i64,
    pub applied_revision: i64,
    pub cache_digest: String,
    pub cohort_outcome: String,
    pub ui_observed_revision: i64,
    pub restart_result: OutcomeRecord,
    pub privacy_result: OutcomeRecord,
    pub summary_digest: String,
    pub receipt_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
}

pub fn sha256_hex(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn is_hex_digest(value: &str) -> bool {
    let normalized = value.trim();
    normalized.len() == 64
        && normalized
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() && !ch.is_uppercase())
}

pub fn is_opaque_partition_key(value: &str) -> bool {
    let normalized = value.trim();
    is_hex_digest(normalized) || normalized.len() >= 43
}

pub fn digest_catalog_snapshot(
    partition_key: &str,
    source_revision: i64,
    catalog_revision: &str,
    audience_revision: i64,
    tools: &[CatalogToolEntry],
) -> String {
    let tools_value = Value::Array(
        tools
            .iter()
            .map(|tool| serde_json::to_value(tool).unwrap_or(Value::Null))
            .collect(),
    );
    let payload = canonical_json(&BTreeMap::from([
        (
            "audienceRevision".to_string(),
            Value::from(audience_revision),
        ),
        (
            "catalogRevision".to_string(),
            Value::String(catalog_revision.trim().to_string()),
        ),
        (
            "partitionKey".to_string(),
            Value::String(partition_key.trim().to_string()),
        ),
        ("sourceRevision".to_string(), Value::from(source_revision)),
        ("tools".to_string(), tools_value),
    ]));
    sha256_hex(&payload)
}

pub(crate) fn canonical_json(map: &BTreeMap<String, Value>) -> String {
    serde_json::to_string(map).unwrap_or_else(|_| "{}".to_string())
}

pub(crate) fn revision_number(value: i64) -> i64 {
    if value >= 0 { value } else { -1 }
}

pub(crate) fn is_stale_candidate(
    current: &CatalogSnapshot,
    candidate_source: i64,
    candidate_audience: i64,
    candidate_catalog: &str,
) -> bool {
    if candidate_audience >= 0 && current.audience_revision >= 0 {
        if candidate_audience < current.audience_revision {
            return true;
        }
        if candidate_audience == current.audience_revision
            && !candidate_catalog.trim().is_empty()
            && !current.catalog_revision.trim().is_empty()
            && candidate_catalog.trim() != current.catalog_revision.trim()
            && candidate_source < current.source_revision
        {
            return true;
        }
    }
    candidate_source >= 0
        && current.source_revision >= 0
        && candidate_source < current.source_revision
}
