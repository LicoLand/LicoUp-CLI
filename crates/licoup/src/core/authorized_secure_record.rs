//! Generic authority for user-present, versioned local security records.
//!
//! Domain state may project these records for UI, but it must never become the
//! authority. Platform adapters keep the canonical record in non-exportable,
//! user-authorized storage and issue operation-bound, short-lived grants.

use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::any::Any;
use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

const RECORD_SCHEMA: &str = "licoup.authorized-secure-record.v1";

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SecureRecordLocator {
    namespace: String,
    key: String,
}

impl SecureRecordLocator {
    pub fn new(namespace: impl Into<String>, key: impl Into<String>) -> Result<Self> {
        let namespace = namespace.into();
        let key = key.into();
        for value in [&namespace, &key] {
            ensure!(
                !value.is_empty()
                    && value.len() <= 192
                    && value.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
                    }),
                "authorized_secure_record_locator_invalid"
            );
        }
        Ok(Self { namespace, key })
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn key(&self) -> &str {
        &self.key
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecureRecordOperation {
    Read,
    RecoverRead,
    Create,
    Replace,
    Delete,
    ConsumeOneShot,
}

impl SecureRecordOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::RecoverRead => "recover-read",
            Self::Create => "create",
            Self::Replace => "replace",
            Self::Delete => "delete",
            Self::ConsumeOneShot => "consume-one-shot",
        }
    }
}

#[derive(Clone, Debug)]
pub struct SecureRecordAuthorizationRequest {
    pub(crate) locator: SecureRecordLocator,
    pub(crate) operation: SecureRecordOperation,
    pub(crate) target_digest_sha256: String,
    pub(crate) expected_prior_version: u64,
    pub(crate) expected_prior_digest_sha256: Option<String>,
    pub(crate) nonce: String,
    pub(crate) reason: String,
    pub(crate) ttl: Duration,
    pub(crate) operation_budget: usize,
    pub(crate) scope_bindings: BTreeMap<String, String>,
}

