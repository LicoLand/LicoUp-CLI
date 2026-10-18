use std::fmt::{Debug, Display};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

use serde_json::json;

use super::super::{
    DeviceTrustState, ProtectedSendPayloadKind, authorize_protected_send,
    authorize_protected_send_from_trust_record, evaluate_device_trust_verification_json,
    qr_verification_payload, sign_device_trust_record,
};
use super::support::{identity_fixture, identity_json};
use crate::core::secure_mesh_secret_store::{
    MAX_SECRET_STORE_PRESENCE_GRANT_TTL, PresenceDecision, SecretStoreApprovedPresenceBatch,
    SecretStoreCallerChannel, SecretStoreConsumedPresence, SecretStoreKeyClass,
    SecretStoreOperation, SecretStorePresenceBatchRequest, SecretStorePresenceGrant,
    SecretStorePresenceNonce, SecretStorePresenceProvider, SecretStorePresencePurpose,
    SecretStorePresenceScope,
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

assert_not_impl!(SecretStoreConsumedPresence: Clone);
assert_not_impl!(SecretStoreConsumedPresence: Copy);
assert_not_impl!(SecretStoreConsumedPresence: Default);
assert_not_impl!(SecretStorePresenceGrant: Clone);
assert_not_impl!(SecretStorePresenceGrant: Copy);
assert_not_impl!(SecretStorePresenceGrant: Default);

const CANARIES: [&str; 10] = [
    "reason-canary-alpha-c115ad",
    "nonce-canary-alpha-79f34e",
    "namespace-canary-alpha-4f55d7",
    "purpose-canary-alpha-76e49d",
    "reason-canary-beta-092acc",
    "nonce-canary-beta-e664a1",
    "namespace-canary-beta-b33c2a",
    "purpose-canary-beta-d8e3a1",
    "key-canary-alpha-51c718",
    "key-canary-beta-2ab194",
];

fn batch_request(
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

fn alpha_batch_request(count: usize, interactive: bool) -> SecretStorePresenceBatchRequest {
    batch_request(
        SecretStorePresenceProvider::MacosKeychain,
        SecretStoreKeyClass::DeviceIdentity,
        count,
        CANARIES[0],
        CANARIES[1],
        SecretStoreCallerChannel::DesktopGui,
        interactive,
    )
}

fn beta_batch_request(count: usize, interactive: bool) -> SecretStorePresenceBatchRequest {
    batch_request(
        SecretStorePresenceProvider::LinuxSecretService,
        SecretStoreKeyClass::PairwiseSession,
        count,
        CANARIES[4],
        CANARIES[5],
        SecretStoreCallerChannel::Mobile,
        interactive,
    )
}

fn approve_batch(
    request: &SecretStorePresenceBatchRequest,
    approved_at: Instant,
) -> SecretStoreApprovedPresenceBatch {
    SecretStoreApprovedPresenceBatch::approve(
        request,
        approved_at,
        MAX_SECRET_STORE_PRESENCE_GRANT_TTL,
        PresenceDecision::Approved,
    )
    .unwrap()
}

fn scope(
    operation: SecretStoreOperation,
    namespace: &str,
    purpose: &str,
) -> SecretStorePresenceScope {
    scope_with_key(operation, namespace, "fixed-test-key", purpose)
}

fn scope_with_key(
    operation: SecretStoreOperation,
    namespace: &str,
    key: &str,
    purpose: &str,
) -> SecretStorePresenceScope {
    SecretStorePresenceScope::new(
        operation,
        namespace,
        key,
        SecretStorePresencePurpose::new(purpose).unwrap(),
    )
    .unwrap()
}

fn grant(
    batch: &SecretStoreApprovedPresenceBatch,
    scope: SecretStorePresenceScope,
) -> SecretStorePresenceGrant {
    batch.issue_grant(scope).unwrap()
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
                "authorization rendering leaked a protected batch or operation context"
            );
        }
    }
}

fn assert_same_redacted_debug<T: Debug>(first: &T, second: &T) {
    let first_debug = format!("{first:?}");
    let second_debug = format!("{second:?}");
    assert_eq!(first_debug, second_debug);
    for rendered in [first_debug, second_debug] {
        for canary in CANARIES {
            assert!(
                !rendered.contains(canary),
                "authorization debug rendering leaked a protected batch or operation context"
            );
        }
    }
}

