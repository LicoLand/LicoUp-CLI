use super::constants::{MAX_EPOCH_LAG, MAX_ROSTER};
use super::helpers::{endpoint_id_from_credential_identity, hex_sha256};
use super::identity_trust::{mls_credential_identity_bytes, require_verified_member_trust};
use anyhow::{Result, anyhow, ensure};
use std::collections::{BTreeMap, BTreeSet};

use crate::core::secure_mesh_directory::{AuthorizedDirectoryLeaf, DirectoryAuthorizationPurpose};
use crate::core::secure_mesh_mls::SecureMeshMlsKeyPackage;
use crate::core::secure_mesh_trust::{DeviceTrustPublicIdentity, DeviceTrustState};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshMlsExpectedInvitation {
    pub group_id: Vec<u8>,
    pub inviter_endpoint_id: String,
    pub expected_roster_endpoint_ids: BTreeSet<String>,
}

impl SecureMeshMlsExpectedInvitation {
    pub fn new(
        group_id: impl AsRef<[u8]>,
        inviter_endpoint_id: impl Into<String>,
        expected_roster_endpoint_ids: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self> {
        let inviter_endpoint_id = inviter_endpoint_id.into();
        ensure!(
            !inviter_endpoint_id.trim().is_empty(),
            "secure mesh MLS inviter endpoint id is required"
        );
        let expected_roster_endpoint_ids = expected_roster_endpoint_ids
            .into_iter()
            .map(Into::into)
            .collect::<BTreeSet<_>>();
        ensure!(
            !expected_roster_endpoint_ids.is_empty(),
            "secure mesh MLS expected roster is required"
        );
        ensure!(
            expected_roster_endpoint_ids.len() <= MAX_ROSTER,
            "secure mesh MLS expected roster is too large"
        );
        ensure!(
            expected_roster_endpoint_ids.contains(&inviter_endpoint_id),
            "secure mesh MLS inviter must be in the expected roster"
        );
        Ok(Self {
            group_id: group_id.as_ref().to_vec(),
            inviter_endpoint_id,
            expected_roster_endpoint_ids,
        })
    }
}

pub fn authorize_welcome_acceptance(
    invitation: &SecureMeshMlsExpectedInvitation,
    inviter_trust_state: &DeviceTrustState,
    welcome_group_id: &[u8],
) -> Result<()> {
    require_verified_member_trust(inviter_trust_state)?;
    ensure!(
        invitation.group_id == welcome_group_id,
        "secure mesh MLS welcome group id mismatch"
    );
    Ok(())
}

pub fn authorize_commit_sender(
    sender_endpoint_id: &str,
    sender_trust_state: &DeviceTrustState,
    roster_endpoint_ids: &BTreeSet<String>,
) -> Result<()> {
    require_verified_member_trust(sender_trust_state)?;
    ensure!(
        roster_endpoint_ids.contains(sender_endpoint_id),
        "secure mesh MLS commit sender is not in the verified roster"
    );
    Ok(())
}

pub fn cross_check_roster(
    expected_roster_endpoint_ids: &BTreeSet<String>,
    observed_credential_identities: &[Vec<u8>],
    trusted_identities: &BTreeMap<String, DeviceTrustPublicIdentity>,
) -> Result<()> {
    ensure!(
        expected_roster_endpoint_ids.len() == observed_credential_identities.len(),
        "secure mesh MLS roster size divergence"
    );
    let mut observed_endpoints = BTreeSet::new();
    for credential in observed_credential_identities {
        let endpoint_id = endpoint_id_from_credential_identity(credential)?;
        let trusted = trusted_identities.get(&endpoint_id).ok_or_else(|| {
            anyhow!("secure mesh MLS roster member lacks a trusted identity binding")
        })?;
        let expected = mls_credential_identity_bytes(trusted)?;
        ensure!(
            &expected == credential,
            "secure mesh MLS roster credential does not match trusted identity"
        );
        observed_endpoints.insert(endpoint_id);
    }
    ensure!(
        &observed_endpoints == expected_roster_endpoint_ids,
        "secure mesh MLS roster endpoint set divergence"
    );
    Ok(())
}

pub fn authorize_sender_endpoint_binding(
    context_sender_endpoint_id: &str,
    trusted_sender_endpoint_id: &str,
) -> Result<()> {
    ensure!(
        context_sender_endpoint_id == trusted_sender_endpoint_id,
        "secure mesh MLS forged sender endpoint rejected"
    );
    Ok(())
}

pub fn authorize_epoch_lag(current_epoch: u64, message_epoch: u64) -> Result<()> {
    ensure!(
        message_epoch <= current_epoch,
        "secure mesh MLS message epoch is from the future"
    );
    let lag = current_epoch.saturating_sub(message_epoch);
    ensure!(
        lag <= MAX_EPOCH_LAG,
        "secure mesh MLS epoch lag exceeds acceptance window; rejoin required"
    );
    Ok(())
}

pub fn authorize_member_add_with_directory(
    authorization: &AuthorizedDirectoryLeaf,
    member_identity: &DeviceTrustPublicIdentity,
    member_key_package: &SecureMeshMlsKeyPackage,
    member_directory_version: u64,
    member_key_package_version: u64,
) -> Result<()> {
    authorization.require_purpose(DirectoryAuthorizationPurpose::MlsMemberAdd)?;
    authorization.require_device_identity(member_identity)?;
    ensure!(
        authorization.claim().version() == member_directory_version,
        "secure mesh MLS directory publication version mismatch"
    );
    authorization.require_mls_key_package_digest(
        &hex_sha256(member_key_package.as_public_bytes()),
        member_key_package_version,
    )?;
    Ok(())
}
