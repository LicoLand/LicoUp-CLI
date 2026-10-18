use super::{
    CapabilityProofPeer, CapabilityProofReplayGuard, NegotiatedCapabilityBinding,
    SecureMeshSessionKind, accept_mls_capability_binding, accept_pairwise_capability_binding,
    create_mls_capability_binding, create_pairwise_capability_binding,
    validate_restored_negotiation,
};
use crate::core::secure_mesh_capability::{
    CapabilityEvaluation, CapabilityEvidenceKind, CapabilityFact, SecurityCapability,
    capability_catalog, mandatory_protocol_facts,
};
use crate::core::secure_mesh_capability_proof::{
    CLIENT_CAPABILITY_PROJECTION_SCHEMA_VERSION, CapabilityProofRequest,
    CapabilityProofVerificationContext, ClientCapabilityProjection, SignedCapabilityProof,
    VerifiedCapabilityProof, encode_sha256_digest, sign_capability_proof, verify_capability_proof,
};
use crate::core::secure_mesh_trust::DeviceTrustPublicIdentity;
use ed25519_dalek::SigningKey;

const FIXED_NOW: i64 = 1_900_000_000;

struct EndpointFixture {
    identity: DeviceTrustPublicIdentity,
    proof: SignedCapabilityProof,
    context: CapabilityProofVerificationContext,
}

fn digest(seed: u8) -> String {
    encode_sha256_digest(&[seed; 32])
}

fn evaluation(extra: &[SecurityCapability]) -> CapabilityEvaluation {
    let mut facts = mandatory_protocol_facts(CapabilityEvidenceKind::TestFixture).unwrap();
    facts.extend(extra.iter().copied().map(|capability| {
        CapabilityFact::supported(capability, CapabilityEvidenceKind::TestFixture)
    }));
    capability_catalog().unwrap().evaluate(&facts).unwrap()
}

fn endpoint(
    endpoint_id: &str,
    seed: u8,
    build_seed: u8,
    extra: &[SecurityCapability],
) -> EndpointFixture {
    let signing_key = SigningKey::from_bytes(&[seed; 32]);
    let identity = DeviceTrustPublicIdentity::new(
        endpoint_id,
        [seed.wrapping_add(1); 32],
        signing_key.verifying_key().to_bytes(),
        4,
    )
    .unwrap();
    let request = CapabilityProofRequest {
        build_protocol_digest: digest(build_seed),
        policy_revision: 12,
        challenge: [0x55; 32],
        issued_at_unix_seconds: FIXED_NOW - 5,
        expires_at_unix_seconds: FIXED_NOW + 120,
    };
    let proof =
        sign_capability_proof(&identity, &signing_key, &evaluation(extra), &request).unwrap();
    let context = CapabilityProofVerificationContext {
        expected_build_protocol_digest: request.build_protocol_digest,
        expected_policy_revision: request.policy_revision,
        expected_challenge: request.challenge,
        now_unix_seconds: FIXED_NOW,
    };
    EndpointFixture {
        identity,
        proof,
        context,
    }
}

fn peer<'a>(endpoint: &'a EndpointFixture) -> CapabilityProofPeer<'a> {
    CapabilityProofPeer {
        identity: &endpoint.identity,
        proof: &endpoint.proof,
        verification_context: &endpoint.context,
    }
}

fn verified(endpoint: &EndpointFixture) -> VerifiedCapabilityProof {
    verify_capability_proof(&endpoint.identity, &endpoint.proof, &endpoint.context).unwrap()
}

fn fixtures() -> (EndpointFixture, EndpointFixture) {
    let first = endpoint(
        "endpoint-alpha",
        31,
        0xa1,
        &[
            SecurityCapability::OsSecureStore,
            SecurityCapability::SoftwareBacked,
            SecurityCapability::LinuxSecretService,
        ],
    );
    let second = endpoint(
        "endpoint-beta",
        32,
        0xb2,
        &[
            SecurityCapability::OsSecureStore,
            SecurityCapability::NonExportable,
            SecurityCapability::DeviceBound,
            SecurityCapability::HardwareBacked,
            SecurityCapability::Tee,
            SecurityCapability::AndroidKeystore,
        ],
    );
    (first, second)
}

