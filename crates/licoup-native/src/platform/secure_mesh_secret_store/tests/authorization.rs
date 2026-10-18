use anyhow::Result;

use crate::core::secure_mesh_secret_store::{
    SecretBytes, SecretStoreAuthorizationRequest, SecretStoreAuthorizationSession,
    SecretStoreHandle, SecureMeshSecretStore,
};

fn secret(value: &str) -> SecretBytes {
    SecretBytes::try_from_string(value.to_owned()).unwrap()
}

#[test]
fn single_system_authorization_context_requires_one_completed_system_attempt() {
    let request = SecretStoreAuthorizationRequest::new("test batch", 3);
    let baseline = SecretStoreAuthorizationSession::new("macos-keychain", &request, true, true);
    assert!(!baseline.single_system_authorization_context_verified());

    let verified = baseline
        .clone()
        .with_test_system_authorization_outcome(1, true, false);
    assert!(verified.single_system_authorization_context_verified());

    let repeated_prompt = verified
        .clone()
        .with_test_system_authorization_outcome(2, true, false);
    assert!(!repeated_prompt.single_system_authorization_context_verified());

    let app_password_prompt = verified.with_test_system_authorization_outcome(1, true, true);
    assert!(!app_password_prompt.single_system_authorization_context_verified());
}

#[test]
fn authorization_session_enforces_operation_budget_across_clones() {
    let request = SecretStoreAuthorizationRequest::new("budgeted batch", 2);
    let session = SecretStoreAuthorizationSession::new("macos-keychain", &request, true, true);
    let clone = session.clone();

    session.record_secret_store_operation("read").unwrap();
    clone.record_secret_store_operation("write").unwrap();

    assert_eq!(session.consumed_operation_count(), 2);
    assert_eq!(clone.remaining_operation_count(), 0);
    assert!(session.authorization_batch_within_budget());
    assert!(clone.record_secret_store_operation("delete").is_err());
    assert_eq!(session.consumed_operation_count(), 2);
}

#[test]
fn default_session_methods_reject_required_shared_system_context() {
    struct SessionRequiredStore;

    impl SecureMeshSecretStore for SessionRequiredStore {
        fn backend(&self) -> &'static str {
            "session-required-test-store"
        }

        fn supported(&self) -> bool {
            true
        }

        fn set_secret(&self, _handle: &SecretStoreHandle, _secret: SecretBytes) -> Result<()> {
            Ok(())
        }

        fn get_secret(&self, _handle: &SecretStoreHandle) -> Result<Option<SecretBytes>> {
            Ok(Some(secret("secret")))
        }

        fn delete_secret(&self, _handle: &SecretStoreHandle) -> Result<()> {
            Ok(())
        }
    }

    let request = SecretStoreAuthorizationRequest::new("required auth", 1);
    let session =
        SecretStoreAuthorizationSession::new("session-required-test-store", &request, true, true);
    let handle = SecretStoreHandle::new("namespace", "key").unwrap();
    let store = SessionRequiredStore;

    assert!(
        store
            .set_secret_with_session(&session, &handle, secret("secret"))
            .is_err()
    );
    assert!(store.get_secret_with_session(&session, &handle).is_err());
    assert!(store.delete_secret_with_session(&session, &handle).is_err());
}

#[cfg(target_os = "macos")]
mod macos_presence {
    use std::fmt::{Debug, Display};
    use std::sync::{
        Arc, Barrier, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };
    use std::time::{Duration, Instant};

    use anyhow::{Error, Result};

    use crate::core::secure_mesh_secret_store::{
        MAX_SECRET_STORE_PRESENCE_GRANT_TTL, PresenceDecision, SecretBytes,
        SecretStoreApprovedPresenceBatch, SecretStoreCallerChannel, SecretStoreKeyClass,
        SecretStoreOperation, SecretStorePresenceBatchRequest, SecretStorePresenceGrant,
        SecretStorePresenceNonce, SecretStorePresenceProvider, SecretStorePresencePurpose,
        SecretStorePresenceScope,
    };
    use crate::platform::secure_mesh_secret_store::macos_user_presence::{
        MacosApprovedPresenceBatch, MacosAuthorizationContext, MacosAuthorizedPresence,
        MacosKeychainEffectPort, MacosPresenceBatchCoordinator, MacosPresencePromptPort,
        MacosSecItemPort, SecurityFrameworkKeychain, delete_secret, get_secret, set_secret,
    };

    macro_rules! assert_not_impl {
        ($type:ty: $trait:path) => {
            const _: fn() = || {
                trait AmbiguousIfImplemented<A> {
                    fn marker() {}
                }
                impl<T: ?Sized> AmbiguousIfImplemented<()> for T {}
                struct Invalid;
                impl<T: ?Sized + $trait> AmbiguousIfImplemented<Invalid> for T {}
                let _ = <$type as AmbiguousIfImplemented<_>>::marker;
            };
        };
    }

    assert_not_impl!(MacosAuthorizedPresence: Clone);
    assert_not_impl!(MacosAuthorizedPresence: Copy);
    assert_not_impl!(MacosAuthorizedPresence: Default);

    const CANARIES: [&str; 10] = [
        "reason-canary-alpha-c115ad",
        "nonce-canary-alpha-79f34e",
        "namespace-canary-alpha-4f55d7",
        "key-canary-alpha-51c718",
        "purpose-canary-alpha-76e49d",
        "reason-canary-beta-092acc",
        "nonce-canary-beta-e664a1",
        "namespace-canary-beta-b33c2a",
        "key-canary-beta-2ab194",
        "purpose-canary-beta-d8e3a1",
    ];

