use anyhow::Result;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use super::super::{
    key_ratchet::SkippedMessageKey,
    support::{decode_secret_32, encode_secret},
};
use super::public_snapshot::PersistedSkippedMessageKeyPublic;
use crate::core::secure_mesh_secret_store::{SecretStoreAuthorizationSession, SecretStoreHandle};

pub(super) struct PendingPairwiseSnapshot {
    pub(super) public_json: String,
    pub(super) secret_handle: SecretStoreHandle,
    pub(super) secret_store_session: SecretStoreAuthorizationSession,
}

#[derive(Serialize, Deserialize)]
pub(in crate::core::secure_mesh_pairwise) struct PersistedPairwiseSessionSecrets {
    pub(super) schema_version: u32,
    pub(super) state_version: u64,
    pub(super) session_id: String,
    pub(super) local_endpoint_id: String,
    pub(super) remote_endpoint_id: String,
    pub(super) public_snapshot_digest: String,
    pub(super) root_key: PairwiseSecretString,
    pub(super) sending_chain_key: PairwiseSecretString,
    pub(super) receiving_chain_key: PairwiseSecretString,
    pub(super) sending_header_key: PairwiseSecretString,
    pub(super) receiving_header_key: PairwiseSecretString,
    pub(super) next_sending_header_key: PairwiseSecretString,
    pub(super) next_receiving_header_key: PairwiseSecretString,
    pub(super) skipped_receiving_header_keys: Vec<PairwiseSecretString>,
    pub(super) local_ratchet_secret: PairwiseSecretString,
    pub(super) sparse_pq_ratchet: PairwiseSecretString,
    pub(super) skipped_keys: Vec<PersistedSkippedMessageKeySecret>,
}

#[derive(Serialize, Deserialize)]
#[serde(transparent)]
pub(super) struct PairwiseSecretString(String);

impl PairwiseSecretString {
    pub(super) fn new(value: String) -> Self {
        Self(value)
    }

    pub(super) fn as_str(&self) -> &str {
        &self.0
    }
}

impl Drop for PairwiseSecretString {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Serialize, Deserialize)]
pub(super) struct PersistedSkippedMessageKeySecret {
    pub(super) message_key: PairwiseSecretString,
}

impl From<&SkippedMessageKey> for PersistedSkippedMessageKeySecret {
    fn from(value: &SkippedMessageKey) -> Self {
        Self {
            message_key: PairwiseSecretString::new(encode_secret(&value.message_key)),
        }
    }
}

impl
    TryFrom<(
        PersistedSkippedMessageKeyPublic,
        &PersistedSkippedMessageKeySecret,
    )> for SkippedMessageKey
{
    type Error = anyhow::Error;

    fn try_from(
        value: (
            PersistedSkippedMessageKeyPublic,
            &PersistedSkippedMessageKeySecret,
        ),
    ) -> Result<Self> {
        let (public, secret) = value;
        Ok(Self {
            message_id: public.message_id,
            dh_epoch: public.dh_epoch,
            chain_index: public.chain_index,
            sender_ratchet_public_key: decode_secret_32(&public.sender_ratchet_public_key)?,
            message_key: Zeroizing::new(decode_secret_32(secret.message_key.as_str())?),
        })
    }
}
