use super::super::verifier::authorize_pairwise_directory_response;
use crate::core::secure_mesh_directory::{AuthorizedDirectoryLeaf, DirectoryAuthorizationPurpose};
use crate::domain::mobile_relay::endpoint_trust::LocalEndpointState;
use anyhow::{Result, anyhow};
use serde_json::Value;
use time::OffsetDateTime;

pub(in crate::domain::mobile_relay) fn authorize_local_pairwise_directory(
    config: &Value,
    endpoint: &LocalEndpointState,
    now: OffsetDateTime,
) -> Result<AuthorizedDirectoryLeaf> {
    authorize_pairwise_directory_response(
        config,
        &endpoint.pairwise_prekey_bundle()?,
        endpoint
            .key_transparency_response
            .clone()
            .ok_or_else(|| anyhow!("mobile relay key transparency response is missing"))?,
        now,
        DirectoryAuthorizationPurpose::PairwiseSessionBootstrap,
    )
}
