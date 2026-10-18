use super::test_support::*;

#[test]
fn intro_validation_rejects_same_endpoint_and_noncanonical_signature() {
    let fixture = handshake_fixture();
    assert!(ensure_intro(&fixture.intro).is_ok());

    let mut same_endpoint = fixture.intro.clone();
    same_endpoint.responder_endpoint_id = same_endpoint.initiator_endpoint_id.clone();
    assert!(ensure_intro(&same_endpoint).is_err());

    let mut padded_signature = fixture.intro;
    padded_signature.initiator_signature.push('=');
    assert!(ensure_intro(&padded_signature).is_err());
}

#[test]
fn local_identity_validation_binds_both_identity_and_signing_secrets() {
    let endpoint = fixed_endpoint("desktop:identity-validation", 1, 51);
    assert!(
        ensure_local_identity_key_material(
            &endpoint.identity,
            &endpoint.identity_secret,
            &endpoint.signing_key,
        )
        .is_ok()
    );
    let wrong_identity_secret = fixed_pairwise_key(52);
    assert!(
        ensure_local_identity_key_material(
            &endpoint.identity,
            &wrong_identity_secret,
            &endpoint.signing_key,
        )
        .is_err()
    );
}
