use std::fmt;
use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::core::secure_mesh_capability::CapabilityEvaluationReport;

pub const MAX_SECRET_STORE_PRESENCE_GRANT_TTL: Duration = Duration::from_secs(30);

const MAX_REASON_BYTES: usize = 512;
const MAX_NONCE_BYTES: usize = 256;
const MAX_SCOPE_COMPONENT_BYTES: usize = 512;
const MAX_BATCH_OPERATION_COUNT: usize = 4_096;

const GRANT_AVAILABLE: u8 = 0;
const GRANT_CONSUMED: u8 = 1;
const GRANT_EXPIRED: u8 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecretStorePresenceProvider {
    MacosKeychain,
    LinuxSecretService,
    WindowsCredentialManager,
}

impl SecretStorePresenceProvider {
    fn tag(self) -> &'static [u8] {
        match self {
            Self::MacosKeychain => b"macos-keychain",
            Self::LinuxSecretService => b"linux-secret-service",
            Self::WindowsCredentialManager => b"windows-credential-manager",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecretStoreKeyClass {
    DeviceIdentity,
    PairwiseSession,
    GroupEpoch,
    GatewayCredential,
}

impl SecretStoreKeyClass {
    fn tag(self) -> &'static [u8] {
        match self {
            Self::DeviceIdentity => b"device-identity",
            Self::PairwiseSession => b"pairwise-session",
            Self::GroupEpoch => b"group-epoch",
            Self::GatewayCredential => b"gateway-credential",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecretStoreCallerChannel {
    DesktopGui,
    Mobile,
    NativeCli,
    GatewaySidecar,
    GatewayCredentialMigration,
}

impl SecretStoreCallerChannel {
    fn tag(self) -> &'static [u8] {
        match self {
            Self::DesktopGui => b"desktop-gui",
            Self::Mobile => b"mobile",
            Self::NativeCli => b"native-cli",
            Self::GatewaySidecar => b"gateway-sidecar",
            Self::GatewayCredentialMigration => b"gateway-credential-migration",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecretStoreOperation {
    Read,
    Write,
    Delete,
}

impl SecretStoreOperation {
    fn tag(self) -> &'static [u8] {
        match self {
            Self::Read => b"read",
            Self::Write => b"write",
            Self::Delete => b"delete",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresenceDecision {
    Approved,
    Cancelled,
    TimedOut,
}

#[derive(Clone, Eq, PartialEq)]
pub struct SecretStorePresenceNonce(String);

impl SecretStorePresenceNonce {
    pub(crate) fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        validate_context_value(&value, MAX_NONCE_BYTES)?;
        Ok(Self(value))
    }

    fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl fmt::Debug for SecretStorePresenceNonce {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretStorePresenceNonce(REDACTED)")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct SecretStorePresencePurpose(String);

impl SecretStorePresencePurpose {
    pub(crate) fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        validate_context_value(&value, MAX_SCOPE_COMPONENT_BYTES)?;
        Ok(Self(value))
    }

    fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl fmt::Debug for SecretStorePresencePurpose {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretStorePresencePurpose(REDACTED)")
    }
}

#[derive(Clone)]
pub struct SecretStorePresenceBatchRequest {
    provider: SecretStorePresenceProvider,
    key_class: SecretStoreKeyClass,
    caller_channel: SecretStoreCallerChannel,
    operation_count: usize,
    reason: String,
    allow_interaction: bool,
    canonical_digest: [u8; 32],
}

impl SecretStorePresenceBatchRequest {
    pub(crate) fn new(
        provider: SecretStorePresenceProvider,
        key_class: SecretStoreKeyClass,
        operation_count: usize,
        reason: impl Into<String>,
        nonce: SecretStorePresenceNonce,
        caller_channel: SecretStoreCallerChannel,
        allow_interaction: bool,
    ) -> Result<Self> {
        let reason = reason.into();
        validate_context_value(&reason, MAX_REASON_BYTES)?;
        if operation_count == 0 || operation_count > MAX_BATCH_OPERATION_COUNT {
            return Err(
                SecretStorePresenceError::new("secure_mesh_presence_batch_count_invalid").into(),
            );
        }

        let canonical_digest = digest_fields(
            b"licoup:secret-store-presence-batch:v1",
            [
                provider.tag(),
                key_class.tag(),
                &u64::try_from(operation_count)
                    .map_err(|_| {
                        SecretStorePresenceError::new("secure_mesh_presence_batch_count_invalid")
                    })?
                    .to_be_bytes(),
                reason.as_bytes(),
                nonce.as_bytes(),
                caller_channel.tag(),
                &[u8::from(allow_interaction)],
            ],
        );
        Ok(Self {
            provider,
            key_class,
            caller_channel,
            operation_count,
            reason,
            allow_interaction,
            canonical_digest,
        })
    }

    pub(crate) fn canonical_digest(&self) -> [u8; 32] {
        self.canonical_digest
    }

    pub(crate) fn provider(&self) -> SecretStorePresenceProvider {
        self.provider
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn key_class(&self) -> SecretStoreKeyClass {
        self.key_class
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn caller_channel(&self) -> SecretStoreCallerChannel {
        self.caller_channel
    }

    pub(crate) fn is_gateway_credential_migration(&self) -> bool {
        self.provider == SecretStorePresenceProvider::MacosKeychain
            && self.key_class == SecretStoreKeyClass::GatewayCredential
            && self.caller_channel == SecretStoreCallerChannel::GatewayCredentialMigration
            && self.allow_interaction
    }

    pub(crate) fn operation_count(&self) -> usize {
        self.operation_count
    }

    pub(crate) fn reason(&self) -> &str {
        &self.reason
    }

    pub(crate) fn allow_interaction(&self) -> bool {
        self.allow_interaction
    }
}

impl fmt::Debug for SecretStorePresenceBatchRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretStorePresenceBatchRequest(REDACTED)")
    }
}

#[derive(Clone)]
pub struct SecretStorePresenceScope {
    operation: SecretStoreOperation,
    namespace: String,
    key: String,
    canonical_digest: [u8; 32],
}

impl SecretStorePresenceScope {
    pub(crate) fn new(
        operation: SecretStoreOperation,
        namespace: impl Into<String>,
        key: impl Into<String>,
        purpose: SecretStorePresencePurpose,
    ) -> Result<Self> {
        let namespace = namespace.into();
        let key = key.into();
        validate_context_value(&namespace, MAX_SCOPE_COMPONENT_BYTES)?;
        validate_context_value(&key, MAX_SCOPE_COMPONENT_BYTES)?;
        let canonical_digest = digest_fields(
            b"licoup:secret-store-presence-scope:v1",
            [
                operation.tag(),
                namespace.as_bytes(),
                key.as_bytes(),
                purpose.as_bytes(),
            ],
        );
        Ok(Self {
            operation,
            namespace,
            key,
            canonical_digest,
        })
    }
}

impl fmt::Debug for SecretStorePresenceScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretStorePresenceScope(REDACTED)")
    }
}

pub struct SecretStoreApprovedPresenceBatch {
    binding_digest: [u8; 32],
    expires_at: Option<Instant>,
    operation_count: usize,
    issued_count: AtomicUsize,
}

impl SecretStoreApprovedPresenceBatch {
    pub(crate) fn approve(
        request: &SecretStorePresenceBatchRequest,
        approved_at: Instant,
        ttl: Duration,
        decision: PresenceDecision,
    ) -> std::result::Result<Self, SecretStorePresenceError> {
        if !request.allow_interaction {
            return Err(SecretStorePresenceError::new(
                "secure_mesh_presence_interaction_required",
            ));
        }
        match decision {
            PresenceDecision::Approved => {}
            PresenceDecision::Cancelled => {
                return Err(SecretStorePresenceError::new(
                    "secure_mesh_presence_cancelled",
                ));
            }
            PresenceDecision::TimedOut => {
                return Err(SecretStorePresenceError::new(
                    "secure_mesh_presence_timed_out",
                ));
            }
        }
        if ttl.is_zero() || ttl > MAX_SECRET_STORE_PRESENCE_GRANT_TTL {
            return Err(SecretStorePresenceError::new(
                "secure_mesh_presence_ttl_invalid",
            ));
        }
        // The explicitly authorized legacy migration can wait in native ACL
        // dialogs. Its platform scope revokes it when the migration returns;
        // operation budgets and one-use grants still apply throughout.
        let expires_at =
            if request.is_gateway_credential_migration() {
                None
            } else {
                Some(approved_at.checked_add(ttl).ok_or_else(|| {
                    SecretStorePresenceError::new("secure_mesh_presence_ttl_invalid")
                })?)
            };
        let approval_nonce = Uuid::new_v4();
        let binding_digest = digest_fields(
            b"licoup:secret-store-approved-presence-batch:v1",
            [
                request.canonical_digest.as_slice(),
                approval_nonce.as_bytes(),
            ],
        );
        Ok(Self {
            binding_digest,
            expires_at,
            operation_count: request.operation_count,
            issued_count: AtomicUsize::new(0),
        })
    }

    pub(crate) fn issue_grant(
        &self,
        scope: SecretStorePresenceScope,
    ) -> std::result::Result<SecretStorePresenceGrant, SecretStorePresenceError> {
        let mut issued = self.issued_count.load(Ordering::Acquire);
        loop {
            if issued >= self.operation_count {
                return Err(SecretStorePresenceError::new(
                    "secure_mesh_presence_batch_count_exceeded",
                ));
            }
            match self.issued_count.compare_exchange_weak(
                issued,
                issued + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(observed) => issued = observed,
            }
        }
        Ok(SecretStorePresenceGrant(PresenceGrantInner {
            batch_binding_digest: self.binding_digest,
            scope_digest: scope.canonical_digest,
            operation: scope.operation,
            namespace: scope.namespace,
            key: scope.key,
            expires_at: self.expires_at,
            state: AtomicU8::new(GRANT_AVAILABLE),
        }))
    }

    pub(crate) fn binding_digest(&self) -> [u8; 32] {
        self.binding_digest
    }

    pub(crate) fn expires_at(&self) -> Option<Instant> {
        self.expires_at
    }
}

impl fmt::Debug for SecretStoreApprovedPresenceBatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretStoreApprovedPresenceBatch(REDACTED)")
    }
}

struct PresenceGrantInner {
    batch_binding_digest: [u8; 32],
    scope_digest: [u8; 32],
    operation: SecretStoreOperation,
    namespace: String,
    key: String,
    expires_at: Option<Instant>,
    state: AtomicU8,
}

pub struct SecretStorePresenceGrant(PresenceGrantInner);

impl SecretStorePresenceGrant {
    pub(crate) fn consume(
        &self,
        batch: &SecretStoreApprovedPresenceBatch,
        expected_scope: &SecretStorePresenceScope,
        now: Instant,
    ) -> std::result::Result<SecretStoreConsumedPresence, SecretStorePresenceError> {
        if !digest_matches(&self.0.batch_binding_digest, &batch.binding_digest) {
            return Err(SecretStorePresenceError::new(
                "secure_mesh_presence_batch_mismatch",
            ));
        }
        if !digest_matches(&self.0.scope_digest, &expected_scope.canonical_digest) {
            return Err(SecretStorePresenceError::new(
                "secure_mesh_presence_scope_mismatch",
            ));
        }

        loop {
            match self.0.state.load(Ordering::Acquire) {
                GRANT_CONSUMED => {
                    return Err(SecretStorePresenceError::new(
                        "secure_mesh_presence_replayed",
                    ));
                }
                GRANT_EXPIRED => {
                    return Err(SecretStorePresenceError::new(
                        "secure_mesh_presence_expired",
                    ));
                }
                GRANT_AVAILABLE if self.0.expires_at.is_some_and(|expiry| now >= expiry) => {
                    if self
                        .0
                        .state
                        .compare_exchange(
                            GRANT_AVAILABLE,
                            GRANT_EXPIRED,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        return Err(SecretStorePresenceError::new(
                            "secure_mesh_presence_expired",
                        ));
                    }
                }
                GRANT_AVAILABLE => {
                    if self
                        .0
                        .state
                        .compare_exchange(
                            GRANT_AVAILABLE,
                            GRANT_CONSUMED,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        )
                        .is_ok()
                    {
                        return Ok(SecretStoreConsumedPresence(ConsumedPresenceInner {
                            batch_binding_digest: self.0.batch_binding_digest,
                            scope_digest: self.0.scope_digest,
                            operation: self.0.operation,
                            namespace: self.0.namespace.clone(),
                            key: self.0.key.clone(),
                            expires_at: self.0.expires_at,
                        }));
                    }
                }
                _ => unreachable!("presence grant state is closed over known values"),
            }
        }
    }
}

impl fmt::Debug for SecretStorePresenceGrant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretStorePresenceGrant(REDACTED)")
    }
}

struct ConsumedPresenceInner {
    batch_binding_digest: [u8; 32],
    scope_digest: [u8; 32],
    operation: SecretStoreOperation,
    namespace: String,
    key: String,
    expires_at: Option<Instant>,
}

pub struct SecretStoreConsumedPresence(ConsumedPresenceInner);

impl SecretStoreConsumedPresence {
    pub(crate) fn batch_binding_digest(&self) -> [u8; 32] {
        self.0.batch_binding_digest
    }

    pub(crate) fn scope_digest(&self) -> [u8; 32] {
        self.0.scope_digest
    }

    pub(crate) fn expires_at(&self) -> Option<Instant> {
        self.0.expires_at
    }

    pub(crate) fn into_effect_target(self) -> (SecretStoreOperation, String, String) {
        (self.0.operation, self.0.namespace, self.0.key)
    }
}

impl fmt::Debug for SecretStoreConsumedPresence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretStoreConsumedPresence(REDACTED)")
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct SecretStorePresenceError {
    code: &'static str,
}

impl SecretStorePresenceError {
    pub fn authorization_required() -> Self {
        Self::new("secure_mesh_authorization_required")
    }

    pub fn authorization_failed() -> Self {
        Self::new("secure_mesh_presence_native_authentication_failed")
    }

    fn new(code: &'static str) -> Self {
        Self { code }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }
}

impl fmt::Display for SecretStorePresenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code)
    }
}

