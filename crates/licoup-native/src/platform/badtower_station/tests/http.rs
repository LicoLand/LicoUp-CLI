//! Real loopback HTTP acceptance for the four station operations.

use std::collections::BTreeSet;

use base64::{Engine as _, engine::general_purpose};
use serde_json::{Value, json};

use licoup_protocol_bindings::licoarc_relay::{
    LICOARC_ENCRYPTED_HEADER_BYTES, LicoArcRelayEnvelope, LicoArcRelayEnvelopeDraft,
};

use super::super::{
    BadTowerStationErrorCategory, BadTowerStationOperation, BadTowerStationTransport,
};
use super::support::{CapturedRequest, serve_once};

const EXPIRES_AT: &str = "2030-01-01T00:00:00Z";

#[test]
fn lease_mailbox_uses_only_the_closed_station_contract() {
    // Lico Arc IDs are opaque contract strings, not necessarily decodable
    // base64url byte strings. A length congruent to one modulo four is valid.
    let mailbox_id = "A".repeat(17);
    let (base_url, server) = serve_once(
        "200 OK",
        "application/json",
        json!({
            "mailboxId": mailbox_id,
            "leaseExpiresAt": "2030-01-01T00:01:00Z"
        })
        .to_string(),
    );
    let transport = BadTowerStationTransport::new(base_url).unwrap();
    let hint = transport.lease_mailbox(&mailbox_id, 60).unwrap();
    assert!(hint.station_reported_leased());

    let request = server.join().unwrap();
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, format!("/v1/mailboxes/{mailbox_id}/lease"));
    assert_eq!(
        json_body(&request),
        json!({
            "leaseSeconds": 60
        })
    );
    assert_no_authority_headers(&request);
}

#[test]
fn send_envelope_posts_exactly_the_five_lico_arc_fields() {
    let envelope = synthetic_envelope();
    let (base_url, server) = serve_once(
        "202 Accepted",
        "application/json; charset=utf-8",
        json!({
            "accepted": true,
            "duplicate": false
        })
        .to_string(),
    );
    let transport = BadTowerStationTransport::new(base_url).unwrap();
    let hint = transport.send_envelope(&envelope).unwrap();
    assert!(hint.station_reported_accepted());
    assert!(!hint.station_reported_duplicate());

    let request = server.join().unwrap();
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, "/v1/envelopes");
    let body = json_body(&request);
    assert_eq!(
        object_keys(&body),
        BTreeSet::from([
            "ciphertext",
            "contractVersion",
            "envelopeId",
            "expiresAt",
            "mailboxId",
        ])
    );
    let expected_body: Value = serde_json::from_str(&envelope.to_json().unwrap()).unwrap();
    assert_eq!(body, expected_body);
    assert_no_authority_headers(&request);
}

#[test]
fn receive_envelopes_accepts_only_bound_validated_lico_arc_values() {
    let envelope = synthetic_envelope();
    let mailbox_id = envelope.mailbox_id().to_string();
    let envelope_json: Value = serde_json::from_str(&envelope.to_json().unwrap()).unwrap();
    let (base_url, server) = serve_once(
        "200 OK",
        "application/json",
        json!({
            "envelopes": [envelope_json]
        })
        .to_string(),
    );
    let transport = BadTowerStationTransport::new(base_url).unwrap();
    let received = transport.receive_envelopes(&mailbox_id, 1).unwrap();
    assert_eq!(received, vec![envelope]);

    let request = server.join().unwrap();
    assert_eq!(request.method, "GET");
    assert_eq!(
        request.target,
        format!("/v1/mailboxes/{mailbox_id}/envelopes?limit=1")
    );
    assert!(request.body.is_empty());
    assert_no_authority_headers(&request);
}

#[test]
fn delete_envelope_returns_only_a_station_transport_hint() {
    let mailbox_id = opaque_id(1, 32);
    let envelope_id = opaque_id(2, 24);
    let (base_url, server) = serve_once(
        "200 OK",
        "application/json",
        json!({
            "acknowledged": true
        })
        .to_string(),
    );
    let transport = BadTowerStationTransport::new(base_url).unwrap();
    let hint = transport
        .delete_envelope(&mailbox_id, &envelope_id)
        .unwrap();
    assert!(hint.station_reported_acknowledged());

    let request = server.join().unwrap();
    assert_eq!(request.method, "DELETE");
    assert_eq!(
        request.target,
        format!("/v1/mailboxes/{mailbox_id}/envelopes/{envelope_id}")
    );
    assert!(request.body.is_empty());
    assert_no_authority_headers(&request);
}

