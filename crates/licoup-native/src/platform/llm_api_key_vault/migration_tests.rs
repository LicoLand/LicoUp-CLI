use super::*;
use crate::core::secure_mesh_secret_store::{
    PresenceDecision, SecretStoreHandle, SecretStorePresenceBatchRequest, SecretStorePresenceNonce,
    SecretStorePresenceProvider,
};
use crate::platform::secure_mesh_secret_store::macos_user_presence::{
    MacosAuthorizationContext, MacosPresencePromptPort, MacosSecItemPort, MacosSecretStoreAccess,
    SecurityFrameworkKeychain,
};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Failure {
    LegacyRead,
    Write,
    Readback,
    LegacyDelete,
}

#[derive(Default)]
struct KeychainState {
    destination: BTreeMap<String, SecretBytes>,
    legacy: BTreeMap<String, SecretBytes>,
    events: Vec<(&'static str, String)>,
    failure: Option<Failure>,
    contexts: Vec<MacosAuthorizationContext>,
}

#[derive(Default)]
struct Keychain(Mutex<KeychainState>);

impl KeychainState {
    fn fail(&mut self, point: Failure) -> bool {
        if self.failure == Some(point) {
            self.failure = None;
            true
        } else {
            false
        }
    }
}

impl MacosSecItemPort for Keychain {
    fn set_secret(
        &self,
        _: &MacosAuthorizationContext,
        _: &str,
        handle: &SecretStoreHandle,
        secret: SecretBytes,
    ) -> Result<()> {
        let mut state = self.0.lock().unwrap();
        state.events.push(("write", handle.key().to_owned()));
        ensure!(!state.fail(Failure::Write), "synthetic_write_failure");
        state.destination.insert(handle.key().to_owned(), secret);
        Ok(())
    }

    fn get_secret(
        &self,
        context: &MacosAuthorizationContext,
        _: &str,
        handle: &SecretStoreHandle,
    ) -> Result<Option<SecretBytes>> {
        let mut state = self.0.lock().unwrap();
        state.contexts.push(context.clone());
        state.events.push(("read", handle.key().to_owned()));
        if state.destination.contains_key(handle.key()) && state.fail(Failure::Readback) {
            return Ok(None);
        }
        Ok(state
            .destination
            .get(handle.key())
            .map(SecretBytes::copy_for_persistent_read))
    }

    fn delete_secret(
        &self,
        _: &MacosAuthorizationContext,
        _: &str,
        handle: &SecretStoreHandle,
    ) -> Result<()> {
        self.0.lock().unwrap().destination.remove(handle.key());
        Ok(())
    }

    fn get_legacy_classic_secret(
        &self,
        _: &MacosAuthorizationContext,
        _: &str,
        handle: &SecretStoreHandle,
    ) -> Result<Option<SecretBytes>> {
        let mut state = self.0.lock().unwrap();
        state.events.push(("legacy-read", handle.key().to_owned()));
        ensure!(
            !state.fail(Failure::LegacyRead),
            "synthetic_legacy_read_failure"
        );
        Ok(state
            .legacy
            .get(handle.key())
            .map(SecretBytes::copy_for_persistent_read))
    }

    fn delete_legacy_classic_secret(
        &self,
        _: &MacosAuthorizationContext,
        _: &str,
        handle: &SecretStoreHandle,
    ) -> Result<()> {
        let mut state = self.0.lock().unwrap();
        state
            .events
            .push(("legacy-delete", handle.key().to_owned()));
        ensure!(
            !state.fail(Failure::LegacyDelete),
            "synthetic_legacy_delete_failure"
        );
        state.legacy.remove(handle.key());
        Ok(())
    }
}

struct Prompt {
    count: Arc<AtomicUsize>,
    decision: PresenceDecision,
}

impl MacosPresencePromptPort for Prompt {
    fn prompt(&mut self, _: &SecretStorePresenceBatchRequest) -> Result<PresenceDecision> {
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(self.decision)
    }
}

struct Fixture {
    root: PathBuf,
    vault: PlatformLlmApiKeyVault,
    keychain: Arc<Keychain>,
    prompts: Arc<AtomicUsize>,
    inventory: LlmApiKeyInventory,
}

impl Fixture {
    fn new(count: usize, protected_inventory: bool, decision: PresenceDecision) -> Self {
        Self::for_request(
            count,
            protected_inventory,
            decision,
            gateway_migration_request(),
            Duration::from_secs(60),
        )
    }

