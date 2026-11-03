use std::collections::HashMap;
use std::fmt;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

use anyhow::{Result, anyhow};
use core_foundation::base::CFType;
use credential_lease_keychain::{
    Keychain, KeychainAuthContext, KeychainBackend, KeychainError, KeychainErrorKind,
    KeychainItemQuery,
};
use uuid::Uuid;

use crate::core::secure_mesh_capability::{
    CapabilityEvidenceKind, CapabilityFact, SecurityCapability,
};
use crate::core::secure_mesh_secret_store::{
    MAX_SECRET_STORE_PRESENCE_GRANT_TTL, PresenceDecision, SecretBytes,
    SecretStoreApprovedPresenceBatch, SecretStoreAuthorizationRequest,
    SecretStoreAuthorizationSession, SecretStoreConsumedPresence, SecretStoreHandle,
    SecretStoreOperation, SecretStorePresenceBatchRequest, SecretStorePresenceError,
    SecretStorePresenceNonce, SecretStorePresenceProvider, SecretStorePresencePurpose,
    SecretStorePresenceScope, derive_presence_binding_digest, digest_matches,
};

const MACOS_AUTHORIZATION_CACHE_MAX_BATCHES: usize = 16;

static TEST_USER_PRESENCE_DISABLED: AtomicBool = AtomicBool::new(false);

#[derive(Clone)]
pub struct MacosAuthorizationContext {
    batch_binding_digest: [u8; 32],
    context: Option<crate::platform::user_presence::UserPresenceSession>,
    effect_lock: Arc<Mutex<()>>,
    legacy_credential_migration: Option<Arc<MacosLegacyCredentialMigration>>,
}

struct MacosLegacyCredentialMigration {
    active: AtomicBool,
}

impl MacosAuthorizationContext {
    pub fn authorize(
        &self,
        consumed_presence: SecretStoreConsumedPresence,
    ) -> std::result::Result<MacosAuthorizedPresence, MacosPresenceError> {
        if !self.is_active() {
            return Err(MacosPresenceError::new(
                "secure_mesh_presence_session_closed",
            ));
        }
        if !digest_matches(
            &self.batch_binding_digest,
            &consumed_presence.batch_binding_digest(),
        ) {
            return Err(MacosPresenceError::new(
                "secure_mesh_presence_native_context_mismatch",
            ));
        }
        let scope_digest = consumed_presence.scope_digest();
        let expires_at = consumed_presence.expires_at();
        let (operation, namespace, key) = consumed_presence.into_effect_target();
        let handle = SecretStoreHandle::new(namespace, key)
            .map_err(|_| MacosPresenceError::new("secure_mesh_presence_effect_target_invalid"))?;
        Ok(MacosAuthorizedPresence(MacosAuthorizedPresenceInner {
            authorization_context: self.clone(),
            scope_digest,
            operation,
            handle,
            expires_at,
        }))
    }

    fn as_cf_type(&self) -> Result<CFType> {
        self.context
            .as_ref()
            .map(crate::platform::user_presence::UserPresenceSession::as_cf_type)
            .ok_or_else(|| anyhow!("secure_mesh_presence_native_context_unavailable"))
    }

    fn is_active(&self) -> bool {
        self.legacy_credential_migration
            .as_ref()
            .is_none_or(|migration| migration.active.load(Ordering::Acquire))
    }

    #[cfg(test)]
    pub(crate) fn migration_session_active_for_test(&self) -> bool {
        self.legacy_credential_migration.is_some() && self.is_active()
    }

    /// Whether the legacy migration consented to classic Keychain UI. The
    /// backend owns the process-wide UI policy itself; the host only hands it
    /// the session's consent flag, and a closed session refuses outright.
    fn with_legacy_classic_interaction<T>(
        &self,
        effect: impl FnOnce(bool) -> Result<T>,
    ) -> Result<T> {
        if !self.is_active() {
            return Err(anyhow!("secure_mesh_presence_session_closed"));
        }
        effect(self.legacy_credential_migration.is_some())
    }
}

impl fmt::Debug for MacosAuthorizationContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MacosAuthorizationContext(REDACTED)")
    }
}

struct MacosAuthorizedPresenceInner {
    authorization_context: MacosAuthorizationContext,
    scope_digest: [u8; 32],
    operation: SecretStoreOperation,
    handle: SecretStoreHandle,
    expires_at: Option<Instant>,
}

pub struct MacosAuthorizedPresence(MacosAuthorizedPresenceInner);

impl MacosAuthorizedPresence {
    fn into_authorized_effect(
        self,
        expected_operation: SecretStoreOperation,
    ) -> Result<(MacosAuthorizationContext, SecretStoreHandle)> {
        if self.0.operation != expected_operation {
            return Err(anyhow!("secure_mesh_presence_effect_operation_mismatch"));
        }
        if !self.0.authorization_context.is_active() {
            return Err(anyhow!("secure_mesh_presence_session_closed"));
        }
        if self
            .0
            .expires_at
            .is_some_and(|expiry| Instant::now() >= expiry)
        {
            return Err(anyhow!("secure_mesh_presence_expired"));
        }
        Ok((self.0.authorization_context, self.0.handle))
    }
}

impl fmt::Debug for MacosAuthorizedPresence {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = self.0.scope_digest;
        formatter.write_str("MacosAuthorizedPresence(REDACTED)")
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct MacosPresenceError {
    code: &'static str,
}

impl MacosPresenceError {
    fn new(code: &'static str) -> Self {
        Self { code }
    }

    #[cfg(test)]
    pub fn code(&self) -> &'static str {
        self.code
    }
}

impl fmt::Display for MacosPresenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code)
    }
}

impl fmt::Debug for MacosPresenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code)
    }
}

