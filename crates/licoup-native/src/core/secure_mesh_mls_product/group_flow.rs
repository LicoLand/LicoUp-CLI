use super::capability_proof::{
    active_pair_capability_proofs, capability_intersection_from_member_proofs,
    mls_capability_proof_request, mls_capability_verification_context,
    mls_key_package_capability_challenge, verify_active_mls_capability_extension,
    verify_complete_member_capability_proof_map,
};
use super::helpers::{
    endpoint_id_from_credential_identity, hex_sha256, key_package_identity_matches,
    participant_identity_matches,
};
use super::identity_trust::{
    directory_roster_from_group, mls_credential_identity_bytes, require_verified_member_trust,
};
use super::invitation_authorization::{
    SecureMeshMlsExpectedInvitation, authorize_commit_sender, authorize_member_add_with_directory,
    authorize_welcome_acceptance, cross_check_roster,
};
use super::security_ledger::{
    PreparedMlsSecurityInputs, empty_prepared_security_inputs, prepare_capability_security_inputs,
    prepare_member_add_security_inputs,
};
use anyhow::{Result, anyhow, ensure};
use ed25519_dalek::SigningKey;
use std::collections::{BTreeMap, BTreeSet};
use time::OffsetDateTime;

use crate::core::secure_mesh_capability::CapabilityEvaluation;
use crate::core::secure_mesh_capability_proof::{
    SignedCapabilityProof, sign_capability_proof, signed_capability_proof_challenge,
};
use crate::core::secure_mesh_directory::AuthorizedDirectoryLeaf;
use crate::core::secure_mesh_mls::{
    MLS_CAPABILITY_EXTENSION_SCHEMA_VERSION, SecureMeshMlsCapabilityExtension, SecureMeshMlsCommit,
    SecureMeshMlsGroup, SecureMeshMlsKeyPackage, SecureMeshMlsMemberCapabilityProof,
    SecureMeshMlsParticipant, SecureMeshMlsRosterTransition, SecureMeshMlsWelcome,
    secure_mesh_mls_capability_extension_digest,
};
use crate::core::secure_mesh_session_negotiation::create_mls_capability_binding;
use crate::core::secure_mesh_trust::{DeviceTrustPublicIdentity, DeviceTrustState};

pub fn create_product_group(
    owner: &SecureMeshMlsParticipant,
    owner_identity: &DeviceTrustPublicIdentity,
    owner_trust_state: &DeviceTrustState,
    group_id: impl AsRef<[u8]>,
) -> Result<SecureMeshMlsGroup> {
    require_verified_member_trust(owner_trust_state)?;
    ensure!(
        participant_identity_matches(owner, owner_identity)?,
        "secure mesh MLS owner credential is not identity-bound"
    );
    SecureMeshMlsGroup::create(owner, group_id)
}

