use super::super::platform_backends::fail_closed;
use super::super::{EphemeralSecretStore, PlatformSecretStore};
use crate::core::secure_mesh_secret_store::{
    SecretBytes, SecretStoreAuthorizationRequest, SecretStoreAuthorizationSession,
    SecretStoreHandle, SecretZeroizeProbe, SecureMeshSecretStore,
};

const STORED_CANARY: &[u8] = b"synthetic-platform-secret-alpha";
const REPLACEMENT_CANARY: &[u8] = b"synthetic-platform-secret-beta";

#[test]
fn ephemeral_store_consumes_owned_secrets_and_wipes_replaced_and_deleted_backing() {
    let store = EphemeralSecretStore::new();
    let handle = SecretStoreHandle::new("zeroizing-ephemeral", "owned-value").unwrap();
    let replaced_probe = SecretZeroizeProbe::new();
    let deleted_probe = SecretZeroizeProbe::new();

    store
        .set_secret(
            &handle,
            SecretBytes::try_from_bytes_with_test_zeroize_probe(
                STORED_CANARY.to_vec(),
                replaced_probe.clone(),
            )
            .unwrap(),
        )
        .unwrap();
    assert!(replaced_probe.observations().is_empty());

    store
        .set_secret(
            &handle,
            SecretBytes::try_from_bytes_with_test_zeroize_probe(
                REPLACEMENT_CANARY.to_vec(),
                deleted_probe.clone(),
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(
        replaced_probe.observations(),
        vec![vec![0; STORED_CANARY.len()]]
    );

    let first_read = store.get_secret(&handle).unwrap().unwrap();
    assert_eq!(first_read.expose_bytes(), REPLACEMENT_CANARY);
    assert!(
        format!("{first_read:?}")
            .to_ascii_lowercase()
            .contains("redacted")
    );
    assert!(!format!("{first_read:?}").contains(std::str::from_utf8(REPLACEMENT_CANARY).unwrap()));
    drop(first_read);
    assert!(
        store.get_secret(&handle).unwrap().is_some(),
        "a read returns a newly owned value without changing persistent-store semantics"
    );

    store.delete_secret(&handle).unwrap();
    assert_eq!(
        deleted_probe.observations(),
        vec![vec![0; REPLACEMENT_CANARY.len()]]
    );
    assert!(store.get_secret(&handle).unwrap().is_none());
}

#[test]
fn fail_closed_platform_adapter_consumes_and_wipes_rejected_secret_without_echo() {
    let store = PlatformSecretStore::new("app.licomesh.synthetic", "zeroizingReject");
    let handle = store
        .handle_for_namespace("zeroizing-platform", "reject")
        .unwrap();
    let request = SecretStoreAuthorizationRequest::noninteractive("synthetic rejection", 1);
    let session =
        SecretStoreAuthorizationSession::new("synthetic-unmeasured", &request, false, false);
    let probe = SecretZeroizeProbe::new();

    let error = fail_closed::set_secret_with_session(
        &store,
        &session,
        &handle,
        SecretBytes::try_from_bytes_with_test_zeroize_probe(STORED_CANARY.to_vec(), probe.clone())
            .unwrap(),
    )
    .unwrap_err();

    assert_eq!(
        probe.observations(),
        vec![vec![0; STORED_CANARY.len()]],
        "the owned argument must be wiped when the real platform dispatch seam rejects it"
    );
    let error = format!("{error:?}");
    assert!(!error.contains(std::str::from_utf8(STORED_CANARY).unwrap()));
    assert!(!error.contains(std::str::from_utf8(REPLACEMENT_CANARY).unwrap()));
}

#[cfg(target_os = "macos")]
#[test]
fn injected_macos_platform_dispatch_transfers_one_owned_secret_without_credentials() {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Instant;

    use super::super::macos_user_presence::{
        MacosAuthorizedPresence, MacosKeychainEffectPort, MacosPresencePromptPort,
        MacosSecretStoreAccess,
    };
    use crate::core::secure_mesh_secret_store::{
        PresenceDecision, SecretStoreCallerChannel, SecretStoreKeyClass,
        SecretStorePresenceBatchRequest, SecretStorePresenceNonce, SecretStorePresenceProvider,
    };

    struct ApprovedPrompt;
    impl MacosPresencePromptPort for ApprovedPrompt {
        fn prompt(
            &mut self,
            _request: &SecretStorePresenceBatchRequest,
        ) -> anyhow::Result<PresenceDecision> {
            Ok(PresenceDecision::Approved)
        }
    }

    #[derive(Default)]
    struct OwnedKeychainEffect {
        value: Mutex<Option<SecretBytes>>,
        set_count: AtomicUsize,
        get_count: AtomicUsize,
    }
    impl MacosKeychainEffectPort for OwnedKeychainEffect {
        fn set_secret(
            &self,
            _authorized_presence: MacosAuthorizedPresence,
            _service: &str,
            _handle: &SecretStoreHandle,
            secret: SecretBytes,
        ) -> anyhow::Result<()> {
            self.set_count.fetch_add(1, Ordering::SeqCst);
            *self.value.lock().unwrap() = Some(secret);
            Ok(())
        }

        fn get_secret(
            &self,
            _authorized_presence: MacosAuthorizedPresence,
            _service: &str,
            _handle: &SecretStoreHandle,
        ) -> anyhow::Result<Option<SecretBytes>> {
            self.get_count.fetch_add(1, Ordering::SeqCst);
            Ok(self.value.lock().unwrap().take())
        }

        fn delete_secret(
            &self,
            _authorized_presence: MacosAuthorizedPresence,
            _service: &str,
            _handle: &SecretStoreHandle,
        ) -> anyhow::Result<()> {
            self.value.lock().unwrap().take();
            Ok(())
        }
    }

    let effect = Arc::new(OwnedKeychainEffect::default());
    let effect_port: Arc<dyn MacosKeychainEffectPort + Send + Sync> = effect.clone();
    let now = Instant::now();
    let access = MacosSecretStoreAccess::new(
        SecretStorePresenceBatchRequest::new(
            SecretStorePresenceProvider::MacosKeychain,
            SecretStoreKeyClass::DeviceIdentity,
            2,
            "zeroizing platform dispatch",
            SecretStorePresenceNonce::new("zeroizing-platform-dispatch-nonce").unwrap(),
            SecretStoreCallerChannel::DesktopGui,
            true,
        )
        .unwrap(),
        now,
        now,
        Box::new(ApprovedPrompt),
        effect_port,
    );
    let store = PlatformSecretStore::new("app.licomesh.synthetic", "zeroizingDispatch")
        .with_macos_secret_store_access(access);
    let session = store
        .begin_authorized_session(&SecretStoreAuthorizationRequest::new(
            "zeroizing platform dispatch",
            2,
        ))
        .unwrap();
    let handle = store
        .handle_for_namespace("zeroizing-platform", "round-trip")
        .unwrap();
    let drop_probe = SecretZeroizeProbe::new();

    store
        .set_secret_with_session(
            &session,
            &handle,
            SecretBytes::try_from_bytes_with_test_zeroize_probe(
                STORED_CANARY.to_vec(),
                drop_probe.clone(),
            )
            .unwrap(),
        )
        .unwrap();
    let returned = store
        .get_secret_with_session(&session, &handle)
        .unwrap()
        .unwrap();
    assert_eq!(returned.expose_bytes(), STORED_CANARY);
    assert_eq!(effect.set_count.load(Ordering::SeqCst), 1);
    assert_eq!(effect.get_count.load(Ordering::SeqCst), 1);
    assert!(drop_probe.observations().is_empty());
    drop(returned);
    assert_eq!(
        drop_probe.observations(),
        vec![vec![0; STORED_CANARY.len()]]
    );
}
