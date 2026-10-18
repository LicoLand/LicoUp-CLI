use std::collections::BTreeSet;
use std::panic::{AssertUnwindSafe, catch_unwind};

use anyhow::{Result, anyhow, ensure};
#[cfg(test)]
use openmls::prelude::{Extensions, LeafNodeParameters};
use openmls::prelude::{LeafNodeIndex, ProcessedMessageContent};

#[cfg(test)]
use crate::core::secure_mesh_mls_pq_epoch::create_mlkem1024_epoch_extension;
use crate::core::secure_mesh_mls_pq_epoch::{
    mlkem1024_epoch_extension_digest, mlkem1024_member_id, open_mlkem1024_epoch_extension,
};

#[cfg(test)]
use super::capability_extension::secure_mesh_mls_group_context_extensions_with_pq;
use super::capability_extension::{
    SecureMeshMlsCapabilityExtension, decode_secure_mesh_mls_capability_extension,
    decode_secure_mesh_mls_pq_epoch_extension, secure_mesh_mls_capability_extension_digest,
};
use super::codec::deserialize_protocol_message;
use super::group_model::SecureMeshMlsGroup;
use super::group_state::basic_credential_identity;
use super::participant::SecureMeshMlsParticipant;

impl SecureMeshMlsGroup {
    #[cfg(test)]
    pub(crate) fn stage_test_stripped_capability_extension_commit(
        &mut self,
        participant: &SecureMeshMlsParticipant,
    ) -> Result<Vec<u8>> {
        let (commit, _, _) = self
            .group
            .update_group_context_extensions(
                &participant.provider,
                Extensions::empty(),
                &participant.signer,
            )
            .map_err(|error| {
                anyhow!("secure mesh MLS stripped extension test commit failed: {error:?}")
            })?;
        commit.to_bytes().map_err(|error| {
            anyhow!("secure mesh MLS stripped extension test serialization failed: {error:?}")
        })
    }

    #[cfg(test)]
    pub(crate) fn self_update(
        &mut self,
        participant: &SecureMeshMlsParticipant,
    ) -> Result<Vec<u8>> {
        let capability_extension = self.capability_extension()?;
        let commit = if capability_extension.require_active().is_ok() {
            let recipient_public_keys =
                self.current_mlkem1024_recipient_public_keys(participant)?;
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
            self.group
                .update_group_context_extensions(
                    &participant.provider,
                    secure_mesh_mls_group_context_extensions_with_pq(
                        &capability_extension,
                        Some(&pq_epoch_extension),
                    )?,
                    &participant.signer,
                )
                .map_err(|error| anyhow!("secure mesh MLS hybrid epoch update failed: {error:?}"))?
                .0
        } else {
            self.group
                .self_update(
                    &participant.provider,
                    &participant.signer,
                    LeafNodeParameters::default(),
                )
                .map_err(|error| anyhow!("secure mesh MLS self update failed: {error:?}"))?
                .into_commit()
        };
        self.group
            .merge_pending_commit(&participant.provider)
            .map_err(|error| {
                anyhow!("secure mesh MLS self update pending commit merge failed: {error:?}")
            })?;
        self.refresh_authenticated_group_context(participant)?;
        commit
            .to_bytes()
            .map_err(|error| anyhow!("secure mesh MLS self update serialization failed: {error:?}"))
    }

    #[cfg(test)]
    pub(crate) fn process_commit(
        &mut self,
        participant: &SecureMeshMlsParticipant,
        commit_message: &[u8],
    ) -> Result<()> {
        self.process_commit_with_capability_verifier(
            participant,
            commit_message,
            false,
            |_, _, _| Ok(()),
            |_, _, _, _| Ok(()),
        )
    }

