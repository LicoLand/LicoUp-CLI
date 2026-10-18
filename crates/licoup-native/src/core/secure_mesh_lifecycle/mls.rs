use anyhow::Result;
use serde_json::Value;
use std::collections::BTreeMap;

use super::evaluate_service_action_json;
use super::projection::{decode_protected_projection, protected_plaintext};
use crate::core::secure_mesh_crypto::{
    OpenedSecureMeshPayload, SecureMeshContentContext, SecureMeshPlaintext,
};
use crate::core::secure_mesh_mls::{SecureMeshMlsGroup, SecureMeshMlsParticipant};
use crate::core::secure_mesh_mls_product::{
    open_product_payload_message, seal_product_payload_message,
};
use crate::core::secure_mesh_trust::{DeviceTrustPublicIdentity, DeviceTrustState};

pub fn seal_lifecycle_service_action_mls(
    group: &mut SecureMeshMlsGroup,
    sender: &SecureMeshMlsParticipant,
    sender_identity: &DeviceTrustPublicIdentity,
    sender_trust_state: &DeviceTrustState,
    trusted_roster: &BTreeMap<String, DeviceTrustPublicIdentity>,
    context: &SecureMeshContentContext,
    params: &Value,
) -> Result<Vec<u8>> {
    let plaintext = prepare_lifecycle_service_action(params)?;
    seal_product_payload_message(
        group,
        sender,
        sender_identity,
        sender_trust_state,
        trusted_roster,
        context,
        &plaintext,
    )
}

pub fn open_lifecycle_service_action_mls(
    group: &mut SecureMeshMlsGroup,
    receiver: &SecureMeshMlsParticipant,
    receiver_identity: &DeviceTrustPublicIdentity,
    trusted_sender_identity: &DeviceTrustPublicIdentity,
    trusted_sender_state: &DeviceTrustState,
    trusted_roster: &BTreeMap<String, DeviceTrustPublicIdentity>,
    context: &SecureMeshContentContext,
    message: &[u8],
) -> Result<(OpenedSecureMeshPayload, Value)> {
    let opened = open_product_payload_message(
        group,
        receiver,
        receiver_identity,
        trusted_sender_identity,
        trusted_sender_state,
        trusted_roster,
        context,
        message,
        crate::core::secure_mesh_crypto::SecureMeshPayloadKind::ServiceAction,
    )?;
    let value = decode_protected_projection(&opened, "MLS")?;
    Ok((opened, value))
}

pub(super) fn prepare_lifecycle_service_action(params: &Value) -> Result<SecureMeshPlaintext> {
    protected_plaintext(&evaluate_service_action_json(params)?)
}
