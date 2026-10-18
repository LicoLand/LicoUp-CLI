use std::collections::BTreeMap;

use anyhow::{Result, anyhow, ensure};
use serde_json::Value;
use time::OffsetDateTime;

use crate::core::secure_mesh_directory::{
    AuthorizedDirectoryLeaf, DirectoryAuthorizationPurpose, DirectoryAuthorizationRequest,
    SecureMeshDirectoryAuthority, SecureMeshKtVerifierConfiguration, UntrustedDirectoryResponse,
};
use crate::core::secure_mesh_transparency::{
    KtFreshnessPolicy, SecureMeshKtAuthorizationReceipt, stable_directory_label,
};
use crate::core::secure_mesh_trust::DeviceTrustPublicIdentity;

pub(super) struct MlsDirectoryReadiness {
    pub(super) tree_size: u64,
    pub(super) root_hash: String,
    pub(super) map_root_hash: String,
    pub(super) receipt_count: usize,
}

fn open_mls_directory_authority(
    config: &Value,
    local_identity: &DeviceTrustPublicIdentity,
) -> Result<SecureMeshDirectoryAuthority> {
    crate::domain::mobile_relay::ensure_secure_mesh_protected_operation_allowed()?;
    let local_configuration = config
        .get("secureMeshKeyTransparency")
        .ok_or_else(|| anyhow!("secure mesh MLS local KT pin configuration is required"))?;
    let local_configuration: SecureMeshKtVerifierConfiguration =
        serde_json::from_value(local_configuration.clone())
            .map_err(|_| anyhow!("secure mesh MLS local KT pin configuration is invalid"))?;
    let policy = KtFreshnessPolicy::strict(
        local_configuration.max_sth_age_seconds,
        local_configuration.max_future_skew_seconds,
    )?;
    let pin = local_configuration.pin.into_pin()?;
    let state_path =
        crate::domain::mobile_relay::secure_mesh_kt_authority_path(&local_identity.endpoint_id)?;
    SecureMeshDirectoryAuthority::open(state_path, pin, policy)
}

pub(super) fn require_mls_directory_authority(
    config: &Value,
    local_identity: &DeviceTrustPublicIdentity,
    identities: &BTreeMap<String, DeviceTrustPublicIdentity>,
) -> Result<MlsDirectoryReadiness> {
    require_mls_directory_authority_with_local_policy(config, local_identity, identities, true)
}

pub(super) fn require_mls_directory_authority_with_local_policy(
    config: &Value,
    local_identity: &DeviceTrustPublicIdentity,
    identities: &BTreeMap<String, DeviceTrustPublicIdentity>,
    require_local_member: bool,
) -> Result<MlsDirectoryReadiness> {
    if require_local_member {
        ensure!(
            identities.get(&local_identity.endpoint_id) == Some(local_identity),
            "secure mesh MLS directory roster is missing the exact local identity"
        );
    } else {
        ensure!(
            !identities.contains_key(&local_identity.endpoint_id),
            "secure mesh MLS post-removal directory roster still contains the local identity"
        );
    }
    let scope = config
        .get("secureMeshDirectoryScopeCommitment")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("secure mesh MLS configured directory scope is required"))?;
    let now_epoch_seconds = u64::try_from(OffsetDateTime::now_utc().unix_timestamp())
        .map_err(|_| anyhow!("secure mesh MLS KT verification time is invalid"))?;
    let mut authority = open_mls_directory_authority(config, local_identity)?;
    let mut checkpoint: Option<(u64, String, String)> = None;
    let mut receipt_count = 0usize;
    for identity in identities.values() {
        let stable_label = stable_directory_label(scope, &identity.endpoint_id);
        let purposes: &[DirectoryAuthorizationPurpose] = if identity == local_identity {
            &[DirectoryAuthorizationPurpose::SelfMonitor]
        } else {
            &[
                DirectoryAuthorizationPurpose::MlsKeyPackage,
                DirectoryAuthorizationPurpose::MlsMemberAdd,
            ]
        };
        let mut identity_receipts = Vec::with_capacity(purposes.len());
        for purpose in purposes {
            let receipt = authority.require_current_authorization(
                &stable_label,
                *purpose,
                now_epoch_seconds,
            )?;
            ensure_mls_receipt_identity_binding(&receipt, identity)?;
            let binding = (
                receipt.tree_size,
                receipt.root_hash.clone(),
                receipt.map_root_hash.clone(),
            );
            if let Some(expected) = &checkpoint {
                ensure!(
                    expected == &binding,
                    "secure mesh MLS KT receipts do not share one current tree"
                );
            } else {
                checkpoint = Some(binding);
            }
            receipt_count = receipt_count.saturating_add(1);
            identity_receipts.push(receipt);
        }
        if identity == local_identity {
            if let (Some(digest), Some(version)) = (
                config
                    .get("mobileRelayE2ee")
                    .and_then(|state| state.get("mlsKeyPackageDigest"))
                    .and_then(Value::as_str),
                config
                    .get("mobileRelayE2ee")
                    .and_then(|state| state.get("mlsKeyPackageVersion"))
                    .and_then(Value::as_u64)
                    .filter(|version| *version > 0),
            ) {
                let self_monitor = &identity_receipts[0];
                ensure!(
                    self_monitor.mls_key_package_digest == digest
                        && self_monitor.mls_key_package_version == version,
                    "secure mesh MLS local KT receipt does not bind the current KeyPackage"
                );
                let key_package_receipt = authority.require_current_authorization(
                    &stable_label,
                    DirectoryAuthorizationPurpose::MlsKeyPackage,
                    now_epoch_seconds,
                )?;
                ensure_mls_receipt_identity_binding(&key_package_receipt, identity)?;
                ensure!(
                    key_package_receipt.mls_key_package_digest == digest
                        && key_package_receipt.mls_key_package_version == version,
                    "secure mesh MLS local KeyPackage receipt differs from local state"
                );
                ensure!(
                    checkpoint.as_ref()
                        == Some(&(
                            key_package_receipt.tree_size,
                            key_package_receipt.root_hash.clone(),
                            key_package_receipt.map_root_hash.clone(),
                        )),
                    "secure mesh MLS local KeyPackage receipt is not on the current tree"
                );
                receipt_count = receipt_count.saturating_add(1);
            }
        } else {
            let key_package = &identity_receipts[0];
            let member_add = &identity_receipts[1];
            ensure!(
                key_package.directory_version == member_add.directory_version
                    && key_package.mls_key_package_version == member_add.mls_key_package_version
                    && key_package.mls_key_package_digest == member_add.mls_key_package_digest
                    && member_add.mls_key_package_version > 0
                    && member_add.mls_key_package_digest != "0".repeat(64),
                "secure mesh MLS remote KT receipts do not bind one real KeyPackage publication"
            );
        }
    }
    let (tree_size, root_hash, map_root_hash) =
        checkpoint.ok_or_else(|| anyhow!("secure mesh MLS current KT receipt set is empty"))?;
    Ok(MlsDirectoryReadiness {
        tree_size,
        root_hash,
        map_root_hash,
        receipt_count,
    })
}

