//! Pinned log authority, signed tree-head verification, and freshness policy.

use anyhow::{Result, anyhow, ensure};
use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};

use super::constants::{
    KT_JSON_SAFE_INTEGER_MAX, KT_PROTOCOL_MAX_FUTURE_SKEW_SECONDS, KT_PROTOCOL_MAX_STH_AGE_SECONDS,
    SECURE_MESH_KT_PROTOCOL_VERSION,
};
use super::json_codec::{
    hex_encode, parse_signature, sth_sign_payload, validate_hex_hash, validate_text,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum KtAuthorityProvenance {
    /// A public key explicitly configured by the user or local administrator. This verifies
    /// cryptography but does not by itself prove who operates the service.
    UserConfiguredExternal,
    #[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
    LocalAcceptanceMock,
}

impl KtAuthorityProvenance {
    pub fn stable_code(&self) -> &'static str {
        match self {
            Self::UserConfiguredExternal => "user-configured-external",
            #[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
            Self::LocalAcceptanceMock => "local-acceptance-mock",
        }
    }

    pub fn is_mock(&self) -> bool {
        match self {
            Self::UserConfiguredExternal => false,
            #[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
            Self::LocalAcceptanceMock => true,
        }
    }

    /// A caller-supplied Ed25519 key proves signatures, not operator identity. Production
    /// service provenance remains false until a separately signed/release-pinned authority
    /// descriptor is implemented and verified.
    pub fn production_service_claim_allowed(&self) -> bool {
        false
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PinnedKtLogKey {
    log_id: String,
    key_id: String,
    public_key: [u8; 32],
    provenance: KtAuthorityProvenance,
}

impl PinnedKtLogKey {
    pub fn from_user_configured_ed25519_bytes(
        log_id: impl Into<String>,
        key_id: impl Into<String>,
        public_key: [u8; 32],
    ) -> Result<Self> {
        let value = Self {
            log_id: log_id.into(),
            key_id: key_id.into(),
            public_key,
            provenance: KtAuthorityProvenance::UserConfiguredExternal,
        };
        validate_text("log_id", &value.log_id)?;
        validate_text("key_id", &value.key_id)?;
        VerifyingKey::from_bytes(&value.public_key)
            .map_err(|_| anyhow!("secure mesh KT pinned public key is invalid"))?;
        Ok(value)
    }

    #[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
    pub fn from_acceptance_mock_ed25519_bytes(
        log_id: impl Into<String>,
        key_id: impl Into<String>,
        public_key: [u8; 32],
    ) -> Result<Self> {
        let mut value = Self::from_user_configured_ed25519_bytes(log_id, key_id, public_key)?;
        value.provenance = KtAuthorityProvenance::LocalAcceptanceMock;
        Ok(value)
    }

    pub fn log_id(&self) -> &str {
        &self.log_id
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    pub fn provenance(&self) -> &KtAuthorityProvenance {
        &self.provenance
    }

    pub fn public_key_hex(&self) -> String {
        hex_encode(&self.public_key)
    }

    fn verifying_key(&self) -> Result<VerifyingKey> {
        VerifyingKey::from_bytes(&self.public_key)
            .map_err(|_| anyhow!("secure mesh KT pinned public key is invalid"))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KtFreshnessPolicy {
    pub max_sth_age_seconds: u64,
    pub max_future_skew_seconds: u64,
}

impl KtFreshnessPolicy {
    pub fn strict(max_sth_age_seconds: u64, max_future_skew_seconds: u64) -> Result<Self> {
        ensure!(
            max_sth_age_seconds > 0,
            "secure mesh KT maximum STH age must be positive"
        );
        ensure!(
            max_sth_age_seconds <= KT_PROTOCOL_MAX_STH_AGE_SECONDS,
            "secure mesh KT maximum STH age exceeds the protocol hard limit"
        );
        ensure!(
            max_future_skew_seconds <= KT_PROTOCOL_MAX_FUTURE_SKEW_SECONDS,
            "secure mesh KT maximum future skew exceeds the protocol hard limit"
        );
        Ok(Self {
            max_sth_age_seconds,
            max_future_skew_seconds,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedKtFreshness {
    pub observed_at_epoch_seconds: u64,
    pub issued_at_epoch_seconds: u64,
    pub max_sth_age_seconds: u64,
    pub max_future_skew_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SecureMeshSignedTreeHead {
    pub protocol_version: String,
    pub log_id: String,
    pub key_id: String,
    pub tree_size: u64,
    pub root_hash: String,
    pub map_root_hash: String,
    pub issued_at_epoch_seconds: u64,
    pub signature: String,
}

impl SecureMeshSignedTreeHead {
    pub fn verify(
        &self,
        pin: &PinnedKtLogKey,
        freshness_policy: KtFreshnessPolicy,
        now_epoch_seconds: u64,
    ) -> Result<VerifiedKtFreshness> {
        self.verify_authenticity(pin)?;
        self.verify_freshness(freshness_policy, now_epoch_seconds)
    }

    pub(super) fn verify_authenticity(&self, pin: &PinnedKtLogKey) -> Result<()> {
        ensure!(
            self.protocol_version == SECURE_MESH_KT_PROTOCOL_VERSION,
            "secure mesh KT protocol version is unsupported"
        );
        ensure!(
            self.log_id == pin.log_id,
            "secure mesh KT log id is not pinned"
        );
        ensure!(
            self.key_id == pin.key_id,
            "secure mesh KT key id is not pinned"
        );
        ensure!(
            self.tree_size <= KT_JSON_SAFE_INTEGER_MAX
                && self.issued_at_epoch_seconds <= KT_JSON_SAFE_INTEGER_MAX,
            "secure mesh KT signed tree head integer exceeds the cross-language safe range"
        );
        validate_hex_hash("root_hash", &self.root_hash)?;
        validate_hex_hash("map_root_hash", &self.map_root_hash)?;
        let payload = sth_sign_payload(self)?;
        let signature = parse_signature(&self.signature)?;
        pin.verifying_key()?
            .verify_strict(&payload, &signature)
            .map_err(|_| anyhow!("secure mesh KT signed tree head signature is invalid"))?;
        Ok(())
    }

    pub(super) fn verify_freshness(
        &self,
        freshness_policy: KtFreshnessPolicy,
        now_epoch_seconds: u64,
    ) -> Result<VerifiedKtFreshness> {
        ensure!(
            self.issued_at_epoch_seconds
                <= now_epoch_seconds.saturating_add(freshness_policy.max_future_skew_seconds),
            "secure mesh KT signed tree head is from the future"
        );
        ensure!(
            now_epoch_seconds
                <= self
                    .issued_at_epoch_seconds
                    .saturating_add(freshness_policy.max_sth_age_seconds),
            "secure mesh KT signed tree head is stale"
        );
        Ok(VerifiedKtFreshness {
            observed_at_epoch_seconds: now_epoch_seconds,
            issued_at_epoch_seconds: self.issued_at_epoch_seconds,
            max_sth_age_seconds: freshness_policy.max_sth_age_seconds,
            max_future_skew_seconds: freshness_policy.max_future_skew_seconds,
        })
    }
}
