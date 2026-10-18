use anyhow::Result;

use super::super::platform_store::PlatformSecretStore;
use crate::core::secure_mesh_secret_store::{
    SecretBytes, SecretStoreAuthorizationRequest, SecretStoreAuthorizationSession,
    SecretStoreHandle, SecretStorePresenceError,
};

fn unavailable() -> anyhow::Error {
    SecretStorePresenceError::authorization_required().into()
}

pub(in crate::platform::secure_mesh_secret_store) fn begin_authorized_session(
    _store: &PlatformSecretStore,
    _request: &SecretStoreAuthorizationRequest,
) -> Result<SecretStoreAuthorizationSession> {
    Err(unavailable())
}

#[cfg(any(test, not(target_os = "macos")))]
pub(in crate::platform::secure_mesh_secret_store) fn set_secret_with_session(
    _store: &PlatformSecretStore,
    _session: &SecretStoreAuthorizationSession,
    _handle: &SecretStoreHandle,
    _secret: SecretBytes,
) -> Result<()> {
    Err(unavailable())
}

#[cfg(any(test, not(target_os = "macos")))]
pub(in crate::platform::secure_mesh_secret_store) fn get_secret_with_session(
    _store: &PlatformSecretStore,
    _session: &SecretStoreAuthorizationSession,
    _handle: &SecretStoreHandle,
) -> Result<Option<SecretBytes>> {
    Err(unavailable())
}

#[cfg(any(test, not(target_os = "macos")))]
pub(in crate::platform::secure_mesh_secret_store) fn delete_secret_with_session(
    _store: &PlatformSecretStore,
    _session: &SecretStoreAuthorizationSession,
    _handle: &SecretStoreHandle,
) -> Result<()> {
    Err(unavailable())
}

pub(in crate::platform::secure_mesh_secret_store) fn set_secret(
    _store: &PlatformSecretStore,
    _handle: &SecretStoreHandle,
    _secret: SecretBytes,
) -> Result<()> {
    Err(unavailable())
}

pub(in crate::platform::secure_mesh_secret_store) fn get_secret(
    _store: &PlatformSecretStore,
    _handle: &SecretStoreHandle,
) -> Result<Option<SecretBytes>> {
    Err(unavailable())
}

pub(in crate::platform::secure_mesh_secret_store) fn delete_secret(
    _store: &PlatformSecretStore,
    _handle: &SecretStoreHandle,
) -> Result<()> {
    Err(unavailable())
}
