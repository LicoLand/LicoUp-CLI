use super::super::ensure::ensure_mobile_relay_key_transparency;
use serde_json::json;

#[test]
fn verifier_orchestration_requires_complete_local_endpoint_material() {
    let error = ensure_mobile_relay_key_transparency(&mut json!({}))
        .err()
        .expect("missing local material must be rejected before authorization");
    assert!(error.to_string().contains("endpoint state is missing"));
}
