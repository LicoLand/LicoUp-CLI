use super::transaction::MobileRelayPairwiseOperation;
#[cfg(test)]
use super::transaction::mobile_relay_pairwise_operation;
use crate::core::secure_mesh_crypto::{
    SecureMeshContentContext, SecureMeshPayloadKind, SecureMeshPlaintext,
};
use crate::domain::mobile_relay::endpoint_trust::{
    bounded_time_window, ensure_peer_authorized_for_protected_send,
    ensure_peer_trust_authorized_for_protected_send, local_endpoint_state, peer_endpoint_state,
    protected_send_kind_from_payload, session_id,
};
use crate::domain::mobile_relay::relay_operations::{
    canonical_mailbox_token, current_mailbox_rotation_epoch, validate_secure_envelope,
};
use crate::domain::mobile_relay::secret_custody::{
    RuntimeSecretMaterial, ensure_secure_mesh_protected_operation_allowed,
};
use crate::domain::mobile_relay::support::{
    MOBILE_RELAY_COMMAND_TTL_SECONDS, MOBILE_RELAY_RESULT_TTL_SECONDS,
};
use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose};
use licoup_protocol_bindings::licoarc_relay::LicoArcRelayEnvelope;
use rand_core::{OsRng, RngCore};
use serde_json::Value;
use uuid::Uuid;

#[cfg(test)]
pub(in crate::domain::mobile_relay) fn seal_mobile_relay_payload(
    config: &Value,
    secret_material: &RuntimeSecretMaterial,
    kind: SecureMeshPayloadKind,
    payload: &Value,
) -> Result<Value> {
    let mut pairwise_operation = mobile_relay_pairwise_operation(
        config,
        secret_material,
        "Mobile Relay pairwise payload authorization batch",
        3,
    )?;
    seal_mobile_relay_payload_with_pairwise_operation(
        config,
        secret_material,
        kind,
        payload,
        &mut pairwise_operation,
    )
}

#[cfg(test)]
pub(in crate::domain::mobile_relay) fn seal_mobile_relay_payload_with_pairwise_operation(
    config: &Value,
    secret_material: &RuntimeSecretMaterial,
    kind: SecureMeshPayloadKind,
    payload: &Value,
    pairwise_operation: &mut MobileRelayPairwiseOperation,
) -> Result<Value> {
    seal_mobile_relay_payload_with_pairwise_operation_and_gate_and_commit(
        config,
        secret_material,
        kind,
        payload,
        pairwise_operation,
        PairwiseDirectoryGate::Required,
        true,
    )
}

pub(in crate::domain::mobile_relay) fn seal_mobile_relay_payload_deferred(
    config: &Value,
    secret_material: &RuntimeSecretMaterial,
    kind: SecureMeshPayloadKind,
    payload: &Value,
    pairwise_operation: &mut MobileRelayPairwiseOperation,
) -> Result<Value> {
    seal_mobile_relay_payload_with_pairwise_operation_and_gate_and_commit(
        config,
        secret_material,
        kind,
        payload,
        pairwise_operation,
        PairwiseDirectoryGate::Required,
        false,
    )
}

pub(in crate::domain::mobile_relay) fn seal_mobile_relay_payload_with_pairwise_operation_and_gate(
    config: &Value,
    secret_material: &RuntimeSecretMaterial,
    kind: SecureMeshPayloadKind,
    payload: &Value,
    pairwise_operation: &mut MobileRelayPairwiseOperation,
    directory_gate: PairwiseDirectoryGate,
) -> Result<Value> {
    seal_mobile_relay_payload_with_pairwise_operation_and_gate_and_commit(
        config,
        secret_material,
        kind,
        payload,
        pairwise_operation,
        directory_gate,
        true,
    )
}

fn seal_mobile_relay_payload_with_pairwise_operation_and_gate_and_commit(
    config: &Value,
    secret_material: &RuntimeSecretMaterial,
    kind: SecureMeshPayloadKind,
    payload: &Value,
    pairwise_operation: &mut MobileRelayPairwiseOperation,
    directory_gate: PairwiseDirectoryGate,
    commit: bool,
) -> Result<Value> {
    ensure_secure_mesh_protected_operation_allowed()?;
    let payload_kind = protected_send_kind_from_payload(kind);
    let _authorization = match directory_gate {
        PairwiseDirectoryGate::Required => {
            ensure_peer_authorized_for_protected_send(config, payload_kind)?
        }
        PairwiseDirectoryGate::KtGossipControl => {
            ensure_peer_trust_authorized_for_protected_send(config, payload_kind)?
        }
    };
    let endpoint = local_endpoint_state(config, secret_material)?;
    let peer = peer_endpoint_state(config)?;
    let (created_at, expires_at) = bounded_time_window(match kind {
        SecureMeshPayloadKind::ResultPayload | SecureMeshPayloadKind::Error => {
            MOBILE_RELAY_RESULT_TTL_SECONDS
        }
        _ => MOBILE_RELAY_COMMAND_TTL_SECONDS,
    })?;
    let mut delivery_id_bytes = [0u8; 24];
    OsRng.fill_bytes(&mut delivery_id_bytes);
    let envelope_id = general_purpose::URL_SAFE_NO_PAD.encode(delivery_id_bytes);
    let message_id = format!("msg_{}", Uuid::new_v4());
    let opaque_mailbox_id = canonical_mailbox_token(
        secret_material,
        &peer.endpoint_id,
        &peer.endpoint_kind,
        current_mailbox_rotation_epoch()?,
    )?;
    let context = SecureMeshContentContext::new(
        &envelope_id,
        &message_id,
        &opaque_mailbox_id,
        &endpoint.endpoint_id,
        &peer.endpoint_id,
        session_id(config)?,
        &created_at,
        &expires_at,
    );
    let body = serde_json::to_vec(payload)?;
    let envelope = pairwise_operation.session.seal_payload_envelope(
        &context,
        &SecureMeshPlaintext::new(kind, body).with_content_type("application/json"),
    )?;
    if commit {
        pairwise_operation.commit()?;
    }
    serde_json::from_str(&envelope.to_json()?)
        .context("mobile relay secure envelope serialization failed")
}