impl fmt::Debug for SecretStorePresenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code)
    }
}

impl std::error::Error for SecretStorePresenceError {}

pub struct SecretStoreAuthorizationRequest {
    reason: String,
    operation_count: usize,
    allow_interaction: bool,
    canonical_digest: [u8; 32],
    key_class: SecretStoreKeyClass,
    caller_channel: SecretStoreCallerChannel,
}

impl SecretStoreAuthorizationRequest {
    pub fn new(reason: impl Into<String>, operation_count: usize) -> Self {
        Self::build(
            reason.into(),
            operation_count,
            true,
            SecretStoreKeyClass::DeviceIdentity,
            SecretStoreCallerChannel::DesktopGui,
        )
    }

    pub fn noninteractive(reason: impl Into<String>, operation_count: usize) -> Self {
        Self::build(
            reason.into(),
            operation_count,
            false,
            SecretStoreKeyClass::DeviceIdentity,
            SecretStoreCallerChannel::DesktopGui,
        )
    }

    pub fn for_scope(
        reason: impl Into<String>,
        operation_count: usize,
        allow_interaction: bool,
        key_class: SecretStoreKeyClass,
        caller_channel: SecretStoreCallerChannel,
    ) -> Self {
        Self::build(
            reason.into(),
            operation_count,
            allow_interaction,
            key_class,
            caller_channel,
        )
    }

