use super::super::authority::open_mobile_relay_directory_authority;
use serde_json::json;

#[test]
fn authority_open_requires_explicit_pinned_verifier_configuration() {
    let error = open_mobile_relay_directory_authority(&json!({}), "endpoint")
        .err()
        .expect("missing verifier configuration must be rejected");
    assert!(error.to_string().contains("must be configured"));
}
