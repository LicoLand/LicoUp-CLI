use crate::domain::mobile_relay::key_transparency::provision::key_transparency_provision;
use serde_json::json;

#[test]
fn provision_rejects_authority_override_input_before_secret_access() {
    let error = key_transparency_provision(&json!({"pin": {}})).unwrap_err();
    assert!(error.to_string().contains("unsupported field"));
}
