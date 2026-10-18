use super::{
    CAPABILITY_PROOF_MAX_CLOCK_SKEW_SECONDS, CAPABILITY_PROOF_MAX_LIFETIME_SECONDS,
    CLIENT_CAPABILITY_PROJECTION_SCHEMA_VERSION, CapabilityProofRequest,
    CapabilityProofVerificationContext, CapabilitySetProjection, SignedCapabilityProof,
    capability_proof_signature_payload, decode_fixed_base64url,
    decode_signed_capability_proof_json, encode_sha256_digest, encode_signed_capability_proof_json,
    sign_capability_proof, verify_capability_proof,
};
use crate::core::secure_mesh_capability::{
    CapabilityEvaluation, CapabilityEvidenceKind, CapabilityFact, CapabilityScope,
    SecurityCapability, capability_catalog, mandatory_protocol_facts,
};
use crate::core::secure_mesh_trust::DeviceTrustPublicIdentity;
use base64::{Engine as _, engine::general_purpose};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier};
use serde_json::json;

const FIXED_NOW: i64 = 1_900_000_000;

fn digest(seed: u8) -> String {
    encode_sha256_digest(&[seed; 32])
}

fn identity_fixture(
    endpoint_id: &str,
    signing_seed: u8,
) -> (SigningKey, DeviceTrustPublicIdentity) {
    let signing_key = SigningKey::from_bytes(&[signing_seed; 32]);
    let identity = DeviceTrustPublicIdentity::new(
        endpoint_id,
        [signing_seed.wrapping_add(1); 32],
        signing_key.verifying_key().to_bytes(),
        7,
    )
    .unwrap();
    (signing_key, identity)
}

fn baseline_evaluation() -> CapabilityEvaluation {
    capability_catalog()
        .unwrap()
        .evaluate(&mandatory_protocol_facts(CapabilityEvidenceKind::TestFixture).unwrap())
        .unwrap()
}

fn enhanced_evaluation() -> CapabilityEvaluation {
    let mut facts = mandatory_protocol_facts(CapabilityEvidenceKind::TestFixture).unwrap();
    facts.extend([
        CapabilityFact::supported(
            SecurityCapability::OsSecureStore,
            CapabilityEvidenceKind::TestFixture,
        ),
        CapabilityFact::supported(
            SecurityCapability::SoftwareBacked,
            CapabilityEvidenceKind::TestFixture,
        ),
    ]);
    capability_catalog().unwrap().evaluate(&facts).unwrap()
}

fn request() -> CapabilityProofRequest {
    CapabilityProofRequest {
        build_protocol_digest: digest(0x42),
        policy_revision: 9,
        challenge: [0x24; 32],
        issued_at_unix_seconds: FIXED_NOW - 10,
        expires_at_unix_seconds: FIXED_NOW + 120,
    }
}

fn context() -> CapabilityProofVerificationContext {
    CapabilityProofVerificationContext {
        expected_build_protocol_digest: digest(0x42),
        expected_policy_revision: 9,
        expected_challenge: [0x24; 32],
        now_unix_seconds: FIXED_NOW,
    }
}

fn proof_fixture() -> (SigningKey, DeviceTrustPublicIdentity, SignedCapabilityProof) {
    let (signing_key, identity) = identity_fixture("endpoint-vector", 7);
    let proof =
        sign_capability_proof(&identity, &signing_key, &baseline_evaluation(), &request()).unwrap();
    (signing_key, identity, proof)
}

fn resign(proof: &mut SignedCapabilityProof, signing_key: &SigningKey) {
    proof.signature = general_purpose::URL_SAFE_NO_PAD.encode(
        signing_key
            .sign(&capability_proof_signature_payload(&proof.claims).unwrap())
            .to_bytes(),
    );
}

#[test]
fn stable_capability_proof_vector_uses_existing_endpoint_identity_signature() {
    let (_, identity, proof) = proof_fixture();
    let verified = verify_capability_proof(&identity, &proof, &context()).unwrap();
    assert_eq!(
        proof.signature,
        "1UZOP7gXoDjrHzIBkFBaT4SVlRYBZ4ovw9G0naGFSSGCH1ev3mLRUC6-i0yMg0Hid0TAzFfaSUxyVJOK3r1bBg"
    );
    assert_eq!(
        verified.proof_digest(),
        "sha256:_GTxCW4cyLrWuGTLMAIXLD9bNWWdUJD_KR2Z80waScQ"
    );

    let signature = Signature::from_bytes(
        &decode_fixed_base64url::<64>(&proof.signature, "test signature").unwrap(),
    );
    identity
        .signing_verifying_key()
        .unwrap()
        .verify(
            &capability_proof_signature_payload(&proof.claims).unwrap(),
            &signature,
        )
        .unwrap();
}