    fn for_request(
        count: usize,
        protected_inventory: bool,
        decision: PresenceDecision,
        request: SecretStoreAuthorizationRequest,
        operation_offset: Duration,
    ) -> Self {
        let root = std::env::temp_dir().join(format!(
            "lico-credential-migration-{}",
            uuid::Uuid::new_v4()
        ));
        let mut vault = PlatformLlmApiKeyVault::at_state_root(&root).unwrap();
        let keychain = Arc::new(Keychain::default());
        let prompts = Arc::new(AtomicUsize::new(0));
        let batch = SecretStorePresenceBatchRequest::new(
            SecretStorePresenceProvider::MacosKeychain,
            request.key_class(),
            request.operation_count(),
            request.reason(),
            SecretStorePresenceNonce::new("synthetic-credential-migration").unwrap(),
            request.caller_channel(),
            true,
        )
        .unwrap();
        let now = Instant::now();
        vault.store = vault
            .store
            .with_macos_secret_store_access(MacosSecretStoreAccess::new(
                batch,
                now,
                now + operation_offset,
                Box::new(Prompt {
                    count: Arc::clone(&prompts),
                    decision,
                }),
                Arc::new(SecurityFrameworkKeychain::with_sec_item_port(
                    keychain.clone(),
                )),
            ));
        let inventory = LlmApiKeyInventory::new(
            GatewayCredentialLeaseDays::Seven,
            (0..count)
                .map(|index| LlmApiKeyMetadata {
                    credential_id: uuid::Uuid::from_u128((index + 1) as u128).to_string(),
                    provider: LlmApiKeyProvider::Kimi,
                    label: "Synthetic credential".to_owned(),
                    created_at_epoch_seconds: 1,
                    expires_at_epoch_seconds: (index % 2 == 0).then_some(2),
                })
                .collect(),
        )
        .unwrap();
        {
            let mut state = keychain.0.lock().unwrap();
            for entry in &inventory.entries {
                state.legacy.insert(
                    credential_key(&entry.credential_id),
                    SecretBytes::try_from_bytes(b"synthetic-api-key".to_vec()).unwrap(),
                );
            }
            if protected_inventory {
                state.legacy.insert(
                    LEGACY_PROTECTED_INVENTORY_KEY.to_owned(),
                    SecretBytes::try_from_bytes(serde_json::to_vec(&inventory).unwrap()).unwrap(),
                );
            } else {
                vault.write_inventory_metadata(&inventory).unwrap();
            }
        }
        Self {
            root,
            vault,
            keychain,
            prompts,
            inventory,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn explicit_migration_copies_full_inventory_and_reads_back_before_deleting() {
    let fixture = Fixture::new(MAX_LLM_API_KEYS, true, PresenceDecision::Approved);
    assert_eq!(
        fixture.vault.migrate_legacy_credentials().unwrap(),
        fixture.inventory
    );
    assert_eq!(fixture.vault.list().unwrap(), fixture.inventory);
    assert_eq!(fixture.prompts.load(Ordering::SeqCst), 1);
    let state = fixture.keychain.0.lock().unwrap();
    assert!(state.legacy.is_empty());
    assert_eq!(state.destination.len(), MAX_LLM_API_KEYS);
    for entry in &fixture.inventory.entries {
        let key = credential_key(&entry.credential_id);
        let events: Vec<_> = state
            .events
            .iter()
            .filter(|(_, target)| target == &key)
            .map(|(event, _)| *event)
            .collect();
        assert_eq!(
            events,
            ["read", "legacy-read", "write", "read", "legacy-delete"]
        );
    }
    assert!(
        state
            .contexts
            .iter()
            .all(|context| !context.migration_session_active_for_test())
    );
}

#[test]
fn explicit_migration_preserves_source_on_failure_and_retries_without_rewriting() {
    for failure in [
        Failure::LegacyRead,
        Failure::Write,
        Failure::Readback,
        Failure::LegacyDelete,
    ] {
        let fixture = Fixture::new(1, false, PresenceDecision::Approved);
        let key = credential_key(&fixture.inventory.entries[0].credential_id);
        fixture.keychain.0.lock().unwrap().failure = Some(failure);
        assert!(fixture.vault.migrate_legacy_credentials().is_err());
        {
            let state = fixture.keychain.0.lock().unwrap();
            assert!(state.legacy.contains_key(&key));
            assert!(
                state
                    .contexts
                    .iter()
                    .all(|context| !context.migration_session_active_for_test())
            );
        }
        fixture.vault.migrate_legacy_credentials().unwrap();
        let state = fixture.keychain.0.lock().unwrap();
        assert!(!state.legacy.contains_key(&key));
        assert!(state.destination.contains_key(&key));
        if matches!(failure, Failure::Readback | Failure::LegacyDelete) {
            assert_eq!(
                state
                    .events
                    .iter()
                    .filter(|(event, _)| *event == "write")
                    .count(),
                1
            );
        }
        assert_eq!(fixture.prompts.load(Ordering::SeqCst), 2);
    }
}

#[test]
fn explicit_migration_cancel_and_conflict_preserve_all_credentials() {
    let cancelled = Fixture::new(1, false, PresenceDecision::Cancelled);
    assert_eq!(
        cancelled
            .vault
            .migrate_legacy_credentials()
            .unwrap_err()
            .to_string(),
        "secure_mesh_presence_cancelled"
    );
    let state = cancelled.keychain.0.lock().unwrap();
    assert_eq!(state.legacy.len(), 1);
    assert!(state.events.is_empty());
    drop(state);

    let conflict = Fixture::new(1, false, PresenceDecision::Approved);
    let key = credential_key(&conflict.inventory.entries[0].credential_id);
    conflict.keychain.0.lock().unwrap().destination.insert(
        key.clone(),
        SecretBytes::try_from_bytes(b"different-synthetic-value".to_vec()).unwrap(),
    );
    assert_eq!(
        conflict
            .vault
            .migrate_legacy_credentials()
            .unwrap_err()
            .to_string(),
        "llm_api_key_migration_verification_failed"
    );
    let state = conflict.keychain.0.lock().unwrap();
    assert!(state.legacy.contains_key(&key));
    assert_eq!(
        state.destination.get(&key).unwrap().expose_bytes(),
        b"different-synthetic-value"
    );
    assert!(
        !state
            .events
            .iter()
            .any(|(event, _)| matches!(*event, "write" | "legacy-delete"))
    );
}

#[test]
fn delete_drops_an_inventory_entry_whose_secret_is_already_gone() {
    let fixture = Fixture::for_request(
        2,
        false,
        PresenceDecision::Approved,
        gateway_request(
            "Authorize LicoUp to delete a model API key",
            5 + MAX_LLM_API_KEYS,
        ),
        Duration::from_secs(15),
    );
    // Neither the protected store nor the legacy store holds this secret:
    // the entry is orphaned and must still be deletable.
    fixture.keychain.0.lock().unwrap().legacy.clear();
    let doomed = fixture.inventory.entries[0].credential_id.clone();
    let survivor = fixture.inventory.entries[1].credential_id.clone();
    let updated = fixture.vault.delete(&doomed).unwrap();
    assert!(
        updated
            .entries
            .iter()
            .all(|entry| entry.credential_id != doomed)
    );
    assert!(
        updated
            .entries
            .iter()
            .any(|entry| entry.credential_id == survivor)
    );
    assert_eq!(fixture.vault.list().unwrap(), updated);
}
