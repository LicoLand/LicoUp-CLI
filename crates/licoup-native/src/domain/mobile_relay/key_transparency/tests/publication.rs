use crate::domain::mobile_relay::key_transparency::publication::key_transparency_publication_request;
use serde_json::json;

#[test]
fn publication_rejects_unknown_input_before_loading_secret_context() {
    let error = key_transparency_publication_request(&json!({"unexpected": true})).unwrap_err();
    assert!(error.to_string().contains("unsupported field"));
}
