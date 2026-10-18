use super::test_support::*;

#[test]
fn relay_codec_round_trips_private_header_and_ciphertext_without_server_plaintext() {
    let (mut alice, mut bob) = pairwise_sessions();
    let context = payload_context(
        &alice,
        "msg-relay-codec",
        &alice.local_endpoint_id,
        &alice.remote_endpoint_id,
    );
    let envelope = alice
        .seal_payload_envelope(
            &context,
            &SecureMeshPlaintext::new(SecureMeshPayloadKind::ResultPayload, b"relay secret"),
        )
        .unwrap();
    assert!(!envelope.ciphertext().contains("relay secret"));
    let opened = bob
        .open_payload_envelope(&envelope, SecureMeshPayloadKind::ResultPayload)
        .unwrap();
    assert_eq!(opened.body, b"relay secret");
}

#[test]
fn relay_codec_rejects_an_expired_authenticated_payload_before_advancing_the_ratchet() {
    let (mut alice, mut bob) = pairwise_sessions();
    let mut context = payload_context(
        &alice,
        "msg-expired-relay-codec",
        &alice.local_endpoint_id,
        &alice.remote_endpoint_id,
    );
    context.created_at = "2026-06-26T00:00:00Z".to_string();
    context.expires_at = "2026-06-26T00:10:00Z".to_string();
    let envelope = alice
        .seal_payload_envelope(
            &context,
            &SecureMeshPlaintext::new(SecureMeshPayloadKind::ResultPayload, b"expired"),
        )
        .unwrap();
    let received_before = bob.received_count();
    let skipped_before = bob.skipped_key_count();
    let now = OffsetDateTime::parse("2026-06-26T00:10:01Z", &Rfc3339).unwrap();

    let error = bob
        .open_payload_envelope_at(&envelope, SecureMeshPayloadKind::ResultPayload, now)
        .unwrap_err();

    assert!(error.to_string().contains("expired"));
    assert_eq!(bob.received_count(), received_before);
    assert_eq!(bob.skipped_key_count(), skipped_before);
}
