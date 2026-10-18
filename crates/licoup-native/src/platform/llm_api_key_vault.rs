//! macOS-backed custody for LLM provider API keys.
//!
//! Every provider key is an independent Keychain item so deletion is a real
//! `SecItemDelete`. Only non-secret inventory metadata is returned to callers.
//! Storage goes through the shared secure-mesh secret store backend so the
//! vault inherits its Data Protection Keychain user-presence authorization.

use anyhow::{Result, anyhow, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    core::secure_mesh_secret_store::{
        SecretBytes, SecretStoreAuthorizationRequest, SecretStoreAuthorizationSession,
        SecretStoreCallerChannel, SecretStoreKeyClass, SecureMeshSecretStore,
    },
    platform::secure_mesh_secret_store::PlatformSecretStore,
};
use licoup_foundation::platform::{file_security, paths};
use licoup_vault::api_key::{
    GatewayCredential, GatewayCredentialChange, GatewayCredentialEpochSource,
    GatewayCredentialHandoff, GatewayCredentialLease, GatewayCredentialLeaseDays,
    LlmApiKeyCredentialUpdate, LlmApiKeyInventory, LlmApiKeyMetadata, LlmApiKeyProvider,
    MAX_LLM_API_KEYS, NewLlmApiKey,
};

const SERVICE: &str = "dev.licoland.licoup.llm-gateway";
const PREFIX: &str = "llm-api-key";
const NAMESPACE: &str = "gateway-credentials-v1";
const LEGACY_PROTECTED_INVENTORY_KEY: &str = "inventory";
const INVENTORY_FILE: &str = "llm-api-key-inventory.json";
const MAX_INVENTORY_BYTES: usize = 64 * 1024;
const EPOCH_FILE: &str = "llm-gateway-credential-epoch";

/// Metadata-only applicability of the published legacy custody transition.
/// It never measures current key availability or requests user presence.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LegacyCredentialMigrationDisposition {
    RequiresAuthorization,
    NotApplicable,
}

pub struct PlatformLlmApiKeyVault {
    store: PlatformSecretStore,
    inventory_path: PathBuf,
    epoch_path: PathBuf,
}

impl PlatformLlmApiKeyVault {
    pub fn legacy_credential_migration_disposition() -> Result<LegacyCredentialMigrationDisposition>
    {
        Self::legacy_credential_migration_disposition_for(std::env::consts::OS)
    }

    /// The same platform-owned declaration is read by the repository diagnostic.
    pub fn legacy_credential_migration_disposition_for(
        platform: &str,
    ) -> Result<LegacyCredentialMigrationDisposition> {
        let policy: BTreeMap<String, LegacyCredentialMigrationDisposition> = serde_json::from_str(
            include_str!("../../resources/gateway-credential-migration.json"),
        )?;
        policy
            .get(platform)
            .copied()
            .ok_or_else(|| anyhow!("gateway_credential_migration_platform_unknown"))
    }

    pub fn production() -> Result<Self> {
        Self::at_state_root(paths::portable_data_dir()?)
    }

    pub fn at_state_root(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        file_security::ensure_private_dir(root)?;
        Ok(Self {
            store: PlatformSecretStore::new(SERVICE, PREFIX),
            inventory_path: root.join(INVENTORY_FILE),
            epoch_path: root.join(EPOCH_FILE),
        })
    }

    pub fn supported(&self) -> bool {
        self.store.supported()
    }

    pub fn platform_supported() -> bool {
        PlatformSecretStore::new(SERVICE, PREFIX).supported()
    }

    pub fn list(&self) -> Result<LlmApiKeyInventory> {
        self.read_inventory_metadata()?.map_or_else(
            || LlmApiKeyInventory::new(GatewayCredentialLeaseDays::default(), Vec::new()),
            Ok,
        )
    }

