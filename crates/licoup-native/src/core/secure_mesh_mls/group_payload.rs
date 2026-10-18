use anyhow::{Result, anyhow, ensure};
use openmls::prelude::LeafNodeIndex;
use openmls_traits::OpenMlsProvider;
use zeroize::Zeroize;

use crate::core::secure_mesh_crypto::{
    ContentKey, OpenedSecureMeshPayload, SecureMeshContentContext, SecureMeshPayloadKind,
    SecureMeshPlaintext, open_private_context_payload, seal_private_context_payload,
};
use crate::core::secure_mesh_mls_pq_epoch::mix_mlkem1024_payload_key;

use super::codec::deserialize_protocol_message;
use super::constants::{
    MLS_PAYLOAD_CONTENT_KEY_LEN, MLS_PAYLOAD_EXPORT_LABEL, SECURE_MESH_MLS_APPLICATION_PUBLIC_AAD,
};
use super::group_model::SecureMeshMlsGroup;
use super::participant::SecureMeshMlsParticipant;
use super::private_context_codec::{
    build_group_payload_export_context, decode_mls_private_context_payload,
    encode_mls_private_context_payload,
};

impl SecureMeshMlsGroup {
    pub(crate) fn derive_group_payload_content_key(
        &self,
        participant: &SecureMeshMlsParticipant,
    ) -> Result<ContentKey> {
        ensure!(
            self.is_active(),
            "secure mesh MLS inactive member cannot derive the current epoch payload key"
        );
        self.require_active_capability_negotiation()?;
        let export_context = build_group_payload_export_context(self)?;
        // `ExportedSecret` zeroizes the inner secret on drop.
        let exported = self
            .group
            .export_secret(
                participant.provider.crypto(),
                MLS_PAYLOAD_EXPORT_LABEL,
                &export_context,
                MLS_PAYLOAD_CONTENT_KEY_LEN,
            )
            .map_err(|error| {
                anyhow!("secure mesh MLS payload content-key export failed: {error:?}")
            })?;
        ensure!(
            exported.as_slice().len() == MLS_PAYLOAD_CONTENT_KEY_LEN,
            "secure mesh MLS payload content-key export length is invalid"
        );
        let pq_epoch_secret = self.mlkem1024_epoch_secret.as_ref().ok_or_else(|| {
            anyhow!("secure mesh MLS ML-KEM-1024 epoch secret is unavailable; re-pair required")
        })?;
        let mut fixed =
            mix_mlkem1024_payload_key(exported.as_slice(), pq_epoch_secret, &export_context)?;
        let content_key = ContentKey::from_bytes(*fixed);
        fixed.zeroize();
        Ok(content_key)
    }

    pub(crate) fn seal_payload_message(
        &mut self,
        sender: &SecureMeshMlsParticipant,
        context: &SecureMeshContentContext,
        plaintext: &SecureMeshPlaintext,
    ) -> Result<Vec<u8>> {
        self.require_active_capability_negotiation()?;
        let content_key = self.derive_group_payload_content_key(sender)?;
        let sealed = seal_private_context_payload(&content_key, context, plaintext)?;
        let encoded = encode_mls_private_context_payload(&sealed)?;
        self.seal_application_message(sender, SECURE_MESH_MLS_APPLICATION_PUBLIC_AAD, &encoded)
    }

    #[cfg(test)]
    pub(crate) fn open_payload_message(
        &mut self,
        receiver: &SecureMeshMlsParticipant,
        context: &SecureMeshContentContext,
        message: &[u8],
        expected_kind: SecureMeshPayloadKind,
    ) -> Result<OpenedSecureMeshPayload> {
        self.require_active_capability_negotiation()?;
        let encoded = self.open_application_message(
            receiver,
            SECURE_MESH_MLS_APPLICATION_PUBLIC_AAD,
            message,
        )?;
        self.open_and_validate_private_context_payload(receiver, context, expected_kind, &encoded)
    }

    pub(crate) fn open_payload_message_with_sender_verifier(
        &mut self,
        receiver: &SecureMeshMlsParticipant,
        context: &SecureMeshContentContext,
        message: &[u8],
        expected_kind: SecureMeshPayloadKind,
        verify_sender: impl FnOnce(&[u8], &[u8], LeafNodeIndex) -> Result<()>,
    ) -> Result<OpenedSecureMeshPayload> {
        self.require_active_capability_negotiation()?;
        let protocol_message = deserialize_protocol_message(
            message,
            "secure mesh MLS application message deserialization failed",
        )?;
        let encoded = self.open_application_message_with_sender_verifier(
            receiver,
            SECURE_MESH_MLS_APPLICATION_PUBLIC_AAD,
            protocol_message,
            verify_sender,
        )?;
        self.open_and_validate_private_context_payload(receiver, context, expected_kind, &encoded)
    }

    fn open_and_validate_private_context_payload(
        &self,
        receiver: &SecureMeshMlsParticipant,
        expected_context: &SecureMeshContentContext,
        expected_kind: SecureMeshPayloadKind,
        encoded: &[u8],
    ) -> Result<OpenedSecureMeshPayload> {
        let sealed = decode_mls_private_context_payload(encoded)?;
        let content_key = self.derive_group_payload_content_key(receiver)?;
        let opened = open_private_context_payload(&content_key, &sealed)?;
        let (actual_context, payload) = opened.into_parts();
        ensure!(
            actual_context == *expected_context,
            "secure mesh MLS encrypted inner context mismatch"
        );
        ensure!(
            payload.kind == expected_kind,
            "secure mesh MLS encrypted inner payload kind mismatch"
        );
        Ok(payload)
    }
}
