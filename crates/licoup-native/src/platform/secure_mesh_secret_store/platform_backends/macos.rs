use std::sync::Arc;

use anyhow::{Result, anyhow, ensure};

use super::super::capability::{
    PlatformSecretStoreRuntimeState, platform_native_secret_store_runtime_state,
    platform_secret_store_capability_facts,
};
use super::super::macos_user_presence;
use super::super::platform_store::PlatformSecretStore;
use super::fail_closed;
use crate::core::secure_mesh_capability::{
    CapabilityEvidenceKind, CapabilityFact, capability_catalog, mandatory_protocol_facts,
};
use crate::core::secure_mesh_secret_store::{
    SecretBytes, SecretStoreAuthorizationRequest, SecretStoreAuthorizationSession,
    SecretStoreCallerChannel, SecretStoreHandle, SecureMeshSecretStore,
};

pub(super) const BACKEND: &str = "macos-keychain";

pub(super) fn supported() -> bool {
    platform_native_secret_store_runtime_state() == PlatformSecretStoreRuntimeState::Available
}

pub(super) fn capability_facts() -> Result<Vec<CapabilityFact>> {
    platform_secret_store_capability_facts()
}

pub(super) fn begin_authorized_session(
    store: &PlatformSecretStore,
    request: &SecretStoreAuthorizationRequest,
) -> Result<SecretStoreAuthorizationSession> {
    ensure!(
        request.caller_channel() != SecretStoreCallerChannel::GatewayCredentialMigration,
        "secure_mesh_keychain_migration_scope_required"
    );
    if request.allow_interaction() {
        if let Some(access) = store
            .macos_secret_store_access()?
            .filter(|access| access.is_injected())
        {
            return access.begin_session(store.backend(), request);
        }
        ensure!(
            macos_user_presence::available(),
            "secure mesh macOS user-presence authorization is unavailable"
        );
        let mut facts = store.capability_facts()?;
        facts.extend(macos_user_presence::capability_facts());
        let mut protocol = mandatory_protocol_facts(CapabilityEvidenceKind::SourceContract)?;
        protocol.extend(facts);
        let report = capability_catalog()?.evaluate(&protocol)?.report();
        let access = Arc::new(macos_user_presence::production_access(request)?);
        let session = access
            .begin_session(store.backend(), request)?
            .with_capability_report(report);
        store.select_macos_secret_store_access(access)?;
        return Ok(session);
    }
    fail_closed::begin_authorized_session(store, request)
}

pub(super) fn set_secret_with_session(
    store: &PlatformSecretStore,
    session: &SecretStoreAuthorizationSession,
    handle: &SecretStoreHandle,
    secret: SecretBytes,
) -> Result<()> {
    store
        .macos_secret_store_access()?
        .ok_or_else(|| anyhow!("secure_mesh_presence_session_batch_mismatch"))?
        .set_secret(store.service, session, handle, secret)
}

pub(super) fn get_secret_with_session(
    store: &PlatformSecretStore,
    session: &SecretStoreAuthorizationSession,
    handle: &SecretStoreHandle,
) -> Result<Option<SecretBytes>> {
    store
        .macos_secret_store_access()?
        .ok_or_else(|| anyhow!("secure_mesh_presence_session_batch_mismatch"))?
        .get_secret(store.service, session, handle)
}

pub(super) fn delete_secret_with_session(
    store: &PlatformSecretStore,
    session: &SecretStoreAuthorizationSession,
    handle: &SecretStoreHandle,
) -> Result<()> {
    store
        .macos_secret_store_access()?
        .ok_or_else(|| anyhow!("secure_mesh_presence_session_batch_mismatch"))?
        .delete_secret(store.service, session, handle)
}

pub(super) fn set_secret(
    store: &PlatformSecretStore,
    handle: &SecretStoreHandle,
    secret: SecretBytes,
) -> Result<()> {
    fail_closed::set_secret(store, handle, secret)
}

pub(super) fn get_secret(
    store: &PlatformSecretStore,
    handle: &SecretStoreHandle,
) -> Result<Option<SecretBytes>> {
    fail_closed::get_secret(store, handle)
}

pub(super) fn delete_secret(store: &PlatformSecretStore, handle: &SecretStoreHandle) -> Result<()> {
    fail_closed::delete_secret(store, handle)
}