impl SecureRecordAuthorizationRequest {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        locator: SecureRecordLocator,
        operation: SecureRecordOperation,
        target_digest_sha256: String,
        expected_prior_version: u64,
        expected_prior_digest_sha256: Option<String>,
        nonce: String,
        reason: String,
        ttl: Duration,
        operation_budget: usize,
        scope_bindings: BTreeMap<String, String>,
    ) -> Result<Self> {
        let request = Self {
            locator,
            operation,
            target_digest_sha256,
            expected_prior_version,
            expected_prior_digest_sha256,
            nonce,
            reason,
            ttl,
            operation_budget,
            scope_bindings,
        };
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            is_sha256(&self.target_digest_sha256)
                && self
                    .expected_prior_digest_sha256
                    .as_deref()
                    .is_none_or(is_sha256)
                && uuid::Uuid::parse_str(&self.nonce)
                    .is_ok_and(|value| value.to_string() == self.nonce)
                && self.reason == self.reason.trim()
                && !self.reason.is_empty()
                && self.reason.len() <= 512
                && !self.reason.chars().any(char::is_control)
                && self.ttl >= Duration::from_secs(1)
                && self.ttl <= Duration::from_secs(120)
                && (1..=8).contains(&self.operation_budget)
                && self.scope_bindings.len() <= 16
                && self.scope_bindings.iter().all(|(key, value)| {
                    !key.is_empty()
                        && key.len() <= 64
                        && key.bytes().all(|byte| {
                            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
                        })
                        && value == value.trim()
                        && !value.is_empty()
                        && value.len() <= 512
                        && !value.chars().any(char::is_control)
                }),
            "authorized_secure_record_authorization_request_invalid"
        );
        match self.operation {
            SecureRecordOperation::Create | SecureRecordOperation::RecoverRead => ensure!(
                self.expected_prior_version == 0 && self.expected_prior_digest_sha256.is_none(),
                "authorized_secure_record_prior_binding_invalid"
            ),
            _ => ensure!(
                self.expected_prior_version > 0 && self.expected_prior_digest_sha256.is_some(),
                "authorized_secure_record_prior_binding_invalid"
            ),
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct AuthorizedSecureRecordGrant {
    request: SecureRecordAuthorizationRequest,
    issuer: &'static str,
    issued_for_process: u32,
    expires_at: Instant,
    consumed: Arc<AtomicUsize>,
    platform_context: Arc<dyn Any + Send + Sync>,
}

impl AuthorizedSecureRecordGrant {
    pub(crate) fn issue<T>(
        request: SecureRecordAuthorizationRequest,
        issuer: &'static str,
        platform_context: T,
    ) -> Result<Self>
    where
        T: Any + Send + Sync,
    {
        request.validate()?;
        let expires_at = Instant::now()
            .checked_add(request.ttl)
            .ok_or_else(|| anyhow!("authorized_secure_record_authorization_expiry_invalid"))?;
        Ok(Self {
            request,
            issuer,
            issued_for_process: std::process::id(),
            expires_at,
            consumed: Arc::new(AtomicUsize::new(0)),
            platform_context: Arc::new(platform_context),
        })
    }

    pub(crate) fn claim(
        &self,
        issuer: &'static str,
        locator: &SecureRecordLocator,
        operation: SecureRecordOperation,
        target_digest: &str,
        expected_prior_version: u64,
        expected_prior_digest: Option<&str>,
    ) -> Result<()> {
        ensure!(
            self.issuer == issuer
                && self.issued_for_process == std::process::id()
                && Instant::now() <= self.expires_at
                && self.request.locator == *locator
                && self.request.operation == operation
                && self.request.target_digest_sha256 == target_digest
                && self.request.expected_prior_version == expected_prior_version
                && self.request.expected_prior_digest_sha256.as_deref() == expected_prior_digest,
            "authorized_secure_record_grant_binding_mismatch"
        );
        let previous = self.consumed.fetch_add(1, Ordering::SeqCst);
        ensure!(
            previous < self.request.operation_budget,
            "authorized_secure_record_grant_consumed"
        );
        Ok(())
    }

    pub(crate) fn platform_context<T>(&self) -> Result<&T>
    where
        T: Any + Send + Sync,
    {
        self.platform_context
            .downcast_ref::<T>()
            .ok_or_else(|| anyhow!("authorized_secure_record_grant_context_invalid"))
    }

    pub fn nonce(&self) -> &str {
        &self.request.nonce
    }

    pub fn scope_binding(&self, key: &str) -> Option<&str> {
        self.request.scope_bindings.get(key).map(String::as_str)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct VersionedSecureRecord {
    schema_version: String,
    version: u64,
    previous_record_digest_sha256: Option<String>,
    payload: String,
    payload_digest_sha256: String,
    record_digest_sha256: String,
}

impl VersionedSecureRecord {
    pub fn new(
        version: u64,
        previous_record_digest_sha256: Option<String>,
        payload: String,
    ) -> Result<Self> {
        ensure!(
            version > 0
                && previous_record_digest_sha256
                    .as_deref()
                    .is_none_or(is_sha256)
                && !payload.is_empty()
                && payload.len() <= 256 * 1024,
            "authorized_secure_record_invalid"
        );
        let payload_digest_sha256 = format!("{:x}", Sha256::digest(payload.as_bytes()));
        let record_digest_sha256 = record_digest(
            version,
            previous_record_digest_sha256.as_deref(),
            &payload_digest_sha256,
        );
        Ok(Self {
            schema_version: RECORD_SCHEMA.to_owned(),
            version,
            previous_record_digest_sha256,
            payload,
            payload_digest_sha256,
            record_digest_sha256,
        })
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == RECORD_SCHEMA
                && self.version > 0
                && self
                    .previous_record_digest_sha256
                    .as_deref()
                    .is_none_or(is_sha256)
                && !self.payload.is_empty()
                && self.payload.len() <= 256 * 1024
                && format!("{:x}", Sha256::digest(self.payload.as_bytes()))
                    == self.payload_digest_sha256
                && record_digest(
                    self.version,
                    self.previous_record_digest_sha256.as_deref(),
                    &self.payload_digest_sha256,
                ) == self.record_digest_sha256,
            "authorized_secure_record_invalid"
        );
        Ok(())
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn previous_record_digest_sha256(&self) -> Option<&str> {
        self.previous_record_digest_sha256.as_deref()
    }

    pub fn payload(&self) -> &str {
        &self.payload
    }

    pub fn payload_digest_sha256(&self) -> &str {
        &self.payload_digest_sha256
    }

    pub fn record_digest_sha256(&self) -> &str {
        &self.record_digest_sha256
    }
}

pub trait AuthorizedSecureRecordStore: Send + Sync {
    fn backend(&self) -> &'static str;
    fn user_presence_available(&self) -> bool;

    fn authorize(
        &self,
        request: SecureRecordAuthorizationRequest,
    ) -> Result<AuthorizedSecureRecordGrant>;

    fn read(
        &self,
        grant: &AuthorizedSecureRecordGrant,
        locator: &SecureRecordLocator,
        expected_version: u64,
        expected_digest_sha256: &str,
    ) -> Result<VersionedSecureRecord>;

    fn read_current(
        &self,
        grant: &AuthorizedSecureRecordGrant,
        locator: &SecureRecordLocator,
        recovery_scope_digest_sha256: &str,
    ) -> Result<VersionedSecureRecord>;

    fn compare_and_swap(
        &self,
        grant: &AuthorizedSecureRecordGrant,
        locator: &SecureRecordLocator,
        expected: Option<&VersionedSecureRecord>,
        replacement: &VersionedSecureRecord,
    ) -> Result<()>;

    fn delete(
        &self,
        grant: &AuthorizedSecureRecordGrant,
        locator: &SecureRecordLocator,
        expected: &VersionedSecureRecord,
    ) -> Result<()>;

    fn consume_one_shot(
        &self,
        grant: &AuthorizedSecureRecordGrant,
        locator: &SecureRecordLocator,
        expected: &VersionedSecureRecord,
    ) -> Result<VersionedSecureRecord>;
}

fn record_digest(version: u64, previous: Option<&str>, payload_digest: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"LICOUP-AUTHORIZED-SECURE-RECORD-V1\0");
    hasher.update(version.to_be_bytes());
    for field in [previous.unwrap_or(""), payload_digest] {
        hasher.update((field.len() as u64).to_be_bytes());
        hasher.update(field.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}
