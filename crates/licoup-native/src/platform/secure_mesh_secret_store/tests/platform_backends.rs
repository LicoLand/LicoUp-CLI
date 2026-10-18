#[cfg(target_os = "macos")]
use super::super::macos_user_presence;
#[cfg(target_os = "macos")]
use super::super::macos_user_presence::{
    MacosAuthorizedPresence, MacosKeychainEffectPort, MacosPresencePromptPort,
    MacosSecretStoreAccess,
};
use super::super::platform_backends::fail_closed;
use super::super::platform_store::{PlatformSecretStore, SecretClassPersistenceProof};
#[cfg(any(target_os = "macos", target_os = "windows"))]
use super::super::{PlatformSecretStoreRuntimeState, platform_native_secret_store_runtime_state};
use crate::core::secure_mesh_secret_store::SecretBytes;
#[cfg(target_os = "macos")]
use crate::core::secure_mesh_secret_store::{
    PresenceDecision, SecretStoreCallerChannel, SecretStoreKeyClass,
    SecretStorePresenceBatchRequest, SecretStorePresenceNonce, SecretStorePresenceProvider,
};
use crate::core::secure_mesh_secret_store::{
    SecretStoreAuthorizationRequest, SecretStoreHandle, SecureMeshSecretStore,
};

fn secret(value: &str) -> SecretBytes {
    SecretBytes::try_from_string(value.to_owned()).unwrap()
}

#[test]
fn secret_store_handle_rejects_empty_or_key_separator_values() {
    assert!(SecretStoreHandle::new("", "privateKeyBase64url").is_err());
    assert!(SecretStoreHandle::new("mobileRelayE2ee", "").is_err());
    assert!(SecretStoreHandle::new("mobileRelayE2ee", "private:key").is_err());
}

#[test]
fn platform_store_builds_opaque_account_handle() {
    let store = PlatformSecretStore::new("app.licomesh.test", "mobileRelayE2ee");
    assert_eq!(store.service, "app.licomesh.test");
    let handle = store
        .handle_for_namespace("namespace", "privateKeyBase64url")
        .unwrap();
    assert_eq!(
        handle.account(),
        "mobileRelayE2ee:namespace:privateKeyBase64url"
    );
}

#[test]
fn platform_store_unit_test_io_is_noninteractive_and_fail_closed() {
    let store = PlatformSecretStore::new("app.licomesh.test", "unitTestSecret");
    let handle = store
        .handle_for_namespace("noninteractive", "proof")
        .unwrap();
    let error = store.get_secret(&handle).unwrap_err();
    assert_eq!(
        error
            .downcast_ref::<crate::core::secure_mesh_secret_store::SecretStorePresenceError>()
            .unwrap()
            .code(),
        "secure_mesh_authorization_required"
    );
}

#[test]
fn unmeasured_platform_backend_rejects_authorization_and_every_io_shape() {
    let store = PlatformSecretStore::new("app.licomesh.test", "unmeasured");
    let handle = store.handle_for_namespace("namespace", "proof").unwrap();
    let request = SecretStoreAuthorizationRequest::new("unmeasured backend", 3);
    assert!(fail_closed::begin_authorized_session(&store, &request).is_err());

    let session = crate::core::secure_mesh_secret_store::SecretStoreAuthorizationSession::new(
        "unmeasured-test-store",
        &request,
        false,
        false,
    );
    assert!(
        fail_closed::set_secret_with_session(&store, &session, &handle, secret("secret")).is_err()
    );
    assert!(fail_closed::get_secret_with_session(&store, &session, &handle).is_err());
    assert!(fail_closed::delete_secret_with_session(&store, &session, &handle).is_err());
    assert!(fail_closed::set_secret(&store, &handle, secret("secret")).is_err());
    assert!(fail_closed::get_secret(&store, &handle).is_err());
    assert!(fail_closed::delete_secret(&store, &handle).is_err());
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[test]
fn desktop_store_without_runtime_round_trip_stays_unverified() {
    assert_eq!(
        platform_native_secret_store_runtime_state(),
        PlatformSecretStoreRuntimeState::Unverified
    );
    let store = PlatformSecretStore::new("app.licomesh.test", "unverifiedDesktop");
    assert!(!store.supported());
    let facts = store.capability_facts().unwrap();
    assert!(facts.iter().all(|fact| {
        fact.state == crate::core::secure_mesh_capability::CapabilityFactState::Unverified
            && fact.evidence_kind
                == crate::core::secure_mesh_capability::CapabilityEvidenceKind::NotMeasured
    }));
}

#[cfg(target_os = "macos")]
#[test]
fn macos_local_authentication_is_never_enabled_inside_unit_tests() {
    assert!(!macos_user_presence::available());
    let store = PlatformSecretStore::new("app.licomesh.test", "macosFailClosed");
    let error = store
        .begin_authorized_session(&SecretStoreAuthorizationRequest::new(
            "macOS user-presence fail-closed test",
            1,
        ))
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("user-presence authorization is unavailable")
    );
}