#[test]
fn pairwise_and_mls_transcripts_bind_both_proofs_and_protocol_only_intersection() {
    let (first, second) = fixtures();
    let first_verified = verified(&first);
    let second_verified = verified(&second);
    let pairwise =
        create_pairwise_capability_binding(&first_verified, &second_verified, &digest(0xc3))
            .unwrap();
    let mls =
        create_mls_capability_binding(&first_verified, &second_verified, &digest(0xd4)).unwrap();

    assert_eq!(pairwise.capability_proof_digests.len(), 2);
    assert_eq!(mls.capability_proof_digests.len(), 2);
    assert_ne!(pairwise.transcript_digest, mls.transcript_digest);
    assert!(
        pairwise
            .negotiated_protocol_capabilities
            .iter()
            .all(|capability| capability.id().starts_with("protocol."))
    );
    assert!(
        !pairwise
            .negotiated_protocol_capabilities
            .contains(&SecurityCapability::OsSecureStore)
    );
    assert!(
        first_verified
            .projection()
            .enabled
            .contains(&SecurityCapability::LinuxSecretService)
    );
    assert!(
        second_verified
            .projection()
            .enabled
            .contains(&SecurityCapability::HardwareBacked)
    );
}

#[test]
fn stable_pairwise_and_mls_transcript_vectors_are_deterministic() {
    let (first, second) = fixtures();
    let first = verified(&first);
    let second = verified(&second);
    let pairwise = create_pairwise_capability_binding(&first, &second, &digest(0xc3)).unwrap();
    let mls = create_mls_capability_binding(&first, &second, &digest(0xd4)).unwrap();
    assert_eq!(
        pairwise.transcript_digest,
        "sha256:QdEF_MBAQpxTB1cuyu6jX80moqptttmXvsb2byh9fGg"
    );
    assert_eq!(
        mls.transcript_digest,
        "sha256:dQTsUOMYHVpn33HVrIvCigfzIFu-2vvQEazzzLY2U-Y"
    );
}

#[test]
fn binding_tamper_downgrade_scope_injection_and_mismatch_fail_before_acceptance() {
    let (first, second) = fixtures();
    let expected =
        create_pairwise_capability_binding(&verified(&first), &verified(&second), &digest(0xc3))
            .unwrap();

    let assert_rejected = |binding: NegotiatedCapabilityBinding| {
        assert!(
            accept_pairwise_capability_binding(
                peer(&first),
                peer(&second),
                &digest(0xc3),
                &binding,
                &mut CapabilityProofReplayGuard::default(),
            )
            .is_err()
        );
    };

    let mut downgrade = expected.clone();
    downgrade
        .negotiated_protocol_capabilities
        .remove(&SecurityCapability::AuthenticatedPadding);
    assert_rejected(downgrade);

    let mut custody_injection = expected.clone();
    custody_injection
        .negotiated_protocol_capabilities
        .insert(SecurityCapability::OsSecureStore);
    assert_rejected(custody_injection);

    let mut proof_tamper = expected.clone();
    proof_tamper.capability_proof_digests[0] = digest(0xee);
    assert_rejected(proof_tamper);

    let mut transcript_tamper = expected.clone();
    transcript_tamper.transcript_digest = digest(0xef);
    assert_rejected(transcript_tamper);

    let base_mismatch = expected.clone();
    assert!(
        accept_pairwise_capability_binding(
            peer(&first),
            peer(&second),
            &digest(0xc4),
            &base_mismatch,
            &mut CapabilityProofReplayGuard::default(),
        )
        .is_err()
    );

    let mut wrong_kind = expected;
    wrong_kind.session_kind = SecureMeshSessionKind::Mls;
    assert_rejected(wrong_kind);
}

#[test]
fn mls_binding_rejects_proof_digest_and_negotiated_set_tamper() {
    let (first, second) = fixtures();
    let expected =
        create_mls_capability_binding(&verified(&first), &verified(&second), &digest(0xd4))
            .unwrap();

    let mut proof_tamper = expected.clone();
    proof_tamper.capability_proof_digests[1] = digest(0xf1);
    assert!(
        accept_mls_capability_binding(
            peer(&first),
            peer(&second),
            &digest(0xd4),
            &proof_tamper,
            &mut CapabilityProofReplayGuard::default(),
        )
        .is_err()
    );

    let mut downgrade = expected;
    downgrade
        .negotiated_protocol_capabilities
        .remove(&SecurityCapability::RatchetForwardSecrecy);
    assert!(
        accept_mls_capability_binding(
            peer(&first),
            peer(&second),
            &digest(0xd4),
            &downgrade,
            &mut CapabilityProofReplayGuard::default(),
        )
        .is_err()
    );
}

