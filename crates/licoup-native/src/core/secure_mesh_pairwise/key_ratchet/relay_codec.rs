use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose};
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

use super::super::codec::{SecureMeshPairwiseMessage, SecureMeshPairwisePrivateRelayHeader};
use super::super::support::{SECURE_MESH_PAIRWISE_CIPHER_SUITE, parse_key_bytes};
use super::SecureMeshPairwiseSession;
use crate::core::secure_mesh::SECURE_MESH_PROTOCOL_VERSION;
use crate::core::secure_mesh_crypto::{
    OpenedSecureMeshPayload, SecureMeshContentContext, SecureMeshPayloadKind, SecureMeshPlaintext,
};
use licoup_protocol_bindings::licoarc_relay::{
    LicoArcRelayEnvelope, LicoArcRelayEnvelopeDraft, open_private_relay_header,
    seal_private_relay_header,
};

const MAX_RELAY_PAYLOAD_LIFETIME_SECONDS: i64 = 10 * 60;
const MAX_RELAY_PAYLOAD_CLOCK_SKEW_SECONDS: i64 = 5 * 60;

impl SecureMeshPairwiseSession {
    pub fn seal_payload_envelope(
        &mut self,
        context: &SecureMeshContentContext,
        plaintext: &SecureMeshPlaintext,
    ) -> Result<LicoArcRelayEnvelope> {
        self.seal_payload_envelope_with_extra_aad(context, plaintext, &[])
    }

    pub fn seal_payload_envelope_with_extra_aad(
        &mut self,
        context: &SecureMeshContentContext,
        plaintext: &SecureMeshPlaintext,
        extra_aad: &[u8],
    ) -> Result<LicoArcRelayEnvelope> {
        let mut candidate = self.try_clone()?;
        let message = candidate.seal_payload_with_extra_aad(context, plaintext, extra_aad)?;
        let envelope = relay_envelope_from_pairwise_message(&candidate, context, &message)?;
        *self = candidate;
        Ok(envelope)
    }

    pub fn open_payload_envelope(
        &mut self,
        envelope: &LicoArcRelayEnvelope,
        expected_kind: SecureMeshPayloadKind,
    ) -> Result<OpenedSecureMeshPayload> {
        self.open_payload_envelope_with_extra_aad(envelope, expected_kind, &[])
    }

    pub fn open_payload_envelope_with_extra_aad(
        &mut self,
        envelope: &LicoArcRelayEnvelope,
        expected_kind: SecureMeshPayloadKind,
        extra_aad: &[u8],
    ) -> Result<OpenedSecureMeshPayload> {
        self.open_payload_envelope_with_extra_aad_at(
            envelope,
            expected_kind,
            extra_aad,
            OffsetDateTime::now_utc(),
        )
    }

    fn open_payload_envelope_with_extra_aad_at(
        &mut self,
        envelope: &LicoArcRelayEnvelope,
        expected_kind: SecureMeshPayloadKind,
        extra_aad: &[u8],
        now: OffsetDateTime,
    ) -> Result<OpenedSecureMeshPayload> {
        // Reject revoked sessions before attempting header-key selection. This
        // keeps the public failure semantic stable and avoids doing any
        // attacker-controlled envelope work after local revocation.
        ensure!(!self.revoked, "secure mesh pairwise session is revoked");
        let (context, message) = pairwise_message_from_relay_envelope(self, envelope)?;
        validate_relay_payload_freshness(&context, now)?;
        self.open_payload_with_extra_aad(&context, &message, expected_kind, extra_aad)
    }

    #[cfg(test)]
    pub(in crate::core::secure_mesh_pairwise) fn open_payload_envelope_at(
        &mut self,
        envelope: &LicoArcRelayEnvelope,
        expected_kind: SecureMeshPayloadKind,
        now: OffsetDateTime,
    ) -> Result<OpenedSecureMeshPayload> {
        self.open_payload_envelope_with_extra_aad_at(envelope, expected_kind, &[], now)
    }
}

fn validate_relay_payload_freshness(
    context: &SecureMeshContentContext,
    now: OffsetDateTime,
) -> Result<()> {
    let created_at = OffsetDateTime::parse(&context.created_at, &Rfc3339)
        .context("secure mesh pairwise relay createdAt is invalid")?;
    let expires_at = OffsetDateTime::parse(&context.expires_at, &Rfc3339)
        .context("secure mesh pairwise relay expiresAt is invalid")?;
    ensure!(
        created_at <= now + Duration::seconds(MAX_RELAY_PAYLOAD_CLOCK_SKEW_SECONDS),
        "secure mesh pairwise relay createdAt exceeds the clock-skew allowance"
    );
    ensure!(
        expires_at > now,
        "secure mesh pairwise relay payload is expired"
    );
    ensure!(
        expires_at > created_at
            && expires_at - created_at <= Duration::seconds(MAX_RELAY_PAYLOAD_LIFETIME_SECONDS),
        "secure mesh pairwise relay payload lifetime is invalid"
    );
    Ok(())
}

