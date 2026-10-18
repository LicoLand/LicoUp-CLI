//! Private-header authentication and expiry-binding checks.

use super::support::*;
use sha2::{Digest, Sha256};

#[test]
fn outer_aad_has_a_stable_cross_implementation_vector() {
    let mailbox = schedule(SecureMeshMailboxDirection::PairwiseInitiatorToResponder)
        .token_for_unix_seconds(VECTOR_TIME_SECONDS)
        .unwrap();
    let draft = LicoArcRelayEnvelopeDraft::begin_with_envelope_id(
        &mailbox,
        FIXTURE_EXPIRY,
        MIN_PADDING_BUCKET_BYTES,
        [0x55; DELIVERY_ID_BYTES],
    )
    .unwrap();
    assert_eq!(
        general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(draft.authenticated_outer_data().unwrap())),
        "Ouol2Mairp8S0xGFZ3v1QD7b4_zIlh8gE9xaAYVL-QY"
    );
}

#[test]
fn private_header_round_trip_preserves_fixed_size_and_zeroizing_payload() {
    let mailbox = SecureMeshMailboxToken::from_base64url(
        general_purpose::URL_SAFE_NO_PAD.encode([0x22; MAILBOX_TOKEN_BYTES]),
    )
    .unwrap();
    let draft = LicoArcRelayEnvelopeDraft::begin_with_envelope_id(
        &mailbox,
        FIXTURE_EXPIRY,
        MIN_PADDING_BUCKET_BYTES,
        [0x44; DELIVERY_ID_BYTES],
    )
    .unwrap();
    let header_key = [0x55; RELAY_HEADER_KEY_BYTES];
    let private_header = br#"{"synthetic":"private-header"}"#;
    let encrypted = seal_private_relay_header_with_nonce(
        &draft,
        &header_key,
        private_header,
        [0x66; RELAY_HEADER_NONCE_BYTES],
    )
    .unwrap();
    let envelope = draft
        .finish(&encrypted, &[0x77; MIN_PADDING_BUCKET_BYTES])
        .unwrap();

    assert_eq!(
        envelope.decoded_encrypted_header().unwrap().len(),
        LICOARC_ENCRYPTED_HEADER_BYTES
    );
    assert_eq!(
        open_private_relay_header(&envelope, [header_key.as_slice()])
            .unwrap()
            .as_slice(),
        private_header
    );
}

#[test]
fn private_header_authentication_binds_identifiers_expiry_and_carrier_lengths() {
    let mailbox = SecureMeshMailboxToken::from_base64url(
        general_purpose::URL_SAFE_NO_PAD.encode([0x22; MAILBOX_TOKEN_BYTES]),
    )
    .unwrap();
    let draft = LicoArcRelayEnvelopeDraft::begin_with_envelope_id(
        &mailbox,
        FIXTURE_EXPIRY,
        MIN_PADDING_BUCKET_BYTES,
        [0x44; DELIVERY_ID_BYTES],
    )
    .unwrap();
    let header_key = [0x55; RELAY_HEADER_KEY_BYTES];
    let encrypted =
        seal_private_relay_header(&draft, &header_key, b"synthetic-private-header").unwrap();
    let envelope = draft
        .finish(&encrypted, &[0x77; MIN_PADDING_BUCKET_BYTES])
        .unwrap();
    let base: Value = serde_json::from_str(&envelope.to_json().unwrap()).unwrap();

    for (field, replacement) in [
        (
            "envelopeId",
            json!(general_purpose::URL_SAFE_NO_PAD.encode([0x45; DELIVERY_ID_BYTES])),
        ),
        (
            "mailboxId",
            json!(general_purpose::URL_SAFE_NO_PAD.encode([0x23; MAILBOX_TOKEN_BYTES])),
        ),
        ("expiresAt", json!("2030-01-01T00:00:01Z")),
    ] {
        let mut changed = base.clone();
        changed[field] = replacement;
        let changed =
            LicoArcRelayEnvelope::from_json(&serde_json::to_string(&changed).unwrap()).unwrap();
        assert!(open_private_relay_header(&changed, [header_key.as_slice()]).is_err());
    }

    let larger_draft = LicoArcRelayEnvelopeDraft::from_contract_fields(
        envelope.mailbox_id(),
        envelope.envelope_id(),
        envelope.expires_at(),
        MIN_PADDING_BUCKET_BYTES * 2,
    )
    .unwrap();
    let rebound = larger_draft
        .finish(
            &envelope.decoded_encrypted_header().unwrap(),
            &[0x77; MIN_PADDING_BUCKET_BYTES * 2],
        )
        .unwrap();
    assert!(open_private_relay_header(&rebound, [header_key.as_slice()]).is_err());
}

#[test]
fn private_header_frame_and_candidate_key_work_are_bounded() {
    assert!(encode_private_relay_header_frame(&vec![0x41; MAX_RELAY_PRIVATE_HEADER_BYTES]).is_ok());
    assert!(
        encode_private_relay_header_frame(&vec![0x41; MAX_RELAY_PRIVATE_HEADER_BYTES + 1]).is_err()
    );

    let mut frame = encode_private_relay_header_frame(b"synthetic").unwrap();
    let last = frame.len() - 1;
    frame[last] = 1;
    assert!(decode_private_relay_header_frame(frame).is_err());

    let envelope = envelope_fixture();
    let wrong_keys = vec![[0x91; RELAY_HEADER_KEY_BYTES]; 1_025];
    assert!(
        open_private_relay_header(&envelope, wrong_keys.iter().map(|key| key.as_slice())).is_err()
    );
}
