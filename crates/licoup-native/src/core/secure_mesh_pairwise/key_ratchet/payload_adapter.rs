use anyhow::{Result, ensure};

use super::super::codec::{
    SecureMeshPairwiseMessage, combine_pairwise_and_extra_aad, message_replay_fingerprint,
    sparse_pq_header_bytes,
};
use super::super::support::{
    MAX_CIPHERTEXT_BYTES, MAX_CONTENT_ENCRYPTED_HEADER_BYTES, SECURE_MESH_PAIRWISE_CIPHER_SUITE,
    encoded_len_limit, validate_message_id,
};
use super::{SecureMeshPairwiseSession, advance_chain};
use crate::core::secure_mesh::SECURE_MESH_PROTOCOL_VERSION;
use crate::core::secure_mesh_crypto::{
    ContentKey, OpenedSecureMeshPayload, SECURE_MESH_CONTENT_CIPHER_SUITE, SealedSecureMeshPayload,
    SecureMeshContentContext, SecureMeshPayloadKind, SecureMeshPlaintext,
};
use crate::core::secure_mesh_sparse_pq_ratchet::derive_hybrid_message_key;

impl SecureMeshPairwiseSession {
    pub fn seal_payload(
        &mut self,
        context: &SecureMeshContentContext,
        plaintext: &SecureMeshPlaintext,
    ) -> Result<SecureMeshPairwiseMessage> {
        self.seal_payload_with_extra_aad(context, plaintext, &[])
    }

    pub fn seal_payload_with_extra_aad(
        &mut self,
        context: &SecureMeshContentContext,
        plaintext: &SecureMeshPlaintext,
        extra_aad: &[u8],
    ) -> Result<SecureMeshPairwiseMessage> {
        ensure!(!self.revoked, "secure mesh pairwise session is revoked");
        self.require_capability_negotiation()?;
        ensure!(
            self.initiator_key_confirmed,
            "secure mesh pairwise handshake key confirmation is incomplete"
        );
        ensure_pairwise_context_for_send(self, context)?;
        let mut candidate = self.try_clone()?;
        candidate.prepare_sending_ratchet_for_send()?;
        let chain_index = candidate.sending_chain_index;
        let (next_chain_key, classical_message_key) = advance_chain(
            &candidate.sending_chain_key,
            candidate.dh_epoch,
            chain_index,
            "message",
        )?;
        let sparse_pq = candidate.sparse_pq_ratchet.send_key()?;
        let message_key = derive_hybrid_message_key(
            &classical_message_key,
            &sparse_pq.message_key,
            candidate.session_id.as_bytes(),
        )?;
        let content_key = ContentKey::from_bytes(*message_key);
        let mut message = SecureMeshPairwiseMessage {
            protocol_version: SECURE_MESH_PROTOCOL_VERSION.to_string(),
            cipher_suite: SECURE_MESH_PAIRWISE_CIPHER_SUITE.to_string(),
            session_id: candidate.session_id.clone(),
            message_id: context.message_id.clone(),
            sender_endpoint_id: candidate.local_endpoint_id.clone(),
            recipient_endpoint_id: candidate.remote_endpoint_id.clone(),
            dh_epoch: candidate.dh_epoch,
            chain_index,
            previous_chain_length: candidate.previous_chain_length,
            sender_ratchet_public_key: candidate.local_ratchet_public_key.to_vec(),
            sparse_pq_header: sparse_pq.header,
            encrypted_header: String::new(),
            ciphertext: String::new(),
            ciphertext_size: 0,
        };
        let combined_aad = combine_pairwise_and_extra_aad(&message, extra_aad)?;
        let sealed = crate::core::secure_mesh_crypto::seal_payload_with_aad_binding(
            &content_key,
            context,
            plaintext,
            &combined_aad,
        )?;
        message.encrypted_header = sealed.encrypted_header;
        message.ciphertext = sealed.ciphertext;
        message.ciphertext_size = sealed.ciphertext_size;
        *candidate.sending_chain_key = *next_chain_key;
        candidate.sending_chain_index += 1;
        *self = candidate;
        Ok(message)
    }

    pub fn open_payload(
        &mut self,
        context: &SecureMeshContentContext,
        message: &SecureMeshPairwiseMessage,
        expected_kind: SecureMeshPayloadKind,
    ) -> Result<OpenedSecureMeshPayload> {
        self.open_payload_with_extra_aad(context, message, expected_kind, &[])
    }

