use super::super::test_support::uses_local_acceptance_mock;
use serde_json::json;

#[test]
fn acceptance_mock_detection_requires_exact_pin_provenance() {
    assert!(uses_local_acceptance_mock(&json!({
        "secureMeshKeyTransparency": {"pin": {"provenance": "local-acceptance-mock"}}
    })));
    assert!(!uses_local_acceptance_mock(&json!({
        "secureMeshKeyTransparency": {"pin": {"provenance": "user-configured-external"}}
    })));
}