pub(crate) fn add_product_member_prepared(
    group: &mut SecureMeshMlsGroup,
    owner: &SecureMeshMlsParticipant,
    owner_identity: &DeviceTrustPublicIdentity,
    owner_signing_key: &SigningKey,
    owner_capability_evaluation: &CapabilityEvaluation,
    owner_trust_state: &DeviceTrustState,
    member_key_package: &SecureMeshMlsKeyPackage,
    member_identity: &DeviceTrustPublicIdentity,
    member_capability_proof: &SignedCapabilityProof,
    member_trust_state: &DeviceTrustState,
    member_directory_authorization: &AuthorizedDirectoryLeaf,
    member_directory_version: u64,
    member_key_package_version: u64,
    key_package_id: &str,
    now: OffsetDateTime,
) -> Result<(SecureMeshMlsWelcome, PreparedMlsSecurityInputs)> {
    require_verified_member_trust(owner_trust_state)?;
    require_verified_member_trust(member_trust_state)?;
    authorize_member_add_with_directory(
        member_directory_authorization,
        member_identity,
        member_key_package,
        member_directory_version,
        member_key_package_version,
    )?;
    ensure!(
        participant_identity_matches(owner, owner_identity)?,
        "secure mesh MLS owner credential is not identity-bound"
    );
    ensure!(
        key_package_identity_matches(member_key_package, member_identity)?,
        "secure mesh MLS keypackage credential is not identity-bound"
    );
    ensure!(
        owner_signing_key.verifying_key().to_bytes() == owner_identity.signing_public_key,
        "secure mesh MLS owner capability signing key does not match identity"
    );
    let previous_extension = group.capability_extension()?;
    if matches!(
        previous_extension,
        SecureMeshMlsCapabilityExtension::AwaitingMemberNegotiation { .. }
    ) {
        ensure!(
            group.member_count() == 1,
            "secure mesh MLS pre-existing members lack capability negotiation"
        );
    }
    let challenge = mls_key_package_capability_challenge(member_key_package);
    ensure!(
        signed_capability_proof_challenge(member_capability_proof)? == challenge,
        "secure mesh MLS member capability proof is not bound to its key package"
    );
    let owner_capability_proof = sign_capability_proof(
        owner_identity,
        owner_signing_key,
        owner_capability_evaluation,
        &mls_capability_proof_request(challenge, now)?,
    )?;
    let verification_context = mls_capability_verification_context(challenge, now)?;
    let owner_verified = crate::core::secure_mesh_capability_proof::verify_capability_proof(
        owner_identity,
        &owner_capability_proof,
        &verification_context,
    )?;
    let member_verified = crate::core::secure_mesh_capability_proof::verify_capability_proof(
        member_identity,
        member_capability_proof,
        &verification_context,
    )?;
    let base_transcript_digest = group.capability_add_base_transcript_digest(member_key_package)?;
    let pair_binding =
        create_mls_capability_binding(&owner_verified, &member_verified, &base_transcript_digest)?;
    let current_roster_endpoint_ids = group
        .member_credential_identities()?
        .into_iter()
        .map(|credential| endpoint_id_from_credential_identity(&credential))
        .collect::<Result<BTreeSet<_>>>()?;
    let (previous_extension_digest, mut member_capability_proofs) = match &previous_extension {
        SecureMeshMlsCapabilityExtension::AwaitingMemberNegotiation { .. } => {
            (None, BTreeMap::new())
        }
        SecureMeshMlsCapabilityExtension::Active {
            member_capability_proofs,
            ..
        } => {
            ensure!(
                member_capability_proofs
                    .keys()
                    .cloned()
                    .collect::<BTreeSet<_>>()
                    == current_roster_endpoint_ids,
                "secure mesh MLS prior member capability proof map does not match roster"
            );
            (
                Some(secure_mesh_mls_capability_extension_digest(
                    &previous_extension,
                )?),
                member_capability_proofs.clone(),
            )
        }
    };
    member_capability_proofs.insert(
        owner_identity.endpoint_id.clone(),
        SecureMeshMlsMemberCapabilityProof {
            endpoint_id: owner_identity.endpoint_id.clone(),
            accepted_at_unix_seconds: now.unix_timestamp(),
            proof: owner_capability_proof,
        },
    );
    ensure!(
        member_capability_proofs
            .insert(
                member_identity.endpoint_id.clone(),
                SecureMeshMlsMemberCapabilityProof {
                    endpoint_id: member_identity.endpoint_id.clone(),
                    accepted_at_unix_seconds: now.unix_timestamp(),
                    proof: member_capability_proof.clone(),
                },
            )
            .is_none(),
        "secure mesh MLS added member already has a capability proof record"
    );
    let mut expected_roster = current_roster_endpoint_ids;
    expected_roster.insert(member_identity.endpoint_id.clone());
    ensure!(
        member_capability_proofs
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>()
            == expected_roster,
        "secure mesh MLS updated member capability proof map does not match roster"
    );
    let group_negotiated_protocol_capabilities =
        capability_intersection_from_member_proofs(&member_capability_proofs)?;
    let capability_extension = SecureMeshMlsCapabilityExtension::Active {
        schema_version: MLS_CAPABILITY_EXTENSION_SCHEMA_VERSION,
        activated_at_epoch: group.epoch().saturating_add(1),
        previous_extension_digest,
        committer_endpoint_id: owner_identity.endpoint_id.clone(),
        roster_transition: Box::new(SecureMeshMlsRosterTransition::MemberAdded {
            member_endpoint_id: member_identity.endpoint_id.clone(),
            pair_binding,
        }),
        member_capability_proofs,
        group_negotiated_protocol_capabilities,
    };
    verify_active_mls_capability_extension(
        &capability_extension,
        owner_identity,
        member_identity,
        now,
    )?;
    let group_id_hash = hex_sha256(&group.group_id_bytes()?);
    let (committer_proof, added_member_proof) =
        active_pair_capability_proofs(&capability_extension)?;
    let prepared_security = prepare_member_add_security_inputs(
        owner_identity,
        key_package_id,
        member_key_package.as_public_bytes(),
        &group_id_hash,
        committer_proof,
        added_member_proof,
        now.unix_timestamp(),
    )?;
    let welcome = group.add_member_with_capability_extension(
        owner,
        member_key_package,
        &capability_extension,
    )?;
    Ok((welcome, prepared_security))
}