impl std::error::Error for MacosPresenceError {}

pub trait MacosPresencePromptPort: Send {
    fn prompt(&mut self, request: &SecretStorePresenceBatchRequest) -> Result<PresenceDecision>;

    fn take_authorization_context(&mut self) -> Option<MacosAuthorizationContext> {
        None
    }
}

pub trait MacosKeychainEffectPort {
    fn set_secret(
        &self,
        authorized_presence: MacosAuthorizedPresence,
        service: &str,
        handle: &SecretStoreHandle,
        secret: SecretBytes,
    ) -> Result<()>;

    fn get_secret(
        &self,
        authorized_presence: MacosAuthorizedPresence,
        service: &str,
        handle: &SecretStoreHandle,
    ) -> Result<Option<SecretBytes>>;

    fn delete_secret(
        &self,
        authorized_presence: MacosAuthorizedPresence,
        service: &str,
        handle: &SecretStoreHandle,
    ) -> Result<()>;

    fn get_legacy_classic_secret(
        &self,
        _authorized_presence: MacosAuthorizedPresence,
        _service: &str,
        _handle: &SecretStoreHandle,
    ) -> Result<Option<SecretBytes>> {
        Ok(None)
    }

    fn delete_legacy_classic_secret(
        &self,
        _authorized_presence: MacosAuthorizedPresence,
        _service: &str,
        _handle: &SecretStoreHandle,
    ) -> Result<()> {
        Ok(())
    }
}

pub trait MacosSecItemPort {
    fn set_secret(
        &self,
        authorization_context: &MacosAuthorizationContext,
        service: &str,
        handle: &SecretStoreHandle,
        secret: SecretBytes,
    ) -> Result<()>;

    fn get_secret(
        &self,
        authorization_context: &MacosAuthorizationContext,
        service: &str,
        handle: &SecretStoreHandle,
    ) -> Result<Option<SecretBytes>>;

    fn delete_secret(
        &self,
        authorization_context: &MacosAuthorizationContext,
        service: &str,
        handle: &SecretStoreHandle,
    ) -> Result<()>;

    fn get_legacy_classic_secret(
        &self,
        _authorization_context: &MacosAuthorizationContext,
        _service: &str,
        _handle: &SecretStoreHandle,
    ) -> Result<Option<SecretBytes>> {
        Ok(None)
    }

    fn delete_legacy_classic_secret(
        &self,
        _authorization_context: &MacosAuthorizationContext,
        _service: &str,
        _handle: &SecretStoreHandle,
    ) -> Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
struct ReadyBatch {
    batch: Arc<SecretStoreApprovedPresenceBatch>,
    authorization_context: Arc<MacosAuthorizationContext>,
}

enum BatchSlotState {
    Pending,
    Ready(ReadyBatch),
    Failed(&'static str),
}

struct BatchSlot {
    state: Mutex<BatchSlotState>,
    ready: Condvar,
}

impl BatchSlot {
    fn pending() -> Self {
        Self {
            state: Mutex::new(BatchSlotState::Pending),
            ready: Condvar::new(),
        }
    }
}

#[derive(Default)]
pub struct MacosPresenceBatchCoordinator {
    batches: Mutex<HashMap<[u8; 32], Arc<BatchSlot>>>,
}

impl MacosPresenceBatchCoordinator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn authorize_batch(
        &self,
        request: &SecretStorePresenceBatchRequest,
        now: Instant,
        prompt: &mut dyn MacosPresencePromptPort,
    ) -> Result<MacosApprovedPresenceBatch> {
        if request.provider() != SecretStorePresenceProvider::MacosKeychain {
            return Err(anyhow!("secure_mesh_presence_provider_mismatch"));
        }
        if !request.allow_interaction() {
            return Err(anyhow!("secure_mesh_presence_interaction_required"));
        }

        let request_digest = request.canonical_digest();
        let (slot, prompt_owner) = {
            let mut batches = self
                .batches
                .lock()
                .map_err(|_| anyhow!("secure_mesh_presence_coordinator_unavailable"))?;
            batches.retain(|_, slot| {
                let Ok(state) = slot.state.lock() else {
                    return false;
                };
                match &*state {
                    BatchSlotState::Ready(ready) => {
                        ready.authorization_context.is_active()
                            && ready.batch.expires_at().is_none_or(|expiry| now < expiry)
                    }
                    BatchSlotState::Pending => true,
                    BatchSlotState::Failed(_) => false,
                }
            });
            if let Some(slot) = batches.get(&request_digest) {
                (Arc::clone(slot), false)
            } else {
                if batches.len() >= MACOS_AUTHORIZATION_CACHE_MAX_BATCHES {
                    return Err(anyhow!("secure_mesh_presence_cache_capacity_exceeded"));
                }
                let slot = Arc::new(BatchSlot::pending());
                batches.insert(request_digest, Arc::clone(&slot));
                (slot, true)
            }
        };

        if !prompt_owner {
            return wait_for_ready_batch(&slot);
        }

        // No coordinator/cache lock is held while system UI is active.
        let completed = (|| -> Result<ReadyBatch> {
            let decision = prompt
                .prompt(request)
                .map_err(|_| anyhow!("secure_mesh_presence_prompt_failed"))?;
            // The approval window starts when the user completes the system
            // prompt, not when the request was constructed: interactive prompts
            // may legitimately outlast the grant TTL. Tests keep the injected
            // anchor so their time control stays deterministic.
            let approved_anchor = if cfg!(test) { now } else { Instant::now() };
            let batch = Arc::new(
                SecretStoreApprovedPresenceBatch::approve(
                    request,
                    approved_anchor,
                    MAX_SECRET_STORE_PRESENCE_GRANT_TTL,
                    decision,
                )
                .map_err(anyhow::Error::from)?,
            );
            let mut authorization_context =
                prompt
                    .take_authorization_context()
                    .unwrap_or(MacosAuthorizationContext {
                        batch_binding_digest: batch.binding_digest(),
                        context: None,
                        effect_lock: Arc::new(Mutex::new(())),
                        legacy_credential_migration: None,
                    });
            authorization_context.batch_binding_digest = batch.binding_digest();
            authorization_context.legacy_credential_migration =
                request.is_gateway_credential_migration().then(|| {
                    Arc::new(MacosLegacyCredentialMigration {
                        active: AtomicBool::new(true),
                    })
                });
            let authorization_context = Arc::new(authorization_context);
            Ok(ReadyBatch {
                batch,
                authorization_context,
            })
        })();

        let mut state = slot
            .state
            .lock()
            .map_err(|_| anyhow!("secure_mesh_presence_coordinator_unavailable"))?;
        match completed {
            Ok(ready) => {
                *state = BatchSlotState::Ready(ready.clone());
                slot.ready.notify_all();
                Ok(MacosApprovedPresenceBatch(ready))
            }
            Err(error) => {
                let code = stable_presence_error_code(&error);
                *state = BatchSlotState::Failed(code);
                slot.ready.notify_all();
                drop(state);
                if let Ok(mut batches) = self.batches.lock()
                    && batches
                        .get(&request_digest)
                        .is_some_and(|current| Arc::ptr_eq(current, &slot))
                {
                    batches.remove(&request_digest);
                }
                Err(anyhow!(code))
            }
        }
    }
}

fn wait_for_ready_batch(slot: &BatchSlot) -> Result<MacosApprovedPresenceBatch> {
    let mut state = slot
        .state
        .lock()
        .map_err(|_| anyhow!("secure_mesh_presence_coordinator_unavailable"))?;
    loop {
        match &*state {
            BatchSlotState::Pending => {
                state = slot
                    .ready
                    .wait(state)
                    .map_err(|_| anyhow!("secure_mesh_presence_coordinator_unavailable"))?;
            }
            BatchSlotState::Ready(ready) => {
                return Ok(MacosApprovedPresenceBatch(ready.clone()));
            }
            BatchSlotState::Failed(code) => return Err(anyhow!(*code)),
        }
    }
}

fn stable_presence_error_code(error: &anyhow::Error) -> &'static str {
    for code in [
        "secure_mesh_presence_cancelled",
        "secure_mesh_presence_timed_out",
        "secure_mesh_presence_interaction_required",
        "secure_mesh_presence_ttl_invalid",
        "secure_mesh_presence_prompt_failed",
    ] {
        if error.to_string() == code {
            return code;
        }
    }
    "secure_mesh_presence_prompt_failed"
}