#[test]
fn canonical_proof_is_independent_of_platform_fact_input_order() {
    let (signing_key, identity) = identity_fixture("endpoint-order", 9);
    let mut facts = mandatory_protocol_facts(CapabilityEvidenceKind::TestFixture).unwrap();
    facts.extend([
        CapabilityFact::supported(
            SecurityCapability::OsSecureStore,
            CapabilityEvidenceKind::TestFixture,
        ),
        CapabilityFact::supported(
            SecurityCapability::SoftwareBacked,
            CapabilityEvidenceKind::TestFixture,
        ),
    ]);
    let first = capability_catalog().unwrap().evaluate(&facts).unwrap();
    facts.reverse();
    let second = capability_catalog().unwrap().evaluate(&facts).unwrap();
    let first = sign_capability_proof(&identity, &signing_key, &first, &request()).unwrap();
    let second = sign_capability_proof(&identity, &signing_key, &second, &request()).unwrap();
    assert_eq!(first, second);
}

#[test]
fn proof_verification_rejects_signature_challenge_freshness_identity_build_and_policy_mismatch() {
    let (signing_key, identity, proof) = proof_fixture();

    let mut tampered = proof.clone();
    tampered.signature.replace_range(0..1, "A");
    assert!(verify_capability_proof(&identity, &tampered, &context()).is_err());

    let mut wrong_challenge = context();
    wrong_challenge.expected_challenge = [0x25; 32];
    assert!(verify_capability_proof(&identity, &proof, &wrong_challenge).is_err());

    let mut stale = context();
    stale.now_unix_seconds = proof.claims.expires_at_unix_seconds + 1;
    assert!(verify_capability_proof(&identity, &proof, &stale).is_err());

    let mut future = proof.clone();
    future.claims.issued_at_unix_seconds =
        context().now_unix_seconds + CAPABILITY_PROOF_MAX_CLOCK_SKEW_SECONDS + 1;
    future.claims.expires_at_unix_seconds = future.claims.issued_at_unix_seconds + 60;
    resign(&mut future, &signing_key);
    assert!(verify_capability_proof(&identity, &future, &context()).is_err());

    let (_, wrong_identity) = identity_fixture("endpoint-other", 8);
    assert!(verify_capability_proof(&wrong_identity, &proof, &context()).is_err());

    let mut wrong_build = context();
    wrong_build.expected_build_protocol_digest = digest(0x43);
    assert!(verify_capability_proof(&identity, &proof, &wrong_build).is_err());

    let mut wrong_policy = context();
    wrong_policy.expected_policy_revision += 1;
    assert!(verify_capability_proof(&identity, &proof, &wrong_policy).is_err());

    const REVISION_1_CATALOG_DIGEST: &str =
        "37796ae8c68f7b93117928a9702f79536e15e43c4a9baf04cd9f3b2c85cf5688";
    assert_ne!(
        capability_catalog().unwrap().digest(),
        REVISION_1_CATALOG_DIGEST
    );
    let mut prior_catalog = proof;
    prior_catalog.claims.capability_catalog_digest = REVISION_1_CATALOG_DIGEST.to_string();
    resign(&mut prior_catalog, &signing_key);
    let error = verify_capability_proof(&identity, &prior_catalog, &context()).unwrap_err();
    assert!(error.to_string().contains("catalog binding"));
}

#[test]
fn proof_verification_rejects_dependency_incomplete_overclaim_and_inexact_sets() {
    let (signing_key, identity, proof) = proof_fixture();

    let mut dependency_overclaim = proof.clone();
    dependency_overclaim
        .claims
        .enabled
        .insert(SecurityCapability::HardwareBacked);
    dependency_overclaim
        .claims
        .available
        .insert(SecurityCapability::HardwareBacked);
    dependency_overclaim
        .claims
        .unverified
        .remove(&SecurityCapability::HardwareBacked);
    dependency_overclaim
        .claims
        .reasons
        .remove(&SecurityCapability::HardwareBacked);
    resign(&mut dependency_overclaim, &signing_key);
    assert!(verify_capability_proof(&identity, &dependency_overclaim, &context()).is_err());

    let mut incomplete_partition = proof.clone();
    incomplete_partition
        .claims
        .unverified
        .remove(&SecurityCapability::Strongbox);
    incomplete_partition
        .claims
        .reasons
        .remove(&SecurityCapability::Strongbox);
    resign(&mut incomplete_partition, &signing_key);
    assert!(verify_capability_proof(&identity, &incomplete_partition, &context()).is_err());

    let mut unsafe_reason = proof;
    unsafe_reason.claims.reasons.insert(
        SecurityCapability::Strongbox,
        "device/path leaked".to_string(),
    );
    resign(&mut unsafe_reason, &signing_key);
    assert!(verify_capability_proof(&identity, &unsafe_reason, &context()).is_err());
}