#[test]
fn secure_mesh_authorize_protected_send_blocks_unverified_key_changed_and_revoked_for_all_kinds() {
    for kind in ProtectedSendPayloadKind::all() {
        let authorized =
            authorize_protected_send("mobile:bob", &DeviceTrustState::Verified, kind).unwrap();
        assert_eq!(authorized.payload_kind(), kind);
        assert_eq!(authorized.peer_endpoint_id(), "mobile:bob");

        for (state, code) in [
            (DeviceTrustState::Unverified, "verification_required"),
            (DeviceTrustState::KeyChanged, "identity_key_changed"),
            (DeviceTrustState::Revoked, "device_revoked"),
            (
                DeviceTrustState::CrossSigned,
                "cross_signature_requires_durable_epoch_validation",
            ),
        ] {
            let error = authorize_protected_send("mobile:bob", &state, kind).unwrap_err();
            let message = error.to_string();
            assert!(message.contains(code));
            assert!(message.contains(kind.as_str()));
        }
    }
}

#[test]
fn secure_mesh_authorize_protected_send_from_trust_record_and_rejects_observation_alone() {
    let (alice_key, alice) = identity_fixture("desktop_gui:alice");
    let (_bob_key, bob) = identity_fixture("mobile:bob");
    let record = sign_device_trust_record(
        &alice_key,
        &alice,
        &bob,
        DeviceTrustState::Verified,
        1,
        "qr",
        100,
        200,
    )
    .unwrap();
    let authorized = authorize_protected_send_from_trust_record(
        &alice,
        &bob,
        &record,
        150,
        ProtectedSendPayloadKind::Command,
    )
    .unwrap();
    assert_eq!(authorized.payload_kind(), ProtectedSendPayloadKind::Command);

    let observation = evaluate_device_trust_verification_json(
        &json!({
            "localIdentity": identity_json(&alice),
            "peerIdentity": identity_json(&bob),
            "qrPayload": qr_verification_payload(&alice, &bob, 1).unwrap(),
            "rosterEpoch": 1
        }),
        "qr",
    )
    .unwrap();
    assert_eq!(observation["observationMatched"], true);
    assert_eq!(observation["decision"]["allowedForHighRiskCommand"], false);
    assert_eq!(
        observation["decision"]["code"],
        "verification_observation_requires_persisted_trust_record"
    );
}

#[test]
fn every_batch_and_operation_dimension_is_bound_and_mismatch_does_not_consume() {
    let now = Instant::now();
    let approved_request = alpha_batch_request(3, true);
    let approved_batch = approve_batch(&approved_request, now);
    let approved_scope = scope(SecretStoreOperation::Read, CANARIES[2], CANARIES[3]);
    let operation_grant = grant(&approved_batch, approved_scope.clone());

    let mismatched_requests = [
        batch_request(
            SecretStorePresenceProvider::LinuxSecretService,
            SecretStoreKeyClass::DeviceIdentity,
            3,
            CANARIES[0],
            CANARIES[1],
            SecretStoreCallerChannel::DesktopGui,
            true,
        ),
        batch_request(
            SecretStorePresenceProvider::MacosKeychain,
            SecretStoreKeyClass::PairwiseSession,
            3,
            CANARIES[0],
            CANARIES[1],
            SecretStoreCallerChannel::DesktopGui,
            true,
        ),
        batch_request(
            SecretStorePresenceProvider::MacosKeychain,
            SecretStoreKeyClass::DeviceIdentity,
            4,
            CANARIES[0],
            CANARIES[1],
            SecretStoreCallerChannel::DesktopGui,
            true,
        ),
        batch_request(
            SecretStorePresenceProvider::MacosKeychain,
            SecretStoreKeyClass::DeviceIdentity,
            3,
            CANARIES[4],
            CANARIES[1],
            SecretStoreCallerChannel::DesktopGui,
            true,
        ),
        batch_request(
            SecretStorePresenceProvider::MacosKeychain,
            SecretStoreKeyClass::DeviceIdentity,
            3,
            CANARIES[0],
            CANARIES[5],
            SecretStoreCallerChannel::DesktopGui,
            true,
        ),
        batch_request(
            SecretStorePresenceProvider::MacosKeychain,
            SecretStoreKeyClass::DeviceIdentity,
            3,
            CANARIES[0],
            CANARIES[1],
            SecretStoreCallerChannel::Mobile,
            true,
        ),
    ];
    for mismatched_request in mismatched_requests {
        let mismatched_batch = approve_batch(&mismatched_request, now);
        let error = operation_grant
            .consume(&mismatched_batch, &approved_scope, now)
            .unwrap_err();
        assert_eq!(error.code(), "secure_mesh_presence_batch_mismatch");
    }

    for mismatched_scope in [
        scope(SecretStoreOperation::Write, CANARIES[2], CANARIES[3]),
        scope(SecretStoreOperation::Read, CANARIES[6], CANARIES[3]),
        scope_with_key(
            SecretStoreOperation::Read,
            CANARIES[2],
            CANARIES[9],
            CANARIES[3],
        ),
        scope(SecretStoreOperation::Read, CANARIES[2], CANARIES[7]),
    ] {
        let error = operation_grant
            .consume(&approved_batch, &mismatched_scope, now)
            .unwrap_err();
        assert_eq!(error.code(), "secure_mesh_presence_scope_mismatch");
    }

    let _consumed = operation_grant
        .consume(&approved_batch, &approved_scope, now)
        .unwrap();
}