#[derive(Clone)]
pub struct MacosApprovedPresenceBatch(ReadyBatch);

impl MacosApprovedPresenceBatch {
    pub fn batch(&self) -> &SecretStoreApprovedPresenceBatch {
        &self.0.batch
    }

    pub fn authorization_context(&self) -> &MacosAuthorizationContext {
        &self.0.authorization_context
    }
}

impl fmt::Debug for MacosApprovedPresenceBatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MacosApprovedPresenceBatch(REDACTED)")
    }
}

struct BoundSession {
    backend: &'static str,
    session_id: String,
    request_digest: [u8; 32],
    session_binding_digest: [u8; 32],
    approved_batch: MacosApprovedPresenceBatch,
}

impl Drop for BoundSession {
    fn drop(&mut self) {
        if let Some(migration) = &self
            .approved_batch
            .authorization_context()
            .legacy_credential_migration
        {
            migration.active.store(false, Ordering::Release);
        }
    }
}

pub struct MacosSecretStoreAccess {
    injected: bool,
    request: SecretStorePresenceBatchRequest,
    approved_at: Instant,
    fixed_operation_now: Option<Instant>,
    coordinator: MacosPresenceBatchCoordinator,
    prompt: Mutex<Box<dyn MacosPresencePromptPort>>,
    keychain: Arc<dyn MacosKeychainEffectPort + Send + Sync>,
    bound_session: Mutex<Option<BoundSession>>,
}

impl MacosSecretStoreAccess {
    #[cfg(test)]
    pub fn new(
        request: SecretStorePresenceBatchRequest,
        approved_at: Instant,
        operation_now: Instant,
        prompt: Box<dyn MacosPresencePromptPort>,
        keychain: Arc<dyn MacosKeychainEffectPort + Send + Sync>,
    ) -> Self {
        Self {
            injected: true,
            request,
            approved_at,
            fixed_operation_now: Some(operation_now),
            coordinator: MacosPresenceBatchCoordinator::new(),
            prompt: Mutex::new(prompt),
            keychain,
            bound_session: Mutex::new(None),
        }
    }

    fn production(request: SecretStorePresenceBatchRequest) -> Self {
        Self {
            injected: false,
            request,
            approved_at: Instant::now(),
            fixed_operation_now: None,
            coordinator: MacosPresenceBatchCoordinator::new(),
            prompt: Mutex::new(Box::new(LocalAuthenticationPrompt::new())),
            keychain: Arc::new(SecurityFrameworkKeychain::new()),
            bound_session: Mutex::new(None),
        }
    }

    pub(crate) fn is_injected(&self) -> bool {
        self.injected
    }

