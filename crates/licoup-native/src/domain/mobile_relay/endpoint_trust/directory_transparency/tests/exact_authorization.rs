use super::super::authorization::authorize_exact_local_directory_response;
use super::support::{endpoint_config, local_claim};
use crate::core::secure_mesh_directory::DirectoryAuthorizationPurpose;
use serde_json::json;
use time::OffsetDateTime;

#[test]
fn exact_authorization_requires_configured_authority_before_verification() {
    let config = endpoint_config();
    let expected = local_claim(&config);
    let error = authorize_exact_local_directory_response(
        &config,
        json!({}),
        &expected,
        OffsetDateTime::now_utc(),
        DirectoryAuthorizationPurpose::SelfMonitor,
    )
    .err()
    .expect("missing configured authority must be rejected");
    assert!(error.to_string().contains("must be configured"));
}
