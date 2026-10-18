use std::collections::HashMap;
use std::fmt;
use std::ptr;
use std::sync::{
    Arc, Condvar, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

use anyhow::{Result, anyhow};
use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::data::CFData;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;
use core_foundation_sys::base::{CFGetTypeID, CFRelease, CFTypeRef};
use core_foundation_sys::data::CFDataRef;
use core_foundation_sys::string::CFStringRef;
use security_framework::access_control::{ProtectionMode, SecAccessControl};
use security_framework_sys::access_control::kSecAccessControlUserPresence;
use security_framework_sys::base::{
    errSecAuthFailed, errSecDuplicateItem, errSecItemNotFound, errSecSuccess,
};
use security_framework_sys::item::{
    kSecAttrAccessControl, kSecAttrAccount, kSecAttrService, kSecClass, kSecClassGenericPassword,
    kSecReturnData, kSecUseAuthenticationContext, kSecUseDataProtectionKeychain, kSecValueData,
};
use security_framework_sys::keychain::{
    SecKeychainGetUserInteractionAllowed, SecKeychainSetUserInteractionAllowed,
};
use security_framework_sys::keychain_item::{
    SecItemAdd, SecItemCopyMatching, SecItemDelete, SecItemUpdate,
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

macro_rules! security_framework_static {
    ($value:expr, $wrapper:ident) => {{
        // SAFETY: every invocation passes a Security.framework SDK-exported static CFString.
        unsafe { $wrapper($value) }
    }};
}

const MACOS_AUTHORIZATION_CACHE_MAX_BATCHES: usize = 16;
const ERR_SEC_INTERACTION_NOT_ALLOWED: i32 = -25308;

static TEST_USER_PRESENCE_DISABLED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MacosKeychainBackend {
    DataProtection,
    Classic,
}

static SELECTED_KEYCHAIN_BACKEND: OnceLock<Option<MacosKeychainBackend>> = OnceLock::new();

// The classic Keychain has a process-wide UI switch, independent of LAContext.
// Serialize temporary changes so migration consent cannot enable UI for a
// concurrent ordinary batch. This never changes an ACL.
static CLASSIC_KEYCHAIN_INTERACTION: Mutex<()> = Mutex::new(());

trait ClassicKeychainInteraction {
    fn allowed(&self) -> Result<bool>;
    fn set_allowed(&self, allowed: bool) -> Result<()>;
}

struct SystemClassicKeychainInteraction;

impl ClassicKeychainInteraction for SystemClassicKeychainInteraction {
    fn allowed(&self) -> Result<bool> {
        let mut allowed = 0;
        // SAFETY: allowed is a valid initialized Boolean out-pointer.
        let status = unsafe { SecKeychainGetUserInteractionAllowed(&mut allowed) };
        if status != errSecSuccess {
            return Err(anyhow!("secure_mesh_keychain_interaction_control_failed"));
        }
        Ok(allowed != 0)
    }

    fn set_allowed(&self, allowed: bool) -> Result<()> {
        // SAFETY: this changes only the process's legacy Keychain UI policy.
        let status = unsafe { SecKeychainSetUserInteractionAllowed(u8::from(allowed)) };
        if status != errSecSuccess {
            return Err(anyhow!("secure_mesh_keychain_interaction_control_failed"));
        }
        Ok(())
    }
}

struct ClassicInteractionRestore<'a> {
    port: &'a dyn ClassicKeychainInteraction,
    was_allowed: Option<bool>,
}

impl Drop for ClassicInteractionRestore<'_> {
    fn drop(&mut self) {
        // Restore the exact previous state, including a caller's pre-existing
        // UI prohibition. The normal path reports a failed restore.
        if let Some(was_allowed) = self.was_allowed.take() {
            let _ = self.port.set_allowed(was_allowed);
        }
    }
}

fn without_classic_keychain_interaction<T>(
    port: &dyn ClassicKeychainInteraction,
    effect: impl FnOnce() -> Result<T>,
) -> Result<T> {
    with_classic_keychain_interaction(port, false, effect)
}

