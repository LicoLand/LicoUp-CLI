use std::sync::Arc;

use anyhow::Result;

use crate::core::secure_mesh_capability::{
    CapabilityEvidenceKind, CapabilityFact, CustodyRestartSemantics, SecretCustodyStrategy,
    SecurityCapability,
};

use super::super::capability::linux_secret_service_capability_facts_from_snapshot;
use super::super::ephemeral::EphemeralSecretStore;
use super::super::selection::SecureMeshSecretStoreSelection;
use super::support::unlocked_linux_probe_fixture;
use crate::core::secure_mesh_secret_store::{
    SecretBytes, SecretStoreHandle, SecureMeshSecretStore,
};

#[test]
fn selector_accepts_safe_software_os_storage_without_hardware_claims() {
    struct SoftwareOsStore(EphemeralSecretStore);

    impl SecureMeshSecretStore for SoftwareOsStore {
        fn backend(&self) -> &'static str {
            "software-os-store-test"
        }

        fn supported(&self) -> bool {
            true
        }

        fn capability_facts(&self) -> Result<Vec<CapabilityFact>> {
            Ok(vec![
                CapabilityFact::supported(
                    SecurityCapability::OsSecureStore,
                    CapabilityEvidenceKind::RuntimeOperation,
                ),
                CapabilityFact::supported(
                    SecurityCapability::SoftwareBacked,
                    CapabilityEvidenceKind::TestFixture,
                ),
            ])
        }

        fn set_secret(&self, handle: &SecretStoreHandle, secret: SecretBytes) -> Result<()> {
            self.0.set_secret(handle, secret)
        }

        fn get_secret(&self, handle: &SecretStoreHandle) -> Result<Option<SecretBytes>> {
            self.0.get_secret(handle)
        }

        fn delete_secret(&self, handle: &SecretStoreHandle) -> Result<()> {
            self.0.delete_secret(handle)
        }
    }

    let selection = SecureMeshSecretStoreSelection::select(Some(Arc::new(SoftwareOsStore(
        EphemeralSecretStore::new(),
    ))))
    .unwrap();
    assert_eq!(selection.strategy(), SecretCustodyStrategy::OsSecureStore);
    assert_eq!(
        selection.restart_semantics(),
        CustodyRestartSemantics::PersistentStateAvailable
    );
    assert!(
        selection
            .capability_evaluation()
            .enabled()
            .contains(&SecurityCapability::SoftwareBacked)
    );
    assert!(
        !selection
            .capability_evaluation()
            .enabled()
            .contains(&SecurityCapability::HardwareBacked)
    );
}

#[test]
fn selector_rejects_declaration_only_os_store_support() {
    struct DeclarationOnlyStore(EphemeralSecretStore);

    impl SecureMeshSecretStore for DeclarationOnlyStore {
        fn backend(&self) -> &'static str {
            "declaration-only-store"
        }

        fn supported(&self) -> bool {
            true
        }

        fn capability_facts(&self) -> Result<Vec<CapabilityFact>> {
            Ok(vec![CapabilityFact::supported(
                SecurityCapability::OsSecureStore,
                CapabilityEvidenceKind::SourceContract,
            )])
        }

        fn set_secret(&self, handle: &SecretStoreHandle, secret: SecretBytes) -> Result<()> {
            self.0.set_secret(handle, secret)
        }

        fn get_secret(&self, handle: &SecretStoreHandle) -> Result<Option<SecretBytes>> {
            self.0.get_secret(handle)
        }

        fn delete_secret(&self, handle: &SecretStoreHandle) -> Result<()> {
            self.0.delete_secret(handle)
        }
    }

    let selection = SecureMeshSecretStoreSelection::select(Some(Arc::new(DeclarationOnlyStore(
        EphemeralSecretStore::new(),
    ))))
    .unwrap();
    assert_eq!(
        selection.strategy(),
        SecretCustodyStrategy::MemoryOnlyEphemeral
    );
    assert!(
        selection
            .capability_evaluation()
            .unverified()
            .contains(&SecurityCapability::OsSecureStore)
    );
}

#[test]
fn selector_falls_back_only_to_memory_and_defines_no_unsafe_strategy() {
    struct UnsupportedStore;

    impl SecureMeshSecretStore for UnsupportedStore {
        fn backend(&self) -> &'static str {
            "unsupported-test-store"
        }

        fn supported(&self) -> bool {
            false
        }

        fn set_secret(&self, _handle: &SecretStoreHandle, _secret: SecretBytes) -> Result<()> {
            unreachable!()
        }

        fn get_secret(&self, _handle: &SecretStoreHandle) -> Result<Option<SecretBytes>> {
            unreachable!()
        }

        fn delete_secret(&self, _handle: &SecretStoreHandle) -> Result<()> {
            unreachable!()
        }
    }

    let selection =
        SecureMeshSecretStoreSelection::select(Some(Arc::new(UnsupportedStore))).unwrap();
    assert_eq!(
        selection.strategy(),
        SecretCustodyStrategy::MemoryOnlyEphemeral
    );
    assert_eq!(
        selection.restart_semantics(),
        CustodyRestartSemantics::RePairRekeyAfterRestart
    );
    let strategies = serde_json::to_string(&[
        SecretCustodyStrategy::MemoryOnlyEphemeral,
        SecretCustodyStrategy::OsSecureStore,
    ])
    .unwrap();
    assert!(!strategies.contains("plaintext"));
    assert!(!strategies.contains("portable"));
    assert!(!strategies.contains("ordinary_file"));
}

#[test]
fn locked_or_unavailable_os_store_facts_select_memory_without_losing_reasons() {
    struct LockedLinuxStore;

    impl SecureMeshSecretStore for LockedLinuxStore {
        fn backend(&self) -> &'static str {
            "linux-secret-service-keyring"
        }

        fn supported(&self) -> bool {
            false
        }

        fn capability_facts(&self) -> Result<Vec<CapabilityFact>> {
            let mut snapshot = unlocked_linux_probe_fixture();
            snapshot.collection = "locked";
            snapshot.prompt = "not_attempted";
            linux_secret_service_capability_facts_from_snapshot(&snapshot)
        }

        fn set_secret(&self, _handle: &SecretStoreHandle, _secret: SecretBytes) -> Result<()> {
            unreachable!()
        }

        fn get_secret(&self, _handle: &SecretStoreHandle) -> Result<Option<SecretBytes>> {
            unreachable!()
        }

        fn delete_secret(&self, _handle: &SecretStoreHandle) -> Result<()> {
            unreachable!()
        }
    }

    let selection =
        SecureMeshSecretStoreSelection::select(Some(Arc::new(LockedLinuxStore))).unwrap();
    assert_eq!(
        selection.strategy(),
        SecretCustodyStrategy::MemoryOnlyEphemeral
    );
    assert_eq!(
        selection
            .capability_evaluation()
            .reasons()
            .get(&SecurityCapability::LinuxSecretService)
            .map(String::as_str),
        Some("linux_secret_service_collection_locked")
    );
    assert!(
        selection
            .capability_evaluation()
            .unavailable()
            .contains(&SecurityCapability::OsSecureStore)
    );
    assert!(
        selection
            .capability_evaluation()
            .unavailable()
            .contains(&SecurityCapability::LinuxSecretService)
    );
}
