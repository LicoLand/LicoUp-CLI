use super::*;
use serde_json::json;

#[test]
fn json_line_codec_round_trips_objects_and_enforces_the_wire_bound() {
    let message = json!({"jsonrpc": "2.0", "id": 1, "result": {}});
    let encoded = encode_json_line(&message).unwrap();
    assert_eq!(encoded.last(), Some(&b'\n'));
    assert_eq!(decode_json_line(&encoded).unwrap(), message);

    let mut crlf = encoded[..encoded.len() - 1].to_vec();
    crlf.extend_from_slice(b"\r\n");
    assert_eq!(decode_json_line(&crlf).unwrap(), message);
    assert_eq!(
        decode_json_line(b"42").unwrap_err(),
        AcpError::JsonLineInvalid
    );
    assert_eq!(
        decode_json_line(&vec![b'x'; MAX_JSON_LINE_BYTES + 1]).unwrap_err(),
        AcpError::MessageTooLarge
    );
}
