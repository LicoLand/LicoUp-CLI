//! Closed five-field Lico Arc v1 JSON contract checks.

use std::collections::BTreeSet;

use super::support::*;

#[test]
fn envelope_serializes_exactly_the_five_normative_fields() {
    let envelope = envelope_fixture();
    let wire = envelope.to_json().unwrap();
    let value: Value = serde_json::from_str(&wire).unwrap();
    let keys: BTreeSet<_> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();

    assert_eq!(keys, LICOARC_RELAY_OUTER_FIELDS.into_iter().collect());
    assert_eq!(value["contractVersion"], LICOARC_RELAY_CONTRACT_VERSION);
    assert_eq!(value["envelopeId"], envelope.envelope_id());
    assert_eq!(value["mailboxId"], envelope.mailbox_id());
    assert_eq!(value["expiresAt"], envelope.expires_at());
    assert_eq!(LicoArcRelayEnvelope::from_json(&wire).unwrap(), envelope);
}

#[test]
fn envelope_rejects_unknown_missing_and_duplicate_fields() {
    let base: Value = serde_json::from_str(&envelope_fixture().to_json().unwrap()).unwrap();
    let mut unknown = base.clone();
    unknown
        .as_object_mut()
        .unwrap()
        .insert("unexpected".to_string(), json!(true));
    assert!(LicoArcRelayEnvelope::from_json(&serde_json::to_string(&unknown).unwrap()).is_err());

    for required in LICOARC_RELAY_OUTER_FIELDS {
        let mut changed = base.clone();
        changed.as_object_mut().unwrap().remove(required);
        assert!(
            LicoArcRelayEnvelope::from_json(&serde_json::to_string(&changed).unwrap()).is_err()
        );
    }

    let duplicate = envelope_fixture().to_json().unwrap().replacen(
        &format!("\"contractVersion\":\"{LICOARC_RELAY_CONTRACT_VERSION}\","),
        &format!(
            "\"contractVersion\":\"{LICOARC_RELAY_CONTRACT_VERSION}\",\
             \"contractVersion\":\"{LICOARC_RELAY_CONTRACT_VERSION}\","
        ),
        1,
    );
    assert!(LicoArcRelayEnvelope::from_json(&duplicate).is_err());
}

#[test]
fn envelope_rejects_invalid_contract_identifiers_expiry_and_ciphertext() {
    let base: Value = serde_json::from_str(&envelope_fixture().to_json().unwrap()).unwrap();
    for (field, value) in [
        ("contractVersion", json!("licoarc.relay.v0")),
        ("envelopeId", json!("short")),
        ("envelopeId", json!("A".repeat(LICOARC_ID_MAX_CHARS + 1))),
        ("mailboxId", json!("invalid+identifier")),
        ("expiresAt", json!("2030-01-01")),
        ("ciphertext", json!("not+base64url")),
    ] {
        let mut changed = base.clone();
        changed
            .as_object_mut()
            .unwrap()
            .insert(field.to_string(), value);
        assert!(
            LicoArcRelayEnvelope::from_json(&serde_json::to_string(&changed).unwrap()).is_err()
        );
    }

    let mut oversized = base;
    oversized["ciphertext"] = json!("A".repeat(LICOARC_MAX_CIPHERTEXT_CHARS + 1));
    assert!(LicoArcRelayEnvelope::from_json(&serde_json::to_string(&oversized).unwrap()).is_err());
}

#[test]
fn expiry_validation_matches_the_normative_rfc3339_boundaries() {
    let base: Value = serde_json::from_str(&envelope_fixture().to_json().unwrap()).unwrap();
    for valid in [
        "2000-02-29T23:59:59.999999999Z",
        "2030-01-01t00:00:00z",
        "2030-01-01T00:00:00-23:59",
        "2030-01-01T23:59:59+23:59",
        "1990-12-31T23:59:60Z",
        "2030-01-01T00:00:00-00:00",
    ] {
        let mut changed = base.clone();
        changed["expiresAt"] = json!(valid);
        assert!(
            LicoArcRelayEnvelope::from_json(&serde_json::to_string(&changed).unwrap()).is_ok(),
            "valid RFC 3339 boundary was rejected"
        );
    }
    for invalid in [
        "2030-02-30T00:00:00Z",
        "2100-02-29T00:00:00Z",
        "2030-01-01T24:00:00Z",
        "1990-12-31T22:59:60Z",
        "2030-01-01T00:60:00Z",
        "2030-01-01T00:00:00+24:00",
        "2030-01-01T00:00:00+00:60",
        "2030-01-01T00:00:00",
    ] {
        let mut changed = base.clone();
        changed["expiresAt"] = json!(invalid);
        assert!(
            LicoArcRelayEnvelope::from_json(&serde_json::to_string(&changed).unwrap()).is_err(),
            "invalid RFC 3339 boundary was accepted"
        );
    }
}

#[test]
fn generated_envelope_ids_are_random_canonical_and_debug_is_redacted() {
    let mailbox = SecureMeshMailboxToken::from_base64url(
        general_purpose::URL_SAFE_NO_PAD.encode([0x22; MAILBOX_TOKEN_BYTES]),
    )
    .unwrap();
    let first = LicoArcRelayEnvelope::new(
        &mailbox,
        FIXTURE_EXPIRY,
        &[0x31; LICOARC_ENCRYPTED_HEADER_BYTES],
        &[0x42; MIN_PADDING_BUCKET_BYTES],
    )
    .unwrap();
    let second = LicoArcRelayEnvelope::new(
        &mailbox,
        FIXTURE_EXPIRY,
        &[0x31; LICOARC_ENCRYPTED_HEADER_BYTES],
        &[0x42; MIN_PADDING_BUCKET_BYTES],
    )
    .unwrap();

    assert_ne!(first.envelope_id(), second.envelope_id());
    assert_eq!(
        general_purpose::URL_SAFE_NO_PAD
            .decode(first.envelope_id())
            .unwrap()
            .len(),
        DELIVERY_ID_BYTES
    );
    let debug = format!("{first:?}");
    assert!(!debug.contains(first.envelope_id()));
    assert!(!debug.contains(first.mailbox_id()));
    assert!(!debug.contains(first.ciphertext()));
    assert!(!debug.contains(first.expires_at()));
}