    pub(crate) fn begin_session(
        &self,
        backend: &'static str,
        request: &SecretStoreAuthorizationRequest,
    ) -> Result<SecretStoreAuthorizationSession> {
        if request.reason() != self.request.reason()
            || request.operation_count() != self.request.operation_count()
            || request.allow_interaction() != self.request.allow_interaction()
            || request.key_class() != self.request.key_class()
            || request.caller_channel() != self.request.caller_channel()
        {
            return Err(anyhow!("secure_mesh_presence_batch_mismatch"));
        }

        let approved_batch = {
            let mut prompt = self
                .prompt
                .lock()
                .map_err(|_| anyhow!("secure_mesh_presence_prompt_unavailable"))?;
            self.coordinator
                .authorize_batch(&self.request, self.approved_at, &mut **prompt)?
        };
        let base_session = SecretStoreAuthorizationSession::new(backend, request, true, true);
        let random_binding = Uuid::new_v4();
        let session_binding_digest = derive_presence_binding_digest(
            b"licoup:macos-presence-session-binding:v1",
            [
                base_session.session_id().as_bytes(),
                request.canonical_digest().as_slice(),
                approved_batch.batch().binding_digest().as_slice(),
                random_binding.as_bytes(),
            ],
        );
        let session = base_session.with_presence_binding(session_binding_digest, 1, true);
        let bound = BoundSession {
            backend,
            session_id: session.session_id().to_string(),
            request_digest: request.canonical_digest(),
            session_binding_digest,
            approved_batch,
        };
        *self
            .bound_session
            .lock()
            .map_err(|_| anyhow!("secure_mesh_presence_session_unavailable"))? = Some(bound);
        Ok(session)
    }

    pub(crate) fn finish_migration_session(&self, session: &SecretStoreAuthorizationSession) {
        if let Ok(mut bound) = self.bound_session.lock()
            && bound
                .as_ref()
                .is_some_and(|bound| bound.session_id == session.session_id())
        {
            bound.take();
        }
    }

    fn approved_for_session(
        &self,
        session: &SecretStoreAuthorizationSession,
    ) -> Result<MacosApprovedPresenceBatch> {
        let bound = self
            .bound_session
            .lock()
            .map_err(|_| anyhow!("secure_mesh_presence_session_unavailable"))?;
        let Some(bound) = bound.as_ref() else {
            return Err(anyhow!("secure_mesh_presence_session_batch_mismatch"));
        };
        let Some(session_binding_digest) = session.presence_binding_digest() else {
            return Err(anyhow!("secure_mesh_presence_session_batch_mismatch"));
        };
        if session.backend() != bound.backend
            || session.session_id() != bound.session_id
            || !digest_matches(&session.request_digest(), &bound.request_digest)
            || !digest_matches(&session_binding_digest, &bound.session_binding_digest)
        {
            return Err(anyhow!("secure_mesh_presence_session_batch_mismatch"));
        }
        Ok(bound.approved_batch.clone())
    }

    fn operation_now(&self) -> Instant {
        self.fixed_operation_now.unwrap_or_else(Instant::now)
    }

    pub(crate) fn set_secret(
        &self,
        service: &str,
        session: &SecretStoreAuthorizationSession,
        handle: &SecretStoreHandle,
        secret: SecretBytes,
    ) -> Result<()> {
        let approved = self.approved_for_session(session)?;
        execute_set(
            service,
            &approved,
            self.operation_now(),
            &*self.keychain,
            handle,
            SecretStorePresencePurpose::new("platform-secret-store-write")?,
            secret,
        )
    }

    pub(crate) fn get_secret(
        &self,
        service: &str,
        session: &SecretStoreAuthorizationSession,
        handle: &SecretStoreHandle,
    ) -> Result<Option<SecretBytes>> {
        let approved = self.approved_for_session(session)?;
        execute_get(
            service,
            &approved,
            self.operation_now(),
            &*self.keychain,
            handle,
            SecretStorePresencePurpose::new("platform-secret-store-read")?,
        )
    }

    pub(crate) fn delete_secret(
        &self,
        service: &str,
        session: &SecretStoreAuthorizationSession,
        handle: &SecretStoreHandle,
    ) -> Result<()> {
        let approved = self.approved_for_session(session)?;
        execute_delete(
            service,
            &approved,
            self.operation_now(),
            &*self.keychain,
            handle,
            SecretStorePresencePurpose::new("platform-secret-store-delete")?,
        )
    }

    pub(crate) fn get_legacy_classic_secret(
        &self,
        service: &str,
        session: &SecretStoreAuthorizationSession,
        handle: &SecretStoreHandle,
    ) -> Result<Option<SecretBytes>> {
        let approved = self.approved_for_session(session)?;
        execute_legacy_classic_get(
            service,
            &approved,
            self.operation_now(),
            &*self.keychain,
            handle,
            SecretStorePresencePurpose::new("platform-secret-store-legacy-read")?,
        )
    }

    pub(crate) fn delete_legacy_classic_secret(
        &self,
        service: &str,
        session: &SecretStoreAuthorizationSession,
        handle: &SecretStoreHandle,
    ) -> Result<()> {
        let approved = self.approved_for_session(session)?;
        execute_legacy_classic_delete(
            service,
            &approved,
            self.operation_now(),
            &*self.keychain,
            handle,
            SecretStorePresencePurpose::new("platform-secret-store-legacy-delete")?,
        )
    }
}

pub(crate) fn production_access(
    request: &SecretStoreAuthorizationRequest,
) -> Result<MacosSecretStoreAccess> {
    let nonce = SecretStorePresenceNonce::new(Uuid::new_v4().to_string())?;
    let presence_request = SecretStorePresenceBatchRequest::new(
        SecretStorePresenceProvider::MacosKeychain,
        request.key_class(),
        request.operation_count(),
        request.reason(),
        nonce,
        request.caller_channel(),
        request.allow_interaction(),
    )?;
    if presence_request.is_gateway_credential_migration()
        && Keychain::selected()
            .ok_or_else(|| anyhow!("secure_mesh_keychain_unavailable"))?
            .backend()
            != KeychainBackend::DataProtection
    {
        return Err(anyhow!("secure_mesh_keychain_migration_target_invalid"));
    }
    Ok(MacosSecretStoreAccess::production(presence_request))
}