fn ensure_mls_receipt_identity_binding(
    receipt: &SecureMeshKtAuthorizationReceipt,
    identity: &DeviceTrustPublicIdentity,
) -> Result<()> {
    ensure!(
        !receipt.revoked
            && receipt.identity_fingerprint == identity.fingerprint()?
            && receipt.identity_rotation_epoch == identity.rotation_epoch,
        "secure mesh MLS KT receipt identity is revoked or differs from the roster"
    );
    Ok(())
}

#[cfg(test)]
pub(super) fn authorize_member_add_directory_response(
    config: &Value,
    local_identity: &DeviceTrustPublicIdentity,
    response: UntrustedDirectoryResponse,
    now: OffsetDateTime,
) -> Result<AuthorizedDirectoryLeaf> {
    let local_configuration = config
        .get("secureMeshKeyTransparency")
        .ok_or_else(|| anyhow!("secure mesh MLS local KT pin configuration is required"))?;
    let local_configuration: SecureMeshKtVerifierConfiguration =
        serde_json::from_value(local_configuration.clone())
            .map_err(|_| anyhow!("secure mesh MLS local KT pin configuration is invalid"))?;
    let policy = KtFreshnessPolicy::strict(
        local_configuration.max_sth_age_seconds,
        local_configuration.max_future_skew_seconds,
    )?;
    let pin = local_configuration.pin.into_pin()?;
    let state_path =
        crate::domain::mobile_relay::secure_mesh_kt_authority_path(&local_identity.endpoint_id)?;
    let mut authority = SecureMeshDirectoryAuthority::open(state_path, pin, policy)?;
    let now_epoch_seconds = u64::try_from(now.unix_timestamp())
        .map_err(|_| anyhow!("secure mesh MLS KT verification time is invalid"))?;
    authority.authorize(
        response,
        DirectoryAuthorizationPurpose::MlsMemberAdd,
        now_epoch_seconds,
    )
}

pub(super) fn authorize_member_directory_response(
    config: &Value,
    local_identity: &DeviceTrustPublicIdentity,
    response: UntrustedDirectoryResponse,
    now: OffsetDateTime,
    purpose: DirectoryAuthorizationPurpose,
    member_identity: &DeviceTrustPublicIdentity,
    member_directory_version: u64,
    member_key_package_digest: &str,
    member_key_package_version: u64,
) -> Result<AuthorizedDirectoryLeaf> {
    let local_configuration = config
        .get("secureMeshKeyTransparency")
        .ok_or_else(|| anyhow!("secure mesh MLS local KT pin configuration is required"))?;
    let local_configuration: SecureMeshKtVerifierConfiguration =
        serde_json::from_value(local_configuration.clone())
            .map_err(|_| anyhow!("secure mesh MLS local KT pin configuration is invalid"))?;
    let policy = KtFreshnessPolicy::strict(
        local_configuration.max_sth_age_seconds,
        local_configuration.max_future_skew_seconds,
    )?;
    let pin = local_configuration.pin.into_pin()?;
    let state_path =
        crate::domain::mobile_relay::secure_mesh_kt_authority_path(&local_identity.endpoint_id)?;
    let mut authority = SecureMeshDirectoryAuthority::open(state_path, pin, policy)?;
    let now_epoch_seconds = u64::try_from(now.unix_timestamp())
        .map_err(|_| anyhow!("secure mesh MLS KT verification time is invalid"))?;
    #[cfg(test)]
    if config
        .get("secureMeshKeyTransparency")
        .and_then(|settings| settings.get("pin"))
        .and_then(|pin| pin.get("provenance"))
        .and_then(Value::as_str)
        == Some("local-acceptance-mock")
    {
        authority.observe_response_gossip_for_test(&response, now_epoch_seconds)?;
    }
    authority.authorize_request(
        response,
        DirectoryAuthorizationRequest::for_mls(
            purpose,
            config
                .get("secureMeshDirectoryScopeCommitment")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("secure mesh MLS configured directory scope is required"))?,
            member_identity,
            member_directory_version,
            member_key_package_digest,
            member_key_package_version,
        ),
        now_epoch_seconds,
    )
}