    fn request(
        provider: SecretStorePresenceProvider,
        key_class: SecretStoreKeyClass,
        count: usize,
        reason: &str,
        nonce: &str,
        caller_channel: SecretStoreCallerChannel,
        interactive: bool,
    ) -> SecretStorePresenceBatchRequest {
        SecretStorePresenceBatchRequest::new(
            provider,
            key_class,
            count,
            reason,
            SecretStorePresenceNonce::new(nonce).unwrap(),
            caller_channel,
            interactive,
        )
        .unwrap()
    }

    fn alpha_request(count: usize, interactive: bool) -> SecretStorePresenceBatchRequest {
        request(
            SecretStorePresenceProvider::MacosKeychain,
            SecretStoreKeyClass::DeviceIdentity,
            count,
            CANARIES[0],
            CANARIES[1],
            SecretStoreCallerChannel::DesktopGui,
            interactive,
        )
    }

    fn beta_request(count: usize, interactive: bool) -> SecretStorePresenceBatchRequest {
        request(
            SecretStorePresenceProvider::MacosKeychain,
            SecretStoreKeyClass::PairwiseSession,
            count,
            CANARIES[5],
            CANARIES[6],
            SecretStoreCallerChannel::Mobile,
            interactive,
        )
    }

    fn purpose(value: &str) -> SecretStorePresencePurpose {
        SecretStorePresencePurpose::new(value).unwrap()
    }

    fn operation_scope(
        operation: SecretStoreOperation,
        namespace: &str,
        purpose: &str,
    ) -> SecretStorePresenceScope {
        operation_scope_with_key(operation, namespace, "fixed-test-key", purpose)
    }

    fn operation_scope_with_key(
        operation: SecretStoreOperation,
        namespace: &str,
        key: &str,
        purpose: &str,
    ) -> SecretStorePresenceScope {
        SecretStorePresenceScope::new(operation, namespace, key, self::purpose(purpose)).unwrap()
    }

    struct CountingPrompt {
        decision: PresenceDecision,
        count: Arc<AtomicUsize>,
    }

    impl CountingPrompt {
        fn new(decision: PresenceDecision, count: Arc<AtomicUsize>) -> Self {
            Self { decision, count }
        }
    }