#[cfg(test)]
pub fn set_secret(
    service: &str,
    coordinator: &MacosPresenceBatchCoordinator,
    request: &SecretStorePresenceBatchRequest,
    approved_at: Instant,
    operation_now: Instant,
    prompt: &mut dyn MacosPresencePromptPort,
    keychain: &dyn MacosKeychainEffectPort,
    handle: &SecretStoreHandle,
    purpose: SecretStorePresencePurpose,
    secret: SecretBytes,
) -> Result<()> {
    let approved = coordinator.authorize_batch(request, approved_at, prompt)?;
    execute_set(
        service,
        &approved,
        operation_now,
        keychain,
        handle,
        purpose,
        secret,
    )
}

#[cfg(test)]
pub fn get_secret(
    service: &str,
    coordinator: &MacosPresenceBatchCoordinator,
    request: &SecretStorePresenceBatchRequest,
    approved_at: Instant,
    operation_now: Instant,
    prompt: &mut dyn MacosPresencePromptPort,
    keychain: &dyn MacosKeychainEffectPort,
    handle: &SecretStoreHandle,
    purpose: SecretStorePresencePurpose,
) -> Result<Option<SecretBytes>> {
    let approved = coordinator.authorize_batch(request, approved_at, prompt)?;
    execute_get(service, &approved, operation_now, keychain, handle, purpose)
}

#[cfg(test)]
pub fn delete_secret(
    service: &str,
    coordinator: &MacosPresenceBatchCoordinator,
    request: &SecretStorePresenceBatchRequest,
    approved_at: Instant,
    operation_now: Instant,
    prompt: &mut dyn MacosPresencePromptPort,
    keychain: &dyn MacosKeychainEffectPort,
    handle: &SecretStoreHandle,
    purpose: SecretStorePresencePurpose,
) -> Result<()> {
    let approved = coordinator.authorize_batch(request, approved_at, prompt)?;
    execute_delete(service, &approved, operation_now, keychain, handle, purpose)
}

fn execute_set(
    service: &str,
    approved: &MacosApprovedPresenceBatch,
    now: Instant,
    keychain: &dyn MacosKeychainEffectPort,
    handle: &SecretStoreHandle,
    purpose: SecretStorePresencePurpose,
    secret: SecretBytes,
) -> Result<()> {
    let authorized =
        authorize_exact_operation(approved, SecretStoreOperation::Write, handle, purpose, now)?;
    keychain
        .set_secret(authorized, service, handle, secret)
        .map_err(|error| keychain_public_error(error, "secure_mesh_keychain_write_failed"))
}

fn execute_get(
    service: &str,
    approved: &MacosApprovedPresenceBatch,
    now: Instant,
    keychain: &dyn MacosKeychainEffectPort,
    handle: &SecretStoreHandle,
    purpose: SecretStorePresencePurpose,
) -> Result<Option<SecretBytes>> {
    let authorized =
        authorize_exact_operation(approved, SecretStoreOperation::Read, handle, purpose, now)?;
    keychain
        .get_secret(authorized, service, handle)
        .map_err(|error| keychain_public_error(error, "secure_mesh_keychain_read_failed"))
}

fn execute_delete(
    service: &str,
    approved: &MacosApprovedPresenceBatch,
    now: Instant,
    keychain: &dyn MacosKeychainEffectPort,
    handle: &SecretStoreHandle,
    purpose: SecretStorePresencePurpose,
) -> Result<()> {
    let authorized =
        authorize_exact_operation(approved, SecretStoreOperation::Delete, handle, purpose, now)?;
    keychain
        .delete_secret(authorized, service, handle)
        .map_err(|error| keychain_public_error(error, "secure_mesh_keychain_delete_failed"))
}

fn execute_legacy_classic_get(
    service: &str,
    approved: &MacosApprovedPresenceBatch,
    now: Instant,
    keychain: &dyn MacosKeychainEffectPort,
    handle: &SecretStoreHandle,
    purpose: SecretStorePresencePurpose,
) -> Result<Option<SecretBytes>> {
    let authorized =
        authorize_exact_operation(approved, SecretStoreOperation::Read, handle, purpose, now)?;
    keychain
        .get_legacy_classic_secret(authorized, service, handle)
        .map_err(|error| keychain_public_error(error, "secure_mesh_keychain_read_failed"))
}

fn execute_legacy_classic_delete(
    service: &str,
    approved: &MacosApprovedPresenceBatch,
    now: Instant,
    keychain: &dyn MacosKeychainEffectPort,
    handle: &SecretStoreHandle,
    purpose: SecretStorePresencePurpose,
) -> Result<()> {
    let authorized =
        authorize_exact_operation(approved, SecretStoreOperation::Delete, handle, purpose, now)?;
    keychain
        .delete_legacy_classic_secret(authorized, service, handle)
        .map_err(|error| keychain_public_error(error, "secure_mesh_keychain_delete_failed"))
}

fn keychain_public_error(error: anyhow::Error, fallback: &'static str) -> anyhow::Error {
    if let Some(presence) = error.downcast_ref::<SecretStorePresenceError>() {
        return anyhow::Error::new(*presence);
    }
    match error.to_string().as_str() {
        "secure_mesh_keychain_classic_access_requires_user_action" => {
            anyhow!("secure_mesh_keychain_classic_access_requires_user_action")
        }
        "secure_mesh_keychain_interaction_control_failed" => {
            anyhow!("secure_mesh_keychain_interaction_control_failed")
        }
        _ => anyhow!(fallback),
    }
}

