use super::capability_proof::verify_complete_member_capability_proof_map;
use super::helpers::participant_identity_matches;
use super::identity_trust::mls_credential_identity_bytes;
use super::invitation_authorization::{authorize_commit_sender, authorize_sender_endpoint_binding};
use anyhow::{Result, ensure};
use std::collections::{BTreeMap, BTreeSet};

use crate::core::secure_mesh_crypto::{
    OpenedSecureMeshPayload, SecureMeshContentContext, SecureMeshPayloadKind, SecureMeshPlaintext,
};
use crate::core::secure_mesh_mls::{SecureMeshMlsGroup, SecureMeshMlsParticipant};
use crate::core::secure_mesh_trust::{DeviceTrustPublicIdentity, DeviceTrustState};

pub fn seal_product_payload_message(
    group: &mut SecureMeshMlsGroup,
    sender: &SecureMeshMlsParticipant,
    sender_identity: &DeviceTrustPublicIdentity,
    sender_trust_state: &DeviceTrustState,
    trusted_roster: &BTreeMap<String, DeviceTrustPublicIdentity>,
    context: &SecureMeshContentContext,
    plaintext: &SecureMeshPlaintext,
) -> Result<Vec<u8>> {
    let roster_endpoint_ids = trusted_roster.keys().cloned().collect::<BTreeSet<_>>();
    authorize_commit_sender(
        &sender_identity.endpoint_id,
        sender_trust_state,
        &roster_endpoint_ids,
    )?;
    verify_complete_member_capability_proof_map(
        &group.capability_extension()?,
        &roster_endpoint_ids,
        trusted_roster,
    )?;
    authorize_sender_endpoint_binding(&context.sender_endpoint_id, &sender_identity.endpoint_id)?;
    ensure!(
        participant_identity_matches(sender, sender_identity)?,
        "secure mesh MLS payload signer does not match trusted sender identity"
    );
    group.require_active_capability_negotiation()?;
    group.seal_payload_message(sender, context, plaintext)
}

pub fn open_product_payload_message(
    group: &mut SecureMeshMlsGroup,
    receiver: &SecureMeshMlsParticipant,
    receiver_identity: &DeviceTrustPublicIdentity,
    trusted_sender_identity: &DeviceTrustPublicIdentity,
    trusted_sender_state: &DeviceTrustState,
    trusted_roster: &BTreeMap<String, DeviceTrustPublicIdentity>,
    context: &SecureMeshContentContext,
    message: &[u8],
    expected_kind: SecureMeshPayloadKind,
) -> Result<OpenedSecureMeshPayload> {
    ensure!(
        participant_identity_matches(receiver, receiver_identity)?,
        "secure mesh MLS receiving participant credential is not identity-bound"
    );
    let roster_endpoint_ids = trusted_roster.keys().cloned().collect::<BTreeSet<_>>();
    authorize_commit_sender(
        &trusted_sender_identity.endpoint_id,
        trusted_sender_state,
        &roster_endpoint_ids,
    )?;
    verify_complete_member_capability_proof_map(
        &group.capability_extension()?,
        &roster_endpoint_ids,
        trusted_roster,
    )?;
    authorize_sender_endpoint_binding(
        &context.sender_endpoint_id,
        &trusted_sender_identity.endpoint_id,
    )?;
    group.require_active_capability_negotiation()?;
    group.open_payload_message_with_sender_verifier(
        receiver,
        context,
        message,
        expected_kind,
        |credential_identity, signing_public_key, _leaf_index| {
            ensure!(
                credential_identity == mls_credential_identity_bytes(trusted_sender_identity)?
                    && signing_public_key == trusted_sender_identity.signing_public_key,
                "secure mesh MLS payload signer does not match trusted sender identity"
            );
            Ok(())
        },
    )
}
