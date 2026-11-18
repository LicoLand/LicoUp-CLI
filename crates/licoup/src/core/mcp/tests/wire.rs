use super::*;
use serde_json::json;

#[test]
fn request_round_trips_through_stdio_without_service_binding() {
    let message = McpMessage::request(
        7,
        "tools/call",
        Some(object(
            json!({"name": "local-tool", "arguments": {"value": 1}}),
        )),
    )
    .unwrap();
    let encoded = encode_stdio_line(&message, DEFAULT_MAX_MESSAGE_BYTES).unwrap();
    assert!(encoded.ends_with(b"\n"));
    assert_eq!(
        decode_stdio_line(&encoded, DEFAULT_MAX_MESSAGE_BYTES).unwrap(),
        message
    );
}

#[test]
fn encoders_reject_oversized_messages_before_transport() {
    let message = McpMessage::notification(
        "notifications/progress",
        Some(object(json!({"value": "x".repeat(256)}))),
    )
    .unwrap();
    assert!(encode_http_body(&message, 64).is_err());
    assert!(encode_stdio_line(&message, 64).is_err());
    assert!(encode_http_body(&message, 0).is_err());
}

#[test]
fn latest_transport_rejects_batches_and_embedded_stdio_frames() {
    assert!(decode_http_body(b"[]", DEFAULT_MAX_MESSAGE_BYTES).is_err());
    assert!(
        decode_stdio_line(
            b"{\"jsonrpc\":\"2.0\",\"method\":\"ping\"}\n{}\n",
            DEFAULT_MAX_MESSAGE_BYTES,
        )
        .is_err()
    );
}

#[test]
fn response_requires_exactly_one_valid_outcome() {
    assert!(
        McpMessage::from_value(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {},
            "error": {"code": -1, "message": "bad"}
        }))
        .is_err()
    );
    assert!(
        McpMessage::from_value(json!({
            "jsonrpc": "2.0",
            "result": {}
        }))
        .is_err()
    );
    assert!(
        McpMessage::from_value(json!({
            "jsonrpc": "2.0",
            "error": {"code": -32700, "message": "Parse error"}
        }))
        .is_ok()
    );
}