fn authorize_exact_operation(
    approved: &MacosApprovedPresenceBatch,
    operation: SecretStoreOperation,
    handle: &SecretStoreHandle,
    purpose: SecretStorePresencePurpose,
    now: Instant,
) -> Result<MacosAuthorizedPresence> {
    let scope =
        SecretStorePresenceScope::new(operation, handle.namespace(), handle.key(), purpose)?;
    let grant = approved.batch().issue_grant(scope.clone())?;
    let consumed = grant.consume(approved.batch(), &scope, now)?;
    approved
        .authorization_context()
        .authorize(consumed)
        .map_err(anyhow::Error::from)
}

pub struct SecurityFrameworkKeychain {
    sec_item_port: Arc<dyn MacosSecItemPort + Send + Sync>,
}

impl SecurityFrameworkKeychain {
    fn new() -> Self {
        Self {
            sec_item_port: Arc::new(SecurityFrameworkSecItem),
        }
    }

    #[cfg(test)]
    pub fn with_sec_item_port(sec_item_port: Arc<dyn MacosSecItemPort + Send + Sync>) -> Self {
        Self { sec_item_port }
    }
}

impl MacosKeychainEffectPort for SecurityFrameworkKeychain {
    fn set_secret(
        &self,
        authorized_presence: MacosAuthorizedPresence,
        service: &str,
        _handle: &SecretStoreHandle,
        secret: SecretBytes,
    ) -> Result<()> {
        // The sealed capability owns the effect target; this repeated interface argument
        // must never select a different Keychain item.
        let (context, bound_handle) =
            authorized_presence.into_authorized_effect(SecretStoreOperation::Write)?;
        let _effect_guard = context
            .effect_lock
            .lock()
            .map_err(|_| anyhow!("secure_mesh_presence_native_context_unavailable"))?;
        self.sec_item_port
            .set_secret(&context, service, &bound_handle, secret)
    }

    fn get_secret(
        &self,
        authorized_presence: MacosAuthorizedPresence,
        service: &str,
        _handle: &SecretStoreHandle,
    ) -> Result<Option<SecretBytes>> {
        // The sealed capability owns the effect target; this repeated interface argument
        // must never select a different Keychain item.
        let (context, bound_handle) =
            authorized_presence.into_authorized_effect(SecretStoreOperation::Read)?;
        let _effect_guard = context
            .effect_lock
            .lock()
            .map_err(|_| anyhow!("secure_mesh_presence_native_context_unavailable"))?;
        self.sec_item_port
            .get_secret(&context, service, &bound_handle)
    }

    fn delete_secret(
        &self,
        authorized_presence: MacosAuthorizedPresence,
        service: &str,
        _handle: &SecretStoreHandle,
    ) -> Result<()> {
        // The sealed capability owns the effect target; this repeated interface argument
        // must never select a different Keychain item.
        let (context, bound_handle) =
            authorized_presence.into_authorized_effect(SecretStoreOperation::Delete)?;
        let _effect_guard = context
            .effect_lock
            .lock()
            .map_err(|_| anyhow!("secure_mesh_presence_native_context_unavailable"))?;
        self.sec_item_port
            .delete_secret(&context, service, &bound_handle)
    }

    fn get_legacy_classic_secret(
        &self,
        authorized_presence: MacosAuthorizedPresence,
        service: &str,
        _handle: &SecretStoreHandle,
    ) -> Result<Option<SecretBytes>> {
        let (context, bound_handle) =
            authorized_presence.into_authorized_effect(SecretStoreOperation::Read)?;
        let _effect_guard = context
            .effect_lock
            .lock()
            .map_err(|_| anyhow!("secure_mesh_presence_native_context_unavailable"))?;
        self.sec_item_port
            .get_legacy_classic_secret(&context, service, &bound_handle)
    }

    fn delete_legacy_classic_secret(
        &self,
        authorized_presence: MacosAuthorizedPresence,
        service: &str,
        _handle: &SecretStoreHandle,
    ) -> Result<()> {
        let (context, bound_handle) =
            authorized_presence.into_authorized_effect(SecretStoreOperation::Delete)?;
        let _effect_guard = context
            .effect_lock
            .lock()
            .map_err(|_| anyhow!("secure_mesh_presence_native_context_unavailable"))?;
        self.sec_item_port
            .delete_legacy_classic_secret(&context, service, &bound_handle)
    }
}

struct SecurityFrameworkSecItem;

impl SecurityFrameworkSecItem {
    fn keychain() -> Result<Keychain> {
        Keychain::selected().ok_or_else(|| anyhow!("secure_mesh_keychain_unavailable"))
    }

    fn auth_context(
        authorization_context: &MacosAuthorizationContext,
    ) -> Result<KeychainAuthContext> {
        authorization_context
            .as_cf_type()
            .map(KeychainAuthContext::from_cf_type)
    }

    fn item_query<'a>(service: &'a str, account: &'a str) -> Result<KeychainItemQuery<'a>> {
        KeychainItemQuery::new(service, account)
            .map_err(|_| anyhow!("secure_mesh_keychain_item_invalid"))
    }

    fn lease_secret(secret: &SecretBytes) -> Result<credential_lease::SecretBytes> {
        credential_lease::SecretBytes::try_from_bytes(secret.expose_bytes().to_vec())
            .map_err(|_| anyhow!("secure_mesh_keychain_data_invalid"))
    }

    fn host_secret(secret: &credential_lease::SecretBytes) -> Result<SecretBytes> {
        SecretBytes::try_from_bytes(secret.expose_bytes().to_vec())
            .map_err(|_| anyhow!("secure_mesh_keychain_data_invalid"))
    }
}

