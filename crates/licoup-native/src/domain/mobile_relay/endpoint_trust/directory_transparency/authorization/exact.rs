use super::super::verifier::authorize_exact_directory_response;
use crate::core::secure_mesh_directory::{
    AuthorizedDirectoryLeaf, DirectoryAuthorizationPurpose, SecureMeshDirectoryLeafClaim,
};
use anyhow::Result;
use serde_json::Value;
use time::OffsetDateTime;

pub(in crate::domain::mobile_relay) fn authorize_exact_local_directory_response(
    config: &Value,
    response_value: Value,
    expected_claim: &SecureMeshDirectoryLeafClaim,
    now: OffsetDateTime,
    purpose: DirectoryAuthorizationPurpose,
) -> Result<AuthorizedDirectoryLeaf> {
    authorize_exact_directory_response(config, response_value, expected_claim, now, purpose)
}