#[test]
fn exact_capability_proof_replay_is_rejected_before_second_session_acceptance() {
    let (first, second) = fixtures();
    let binding =
        create_pairwise_capability_binding(&verified(&first), &verified(&second), &digest(0xc3))
            .unwrap();
    let mut guard = CapabilityProofReplayGuard::default();
    accept_pairwise_capability_binding(
        peer(&first),
        peer(&second),
        &digest(0xc3),
        &binding,
        &mut guard,
    )
    .unwrap();
    assert!(
        accept_pairwise_capability_binding(
            peer(&first),
            peer(&second),
            &digest(0xc3),
            &binding,
            &mut guard,
        )
        .is_err()
    );
}

#[test]
fn missing_mandatory_capability_and_cross_session_binding_fail_closed() {
    let first = endpoint("endpoint-alpha", 41, 0xa1, &[]);
    let mut facts = mandatory_protocol_facts(CapabilityEvidenceKind::TestFixture).unwrap();
    facts.retain(|fact| fact.capability != SecurityCapability::AuthenticatedPadding);
    let incomplete = capability_catalog().unwrap().evaluate(&facts).unwrap();
    let signing_key = SigningKey::from_bytes(&[42; 32]);
    let identity = DeviceTrustPublicIdentity::new(
        "endpoint-incomplete",
        [43; 32],
        signing_key.verifying_key().to_bytes(),
        4,
    )
    .unwrap();
    let request = CapabilityProofRequest {
        build_protocol_digest: digest(0xb2),
        policy_revision: 12,
        challenge: [0x55; 32],
        issued_at_unix_seconds: FIXED_NOW - 5,
        expires_at_unix_seconds: FIXED_NOW + 120,
    };
    let proof = sign_capability_proof(&identity, &signing_key, &incomplete, &request).unwrap();
    let context = CapabilityProofVerificationContext {
        expected_build_protocol_digest: request.build_protocol_digest,
        expected_policy_revision: request.policy_revision,
        expected_challenge: request.challenge,
        now_unix_seconds: FIXED_NOW,
    };
    let second = EndpointFixture {
        identity,
        proof,
        context,
    };
    assert!(
        create_pairwise_capability_binding(&verified(&first), &verified(&second), &digest(0xc3),)
            .is_err()
    );

    let (first, second) = fixtures();
    let pairwise =
        create_pairwise_capability_binding(&verified(&first), &verified(&second), &digest(0xc3))
            .unwrap();
    assert!(
        accept_mls_capability_binding(
            peer(&first),
            peer(&second),
            &digest(0xc3),
            &pairwise,
            &mut CapabilityProofReplayGuard::default(),
        )
        .is_err()
    );
}

#[test]
fn challenge_and_policy_mismatch_are_rejected_before_transcript_acceptance() {
    let (first, mut second) = fixtures();
    second.context.expected_challenge = [0x56; 32];
    let binding = create_pairwise_capability_binding(
        &verified(&first),
        &verify_capability_proof(
            &second.identity,
            &second.proof,
            &CapabilityProofVerificationContext {
                expected_challenge: [0x55; 32],
                ..second.context.clone()
            },
        )
        .unwrap(),
        &digest(0xc3),
    )
    .unwrap();
    assert!(
        accept_pairwise_capability_binding(
            peer(&first),
            peer(&second),
            &digest(0xc3),
            &binding,
            &mut CapabilityProofReplayGuard::default(),
        )
        .is_err()
    );

    let (first, mut second) = fixtures();
    second.context.expected_policy_revision += 1;
    assert!(
        accept_pairwise_capability_binding(
            peer(&first),
            peer(&second),
            &digest(0xc3),
            &create_pairwise_capability_binding(
                &verified(&first),
                &verify_capability_proof(
                    &second.identity,
                    &second.proof,
                    &CapabilityProofVerificationContext {
                        expected_policy_revision: 12,
                        ..second.context.clone()
                    },
                )
                .unwrap(),
                &digest(0xc3),
            )
            .unwrap(),
            &mut CapabilityProofReplayGuard::default(),
        )
        .is_err()
    );
}

