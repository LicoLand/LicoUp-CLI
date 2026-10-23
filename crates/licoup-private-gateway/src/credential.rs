//! LLM-provider credential domain for the private Gateway.
//!
//! The lease machinery — handoff document, process-bound lease, epoch
//! revocation, custody port — comes from `credential-lease`. This module
//! supplies the domain layer on top: which credential kind the Gateway
//! leases (`LlmApiKeyProvider`), how LLM API keys are inventoried and
//! administered, and the owner-facing validity choices.
//!
//! API-key bytes never appear in an inventory, configuration document, debug
//! projection, or command result.

use anyhow::{Result, anyhow, ensure};
use credential_lease::CredentialKind;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;

pub use credential_lease::LeaseEpochSource as GatewayCredentialEpochSource;
use licoup_foundation::core::secret_bytes::SecretBytes;

/// The credential kind the Gateway leases: provider API keys.
pub type GatewayCredential = credential_lease::LeasedCredential;
/// A custody-to-gateway handoff of LLM API keys.
pub type GatewayCredentialHandoff = credential_lease::CredentialHandoff<LlmApiKeyProvider>;
/// The process-bound LLM API key lease.
pub type GatewayCredentialLease = credential_lease::CredentialLease<LlmApiKeyProvider>;
/// The hot-swappable lease holder inside the running Gateway.
pub type GatewayCredentialSlot = credential_lease::CredentialSlot<LlmApiKeyProvider>;

pub const LLM_API_KEY_INVENTORY_SCHEMA: &str = "licoup.llm-api-key-inventory.v1";
pub const GATEWAY_CREDENTIAL_HANDOFF_SCHEMA: &str = "licoup.llm-gateway-credential-handoff.v3";
pub const MAX_LLM_API_KEYS: usize = 64;
pub const MAX_LLM_API_KEY_BYTES: usize = 8 * 1024;
pub const MAX_LLM_API_KEY_LABEL_BYTES: usize = 96;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GatewayCredentialChange {
    CredentialCreated,
    CredentialDeleted,
    LeaseDaysChanged,
}

impl GatewayCredentialChange {
    pub fn revokes_active_leases(self) -> bool {
        matches!(self, Self::CredentialCreated | Self::CredentialDeleted)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LlmApiKeyProvider {
    Kimi,
    DeepSeek,
    Kilo,
}

impl LlmApiKeyProvider {
    pub const ALL: [Self; 3] = [Self::Kimi, Self::DeepSeek, Self::Kilo];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Kimi => "kimi",
            Self::DeepSeek => "deepseek",
            Self::Kilo => "kilo",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Kimi => "Kimi",
            Self::DeepSeek => "DeepSeek",
            Self::Kilo => "Kilo",
        }
    }
}

impl CredentialKind for LlmApiKeyProvider {
    const HANDOFF_SCHEMA: &'static str = GATEWAY_CREDENTIAL_HANDOFF_SCHEMA;

    fn as_str(self) -> &'static str {
        LlmApiKeyProvider::as_str(self)
    }

    fn from_str(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "u16", into = "u16")]
pub enum GatewayCredentialLeaseDays {
    Seven,
    Thirty,
    Sixty,
    Ninety,
    OneEighty,
    ThreeSixtyFive,
}

impl GatewayCredentialLeaseDays {
    pub fn days(self) -> u16 {
        match self {
            Self::Seven => 7,
            Self::Thirty => 30,
            Self::Sixty => 60,
            Self::Ninety => 90,
            Self::OneEighty => 180,
            Self::ThreeSixtyFive => 365,
        }
    }

    pub fn duration(self) -> Duration {
        Duration::from_secs(u64::from(self.days()) * 24 * 60 * 60)
    }
}

impl Default for GatewayCredentialLeaseDays {
    fn default() -> Self {
        Self::Seven
    }
}

impl TryFrom<u16> for GatewayCredentialLeaseDays {
    type Error = &'static str;

    fn try_from(value: u16) -> std::result::Result<Self, Self::Error> {
        match value {
            7 => Ok(Self::Seven),
            30 => Ok(Self::Thirty),
            60 => Ok(Self::Sixty),
            90 => Ok(Self::Ninety),
            180 => Ok(Self::OneEighty),
            365 => Ok(Self::ThreeSixtyFive),
            _ => Err("llm_api_key_lease_days_invalid"),
        }
    }
}

impl From<GatewayCredentialLeaseDays> for u16 {
    fn from(value: GatewayCredentialLeaseDays) -> Self {
        value.days()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LlmApiKeyMetadata {
    pub credential_id: String,
    pub provider: LlmApiKeyProvider,
    pub label: String,
    pub created_at_epoch_seconds: u64,
    /// Per-key storage validity chosen at creation time. Legacy entries
    /// written before validity existed deserialize as `None` and are treated
    /// as non-expiring until the owner sets a period through an update.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_epoch_seconds: Option<u64>,
}

impl LlmApiKeyMetadata {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            uuid::Uuid::parse_str(&self.credential_id)
                .is_ok_and(|value| value.to_string() == self.credential_id),
            "llm_api_key_credential_id_invalid"
        );
        validate_label(&self.label)?;
        ensure!(
            self.created_at_epoch_seconds > 0,
            "llm_api_key_created_at_invalid"
        );
        if let Some(expires_at) = self.expires_at_epoch_seconds {
            ensure!(expires_at > 0, "llm_api_key_expires_at_invalid");
        }
        Ok(())
    }