    /// Move every inventoried credential, including expired entries, from the
    /// legacy macOS Keychain. Only this explicitly selected migration may ask
    /// for native legacy ACL consent. Failed or cancelled work is retryable.
    pub fn migrate_legacy_credentials(&self) -> Result<LlmApiKeyInventory> {
        #[cfg(target_os = "macos")]
        {
            let request = gateway_migration_request();
            self.store
                .with_legacy_credential_migration_session(&request, |session| {
                    let inventory = self.inventory_for_authorized_operation(session)?;
                    for entry in &inventory.entries {
                        self.migrate_secret(session, &credential_key(&entry.credential_id), true)?
                            .ok_or_else(|| anyhow!("llm_api_key_inventory_inconsistent"))?;
                    }
                    // Metadata may already have been persisted before an earlier
                    // attempt failed. Complete cleanup even on that retry path.
                    let handle = self
                        .store
                        .handle_for_namespace(NAMESPACE, LEGACY_PROTECTED_INVENTORY_KEY)?;
                    self.store
                        .delete_legacy_classic_secret_with_session(session, &handle)?;
                    self.delete_secret(session, LEGACY_PROTECTED_INVENTORY_KEY)?;
                    Ok(inventory)
                })
        }
        #[cfg(not(target_os = "macos"))]
        Err(anyhow!("llm_api_key_legacy_migration_unsupported"))
    }

