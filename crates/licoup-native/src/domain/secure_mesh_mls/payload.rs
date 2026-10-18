use anyhow::Result;
use serde_json::{Value, json};

use crate::core::secure_mesh_crypto::SecureMeshPlaintext;
use crate::core::secure_mesh_mls_product::{
    open_product_payload_message, seal_product_payload_message,
};

use super::group_state::load_group_checked;
use super::input_codec::{
    MAX_GROUP_ID_BYTES, MAX_MLS_MESSAGE_BYTES, MAX_PAYLOAD_BYTES, PayloadOpenRequest,
    PayloadSealRequest, decode_base64url, encode_base64url, parse_params, parse_payload_kind,
    reject_caller_asserted_trust, trusted_roster,
};
use super::participant_runtime::{
    ParticipantRequirement, group_state_store, with_local_participant,
};

pub(super) fn payload_seal(params: &Value) -> Result<Value> {
    reject_caller_asserted_trust(params)?;
    let request: PayloadSealRequest = parse_params(params)?;
    let group_id = decode_base64url(
        &request.group_id_base64url,
        "MLS group id",
        MAX_GROUP_ID_BYTES,
    )?;
    let context = request.context.to_context();
    let body = decode_base64url(
        &request.body_base64url,
        "MLS payload body",
        MAX_PAYLOAD_BYTES,
    )?;
    let mut plaintext = SecureMeshPlaintext::new(parse_payload_kind(&request.payload_kind)?, body);
    if let Some(content_type) = request.content_type.as_deref() {
        plaintext = plaintext.with_content_type(content_type);
    }
    with_local_participant(params, ParticipantRequirement::Required, |runtime| {
        let trusted_roster =
            trusted_roster(&request.trusted_roster, runtime.config, runtime.identity)?;
        let mut group = load_group_checked(
            group_state_store(&mut *runtime.group_store)?,
            runtime.participant,
            runtime.identity,
            &group_id,
        )?;
        let sender_state = trusted_roster.state_for(runtime.identity)?;
        let message = seal_product_payload_message(
            &mut group,
            runtime.participant,
            runtime.identity,
            sender_state,
            &trusted_roster.identities,
            &context,
            &plaintext,
        )?;
        Ok((
            json!({
                "ok": true,
                "messageBase64url": encode_base64url(&message),
                "payloadKind": plaintext.kind.as_str(),
                "bodyRedacted": true
            }),
            true,
        ))
    })
}

pub(super) fn payload_open(params: &Value) -> Result<Value> {
    reject_caller_asserted_trust(params)?;
    let request: PayloadOpenRequest = parse_params(params)?;
    let group_id = decode_base64url(
        &request.group_id_base64url,
        "MLS group id",
        MAX_GROUP_ID_BYTES,
    )?;
    let message = decode_base64url(
        &request.message_base64url,
        "MLS application message",
        MAX_MLS_MESSAGE_BYTES,
    )?;
    let trusted_sender_identity = request.trusted_sender_identity.to_identity()?;
    let context = request.context.to_context();
    let expected_kind = parse_payload_kind(&request.expected_payload_kind)?;
    with_local_participant(params, ParticipantRequirement::Required, |runtime| {
        let trusted_roster =
            trusted_roster(&request.trusted_roster, runtime.config, runtime.identity)?;
        let trusted_sender_state = trusted_roster.state_for(&trusted_sender_identity)?.clone();
        let mut group = load_group_checked(
            group_state_store(&mut *runtime.group_store)?,
            runtime.participant,
            runtime.identity,
            &group_id,
        )?;
        let opened = open_product_payload_message(
            &mut group,
            runtime.participant,
            runtime.identity,
            &trusted_sender_identity,
            &trusted_sender_state,
            &trusted_roster.identities,
            &context,
            &message,
            expected_kind,
        )?;
        Ok((
            json!({
                "ok": true,
                "payloadKind": opened.kind.as_str(),
                "bodyBase64url": encode_base64url(&opened.body),
                "contentType": opened.content_type,
                "createdAt": opened.created_at,
                "expiresAt": opened.expires_at
            }),
            true,
        ))
    })
}