    pub fn is_expired(&self, now_epoch_seconds: u64) -> bool {
        self.expires_at_epoch_seconds
            .is_some_and(|expires_at| now_epoch_seconds >= expires_at)
    }

    /// Applies a rename and/or a validity extension. The new expiry counts
    /// from the later of "now" and the current expiry so extending an
    /// already-expired key reactivates it for the chosen full period.
    pub fn apply_update(
        &mut self,
        update: &LlmApiKeyCredentialUpdate,
        now_epoch_seconds: u64,
    ) -> Result<()> {
        if let Some(label) = &update.label {
            validate_label(label)?;
            self.label = label.clone();
        }
        if let Some(extension) = update.extension {
            let base = self
                .expires_at_epoch_seconds
                .map_or(now_epoch_seconds, |expires_at| {
                    expires_at.max(now_epoch_seconds)
                });
            self.expires_at_epoch_seconds = Some(
                base.checked_add(extension.duration().as_secs())
                    .ok_or_else(|| anyhow!("llm_api_key_expires_at_invalid"))?,
            );
        }
        self.validate()
    }
}

/// Owner-initiated metadata change: a new label and/or a validity extension.
/// At least one field must be present.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LlmApiKeyCredentialUpdate {
    pub label: Option<String>,
    pub extension: Option<GatewayCredentialLeaseDays>,
}

impl LlmApiKeyCredentialUpdate {
    pub fn new(
        label: Option<String>,
        extension: Option<GatewayCredentialLeaseDays>,
    ) -> Result<Self> {
        ensure!(
            label.is_some() || extension.is_some(),
            "llm_api_key_update_empty"
        );
        if let Some(label) = &label {
            validate_label(label)?;
        }
        Ok(Self { label, extension })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmApiKeyInventory {
    pub schema_version: &'static str,
    pub lease_days: u16,
    pub entries: Vec<LlmApiKeyMetadata>,
}

impl LlmApiKeyInventory {
    pub fn new(
        lease_days: GatewayCredentialLeaseDays,
        mut entries: Vec<LlmApiKeyMetadata>,
    ) -> Result<Self> {
        ensure!(
            entries.len() <= MAX_LLM_API_KEYS,
            "llm_api_key_inventory_capacity_exceeded"
        );
        entries.sort_by(|left, right| {
            left.provider
                .cmp(&right.provider)
                .then_with(|| {
                    left.created_at_epoch_seconds
                        .cmp(&right.created_at_epoch_seconds)
                })
                .then_with(|| left.credential_id.cmp(&right.credential_id))
        });
        for entry in &entries {
            entry.validate()?;
        }
        Ok(Self {
            schema_version: LLM_API_KEY_INVENTORY_SCHEMA,
            lease_days: lease_days.days(),
            entries,
        })
    }
}

pub struct NewLlmApiKey {
    provider: LlmApiKeyProvider,
    label: String,
    validity: GatewayCredentialLeaseDays,
    secret: SecretBytes,
}

impl NewLlmApiKey {
    pub fn new(
        provider: LlmApiKeyProvider,
        label: String,
        api_key: String,
        validity: GatewayCredentialLeaseDays,
    ) -> Result<Self> {
        validate_label(&label)?;
        validate_api_key(&api_key)?;
        let secret = SecretBytes::try_from_string(api_key)
            .map_err(|_| anyhow!("llm_api_key_secret_invalid"))?;
        Ok(Self {
            provider,
            label,
            validity,
            secret,
        })
    }

    pub fn provider(&self) -> LlmApiKeyProvider {
        self.provider
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn validity(&self) -> GatewayCredentialLeaseDays {
        self.validity
    }

    pub fn into_secret(self) -> SecretBytes {
        self.secret
    }
}

impl fmt::Debug for NewLlmApiKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NewLlmApiKey")
            .field("provider", &self.provider)
            .field("label", &self.label)
            .field("secret", &"[redacted]")
            .finish()
    }
}

fn validate_label(label: &str) -> Result<()> {
    ensure!(
        label == label.trim()
            && !label.is_empty()
            && label.len() <= MAX_LLM_API_KEY_LABEL_BYTES
            && !label.chars().any(char::is_control),
        "llm_api_key_label_invalid"
    );
    Ok(())
}

fn validate_api_key(api_key: &str) -> Result<()> {
    ensure!(
        api_key == api_key.trim()
            && (12..=MAX_LLM_API_KEY_BYTES).contains(&api_key.len())
            && api_key.bytes().all(|byte| (0x21..=0x7e).contains(&byte)),
        "llm_api_key_secret_invalid"
    );
    Ok(())
}
