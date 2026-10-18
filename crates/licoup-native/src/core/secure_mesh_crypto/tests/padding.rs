use super::test_support::*;

#[test]
fn secure_mesh_content_crypto_bucket_padding_hides_length_and_round_trips_boundaries() {
    let key = key_fixture(23);
    let context = context_fixture();
    let mut observed_buckets = Vec::new();
    for body_len in [
        0usize, 1, 31, 32, 127, 128, 511, 4095, 65_535, 65_536, 131_071,
    ] {
        let payload =
            SecureMeshPlaintext::new(SecureMeshPayloadKind::Command, vec![0x5a; body_len]);
        let sealed = seal_payload_with_nonce(
            &key,
            &context,
            &payload,
            [body_len as u8; CONTENT_NONCE_LEN],
        )
        .unwrap();
        assert_eq!(sealed.ciphertext_size % MIN_PADDING_BUCKET_BYTES, 0);
        assert!(sealed.ciphertext_size <= MAX_PADDING_BUCKET_BYTES);
        if sealed.ciphertext_size > POWER_OF_TWO_PADDING_LIMIT_BYTES {
            assert_eq!(sealed.ciphertext_size % LARGE_PADDING_BUCKET_STEP_BYTES, 0);
        } else {
            assert!(sealed.ciphertext_size.is_power_of_two());
        }
        let opened = open_payload(&key, &context, &sealed, SecureMeshPayloadKind::Command).unwrap();
        assert_eq!(opened.body, payload.body);
        observed_buckets.push(sealed.ciphertext_size);
    }
    assert!(
        observed_buckets
            .windows(2)
            .all(|window| window[0] <= window[1]),
        "padding buckets must be monotonic"
    );
}

#[test]
fn secure_mesh_content_crypto_rejects_invalid_padding_and_oversized_bucket() {
    let encoded = encode_plaintext(
        &context_fixture(),
        &SecureMeshPlaintext::new(SecureMeshPayloadKind::ResultPayload, b"bounded"),
    )
    .unwrap();
    let mut padded = add_bucket_padding(&encoded).unwrap();
    let last = padded.len() - 1;
    padded[last] = 1;
    assert!(
        remove_authenticated_padding(&padded)
            .unwrap_err()
            .to_string()
            .contains("padded payload")
    );
    assert!(
        padding_bucket_for_ciphertext_size(MAX_PADDING_BUCKET_BYTES)
            .unwrap_err()
            .to_string()
            .contains("maximum padding bucket")
    );
}