    pub fn create(&self, new_key: NewLlmApiKey) -> Result<LlmApiKeyInventory> {
        ensure!(self.supported(), "llm_api_key_system_keyring_unavailable");
        let session = self.store.begin_authorized_session(&gateway_request(
            "Authorize LicoUp to save a model API key",
            4 + MAX_LLM_API_KEYS,
        ))?;
        let mut inventory = self.inventory_for_authorized_operation(&session)?;
        ensure!(
            inventory.entries.len() < MAX_LLM_API_KEYS,
            "llm_api_key_inventory_capacity_exceeded"
        );
        let credential_id = uuid::Uuid::new_v4().to_string();
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| anyhow!("llm_api_key_clock_invalid"))?
            .as_secs()
            .max(1);
        let metadata = LlmApiKeyMetadata {
            credential_id: credential_id.clone(),
            provider: new_key.provider(),
            label: new_key.label().to_owned(),
            created_at_epoch_seconds: created_at,
            expires_at_epoch_seconds: Some(
                created_at
                    .checked_add(new_key.validity().duration().as_secs())
                    .ok_or_else(|| anyhow!("llm_api_key_expires_at_invalid"))?,
            ),
        };
        self.write_secret(
            &session,
            &credential_key(&credential_id),
            new_key.into_secret(),
        )?;
        inventory.entries.push(metadata);
        let updated = LlmApiKeyInventory::new(
            GatewayCredentialLeaseDays::try_from(inventory.lease_days)
                .map_err(|code| anyhow!(code))?,
            inventory.entries,
        )?;
        if let Err(error) = self.write_inventory_metadata(&updated) {
            let _ = self.delete_secret(&session, &credential_key(&credential_id));
            return Err(error);
        }
        self.apply_lease_revocation(GatewayCredentialChange::CredentialCreated)?;
        #[cfg(unix)]
        crate::platform::llm_gateway_service::reconcile_gateway_session_credentials(
            self, &session, &updated,
        )?;
        Ok(updated)
    }

    pub fn delete(&self, credential_id: &str) -> Result<LlmApiKeyInventory> {
        ensure!(
            uuid::Uuid::parse_str(credential_id).is_ok_and(|id| id.to_string() == credential_id),
            "llm_api_key_credential_id_invalid"
        );
        let session = self.store.begin_authorized_session(&gateway_request(
            "Authorize LicoUp to delete a model API key",
            5 + MAX_LLM_API_KEYS,
        ))?;
        let mut inventory = self.inventory_for_authorized_operation(&session)?;
        ensure!(
            inventory
                .entries
                .iter()
                .any(|entry| entry.credential_id == credential_id),
            "llm_api_key_credential_not_found"
        );
        let key = credential_key(credential_id);
        // A secret proven absent offers nothing to roll back. Requiring its
        // presence here would make an orphaned inventory entry undeletable.
        let rollback_secret = self.read_secret(&session, &key)?;
        self.delete_secret(&session, &key)?;
        inventory
            .entries
            .retain(|entry| entry.credential_id != credential_id);
        let updated = LlmApiKeyInventory::new(
            GatewayCredentialLeaseDays::try_from(inventory.lease_days)
                .map_err(|code| anyhow!(code))?,
            inventory.entries,
        )?;
        if let Err(error) = self.write_inventory_metadata(&updated) {
            if let Some(rollback_secret) = rollback_secret {
                let _ = self.write_secret(&session, &key, rollback_secret);
            }
            return Err(error);
        }
        self.apply_lease_revocation(GatewayCredentialChange::CredentialDeleted)?;
        #[cfg(unix)]
        crate::platform::llm_gateway_service::reconcile_gateway_session_credentials(
            self, &session, &updated,
        )?;
        Ok(updated)
    }

    /// Renames a credential and/or extends its validity period. Metadata-only
    /// change: secret bytes are never touched and no lease revocation applies.
    pub fn update(
        &self,
        credential_id: &str,
        update: LlmApiKeyCredentialUpdate,
    ) -> Result<LlmApiKeyInventory> {
        ensure!(
            uuid::Uuid::parse_str(credential_id).is_ok_and(|id| id.to_string() == credential_id),
            "llm_api_key_credential_id_invalid"
        );
        let mut inventory = self.list()?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| anyhow!("llm_api_key_clock_invalid"))?
            .as_secs();
        let entry = inventory
            .entries
            .iter_mut()
            .find(|entry| entry.credential_id == credential_id)
            .ok_or_else(|| anyhow!("llm_api_key_credential_not_found"))?;
        entry.apply_update(&update, now)?;
        let updated = LlmApiKeyInventory::new(
            GatewayCredentialLeaseDays::try_from(inventory.lease_days)
                .map_err(|code| anyhow!(code))?,
            inventory.entries,
        )?;
        self.write_inventory_metadata(&updated)?;
        Ok(updated)
    }

    pub fn set_lease_days(&self, days: GatewayCredentialLeaseDays) -> Result<LlmApiKeyInventory> {
        let inventory = self.list()?;
        let updated = LlmApiKeyInventory::new(days, inventory.entries)?;
        self.write_inventory_metadata(&updated)?;
        self.apply_lease_revocation(GatewayCredentialChange::LeaseDaysChanged)?;
        Ok(updated)
    }

    /// Authorize the vault once, complete any metadata migration, and project
    /// available credentials into a portable handoff. An empty inventory is a
    /// normal typed result so the UI can respond instead of treating it as an
    /// opaque authorization failure.
    pub fn authorize_gateway_handoff(&self) -> Result<Option<GatewayCredentialHandoff>> {
        self.authorize_gateway_handoff_filtered(None)
    }

    /// Like [`authorize_gateway_handoff`], but only includes the selected
    /// credential IDs in the handoff. `None` means every non-expired entry.
    pub fn authorize_gateway_handoff_filtered(
        &self,
        credential_ids: Option<&[String]>,
    ) -> Result<Option<GatewayCredentialHandoff>> {
        let session =
            self.store
                .begin_authorized_session(&SecretStoreAuthorizationRequest::for_scope(
                    "Authorize LicoUp Gateway to use model API keys",
                    5 * (1 + MAX_LLM_API_KEYS) + 1,
                    true,
                    SecretStoreKeyClass::GatewayCredential,
                    SecretStoreCallerChannel::GatewaySidecar,
                ))?;
        let inventory = self.inventory_for_authorized_operation(&session)?;
        self.gateway_handoff_from_authorized_inventory(&session, &inventory, credential_ids)
    }

    pub fn unlock_gateway_handoff(&self) -> Result<GatewayCredentialHandoff> {
        self.authorize_gateway_handoff()?
            .ok_or_else(|| anyhow!("llm_api_key_credential_unavailable"))
    }

    pub(crate) fn gateway_handoff_from_authorized_inventory(
        &self,
        session: &SecretStoreAuthorizationSession,
        inventory: &LlmApiKeyInventory,
        credential_ids: Option<&[String]>,
    ) -> Result<Option<GatewayCredentialHandoff>> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| anyhow!("llm_api_key_clock_invalid"))?
            .as_secs();
        let selected = credential_ids.map(|ids| ids.iter().cloned().collect::<BTreeSet<_>>());
        if let Some(selected) = selected.as_ref() {
            for id in selected {
                ensure!(
                    inventory
                        .entries
                        .iter()
                        .any(|entry| entry.credential_id == *id),
                    "llm_api_key_credential_unavailable"
                );
            }
        }
        let mut credentials = BTreeMap::<LlmApiKeyProvider, Vec<GatewayCredential>>::new();
        for entry in &inventory.entries {
            if entry.is_expired(now)
                || selected
                    .as_ref()
                    .is_some_and(|ids| !ids.contains(&entry.credential_id))
            {
                continue;
            }
            let key = credential_key(&entry.credential_id);
            let secret = self
                .read_or_migrate_secret(session, &key)?
                .ok_or_else(|| anyhow!("llm_api_key_inventory_inconsistent"))?;
            // Authorization reads the selected items. Existing protection is
            // not rewritten: SecAccessControl cannot convert a classic macOS
            // application ACL, and another write may demand per-item consent.
            credentials
                .entry(entry.provider)
                .or_default()
                .push(GatewayCredential::new(
                    entry.credential_id.clone(),
                    secret,
                    entry.expires_at_epoch_seconds,
                )?);
        }
        if credentials.is_empty() {
            return Ok(None);
        }
        let epoch = self.ensure_epoch()?;
        Ok(Some(GatewayCredentialHandoff::new(
            credentials,
            GatewayCredentialLeaseDays::try_from(inventory.lease_days)
                .map_err(|code| anyhow!(code))?,
            epoch,
        )?))
    }

    /// Rebuild a gateway lease from a handoff received from an unlocking
    /// process, bound to this vault's epoch file for revocation.
    pub fn gateway_lease_from_handoff(
        &self,
        handoff: GatewayCredentialHandoff,
    ) -> Result<GatewayCredentialLease> {
        GatewayCredentialLease::from_handoff(
            handoff,
            Arc::new(FileEpochSource {
                path: self.epoch_path.clone(),
            }),
        )
    }

    pub fn unlock_gateway(&self) -> Result<GatewayCredentialLease> {
        self.gateway_lease_from_handoff(self.unlock_gateway_handoff()?)
    }

    fn read_secret(
        &self,
        session: &SecretStoreAuthorizationSession,
        key: &str,
    ) -> Result<Option<SecretBytes>> {
        let handle = self.store.handle_for_namespace(NAMESPACE, key)?;
        self.store.get_secret_with_session(session, &handle)
    }

    fn read_or_migrate_secret(
        &self,
        session: &SecretStoreAuthorizationSession,
        key: &str,
    ) -> Result<Option<SecretBytes>> {
        #[cfg(target_os = "macos")]
        {
            self.migrate_secret(session, key, false)
        }
        #[cfg(not(target_os = "macos"))]
        self.read_secret(session, key)
    }

    #[cfg(target_os = "macos")]
    fn migrate_secret(
        &self,
        session: &SecretStoreAuthorizationSession,
        key: &str,
        cleanup_existing: bool,
    ) -> Result<Option<SecretBytes>> {
        let handle = self.store.handle_for_namespace(NAMESPACE, key)?;
        if let Some(secret) = self.read_secret(session, key)? {
            if cleanup_existing {
                // A readable destination must never be overwritten. Compare
                // the remaining source before cleanup so a failed readback or
                // conflicting value cannot silently discard the legacy key.
                if let Some(legacy) = self
                    .store
                    .get_legacy_classic_secret_with_session(session, &handle)?
                {
                    ensure_migration_value_matches(&secret, &legacy)?;
                    self.store
                        .delete_legacy_classic_secret_with_session(session, &handle)?;
                }
            }
            return Ok(Some(secret));
        }
        let Some(secret) = self
            .store
            .get_legacy_classic_secret_with_session(session, &handle)?
        else {
            return Ok(None);
        };
        self.write_secret(session, key, secret.copy_for_persistent_read())?;
        let persisted = self.read_secret(session, key)?;
        let persisted =
            persisted.ok_or_else(|| anyhow!("llm_api_key_migration_verification_failed"))?;
        ensure_migration_value_matches(&persisted, &secret)?;
        self.store
            .delete_legacy_classic_secret_with_session(session, &handle)?;
        Ok(Some(secret))
    }

    fn write_secret(
        &self,
        session: &SecretStoreAuthorizationSession,
        key: &str,
        secret: SecretBytes,
    ) -> Result<()> {
        let handle = self.store.handle_for_namespace(NAMESPACE, key)?;
        self.store.set_secret_with_session(session, &handle, secret)
    }

    fn delete_secret(&self, session: &SecretStoreAuthorizationSession, key: &str) -> Result<()> {
        let handle = self.store.handle_for_namespace(NAMESPACE, key)?;
        self.store.delete_secret_with_session(session, &handle)
    }

    fn inventory_for_authorized_operation(
        &self,
        session: &SecretStoreAuthorizationSession,
    ) -> Result<LlmApiKeyInventory> {
        if let Some(inventory) = self.read_inventory_metadata()? {
            return Ok(inventory);
        }
        let Some(inventory) = self.read_legacy_protected_inventory(session)? else {
            return LlmApiKeyInventory::new(GatewayCredentialLeaseDays::default(), Vec::new());
        };
        // One-time complete migration: persist non-secret metadata privately,
        // then remove the obsolete protected metadata item. Secret entries
        // remain independent Keychain items.
        self.write_inventory_metadata(&inventory)?;
        self.delete_secret(session, LEGACY_PROTECTED_INVENTORY_KEY)?;
        Ok(inventory)
    }

    fn read_legacy_protected_inventory(
        &self,
        session: &SecretStoreAuthorizationSession,
    ) -> Result<Option<LlmApiKeyInventory>> {
        let Some(secret) = self.read_or_migrate_secret(session, LEGACY_PROTECTED_INVENTORY_KEY)?
        else {
            return Ok(None);
        };
        decode_inventory(secret.expose_bytes()).map(Some)
    }

    fn read_inventory_metadata(&self) -> Result<Option<LlmApiKeyInventory>> {
        let Some(text) =
            file_security::read_private_text_bounded(&self.inventory_path, MAX_INVENTORY_BYTES)?
        else {
            return Ok(None);
        };
        decode_inventory(text.as_bytes()).map(Some)
    }

    fn write_inventory_metadata(&self, inventory: &LlmApiKeyInventory) -> Result<()> {
        let body = serde_json::to_string_pretty(inventory)?;
        ensure!(
            body.len() <= MAX_INVENTORY_BYTES,
            "llm_api_key_inventory_invalid"
        );
        file_security::atomic_write_private_text(&self.inventory_path, &body)
    }

    fn ensure_epoch(&self) -> Result<String> {
        if let Ok(Some(value)) = file_security::read_private_text_bounded(&self.epoch_path, 128) {
            let value = value.trim().to_owned();
            if uuid::Uuid::parse_str(&value).is_ok_and(|id| id.to_string() == value) {
                return Ok(value);
            }
        }
        self.rotate_epoch()
    }

    fn rotate_epoch(&self) -> Result<String> {
        let epoch = uuid::Uuid::new_v4().to_string();
        file_security::atomic_write_private_text(&self.epoch_path, &epoch)?;
        Ok(epoch)
    }

    fn apply_lease_revocation(&self, change: GatewayCredentialChange) -> Result<()> {
        if change.revokes_active_leases() {
            self.rotate_epoch()?;
        }
        Ok(())
    }
}