/// The backend classifies every platform refusal; the host maps the
/// classification back to its stable public codes. `AuthorizationRequired`
/// additionally revokes the process-scoped presence cache: a lock, logout,
/// credential change, or invalidated LAContext means the next explicit
/// operation authenticates again instead of silently falling back to a
/// password prompt.
fn keychain_operation_error(error: KeychainError) -> anyhow::Error {
    match error.kind() {
        KeychainErrorKind::AuthorizationRequired => {
            crate::platform::user_presence::invalidate();
            SecretStorePresenceError::authorization_required().into()
        }
        KeychainErrorKind::ClassicAccessRequiresUserAction => {
            anyhow!("secure_mesh_keychain_classic_access_requires_user_action")
        }
        KeychainErrorKind::InteractionControlFailed => {
            anyhow!("secure_mesh_keychain_interaction_control_failed")
        }
        KeychainErrorKind::Failed => {
            let code = match error.code() {
                "credential_lease_keychain_write_failed" => "secure_mesh_keychain_write_failed",
                "credential_lease_keychain_read_failed" => "secure_mesh_keychain_read_failed",
                "credential_lease_keychain_delete_failed" => "secure_mesh_keychain_delete_failed",
                "credential_lease_keychain_add_failed" => "secure_mesh_keychain_add_failed",
                "credential_lease_keychain_access_control_unavailable" => {
                    "secure_mesh_keychain_access_control_unavailable"
                }
                "credential_lease_keychain_data_invalid" => "secure_mesh_keychain_data_invalid",
                "credential_lease_keychain_item_invalid" => "secure_mesh_keychain_item_invalid",
                "credential_lease_keychain_migration_target_invalid" => {
                    "secure_mesh_keychain_migration_target_invalid"
                }
                _ => "secure_mesh_keychain_operation_failed",
            };
            anyhow!(code)
        }
    }
}

impl MacosSecItemPort for SecurityFrameworkSecItem {
    fn set_secret(
        &self,
        authorization_context: &MacosAuthorizationContext,
        service: &str,
        handle: &SecretStoreHandle,
        secret: SecretBytes,
    ) -> Result<()> {
        let account = handle.account();
        Self::keychain()?
            .set_item(
                &Self::auth_context(authorization_context)?,
                &Self::item_query(service, &account)?,
                &Self::lease_secret(&secret)?,
            )
            .map_err(keychain_operation_error)
    }

    fn get_secret(
        &self,
        authorization_context: &MacosAuthorizationContext,
        service: &str,
        handle: &SecretStoreHandle,
    ) -> Result<Option<SecretBytes>> {
        let account = handle.account();
        Self::keychain()?
            .copy_secret(
                &Self::auth_context(authorization_context)?,
                &Self::item_query(service, &account)?,
            )
            .map_err(keychain_operation_error)?
            .map(|secret| Self::host_secret(&secret))
            .transpose()
    }

    fn delete_secret(
        &self,
        authorization_context: &MacosAuthorizationContext,
        service: &str,
        handle: &SecretStoreHandle,
    ) -> Result<()> {
        let account = handle.account();
        Self::keychain()?
            .delete_item(
                &Self::auth_context(authorization_context)?,
                &Self::item_query(service, &account)?,
            )
            .map_err(keychain_operation_error)
    }

    fn get_legacy_classic_secret(
        &self,
        authorization_context: &MacosAuthorizationContext,
        service: &str,
        handle: &SecretStoreHandle,
    ) -> Result<Option<SecretBytes>> {
        let account = handle.account();
        let keychain = Self::keychain()?;
        let auth = Self::auth_context(authorization_context)?;
        let query = Self::item_query(service, &account)?;
        authorization_context
            .with_legacy_classic_interaction(|allow_interaction| {
                keychain
                    .copy_legacy_classic_secret(&auth, &query, allow_interaction)
                    .map_err(keychain_operation_error)
            })?
            .map(|secret| Self::host_secret(&secret))
            .transpose()
    }

    fn delete_legacy_classic_secret(
        &self,
        authorization_context: &MacosAuthorizationContext,
        service: &str,
        handle: &SecretStoreHandle,
    ) -> Result<()> {
        let account = handle.account();
        let keychain = Self::keychain()?;
        let auth = Self::auth_context(authorization_context)?;
        let query = Self::item_query(service, &account)?;
        authorization_context.with_legacy_classic_interaction(|allow_interaction| {
            keychain
                .delete_legacy_classic_item(&auth, &query, allow_interaction)
                .map_err(keychain_operation_error)
        })
    }
}

/// Bounded runtime evidence for the selected macOS Keychain backend. A signed
/// production helper selects the Data Protection Keychain. Local builds that
/// cannot hold its required access-group entitlement select the classic macOS
/// Keychain instead. Classic application ACLs are separate from LocalAuthentication
/// and cannot be replaced by passing SecAccessControl to SecItemUpdate.
pub fn adaptive_keychain_roundtrip_probe() -> bool {
    Keychain::probe_available()
}

struct LocalAuthenticationPrompt {
    context: Option<MacosAuthorizationContext>,
}

impl LocalAuthenticationPrompt {
    fn new() -> Self {
        Self { context: None }
    }
}

impl MacosPresencePromptPort for LocalAuthenticationPrompt {
    fn prompt(&mut self, request: &SecretStorePresenceBatchRequest) -> Result<PresenceDecision> {
        match crate::platform::user_presence::authorize(
            request.reason(),
            "secure-mesh-secret-store",
        ) {
            Ok(session) => {
                let effect_lock = session.effect_lock();
                self.context = Some(MacosAuthorizationContext {
                    batch_binding_digest: [0; 32],
                    context: Some(session),
                    effect_lock,
                    legacy_credential_migration: None,
                });
                Ok(PresenceDecision::Approved)
            }
            Err(error) if user_presence_was_cancelled(&error) => Ok(PresenceDecision::Cancelled),
            Err(_) => Err(SecretStorePresenceError::authorization_failed().into()),
        }
    }

