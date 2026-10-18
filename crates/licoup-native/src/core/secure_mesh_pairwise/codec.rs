use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::support::{
    MAX_SPARSE_PQ_HEADER_BYTES, MESSAGE_AAD_MAGIC, PAYLOAD_AAD_BINDING_MAGIC,
    append_len_prefixed_bytes, hash_bytes, parse_key_bytes, validate_message_id,
};
use crate::core::secure_mesh_sparse_pq_ratchet::SecureMeshSparsePqHeader;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshPairwiseMessage {
    pub protocol_version: String,
    pub cipher_suite: String,
    pub session_id: String,
    pub message_id: String,
    pub sender_endpoint_id: String,
    pub recipient_endpoint_id: String,
    pub dh_epoch: u64,
    pub chain_index: u64,
    pub previous_chain_length: u64,
    pub sender_ratchet_public_key: Vec<u8>,
    pub sparse_pq_header: SecureMeshSparsePqHeader,
    pub encrypted_header: String,
    pub ciphertext: String,
    pub ciphertext_size: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct SecureMeshPairwisePrivateRelayHeader {
    pub(super) protocol_version: String,
    pub(super) cipher_suite: String,
    pub(super) envelope_id: String,
    pub(super) mailbox_id: String,
    pub(super) message_id: String,
    pub(super) session_id: String,
    pub(super) sender_endpoint_id: String,
    pub(super) recipient_endpoint_id: String,
    pub(super) created_at: String,
    pub(super) expires_at: String,
    pub(super) dh_epoch: u64,
    pub(super) chain_index: u64,
    pub(super) previous_chain_length: u64,
    pub(super) sender_ratchet_public_key: String,
    pub(super) sparse_pq_header: SecureMeshSparsePqHeader,
    pub(super) content_encrypted_header: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenedPairwiseMessage {
    pub message_id: String,
    pub sender_endpoint_id: String,
    pub body: Vec<u8>,
}

pub(super) fn message_aad(message: &SecureMeshPairwiseMessage) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(MESSAGE_AAD_MAGIC);
    append_len_prefixed_bytes(&mut out, message.protocol_version.as_bytes())?;
    append_len_prefixed_bytes(&mut out, message.cipher_suite.as_bytes())?;
    append_len_prefixed_bytes(&mut out, message.session_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, message.message_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, message.sender_endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, message.recipient_endpoint_id.as_bytes())?;
    out.extend_from_slice(&message.dh_epoch.to_be_bytes());
    out.extend_from_slice(&message.chain_index.to_be_bytes());
    out.extend_from_slice(&message.previous_chain_length.to_be_bytes());
    append_len_prefixed_bytes(&mut out, &message.sender_ratchet_public_key)?;
    append_len_prefixed_bytes(
        &mut out,
        &sparse_pq_header_bytes(&message.sparse_pq_header)?,
    )?;
    append_len_prefixed_bytes(&mut out, message.encrypted_header.as_bytes())?;
    Ok(out)
}

pub(super) fn pairwise_payload_aad_binding(message: &SecureMeshPairwiseMessage) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(PAYLOAD_AAD_BINDING_MAGIC);
    append_len_prefixed_bytes(&mut out, message.protocol_version.as_bytes())?;
    append_len_prefixed_bytes(&mut out, message.cipher_suite.as_bytes())?;
    append_len_prefixed_bytes(&mut out, message.session_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, message.message_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, message.sender_endpoint_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, message.recipient_endpoint_id.as_bytes())?;
    out.extend_from_slice(&message.dh_epoch.to_be_bytes());
    out.extend_from_slice(&message.chain_index.to_be_bytes());
    out.extend_from_slice(&message.previous_chain_length.to_be_bytes());
    append_len_prefixed_bytes(&mut out, &message.sender_ratchet_public_key)?;
    append_len_prefixed_bytes(
        &mut out,
        &sparse_pq_header_bytes(&message.sparse_pq_header)?,
    )?;
    Ok(out)
}

pub(super) fn sparse_pq_header_bytes(header: &SecureMeshSparsePqHeader) -> Result<Vec<u8>> {
    ensure!(
        header.message_number > 0,
        "secure mesh pairwise sparse PQ message number is invalid"
    );
    let encoded = serde_json::to_vec(header)
        .context("secure mesh pairwise sparse PQ header serialization failed")?;
    ensure!(
        encoded.len() <= MAX_SPARSE_PQ_HEADER_BYTES,
        "secure mesh pairwise sparse PQ header is too large"
    );
    Ok(encoded)
}

pub(super) fn combine_pairwise_and_extra_aad(
    message: &SecureMeshPairwiseMessage,
    extra_aad: &[u8],
) -> Result<Vec<u8>> {
    let mut out = pairwise_payload_aad_binding(message)?;
    if !extra_aad.is_empty() {
        out.extend_from_slice(extra_aad);
    }
    Ok(out)
}

pub(super) fn message_replay_fingerprint(message: &SecureMeshPairwiseMessage) -> Result<String> {
    validate_message_id(&message.message_id)?;
    let sender_ratchet_public_key = parse_key_bytes(
        &message.sender_ratchet_public_key,
        "replay sender ratchet public key",
    )?;
    let ciphertext_hash = hash_bytes(message.ciphertext.as_bytes());
    let mut out = Vec::new();
    append_len_prefixed_bytes(&mut out, message.session_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, &sender_ratchet_public_key)?;
    out.extend_from_slice(&message.dh_epoch.to_be_bytes());
    out.extend_from_slice(&message.chain_index.to_be_bytes());
    append_len_prefixed_bytes(&mut out, message.message_id.as_bytes())?;
    append_len_prefixed_bytes(&mut out, ciphertext_hash.as_bytes())?;
    Ok(hash_bytes(&out))
}
