use crate::domain::mobile_relay::key_transparency::status::key_transparency_status;
use serde_json::json;

#[test]
fn status_rejects_nonempty_input_without_reading_persistent_state() {
    let error = key_transparency_status(&json!({"unexpected": true})).unwrap_err();
    assert!(error.to_string().contains("unsupported field"));
}
