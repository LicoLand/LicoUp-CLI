use super::test_support::*;

#[test]
fn capability_request_and_verification_context_share_exact_digest_policy_and_challenge() {
    let now = handshake_now();
    let challenge = [0x5a; 32];
    let request = capability_proof_request(challenge, now).unwrap();
    let context = capability_verification_context(challenge, now).unwrap();
    assert_eq!(
        request.build_protocol_digest,
        context.expected_build_protocol_digest
    );
    assert_eq!(request.policy_revision, context.expected_policy_revision);
    assert_eq!(request.challenge, context.expected_challenge);
    assert_eq!(request.issued_at_unix_seconds, context.now_unix_seconds);
    assert!(request.expires_at_unix_seconds > request.issued_at_unix_seconds);
}

#[test]
fn build_protocol_digest_is_deterministic_and_binds_profile_revision() {
    let current = secure_mesh_pairwise_build_protocol_digest_for_revision(
        SECURE_MESH_PROTOCOL_BUILD_REVISION,
    )
    .unwrap();
    let repeated = secure_mesh_pairwise_build_protocol_digest_for_revision(
        SECURE_MESH_PROTOCOL_BUILD_REVISION,
    )
    .unwrap();
    let changed = secure_mesh_pairwise_build_protocol_digest_for_revision(
        SECURE_MESH_PROTOCOL_BUILD_REVISION + 1,
    )
    .unwrap();
    assert_eq!(current, repeated);
    assert_ne!(current, changed);
}