pub(crate) fn remove_product_member_prepared(
    group: &mut SecureMeshMlsGroup,
    remover: &SecureMeshMlsParticipant,
    remover_identity: &DeviceTrustPublicIdentity,
    remover_trust_state: &DeviceTrustState,
    removed_member_identity: &DeviceTrustPublicIdentity,
    removed_member_trust_state: &DeviceTrustState,
    now: OffsetDateTime,
) -> Result<(SecureMeshMlsCommit, PreparedMlsSecurityInputs)> {
    require_verified_member_trust(remover_trust_state)?;
    ensure!(
        matches!(
            removed_member_trust_state,
            DeviceTrustState::Verified | DeviceTrustState::CrossSigned | DeviceTrustState::Revoked
        ),
        "secure mesh MLS removed member identity is not locally verified"
    );
    ensure!(
        participant_identity_matches(remover, remover_identity)?,
        "secure mesh MLS remover credential is not identity-bound"
    );
    ensure!(
        remover_identity.endpoint_id != removed_member_identity.endpoint_id,
        "secure mesh MLS member-remove action cannot remove the local identity"
    );
    ensure!(
        group.member_count() > 1,
        "secure mesh MLS member-remove action cannot empty the group"
    );

    let current_roster = directory_roster_from_group(group)?;
    ensure!(
        current_roster.get(&remover_identity.endpoint_id) == Some(remover_identity),
        "secure mesh MLS remover is not the exact current roster identity"
    );
    ensure!(
        current_roster.get(&removed_member_identity.endpoint_id) == Some(removed_member_identity),
        "secure mesh MLS removed member is not the exact current roster identity"
    );
    let current_roster_endpoint_ids = current_roster.keys().cloned().collect::<BTreeSet<_>>();
    let current_extension = group.capability_extension()?;
    verify_complete_member_capability_proof_map(
        &current_extension,
        &current_roster_endpoint_ids,
        &current_roster,
    )?;
    let SecureMeshMlsCapabilityExtension::Active {
        member_capability_proofs,
        ..
    } = &current_extension
    else {
        return Err(anyhow!(
            "secure mesh MLS member capability negotiation is incomplete"
        ));
    };

    let removed_leaf_index = group.member_leaf_index_for_identity(
        &mls_credential_identity_bytes(removed_member_identity)?,
        &removed_member_identity.signing_public_key,
    )?;
    ensure!(
        removed_leaf_index != group.own_leaf_index(),
        "secure mesh MLS member-remove action resolved the local leaf"
    );
    let mut next_member_capability_proofs = member_capability_proofs.clone();
    let removed_proof = next_member_capability_proofs
        .remove(&removed_member_identity.endpoint_id)
        .ok_or_else(|| anyhow!("secure mesh MLS removed member capability proof is missing"))?;
    ensure!(
        removed_proof.endpoint_id == removed_member_identity.endpoint_id,
        "secure mesh MLS removed member capability proof binding is invalid"
    );
    let mut next_roster = current_roster;
    ensure!(
        next_roster
            .remove(&removed_member_identity.endpoint_id)
            .is_some(),
        "secure mesh MLS removed member disappeared from the current roster"
    );
    let next_group_capabilities =
        capability_intersection_from_member_proofs(&next_member_capability_proofs)?;
    let next_extension = SecureMeshMlsCapabilityExtension::Active {
        schema_version: MLS_CAPABILITY_EXTENSION_SCHEMA_VERSION,
        activated_at_epoch: group.epoch().saturating_add(1),
        previous_extension_digest: Some(secure_mesh_mls_capability_extension_digest(
            &current_extension,
        )?),
        committer_endpoint_id: remover_identity.endpoint_id.clone(),
        roster_transition: Box::new(SecureMeshMlsRosterTransition::MemberRemoved {
            member_endpoint_id: removed_member_identity.endpoint_id.clone(),
        }),
        member_capability_proofs: next_member_capability_proofs,
        group_negotiated_protocol_capabilities: next_group_capabilities,
    };
    verify_complete_member_capability_proof_map(
        &next_extension,
        &next_roster.keys().cloned().collect(),
        &next_roster,
    )?;
    let commit = group.remove_member_with_capability_extension(
        remover,
        removed_leaf_index,
        &next_extension,
    )?;
    ensure!(
        directory_roster_from_group(group)? == next_roster,
        "secure mesh MLS committed remove roster differs from the verified next roster"
    );
    Ok((
        commit,
        empty_prepared_security_inputs(remover_identity, now.unix_timestamp())?,
    ))
}

