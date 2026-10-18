//! Stable provider-quota snapshot contract and wire serialization.
//!
//! Every provider source normalizes into the shared snapshot type defined
//! here so the scheduler, retained store, and projection never branch on
//! provider identity beyond capability flags. No field of this contract may
//! carry credential material: snapshots, retained state, logs, and
//! diagnostics contain only quota metrics and identity labels.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(super) const SNAPSHOT_SCHEMA_VERSION: &str = "v0.0.1:provider-quota-snapshots-1";
pub(super) const SNAPSHOT_COLLECTION: &str = "provider-quota-snapshots";
pub(super) const MAX_RETAINED_PROVIDERS: usize = 30;
pub(super) const DEFAULT_STALE_AFTER_SECONDS: u64 = 3600;

/// Quota capability flags embedded in the packaged native-capability
/// inventory so the UI can tell which agents have a quota source.
const NATIVE_CAPABILITY_JSON: &str =
    include_str!("../../../resources/agent-native-capabilities.json");

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum QuotaProvider {
    Antigravity,
    Codex,
    Cursor,
    /// The Kimi Code CLI agent id is hyphenated; the wire value overrides the
    /// lowercase serde default.
    #[serde(rename = "kimi-code", alias = "kimicode")]
    KimiCode,
}

impl QuotaProvider {
    pub(super) fn wire_name(self) -> &'static str {
        match self {
            Self::Antigravity => "antigravity",
            Self::Codex => "codex",
            Self::Cursor => "cursor",
            Self::KimiCode => "kimi-code",
        }
    }

    /// The quota source serves the agent of the same id in this delivery.
    pub(super) fn agent_id(self) -> &'static str {
        self.wire_name()
    }

    pub(super) fn parse(agent_id: &str) -> Option<Self> {
        match agent_id.trim().to_ascii_lowercase().as_str() {
            "antigravity" => Some(Self::Antigravity),
            "codex" => Some(Self::Codex),
            "cursor" => Some(Self::Cursor),
            "kimi-code" | "kimicode" => Some(Self::KimiCode),
            _ => None,
        }
    }
}

/// The first-delivery provider set. Claude Code, Gemini, and Copilot slot
/// into this same contract later without wire changes.
pub(super) const QUOTA_PROVIDERS: &[QuotaProvider] = &[
    QuotaProvider::Antigravity,
    QuotaProvider::Codex,
    QuotaProvider::Cursor,
    QuotaProvider::KimiCode,
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum QuotaStatus {
    Live,
    Stale,
    Unavailable,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct QuotaWindow {
    pub(super) label: String,
    /// Raw provider value; may exceed 100. The UI clamps for display.
    pub(super) used_percent: f64,
    /// Absolute usage amounts for currency-metered plans, normalized to USD by
    /// the source so every provider shares one unit. Percent-only providers
    /// omit all three fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) used: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) limit: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) remaining: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) window_minutes: Option<u64>,
    /// RFC 3339; backfilled from the cached snapshot when a fetch omits it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) resets_at: Option<String>,
    #[serde(default)]
    pub(super) reset_description: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct QuotaIdentity {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) account_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) plan: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProviderQuotaSnapshot {
    pub(super) agent_id: String,
    pub(super) provider: QuotaProvider,
    pub(super) status: QuotaStatus,
    pub(super) windows: Vec<QuotaWindow>,
    #[serde(default)]
    pub(super) identity: QuotaIdentity,
    /// RFC 3339 UTC capture time of the data in this snapshot.
    pub(super) captured_at: String,
    pub(super) stale_after_seconds: u64,
}

impl ProviderQuotaSnapshot {
    pub(super) fn wire_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| Value::Null)
    }
}

/// One bounded fetch failure. Carries only a sanitized code; credential
/// material and endpoint payloads never appear in errors.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct QuotaFetchError {
    pub(super) code: &'static str,
}

impl QuotaFetchError {
    pub(super) fn new(code: &'static str) -> Self {
        Self { code }
    }
}

impl std::fmt::Display for QuotaFetchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "provider quota fetch failed ({})", self.code)
    }
}

impl std::error::Error for QuotaFetchError {}

/// Quota capability flag projected from the packaged native-capability
/// inventory. The inventory stays the canonical agent-capability document;
/// these flags only announce which agents have a quota source.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct QuotaCapability {
    pub(super) agent_id: String,
    pub(super) provider: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NativeCapabilityInventory {
    #[serde(default)]
    quota_sources: Vec<QuotaCapability>,
}

/// Read the quota-capability flags from the packaged inventory. Unknown or
/// unsupported providers are dropped so a drifting inventory can never
/// fabricate a quota source.
pub(super) fn quota_capabilities() -> Vec<QuotaCapability> {
    serde_json::from_str::<NativeCapabilityInventory>(NATIVE_CAPABILITY_JSON)
        .map(|inventory| {
            inventory
                .quota_sources
                .into_iter()
                .filter(|entry| QuotaProvider::parse(&entry.provider).is_some())
                .collect()
        })
        .unwrap_or_default()
}