#[cfg(test)]
pub(in crate::domain::mobile_relay) fn open_mobile_relay_payload(
    config: &Value,
    secret_material: &RuntimeSecretMaterial,
    envelope: &Value,
    kind: SecureMeshPayloadKind,
) -> Result<Vec<u8>> {
    let mut pairwise_operation = mobile_relay_pairwise_operation(
        config,
        secret_material,
        "Mobile Relay pairwise payload authorization batch",
        3,
    )?;
    open_mobile_relay_payload_with_pairwise_operation(
        config,
        secret_material,
        envelope,
        kind,
        &mut pairwise_operation,
    )
}

pub(in crate::domain::mobile_relay) fn open_mobile_relay_payload_with_pairwise_operation(
    config: &Value,
    secret_material: &RuntimeSecretMaterial,
    envelope: &Value,
    kind: SecureMeshPayloadKind,
    pairwise_operation: &mut MobileRelayPairwiseOperation,
) -> Result<Vec<u8>> {
    open_mobile_relay_payload_with_pairwise_operation_and_gate_and_commit(
        config,
        secret_material,
        envelope,
        kind,
        pairwise_operation,
        PairwiseDirectoryGate::Required,
        true,
    )
}

pub(in crate::domain::mobile_relay) fn open_mobile_relay_payload_deferred(
    config: &Value,
    secret_material: &RuntimeSecretMaterial,
    envelope: &Value,
    kind: SecureMeshPayloadKind,
    pairwise_operation: &mut MobileRelayPairwiseOperation,
) -> Result<Vec<u8>> {
    open_mobile_relay_payload_with_pairwise_operation_and_gate_and_commit(
        config,
        secret_material,
        envelope,
        kind,
        pairwise_operation,
        PairwiseDirectoryGate::Required,
        false,
    )
}

pub(in crate::domain::mobile_relay) fn open_mobile_relay_payload_with_pairwise_operation_and_gate(
    config: &Value,
    _secret_material: &RuntimeSecretMaterial,
    envelope: &Value,
    kind: SecureMeshPayloadKind,
    pairwise_operation: &mut MobileRelayPairwiseOperation,
    directory_gate: PairwiseDirectoryGate,
) -> Result<Vec<u8>> {
    open_mobile_relay_payload_with_pairwise_operation_and_gate_and_commit(
        config,
        _secret_material,
        envelope,
        kind,
        pairwise_operation,
        directory_gate,
        true,
    )
}

fn open_mobile_relay_payload_with_pairwise_operation_and_gate_and_commit(
    config: &Value,
    _secret_material: &RuntimeSecretMaterial,
    envelope: &Value,
    kind: SecureMeshPayloadKind,
    pairwise_operation: &mut MobileRelayPairwiseOperation,
    directory_gate: PairwiseDirectoryGate,
    commit: bool,
) -> Result<Vec<u8>> {
    ensure_secure_mesh_protected_operation_allowed()?;
    let payload_kind = protected_send_kind_from_payload(kind);
    let _authorization = match directory_gate {
        PairwiseDirectoryGate::Required => {
            ensure_peer_authorized_for_protected_send(config, payload_kind)?
        }
        PairwiseDirectoryGate::KtGossipControl => {
            ensure_peer_trust_authorized_for_protected_send(config, payload_kind)?
        }
    };
    validate_secure_envelope(envelope)?;
    let wire = serde_json::to_string(envelope)
        .context("mobile relay secure envelope serialization failed")?;
    let pairwise_envelope = LicoArcRelayEnvelope::from_json(&wire)?;
    let opened = pairwise_operation
        .session
        .open_payload_envelope(&pairwise_envelope, kind)?;
    if commit {
        pairwise_operation.commit()?;
    }
    Ok(opened.body)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::domain::mobile_relay) enum PairwiseDirectoryGate {
    Required,
    KtGossipControl,
}