#[test]
fn accepted_projection_contains_exact_local_peer_and_negotiated_sets_without_tier() {
    let (first, second) = fixtures();
    let binding =
        create_mls_capability_binding(&verified(&first), &verified(&second), &digest(0xd4))
            .unwrap();
    let accepted = accept_mls_capability_binding(
        peer(&first),
        peer(&second),
        &digest(0xd4),
        &binding,
        &mut CapabilityProofReplayGuard::default(),
    )
    .unwrap();
    let encoded = serde_json::to_string(accepted.projection()).unwrap();
    assert!(
        accepted
            .projection()
            .local
            .enabled
            .contains(&SecurityCapability::LinuxSecretService)
    );
    assert!(
        accepted
            .projection()
            .peer
            .as_ref()
            .unwrap()
            .enabled
            .contains(&SecurityCapability::HardwareBacked)
    );
    assert_eq!(
        accepted.projection().negotiated_protocol_capabilities,
        binding.negotiated_protocol_capabilities
    );
    assert_eq!(
        accepted.projection().schema_version,
        CLIENT_CAPABILITY_PROJECTION_SCHEMA_VERSION
    );
    assert_eq!(
        accepted.projection().local.catalog_digest,
        capability_catalog().unwrap().digest()
    );
    assert_eq!(
        accepted.projection().peer.as_ref().unwrap().catalog_digest,
        accepted.projection().local.catalog_digest
    );
    let mut catalog_tamper = accepted.projection().clone();
    catalog_tamper.peer.as_mut().unwrap().catalog_digest = "0".repeat(64);
    assert!(
        validate_restored_negotiation(SecureMeshSessionKind::Mls, &binding, &catalog_tamper,)
            .is_err()
    );
    assert!(!encoded.contains("\"tier\""));
    assert!(!encoded.contains("\"level\""));
    assert!(!encoded.contains("\"ready\""));
    let mut fixed_tier = serde_json::to_value(accepted.projection()).unwrap();
    fixed_tier["tier"] = serde_json::json!("highest");
    assert!(serde_json::from_value::<ClientCapabilityProjection>(fixed_tier).is_err());
}

#[test]
fn replay_guard_never_evicts_unexpired_proofs_to_accept_new_sessions() {
    let (first, second) = fixtures();
    let binding =
        create_pairwise_capability_binding(&verified(&first), &verified(&second), &digest(0xc3))
            .unwrap();
    let mut guard = CapabilityProofReplayGuard::with_capacity(2).unwrap();
    accept_pairwise_capability_binding(
        peer(&first),
        peer(&second),
        &digest(0xc3),
        &binding,
        &mut guard,
    )
    .unwrap();

    let third = endpoint("endpoint-gamma", 33, 0xc4, &[]);
    let fourth = endpoint("endpoint-delta", 34, 0xd5, &[]);
    let next_binding =
        create_pairwise_capability_binding(&verified(&third), &verified(&fourth), &digest(0xe6))
            .unwrap();
    assert!(
        accept_pairwise_capability_binding(
            peer(&third),
            peer(&fourth),
            &digest(0xe6),
            &next_binding,
            &mut guard,
        )
        .is_err()
    );

    guard.prune_expired(FIXED_NOW + 120);
    assert_eq!(guard.consumed.len(), 2);
    guard.prune_expired(FIXED_NOW + 121);
    assert!(guard.consumed.is_empty());
    assert!(guard.expiry_index.is_empty());
}

#[test]
fn proof_digests_are_bound_even_when_builds_differ() {
    let (first, second) = fixtures();
    let first = verified(&first);
    let second = verified(&second);
    assert_ne!(
        first.claims().build_protocol_digest,
        second.claims().build_protocol_digest
    );
    assert_ne!(first.proof_digest(), second.proof_digest());
    assert!(create_pairwise_capability_binding(&first, &second, &digest(0xc3)).is_ok());
}
