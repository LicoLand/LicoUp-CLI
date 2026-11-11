//! The device's mailbox key, held by the OS secret store.
//!
//! The 32-byte Ed25519 seed the Station binds to this device's mailbox is
//! minted once at enrollment and kept only in the platform secret store (macOS
//! Keychain, Linux Secret Service, Windows DPAPI). It is never written under the
//! data home. When the client sets [`DEVICE_KEY_USER_PRESENCE_ENV`] to
//! `required`, every read and write of the key asks for user presence.

use anyhow::{Result, anyhow, ensure};
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;
use zeroize::Zeroizing;

use crate::core::secret_store::{
    SecretBytes, SecretStoreAuthorizationRequest, SecretStoreCallerChannel, SecretStoreHandle,
    SecretStoreKeyClass,
};
use crate::platform::secret_store::{PlatformSecretStore, SecretStore};

/// Set to `required` by the production desktop client.
pub const DEVICE_KEY_USER_PRESENCE_ENV: &str = "LICOUP_DEVICE_KEY_USER_PRESENCE";
const SERVICE: &str = "land.lico.licoup.devices";
const ACCOUNT_PREFIX: &str = "devices";
const DEVICE_KEY: &str = "mailbox-device-key";

#[cfg(test)]
thread_local! {
    static STORE_OVERRIDE: std::cell::RefCell<Option<Arc<dyn SecretStore>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn with_secret_store_override<T>(
    store: Arc<dyn SecretStore>,
    operation: impl FnOnce() -> T,
) -> T {
    struct Reset(Option<Arc<dyn SecretStore>>);
    impl Drop for Reset {
        fn drop(&mut self) {
            STORE_OVERRIDE.with(|slot| {
                slot.replace(self.0.take());
            });
        }
    }
    let previous = STORE_OVERRIDE.with(|slot| slot.replace(Some(store)));
    let _reset = Reset(previous);
    operation()
}

fn platform_store() -> Arc<dyn SecretStore> {
    #[cfg(test)]
    if let Some(store) = STORE_OVERRIDE.with(|slot| slot.borrow().clone()) {
        return store;
    }
    Arc::new(PlatformSecretStore::new(SERVICE, ACCOUNT_PREFIX))
}

fn user_presence_required() -> bool {
    std::env::var(DEVICE_KEY_USER_PRESENCE_ENV).is_ok_and(|value| value.trim() == "required")
}

/// The secret store the session key custody ([`super::session_custody`])
/// rides on: the same platform backend and the same test override as the
/// mailbox device key.
pub(crate) fn session_secret_store() -> Arc<dyn SecretStore> {
    platform_store()
}

/// Whether session custody reads and writes ask for user presence — the same
/// `LICOUP_DEVICE_KEY_USER_PRESENCE=required` policy as the device key.
pub(crate) fn session_user_presence_required() -> bool {
    user_presence_required()
}

pub(crate) struct DeviceKeyCustody {
    store: Arc<dyn SecretStore>,
    handle: SecretStoreHandle,
}

impl DeviceKeyCustody {
    /// One custody slot per data home: the namespace is derived from the
    /// devices directory, so two data homes never share a device key.
    pub(crate) fn for_devices_dir(dir: &Path) -> Result<Self> {
        let digest = Sha256::digest(dir.to_string_lossy().as_bytes());
        let namespace: String = digest[..12]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let handle = SecretStoreHandle::new(format!("{ACCOUNT_PREFIX}:{namespace}"), DEVICE_KEY)
            .map_err(|_| anyhow!("devices_device_key_unavailable"))?;
        let store = platform_store();
        ensure!(store.supported(), "devices_device_key_store_unsupported");
        Ok(Self { store, handle })
    }

    fn request(reason: &str) -> SecretStoreAuthorizationRequest {
        SecretStoreAuthorizationRequest::for_scope(
            reason,
            4,
            user_presence_required(),
            SecretStoreKeyClass::DeviceIdentity,
            SecretStoreCallerChannel::NativeCli,
        )
    }

    /// Mints and stores a fresh device seed.
    pub(crate) fn mint(&self) -> Result<Zeroizing<[u8; 32]>> {
        let mut seed = Zeroizing::new([0_u8; 32]);
        OsRng.fill_bytes(&mut seed[..]);
        let secret = SecretBytes::try_from_bytes(seed.to_vec())
            .map_err(|_| anyhow!("devices_device_key_unavailable"))?;
        let session = self
            .store
            .begin_authorized_session(&Self::request("Create this device's LicoUp pairing key"))?;
        self.store
            .set_secret_with_session(&session, &self.handle, secret)
            .map_err(|_| anyhow!("devices_device_key_unavailable"))?;
        Ok(seed)
    }

    /// Reads the stored device seed.
    pub(crate) fn load(&self) -> Result<Zeroizing<[u8; 32]>> {
        let session = self
            .store
            .begin_authorized_session(&Self::request("Use this device's LicoUp pairing key"))?;
        let secret = self
            .store
            .get_secret_with_session(&session, &self.handle)
            .map_err(|_| anyhow!("devices_device_key_unavailable"))?
            .ok_or_else(|| anyhow!("devices_device_key_missing"))?;
        let bytes = secret.expose_bytes();
        ensure!(bytes.len() == 32, "devices_device_key_invalid");
        let mut seed = Zeroizing::new([0_u8; 32]);
        seed.copy_from_slice(bytes);
        Ok(seed)
    }

    /// Removes a key whose enrollment did not complete.
    pub(crate) fn remove(&self) -> Result<()> {
        let session = self
            .store
            .begin_authorized_session(&Self::request("Remove an unused LicoUp pairing key"))?;
        self.store
            .delete_secret_with_session(&session, &self.handle)
            .map_err(|_| anyhow!("devices_device_key_unavailable"))
    }
}
