use super::super::{
    DeviceTrustState, sign_device_cross_signature, sign_device_trust_record,
    verify_device_cross_signature, verify_device_trust_record,
};
use super::support::identity_fixture;

#[test]
fn secure_mesh_device_cross_signature_verifies_and_rejects_tamper() {
    let (alice_key, alice) = identity_fixture("desktop_gui:alice");
    let (_bob_key, bob) = identity_fixture("mobile:bob");
    let cross_signature = sign_device_cross_signature(&alice_key, &alice, &bob, 7).unwrap();
    assert_eq!(
        verify_device_cross_signature(&alice, &bob, &cross_signature).unwrap(),
        DeviceTrustState::CrossSigned
    );

    let mut tampered = bob.clone();
    tampered.rotation_epoch = 2;
    let error = verify_device_cross_signature(&alice, &tampered, &cross_signature).unwrap_err();
    assert!(error.to_string().contains("fingerprint mismatch"));
}

#[test]
fn secure_mesh_device_trust_record_signature_binds_peer_and_expiry() {
    let (alice_key, alice) = identity_fixture("desktop_gui:alice");
    let (_bob_key, bob) = identity_fixture("mobile:bob");
    let record = sign_device_trust_record(
        &alice_key,
        &alice,
        &bob,
        DeviceTrustState::Verified,
        12,
        "sas",
        100,
        200,
    )
    .unwrap();
    assert_eq!(
        verify_device_trust_record(&alice, &bob, &record, 150).unwrap(),
        DeviceTrustState::Verified
    );

    let (_mallory_key, mut mallory) = identity_fixture("mobile:bob");
    mallory.endpoint_id = bob.endpoint_id.clone();
    let error = verify_device_trust_record(&alice, &mallory, &record, 150).unwrap_err();
    assert!(error.to_string().contains("fingerprint mismatch"));

    let expired = verify_device_trust_record(&alice, &bob, &record, 200).unwrap_err();
    assert!(expired.to_string().contains("expired"));

    let mut tampered = record.clone();
    tampered.verification_method = "qr".to_string();
    let error = verify_device_trust_record(&alice, &bob, &tampered, 150).unwrap_err();
    assert!(error.to_string().contains("verification failed"));
}
