use serde_json::json;

use super::super::input_codec::{
    MAX_PAYLOAD_BYTES, PayloadSealRequest, decode_base64url, encode_base64url, parse_params,
    parse_payload_kind,
};

#[test]
fn payload_request_projects_canonical_context_and_bounded_body() {
    let encoded_body = encode_base64url(b"bounded-payload");
    let request: PayloadSealRequest = parse_params(&json!({
        "groupIdBase64url": encode_base64url(b"payload-group"),
        "trustedRoster": [],
        "context": {
            "envelopeId": "env-1",
            "messageId": "msg-1",
            "opaqueMailboxId": "mailbox-1",
            "senderEndpointId": "desktop_gui:sender",
            "recipientEndpointId": "mobile:recipient",
            "sessionId": "session-1",
            "createdAt": "2026-07-16T00:00:00Z",
            "expiresAt": "2026-07-16T00:10:00Z"
        },
        "payloadKind": "command",
        "bodyBase64url": encoded_body,
        "contentType": "application/octet-stream"
    }))
    .unwrap();

    let context = request.context.to_context();
    assert_eq!(context.envelope_id, "env-1");
    assert_eq!(context.recipient_endpoint_id, "mobile:recipient");
    assert_eq!(
        parse_payload_kind(&request.payload_kind).unwrap().as_str(),
        "command"
    );
    assert_eq!(
        decode_base64url(&request.body_base64url, "payload body", MAX_PAYLOAD_BYTES).unwrap(),
        b"bounded-payload"
    );
}

#[test]
fn payload_kind_rejects_unregistered_product_values() {
    assert!(parse_payload_kind("raw_private_material").is_err());
}
