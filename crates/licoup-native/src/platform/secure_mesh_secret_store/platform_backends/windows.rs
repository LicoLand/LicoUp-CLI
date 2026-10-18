use anyhow::Result;

use super::super::capability::platform_secret_store_capability_facts;
use super::super::platform_store::PlatformSecretStore;
use super::fail_closed;
use crate::core::secure_mesh_capability::CapabilityFact;
use crate::core::secure_mesh_secret_store::{
    SecretBytes, SecretStoreAuthorizationRequest, SecretStoreAuthorizationSession,
    SecretStoreHandle,
};

pub(super) const BACKEND: &str = "windows-credential-manager";

pub(super) fn supported() -> bool {
    false
}

pub(super) fn capability_facts() -> Result<Vec<CapabilityFact>> {
    platform_secret_store_capability_facts()
}

pub(super) fn begin_authorized_session(
    store: &PlatformSecretStore,
    request: &SecretStoreAuthorizationRequest,
) -> Result<SecretStoreAuthorizationSession> {
    fail_closed::begin_authorized_session(store, request)
}

pub(super) fn set_secret_with_session(
    store: &PlatformSecretStore,
    session: &SecretStoreAuthorizationSession,
    handle: &SecretStoreHandle,
    secret: SecretBytes,
) -> Result<()> {
    fail_closed::set_secret_with_session(store, session, handle, secret)
}

pub(super) fn get_secret_with_session(
    store: &PlatformSecretStore,
    session: &SecretStoreAuthorizationSession,
    handle: &SecretStoreHandle,
) -> Result<Option<SecretBytes>> {
    fail_closed::get_secret_with_session(store, session, handle)
}

pub(super) fn delete_secret_with_session(
    store: &PlatformSecretStore,
    session: &SecretStoreAuthorizationSession,
    handle: &SecretStoreHandle,
) -> Result<()> {
    fail_closed::delete_secret_with_session(store, session, handle)
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
