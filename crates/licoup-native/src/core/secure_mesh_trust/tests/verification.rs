use super::super::{
    DeviceTrustState, SAFETY_NUMBER_CHUNK_COUNT, SAFETY_NUMBER_DIGITS_PER_CHUNK,
    SECURE_MESH_DEVICE_TRUST_PROTOCOL_VERSION, detect_identity_key_change, qr_verification_payload,
    sas_decimal_chunks,
};
use super::support::identity_fixture;
use base64::{Engine as _, engine::general_purpose};

#[test]
fn secure_mesh_device_sas_is_symmetric() {
    let (_alice_key, alice) = identity_fixture("desktop_gui:alice");
    let (_bob_key, bob) = identity_fixture("mobile:bob");
    let forward = sas_decimal_chunks(&alice, &bob).unwrap();
    let reverse = sas_decimal_chunks(&bob, &alice).unwrap();
    assert_eq!(forward, reverse);
    assert_eq!(forward.len(), SAFETY_NUMBER_CHUNK_COUNT);
    assert_eq!(
        SAFETY_NUMBER_CHUNK_COUNT * SAFETY_NUMBER_DIGITS_PER_CHUNK,
        60
    );
    assert!(forward.iter().all(|chunk| {
        chunk.len() == SAFETY_NUMBER_DIGITS_PER_CHUNK
            && chunk.bytes().all(|byte| byte.is_ascii_digit())
    }));
}

#[test]
fn secure_mesh_device_qr_payload_uses_fingerprints_not_raw_keys() {
    let (_alice_key, alice) = identity_fixture("desktop_gui:alice");
    let (_bob_key, bob) = identity_fixture("mobile:bob");
    let payload = qr_verification_payload(&alice, &bob, 9).unwrap();
    assert!(payload.starts_with(SECURE_MESH_DEVICE_TRUST_PROTOCOL_VERSION));
    assert!(!payload.contains(&general_purpose::URL_SAFE_NO_PAD.encode(alice.identity_public_key)));
    assert!(!payload.contains(&general_purpose::URL_SAFE_NO_PAD.encode(bob.signing_public_key)));
}

#[test]
fn secure_mesh_device_key_change_is_detected() {
    let (_alice_key, alice) = identity_fixture("desktop_gui:alice");
    let (_replacement_key, mut replacement) = identity_fixture("desktop_gui:alice");
    replacement.endpoint_id = alice.endpoint_id.clone();
    assert_eq!(
        detect_identity_key_change(&alice, &alice).unwrap(),
        DeviceTrustState::Verified
    );
    assert_eq!(
        detect_identity_key_change(&alice, &replacement).unwrap(),
        DeviceTrustState::KeyChanged
    );
}