    impl MacosPresencePromptPort for CountingPrompt {
        fn prompt(
            &mut self,
            _request: &SecretStorePresenceBatchRequest,
        ) -> Result<PresenceDecision> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(self.decision)
        }
    }

    struct EventPrompt {
        count: Arc<AtomicUsize>,
        events: Arc<Mutex<Vec<&'static str>>>,
    }

    impl MacosPresencePromptPort for EventPrompt {
        fn prompt(
            &mut self,
            _request: &SecretStorePresenceBatchRequest,
        ) -> Result<PresenceDecision> {
            self.count.fetch_add(1, Ordering::SeqCst);
            self.events.lock().unwrap().push("prompt");
            Ok(PresenceDecision::Approved)
        }
    }

    #[derive(Default)]
    struct SyntheticKeychain {
        set_count: Arc<AtomicUsize>,
        get_count: Arc<AtomicUsize>,
        delete_count: Arc<AtomicUsize>,
        events: Arc<Mutex<Vec<&'static str>>>,
    }

    impl MacosKeychainEffectPort for SyntheticKeychain {
        fn set_secret(
            &self,
            _authorized_presence: MacosAuthorizedPresence,
            _service: &str,
            _handle: &crate::core::secure_mesh_secret_store::SecretStoreHandle,
            _secret: SecretBytes,
        ) -> Result<()> {
            self.set_count.fetch_add(1, Ordering::SeqCst);
            self.events.lock().unwrap().push("set");
            Ok(())
        }

        fn get_secret(
            &self,
            _authorized_presence: MacosAuthorizedPresence,
            _service: &str,
            _handle: &crate::core::secure_mesh_secret_store::SecretStoreHandle,
        ) -> Result<Option<SecretBytes>> {
            self.get_count.fetch_add(1, Ordering::SeqCst);
            self.events.lock().unwrap().push("get");
            Ok(Some(SecretBytes::try_from_bytes(
                b"synthetic-secret".to_vec(),
            )?))
        }

        fn delete_secret(
            &self,
            _authorized_presence: MacosAuthorizedPresence,
            _service: &str,
            _handle: &crate::core::secure_mesh_secret_store::SecretStoreHandle,
        ) -> Result<()> {
            self.delete_count.fetch_add(1, Ordering::SeqCst);
            self.events.lock().unwrap().push("delete");
            Ok(())
        }
    }

    #[derive(Default)]
    struct SecItemSpy {
        set_count: AtomicUsize,
        get_count: AtomicUsize,
        delete_count: AtomicUsize,
        targets: Mutex<Vec<ObservedSecItemTarget>>,
    }

    #[derive(Debug, Eq, PartialEq)]
    struct ObservedSecItemTarget {
        operation: &'static str,
        namespace: String,
        key: String,
    }

    impl SecItemSpy {
        fn record_target(
            &self,
            operation: &'static str,
            handle: &crate::core::secure_mesh_secret_store::SecretStoreHandle,
        ) {
            self.targets.lock().unwrap().push(ObservedSecItemTarget {
                operation,
                namespace: handle.namespace().to_string(),
                key: handle.key().to_string(),
            });
        }
    }

    impl MacosSecItemPort for SecItemSpy {
        fn set_secret(
            &self,
            _authorization_context: &MacosAuthorizationContext,
            _service: &str,
            handle: &crate::core::secure_mesh_secret_store::SecretStoreHandle,
            _secret: SecretBytes,
        ) -> Result<()> {
            self.set_count.fetch_add(1, Ordering::SeqCst);
            self.record_target("set", handle);
            Ok(())
        }

        fn get_secret(
            &self,
            _authorization_context: &MacosAuthorizationContext,
            _service: &str,
            handle: &crate::core::secure_mesh_secret_store::SecretStoreHandle,
        ) -> Result<Option<SecretBytes>> {
            self.get_count.fetch_add(1, Ordering::SeqCst);
            self.record_target("get", handle);
            Ok(Some(SecretBytes::try_from_bytes(
                b"sec-item-spy-secret".to_vec(),
            )?))
        }

        fn delete_secret(
            &self,
            _authorization_context: &MacosAuthorizationContext,
            _service: &str,
            handle: &crate::core::secure_mesh_secret_store::SecretStoreHandle,
        ) -> Result<()> {
            self.delete_count.fetch_add(1, Ordering::SeqCst);
            self.record_target("delete", handle);
            Ok(())
        }
    }

    struct BlockingPrompt {
        entered: mpsc::SyncSender<()>,
        release: mpsc::Receiver<()>,
    }

    impl MacosPresencePromptPort for BlockingPrompt {
        fn prompt(
            &mut self,
            _request: &SecretStorePresenceBatchRequest,
        ) -> Result<PresenceDecision> {
            self.entered.send(()).unwrap();
            self.release.recv().unwrap();
            Ok(PresenceDecision::Approved)
        }
    }

    struct SignalingPrompt {
        entered: mpsc::SyncSender<()>,
    }

    impl MacosPresencePromptPort for SignalingPrompt {
        fn prompt(
            &mut self,
            _request: &SecretStorePresenceBatchRequest,
        ) -> Result<PresenceDecision> {
            self.entered.send(()).unwrap();
            Ok(PresenceDecision::Approved)
        }
    }

    fn approve(
        coordinator: &MacosPresenceBatchCoordinator,
        request: &SecretStorePresenceBatchRequest,
        approved_at: Instant,
        prompt_count: Arc<AtomicUsize>,
    ) -> MacosApprovedPresenceBatch {
        let mut prompt = CountingPrompt::new(PresenceDecision::Approved, prompt_count);
        coordinator
            .authorize_batch(request, approved_at, &mut prompt)
            .unwrap()
    }

    fn assert_same_redacted<T: Debug + Display>(first: &T, second: &T, expected_code: &str) {
        let first_display = first.to_string();
        let second_display = second.to_string();
        let first_debug = format!("{first:?}");
        let second_debug = format!("{second:?}");
        assert_eq!(first_display, second_display);
        assert_eq!(first_debug, second_debug);
        assert!(first_display.contains(expected_code));
        for rendered in [first_display, second_display, first_debug, second_debug] {
            for canary in CANARIES {
                assert!(
                    !rendered.contains(canary),
                    "macOS authorization error leaked exact batch context"
                );
            }
        }
    }

    fn assert_same_redacted_debug<T: Debug>(first: &T, second: &T) {
        let first_debug = format!("{first:?}");
        let second_debug = format!("{second:?}");
        assert_eq!(first_debug, second_debug);
        for rendered in [first_debug, second_debug] {
            assert!(!rendered.contains("MacosKeychain"));
            for canary in CANARIES {
                assert!(
                    !rendered.contains(canary),
                    "macOS authorization debug rendering leaked exact batch or operation context"
                );
            }
        }
    }

    fn rejected_grant_consume(
        batch: &SecretStoreApprovedPresenceBatch,
        grant: &SecretStorePresenceGrant,
        expected_scope: &SecretStorePresenceScope,
        now: Instant,
    ) -> Error {
        grant
            .consume(batch, expected_scope, now)
            .unwrap_err()
            .into()
    }

    #[test]
    fn real_set_get_delete_consumers_share_exact_batch_and_run_only_the_effect_port() {
        let coordinator = MacosPresenceBatchCoordinator::new();
        let now = Instant::now();
        let request = alpha_request(3, true);
        let prompt_count = Arc::new(AtomicUsize::new(0));
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut prompt = EventPrompt {
            count: Arc::clone(&prompt_count),
            events: Arc::clone(&events),
        };
        let keychain = SyntheticKeychain {
            events: Arc::clone(&events),
            ..SyntheticKeychain::default()
        };
        let handle = crate::core::secure_mesh_secret_store::SecretStoreHandle::new(
            "batch-namespace",
            "batch-key",
        )
        .unwrap();

        set_secret(
            "synthetic-service",
            &coordinator,
            &request,
            now,
            now,
            &mut prompt,
            &keychain,
            &handle,
            purpose("batch-purpose-write"),
            super::secret("synthetic-secret"),
        )
        .unwrap();
        let read = get_secret(
            "synthetic-service",
            &coordinator,
            &request,
            now,
            now,
            &mut prompt,
            &keychain,
            &handle,
            purpose("batch-purpose-read"),
        )
        .unwrap();
        assert_eq!(
            read.as_ref().map(SecretBytes::expose_bytes),
            Some(b"synthetic-secret".as_slice())
        );
        delete_secret(
            "synthetic-service",
            &coordinator,
            &request,
            now,
            now,
            &mut prompt,
            &keychain,
            &handle,
            purpose("batch-purpose-delete"),
        )
        .unwrap();

        assert_eq!(prompt_count.load(Ordering::SeqCst), 1);
        assert_eq!(keychain.set_count.load(Ordering::SeqCst), 1);
        assert_eq!(keychain.get_count.load(Ordering::SeqCst), 1);
        assert_eq!(keychain.delete_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            events.lock().unwrap().as_slice(),
            &["prompt", "set", "get", "delete"]
        );

        let exceeded = get_secret(
            "synthetic-service",
            &coordinator,
            &request,
            now,
            now,
            &mut prompt,
            &keychain,
            &handle,
            purpose("batch-purpose-excess"),
        )
        .unwrap_err();
        assert!(
            exceeded
                .to_string()
                .contains("secure_mesh_presence_batch_count_exceeded")
        );
        assert_eq!(keychain.get_count.load(Ordering::SeqCst), 1);
        assert_eq!(prompt_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn concurrent_real_get_consumer_runs_one_effect_for_one_counted_batch_slot() {
        const CONTENDER_COUNT: usize = 24;

        let coordinator = Arc::new(MacosPresenceBatchCoordinator::new());
        let request = Arc::new(alpha_request(1, true));
        let handle = Arc::new(
            crate::core::secure_mesh_secret_store::SecretStoreHandle::new(
                "concurrent-consumer-namespace",
                "concurrent-consumer-key",
            )
            .unwrap(),
        );
        let keychain = Arc::new(SyntheticKeychain::default());
        let prompt_count = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(CONTENDER_COUNT));
        let now = Instant::now();

        let contenders = (0..CONTENDER_COUNT)
            .map(|_| {
                let coordinator = Arc::clone(&coordinator);
                let request = Arc::clone(&request);
                let handle = Arc::clone(&handle);
                let keychain = Arc::clone(&keychain);
                let prompt_count = Arc::clone(&prompt_count);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    let mut prompt = CountingPrompt::new(PresenceDecision::Approved, prompt_count);
                    barrier.wait();
                    get_secret(
                        "synthetic-service",
                        &coordinator,
                        &request,
                        now,
                        now,
                        &mut prompt,
                        &*keychain,
                        &handle,
                        purpose("concurrent-consumer-purpose"),
                    )
                })
            })
            .collect::<Vec<_>>();

        let results = contenders
            .into_iter()
            .map(|contender| contender.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| {
                    result.as_ref().is_err_and(|error| {
                        error
                            .to_string()
                            .contains("secure_mesh_presence_batch_count_exceeded")
                    })
                })
                .count(),
            CONTENDER_COUNT - 1
        );
        assert_eq!(keychain.get_count.load(Ordering::SeqCst), 1);
        assert_eq!(prompt_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn exact_context_cache_expires_with_approval_and_refreshed_batches_never_cross_bind() {
        let coordinator = MacosPresenceBatchCoordinator::new();
        let approved_at = Instant::now();
        let expiry = approved_at + MAX_SECRET_STORE_PRESENCE_GRANT_TTL;
        let request = alpha_request(4, true);
        let prompt_count = Arc::new(AtomicUsize::new(0));

        let original = approve(
            &coordinator,
            &request,
            approved_at,
            Arc::clone(&prompt_count),
        );
        let before_expiry = approve(
            &coordinator,
            &request,
            expiry - Duration::from_nanos(1),
            Arc::clone(&prompt_count),
        );
        assert_eq!(prompt_count.load(Ordering::SeqCst), 1);

        let old_scope = operation_scope(
            SecretStoreOperation::Read,
            "cache-namespace",
            "cache-old-purpose",
        );
        let old_grant = original.batch().issue_grant(old_scope.clone()).unwrap();
        before_expiry
            .batch()
            .issue_grant(operation_scope(
                SecretStoreOperation::Write,
                "cache-namespace",
                "cache-shared-count-purpose",
            ))
            .unwrap();
        let refreshed = approve(&coordinator, &request, expiry, Arc::clone(&prompt_count));
        assert_eq!(prompt_count.load(Ordering::SeqCst), 2);
        let new_scope = operation_scope(
            SecretStoreOperation::Read,
            "cache-namespace",
            "cache-new-purpose",
        );
        let new_grant = refreshed.batch().issue_grant(new_scope.clone()).unwrap();

        let old_with_new_error =
            rejected_grant_consume(refreshed.batch(), &old_grant, &old_scope, expiry);
        let new_with_old_error =
            rejected_grant_consume(original.batch(), &new_grant, &new_scope, expiry);
        assert!(
            old_with_new_error
                .to_string()
                .contains("secure_mesh_presence_batch_mismatch")
        );
        assert!(
            new_with_old_error
                .to_string()
                .contains("secure_mesh_presence_batch_mismatch")
        );

        let sec_item_spy = Arc::new(SecItemSpy::default());
        let sec_item_port: Arc<dyn MacosSecItemPort + Send + Sync> = sec_item_spy.clone();
        let real_keychain = SecurityFrameworkKeychain::with_sec_item_port(sec_item_port);
        let handle = crate::core::secure_mesh_secret_store::SecretStoreHandle::new(
            "native-context-namespace",
            "native-context-key",
        )
        .unwrap();

        let old_consumed_for_wrong_context = old_grant
            .consume(
                original.batch(),
                &old_scope,
                expiry - Duration::from_nanos(1),
            )
            .unwrap();
        let wrong_context = refreshed
            .authorization_context()
            .authorize(old_consumed_for_wrong_context)
            .unwrap_err();
        assert_eq!(
            wrong_context.code(),
            "secure_mesh_presence_native_context_mismatch"
        );
        assert_eq!(sec_item_spy.get_count.load(Ordering::SeqCst), 0);

        let exact_old_grant = original.batch().issue_grant(old_scope.clone()).unwrap();
        original
            .batch()
            .issue_grant(operation_scope(
                SecretStoreOperation::Delete,
                "cache-namespace",
                "cache-final-slot-purpose",
            ))
            .unwrap();
        let shared_count_exceeded = before_expiry
            .batch()
            .issue_grant(operation_scope(
                SecretStoreOperation::Delete,
                "cache-namespace",
                "cache-excess-purpose",
            ))
            .unwrap_err();
        assert_eq!(
            shared_count_exceeded.code(),
            "secure_mesh_presence_batch_count_exceeded"
        );
        let exact_old_consumed = exact_old_grant
            .consume(
                original.batch(),
                &old_scope,
                expiry - Duration::from_nanos(1),
            )
            .unwrap();
        let exact_old_authorized = original
            .authorization_context()
            .authorize(exact_old_consumed)
            .unwrap();

        let new_consumed_for_wrong_context = new_grant
            .consume(refreshed.batch(), &new_scope, expiry)
            .unwrap();
        let wrong_context = original
            .authorization_context()
            .authorize(new_consumed_for_wrong_context)
            .unwrap_err();
        assert_eq!(
            wrong_context.code(),
            "secure_mesh_presence_native_context_mismatch"
        );
        assert_eq!(sec_item_spy.get_count.load(Ordering::SeqCst), 0);

        let exact_new_grant = refreshed.batch().issue_grant(new_scope.clone()).unwrap();
        let exact_new_consumed = exact_new_grant
            .consume(refreshed.batch(), &new_scope, expiry)
            .unwrap();
        let exact_new_authorized = refreshed
            .authorization_context()
            .authorize(exact_new_consumed)
            .unwrap();

        let write_scope = operation_scope_with_key(
            SecretStoreOperation::Write,
            "capability-write-namespace",
            "capability-write-key",
            "capability-write-purpose",
        );
        let write_authorized = refreshed
            .authorization_context()
            .authorize(
                refreshed
                    .batch()
                    .issue_grant(write_scope.clone())
                    .unwrap()
                    .consume(refreshed.batch(), &write_scope, expiry)
                    .unwrap(),
            )
            .unwrap();
        let delete_scope = operation_scope_with_key(
            SecretStoreOperation::Delete,
            "capability-delete-namespace",
            "capability-delete-key",
            "capability-delete-purpose",
        );
        let delete_authorized = refreshed
            .authorization_context()
            .authorize(
                refreshed
                    .batch()
                    .issue_grant(delete_scope.clone())
                    .unwrap()
                    .consume(refreshed.batch(), &delete_scope, expiry)
                    .unwrap(),
            )
            .unwrap();

        MacosKeychainEffectPort::set_secret(
            &real_keychain,
            write_authorized,
            "synthetic-service",
            &handle,
            super::secret("synthetic-secret"),
        )
        .unwrap();
        let old_read = MacosKeychainEffectPort::get_secret(
            &real_keychain,
            exact_old_authorized,
            "synthetic-service",
            &handle,
        )
        .unwrap();
        let new_read = MacosKeychainEffectPort::get_secret(
            &real_keychain,
            exact_new_authorized,
            "synthetic-service",
            &handle,
        )
        .unwrap();
        MacosKeychainEffectPort::delete_secret(
            &real_keychain,
            delete_authorized,
            "synthetic-service",
            &handle,
        )
        .unwrap();
        assert_eq!(
            old_read.as_ref().map(SecretBytes::expose_bytes),
            Some(b"sec-item-spy-secret".as_slice())
        );
        assert_eq!(
            new_read.as_ref().map(SecretBytes::expose_bytes),
            Some(b"sec-item-spy-secret".as_slice())
        );
        assert_eq!(sec_item_spy.set_count.load(Ordering::SeqCst), 1);
        assert_eq!(sec_item_spy.get_count.load(Ordering::SeqCst), 2);
        assert_eq!(sec_item_spy.delete_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            sec_item_spy.targets.lock().unwrap().as_slice(),
            &[
                ObservedSecItemTarget {
                    operation: "set",
                    namespace: "capability-write-namespace".to_string(),
                    key: "capability-write-key".to_string(),
                },
                ObservedSecItemTarget {
                    operation: "get",
                    namespace: "cache-namespace".to_string(),
                    key: "fixed-test-key".to_string(),
                },
                ObservedSecItemTarget {
                    operation: "get",
                    namespace: "cache-namespace".to_string(),
                    key: "fixed-test-key".to_string(),
                },
                ObservedSecItemTarget {
                    operation: "delete",
                    namespace: "capability-delete-namespace".to_string(),
                    key: "capability-delete-key".to_string(),
                },
            ]
        );

        let after_coordinator = MacosPresenceBatchCoordinator::new();
        let after_prompt_count = Arc::new(AtomicUsize::new(0));
        approve(
            &after_coordinator,
            &request,
            approved_at,
            Arc::clone(&after_prompt_count),
        );
        approve(
            &after_coordinator,
            &request,
            expiry + Duration::from_nanos(1),
            Arc::clone(&after_prompt_count),
        );
        assert_eq!(after_prompt_count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn every_changed_batch_dimension_requires_an_independent_prompt_or_rejection() {
        let coordinator = MacosPresenceBatchCoordinator::new();
        let now = Instant::now();
        let prompt_count = Arc::new(AtomicUsize::new(0));
        let base = alpha_request(2, true);
        approve(&coordinator, &base, now, Arc::clone(&prompt_count));
        assert_eq!(prompt_count.load(Ordering::SeqCst), 1);

        let mismatches = [
            request(
                SecretStorePresenceProvider::LinuxSecretService,
                SecretStoreKeyClass::DeviceIdentity,
                2,
                CANARIES[0],
                CANARIES[1],
                SecretStoreCallerChannel::DesktopGui,
                true,
            ),
            request(
                SecretStorePresenceProvider::MacosKeychain,
                SecretStoreKeyClass::PairwiseSession,
                2,
                CANARIES[0],
                CANARIES[1],
                SecretStoreCallerChannel::DesktopGui,
                true,
            ),
            alpha_request(3, true),
            request(
                SecretStorePresenceProvider::MacosKeychain,
                SecretStoreKeyClass::DeviceIdentity,
                2,
                CANARIES[5],
                CANARIES[1],
                SecretStoreCallerChannel::DesktopGui,
                true,
            ),
            request(
                SecretStorePresenceProvider::MacosKeychain,
                SecretStoreKeyClass::DeviceIdentity,
                2,
                CANARIES[0],
                CANARIES[6],
                SecretStoreCallerChannel::DesktopGui,
                true,
            ),
            request(
                SecretStorePresenceProvider::MacosKeychain,
                SecretStoreKeyClass::DeviceIdentity,
                2,
                CANARIES[0],
                CANARIES[1],
                SecretStoreCallerChannel::Mobile,
                true,
            ),
        ];

        for mismatch in mismatches {
            let before = prompt_count.load(Ordering::SeqCst);
            let mut prompt =
                CountingPrompt::new(PresenceDecision::Approved, Arc::clone(&prompt_count));
            let result = coordinator.authorize_batch(&mismatch, now, &mut prompt);
            let after = prompt_count.load(Ordering::SeqCst);
            assert!(
                result.is_err() || after == before + 1,
                "a changed batch dimension borrowed an existing native context"
            );
        }
    }

    #[test]
    fn independent_batch_can_enter_prompt_while_another_system_prompt_is_blocked() {
        let coordinator = Arc::new(MacosPresenceBatchCoordinator::new());
        let start = Arc::new(Barrier::new(3));
        let now = Instant::now();
        let first_request = alpha_request(1, true);
        let second_request = beta_request(1, true);
        let (first_entered_tx, first_entered_rx) = mpsc::sync_channel(1);
        let (release_first_tx, release_first_rx) = mpsc::sync_channel(1);
        let (start_second_tx, start_second_rx) = mpsc::sync_channel(1);
        let (second_entered_tx, second_entered_rx) = mpsc::sync_channel(1);
        let (first_completed_tx, first_completed_rx) = mpsc::sync_channel(1);
        let (second_completed_tx, second_completed_rx) = mpsc::sync_channel(1);

        let first_coordinator = Arc::clone(&coordinator);
        let first_start = Arc::clone(&start);
        let first = std::thread::spawn(move || {
            first_start.wait();
            let mut prompt = BlockingPrompt {
                entered: first_entered_tx,
                release: release_first_rx,
            };
            let result = first_coordinator.authorize_batch(&first_request, now, &mut prompt);
            let _ = first_completed_tx.send(());
            result
        });

        let second_coordinator = Arc::clone(&coordinator);
        let second_start = Arc::clone(&start);
        let second = std::thread::spawn(move || {
            second_start.wait();
            let _ = start_second_rx.recv();
            let mut prompt = SignalingPrompt {
                entered: second_entered_tx,
            };
            let result = second_coordinator.authorize_batch(&second_request, now, &mut prompt);
            let _ = second_completed_tx.send(());
            result
        });

        start.wait();
        let first_prompt_entered = first_entered_rx
            .recv_timeout(Duration::from_secs(2))
            .is_ok();
        let _ = start_second_tx.send(());
        let second_entered_while_first_blocked = second_entered_rx
            .recv_timeout(Duration::from_secs(2))
            .is_ok();

        let _ = release_first_tx.send(());
        let first_completed = first_completed_rx
            .recv_timeout(Duration::from_secs(2))
            .is_ok();
        let second_completed = second_completed_rx
            .recv_timeout(Duration::from_secs(2))
            .is_ok();
        let first_result = if first_completed {
            Some(first.join().unwrap())
        } else {
            drop(first);
            None
        };
        let second_result = if second_completed {
            Some(second.join().unwrap())
        } else {
            drop(second);
            None
        };

        assert!(first_prompt_entered, "first prompt never entered");
        assert!(
            second_entered_while_first_blocked,
            "a global authorization cache lock was held across system UI"
        );
        assert!(
            first_completed,
            "first authorization did not cleanly complete"
        );
        assert!(
            second_completed,
            "second authorization did not cleanly complete"
        );
        assert!(first_result.is_some_and(|result| result.is_ok()));
        assert!(second_result.is_some_and(|result| result.is_ok()));
    }

    #[test]
    fn approved_prompt_cannot_bypass_expired_core_consume_in_operation_seam() {
        let coordinator = MacosPresenceBatchCoordinator::new();
        let approved_at = Instant::now();
        let prompt_count = Arc::new(AtomicUsize::new(0));
        let mut prompt = CountingPrompt::new(PresenceDecision::Approved, Arc::clone(&prompt_count));
        let keychain = SyntheticKeychain::default();
        let error = get_secret(
            "synthetic-service",
            &coordinator,
            &alpha_request(1, true),
            approved_at,
            approved_at + MAX_SECRET_STORE_PRESENCE_GRANT_TTL,
            &mut prompt,
            &keychain,
            &crate::core::secure_mesh_secret_store::SecretStoreHandle::new(
                CANARIES[2],
                CANARIES[3],
            )
            .unwrap(),
            purpose(CANARIES[4]),
        )
        .unwrap_err();
        assert_eq!(prompt_count.load(Ordering::SeqCst), 1);
        assert!(error.to_string().contains("secure_mesh_presence_expired"));
        assert_eq!(keychain.get_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn platform_mismatch_replay_and_expiry_are_double_canary_redacted_and_effect_free() {
        let now = Instant::now();
        let alpha_coordinator = MacosPresenceBatchCoordinator::new();
        let beta_coordinator = MacosPresenceBatchCoordinator::new();
        let alpha = approve(
            &alpha_coordinator,
            &alpha_request(7, true),
            now,
            Arc::new(AtomicUsize::new(0)),
        );
        let beta = approve(
            &beta_coordinator,
            &beta_request(8, true),
            now,
            Arc::new(AtomicUsize::new(0)),
        );
        let alpha_scope = operation_scope_with_key(
            SecretStoreOperation::Read,
            CANARIES[2],
            CANARIES[3],
            CANARIES[4],
        );
        let beta_scope = operation_scope_with_key(
            SecretStoreOperation::Delete,
            CANARIES[7],
            CANARIES[8],
            CANARIES[9],
        );
        assert_same_redacted_debug(&alpha, &beta);
        assert_same_redacted_debug(alpha.authorization_context(), beta.authorization_context());

        let alpha_context_swap = alpha.batch().issue_grant(alpha_scope.clone()).unwrap();
        let alpha_consumed = alpha_context_swap
            .consume(alpha.batch(), &alpha_scope, now)
            .unwrap();
        let full_beta_request = request(
            SecretStorePresenceProvider::LinuxSecretService,
            SecretStoreKeyClass::PairwiseSession,
            9,
            CANARIES[5],
            CANARIES[6],
            SecretStoreCallerChannel::Mobile,
            true,
        );
        let full_beta_batch = SecretStoreApprovedPresenceBatch::approve(
            &full_beta_request,
            now,
            MAX_SECRET_STORE_PRESENCE_GRANT_TTL,
            PresenceDecision::Approved,
        )
        .unwrap();
        let full_beta_grant = full_beta_batch.issue_grant(beta_scope.clone()).unwrap();
        let beta_consumed = full_beta_grant
            .consume(&full_beta_batch, &beta_scope, now)
            .unwrap();
        let alpha_error: Error = beta
            .authorization_context()
            .authorize(alpha_consumed)
            .unwrap_err()
            .into();
        let beta_error: Error = alpha
            .authorization_context()
            .authorize(beta_consumed)
            .unwrap_err()
            .into();
        assert_same_redacted(
            &alpha_error,
            &beta_error,
            "secure_mesh_presence_native_context_mismatch",
        );

        let alpha_debug_grant = alpha.batch().issue_grant(alpha_scope.clone()).unwrap();
        let beta_debug_grant = beta.batch().issue_grant(beta_scope.clone()).unwrap();
        let alpha_authorized = alpha
            .authorization_context()
            .authorize(
                alpha_debug_grant
                    .consume(alpha.batch(), &alpha_scope, now)
                    .unwrap(),
            )
            .unwrap();
        let beta_authorized = beta
            .authorization_context()
            .authorize(
                beta_debug_grant
                    .consume(beta.batch(), &beta_scope, now)
                    .unwrap(),
            )
            .unwrap();
        assert_same_redacted_debug(&alpha_authorized, &beta_authorized);

        let alpha_wrong = alpha
            .batch()
            .issue_grant(operation_scope_with_key(
                SecretStoreOperation::Write,
                CANARIES[2],
                CANARIES[3],
                CANARIES[4],
            ))
            .unwrap();
        let beta_wrong = beta
            .batch()
            .issue_grant(operation_scope_with_key(
                SecretStoreOperation::Write,
                CANARIES[7],
                CANARIES[8],
                CANARIES[9],
            ))
            .unwrap();
        let alpha_error = rejected_grant_consume(alpha.batch(), &alpha_wrong, &alpha_scope, now);
        let beta_error = rejected_grant_consume(beta.batch(), &beta_wrong, &beta_scope, now);
        assert_same_redacted(
            &alpha_error,
            &beta_error,
            "secure_mesh_presence_scope_mismatch",
        );

        let alpha_replay = alpha.batch().issue_grant(alpha_scope.clone()).unwrap();
        let beta_replay = beta.batch().issue_grant(beta_scope.clone()).unwrap();
        let _alpha_consumed = alpha_replay
            .consume(alpha.batch(), &alpha_scope, now)
            .unwrap();
        let _beta_consumed = beta_replay.consume(beta.batch(), &beta_scope, now).unwrap();
        let alpha_error = rejected_grant_consume(alpha.batch(), &alpha_replay, &alpha_scope, now);
        let beta_error = rejected_grant_consume(beta.batch(), &beta_replay, &beta_scope, now);
        assert_same_redacted(&alpha_error, &beta_error, "secure_mesh_presence_replayed");

        let alpha_terminal = alpha.batch().issue_grant(alpha_scope.clone()).unwrap();
        let beta_terminal = beta.batch().issue_grant(beta_scope.clone()).unwrap();
        let expired_at = now + MAX_SECRET_STORE_PRESENCE_GRANT_TTL;
        let alpha_error =
            rejected_grant_consume(alpha.batch(), &alpha_terminal, &alpha_scope, expired_at);
        let beta_error =
            rejected_grant_consume(beta.batch(), &beta_terminal, &beta_scope, expired_at);
        assert_same_redacted(&alpha_error, &beta_error, "secure_mesh_presence_expired");
        let revived_at = expired_at - Duration::from_nanos(1);
        let alpha_error =
            rejected_grant_consume(alpha.batch(), &alpha_terminal, &alpha_scope, revived_at);
        let beta_error =
            rejected_grant_consume(beta.batch(), &beta_terminal, &beta_scope, revived_at);
        assert_same_redacted(&alpha_error, &beta_error, "secure_mesh_presence_expired");

        let alpha_expiry_coordinator = MacosPresenceBatchCoordinator::new();
        let beta_expiry_coordinator = MacosPresenceBatchCoordinator::new();
        let mut alpha_prompt =
            CountingPrompt::new(PresenceDecision::Approved, Arc::new(AtomicUsize::new(0)));
        let mut beta_prompt =
            CountingPrompt::new(PresenceDecision::Approved, Arc::new(AtomicUsize::new(0)));
        let alpha_keychain = SyntheticKeychain::default();
        let beta_keychain = SyntheticKeychain::default();
        let alpha_expired = get_secret(
            "synthetic-service",
            &alpha_expiry_coordinator,
            &alpha_request(1, true),
            now,
            expired_at,
            &mut alpha_prompt,
            &alpha_keychain,
            &crate::core::secure_mesh_secret_store::SecretStoreHandle::new(
                CANARIES[2],
                CANARIES[3],
            )
            .unwrap(),
            purpose(CANARIES[4]),
        )
        .unwrap_err();
        let beta_expired = get_secret(
            "synthetic-service",
            &beta_expiry_coordinator,
            &beta_request(1, true),
            now,
            expired_at,
            &mut beta_prompt,
            &beta_keychain,
            &crate::core::secure_mesh_secret_store::SecretStoreHandle::new(
                CANARIES[7],
                CANARIES[8],
            )
            .unwrap(),
            purpose(CANARIES[9]),
        )
        .unwrap_err();
        assert_eq!(alpha_keychain.get_count.load(Ordering::SeqCst), 0);
        assert_eq!(beta_keychain.get_count.load(Ordering::SeqCst), 0);
        assert_same_redacted(
            &alpha_expired,
            &beta_expired,
            "secure_mesh_presence_expired",
        );
    }

    #[test]
    fn platform_cancel_timeout_and_noninteractive_are_double_canary_redacted_and_effect_free() {
        let now = Instant::now();
        for (decision, expected_code) in [
            (
                PresenceDecision::Cancelled,
                "secure_mesh_presence_cancelled",
            ),
            (PresenceDecision::TimedOut, "secure_mesh_presence_timed_out"),
        ] {
            let alpha_coordinator = MacosPresenceBatchCoordinator::new();
            let beta_coordinator = MacosPresenceBatchCoordinator::new();
            let alpha_prompts = Arc::new(AtomicUsize::new(0));
            let beta_prompts = Arc::new(AtomicUsize::new(0));
            let mut alpha_prompt = CountingPrompt::new(decision, Arc::clone(&alpha_prompts));
            let mut beta_prompt = CountingPrompt::new(decision, Arc::clone(&beta_prompts));
            let alpha_keychain = SyntheticKeychain::default();
            let beta_keychain = SyntheticKeychain::default();
            let alpha_error = delete_secret(
                "synthetic-service",
                &alpha_coordinator,
                &alpha_request(1, true),
                now,
                now,
                &mut alpha_prompt,
                &alpha_keychain,
                &crate::core::secure_mesh_secret_store::SecretStoreHandle::new(
                    CANARIES[2],
                    CANARIES[3],
                )
                .unwrap(),
                purpose(CANARIES[4]),
            )
            .unwrap_err();
            let beta_error = delete_secret(
                "synthetic-service",
                &beta_coordinator,
                &beta_request(1, true),
                now,
                now,
                &mut beta_prompt,
                &beta_keychain,
                &crate::core::secure_mesh_secret_store::SecretStoreHandle::new(
                    CANARIES[7],
                    CANARIES[8],
                )
                .unwrap(),
                purpose(CANARIES[9]),
            )
            .unwrap_err();
            assert_eq!(alpha_prompts.load(Ordering::SeqCst), 1);
            assert_eq!(beta_prompts.load(Ordering::SeqCst), 1);
            assert_eq!(alpha_keychain.delete_count.load(Ordering::SeqCst), 0);
            assert_eq!(beta_keychain.delete_count.load(Ordering::SeqCst), 0);
            assert_same_redacted(&alpha_error, &beta_error, expected_code);
        }

        let alpha_coordinator = MacosPresenceBatchCoordinator::new();
        let beta_coordinator = MacosPresenceBatchCoordinator::new();
        let alpha_prompts = Arc::new(AtomicUsize::new(0));
        let beta_prompts = Arc::new(AtomicUsize::new(0));
        let mut alpha_prompt =
            CountingPrompt::new(PresenceDecision::Approved, Arc::clone(&alpha_prompts));
        let mut beta_prompt =
            CountingPrompt::new(PresenceDecision::Approved, Arc::clone(&beta_prompts));
        let alpha_keychain = SyntheticKeychain::default();
        let beta_keychain = SyntheticKeychain::default();
        let alpha_error = get_secret(
            "synthetic-service",
            &alpha_coordinator,
            &alpha_request(1, false),
            now,
            now,
            &mut alpha_prompt,
            &alpha_keychain,
            &crate::core::secure_mesh_secret_store::SecretStoreHandle::new(
                CANARIES[2],
                CANARIES[3],
            )
            .unwrap(),
            purpose(CANARIES[4]),
        )
        .unwrap_err();
        let beta_error = get_secret(
            "synthetic-service",
            &beta_coordinator,
            &beta_request(1, false),
            now,
            now,
            &mut beta_prompt,
            &beta_keychain,
            &crate::core::secure_mesh_secret_store::SecretStoreHandle::new(
                CANARIES[7],
                CANARIES[8],
            )
            .unwrap(),
            purpose(CANARIES[9]),
        )
        .unwrap_err();
        assert_eq!(alpha_prompts.load(Ordering::SeqCst), 0);
        assert_eq!(beta_prompts.load(Ordering::SeqCst), 0);
        assert_eq!(alpha_keychain.get_count.load(Ordering::SeqCst), 0);
        assert_eq!(beta_keychain.get_count.load(Ordering::SeqCst), 0);
        assert_same_redacted(
            &alpha_error,
            &beta_error,
            "secure_mesh_presence_interaction_required",
        );
    }
}