    fn build(
        reason: String,
        operation_count: usize,
        allow_interaction: bool,
        key_class: SecretStoreKeyClass,
        caller_channel: SecretStoreCallerChannel,
    ) -> Self {
        let canonical_digest = digest_fields(
            b"licoup:secret-store-authorization-request:v1",
            [
                reason.as_bytes(),
                &u64::try_from(operation_count)
                    .unwrap_or(u64::MAX)
                    .to_be_bytes(),
                &[u8::from(allow_interaction)],
                key_class.tag(),
                caller_channel.tag(),
            ],
        );
        Self {
            reason,
            operation_count,
            allow_interaction,
            canonical_digest,
            key_class,
            caller_channel,
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    pub fn operation_count(&self) -> usize {
        self.operation_count
    }

    pub fn allow_interaction(&self) -> bool {
        self.allow_interaction
    }

    pub fn key_class(&self) -> SecretStoreKeyClass {
        self.key_class
    }

    pub fn caller_channel(&self) -> SecretStoreCallerChannel {
        self.caller_channel
    }

    pub(crate) fn canonical_digest(&self) -> [u8; 32] {
        self.canonical_digest
    }
}

#[derive(Clone)]
pub struct SecretStoreAuthorizationSession {
    session_id: String,
    backend: &'static str,
    reason: String,
    operation_count: usize,
    allow_interaction: bool,
    request_digest: [u8; 32],
    shared_system_context_required: bool,
    shared_system_context_available: bool,
    system_authorization_attempt_count: usize,
    system_authorization_completed: bool,
    app_password_prompt_used: bool,
    consumed_operation_count: Arc<AtomicUsize>,
    capability_report: Option<CapabilityEvaluationReport>,
    presence_binding_digest: Option<[u8; 32]>,
}

impl SecretStoreAuthorizationSession {
    pub(crate) fn new(
        backend: &'static str,
        request: &SecretStoreAuthorizationRequest,
        shared_system_context_required: bool,
        shared_system_context_available: bool,
    ) -> Self {
        Self {
            session_id: Uuid::new_v4().to_string(),
            backend,
            reason: request.reason().to_string(),
            operation_count: request.operation_count(),
            allow_interaction: request.allow_interaction(),
            request_digest: request.canonical_digest(),
            shared_system_context_required,
            shared_system_context_available,
            system_authorization_attempt_count: 0,
            system_authorization_completed: false,
            app_password_prompt_used: false,
            consumed_operation_count: Arc::new(AtomicUsize::new(0)),
            capability_report: None,
            presence_binding_digest: None,
        }
    }

    pub(crate) fn with_presence_binding(
        mut self,
        binding_digest: [u8; 32],
        system_authorization_attempt_count: usize,
        system_authorization_completed: bool,
    ) -> Self {
        self.shared_system_context_available = true;
        self.system_authorization_attempt_count = system_authorization_attempt_count;
        self.system_authorization_completed = system_authorization_completed;
        self.presence_binding_digest = Some(binding_digest);
        self
    }

    pub(crate) fn presence_binding_digest(&self) -> Option<[u8; 32]> {
        self.presence_binding_digest
    }

    pub(crate) fn request_digest(&self) -> [u8; 32] {
        self.request_digest
    }

    #[cfg(test)]
    pub(crate) fn with_test_system_authorization_outcome(
        mut self,
        attempt_count: usize,
        completed: bool,
        app_password_prompt_used: bool,
    ) -> Self {
        self.system_authorization_attempt_count = attempt_count;
        self.system_authorization_completed = completed;
        self.app_password_prompt_used = app_password_prompt_used;
        self
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn backend(&self) -> &'static str {
        self.backend
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    pub fn operation_count(&self) -> usize {
        self.operation_count
    }

    pub fn allow_interaction(&self) -> bool {
        self.allow_interaction
    }

    pub fn shared_system_context_required(&self) -> bool {
        self.shared_system_context_required
    }

    pub fn shared_system_context_available(&self) -> bool {
        self.shared_system_context_available
    }

    pub fn system_authorization_attempt_count(&self) -> usize {
        self.system_authorization_attempt_count
    }

    pub fn system_authorization_completed(&self) -> bool {
        self.system_authorization_completed
    }

    pub fn app_password_prompt_used(&self) -> bool {
        self.app_password_prompt_used
    }

    pub fn capability_report(&self) -> Option<&CapabilityEvaluationReport> {
        self.capability_report.as_ref()
    }

    pub fn consumed_operation_count(&self) -> usize {
        self.consumed_operation_count.load(Ordering::SeqCst)
    }

    pub fn remaining_operation_count(&self) -> usize {
        self.operation_count
            .saturating_sub(self.consumed_operation_count())
    }

    pub fn authorization_batch_within_budget(&self) -> bool {
        self.consumed_operation_count() <= self.operation_count
    }

    pub fn record_secret_store_operation(&self, _operation: &str) -> Result<()> {
        let mut current = self.consumed_operation_count.load(Ordering::SeqCst);
        loop {
            if current >= self.operation_count {
                return Err(anyhow!(
                    "secure_mesh_secret_store_operation_budget_exceeded"
                ));
            }
            match self.consumed_operation_count.compare_exchange(
                current,
                current + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return Ok(()),
                Err(next) => current = next,
            }
        }
    }

    pub fn single_system_authorization_context_verified(&self) -> bool {
        self.shared_system_context_required
            && self.shared_system_context_available
            && self.system_authorization_attempt_count == 1
            && self.system_authorization_completed
            && !self.app_password_prompt_used
    }

    pub(crate) fn with_capability_report(
        mut self,
        capability_report: CapabilityEvaluationReport,
    ) -> Self {
        self.capability_report = Some(capability_report);
        self
    }
}

impl fmt::Debug for SecretStoreAuthorizationSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecretStoreAuthorizationSession")
            .field("backend", &self.backend)
            .field("operation_count", &self.operation_count)
            .field("allow_interaction", &self.allow_interaction)
            .field(
                "shared_system_context_required",
                &self.shared_system_context_required,
            )
            .field(
                "shared_system_context_available",
                &self.shared_system_context_available,
            )
            .field("consumed_operation_count", &self.consumed_operation_count())
            .field(
                "presence_binding",
                &self.presence_binding_digest.map(|_| "redacted"),
            )
            .finish()
    }
}

impl PartialEq for SecretStoreAuthorizationSession {
    fn eq(&self, other: &Self) -> bool {
        self.session_id == other.session_id
            && self.backend == other.backend
            && self.reason == other.reason
            && self.operation_count == other.operation_count
            && self.allow_interaction == other.allow_interaction
            && self.request_digest == other.request_digest
            && self.shared_system_context_required == other.shared_system_context_required
            && self.shared_system_context_available == other.shared_system_context_available
            && self.system_authorization_attempt_count == other.system_authorization_attempt_count
            && self.system_authorization_completed == other.system_authorization_completed
            && self.app_password_prompt_used == other.app_password_prompt_used
            && self.consumed_operation_count() == other.consumed_operation_count()
            && self.capability_report == other.capability_report
            && self.presence_binding_digest == other.presence_binding_digest
    }
}

impl Eq for SecretStoreAuthorizationSession {}

fn validate_context_value(value: &str, max_bytes: usize) -> Result<()> {
    if value.trim().is_empty() || value.len() > max_bytes || value.contains('\0') {
        return Err(SecretStorePresenceError::new("secure_mesh_presence_context_invalid").into());
    }
    Ok(())
}

fn digest_fields<'a>(domain: &[u8], fields: impl IntoIterator<Item = &'a [u8]>) -> [u8; 32] {
    let mut hasher = Sha256::new();
    update_length_prefixed(&mut hasher, domain);
    for field in fields {
        update_length_prefixed(&mut hasher, field);
    }
    hasher.finalize().into()
}

fn update_length_prefixed(hasher: &mut Sha256, value: &[u8]) {
    hasher.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(value);
}

pub(crate) fn digest_matches(left: &[u8; 32], right: &[u8; 32]) -> bool {
    bool::from(left.ct_eq(right))
}

pub(crate) fn derive_presence_binding_digest(
    domain: &[u8],
    fields: impl IntoIterator<Item = impl AsRef<[u8]>>,
) -> [u8; 32] {
    let owned = fields
        .into_iter()
        .map(|field| field.as_ref().to_vec())
        .collect::<Vec<_>>();
    digest_fields(domain, owned.iter().map(Vec::as_slice))
}