    fn take_authorization_context(&mut self) -> Option<MacosAuthorizationContext> {
        self.context.take()
    }
}

fn user_presence_was_cancelled(error: &anyhow::Error) -> bool {
    let code = error.to_string();
    code.ends_with(":user_cancelled")
        || code.ends_with(":system_cancelled")
        || code.ends_with(":application_cancelled")
        || code.ends_with(":password_fallback_blocked")
}

pub fn available() -> bool {
    if TEST_USER_PRESENCE_DISABLED.load(Ordering::SeqCst) || cfg!(test) {
        return false;
    }
    crate::platform::user_presence::available()
}

pub fn capability_facts() -> Vec<CapabilityFact> {
    [
        SecurityCapability::DeviceBound,
        SecurityCapability::UnlockedDeviceRequired,
        SecurityCapability::OsUserPresence,
        SecurityCapability::DeviceCredential,
        SecurityCapability::DataProtectionKeychain,
    ]
    .into_iter()
    .map(|capability| {
        CapabilityFact::supported(capability, CapabilityEvidenceKind::OsAuthorization)
    })
    .collect()
}

pub fn set_test_user_presence_disabled(disabled: bool) -> bool {
    TEST_USER_PRESENCE_DISABLED.swap(disabled, Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::secure_mesh_secret_store::{SecretStoreCallerChannel, SecretStoreKeyClass};

    #[test]
    fn migration_scope_binds_channel_and_revokes_issued_effects_on_completion() {
        struct Prompt;
        impl MacosPresencePromptPort for Prompt {
            fn prompt(&mut self, _: &SecretStorePresenceBatchRequest) -> Result<PresenceDecision> {
                Ok(PresenceDecision::Approved)
            }
        }
        let now = Instant::now();
        let request = SecretStoreAuthorizationRequest::for_scope(
            "Synthetic migration",
            4,
            true,
            SecretStoreKeyClass::GatewayCredential,
            SecretStoreCallerChannel::GatewayCredentialMigration,
        );
        let batch_request = SecretStorePresenceBatchRequest::new(
            SecretStorePresenceProvider::MacosKeychain,
            request.key_class(),
            4,
            request.reason(),
            SecretStorePresenceNonce::new("synthetic-migration-nonce").unwrap(),
            request.caller_channel(),
            true,
        )
        .unwrap();
        let access = MacosSecretStoreAccess::new(
            batch_request,
            now,
            now,
            Box::new(Prompt),
            Arc::new(SecurityFrameworkKeychain::new()),
        );
        for (key_class, caller_channel) in [
            (
                SecretStoreKeyClass::GatewayCredential,
                SecretStoreCallerChannel::GatewaySidecar,
            ),
            (
                SecretStoreKeyClass::DeviceIdentity,
                SecretStoreCallerChannel::GatewayCredentialMigration,
            ),
        ] {
            let wrong_request = SecretStoreAuthorizationRequest::for_scope(
                request.reason(),
                4,
                true,
                key_class,
                caller_channel,
            );
            assert_ne!(wrong_request.canonical_digest(), request.canonical_digest());
            assert_eq!(
                access
                    .begin_session("macos-keychain", &wrong_request)
                    .unwrap_err()
                    .to_string(),
                "secure_mesh_presence_batch_mismatch"
            );
        }
        let session = access.begin_session("macos-keychain", &request).unwrap();
        let approved = access.approved_for_session(&session).unwrap();
        let handle = SecretStoreHandle::new("synthetic-namespace", "synthetic-key").unwrap();
        let authorized = authorize_exact_operation(
            &approved,
            SecretStoreOperation::Read,
            &handle,
            SecretStorePresencePurpose::new("synthetic-read").unwrap(),
            now + std::time::Duration::from_secs(60),
        )
        .unwrap();
        assert!(
            approved
                .authorization_context()
                .migration_session_active_for_test()
        );
        access.finish_migration_session(&session);
        assert!(
            !approved
                .authorization_context()
                .migration_session_active_for_test()
        );
        assert_eq!(
            authorized
                .into_authorized_effect(SecretStoreOperation::Read)
                .unwrap_err()
                .to_string(),
            "secure_mesh_presence_session_closed"
        );
        assert!(access.approved_for_session(&session).is_err());
        assert_eq!(
            authorize_exact_operation(
                &approved,
                SecretStoreOperation::Read,
                &handle,
                SecretStorePresencePurpose::new("synthetic-read").unwrap(),
                now,
            )
            .unwrap_err()
            .to_string(),
            "secure_mesh_presence_session_closed"
        );
    }

    #[test]
    fn public_error_projection_never_exposes_platform_details() {
        for presence in [
            SecretStorePresenceError::authorization_required(),
            SecretStorePresenceError::authorization_failed(),
        ] {
            let projected = keychain_public_error(
                anyhow::Error::new(presence).context("synthetic private detail"),
                "secure_mesh_keychain_read_failed",
            );
            assert_eq!(
                projected.downcast_ref::<SecretStorePresenceError>(),
                Some(&presence)
            );
            assert_eq!(projected.to_string(), presence.code());
        }
        for code in [
            "secure_mesh_keychain_classic_access_requires_user_action",
            "secure_mesh_keychain_interaction_control_failed",
        ] {
            assert_eq!(
                keychain_public_error(anyhow!(code), "secure_mesh_keychain_read_failed")
                    .to_string(),
                code
            );
        }
        assert_eq!(
            keychain_public_error(
                anyhow!("synthetic private platform detail"),
                "secure_mesh_keychain_read_failed"
            )
            .to_string(),
            "secure_mesh_keychain_read_failed"
        );
    }
}
