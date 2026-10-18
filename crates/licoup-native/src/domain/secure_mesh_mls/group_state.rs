use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::core::secure_mesh_mls::{
    SECURE_MESH_GROUP_MLS_PROTOCOL_VERSION, SecureMeshMlsDurableStore, SecureMeshMlsGroup,
    SecureMeshMlsParticipant,
};
use crate::core::secure_mesh_trust::DeviceTrustPublicIdentity;

use super::input_codec::{encode_base64url, identity_to_json};
use super::journal_recovery::current_group_metadata;

pub(super) fn load_group_checked(
    group_store: &mut SecureMeshMlsDurableStore,
    participant: &SecureMeshMlsParticipant,
    identity: &DeviceTrustPublicIdentity,
    group_id: &[u8],
) -> Result<SecureMeshMlsGroup> {
    let group = SecureMeshMlsGroup::load(participant, group_id)?;
    let metadata = group.public_metadata(identity.fingerprint()?)?;
    if group_store
        .read(&metadata.group_id_hash, &metadata.participant_endpoint_id)?
        .is_some()
    {
        let previous = group_store.reconcile_authenticated_snapshot(
            &metadata,
            OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .map_err(|_| anyhow!("secure mesh MLS metadata timestamp is invalid"))?,
        )?;
        ensure!(
            metadata.epoch == previous.epoch
                && metadata.public_state_digest == previous.public_state_digest
                && metadata.member_count == previous.member_count
                && metadata.own_leaf_index == previous.own_leaf_index
                && metadata.active == previous.active,
            "secure mesh MLS selected-custody group state differs from durable authority"
        );
    } else {
        return Err(anyhow!(
            "secure mesh MLS durable group authority is missing"
        ));
    }
    Ok(group)
}

pub(super) fn load_group_for_journal(
    group_store: &mut SecureMeshMlsDurableStore,
    participant: &SecureMeshMlsParticipant,
    identity: &DeviceTrustPublicIdentity,
    group_id: &[u8],
) -> Result<SecureMeshMlsGroup> {
    let group = SecureMeshMlsGroup::load(participant, group_id)?;
    let metadata = current_group_metadata(&group, identity)?;
    if group_store
        .read(&metadata.group_id_hash, &metadata.participant_endpoint_id)?
        .is_some()
    {
        let previous = group_store.reconcile_authenticated_snapshot(
            &metadata,
            OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .map_err(|_| anyhow!("secure mesh MLS metadata timestamp is invalid"))?,
        )?;
        ensure!(
            metadata.epoch >= previous.epoch,
            "secure mesh MLS selected-custody group state rollback detected"
        );
        if metadata.epoch == previous.epoch {
            ensure!(
                metadata.public_state_digest == previous.public_state_digest
                    && metadata.member_count == previous.member_count
                    && metadata.own_leaf_index == previous.own_leaf_index
                    && metadata.active == previous.active,
                "secure mesh MLS same-epoch selected-custody state diverges"
            );
        }
    }
    Ok(group)
}

pub(super) fn require_group_base_current(
    group_store: &SecureMeshMlsDurableStore,
    base: Option<&crate::core::secure_mesh_mls::SecureMeshMlsGroupMetadata>,
    group_id_hash: &str,
    participant_scope: &str,
) -> Result<()> {
    let durable = group_store.read(group_id_hash, participant_scope)?;
    match (base, durable) {
        (None, None) => Ok(()),
        (Some(base), Some(durable)) => {
            ensure!(
                durable.group_id_hash == base.group_id_hash
                    && durable.participant_endpoint_id == base.participant_endpoint_id
                    && durable.public_state_digest == base.public_state_digest
                    && durable.epoch == base.epoch
                    && durable.member_count == base.member_count
                    && durable.own_leaf_index == base.own_leaf_index
                    && durable.active == base.active,
                "secure mesh MLS operation base state is stale"
            );
            Ok(())
        }
        _ => Err(anyhow!(
            "secure mesh MLS operation base state diverges from durable metadata"
        )),
    }
}

pub(super) fn reconcile_group_metadata(
    group_store: &mut SecureMeshMlsDurableStore,
    group: &SecureMeshMlsGroup,
    identity: &DeviceTrustPublicIdentity,
) -> Result<crate::core::secure_mesh_mls::SecureMeshMlsDurableRecord> {
    let metadata = group.public_metadata(identity.fingerprint()?)?;
    let updated_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|_| anyhow!("secure mesh MLS metadata timestamp is invalid"))?;
    let previous = group_store.read(&metadata.group_id_hash, &metadata.participant_endpoint_id)?;
    let previous = match previous {
        Some(_) => {
            Some(group_store.reconcile_authenticated_snapshot(&metadata, updated_at.clone())?)
        }
        None => None,
    };
    match previous {
        None => group_store.upsert_initial(&metadata, updated_at),
        Some(previous)
            if previous.epoch == metadata.epoch
                && previous.public_state_digest == metadata.public_state_digest
                && previous.member_count == metadata.member_count
                && previous.own_leaf_index == metadata.own_leaf_index
                && previous.active == metadata.active =>
        {
            Ok(previous)
        }
        Some(previous) => group_store.commit_epoch(&previous, &metadata, updated_at),
    }
}

pub(super) fn group_status_json(
    group: &SecureMeshMlsGroup,
    record: &crate::core::secure_mesh_mls::SecureMeshMlsDurableRecord,
) -> Value {
    json!({
        "ok": true,
        "protocolVersion": SECURE_MESH_GROUP_MLS_PROTOCOL_VERSION,
        "groupIdHash": record.group_id_hash,
        "epoch": group.epoch(),
        "stateVersion": record.state_version,
        "memberCount": group.member_count(),
        "active": group.is_active(),
        "capabilityNegotiated": group.require_active_capability_negotiation().is_ok(),
        "participantScopeRedacted": true
    })
}

pub(super) fn public_local_participant(
    identity: &DeviceTrustPublicIdentity,
    participant: &SecureMeshMlsParticipant,
) -> Result<Value> {
    ensure!(
        participant.signing_public_key() == identity.signing_public_key,
        "secure mesh MLS participant signer does not match local identity"
    );
    Ok(json!({
        "identity": identity_to_json(identity),
        "credentialBound": true,
        "signingPublicKeyBase64url": encode_base64url(&participant.signing_public_key())
    }))
}
