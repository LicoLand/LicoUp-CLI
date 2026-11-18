use super::*;

#[test]
fn json_line_codec_accepts_numeric_or_string_request_ids() {
    let message = json!({"jsonrpc": "2.0", "id": 1, "result": {}});
    let encoded = encode_message(&message).unwrap();
    assert_eq!(decode_message(&encoded).unwrap(), message);
    assert!(request_id_matches(&message, 1));
    assert!(request_id_matches(&json!({"id": "1"}), 1));
    assert!(decode_message(b"not-json").is_err());
}
