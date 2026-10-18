use crate::domain::mobile_relay::key_transparency::revocation::key_transparency_revocation_request;
use serde_json::json;

#[test]
fn revocation_rejects_unknown_input_before_loading_secret_context() {
    let error = key_transparency_revocation_request(&json!({"unexpected": true})).unwrap_err();
    assert!(error.to_string().contains("unsupported field"));
}
