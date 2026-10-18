//! Mailbox schedule checks.

use sha2::{Digest, Sha256};

use super::support::*;

#[test]
fn mailbox_hkdf_has_stable_vector_and_rotates_without_endpoint_hashes() {
    let schedule = schedule(SecureMeshMailboxDirection::PairwiseInitiatorToResponder);
    let token = schedule
        .token_for_unix_seconds(VECTOR_TIME_SECONDS)
        .unwrap();
    assert_eq!(
        token.as_str(),
        "f87aMUBVVxfyb7uf55cSklgJH1424u_d5gpjpue5qr0"
    );
    assert_eq!(
        token.epoch(),
        VECTOR_TIME_SECONDS / SECURE_MESH_MAILBOX_ROTATION_WINDOW_SECONDS
    );
    let next = schedule
        .token_for_unix_seconds(VECTOR_TIME_SECONDS + SECURE_MESH_MAILBOX_ROTATION_WINDOW_SECONDS)
        .unwrap();
    assert_ne!(token.as_str(), next.as_str());
    let unkeyed_endpoint_hash =
        general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(b"example-endpoint-id"));
    assert_ne!(token.as_str(), unkeyed_endpoint_hash);
}

#[test]
fn mailbox_accepts_only_current_and_previous_directional_windows() {
    let now = 50 * SECURE_MESH_MAILBOX_ROTATION_WINDOW_SECONDS + 123;
    let expected = schedule(SecureMeshMailboxDirection::PairwiseInitiatorToResponder);
    let current = expected.token_for_unix_seconds(now).unwrap();
    let previous = expected
        .token_for_unix_seconds(now - SECURE_MESH_MAILBOX_ROTATION_WINDOW_SECONDS)
        .unwrap();
    let future = expected
        .token_for_unix_seconds(now + SECURE_MESH_MAILBOX_ROTATION_WINDOW_SECONDS)
        .unwrap();
    let expired = expected
        .token_for_unix_seconds(now - 2 * SECURE_MESH_MAILBOX_ROTATION_WINDOW_SECONDS)
        .unwrap();

    assert_eq!(
        expected
            .validate_token_for_unix_seconds(current.as_str(), now)
            .unwrap()
            .epoch(),
        current.epoch()
    );
    assert_eq!(
        expected
            .validate_token_for_unix_seconds(previous.as_str(), now)
            .unwrap()
            .epoch(),
        previous.epoch()
    );
    assert!(
        expected
            .validate_token_for_unix_seconds(future.as_str(), now)
            .is_err()
    );
    assert!(
        expected
            .validate_token_for_unix_seconds(expired.as_str(), now)
            .is_err()
    );

    let wrong_direction = schedule(SecureMeshMailboxDirection::PairwiseResponderToInitiator);
    assert!(
        wrong_direction
            .validate_token_for_unix_seconds(current.as_str(), now)
            .is_err()
    );
    let wrong_channel = SecureMeshMailboxSchedule::new(
        SecureMeshDeliverySecret::from_bytes([0x11; DELIVERY_SECRET_BYTES]),
        SecureMeshMailboxDirection::PairwiseInitiatorToResponder,
        SecureMeshRelayChannelBinding::from_bytes([0x23; CHANNEL_BINDING_BYTES]),
    );
    assert!(
        wrong_channel
            .validate_token_for_unix_seconds(current.as_str(), now)
            .is_err()
    );
    let wrong_secret = SecureMeshMailboxSchedule::new(
        SecureMeshDeliverySecret::from_bytes([0x12; DELIVERY_SECRET_BYTES]),
        SecureMeshMailboxDirection::PairwiseInitiatorToResponder,
        SecureMeshRelayChannelBinding::from_bytes([0x22; CHANNEL_BINDING_BYTES]),
    );
    assert!(
        wrong_secret
            .validate_token_for_unix_seconds(current.as_str(), now)
            .is_err()
    );
    let unrelated = general_purpose::URL_SAFE_NO_PAD.encode([0x99; MAILBOX_TOKEN_BYTES]);
    assert!(
        expected
            .validate_token_for_unix_seconds(&unrelated, now)
            .is_err()
    );
    assert!(
        expected
            .token_for_unix_seconds(JSON_SAFE_INTEGER_MAX + 1)
            .is_err()
    );
}

#[test]
fn mailbox_rotation_overlap_is_fixed_and_bounded() {
    let schedule = schedule(SecureMeshMailboxDirection::MlsGroupToMembers);
    for epoch in 1..128u64 {
        let now = epoch * SECURE_MESH_MAILBOX_ROTATION_WINDOW_SECONDS;
        let accepted = schedule.accepted_tokens_for_unix_seconds(now).unwrap();
        assert_eq!(
            accepted.len(),
            1 + SECURE_MESH_MAILBOX_PREVIOUS_WINDOW_COUNT
        );
        assert_eq!(accepted[0].epoch(), epoch);
        assert_eq!(accepted[1].epoch(), epoch - 1);
        assert_ne!(accepted[0].as_str(), accepted[1].as_str());
    }
    assert_eq!(
        schedule.accepted_tokens_for_unix_seconds(0).unwrap().len(),
        1
    );
}