#[test]
fn canonical_scope_encoding_cannot_confuse_adjacent_fields() {
    let now = Instant::now();
    let batch = approve_batch(&alpha_batch_request(1, true), now);
    let approved_namespace = "a";
    let approved_key = "b";
    let approved_purpose = "cd";
    let ambiguous_namespace = "ab";
    let ambiguous_key = "c";
    let ambiguous_purpose = "d";
    assert_eq!(
        format!("{approved_namespace}{approved_key}{approved_purpose}"),
        format!("{ambiguous_namespace}{ambiguous_key}{ambiguous_purpose}")
    );
    let approved_scope = scope_with_key(
        SecretStoreOperation::Write,
        approved_namespace,
        approved_key,
        approved_purpose,
    );
    let ambiguous_scope = scope_with_key(
        SecretStoreOperation::Write,
        ambiguous_namespace,
        ambiguous_key,
        ambiguous_purpose,
    );
    let operation_grant = grant(&batch, approved_scope.clone());

    let error = operation_grant
        .consume(&batch, &ambiguous_scope, now)
        .unwrap_err();
    assert_eq!(error.code(), "secure_mesh_presence_scope_mismatch");
    operation_grant
        .consume(&batch, &approved_scope, now)
        .unwrap();
}

#[test]
fn canonical_batch_encoding_cannot_confuse_adjacent_reason_and_nonce_fields() {
    let now = Instant::now();
    let alpha_reason = "reason-prefix";
    let alpha_nonce = "nonce-suffix";
    let beta_reason = "reason";
    let beta_nonce = "-prefixnonce-suffix";
    let alpha = batch_request(
        SecretStorePresenceProvider::MacosKeychain,
        SecretStoreKeyClass::DeviceIdentity,
        1,
        alpha_reason,
        alpha_nonce,
        SecretStoreCallerChannel::DesktopGui,
        true,
    );
    let beta = batch_request(
        SecretStorePresenceProvider::MacosKeychain,
        SecretStoreKeyClass::DeviceIdentity,
        1,
        beta_reason,
        beta_nonce,
        SecretStoreCallerChannel::DesktopGui,
        true,
    );
    assert_eq!(
        format!("{alpha_reason}{alpha_nonce}"),
        format!("{beta_reason}{beta_nonce}")
    );
    assert_ne!(alpha.canonical_digest(), beta.canonical_digest());

    let alpha_batch = approve_batch(&alpha, now);
    let beta_batch = approve_batch(&beta, now);
    let approved_scope = scope(
        SecretStoreOperation::Read,
        "canonical-batch-namespace",
        "canonical-batch-purpose",
    );
    let operation_grant = alpha_batch.issue_grant(approved_scope.clone()).unwrap();
    let error = operation_grant
        .consume(&beta_batch, &approved_scope, now)
        .unwrap_err();
    assert_eq!(error.code(), "secure_mesh_presence_batch_mismatch");
    operation_grant
        .consume(&alpha_batch, &approved_scope, now)
        .unwrap();
}