fn with_classic_keychain_interaction<T>(
    port: &dyn ClassicKeychainInteraction,
    allowed: bool,
    effect: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let _lock = CLASSIC_KEYCHAIN_INTERACTION
        .lock()
        .map_err(|_| anyhow!("secure_mesh_keychain_interaction_control_failed"))?;
    let was_allowed = port.allowed()?;
    port.set_allowed(allowed)?;
    let mut restore = ClassicInteractionRestore {
        port,
        was_allowed: Some(was_allowed),
    };
    let result = effect();
    let restored = port.set_allowed(was_allowed);
    // The explicit restore reports failure on the normal path; Drop also
    // restores during unwinding, before releasing the serialization lock.
    restore.was_allowed = None;
    restored?;
    result
}

fn with_keychain_backend<T>(
    backend: MacosKeychainBackend,
    effect: impl FnOnce() -> Result<T>,
) -> Result<T> {
    match backend {
        MacosKeychainBackend::DataProtection => effect(),
        MacosKeychainBackend::Classic => {
            without_classic_keychain_interaction(&SystemClassicKeychainInteraction, effect)
        }
    }
}

fn keychain_effect(backend: MacosKeychainBackend, effect: impl FnOnce() -> i32) -> Result<i32> {
    with_keychain_backend(backend, || keychain_effect_status(backend, effect()))
}

fn keychain_effect_status(backend: MacosKeychainBackend, status: i32) -> Result<i32> {
    if backend == MacosKeychainBackend::Classic
        && (status == errSecAuthFailed || status == ERR_SEC_INTERACTION_NOT_ALLOWED)
    {
        return Err(anyhow!(
            "secure_mesh_keychain_classic_access_requires_user_action"
        ));
    }
    Ok(status)
}

