use super::test_support::*;

#[test]
fn secure_mesh_private_context_crypto_round_trips_full_context_and_payload() {
    let key = key_fixture(61);
    let context = SecureMeshContentContext::new(
        "private-envelope-canary",
        "private-message-canary",
        "private-mailbox-canary",
        "private-sender-canary",
        "private-recipient-canary",
        "private-session-canary",
        "2031-04-05T06:07:08.000Z",
        "2031-04-05T06:17:08.000Z",
    );
    let plaintext = SecureMeshPlaintext::new(
        SecureMeshPayloadKind::ServiceAction,
        b"private-body-canary".as_slice(),
    )
    .with_content_type("application/x-private-canary");
    let sealed = seal_private_context_payload_with_nonce(
        &key,
        &context,
        &plaintext,
        [0x45; CONTENT_NONCE_LEN],
    )
    .unwrap();

    validate_authenticated_padding_bucket(sealed.ciphertext_size()).unwrap();
    assert_eq!(
        sealed.encrypted_header(),
        encode_private_context_header(&[0x45; CONTENT_NONCE_LEN])
    );

    let opened = open_private_context_payload(&key, &sealed).unwrap();
    let (opened_context, opened_payload) = opened.into_parts();
    assert_eq!(opened_context, context);
    assert_eq!(opened_payload.kind, plaintext.kind);
    assert_eq!(opened_payload.body, plaintext.body);
    assert_eq!(opened_payload.content_type, plaintext.content_type);
    assert_eq!(opened_payload.created_at, context.created_at);
    assert_eq!(opened_payload.expires_at, context.expires_at);
}

#[test]
fn secure_mesh_private_context_crypto_rejects_wrong_key_and_profile_header_tamper() {
    let context = context_fixture();
    let plaintext = SecureMeshPlaintext::new(SecureMeshPayloadKind::Command, b"private-command");
    let sealed = seal_private_context_payload_with_nonce(
        &key_fixture(62),
        &context,
        &plaintext,
        [0x46; CONTENT_NONCE_LEN],
    )
    .unwrap();

    let wrong_key_error = open_private_context_payload(&key_fixture(63), &sealed)
        .err()
        .expect("wrong private-context key must fail closed");
    assert!(
        wrong_key_error
            .to_string()
            .contains("authentication failed")
    );

    let mut tampered_header = general_purpose::URL_SAFE_NO_PAD
        .decode(sealed.encrypted_header())
        .unwrap();
    let last = tampered_header.len() - 1;
    tampered_header[last] ^= 0x01;
    let tampered_error = SealedSecureMeshPrivateContextPayload::from_encoded_parts(
        general_purpose::URL_SAFE_NO_PAD.encode(tampered_header),
        sealed.ciphertext().to_string(),
        sealed.ciphertext_size(),
    )
    .unwrap_err();
    assert!(tampered_error.to_string().contains("profile hash mismatch"));
}

#[test]
fn secure_mesh_private_context_crypto_authenticates_padding_and_enforces_bucket_cap() {
    let key = key_fixture(64);
    let context = context_fixture();
    let plaintext =
        SecureMeshPlaintext::new(SecureMeshPayloadKind::ResultPayload, b"bounded-private");
    let frame = encode_private_context_frame(&context, &plaintext).unwrap();
    let mut padded = add_bucket_padding(&frame).unwrap();
    let last = padded.len() - 1;
    padded[last] = 1;
    let nonce = [0x47; CONTENT_NONCE_LEN];
    let derived_key = derive_private_context_aead_key(&key).unwrap();
    let cipher = ChaCha20Poly1305::new(Key::from_slice(derived_key.as_slice()));
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            AeadPayload {
                msg: padded.as_slice(),
                aad: PRIVATE_CONTEXT_AEAD_AAD,
            },
        )
        .unwrap();
    let malformed = SealedSecureMeshPrivateContextPayload::from_encoded_parts(
        encode_private_context_header(&nonce),
        general_purpose::URL_SAFE_NO_PAD.encode(&ciphertext),
        ciphertext.len(),
    )
    .unwrap();
    let padding_error = open_private_context_payload(&key, &malformed)
        .err()
        .expect("authenticated non-zero padding must fail closed");
    assert!(padding_error.to_string().contains("padded payload bytes"));

    let valid = seal_private_context_payload_with_nonce(
        &key,
        &context,
        &plaintext,
        [0x48; CONTENT_NONCE_LEN],
    )
    .unwrap();
    let cap_error = SealedSecureMeshPrivateContextPayload::from_encoded_parts(
        valid.encrypted_header().to_string(),
        valid.ciphertext().to_string(),
        MAX_PADDING_BUCKET_BYTES + LARGE_PADDING_BUCKET_STEP_BYTES,
    )
    .unwrap_err();
    assert!(cap_error.to_string().contains("bucket is outside bounds"));

    let mut oversized_context = context_fixture();
    oversized_context.message_id = format!("{}x", " ".repeat(MAX_CONTEXT_FIELD_BYTES));
    let context_cap_error = seal_private_context_payload_with_nonce(
        &key,
        &oversized_context,
        &plaintext,
        [0x49; CONTENT_NONCE_LEN],
    )
    .unwrap_err();
    assert!(
        context_cap_error
            .to_string()
            .contains("message_id is too large")
    );
}

#[test]
fn secure_mesh_private_context_crypto_rejects_noncanonical_base64url() {
    let key = key_fixture(65);
    let context = context_fixture();
    let plaintext = SecureMeshPlaintext::new(SecureMeshPayloadKind::Command, b"canonical-private");
    let sealed = seal_private_context_payload_with_nonce(
        &key,
        &context,
        &plaintext,
        [0x4a; CONTENT_NONCE_LEN],
    )
    .unwrap();

    assert!(
        SealedSecureMeshPrivateContextPayload::from_encoded_parts(
            format!("{}=", sealed.encrypted_header()),
            sealed.ciphertext().to_string(),
            sealed.ciphertext_size(),
        )
        .is_err()
    );
    assert!(
        SealedSecureMeshPrivateContextPayload::from_encoded_parts(
            sealed.encrypted_header().to_string(),
            format!("{}=", sealed.ciphertext()),
            sealed.ciphertext_size(),
        )
        .is_err()
    );
}
