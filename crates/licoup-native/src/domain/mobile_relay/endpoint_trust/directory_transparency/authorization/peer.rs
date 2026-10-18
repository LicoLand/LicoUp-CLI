use super::super::verifier::authorize_pairwise_directory_response;
use crate::core::secure_mesh_directory::{AuthorizedDirectoryLeaf, DirectoryAuthorizationPurpose};
use crate::core::secure_mesh_prekey::SecureMeshPairwisePreKeyBundle;
use anyhow::{Result, anyhow};
use serde_json::Value;
use time::OffsetDateTime;

pub(in crate::domain::mobile_relay) fn authorize_peer_pairwise_directory(
    config: &Value,
    descriptor: &Value,
    bundle: &SecureMeshPairwisePreKeyBundle,
    now: OffsetDateTime,
) -> Result<AuthorizedDirectoryLeaf> {
    authorize_peer_pairwise_directory_for_purpose(
        config,
        descriptor,
        bundle,
        now,
        DirectoryAuthorizationPurpose::PairwiseSessionBootstrap,
    )
}

pub(in crate::domain::mobile_relay) fn authorize_peer_pairwise_directory_for_purpose(
    config: &Value,
    descriptor: &Value,
    bundle: &SecureMeshPairwisePreKeyBundle,
    now: OffsetDateTime,
    purpose: DirectoryAuthorizationPurpose,
) -> Result<AuthorizedDirectoryLeaf> {
    let response_value = descriptor
        .get("preKeyBundle")
        .and_then(|value| value.get("keyTransparency"))
        .filter(|value| value.is_object())
        .cloned()
        .ok_or_else(|| anyhow!("mobile relay peer key transparency response is missing"))?;
    authorize_pairwise_directory_response(config, bundle, response_value, now, purpose)
}