#[test]
fn canonical_batch_encoding_cannot_confuse_adjacent_count_and_reason_fields() {
    let now = Instant::now();
    let alpha_count = 1;
    let alpha_reason = "23-count-reason-collision";
    let beta_count = 12;
    let beta_reason = "3-count-reason-collision";
    let shared_nonce = "count-reason-shared-nonce";
    assert_eq!(
        format!("{alpha_count}{alpha_reason}"),
        format!("{beta_count}{beta_reason}")
    );

    let alpha = batch_request(
        SecretStorePresenceProvider::MacosKeychain,
        SecretStoreKeyClass::DeviceIdentity,
        alpha_count,
        alpha_reason,
        shared_nonce,
        SecretStoreCallerChannel::DesktopGui,
        true,
    );
    let beta = batch_request(
        SecretStorePresenceProvider::MacosKeychain,
        SecretStoreKeyClass::DeviceIdentity,
        beta_count,
        beta_reason,
        shared_nonce,
        SecretStoreCallerChannel::DesktopGui,
        true,
    );
    assert_ne!(alpha.canonical_digest(), beta.canonical_digest());

    let alpha_batch = approve_batch(&alpha, now);
    let beta_batch = approve_batch(&beta, now);
    let approved_scope = scope_with_key(
        SecretStoreOperation::Read,
        "count-reason-namespace",
        "count-reason-key",
        "count-reason-purpose",
    );
    let operation_grant = alpha_batch.issue_grant(approved_scope.clone()).unwrap();
    let error = operation_grant
        .consume(&beta_batch, &approved_scope, now)
        .unwrap_err();
    assert_eq!(error.code(), "secure_mesh_presence_batch_mismatch");
    operation_grant
        .consume(&alpha_batch, &approved_scope, now)
        .unwrap();
}

#[test]
fn approved_batch_count_is_exact_and_cannot_issue_excess_operation_grants() {
    let now = Instant::now();
    let batch = approve_batch(&alpha_batch_request(2, true), now);
    batch
        .issue_grant(scope(
            SecretStoreOperation::Read,
            "count-namespace",
            "count-purpose-read",
        ))
        .unwrap();
    batch
        .issue_grant(scope(
            SecretStoreOperation::Write,
            "count-namespace",
            "count-purpose-write",
        ))
        .unwrap();
    let exceeded = batch
        .issue_grant(scope(
            SecretStoreOperation::Delete,
            "count-namespace",
            "count-purpose-delete",
        ))
        .unwrap_err();
    assert_eq!(exceeded.code(), "secure_mesh_presence_batch_count_exceeded");
}

#[test]
fn concurrent_consumers_of_one_operation_grant_produce_exactly_one_consumed_presence_token() {
    const CONTENDER_COUNT: usize = 24;

    let now = Instant::now();
    let batch = Arc::new(approve_batch(&alpha_batch_request(1, true), now));
    let approved_scope = scope(
        SecretStoreOperation::Delete,
        "concurrent-namespace",
        "concurrent-purpose",
    );
    let operation_grant = Arc::new(grant(&batch, approved_scope.clone()));
    let barrier = Arc::new(Barrier::new(CONTENDER_COUNT));
    let contenders = (0..CONTENDER_COUNT)
        .map(|_| {
            let batch = Arc::clone(&batch);
            let operation_grant = Arc::clone(&operation_grant);
            let approved_scope = approved_scope.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                operation_grant
                    .consume(&batch, &approved_scope, now)
                    .map_err(|error| error.code())
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
                matches!(result, Err(code) if *code == "secure_mesh_presence_replayed")
            })
            .count(),
        CONTENDER_COUNT - 1
    );
}