#[test]
fn each_operation_rejects_a_non_closed_or_unbound_response() {
    let mailbox_id = opaque_id(1, 32);
    let envelope_id = opaque_id(2, 24);

    let (base_url, server) = serve_once(
        "200 OK",
        "application/json",
        json!({
            "mailboxId": mailbox_id,
            "leaseExpiresAt": "2030-01-01T00:01:00Z",
            "authority": true
        })
        .to_string(),
    );
    let error = BadTowerStationTransport::new(base_url)
        .unwrap()
        .lease_mailbox(&mailbox_id, 60)
        .unwrap_err();
    assert_response_protocol(error, BadTowerStationOperation::LeaseMailbox);
    server.join().unwrap();

    let envelope = synthetic_envelope();
    let (base_url, server) = serve_once(
        "202 Accepted",
        "application/json",
        json!({
            "accepted": true,
            "duplicate": false,
            "endpointReceipt": true
        })
        .to_string(),
    );
    let error = BadTowerStationTransport::new(base_url)
        .unwrap()
        .send_envelope(&envelope)
        .unwrap_err();
    assert_response_protocol(error, BadTowerStationOperation::SendEnvelope);
    server.join().unwrap();

    let mut envelope_json: Value =
        serde_json::from_str(&synthetic_envelope().to_json().unwrap()).unwrap();
    envelope_json["endpointTrust"] = json!(true);
    let (base_url, server) = serve_once(
        "200 OK",
        "application/json",
        json!({
            "envelopes": [envelope_json]
        })
        .to_string(),
    );
    let error = BadTowerStationTransport::new(base_url)
        .unwrap()
        .receive_envelopes(&mailbox_id, 1)
        .unwrap_err();
    assert_response_protocol(error, BadTowerStationOperation::ReceiveEnvelopes);
    server.join().unwrap();

    let (base_url, server) = serve_once(
        "200 OK",
        "application/json",
        json!({
            "acknowledged": true,
            "deletionProof": true
        })
        .to_string(),
    );
    let error = BadTowerStationTransport::new(base_url)
        .unwrap()
        .delete_envelope(&mailbox_id, &envelope_id)
        .unwrap_err();
    assert_response_protocol(error, BadTowerStationOperation::DeleteEnvelope);
    server.join().unwrap();
}

#[test]
fn station_failures_are_stable_hints_without_sensitive_values() {
    let envelope = synthetic_envelope();
    let mailbox_id = envelope.mailbox_id().to_string();
    let envelope_id = envelope.envelope_id().to_string();
    let marker = "synthetic-sensitive-marker";
    let (base_url, server) = serve_once(
        "409 Conflict",
        "application/json",
        format!(r#"{{"error":{{"code":"transport_conflict"}},"detail":"{marker}"}}"#),
    );
    let error = BadTowerStationTransport::new(&base_url)
        .unwrap()
        .send_envelope(&envelope)
        .unwrap_err();
    server.join().unwrap();

    assert_eq!(
        error.category(),
        BadTowerStationErrorCategory::ResponseProtocol
    );
    let projected = format!("{error:?} {error}");
    for forbidden in [&base_url, &mailbox_id, &envelope_id, marker] {
        assert!(!projected.contains(forbidden));
    }

    let (base_url, server) = serve_once(
        "409 Conflict",
        "application/json",
        r#"{"error":{"code":"transport_conflict"}}"#.to_string(),
    );
    let error = BadTowerStationTransport::new(base_url)
        .unwrap()
        .send_envelope(&envelope)
        .unwrap_err();
    server.join().unwrap();
    assert_eq!(
        error.category(),
        BadTowerStationErrorCategory::TransportConflict
    );
    assert!(!error.retryable());
}

#[test]
fn endpoint_and_path_inputs_fail_closed_before_network_io() {
    for denied in [
        "http://station.example",
        "https://user@station.example",
        "https://station.example/v1",
        "https://station.example?token=synthetic",
    ] {
        let error = match BadTowerStationTransport::new(denied) {
            Ok(_) => panic!("invalid station endpoint accepted"),
            Err(error) => error,
        };
        assert_eq!(
            error.operation(),
            BadTowerStationOperation::ConfigureTransport
        );
        assert_eq!(
            error.category(),
            BadTowerStationErrorCategory::InvalidEndpoint
        );
    }

    let transport = BadTowerStationTransport::new("https://station.example").unwrap();
    for invalid_id in ["short", "not%2Fcanonical000", "AAAAAAAAAAAAAAAA="] {
        let error = transport.lease_mailbox(invalid_id, 60).unwrap_err();
        assert_eq!(error.category(), BadTowerStationErrorCategory::InvalidInput);
    }
    let error = transport
        .receive_envelopes(&opaque_id(1, 32), 17)
        .unwrap_err();
    assert_eq!(error.category(), BadTowerStationErrorCategory::InvalidInput);
}

fn synthetic_envelope() -> LicoArcRelayEnvelope {
    LicoArcRelayEnvelopeDraft::from_contract_fields(
        &opaque_id(1, 32),
        &opaque_id(2, 24),
        EXPIRES_AT,
        256,
    )
    .unwrap()
    .finish(&[0u8; LICOARC_ENCRYPTED_HEADER_BYTES], &[0u8; 256])
    .unwrap()
}

fn opaque_id(byte: u8, length: usize) -> String {
    general_purpose::URL_SAFE_NO_PAD.encode(vec![byte; length])
}

fn json_body(request: &CapturedRequest) -> Value {
    serde_json::from_slice(&request.body).unwrap()
}

fn object_keys(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect()
}

fn assert_no_authority_headers(request: &CapturedRequest) {
    assert_eq!(
        request.headers.get("accept").map(String::as_str),
        Some("application/json")
    );
    assert!(!request.headers.contains_key("authorization"));
    assert!(!request.headers.contains_key("cookie"));
    assert!(
        request
            .headers
            .keys()
            .all(|name| !name.starts_with("x-lico-"))
    );
}

fn assert_response_protocol(
    error: super::super::BadTowerStationError,
    operation: BadTowerStationOperation,
) {
    assert_eq!(error.operation(), operation);
    assert_eq!(
        error.category(),
        BadTowerStationErrorCategory::ResponseProtocol
    );
    assert!(!error.retryable());
}