pub(crate) fn join_product_group_from_welcome_prepared(
    participant: &SecureMeshMlsParticipant,
    participant_identity: &DeviceTrustPublicIdentity,
    invitation: &SecureMeshMlsExpectedInvitation,
    inviter_identity: &DeviceTrustPublicIdentity,
    inviter_trust_state: &DeviceTrustState,
    trusted_roster: &BTreeMap<String, DeviceTrustPublicIdentity>,
    welcome: &SecureMeshMlsWelcome,
    now: OffsetDateTime,
) -> Result<(SecureMeshMlsGroup, PreparedMlsSecurityInputs)> {
    ensure!(
        participant_identity_matches(participant, participant_identity)?,
        "secure mesh MLS joiner credential is not identity-bound"
    );
    require_verified_member_trust(inviter_trust_state)?;
    ensure!(
        inviter_identity.endpoint_id == invitation.inviter_endpoint_id,
        "secure mesh MLS inviter identity does not match invitation"
    );
    let group = SecureMeshMlsGroup::join_from_welcome_with_capability_verifier(
        participant,
        &welcome.welcome_message,
        |extension| {
            verify_complete_member_capability_proof_map(
                extension,
                &invitation.expected_roster_endpoint_ids,
                trusted_roster,
            )?;
            verify_active_mls_capability_extension(
                extension,
                inviter_identity,
                participant_identity,
                now,
            )?;
            Ok(())
        },
    )?;
    authorize_welcome_acceptance(invitation, inviter_trust_state, &group.group_id_bytes()?)?;
    cross_check_roster(
        &invitation.expected_roster_endpoint_ids,
        &group.member_credential_identities()?,
        trusted_roster,
    )?;
    let extension = group.capability_extension()?;
    let SecureMeshMlsCapabilityExtension::Active {
        activated_at_epoch,
        previous_extension_digest,
        ..
    } = &extension
    else {
        return Err(anyhow!(
            "secure mesh MLS joined capability extension is inactive"
        ));
    };
    ensure!(
        *activated_at_epoch <= group.epoch(),
        "secure mesh MLS capability extension epoch is from the future"
    );
    if invitation.expected_roster_endpoint_ids.len() == 2 {
        ensure!(
            previous_extension_digest.is_none(),
            "secure mesh MLS initial capability extension has unexpected history"
        );
    }
    let (committer_proof, added_member_proof) = active_pair_capability_proofs(&extension)?;
    let prepared_security = prepare_capability_security_inputs(
        participant_identity,
        committer_proof,
        added_member_proof,
        now.unix_timestamp(),
    )?;
    Ok((group, prepared_security))
}