#[test]
fn expiry_boundary_executes_before_expiry_and_terminal_expiry_cannot_be_revived() {
    let approved_at = Instant::now();
    let batch = approve_batch(&alpha_batch_request(2, true), approved_at);
    let expiry = approved_at + MAX_SECRET_STORE_PRESENCE_GRANT_TTL;
    let approved_scope = scope(
        SecretStoreOperation::Read,
        "expiry-namespace",
        "expiry-purpose",
    );

    let before_expiry = grant(&batch, approved_scope.clone());
    let _consumed = before_expiry
        .consume(&batch, &approved_scope, expiry - Duration::from_nanos(1))
        .unwrap();

    let terminally_expired = grant(&batch, approved_scope.clone());
    for now in [
        expiry,
        expiry + Duration::from_nanos(1),
        expiry - Duration::from_nanos(1),
    ] {
        let error = terminally_expired
            .consume(&batch, &approved_scope, now)
            .unwrap_err();
        assert_eq!(error.code(), "secure_mesh_presence_expired");
    }
}

#[test]
fn approval_rejects_invalid_ttl_cancel_timeout_and_noninteractive_batch() {
    assert_eq!(MAX_SECRET_STORE_PRESENCE_GRANT_TTL, Duration::from_secs(30));
    let now = Instant::now();
    let interactive = alpha_batch_request(1, true);

    for ttl in [
        Duration::ZERO,
        MAX_SECRET_STORE_PRESENCE_GRANT_TTL + Duration::from_nanos(1),
    ] {
        let error = SecretStoreApprovedPresenceBatch::approve(
            &interactive,
            now,
            ttl,
            PresenceDecision::Approved,
        )
        .unwrap_err();
        assert_eq!(error.code(), "secure_mesh_presence_ttl_invalid");
    }
    for (decision, code) in [
        (
            PresenceDecision::Cancelled,
            "secure_mesh_presence_cancelled",
        ),
        (PresenceDecision::TimedOut, "secure_mesh_presence_timed_out"),
    ] {
        let error = SecretStoreApprovedPresenceBatch::approve(
            &interactive,
            now,
            MAX_SECRET_STORE_PRESENCE_GRANT_TTL,
            decision,
        )
        .unwrap_err();
        assert_eq!(error.code(), code);
    }
    let noninteractive = alpha_batch_request(1, false);
    let error = SecretStoreApprovedPresenceBatch::approve(
        &noninteractive,
        now,
        MAX_SECRET_STORE_PRESENCE_GRANT_TTL,
        PresenceDecision::Approved,
    )
    .unwrap_err();
    assert_eq!(error.code(), "secure_mesh_presence_interaction_required");
}

