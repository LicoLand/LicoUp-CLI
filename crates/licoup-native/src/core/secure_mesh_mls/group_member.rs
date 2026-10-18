use anyhow::{Result, anyhow, ensure};
use openmls::prelude::LeafNodeIndex;
use openmls_traits::OpenMlsProvider;

use crate::core::secure_mesh_mls_pq_epoch::{
    create_mlkem1024_epoch_extension, mlkem1024_epoch_extension_digest, mlkem1024_member_id,
};

use super::capability_extension::{
    SecureMeshMlsCapabilityExtension, secure_mesh_mls_group_context_extensions_with_pq,
};
use super::group_model::{SecureMeshMlsCommit, SecureMeshMlsGroup, SecureMeshMlsWelcome};
use super::group_state::basic_credential_identity;
use super::key_package::SecureMeshMlsKeyPackage;
use super::participant::SecureMeshMlsParticipant;

impl SecureMeshMlsGroup {
    #[cfg(test)]
    pub(crate) fn add_member(
        &mut self,
        owner: &SecureMeshMlsParticipant,
        key_package: &SecureMeshMlsKeyPackage,
    ) -> Result<SecureMeshMlsWelcome> {
        self.add_member_for_runtime_crypto_self_test(owner, key_package)
    }

    pub(super) fn add_member_for_runtime_crypto_self_test(
        &mut self,
        owner: &SecureMeshMlsParticipant,
        key_package: &SecureMeshMlsKeyPackage,
    ) -> Result<SecureMeshMlsWelcome> {
        ensure!(
            matches!(
                self.capability_extension()?,
                SecureMeshMlsCapabilityExtension::AwaitingMemberNegotiation { .. }
            ),
            "secure mesh MLS active group member add requires capability-negotiated product path"
        );
        let (commit_message, welcome_message, _group_info) = self
            .group
            .add_members(
                &owner.provider,
                &owner.signer,
                core::slice::from_ref(&key_package.public_key_package),
            )
            .map_err(|error| anyhow!("secure mesh MLS add member failed: {error:?}"))?;
        self.group
            .merge_pending_commit(&owner.provider)
            .map_err(|error| anyhow!("secure mesh MLS pending commit merge failed: {error:?}"))?;
        self.refresh_authenticated_group_context(owner)?;
        Ok(SecureMeshMlsWelcome {
            commit_message: commit_message.to_bytes().map_err(|error| {
                anyhow!("secure mesh MLS commit serialization failed: {error:?}")
            })?,
            welcome_message: welcome_message.to_bytes().map_err(|error| {
                anyhow!("secure mesh MLS welcome serialization failed: {error:?}")
            })?,
        })
    }

    pub(crate) fn add_member_with_capability_extension(
        &mut self,
        owner: &SecureMeshMlsParticipant,
        key_package: &SecureMeshMlsKeyPackage,
        capability_extension: &SecureMeshMlsCapabilityExtension,
    ) -> Result<SecureMeshMlsWelcome> {
        capability_extension.require_active()?;
        let mut recipient_public_keys = self.current_mlkem1024_recipient_public_keys(owner)?;
        let added_member_id = mlkem1024_member_id(&key_package.credential_identity_bytes()?)?;
        ensure!(
            recipient_public_keys
                .insert(added_member_id, key_package.mlkem1024_public_key().to_vec())
                .is_none(),
            "secure mesh MLS ML-KEM-1024 member already exists"
        );
        let previous_epoch_digest = self
            .mlkem1024_epoch_extension
            .as_ref()
            .map(mlkem1024_epoch_extension_digest)
            .transpose()?;
        let (pq_epoch_extension, _) = create_mlkem1024_epoch_extension(
            self.group.group_id().as_slice(),
            self.epoch().saturating_add(1),
            previous_epoch_digest,
            &recipient_public_keys,
        )?;
        let commit = self
            .group
            .commit_builder()
            .propose_adds(Some(key_package.public_key_package.clone()))
            .propose_group_context_extensions(secure_mesh_mls_group_context_extensions_with_pq(
                capability_extension,
                Some(&pq_epoch_extension),
            )?)
            .map_err(|error| {
                anyhow!("secure mesh MLS capability extension proposal failed: {error:?}")
            })?
            .load_psks(owner.provider.storage())
            .map_err(|error| anyhow!("secure mesh MLS PSK load failed: {error:?}"))?
            .build(
                owner.provider.rand(),
                owner.provider.crypto(),
                &owner.signer,
                |_| true,
            )
            .map_err(|error| anyhow!("secure mesh MLS capability commit build failed: {error:?}"))?
            .stage_commit(&owner.provider)
            .map_err(|error| {
                anyhow!("secure mesh MLS capability commit staging failed: {error:?}")
            })?;
        let commit_message = commit
            .commit()
            .to_bytes()
            .map_err(|error| anyhow!("secure mesh MLS commit serialization failed: {error:?}"))?;
        let welcome_message = commit
            .to_welcome_msg()
            .ok_or_else(|| anyhow!("secure mesh MLS capability commit welcome is missing"))?
            .to_bytes()
            .map_err(|error| anyhow!("secure mesh MLS welcome serialization failed: {error:?}"))?;
        self.group
            .merge_pending_commit(&owner.provider)
            .map_err(|error| anyhow!("secure mesh MLS pending commit merge failed: {error:?}"))?;
        self.refresh_authenticated_group_context(owner)?;
        ensure!(
            self.capability_extension()? == *capability_extension,
            "secure mesh MLS committed capability extension mismatch"
        );
        ensure!(
            self.mlkem1024_epoch_extension.as_ref() == Some(&pq_epoch_extension),
            "secure mesh MLS committed ML-KEM-1024 epoch extension mismatch"
        );
        Ok(SecureMeshMlsWelcome {
            commit_message,
            welcome_message,
        })
    }

