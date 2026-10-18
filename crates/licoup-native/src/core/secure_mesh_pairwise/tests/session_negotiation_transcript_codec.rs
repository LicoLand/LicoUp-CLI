use super::test_support::*;

#[test]
fn fixed_base64url_decoder_requires_exact_canonical_encoding() {
    let bytes = [0x42; 32];
    let encoded = general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    assert_eq!(
        decode_fixed_base64url::<32>(&encoded, "vector").unwrap(),
        bytes
    );
    assert!(decode_fixed_base64url::<32>(&format!("{encoded}="), "vector").is_err());
    assert!(decode_fixed_base64url::<32>(&encoded[..encoded.len() - 1], "vector").is_err());
}

#[test]
fn session_id_transcript_is_deterministic_and_binds_pq_ciphertext() {
    let alice = fixed_endpoint("desktop:codec-alice", 1, 41);
    let bob = fixed_endpoint("mobile:codec-bob", 2, 42);
    let ephemeral = fixed_pairwise_key(43).public_key();
    let signed = fixed_pairwise_key(44).public_key();
    let one_time = fixed_pairwise_key(45).public_key();
    let pq =
        SecureMeshMlKem1024PreKeySeed::from_bytes([0x46; ML_KEM_1024_KEY_GENERATION_SEED_BYTES])
            .public_key();
    let make = |ciphertext: &[u8]| {
        derive_session_id(
            &alice.identity,
            &bob.identity,
            &ephemeral,
            "spk-codec",
            &signed,
            Some("otpk-codec"),
            Some(&one_time),
            "pqotpk-codec",
            &pq,
            ciphertext,
            &"ab".repeat(32),
        )
        .unwrap()
    };
    let first = make(&[0x47; ML_KEM_1024_CIPHERTEXT_BYTES]);
    assert_eq!(first, make(&[0x47; ML_KEM_1024_CIPHERTEXT_BYTES]));
    assert_ne!(first, make(&[0x48; ML_KEM_1024_CIPHERTEXT_BYTES]));
    assert!(first.starts_with("sha256:"));
}