#[cfg(target_os = "macos")]
#[test]
fn platform_store_trait_session_dispatches_one_exact_presence_batch_to_macos_consumers() {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use std::time::Instant;

    struct ApprovedPrompt {
        count: Arc<AtomicUsize>,
    }

    impl MacosPresencePromptPort for ApprovedPrompt {
        fn prompt(
            &mut self,
            _request: &SecretStorePresenceBatchRequest,
        ) -> anyhow::Result<PresenceDecision> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(PresenceDecision::Approved)
        }
    }

    #[derive(Default)]
    struct InMemoryKeychain {
        value: Mutex<Option<SecretBytes>>,
        set_count: AtomicUsize,
        get_count: AtomicUsize,
        delete_count: AtomicUsize,
    }

    impl MacosKeychainEffectPort for InMemoryKeychain {
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
            Ok(self
                .value
                .lock()
                .unwrap()
                .as_ref()
                .map(SecretBytes::copy_for_persistent_read))
        }

        fn delete_secret(
            &self,
            _authorized_presence: MacosAuthorizedPresence,
            _service: &str,
            _handle: &SecretStoreHandle,
        ) -> anyhow::Result<()> {
            self.delete_count.fetch_add(1, Ordering::SeqCst);
            *self.value.lock().unwrap() = None;
            Ok(())
        }
    }

    let prompt_count = Arc::new(AtomicUsize::new(0));
    let keychain = Arc::new(InMemoryKeychain::default());
    let presence_request = SecretStorePresenceBatchRequest::new(
        SecretStorePresenceProvider::MacosKeychain,
        SecretStoreKeyClass::DeviceIdentity,
        3,
        "platform-store exact batch",
        SecretStorePresenceNonce::new("platform-store-batch-nonce").unwrap(),
        SecretStoreCallerChannel::DesktopGui,
        true,
    )
    .unwrap();
    let now = Instant::now();
    let keychain_port: Arc<dyn MacosKeychainEffectPort + Send + Sync> = keychain.clone();
    let access = MacosSecretStoreAccess::new(
        presence_request,
        now,
        now,
        Box::new(ApprovedPrompt {
            count: Arc::clone(&prompt_count),
        }),
        keychain_port,
    );
    let store = PlatformSecretStore::new("app.licomesh.test", "macosDispatch")
        .with_macos_secret_store_access(access);
    let session = store
        .begin_authorized_session(&SecretStoreAuthorizationRequest::new(
            "platform-store exact batch",
            3,
        ))
        .unwrap();
    let handle = store
        .handle_for_namespace("dispatch-namespace", "dispatch-key")
        .unwrap();

    store
        .set_secret_with_session(&session, &handle, secret("dispatch-secret"))
        .unwrap();
    assert_eq!(
        store
            .get_secret_with_session(&session, &handle)
            .unwrap()
            .as_ref()
            .map(SecretBytes::expose_bytes),
        Some(b"dispatch-secret".as_slice())
    );
    store.delete_secret_with_session(&session, &handle).unwrap();

    assert_eq!(prompt_count.load(Ordering::SeqCst), 1);
    assert_eq!(keychain.set_count.load(Ordering::SeqCst), 1);
    assert_eq!(keychain.get_count.load(Ordering::SeqCst), 1);
    assert_eq!(keychain.delete_count.load(Ordering::SeqCst), 1);
    assert!(keychain.value.lock().unwrap().is_none());

    let forged_backend_session =
        crate::core::secure_mesh_secret_store::SecretStoreAuthorizationSession::new(
            "forged-secret-store-backend",
            &SecretStoreAuthorizationRequest::new("platform-store exact batch", 1),
            true,
            true,
        );
    let error = store
        .set_secret_with_session(
            &forged_backend_session,
            &handle,
            secret("must-not-reach-keychain"),
        )
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "secure_mesh_presence_session_batch_mismatch"
    );
    assert_eq!(keychain.set_count.load(Ordering::SeqCst), 1);
    assert_eq!(keychain.get_count.load(Ordering::SeqCst), 1);
    assert_eq!(keychain.delete_count.load(Ordering::SeqCst), 1);

    let replacement_request =
        SecretStoreAuthorizationRequest::new("replacement request must not inherit batch", 1);
    let replaced_request_session =
        crate::core::secure_mesh_secret_store::SecretStoreAuthorizationSession::new(
            store.backend(),
            &replacement_request,
            true,
            true,
        );
    let error = store
        .get_secret_with_session(&replaced_request_session, &handle)
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "secure_mesh_presence_session_batch_mismatch"
    );
    assert_eq!(keychain.set_count.load(Ordering::SeqCst), 1);
    assert_eq!(keychain.get_count.load(Ordering::SeqCst), 1);
    assert_eq!(keychain.delete_count.load(Ordering::SeqCst), 1);

    let mismatch_prompt_count = Arc::new(AtomicUsize::new(0));
    let mismatch_keychain = Arc::new(InMemoryKeychain::default());
    let mismatch_keychain_port: Arc<dyn MacosKeychainEffectPort + Send + Sync> =
        mismatch_keychain.clone();
    let mismatch_access = MacosSecretStoreAccess::new(
        SecretStorePresenceBatchRequest::new(
            SecretStorePresenceProvider::MacosKeychain,
            SecretStoreKeyClass::DeviceIdentity,
            2,
            "injected presence reason",
            SecretStorePresenceNonce::new("mismatch-batch-nonce").unwrap(),
            SecretStoreCallerChannel::DesktopGui,
            true,
        )
        .unwrap(),
        now,
        now,
        Box::new(ApprovedPrompt {
            count: Arc::clone(&mismatch_prompt_count),
        }),
        mismatch_keychain_port,
    );
    let mismatch_store = PlatformSecretStore::new("app.licomesh.test", "macosMismatch")
        .with_macos_secret_store_access(mismatch_access);
    let error = mismatch_store
        .begin_authorized_session(&SecretStoreAuthorizationRequest::new(
            "different begin-session reason",
            2,
        ))
        .unwrap_err();
    assert_eq!(error.to_string(), "secure_mesh_presence_batch_mismatch");
    assert_eq!(mismatch_prompt_count.load(Ordering::SeqCst), 0);
    assert_eq!(mismatch_keychain.set_count.load(Ordering::SeqCst), 0);
    assert_eq!(mismatch_keychain.get_count.load(Ordering::SeqCst), 0);
    assert_eq!(mismatch_keychain.delete_count.load(Ordering::SeqCst), 0);

    let count_mismatch_prompt_count = Arc::new(AtomicUsize::new(0));
    let count_mismatch_keychain = Arc::new(InMemoryKeychain::default());
    let count_mismatch_keychain_port: Arc<dyn MacosKeychainEffectPort + Send + Sync> =
        count_mismatch_keychain.clone();
    let count_mismatch_access = MacosSecretStoreAccess::new(
        SecretStorePresenceBatchRequest::new(
            SecretStorePresenceProvider::MacosKeychain,
            SecretStoreKeyClass::DeviceIdentity,
            2,
            "count-only exact reason",
            SecretStorePresenceNonce::new("count-mismatch-batch-nonce").unwrap(),
            SecretStoreCallerChannel::DesktopGui,
            true,
        )
        .unwrap(),
        now,
        now,
        Box::new(ApprovedPrompt {
            count: Arc::clone(&count_mismatch_prompt_count),
        }),
        count_mismatch_keychain_port,
    );
    let count_mismatch_store = PlatformSecretStore::new("app.licomesh.test", "macosCountMismatch")
        .with_macos_secret_store_access(count_mismatch_access);
    let error = count_mismatch_store
        .begin_authorized_session(&SecretStoreAuthorizationRequest::new(
            "count-only exact reason",
            3,
        ))
        .unwrap_err();
    assert_eq!(error.to_string(), "secure_mesh_presence_batch_mismatch");
    assert_eq!(count_mismatch_prompt_count.load(Ordering::SeqCst), 0);
    assert_eq!(count_mismatch_keychain.set_count.load(Ordering::SeqCst), 0);
    assert_eq!(count_mismatch_keychain.get_count.load(Ordering::SeqCst), 0);
    assert_eq!(
        count_mismatch_keychain.delete_count.load(Ordering::SeqCst),
        0
    );

    let forged_same_fields_prompt_count = Arc::new(AtomicUsize::new(0));
    let forged_same_fields_keychain = Arc::new(InMemoryKeychain::default());
    let forged_same_fields_keychain_port: Arc<dyn MacosKeychainEffectPort + Send + Sync> =
        forged_same_fields_keychain.clone();
    let identical_reason = "surface-identical session request";
    let identical_count = 3;
    let forged_same_fields_access = MacosSecretStoreAccess::new(
        SecretStorePresenceBatchRequest::new(
            SecretStorePresenceProvider::MacosKeychain,
            SecretStoreKeyClass::DeviceIdentity,
            identical_count,
            identical_reason,
            SecretStorePresenceNonce::new("surface-identical-session-nonce").unwrap(),
            SecretStoreCallerChannel::DesktopGui,
            true,
        )
        .unwrap(),
        now,
        now,
        Box::new(ApprovedPrompt {
            count: Arc::clone(&forged_same_fields_prompt_count),
        }),
        forged_same_fields_keychain_port,
    );
    let forged_same_fields_store =
        PlatformSecretStore::new("app.licomesh.test", "macosForgedSameFields")
            .with_macos_secret_store_access(forged_same_fields_access);
    let identical_request = SecretStoreAuthorizationRequest::new(identical_reason, identical_count);
    let forged_same_fields_session =
        crate::core::secure_mesh_secret_store::SecretStoreAuthorizationSession::new(
            forged_same_fields_store.backend(),
            &identical_request,
            true,
            true,
        );
    let forged_same_fields_handle = forged_same_fields_store
        .handle_for_namespace("surface-identical", "forged-session")
        .unwrap();
    assert_eq!(
        forged_same_fields_session.consumed_operation_count(),
        0,
        "the independent forged session must begin with an untouched operation budget"
    );

    for error in [
        forged_same_fields_store
            .set_secret_with_session(
                &forged_same_fields_session,
                &forged_same_fields_handle,
                secret("must-not-reach-keychain"),
            )
            .unwrap_err(),
        forged_same_fields_store
            .get_secret_with_session(&forged_same_fields_session, &forged_same_fields_handle)
            .unwrap_err(),
        forged_same_fields_store
            .delete_secret_with_session(&forged_same_fields_session, &forged_same_fields_handle)
            .unwrap_err(),
    ] {
        assert_eq!(
            error.to_string(),
            "secure_mesh_presence_session_batch_mismatch"
        );
    }
    assert_eq!(forged_same_fields_session.consumed_operation_count(), 0);
    assert_eq!(forged_same_fields_prompt_count.load(Ordering::SeqCst), 0);
    assert_eq!(
        forged_same_fields_keychain.set_count.load(Ordering::SeqCst),
        0
    );
    assert_eq!(
        forged_same_fields_keychain.get_count.load(Ordering::SeqCst),
        0
    );
    assert_eq!(
        forged_same_fields_keychain
            .delete_count
            .load(Ordering::SeqCst),
        0
    );
}

#[test]
fn class_persistence_report_shape_is_redacted() {
    let report = SecretClassPersistenceProof {
        backend: "test-backend",
        secret_classes: vec!["pairwiseSessionSnapshot".to_string()],
        requested_class_count: 1,
        persisted_class_count: 1,
        deleted_class_count: 1,
        all_classes_persisted: true,
        all_classes_deleted: true,
        raw_secret_material_included: false,
    };
    assert!(report.all_classes_persisted);
    assert!(report.all_classes_deleted);
    assert!(!report.raw_secret_material_included);
}
