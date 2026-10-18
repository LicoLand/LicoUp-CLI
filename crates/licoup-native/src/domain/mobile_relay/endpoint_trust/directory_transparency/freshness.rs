use super::authority::open_mobile_relay_directory_authority;
use super::config::configured_directory_scope_commitment;
use crate::core::secure_mesh_directory::DirectoryAuthorizationPurpose;
use crate::core::secure_mesh_transparency::{
    SecureMeshKtAuthorizationReceipt, stable_directory_label,
};
use crate::domain::mobile_relay::endpoint_trust::{
    local_public_device_identity, peer_device_identity_from_state,
};
use anyhow::{Result, anyhow, ensure};
use serde_json::Value;

#[derive(Clone, Debug)]
pub(in crate::domain::mobile_relay) struct PairwiseDirectoryFreshness {
    pub(in crate::domain::mobile_relay) tree_size: u64,
    pub(in crate::domain::mobile_relay) expires_at_epoch_seconds: u64,
}

pub(in crate::domain::mobile_relay) fn require_current_pairwise_directory_authority(
    config: &Value,
    now_epoch_seconds: u64,
) -> Result<PairwiseDirectoryFreshness> {
    let local = local_public_device_identity(config)?;
    let peer = peer_device_identity_from_state(
        config
            .get("mobileRelayE2ee")
            .ok_or_else(|| anyhow!("mobile relay E2EE endpoint state is missing"))?,
    )?;
    let scope = configured_directory_scope_commitment(config)?;
    let local_label = stable_directory_label(scope, &local.endpoint_id);
    let peer_label = stable_directory_label(scope, &peer.endpoint_id);
    let mut authority = open_mobile_relay_directory_authority(config, &local.endpoint_id)?;
    let local_monitor = authority.require_current_authorization(
        &local_label,
        DirectoryAuthorizationPurpose::SelfMonitor,
        now_epoch_seconds,
    )?;
    let peer_signed_prekey = authority.require_current_authorization(
        &peer_label,
        DirectoryAuthorizationPurpose::PairwiseSignedPrekey,
        now_epoch_seconds,
    )?;
    let peer_one_time_prekey = authority.require_current_authorization(
        &peer_label,
        DirectoryAuthorizationPurpose::PairwiseOneTimePrekey,
        now_epoch_seconds,
    )?;
    let current = authority
        .latest_checkpoint()?
        .ok_or_else(|| anyhow!("secure mesh KT current checkpoint is unavailable"))?;
    for receipt in [&local_monitor, &peer_signed_prekey, &peer_one_time_prekey] {
        ensure_pairwise_authorization_receipt_current(receipt, current.tree_size)?;
    }
    Ok(PairwiseDirectoryFreshness {
        tree_size: current.tree_size,
        expires_at_epoch_seconds: [
            local_monitor.expires_at_epoch_seconds,
            peer_signed_prekey.expires_at_epoch_seconds,
            peer_one_time_prekey.expires_at_epoch_seconds,
        ]
        .into_iter()
        .min()
        .ok_or_else(|| anyhow!("secure mesh KT freshness receipt is unavailable"))?,
    })
}

pub(in crate::domain::mobile_relay) fn ensure_pairwise_authorization_receipt_current(
    receipt: &SecureMeshKtAuthorizationReceipt,
    current_tree_size: u64,
) -> Result<()> {
    ensure!(
        !receipt.revoked && receipt.tree_size == current_tree_size,
        "secure mesh KT Pairwise authorization requires a current active directory claim"
    );
    Ok(())
}
