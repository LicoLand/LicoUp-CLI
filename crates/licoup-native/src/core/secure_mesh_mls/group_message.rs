use std::panic::{AssertUnwindSafe, catch_unwind};

use anyhow::{Context, Result, anyhow, ensure};
use openmls::prelude::{
    LeafNodeIndex, MlsMessageIn, ProcessedMessageContent, ProtocolMessage,
    tls_codec::Deserialize as TlsDeserialize,
};

use super::group_model::SecureMeshMlsGroup;
use super::participant::SecureMeshMlsParticipant;

impl SecureMeshMlsGroup {
    pub(crate) fn seal_application_message(
        &mut self,
        sender: &SecureMeshMlsParticipant,
        aad: &[u8],
        plaintext: &[u8],
    ) -> Result<Vec<u8>> {
        self.group.set_aad(aad.to_vec());
        let message = self
            .group
            .create_message(&sender.provider, &sender.signer, plaintext)
            .map_err(|error| {
                anyhow!("secure mesh MLS application message seal failed: {error:?}")
            })?;
        message.to_bytes().map_err(|error| {
            anyhow!("secure mesh MLS application message serialization failed: {error:?}")
        })
    }

    #[cfg(test)]
    pub(crate) fn open_application_message(
        &mut self,
        receiver: &SecureMeshMlsParticipant,
        aad: &[u8],
        message: &[u8],
    ) -> Result<Vec<u8>> {
        self.open_application_message_for_runtime_crypto_self_test(receiver, aad, message)
    }

    pub(super) fn open_application_message_for_runtime_crypto_self_test(
        &mut self,
        receiver: &SecureMeshMlsParticipant,
        aad: &[u8],
        message: &[u8],
    ) -> Result<Vec<u8>> {
        let protocol_message: ProtocolMessage = MlsMessageIn::tls_deserialize_exact(message)
            .context("secure mesh MLS application message deserialization failed")?
            .try_into_protocol_message()
            .map_err(|_| {
                anyhow!("secure mesh MLS message is not an application protocol message")
            })?;
        self.open_application_message_with_sender_verifier(
            receiver,
            aad,
            protocol_message,
            |_, _, _| Ok(()),
        )
    }

    pub(crate) fn open_application_message_with_sender_verifier(
        &mut self,
        receiver: &SecureMeshMlsParticipant,
        aad: &[u8],
        protocol_message: ProtocolMessage,
        verify_sender: impl FnOnce(&[u8], &[u8], LeafNodeIndex) -> Result<()>,
    ) -> Result<Vec<u8>> {
        let processed = catch_unwind(AssertUnwindSafe(|| {
            self.group
                .process_message(&receiver.provider, protocol_message)
        }))
        .map_err(|_| anyhow!("secure mesh MLS application message rejected"))?
        .map_err(|error| anyhow!("secure mesh MLS application message open failed: {error:?}"))?;
        ensure!(
            processed.aad() == aad,
            "secure mesh MLS application message AAD mismatch"
        );
        let (credential_identity, signing_public_key, leaf_index) =
            self.authenticated_member_sender(&processed)?;
        verify_sender(&credential_identity, &signing_public_key, leaf_index)?;
        match processed.into_content() {
            ProcessedMessageContent::ApplicationMessage(application_message) => {
                Ok(application_message.into_bytes())
            }
            _ => Err(anyhow!(
                "secure mesh MLS message did not contain application data"
            )),
        }
    }
}