fn decode_inventory(bytes: &[u8]) -> Result<LlmApiKeyInventory> {
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    ensure!(
        value
            .get("schemaVersion")
            .and_then(serde_json::Value::as_str)
            == Some(licoup_vault::api_key::LLM_API_KEY_INVENTORY_SCHEMA),
        "llm_api_key_inventory_invalid"
    );
    let lease_days = value
        .get("leaseDays")
        .and_then(serde_json::Value::as_u64)
        .and_then(|v| u16::try_from(v).ok())
        .ok_or_else(|| anyhow!("llm_api_key_inventory_invalid"))?;
    let entries: Vec<LlmApiKeyMetadata> = serde_json::from_value(
        value
            .get("entries")
            .cloned()
            .ok_or_else(|| anyhow!("llm_api_key_inventory_invalid"))?,
    )?;
    LlmApiKeyInventory::new(
        GatewayCredentialLeaseDays::try_from(lease_days).map_err(|code| anyhow!(code))?,
        entries,
    )
}

fn gateway_request(reason: &str, operation_count: usize) -> SecretStoreAuthorizationRequest {
    SecretStoreAuthorizationRequest::for_scope(
        reason,
        operation_count.saturating_add(5 * (1 + MAX_LLM_API_KEYS)),
        true,
        SecretStoreKeyClass::GatewayCredential,
        SecretStoreCallerChannel::DesktopGui,
    )
}

