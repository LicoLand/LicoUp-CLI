use super::super::envelope::{
    encoded_len_limit, validate_envelope_text_field, validate_secure_envelope,
};
use serde_json::json;

#[test]
fn envelope_codec_rejects_noncanonical_or_oversized_input() {
    assert!(validate_secure_envelope(&json!({"plaintext": "forbidden"})).is_err());
    assert!(validate_envelope_text_field("field", "", 4).is_err());
    assert!(validate_envelope_text_field("field", "12345", 4).is_err());
    assert_eq!(encoded_len_limit(32), 44);
}