#[test]
fn proof_json_rejects_unknown_capabilities_unknown_fields_and_noncanonical_encodings() {
    let (_, _, proof) = proof_fixture();
    let mut unknown_capability = serde_json::to_value(&proof).unwrap();
    unknown_capability["claims"]["enabled"] = json!(["protocol.future_mandatory"]);
    assert!(
        decode_signed_capability_proof_json(&serde_json::to_vec(&unknown_capability).unwrap())
            .is_err()
    );

    let mut unknown_field = serde_json::to_value(&proof).unwrap();
    unknown_field["claims"]["deviceName"] = json!("forbidden");
    assert!(
        decode_signed_capability_proof_json(&serde_json::to_vec(&unknown_field).unwrap()).is_err()
    );

    let mut noncanonical = proof;
    noncanonical.claims.challenge.push('=');
    assert!(
        verify_capability_proof(
            &identity_fixture("endpoint-vector", 7).1,
            &noncanonical,
            &context()
        )
        .is_err()
    );
}

#[test]
fn projection_contains_exact_sets_and_redacted_reasons_without_fixed_posture_fields() {
    let (signing_key, identity) = identity_fixture("endpoint-enhanced", 11);
    let proof =
        sign_capability_proof(&identity, &signing_key, &enhanced_evaluation(), &request()).unwrap();
    let projection = verify_capability_proof(&identity, &proof, &context())
        .unwrap()
        .projection()
        .clone();
    assert_eq!(
        projection.schema_version,
        CLIENT_CAPABILITY_PROJECTION_SCHEMA_VERSION
    );
    assert_eq!(
        projection.catalog_digest,
        capability_catalog().unwrap().digest()
    );
    assert!(
        projection
            .enabled
            .contains(&SecurityCapability::OsSecureStore)
    );
    assert!(
        projection
            .enabled
            .contains(&SecurityCapability::SoftwareBacked)
    );
    let encoded = serde_json::to_string(&projection).unwrap();
    assert!(!encoded.contains("\"tier\""));
    assert!(!encoded.contains("\"level\""));
    assert!(!encoded.contains("\"ready\""));
    assert!(!encoded.contains("endpoint-enhanced"));
    assert!(
        projection
            .reasons
            .values()
            .all(|reason| { !reason.contains('/') && !reason.contains(' ') && reason.len() <= 96 })
    );
    let mut fixed_tier = serde_json::to_value(&projection).unwrap();
    fixed_tier["tier"] = json!("highest");
    assert!(serde_json::from_value::<CapabilitySetProjection>(fixed_tier).is_err());
}

#[test]
fn proof_signing_rejects_wrong_signing_key_and_unbounded_lifetime() {
    let (_, identity) = identity_fixture("endpoint-bounds", 12);
    let wrong_key = SigningKey::from_bytes(&[13; 32]);
    assert!(
        sign_capability_proof(&identity, &wrong_key, &baseline_evaluation(), &request()).is_err()
    );

    let (signing_key, identity) = identity_fixture("endpoint-bounds", 12);
    let mut long_lived = request();
    long_lived.expires_at_unix_seconds =
        long_lived.issued_at_unix_seconds + CAPABILITY_PROOF_MAX_LIFETIME_SECONDS + 1;
    assert!(
        sign_capability_proof(&identity, &signing_key, &baseline_evaluation(), &long_lived)
            .is_err()
    );
}

#[test]
fn exact_sets_remain_authoritative_across_json_round_trip() {
    let (_, identity, proof) = proof_fixture();
    let encoded = encode_signed_capability_proof_json(&proof).unwrap();
    let decoded = decode_signed_capability_proof_json(&encoded).unwrap();
    assert_eq!(decoded, proof);
    assert_eq!(
        verify_capability_proof(&identity, &decoded, &context())
            .unwrap()
            .projection(),
        verify_capability_proof(&identity, &proof, &context())
            .unwrap()
            .projection()
    );
}

#[test]
fn proof_scope_catalog_distinguishes_protocol_from_local_custody() {
    let catalog = capability_catalog().unwrap();
    assert_eq!(
        catalog
            .definition(SecurityCapability::AuthenticatedEncryption)
            .unwrap()
            .scope,
        CapabilityScope::ProtocolSession
    );
    assert_eq!(
        catalog
            .definition(SecurityCapability::OsSecureStore)
            .unwrap()
            .scope,
        CapabilityScope::LocalCustody
    );
}