#[test]
fn batch_scope_grant_and_every_failure_category_are_double_canary_redacted() {
    let now = Instant::now();
    let alpha_request = alpha_batch_request(4, true);
    let beta_request = beta_batch_request(5, true);
    let alpha_batch = approve_batch(&alpha_request, now);
    let beta_batch = approve_batch(&beta_request, now);
    let alpha_scope = scope_with_key(
        SecretStoreOperation::Read,
        CANARIES[2],
        CANARIES[8],
        CANARIES[3],
    );
    let beta_scope = scope_with_key(
        SecretStoreOperation::Delete,
        CANARIES[6],
        CANARIES[9],
        CANARIES[7],
    );
    let alpha_grant = grant(&alpha_batch, alpha_scope.clone());
    let beta_grant = grant(&beta_batch, beta_scope.clone());

    assert_same_redacted_debug(&alpha_request, &beta_request);
    assert_same_redacted_debug(&alpha_batch, &beta_batch);
    assert_same_redacted_debug(&alpha_scope, &beta_scope);
    assert_same_redacted_debug(&alpha_grant, &beta_grant);

    let alpha_mismatch = alpha_grant
        .consume(&beta_batch, &alpha_scope, now)
        .unwrap_err();
    let beta_mismatch = beta_grant
        .consume(&alpha_batch, &beta_scope, now)
        .unwrap_err();
    assert_same_redacted(
        &alpha_mismatch,
        &beta_mismatch,
        "secure_mesh_presence_batch_mismatch",
    );

    let alpha_scope_mismatch = alpha_grant
        .consume(&alpha_batch, &beta_scope, now)
        .unwrap_err();
    let beta_scope_mismatch = beta_grant
        .consume(&beta_batch, &alpha_scope, now)
        .unwrap_err();
    assert_same_redacted(
        &alpha_scope_mismatch,
        &beta_scope_mismatch,
        "secure_mesh_presence_scope_mismatch",
    );

    let alpha_consumed = alpha_grant
        .consume(&alpha_batch, &alpha_scope, now)
        .unwrap();
    let beta_consumed = beta_grant.consume(&beta_batch, &beta_scope, now).unwrap();
    assert_same_redacted_debug(&alpha_consumed, &beta_consumed);
    let alpha_replay = alpha_grant
        .consume(&alpha_batch, &alpha_scope, now)
        .unwrap_err();
    let beta_replay = beta_grant
        .consume(&beta_batch, &beta_scope, now)
        .unwrap_err();
    assert_same_redacted(&alpha_replay, &beta_replay, "secure_mesh_presence_replayed");

    let alpha_expired = grant(&alpha_batch, alpha_scope.clone())
        .consume(
            &alpha_batch,
            &alpha_scope,
            now + MAX_SECRET_STORE_PRESENCE_GRANT_TTL,
        )
        .unwrap_err();
    let beta_expired = grant(&beta_batch, beta_scope.clone())
        .consume(
            &beta_batch,
            &beta_scope,
            now + MAX_SECRET_STORE_PRESENCE_GRANT_TTL,
        )
        .unwrap_err();
    assert_same_redacted(
        &alpha_expired,
        &beta_expired,
        "secure_mesh_presence_expired",
    );

    let alpha_count_batch = approve_batch(&alpha_batch_request(2, true), now);
    let beta_count_batch = approve_batch(&beta_batch_request(3, true), now);
    for _ in 0..2 {
        alpha_count_batch.issue_grant(alpha_scope.clone()).unwrap();
    }
    for _ in 0..3 {
        beta_count_batch.issue_grant(beta_scope.clone()).unwrap();
    }
    let alpha_count_error = alpha_count_batch
        .issue_grant(alpha_scope.clone())
        .unwrap_err();
    let beta_count_error = beta_count_batch
        .issue_grant(beta_scope.clone())
        .unwrap_err();
    assert_same_redacted(
        &alpha_count_error,
        &beta_count_error,
        "secure_mesh_presence_batch_count_exceeded",
    );

    let alpha_ttl_error = SecretStoreApprovedPresenceBatch::approve(
        &alpha_request,
        now,
        MAX_SECRET_STORE_PRESENCE_GRANT_TTL + Duration::from_nanos(1),
        PresenceDecision::Approved,
    )
    .unwrap_err();
    let beta_ttl_error = SecretStoreApprovedPresenceBatch::approve(
        &beta_request,
        now,
        MAX_SECRET_STORE_PRESENCE_GRANT_TTL + Duration::from_nanos(1),
        PresenceDecision::Approved,
    )
    .unwrap_err();
    assert_same_redacted(
        &alpha_ttl_error,
        &beta_ttl_error,
        "secure_mesh_presence_ttl_invalid",
    );

    for (decision, expected_code) in [
        (
            PresenceDecision::Cancelled,
            "secure_mesh_presence_cancelled",
        ),
        (PresenceDecision::TimedOut, "secure_mesh_presence_timed_out"),
    ] {
        let alpha_error = SecretStoreApprovedPresenceBatch::approve(
            &alpha_request,
            now,
            MAX_SECRET_STORE_PRESENCE_GRANT_TTL,
            decision,
        )
        .unwrap_err();
        let beta_error = SecretStoreApprovedPresenceBatch::approve(
            &beta_request,
            now,
            MAX_SECRET_STORE_PRESENCE_GRANT_TTL,
            decision,
        )
        .unwrap_err();
        assert_same_redacted(&alpha_error, &beta_error, expected_code);
    }

    let alpha_noninteractive = alpha_batch_request(1, false);
    let beta_noninteractive = beta_batch_request(1, false);
    let alpha_error = SecretStoreApprovedPresenceBatch::approve(
        &alpha_noninteractive,
        now,
        MAX_SECRET_STORE_PRESENCE_GRANT_TTL,
        PresenceDecision::Approved,
    )
    .unwrap_err();
    let beta_error = SecretStoreApprovedPresenceBatch::approve(
        &beta_noninteractive,
        now,
        MAX_SECRET_STORE_PRESENCE_GRANT_TTL,
        PresenceDecision::Approved,
    )
    .unwrap_err();
    assert_same_redacted(
        &alpha_error,
        &beta_error,
        "secure_mesh_presence_interaction_required",
    );
}