#[cfg(target_os = "macos")]
fn gateway_migration_request() -> SecretStoreAuthorizationRequest {
    SecretStoreAuthorizationRequest::for_scope(
        "Migrate LicoUp model API keys to protected storage",
        5 * (1 + MAX_LLM_API_KEYS) + 3,
        true,
        SecretStoreKeyClass::GatewayCredential,
        SecretStoreCallerChannel::GatewayCredentialMigration,
    )
}

fn credential_key(credential_id: &str) -> String {
    format!("credential-{credential_id}")
}

#[cfg(target_os = "macos")]
fn ensure_migration_value_matches(destination: &SecretBytes, source: &SecretBytes) -> Result<()> {
    use subtle::ConstantTimeEq;
    ensure!(
        bool::from(destination.expose_bytes().ct_eq(source.expose_bytes())),
        "llm_api_key_migration_verification_failed"
    );
    Ok(())
}

struct FileEpochSource {
    path: PathBuf,
}
impl GatewayCredentialEpochSource for FileEpochSource {
    fn active_epoch(&self) -> Result<Option<String>> {
        Ok(file_security::read_private_text_bounded(&self.path, 128)?
            .map(|value| value.trim().to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn authorize_two_keys_uses_one_presence_batch_without_rewriting_items() {
        use crate::core::secure_mesh_secret_store::{
            PresenceDecision, SecretStoreHandle, SecretStorePresenceBatchRequest,
            SecretStorePresenceNonce, SecretStorePresenceProvider,
        };
        use crate::platform::secure_mesh_secret_store::macos_user_presence::{
            MacosAuthorizedPresence, MacosKeychainEffectPort, MacosPresencePromptPort,
            MacosSecretStoreAccess,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::Instant;

        struct Prompt(Arc<AtomicUsize>);
        impl MacosPresencePromptPort for Prompt {
            fn prompt(&mut self, _: &SecretStorePresenceBatchRequest) -> Result<PresenceDecision> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(PresenceDecision::Approved)
            }
        }

        struct Keychain {
            reads: AtomicUsize,
            writes: AtomicUsize,
            legacy_reads: AtomicUsize,
            blocked_legacy: bool,
        }
        impl MacosKeychainEffectPort for Keychain {
            fn set_secret(
                &self,
                _: MacosAuthorizedPresence,
                _: &str,
                _: &SecretStoreHandle,
                _: SecretBytes,
            ) -> Result<()> {
                self.writes.fetch_add(1, Ordering::SeqCst);
                Err(anyhow!("synthetic_unexpected_write"))
            }

            fn get_secret(
                &self,
                _: MacosAuthorizedPresence,
                _: &str,
                _: &SecretStoreHandle,
            ) -> Result<Option<SecretBytes>> {
                self.reads.fetch_add(1, Ordering::SeqCst);
                if self.blocked_legacy {
                    Ok(None)
                } else {
                    Ok(Some(SecretBytes::try_from_bytes(
                        b"synthetic-api-key".to_vec(),
                    )?))
                }
            }

            fn delete_secret(
                &self,
                _: MacosAuthorizedPresence,
                _: &str,
                _: &SecretStoreHandle,
            ) -> Result<()> {
                self.writes.fetch_add(1, Ordering::SeqCst);
                Err(anyhow!("synthetic_unexpected_delete"))
            }

            fn get_legacy_classic_secret(
                &self,
                _: MacosAuthorizedPresence,
                _: &str,
                _: &SecretStoreHandle,
            ) -> Result<Option<SecretBytes>> {
                self.legacy_reads.fetch_add(1, Ordering::SeqCst);
                Err(anyhow!(
                    "secure_mesh_keychain_classic_access_requires_user_action"
                ))
            }
        }

        for blocked_legacy in [false, true] {
            let root =
                std::env::temp_dir().join(format!("lico-vault-auth-{}", uuid::Uuid::new_v4()));
            let mut vault = PlatformLlmApiKeyVault::at_state_root(&root).unwrap();
            let prompts = Arc::new(AtomicUsize::new(0));
            let keychain = Arc::new(Keychain {
                reads: AtomicUsize::new(0),
                writes: AtomicUsize::new(0),
                legacy_reads: AtomicUsize::new(0),
                blocked_legacy,
            });
            let request = SecretStorePresenceBatchRequest::new(
                SecretStorePresenceProvider::MacosKeychain,
                SecretStoreKeyClass::GatewayCredential,
                5 * (1 + MAX_LLM_API_KEYS) + 1,
                "Authorize LicoUp Gateway to use model API keys",
                SecretStorePresenceNonce::new("synthetic-model-api-key-batch").unwrap(),
                SecretStoreCallerChannel::GatewaySidecar,
                true,
            )
            .unwrap();
            let now = Instant::now();
            vault.store = vault
                .store
                .with_macos_secret_store_access(MacosSecretStoreAccess::new(
                    request,
                    now,
                    now,
                    Box::new(Prompt(Arc::clone(&prompts))),
                    keychain.clone(),
                ));
            vault
                .write_inventory_metadata(
                    &LlmApiKeyInventory::new(
                        GatewayCredentialLeaseDays::Seven,
                        [
                            "11111111-1111-4111-8111-111111111111",
                            "22222222-2222-4222-8222-222222222222",
                        ]
                        .into_iter()
                        .map(|credential_id| LlmApiKeyMetadata {
                            credential_id: credential_id.to_owned(),
                            provider: LlmApiKeyProvider::Kimi,
                            label: "Synthetic credential".to_owned(),
                            created_at_epoch_seconds: 1,
                            expires_at_epoch_seconds: None,
                        })
                        .collect(),
                    )
                    .unwrap(),
                )
                .unwrap();

            let result = vault.authorize_gateway_handoff();
            if blocked_legacy {
                assert_eq!(
                    result.unwrap_err().to_string(),
                    "secure_mesh_keychain_classic_access_requires_user_action"
                );
                assert_eq!(keychain.reads.load(Ordering::SeqCst), 1);
                assert_eq!(keychain.legacy_reads.load(Ordering::SeqCst), 1);
            } else {
                assert!(result.unwrap().is_some());
                assert_eq!(keychain.reads.load(Ordering::SeqCst), 2);
                assert_eq!(keychain.legacy_reads.load(Ordering::SeqCst), 0);
            }
            assert_eq!(prompts.load(Ordering::SeqCst), 1);
            assert_eq!(keychain.writes.load(Ordering::SeqCst), 0);
            assert_eq!(vault.list().unwrap().entries.len(), 2);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn credential_account_is_stable_and_separator_free() {
        let store = PlatformSecretStore::new(SERVICE, PREFIX);
        let credential_id = "11111111-1111-4111-8111-111111111111";
        let handle = store
            .handle_for_namespace(NAMESPACE, credential_key(credential_id))
            .expect("credential handle");
        assert_eq!(
            handle.account(),
            "llm-api-key:gateway-credentials-v1:credential-11111111-1111-4111-8111-111111111111"
        );
    }

    #[test]
    fn vault_constructs_at_temporary_state_root() {
        let root =
            std::env::temp_dir().join(format!("lico-llm-api-key-vault-{}", uuid::Uuid::new_v4()));
        PlatformLlmApiKeyVault::at_state_root(root).expect("vault at a temporary state root");
    }

    #[test]
    fn inventory_metadata_lists_without_keychain_authorization() {
        let root =
            std::env::temp_dir().join(format!("lico-llm-api-key-vault-{}", uuid::Uuid::new_v4()));
        let vault = PlatformLlmApiKeyVault::at_state_root(root).unwrap();
        let inventory = LlmApiKeyInventory::new(GatewayCredentialLeaseDays::Seven, vec![]).unwrap();

        vault.write_inventory_metadata(&inventory).unwrap();

        assert_eq!(vault.list().unwrap(), inventory);
    }

    #[test]
    fn metadata_updates_do_not_require_a_keychain_session() {
        let root =
            std::env::temp_dir().join(format!("lico-llm-api-key-vault-{}", uuid::Uuid::new_v4()));
        let vault = PlatformLlmApiKeyVault::at_state_root(root).unwrap();
        let credential_id = "11111111-1111-4111-8111-111111111111";
        let inventory = LlmApiKeyInventory::new(
            GatewayCredentialLeaseDays::Seven,
            vec![LlmApiKeyMetadata {
                credential_id: credential_id.to_owned(),
                provider: LlmApiKeyProvider::Kimi,
                label: "Before".to_owned(),
                created_at_epoch_seconds: 1,
                expires_at_epoch_seconds: Some(2),
            }],
        )
        .unwrap();
        vault.write_inventory_metadata(&inventory).unwrap();

        let updated = vault
            .update(
                credential_id,
                LlmApiKeyCredentialUpdate::new(Some("After".to_owned()), None).unwrap(),
            )
            .unwrap();
        assert_eq!(updated.entries[0].label, "After");

        let updated = vault
            .set_lease_days(GatewayCredentialLeaseDays::Thirty)
            .unwrap();
        assert_eq!(updated.lease_days, 30);
        assert_eq!(vault.list().unwrap(), updated);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod migration_tests;

#[cfg(test)]
mod disposition_tests {
    use super::*;

    #[test]
    fn legacy_custody_disposition_is_owned_by_the_platform_projection() {
        for (platform, expected) in [
            (
                "macos",
                LegacyCredentialMigrationDisposition::RequiresAuthorization,
            ),
            (
                "windows",
                LegacyCredentialMigrationDisposition::NotApplicable,
            ),
            ("linux", LegacyCredentialMigrationDisposition::NotApplicable),
        ] {
            assert_eq!(
                PlatformLlmApiKeyVault::legacy_credential_migration_disposition_for(platform)
                    .unwrap(),
                expected
            );
        }
        assert!(
            PlatformLlmApiKeyVault::legacy_credential_migration_disposition_for("unrecognized")
                .is_err()
        );
    }
}