    pub(crate) fn process_commit_with_capability_verifier(
        &mut self,
        participant: &SecureMeshMlsParticipant,
        commit_message: &[u8],
        allow_capability_update: bool,
        verify_sender: impl FnOnce(&[u8], &[u8], LeafNodeIndex) -> Result<()>,
        verifier: impl FnOnce(
            &SecureMeshMlsCapabilityExtension,
            &SecureMeshMlsCapabilityExtension,
            &[LeafNodeIndex],
            usize,
        ) -> Result<()>,
    ) -> Result<()> {
        let current_extension = self.capability_extension()?;
        let protocol_message = deserialize_protocol_message(
            commit_message,
            "secure mesh MLS commit deserialization failed",
        )?;
        let processed = catch_unwind(AssertUnwindSafe(|| {
            self.group
                .process_message(&participant.provider, protocol_message)
        }))
        .map_err(|_| anyhow!("secure mesh MLS commit rejected"))?
        .map_err(|error| anyhow!("secure mesh MLS commit process failed: {error:?}"))?;
        let (credential_identity, signing_public_key, leaf_index) =
            self.authenticated_member_sender(&processed)?;
        verify_sender(&credential_identity, &signing_public_key, leaf_index)?;
        match processed.into_content() {
            ProcessedMessageContent::StagedCommitMessage(staged_commit) => {
                let staged_extension = decode_secure_mesh_mls_capability_extension(
                    staged_commit.group_context().extensions(),
                )?;
                if staged_extension != current_extension {
                    ensure!(
                        allow_capability_update,
                        "secure mesh MLS capability extension update requires product verification"
                    );
                    let SecureMeshMlsCapabilityExtension::Active {
                        previous_extension_digest,
                        group_negotiated_protocol_capabilities: staged_group_capabilities,
                        ..
                    } = &staged_extension
                    else {
                        return Err(anyhow!(
                            "secure mesh MLS capability extension downgrade rejected"
                        ));
                    };
                    let expected_previous_digest =
                        secure_mesh_mls_capability_extension_digest(&current_extension)?;
                    ensure!(
                        previous_extension_digest.as_deref()
                            == Some(expected_previous_digest.as_str()),
                        "secure mesh MLS capability extension continuity failed"
                    );
                    let _ = staged_group_capabilities;
                }
                let removed_leaf_indices = staged_commit
                    .remove_proposals()
                    .map(|proposal| proposal.remove_proposal().removed())
                    .collect::<Vec<_>>();
                let added_member_count = staged_commit.add_proposals().count();
                if matches!(
                    staged_extension,
                    SecureMeshMlsCapabilityExtension::Active { .. }
                ) {
                    let staged_pq_extension = decode_secure_mesh_mls_pq_epoch_extension(
                        staged_commit.group_context().extensions(),
                    )?;
                    ensure!(
                        staged_pq_extension.epoch == self.epoch().saturating_add(1),
                        "secure mesh MLS ML-KEM-1024 epoch did not advance"
                    );
                    let expected_previous_pq_digest = self
                        .mlkem1024_epoch_extension
                        .as_ref()
                        .map(mlkem1024_epoch_extension_digest)
                        .transpose()?;
                    ensure!(
                        staged_pq_extension.previous_epoch_digest == expected_previous_pq_digest,
                        "secure mesh MLS ML-KEM-1024 epoch continuity failed"
                    );
                    let expected_member_count = self
                        .member_count()
                        .checked_add(added_member_count)
                        .and_then(|count| count.checked_sub(removed_leaf_indices.len()))
                        .ok_or_else(|| anyhow!("secure mesh MLS staged roster size is invalid"))?;
                    ensure!(
                        staged_pq_extension.recipients.len() == expected_member_count,
                        "secure mesh MLS ML-KEM-1024 recipient count differs from staged roster"
                    );
                    let removed_leaf_set = removed_leaf_indices
                        .iter()
                        .copied()
                        .collect::<BTreeSet<_>>();
                    let mut expected_recipient_ids = self
                        .group
                        .members()
                        .filter(|member| !removed_leaf_set.contains(&member.index))
                        .map(|member| {
                            mlkem1024_member_id(&basic_credential_identity(&member.credential)?)
                        })
                        .collect::<Result<BTreeSet<_>>>()?;
                    for add in staged_commit.add_proposals() {
                        expected_recipient_ids.insert(mlkem1024_member_id(
                            &basic_credential_identity(
                                add.add_proposal().key_package().leaf_node().credential(),
                            )?,
                        )?);
                    }
                    ensure!(
                        staged_pq_extension
                            .recipients
                            .keys()
                            .cloned()
                            .collect::<BTreeSet<_>>()
                            == expected_recipient_ids,
                        "secure mesh MLS ML-KEM-1024 recipient roster differs from staged MLS roster"
                    );
                    let local_is_removed = removed_leaf_indices.contains(&self.own_leaf_index());
                    if !local_is_removed {
                        let local_member_id =
                            mlkem1024_member_id(&participant.credential_identity_bytes()?)?;
                        ensure!(
                            staged_pq_extension
                                .recipients
                                .contains_key(&local_member_id),
                            "secure mesh MLS ML-KEM-1024 local recipient is missing"
                        );
                        let staged_recipient_ids = staged_pq_extension
                            .recipients
                            .keys()
                            .cloned()
                            .collect::<BTreeSet<_>>();
                        open_mlkem1024_epoch_extension(
                            self.group.group_id().as_slice(),
                            staged_pq_extension.epoch,
                            &staged_recipient_ids,
                            &participant.credential_identity_bytes()?,
                            &participant.provider.mlkem1024_seed,
                            &staged_pq_extension,
                        )?;
                    }
                }
                verifier(
                    &current_extension,
                    &staged_extension,
                    &removed_leaf_indices,
                    added_member_count,
                )?;
                self.group
                    .merge_staged_commit(&participant.provider, *staged_commit)
                    .map_err(|error| {
                        anyhow!("secure mesh MLS staged commit merge failed: {error:?}")
                    })?;
                self.refresh_authenticated_group_context(participant)?;
                ensure!(
                    self.capability_extension()? == staged_extension,
                    "secure mesh MLS merged capability extension mismatch"
                );
                Ok(())
            }
            _ => Err(anyhow!("secure mesh MLS message did not contain a commit")),
        }
    }
}
