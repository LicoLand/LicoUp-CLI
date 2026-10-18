pub(super) mod fail_closed;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
mod unsupported;
#[cfg(target_os = "windows")]
mod windows;

use anyhow::Result;

use super::platform_store::PlatformSecretStore;
use crate::core::secure_mesh_capability::CapabilityFact;
use crate::core::secure_mesh_secret_store::{
    SecretBytes, SecretStoreAuthorizationRequest, SecretStoreAuthorizationSession,
    SecretStoreHandle, SecureMeshSecretStore,
};

#[cfg(target_os = "linux")]
use linux as selected;
#[cfg(target_os = "macos")]
use macos as selected;
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
use unsupported as selected;
#[cfg(target_os = "windows")]
use windows as selected;

pub const NATIVE_SECRET_STORE_BACKEND_UNSUPPORTED: &str =
    "native_platform_secret_store_unsupported";

pub(super) fn backend() -> &'static str {
    selected::BACKEND
}

impl SecureMeshSecretStore for PlatformSecretStore {
    fn backend(&self) -> &'static str {
        backend()
    }

    fn supported(&self) -> bool {
        selected::supported()
    }

    fn capability_facts(&self) -> Result<Vec<CapabilityFact>> {
        selected::capability_facts()
    }

    fn begin_authorized_session(
        &self,
        request: &SecretStoreAuthorizationRequest,
    ) -> Result<SecretStoreAuthorizationSession> {
        selected::begin_authorized_session(self, request)
    }

    fn set_secret_with_session(
        &self,
        session: &SecretStoreAuthorizationSession,
        handle: &SecretStoreHandle,
        secret: SecretBytes,
    ) -> Result<()> {
        selected::set_secret_with_session(self, session, handle, secret)
    }

    fn get_secret_with_session(
        &self,
        session: &SecretStoreAuthorizationSession,
        handle: &SecretStoreHandle,
    ) -> Result<Option<SecretBytes>> {
        selected::get_secret_with_session(self, session, handle)
    }

    fn delete_secret_with_session(
        &self,
        session: &SecretStoreAuthorizationSession,
        handle: &SecretStoreHandle,
    ) -> Result<()> {
        selected::delete_secret_with_session(self, session, handle)
    }

    fn set_secret(&self, handle: &SecretStoreHandle, secret: SecretBytes) -> Result<()> {
        selected::set_secret(self, handle, secret)
    }

    fn get_secret(&self, handle: &SecretStoreHandle) -> Result<Option<SecretBytes>> {
        selected::get_secret(self, handle)
    }

    fn delete_secret(&self, handle: &SecretStoreHandle) -> Result<()> {
        selected::delete_secret(self, handle)
    }
}
