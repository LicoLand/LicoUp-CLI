use super::authority::open_mobile_relay_directory_authority;
use super::clock::epoch_seconds;
use super::config::configured_directory_scope_commitment;
#[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
use super::test_support::{
    refresh_mobile_relay_test_directory_response, uses_local_acceptance_mock,
};
use crate::core::secure_mesh_directory::{
    AuthorizedDirectoryLeaf, DirectoryAuthorizationPurpose, DirectoryAuthorizationRequest,
    SecureMeshDirectoryAuthority, SecureMeshDirectoryLeafClaim, UntrustedDirectoryResponse,
};
use crate::core::secure_mesh_prekey::{
    SecureMeshPairwisePreKeyBundle, one_time_prekey_batch_digest, signed_prekey_bundle_digest,
};
use crate::domain::mobile_relay::endpoint_trust::descriptor_text;
use anyhow::{Result, anyhow};
use serde_json::Value;
use time::OffsetDateTime;

struct PreparedDirectoryResponse {
    authority: SecureMeshDirectoryAuthority,
    response: UntrustedDirectoryResponse,
    now_epoch_seconds: u64,
}

fn prepare_directory_response(
    config: &Value,
    response_value: Value,
    now_epoch_seconds: u64,
) -> Result<PreparedDirectoryResponse> {
    let local_endpoint_id = descriptor_text(
        config
            .get("mobileRelayE2ee")
            .ok_or_else(|| anyhow!("mobile relay local endpoint state is missing"))?,
        "endpointId",
    )?;
    let authority = open_mobile_relay_directory_authority(config, &local_endpoint_id)?;
    #[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
    let mut authority = authority;
    #[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
    let response_value = if uses_local_acceptance_mock(config) {
        refresh_mobile_relay_test_directory_response(
            response_value,
            authority
                .latest_checkpoint()?
                .map(|checkpoint| checkpoint.tree_size),
            now_epoch_seconds,
        )?
    } else {
        response_value
    };
    let response: UntrustedDirectoryResponse = serde_json::from_value(response_value)
        .map_err(|_| anyhow!("mobile relay key transparency response is invalid"))?;
    #[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
    if uses_local_acceptance_mock(config) {
        authority.observe_response_gossip_for_test(&response, now_epoch_seconds)?;
    }
    Ok(PreparedDirectoryResponse {
        authority,
        response,
        now_epoch_seconds,
    })
}

pub(super) fn authorize_pairwise_directory_response(
    config: &Value,
    bundle: &SecureMeshPairwisePreKeyBundle,
    response_value: Value,
    now: OffsetDateTime,
    purpose: DirectoryAuthorizationPurpose,
) -> Result<AuthorizedDirectoryLeaf> {
    let PreparedDirectoryResponse {
        mut authority,
        response,
        now_epoch_seconds,
    } = prepare_directory_response(config, response_value, epoch_seconds(now)?)?;
    let signed_prekey_digest = signed_prekey_bundle_digest(bundle)?;
    let one_time_prekey_digest = one_time_prekey_batch_digest(bundle)?;
    authority.authorize_request(
        response,
        DirectoryAuthorizationRequest::for_pairwise(
            purpose,
            configured_directory_scope_commitment(config)?,
            &bundle.endpoint_identity,
            &signed_prekey_digest,
            &one_time_prekey_digest,
            bundle.prekey_publication_version,
        ),
        now_epoch_seconds,
    )
}

pub(super) fn authorize_exact_directory_response(
    config: &Value,
    response_value: Value,
    expected_claim: &SecureMeshDirectoryLeafClaim,
    now: OffsetDateTime,
    purpose: DirectoryAuthorizationPurpose,
) -> Result<AuthorizedDirectoryLeaf> {
    let PreparedDirectoryResponse {
        mut authority,
        response,
        now_epoch_seconds,
    } = prepare_directory_response(config, response_value, epoch_seconds(now)?)?;
    authority.authorize_request(
        response,
        DirectoryAuthorizationRequest::for_exact_claim(
            purpose,
            configured_directory_scope_commitment(config)?,
            expected_claim,
        ),
        now_epoch_seconds,
    )
}

pub(super) fn authorize_mls_directory_response(
    config: &Value,
    bundle: &SecureMeshPairwisePreKeyBundle,
    response_value: Value,
    now_epoch_seconds: u64,
    key_package_digest: &str,
    key_package_version: u64,
) -> Result<AuthorizedDirectoryLeaf> {
    let PreparedDirectoryResponse {
        mut authority,
        response,
        now_epoch_seconds,
    } = prepare_directory_response(config, response_value, now_epoch_seconds)?;
    let directory_version = response.claim.directory_version;
    authority.authorize_request(
        response,
        DirectoryAuthorizationRequest::for_mls(
            DirectoryAuthorizationPurpose::MlsKeyPackage,
            configured_directory_scope_commitment(config)?,
            &bundle.endpoint_identity,
            directory_version,
            key_package_digest,
            key_package_version,
        ),
        now_epoch_seconds,
    )
}