pub(super) fn relay_envelope_from_pairwise_message(
    session: &SecureMeshPairwiseSession,
    context: &SecureMeshContentContext,
    message: &SecureMeshPairwiseMessage,
) -> Result<LicoArcRelayEnvelope> {
    ensure!(
        context.message_id == message.message_id
            && context.session_id == message.session_id
            && context.sender_endpoint_id == message.sender_endpoint_id
            && context.recipient_endpoint_id == message.recipient_endpoint_id,
        "secure mesh pairwise relay context does not match message"
    );
    let draft = LicoArcRelayEnvelopeDraft::from_contract_fields(
        &context.opaque_mailbox_id,
        &context.envelope_id,
        &context.expires_at,
        message.ciphertext_size,
    )?;
    let private_header = SecureMeshPairwisePrivateRelayHeader {
        protocol_version: SECURE_MESH_PROTOCOL_VERSION.to_string(),
        cipher_suite: SECURE_MESH_PAIRWISE_CIPHER_SUITE.to_string(),
        envelope_id: context.envelope_id.clone(),
        mailbox_id: context.opaque_mailbox_id.clone(),
        message_id: context.message_id.clone(),
        session_id: context.session_id.clone(),
        sender_endpoint_id: context.sender_endpoint_id.clone(),
        recipient_endpoint_id: context.recipient_endpoint_id.clone(),
        created_at: context.created_at.clone(),
        expires_at: context.expires_at.clone(),
        dh_epoch: message.dh_epoch,
        chain_index: message.chain_index,
        previous_chain_length: message.previous_chain_length,
        sender_ratchet_public_key: general_purpose::URL_SAFE_NO_PAD
            .encode(&message.sender_ratchet_public_key),
        sparse_pq_header: message.sparse_pq_header.clone(),
        content_encrypted_header: message.encrypted_header.clone(),
    };
    let private_header = serde_json::to_vec(&private_header)
        .context("secure mesh pairwise private relay header serialization failed")?;
    let encrypted_header =
        seal_private_relay_header(&draft, session.sending_header_key.as_ref(), &private_header)?;
    let ciphertext = general_purpose::URL_SAFE_NO_PAD
        .decode(&message.ciphertext)
        .context("secure mesh pairwise ciphertext is not base64url")?;
    ensure!(
        ciphertext.len() == message.ciphertext_size,
        "secure mesh pairwise ciphertext size mismatch"
    );
    draft.finish(&encrypted_header, &ciphertext)
}

pub(super) fn pairwise_message_from_relay_envelope(
    session: &SecureMeshPairwiseSession,
    envelope: &LicoArcRelayEnvelope,
) -> Result<(SecureMeshContentContext, SecureMeshPairwiseMessage)> {
    let private_header = open_private_relay_header(
        envelope,
        std::iter::once(session.receiving_header_key.as_ref())
            .chain(std::iter::once(session.next_receiving_header_key.as_ref()))
            .chain(
                session
                    .skipped_receiving_header_keys
                    .iter()
                    .rev()
                    .map(AsRef::as_ref),
            ),
    )?;
    let header: SecureMeshPairwisePrivateRelayHeader = serde_json::from_slice(&private_header)
        .context("secure mesh pairwise private relay header is invalid")?;
    ensure!(
        header.protocol_version == SECURE_MESH_PROTOCOL_VERSION
            && header.cipher_suite == SECURE_MESH_PAIRWISE_CIPHER_SUITE,
        "secure mesh pairwise private relay protocol is unsupported"
    );
    ensure!(
        header.envelope_id == envelope.envelope_id()
            && header.mailbox_id == envelope.mailbox_id()
            && header.expires_at == envelope.expires_at(),
        "secure mesh pairwise private relay routing binding mismatch"
    );
    ensure!(
        header.session_id == session.session_id
            && header.sender_endpoint_id == session.remote_endpoint_id
            && header.recipient_endpoint_id == session.local_endpoint_id,
        "secure mesh pairwise private relay receiver binding mismatch"
    );
    let sender_ratchet_public_key = general_purpose::URL_SAFE_NO_PAD
        .decode(&header.sender_ratchet_public_key)
        .context("secure mesh pairwise ratchet public key is not base64url")?;
    parse_key_bytes(
        &sender_ratchet_public_key,
        "relay sender ratchet public key",
    )?;
    let context = SecureMeshContentContext::new(
        &header.envelope_id,
        &header.message_id,
        &header.mailbox_id,
        &header.sender_endpoint_id,
        &header.recipient_endpoint_id,
        &header.session_id,
        &header.created_at,
        &header.expires_at,
    );
    let content_ciphertext = envelope.decoded_content_ciphertext()?;
    let ciphertext_size = content_ciphertext.len();
    let message = SecureMeshPairwiseMessage {
        protocol_version: SECURE_MESH_PROTOCOL_VERSION.to_string(),
        cipher_suite: SECURE_MESH_PAIRWISE_CIPHER_SUITE.to_string(),
        session_id: header.session_id,
        message_id: header.message_id,
        sender_endpoint_id: header.sender_endpoint_id,
        recipient_endpoint_id: header.recipient_endpoint_id,
        dh_epoch: header.dh_epoch,
        chain_index: header.chain_index,
        previous_chain_length: header.previous_chain_length,
        sender_ratchet_public_key,
        sparse_pq_header: header.sparse_pq_header,
        encrypted_header: header.content_encrypted_header,
        ciphertext: general_purpose::URL_SAFE_NO_PAD.encode(content_ciphertext),
        ciphertext_size,
    };
    Ok((context, message))
}