    pub(crate) fn remove_member_with_capability_extension(
        &mut self,
        remover: &SecureMeshMlsParticipant,
        removed: LeafNodeIndex,
        capability_extension: &SecureMeshMlsCapabilityExtension,
    ) -> Result<SecureMeshMlsCommit> {
        capability_extension.require_active()?;
        ensure!(
            removed != self.own_leaf_index(),
            "secure mesh MLS member-remove action cannot remove its own leaf"
        );
        let removed_identity = self
            .group
            .members()
            .find(|member| member.index == removed)
            .ok_or_else(|| anyhow!("secure mesh MLS removed member is missing"))
            .and_then(|member| basic_credential_identity(&member.credential))?;
        let mut recipient_public_keys = self.current_mlkem1024_recipient_public_keys(remover)?;
        let removed_member_id = mlkem1024_member_id(&removed_identity)?;
        ensure!(
            recipient_public_keys.remove(&removed_member_id).is_some(),
            "secure mesh MLS removed member ML-KEM-1024 key is missing"
        );
        let previous_epoch_digest = self
            .mlkem1024_epoch_extension
            .as_ref()
            .map(mlkem1024_epoch_extension_digest)
            .transpose()?;
        let (pq_epoch_extension, _) = create_mlkem1024_epoch_extension(
            self.group.group_id().as_slice(),
            self.epoch().saturating_add(1),
            previous_epoch_digest,
            &recipient_public_keys,
        )?;
        let commit = self
            .group
            .commit_builder()
            .propose_removals([removed])
            .propose_group_context_extensions(secure_mesh_mls_group_context_extensions_with_pq(
                capability_extension,
                Some(&pq_epoch_extension),
            )?)
            .map_err(|error| {
                anyhow!("secure mesh MLS remove capability proposal failed: {error:?}")
            })?
            .load_psks(remover.provider.storage())
            .map_err(|error| anyhow!("secure mesh MLS remove PSK load failed: {error:?}"))?
            .build(
                remover.provider.rand(),
                remover.provider.crypto(),
                &remover.signer,
                |_| true,
            )
            .map_err(|error| anyhow!("secure mesh MLS remove commit build failed: {error:?}"))?
            .stage_commit(&remover.provider)
            .map_err(|error| anyhow!("secure mesh MLS remove commit staging failed: {error:?}"))?;
        let commit_message = commit.commit().to_bytes().map_err(|error| {
            anyhow!("secure mesh MLS remove commit serialization failed: {error:?}")
        })?;
        ensure!(
            commit.to_welcome_msg().is_none(),
            "secure mesh MLS remove commit unexpectedly produced a welcome"
        );
        self.group
            .merge_pending_commit(&remover.provider)
            .map_err(|error| {
                anyhow!("secure mesh MLS remove member pending commit merge failed: {error:?}")
            })?;
        self.refresh_authenticated_group_context(remover)?;
        ensure!(
            self.capability_extension()? == *capability_extension,
            "secure mesh MLS removed-member capability extension mismatch"
        );
        ensure!(
            self.mlkem1024_epoch_extension.as_ref() == Some(&pq_epoch_extension),
            "secure mesh MLS removed-member ML-KEM-1024 epoch extension mismatch"
        );
        Ok(SecureMeshMlsCommit {
            commit_message,
            welcome_message: None,
        })
    }
}