    pub fn open_payload_with_extra_aad(
        &mut self,
        context: &SecureMeshContentContext,
        message: &SecureMeshPairwiseMessage,
        expected_kind: SecureMeshPayloadKind,
        extra_aad: &[u8],
    ) -> Result<OpenedSecureMeshPayload> {
        ensure!(!self.revoked, "secure mesh pairwise session is revoked");
        self.require_capability_negotiation()?;
        ensure!(
            self.initiator_key_confirmed,
            "secure mesh pairwise handshake key confirmation is incomplete"
        );
        ensure_message_for_session(self, message)?;
        ensure_pairwise_context_for_open(self, context, message)?;
        let replay_fingerprint = message_replay_fingerprint(message)?;
        ensure!(
            !self
                .received_message_ids
                .iter()
                .any(|id| id == &replay_fingerprint),
            "secure mesh pairwise message replay detected"
        );
        let mut candidate = self.try_clone()?;
        let classical_message_key = candidate.message_key_for_open(message)?;
        let post_quantum_message_key = candidate
            .sparse_pq_ratchet
            .receive_key(&message.sparse_pq_header)?;
        let message_key = derive_hybrid_message_key(
            &classical_message_key,
            &post_quantum_message_key,
            candidate.session_id.as_bytes(),
        )?;
        let content_key = ContentKey::from_bytes(*message_key);
        let combined_aad = combine_pairwise_and_extra_aad(message, extra_aad)?;
        let sealed = SealedSecureMeshPayload {
            protocol_version: SECURE_MESH_PROTOCOL_VERSION.to_string(),
            cipher_suite: SECURE_MESH_CONTENT_CIPHER_SUITE.to_string(),
            encrypted_header: message.encrypted_header.clone(),
            ciphertext: message.ciphertext.clone(),
            ciphertext_size: message.ciphertext_size,
        };
        let opened = crate::core::secure_mesh_crypto::open_payload_with_aad_binding(
            &content_key,
            context,
            &sealed,
            expected_kind,
            &combined_aad,
        )?;
        candidate.record_received_message_id(replay_fingerprint);
        *self = candidate;
        Ok(opened)
    }
}

pub(super) fn ensure_message_for_session(
    session: &SecureMeshPairwiseSession,
    message: &SecureMeshPairwiseMessage,
) -> Result<()> {
    ensure!(
        message.protocol_version == SECURE_MESH_PROTOCOL_VERSION
            && message.cipher_suite == SECURE_MESH_PAIRWISE_CIPHER_SUITE,
        "secure mesh pairwise message protocol is unsupported"
    );
    ensure!(
        message.session_id == session.session_id,
        "secure mesh pairwise message session mismatch"
    );
    ensure!(
        message.sender_endpoint_id == session.remote_endpoint_id
            && message.recipient_endpoint_id == session.local_endpoint_id,
        "secure mesh pairwise message endpoint mismatch"
    );
    validate_message_id(&message.message_id)?;
    ensure!(
        message.ciphertext_size > 0 && message.ciphertext_size <= MAX_CIPHERTEXT_BYTES,
        "secure mesh pairwise ciphertext size is outside bounds"
    );
    ensure!(
        message.ciphertext.len() <= encoded_len_limit(MAX_CIPHERTEXT_BYTES),
        "secure mesh pairwise encoded ciphertext is too large"
    );
    ensure!(
        message.encrypted_header.len() <= encoded_len_limit(MAX_CONTENT_ENCRYPTED_HEADER_BYTES),
        "secure mesh pairwise encrypted header is too large"
    );
    sparse_pq_header_bytes(&message.sparse_pq_header)?;
    Ok(())
}

pub(super) fn ensure_pairwise_context_for_send(
    session: &SecureMeshPairwiseSession,
    context: &SecureMeshContentContext,
) -> Result<()> {
    ensure!(
        context.session_id == session.session_id,
        "secure mesh pairwise payload context session mismatch"
    );
    ensure!(
        context.sender_endpoint_id == session.local_endpoint_id
            && context.recipient_endpoint_id == session.remote_endpoint_id,
        "secure mesh pairwise payload context endpoint mismatch"
    );
    validate_message_id(&context.message_id)?;
    Ok(())
}

pub(super) fn ensure_pairwise_context_for_open(
    session: &SecureMeshPairwiseSession,
    context: &SecureMeshContentContext,
    message: &SecureMeshPairwiseMessage,
) -> Result<()> {
    ensure!(
        context.session_id == message.session_id && context.session_id == session.session_id,
        "secure mesh pairwise payload context session mismatch"
    );
    ensure!(
        context.message_id == message.message_id,
        "secure mesh pairwise payload context message mismatch"
    );
    ensure!(
        context.sender_endpoint_id == message.sender_endpoint_id
            && context.recipient_endpoint_id == message.recipient_endpoint_id,
        "secure mesh pairwise payload context endpoint mismatch"
    );
    ensure!(
        context.sender_endpoint_id == session.remote_endpoint_id
            && context.recipient_endpoint_id == session.local_endpoint_id,
        "secure mesh pairwise payload context local endpoint mismatch"
    );
    Ok(())
}