pub(crate) fn process_product_commit_prepared(
    group: &mut SecureMeshMlsGroup,
    participant: &SecureMeshMlsParticipant,
    observing_identity: &DeviceTrustPublicIdentity,
    committer_identity: &DeviceTrustPublicIdentity,
    committer_trust_state: &DeviceTrustState,
    added_member_identity: Option<&DeviceTrustPublicIdentity>,
    removed_member_identity: Option<&DeviceTrustPublicIdentity>,
    trusted_roster: &BTreeMap<String, DeviceTrustPublicIdentity>,
    commit_message: &[u8],
    now: OffsetDateTime,
) -> Result<PreparedMlsSecurityInputs> {
    require_verified_member_trust(committer_trust_state)?;
    ensure!(
        participant_identity_matches(participant, observing_identity)?,
        "secure mesh MLS observing participant credential is not identity-bound"
    );
    ensure!(
        added_member_identity.is_none() || removed_member_identity.is_none(),
        "secure mesh MLS commit cannot add and remove a member in one product transition"
    );
    let current_roster = directory_roster_from_group(group)?;
    let roster = current_roster.keys().cloned().collect::<BTreeSet<_>>();
    ensure!(
        current_roster.get(&observing_identity.endpoint_id) == Some(observing_identity),
        "secure mesh MLS observing identity is not the exact current roster member"
    );
    authorize_commit_sender(
        &committer_identity.endpoint_id,
        committer_trust_state,
        &roster,
    )?;
    ensure!(
        current_roster.get(&committer_identity.endpoint_id) == Some(committer_identity),
        "secure mesh MLS committer identity differs from the current roster"
    );
    verify_complete_member_capability_proof_map(
        &group.capability_extension()?,
        &roster,
        &current_roster,
    )?;

    let mut expected_roster = current_roster.clone();
    if let Some(added_member_identity) = added_member_identity {
        ensure!(
            expected_roster
                .insert(
                    added_member_identity.endpoint_id.clone(),
                    added_member_identity.clone(),
                )
                .is_none(),
            "secure mesh MLS commit added member is already in the current roster"
        );
    }
    let expected_removed_leaf = if let Some(removed_member_identity) = removed_member_identity {
        ensure!(
            expected_roster.get(&removed_member_identity.endpoint_id)
                == Some(removed_member_identity),
            "secure mesh MLS removed member identity differs from the current roster"
        );
        let leaf = group.member_leaf_index_for_identity(
            &mls_credential_identity_bytes(removed_member_identity)?,
            &removed_member_identity.signing_public_key,
        )?;
        ensure!(
            expected_roster
                .remove(&removed_member_identity.endpoint_id)
                .is_some(),
            "secure mesh MLS removed member is absent from the current roster"
        );
        Some(leaf)
    } else {
        None
    };
    ensure!(
        &expected_roster == trusted_roster,
        "secure mesh MLS trusted roster does not equal the expected post-commit roster"
    );
    let expected_roster_endpoint_ids = expected_roster.keys().cloned().collect::<BTreeSet<_>>();
    let expected_next_epoch = group.epoch().saturating_add(1);
    let mut prepared_security = None;
    group.process_commit_with_capability_verifier(
        participant,
        commit_message,
        true,
        |credential_identity, signing_public_key, _leaf_index| {
            ensure!(
                credential_identity == mls_credential_identity_bytes(committer_identity)?
                    && signing_public_key == committer_identity.signing_public_key,
                "secure mesh MLS commit signer does not match trusted committer identity"
            );
            Ok(())
        },
        |current, staged, removed_leaf_indices, added_member_count| {
            verify_complete_member_capability_proof_map(
                staged,
                &expected_roster_endpoint_ids,
                trusted_roster,
            )?;
            if current == staged {
                ensure!(
                    added_member_identity.is_none()
                        && removed_member_identity.is_none()
                        && removed_leaf_indices.is_empty()
                        && added_member_count == 0,
                    "secure mesh MLS roster-changing commit did not authenticate a capability transition"
                );
                return Ok(());
            }
            let SecureMeshMlsCapabilityExtension::Active {
                activated_at_epoch,
                committer_endpoint_id,
                roster_transition,
                group_negotiated_protocol_capabilities: staged_group_capabilities,
                ..
            } = staged
            else {
                return Err(anyhow!(
                    "secure mesh MLS capability-changing commit is inactive"
                ));
            };
            ensure!(
                *activated_at_epoch == expected_next_epoch
                    && committer_endpoint_id == &committer_identity.endpoint_id,
                "secure mesh MLS roster transition epoch or committer binding is invalid"
            );
            match (added_member_identity, removed_member_identity) {
                (Some(added_member_identity), None) => {
                    ensure!(
                        added_member_count == 1 && removed_leaf_indices.is_empty(),
                        "secure mesh MLS member-add commit has an invalid roster delta"
                    );
                    let pair_capabilities = verify_active_mls_capability_extension(
                        staged,
                        committer_identity,
                        added_member_identity,
                        now,
                    )?;
                    let (committer_proof, added_member_proof) =
                        active_pair_capability_proofs(staged)?;
                    prepared_security = Some(prepare_capability_security_inputs(
                        observing_identity,
                        committer_proof,
                        added_member_proof,
                        now.unix_timestamp(),
                    )?);
                    let expected_group_capabilities = current
                        .group_negotiated_protocol_capabilities()
                        .map(|capabilities| {
                            capabilities
                                .intersection(&pair_capabilities)
                                .copied()
                                .collect::<BTreeSet<_>>()
                        })
                        .unwrap_or(pair_capabilities);
                    ensure!(
                        staged_group_capabilities == &expected_group_capabilities,
                        "secure mesh MLS cumulative capability intersection is invalid"
                    );
                }
                (None, Some(removed_member_identity)) => {
                    ensure!(
                        added_member_count == 0
                            && removed_leaf_indices == expected_removed_leaf.as_slice(),
                        "secure mesh MLS member-remove commit targets the wrong leaf"
                    );
                    ensure!(
                        matches!(
                            roster_transition.as_ref(),
                            SecureMeshMlsRosterTransition::MemberRemoved { member_endpoint_id }
                                if member_endpoint_id == &removed_member_identity.endpoint_id
                        ),
                        "secure mesh MLS member-remove capability transition targets the wrong endpoint"
                    );
                }
                (None, None) => {
                    return Err(anyhow!(
                        "secure mesh MLS capability-changing commit lacks a roster transition"
                    ));
                }
                (Some(_), Some(_)) => unreachable!(),
            }
            Ok(())
        },
    )?;
    cross_check_roster(
        &expected_roster_endpoint_ids,
        &group.member_credential_identities()?,
        trusted_roster,
    )?;
    prepared_security
        .map(Ok)
        .unwrap_or_else(|| empty_prepared_security_inputs(observing_identity, now.unix_timestamp()))
}
