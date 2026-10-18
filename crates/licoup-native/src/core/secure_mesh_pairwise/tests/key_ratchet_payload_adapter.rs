use super::test_support::*;

#[test]
fn payload_adapter_round_trips_with_bound_extra_aad() {
    let (mut alice, mut bob) = pairwise_sessions();
    let context = payload_context(
        &alice,
        "msg-payload-adapter",
        &alice.local_endpoint_id,
        &alice.remote_endpoint_id,
    );
    let message = alice
        .seal_payload_with_extra_aad(
            &context,
            &SecureMeshPlaintext::new(SecureMeshPayloadKind::Error, b"adapter payload"),
            b"bound-adapter-aad",
        )
        .unwrap();
    assert!(
        bob.open_payload_with_extra_aad(
            &context,
            &message,
            SecureMeshPayloadKind::Error,
            b"wrong-adapter-aad",
        )
        .is_err()
    );
    let opened = bob
        .open_payload_with_extra_aad(
            &context,
            &message,
            SecureMeshPayloadKind::Error,
            b"bound-adapter-aad",
        )
        .unwrap();
    assert_eq!(opened.body, b"adapter payload");
}