fn required_keychain_backend() -> Result<MacosKeychainBackend> {
    selected_keychain_backend().ok_or_else(|| anyhow!("secure_mesh_keychain_unavailable"))
}

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

    fn with_legacy_classic_interaction<T>(&self, effect: impl FnOnce() -> Result<T>) -> Result<T> {
        if !self.is_active() {
            return Err(anyhow!("secure_mesh_presence_session_closed"));
        }
        with_classic_keychain_interaction(
            &SystemClassicKeychainInteraction,
            self.legacy_credential_migration.is_some(),
            effect,
        )
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
        && required_keychain_backend()? != MacosKeychainBackend::DataProtection
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

impl MacosSecItemPort for SecurityFrameworkSecItem {
    fn set_secret(
        &self,
        authorization_context: &MacosAuthorizationContext,
        service: &str,
        handle: &SecretStoreHandle,
        secret: SecretBytes,
    ) -> Result<()> {
        let account = handle.account();
        let backend = required_keychain_backend()?;
        let query =
            CFDictionary::from_CFType_pairs(&base_pairs(service, &account, authorization_context)?);
        // Data Protection applies SecAccessControl. The classic Keychain has
        // separate application ACLs; this update cannot convert those ACLs.
        let update =
            CFDictionary::from_CFType_pairs(&protected_secret_pairs(secret.expose_bytes())?);
        // SAFETY: query and update own their CF values for the synchronous call.
        let update_status = keychain_effect(backend, || unsafe {
            SecItemUpdate(query.as_concrete_TypeRef(), update.as_concrete_TypeRef())
        })?;
        match keychain_update_transition(update_status) {
            KeychainUpdateTransition::Complete => return Ok(()),
            KeychainUpdateTransition::AddNew => {}
            KeychainUpdateTransition::Fail(status) => {
                return status_result("write", status);
            }
        }

        let mut pairs = base_pairs(service, &account, authorization_context)?;
        pairs.extend(protected_secret_pairs(secret.expose_bytes())?);
        let add_query = CFDictionary::from_CFType_pairs(&pairs);
        // SAFETY: add_query owns all referenced values for the synchronous call.
        let add_status = keychain_effect(backend, || unsafe {
            SecItemAdd(add_query.as_concrete_TypeRef(), ptr::null_mut())
        })?;
        if add_status == errSecSuccess {
            return Ok(());
        }
        if add_status == errSecDuplicateItem {
            // SAFETY: query and update remain valid for the synchronous retry.
            let retry_status = keychain_effect(backend, || unsafe {
                SecItemUpdate(query.as_concrete_TypeRef(), update.as_concrete_TypeRef())
            })?;
            return status_result("write", retry_status);
        }
        status_result("write", add_status)
    }

    fn get_secret(
        &self,
        authorization_context: &MacosAuthorizationContext,
        service: &str,
        handle: &SecretStoreHandle,
    ) -> Result<Option<SecretBytes>> {
        let account = handle.account();
        let mut pairs = base_pairs(service, &account, authorization_context)?;
        pairs.push((
            security_framework_static!(kSecReturnData, sec_key),
            CFBoolean::from(true).into_CFType(),
        ));
        copy_secret_from_query(&pairs, required_keychain_backend()?)
    }

    fn delete_secret(
        &self,
        authorization_context: &MacosAuthorizationContext,
        service: &str,
        handle: &SecretStoreHandle,
    ) -> Result<()> {
        let account = handle.account();
        let query =
            CFDictionary::from_CFType_pairs(&base_pairs(service, &account, authorization_context)?);
        // SAFETY: query owns valid CF values for the synchronous call.
        let status = keychain_effect(required_keychain_backend()?, || unsafe {
            SecItemDelete(query.as_concrete_TypeRef())
        })?;
        if status == errSecItemNotFound {
            Ok(())
        } else {
            status_result("delete", status)
        }
    }

    fn get_legacy_classic_secret(
        &self,
        authorization_context: &MacosAuthorizationContext,
        service: &str,
        handle: &SecretStoreHandle,
    ) -> Result<Option<SecretBytes>> {
        let account = handle.account();
        let Some(mut pairs) = classic_migration_pairs(service, &account, authorization_context)?
        else {
            return Ok(None);
        };
        pairs.push((
            security_framework_static!(kSecReturnData, sec_key),
            CFBoolean::from(true).into_CFType(),
        ));
        authorization_context.with_legacy_classic_interaction(|| {
            copy_secret_from_query_effect(&pairs, MacosKeychainBackend::Classic)
        })
    }

    fn delete_legacy_classic_secret(
        &self,
        authorization_context: &MacosAuthorizationContext,
        service: &str,
        handle: &SecretStoreHandle,
    ) -> Result<()> {
        let account = handle.account();
        let pairs = classic_migration_pairs(service, &account, authorization_context)?
            .ok_or_else(|| anyhow!("secure_mesh_keychain_migration_target_invalid"))?;
        let query = CFDictionary::from_CFType_pairs(&pairs);
        // SAFETY: query owns valid CF values for the synchronous call.
        let status = authorization_context.with_legacy_classic_interaction(|| {
            // SAFETY: query owns valid CF values for the synchronous call.
            keychain_effect_status(MacosKeychainBackend::Classic, unsafe {
                SecItemDelete(query.as_concrete_TypeRef())
            })
        })?;
        if status == errSecItemNotFound {
            Ok(())
        } else {
            status_result("delete", status)
        }
    }
}

fn protected_secret_pairs(secret: &[u8]) -> Result<Vec<(CFString, CFType)>> {
    let access_control = SecAccessControl::create_with_protection(
        Some(ProtectionMode::AccessibleWhenUnlockedThisDeviceOnly),
        kSecAccessControlUserPresence,
    )
    .map_err(|_| anyhow!("secure_mesh_keychain_access_control_unavailable"))?;
    Ok(vec![
        (
            security_framework_static!(kSecAttrAccessControl, sec_key),
            access_control.into_CFType(),
        ),
        (
            security_framework_static!(kSecValueData, sec_key),
            CFData::from_buffer(secret).into_CFType(),
        ),
    ])
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum KeychainUpdateTransition {
    Complete,
    AddNew,
    Fail(i32),
}

fn keychain_update_transition(status: i32) -> KeychainUpdateTransition {
    if status == errSecSuccess {
        KeychainUpdateTransition::Complete
    } else if status == errSecItemNotFound {
        KeychainUpdateTransition::AddNew
    } else {
        KeychainUpdateTransition::Fail(status)
    }
}

fn base_pairs(
    service: &str,
    account: &str,
    authorization_context: &MacosAuthorizationContext,
) -> Result<Vec<(CFString, CFType)>> {
    let backend = required_keychain_backend()?;
    let mut pairs = keychain_query_pairs(
        service,
        account,
        authorization_context.as_cf_type()?,
        backend,
    );
    if backend == MacosKeychainBackend::Classic {
        // Preserve the existing SecItem routing for local-build writes. In
        // particular, an add carrying SecAccessControl must not be forced into
        // the classic store, whose shim would discard that protection. Only
        // the explicit legacy migration path forces a classic-only query.
        pairs.pop();
    }
    Ok(pairs)
}

fn keychain_query_pairs(
    service: &str,
    account: &str,
    context: CFType,
    backend: MacosKeychainBackend,
) -> Vec<(CFString, CFType)> {
    vec![
        (
            security_framework_static!(kSecClass, sec_key),
            security_framework_static!(kSecClassGenericPassword, sec_string_value),
        ),
        (
            security_framework_static!(kSecAttrService, sec_key),
            CFString::from(service).into_CFType(),
        ),
        (
            security_framework_static!(kSecAttrAccount, sec_key),
            CFString::from(account).into_CFType(),
        ),
        (
            security_framework_static!(kSecUseAuthenticationContext, sec_key),
            context,
        ),
        (
            security_framework_static!(kSecUseDataProtectionKeychain, sec_key),
            CFBoolean::from(backend == MacosKeychainBackend::DataProtection).into_CFType(),
        ),
    ]
}

fn classic_migration_pairs(
    service: &str,
    account: &str,
    authorization_context: &MacosAuthorizationContext,
) -> Result<Option<Vec<(CFString, CFType)>>> {
    Ok(classic_migration_query_pairs(
        service,
        account,
        authorization_context.as_cf_type()?,
        required_keychain_backend()?,
    ))
}

fn classic_migration_query_pairs(
    service: &str,
    account: &str,
    context: CFType,
    destination: MacosKeychainBackend,
) -> Option<Vec<(CFString, CFType)>> {
    // A same-store copy followed by delete is not a migration. In particular,
    // an item created between two classic reads must never be copied/deleted.
    (destination == MacosKeychainBackend::DataProtection)
        .then(|| keychain_query_pairs(service, account, context, MacosKeychainBackend::Classic))
}

fn copy_secret_from_query(
    pairs: &[(CFString, CFType)],
    backend: MacosKeychainBackend,
) -> Result<Option<SecretBytes>> {
    with_keychain_backend(backend, || copy_secret_from_query_effect(pairs, backend))
}

fn copy_secret_from_query_effect(
    pairs: &[(CFString, CFType)],
    backend: MacosKeychainBackend,
) -> Result<Option<SecretBytes>> {
    let query = CFDictionary::from_CFType_pairs(pairs);
    let mut copied: CFTypeRef = ptr::null();
    // SAFETY: query is valid and copied is an initialized out-pointer.
    let status = unsafe { SecItemCopyMatching(query.as_concrete_TypeRef(), &mut copied) };
    // Own any result before resolving status or restoring classic UI. In
    // particular, a restore failure must drop the returned secret object.
    let value = if copied.is_null() {
        None
    } else {
        // SAFETY: SecItemCopyMatching returns its object at +1 ownership.
        Some(unsafe { CFType::wrap_under_create_rule(copied) })
    };
    let status = keychain_effect_status(backend, status)?;
    if status == errSecItemNotFound {
        return Ok(None);
    }
    status_result("read", status)?;
    let Some(value) = value else {
        return Ok(None);
    };
    let data = value
        .downcast::<CFData>()
        .ok_or_else(|| anyhow!("secure_mesh_keychain_data_invalid"))?;
    SecretBytes::try_from_bytes(data.bytes().to_vec())
        .map(Some)
        .map_err(|_| anyhow!("secure_mesh_keychain_data_invalid"))
}

fn selected_keychain_backend() -> Option<MacosKeychainBackend> {
    *SELECTED_KEYCHAIN_BACKEND.get_or_init(|| {
        if keychain_roundtrip_probe(MacosKeychainBackend::DataProtection) {
            Some(MacosKeychainBackend::DataProtection)
        } else if keychain_roundtrip_probe(MacosKeychainBackend::Classic) {
            Some(MacosKeychainBackend::Classic)
        } else {
            None
        }
    })
}

/// Bounded runtime evidence for the selected macOS Keychain backend. A signed
/// production helper selects the Data Protection Keychain. Local builds that
/// cannot hold its required access-group entitlement select the classic macOS
/// Keychain instead. Classic application ACLs are separate from LocalAuthentication
/// and cannot be replaced by passing SecAccessControl to SecItemUpdate.
pub fn adaptive_keychain_roundtrip_probe() -> bool {
    selected_keychain_backend().is_some()
}

fn keychain_roundtrip_probe(backend: MacosKeychainBackend) -> bool {
    with_keychain_backend(backend, || Ok(silent_keychain_roundtrip_probe(backend))).unwrap_or(false)
}

fn silent_keychain_roundtrip_probe(backend: MacosKeychainBackend) -> bool {
    const PROBE_SERVICE: &str = "dev.licoland.licoup.secure-mesh-probe";
    const PROBE_SECRET: &[u8] = b"secure-mesh-keychain-probe";
    let account = format!("probe-{}", Uuid::new_v4());

    let mut pairs = vec![
        (
            security_framework_static!(kSecClass, sec_key),
            security_framework_static!(kSecClassGenericPassword, sec_string_value),
        ),
        (
            security_framework_static!(kSecAttrService, sec_key),
            CFString::from(PROBE_SERVICE).into_CFType(),
        ),
        (
            security_framework_static!(kSecAttrAccount, sec_key),
            CFString::from(account.as_str()).into_CFType(),
        ),
    ];
    pairs.push((
        security_framework_static!(kSecUseDataProtectionKeychain, sec_key),
        CFBoolean::from(backend == MacosKeychainBackend::DataProtection).into_CFType(),
    ));
    // The production write always carries a SecAccessControl; probing without
    // one would pass on ad hoc builds that cannot actually persist a protected
    // item (errSecMissingEntitlement) and misreport the vault as available. A
    // plain ACL exercises the same boundary without any interactive readback.
    let Ok(probe_access) =
        SecAccessControl::create_with_protection(Some(ProtectionMode::AccessibleWhenUnlocked), 0)
    else {
        return false;
    };
    pairs.push((
        security_framework_static!(kSecAttrAccessControl, sec_key),
        probe_access.into_CFType(),
    ));
    let query_pair_count = pairs.len() - 1;
    pairs.push((
        security_framework_static!(kSecValueData, sec_key),
        CFData::from_buffer(PROBE_SECRET).into_CFType(),
    ));
    let add_query = CFDictionary::from_CFType_pairs(&pairs);
    // SAFETY: add_query owns all referenced values for the synchronous call.
    let add_status = unsafe { SecItemAdd(add_query.as_concrete_TypeRef(), ptr::null_mut()) };
    if add_status != errSecSuccess {
        return false;
    }

    let mut query_pairs = pairs
        .iter()
        .take(query_pair_count)
        .cloned()
        .collect::<Vec<(CFString, CFType)>>();
    query_pairs.push((
        security_framework_static!(kSecReturnData, sec_key),
        CFBoolean::from(true).into_CFType(),
    ));
    let read_query = CFDictionary::from_CFType_pairs(&query_pairs);
    let mut copied: CFTypeRef = ptr::null();
    // SAFETY: read_query is valid and copied is an initialized out-pointer.
    let read_status = unsafe { SecItemCopyMatching(read_query.as_concrete_TypeRef(), &mut copied) };
    let read_ok = if read_status == errSecSuccess && !copied.is_null() {
        // SAFETY: copied is a live CF object returned at +1 ownership.
        let type_id = unsafe { CFGetTypeID(copied) };
        if type_id == CFData::type_id() {
            // SAFETY: type identity is CFData and +1 ownership transfers to the wrapper.
            let data = unsafe { CFData::wrap_under_create_rule(copied as CFDataRef) };
            data.bytes() == PROBE_SECRET
        } else {
            // SAFETY: copied has not yet been released.
            unsafe { CFRelease(copied) };
            false
        }
    } else {
        false
    };

    let delete_pairs = pairs
        .iter()
        .take(query_pair_count)
        .cloned()
        .collect::<Vec<(CFString, CFType)>>();
    let delete_query = CFDictionary::from_CFType_pairs(&delete_pairs);
    // SAFETY: delete_query owns valid CF values for the synchronous call.
    let delete_status = unsafe { SecItemDelete(delete_query.as_concrete_TypeRef()) };
    read_ok && delete_status == errSecSuccess
}

fn status_result(operation: &'static str, status: i32) -> Result<()> {
    if status == errSecSuccess {
        return Ok(());
    }
    if status == errSecAuthFailed || status == ERR_SEC_INTERACTION_NOT_ALLOWED {
        // A lock, logout, credential change, or invalidated LAContext revokes
        // the process-scoped grant. The next explicit operation authenticates
        // again instead of silently falling back to a password prompt.
        crate::platform::user_presence::invalidate();
        return Err(SecretStorePresenceError::authorization_required().into());
    }
    let code = match operation {
        "write" => "secure_mesh_keychain_write_failed",
        "read" => "secure_mesh_keychain_read_failed",
        "delete" => "secure_mesh_keychain_delete_failed",
        _ => "secure_mesh_keychain_operation_failed",
    };
    Err(anyhow!(code))
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

fn sec_key(value: CFStringRef) -> CFString {
    // SAFETY: callers supply SDK-exported static CFString constants.
    unsafe { CFString::wrap_under_get_rule(value) }
}

fn sec_string_value(value: CFStringRef) -> CFType {
    sec_key(value).into_CFType()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::secure_mesh_secret_store::{SecretStoreCallerChannel, SecretStoreKeyClass};
    use std::cell::{Cell, RefCell};

    struct SyntheticInteraction {
        allowed: Cell<bool>,
        changes: RefCell<Vec<bool>>,
        fail_disable: bool,
        fail_restore: bool,
    }

    impl ClassicKeychainInteraction for SyntheticInteraction {
        fn allowed(&self) -> Result<bool> {
            Ok(self.allowed.get())
        }

        fn set_allowed(&self, allowed: bool) -> Result<()> {
            self.changes.borrow_mut().push(allowed);
            if (self.fail_disable && !allowed) || (self.fail_restore && allowed) {
                return Err(anyhow!("synthetic_interaction_control_failed"));
            }
            self.allowed.set(allowed);
            Ok(())
        }
    }

    #[test]
    fn classic_batch_suppresses_per_item_ui_and_restores_exact_policy() {
        for was_allowed in [false, true] {
            let port = SyntheticInteraction {
                allowed: Cell::new(was_allowed),
                changes: RefCell::default(),
                fail_disable: false,
                fail_restore: false,
            };
            let reads = Cell::new(0);
            let result = without_classic_keychain_interaction(&port, || {
                for _ in 0..2 {
                    assert!(!port.allowed.get());
                    reads.set(reads.get() + 1);
                }
                Err::<(), _>(anyhow!("synthetic_legacy_acl_denied"))
            });
            assert_eq!(
                result.unwrap_err().to_string(),
                "synthetic_legacy_acl_denied"
            );
            assert_eq!(reads.get(), 2);
            assert_eq!(port.allowed.get(), was_allowed);
            assert_eq!(*port.changes.borrow(), [false, was_allowed]);
        }
    }

    #[test]
    fn legacy_migration_ui_restores_both_previous_policies_after_cancel() {
        for was_allowed in [false, true] {
            let port = SyntheticInteraction {
                allowed: Cell::new(was_allowed),
                changes: RefCell::default(),
                fail_disable: false,
                fail_restore: false,
            };
            let result = with_classic_keychain_interaction(&port, true, || {
                assert!(port.allowed.get());
                Err::<(), _>(anyhow!("synthetic_cancelled"))
            });
            assert!(result.is_err());
            assert_eq!(port.allowed.get(), was_allowed);
            assert_eq!(*port.changes.borrow(), [true, was_allowed]);
        }
    }

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
    fn classic_ui_control_failure_prevents_any_keychain_effect() {
        let port = SyntheticInteraction {
            allowed: Cell::new(true),
            changes: RefCell::default(),
            fail_disable: true,
            fail_restore: false,
        };
        let reads = Cell::new(0);
        let result = without_classic_keychain_interaction(&port, || {
            reads.set(reads.get() + 1);
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(reads.get(), 0);
        assert!(port.allowed.get());
    }

    #[test]
    fn query_targets_exact_keychain_and_retains_one_authorization_context() {
        let context = CFBoolean::true_value().into_CFType();
        for backend in [
            MacosKeychainBackend::Classic,
            MacosKeychainBackend::DataProtection,
        ] {
            for account in ["synthetic-key-one", "synthetic-key-two"] {
                let pairs =
                    keychain_query_pairs("synthetic-service", account, context.clone(), backend);
                let dict = CFDictionary::from_CFType_pairs(&pairs);
                assert_eq!(
                    *dict.get(security_framework_static!(
                        kSecUseDataProtectionKeychain,
                        sec_key
                    )),
                    CFBoolean::from(backend == MacosKeychainBackend::DataProtection).into_CFType()
                );
                assert_eq!(
                    dict.get(security_framework_static!(
                        kSecUseAuthenticationContext,
                        sec_key
                    ))
                    .as_CFTypeRef(),
                    context.as_CFTypeRef()
                );
            }
        }
    }

    #[test]
    fn classic_migration_requires_a_distinct_data_protection_destination() {
        let context = CFBoolean::true_value().into_CFType();
        assert!(
            classic_migration_query_pairs(
                "synthetic-service",
                "same-item",
                context.clone(),
                MacosKeychainBackend::Classic
            )
            .is_none()
        );
        let pairs = classic_migration_query_pairs(
            "synthetic-service",
            "same-item",
            context,
            MacosKeychainBackend::DataProtection,
        )
        .unwrap();
        let dict = CFDictionary::from_CFType_pairs(&pairs);
        assert_eq!(
            *dict.get(security_framework_static!(
                kSecUseDataProtectionKeychain,
                sec_key
            )),
            CFBoolean::false_value().into_CFType()
        );
    }

    #[test]
    fn classic_restore_failure_drops_successful_read_result() {
        struct ReadResult<'a>(&'a Cell<usize>);
        impl Drop for ReadResult<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let port = SyntheticInteraction {
            allowed: Cell::new(true),
            changes: RefCell::default(),
            fail_disable: false,
            fail_restore: true,
        };
        let drops = Cell::new(0);
        let result = without_classic_keychain_interaction(&port, || Ok(ReadResult(&drops)));
        assert!(result.is_err());
        assert_eq!(drops.get(), 1);
        assert!(!port.allowed.get());
    }

    #[test]
    fn concurrent_classic_batches_never_restore_ui_during_another_effect() {
        use std::sync::{Barrier, atomic::AtomicUsize};

        struct Interaction(AtomicBool);
        impl ClassicKeychainInteraction for Interaction {
            fn allowed(&self) -> Result<bool> {
                Ok(self.0.load(Ordering::SeqCst))
            }
            fn set_allowed(&self, allowed: bool) -> Result<()> {
                self.0.store(allowed, Ordering::SeqCst);
                Ok(())
            }
        }
        let port = Interaction(AtomicBool::new(true));
        let start = Barrier::new(2);
        let active = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            let port = &port;
            let start = &start;
            let active = &active;
            for allow_migration_ui in [false, true] {
                scope.spawn(move || {
                    start.wait();
                    for _ in 0..16 {
                        with_classic_keychain_interaction(port, allow_migration_ui, || {
                            assert_eq!(active.fetch_add(1, Ordering::SeqCst), 0);
                            assert_eq!(port.allowed()?, allow_migration_ui);
                            std::thread::yield_now();
                            assert_eq!(port.allowed()?, allow_migration_ui);
                            assert_eq!(active.fetch_sub(1, Ordering::SeqCst), 1);
                            Ok(())
                        })
                        .unwrap();
                    }
                });
            }
        });
        assert!(port.allowed().unwrap());
    }

    #[test]
    fn classic_access_denial_is_actionable_without_exposing_platform_errors() {
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
        for status in [errSecAuthFailed, ERR_SEC_INTERACTION_NOT_ALLOWED] {
            let error = keychain_effect_status(MacosKeychainBackend::Classic, status).unwrap_err();
            assert_eq!(
                keychain_public_error(error, "secure_mesh_keychain_read_failed").to_string(),
                "secure_mesh_keychain_classic_access_requires_user_action"
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

    #[test]
    fn update_failure_never_falls_through_to_add() {
        assert_eq!(
            keychain_update_transition(errSecAuthFailed),
            KeychainUpdateTransition::Fail(errSecAuthFailed)
        );
        assert_eq!(
            keychain_update_transition(ERR_SEC_INTERACTION_NOT_ALLOWED),
            KeychainUpdateTransition::Fail(ERR_SEC_INTERACTION_NOT_ALLOWED)
        );
    }

    #[test]
    fn add_is_reached_only_after_verified_not_found() {
        assert_eq!(
            keychain_update_transition(errSecSuccess),
            KeychainUpdateTransition::Complete
        );
        assert_eq!(
            keychain_update_transition(errSecItemNotFound),
            KeychainUpdateTransition::AddNew
        );
    }

    #[test]
    fn existing_item_updates_include_user_presence_access_control() {
        let pairs = protected_secret_pairs(b"synthetic-secret").unwrap();
        assert_eq!(pairs.len(), 2);
        assert_eq!(
            pairs[0].0,
            security_framework_static!(kSecAttrAccessControl, sec_key)
        );
        assert_eq!(
            pairs[1].0,
            security_framework_static!(kSecValueData, sec_key)
        );
    }
}
