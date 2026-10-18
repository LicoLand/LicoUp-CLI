use super::test_support::*;

#[test]
fn secure_mesh_content_crypto_rejects_aad_tamper() {
    let key = key_fixture(9);
    let context = context_fixture();
    let payload = SecureMeshPlaintext::new(SecureMeshPayloadKind::Command, b"command");
    let sealed =
        seal_payload_with_nonce(&key, &context, &payload, [3u8; CONTENT_NONCE_LEN]).unwrap();
    let mut tampered = context.clone();
    tampered.message_id = "msg_tampered".to_string();
    let error = open_payload(&key, &tampered, &sealed, SecureMeshPayloadKind::Command).unwrap_err();
    assert!(error.to_string().contains("AAD hash mismatch"));
}

#[test]
fn secure_mesh_content_crypto_rejects_wrong_key() {
    let context = context_fixture();
    let payload = SecureMeshPlaintext::new(SecureMeshPayloadKind::ResultPayload, b"result");
    let sealed = seal_payload_with_nonce(
        &key_fixture(1),
        &context,
        &payload,
        [4u8; CONTENT_NONCE_LEN],
    )
    .unwrap();
    let error = open_payload(
        &key_fixture(2),
        &context,
        &sealed,
        SecureMeshPayloadKind::ResultPayload,
    )
    .unwrap_err();
    assert!(error.to_string().contains("authentication failed"));
}

#[test]
fn secure_mesh_content_crypto_rejects_noncanonical_base64url() {
    let key = key_fixture(5);
    let context = context_fixture();
    let payload = SecureMeshPlaintext::new(SecureMeshPayloadKind::Command, b"canonical");
    let sealed =
        seal_payload_with_nonce(&key, &context, &payload, [5u8; CONTENT_NONCE_LEN]).unwrap();

    let mut padded_header = sealed.clone();
    padded_header.encrypted_header.push('=');
    assert!(
        open_payload(
            &key,
            &context,
            &padded_header,
            SecureMeshPayloadKind::Command,
        )
        .is_err()
    );

    let mut padded_ciphertext = sealed;
    padded_ciphertext.ciphertext.push('=');
    assert!(
        open_payload(
            &key,
            &context,
            &padded_ciphertext,
            SecureMeshPayloadKind::Command,
        )
        .is_err()
    );
}
