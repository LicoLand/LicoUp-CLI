use serde::{Deserialize, Serialize};

use super::super::{key_ratchet::SkippedMessageKey, support::encode_secret};
use crate::core::secure_mesh_capability_proof::{
    ClientCapabilityProjection, SignedCapabilityProof,
};
use crate::core::secure_mesh_session_negotiation::NegotiatedCapabilityBinding;

#[derive(Clone, Serialize, Deserialize)]
pub(in crate::core::secure_mesh_pairwise) struct PersistedPairwisePublicSession {
    pub(in crate::core::secure_mesh_pairwise) schema_version: u32,
    pub(in crate::core::secure_mesh_pairwise) state_version: u64,
    pub(in crate::core::secure_mesh_pairwise) secret_store_class: String,
    pub(in crate::core::secure_mesh_pairwise) secret_store_namespace: String,
    pub(in crate::core::secure_mesh_pairwise) secret_store_key: String,
    pub(in crate::core::secure_mesh_pairwise) session_id: String,
    pub(in crate::core::secure_mesh_pairwise) local_endpoint_id: String,
    pub(in crate::core::secure_mesh_pairwise) remote_endpoint_id: String,
    pub(in crate::core::secure_mesh_pairwise) role: String,
    pub(in crate::core::secure_mesh_pairwise) local_ratchet_public_key: String,
    pub(in crate::core::secure_mesh_pairwise) remote_ratchet_public_key: String,
    pub(in crate::core::secure_mesh_pairwise) handshake_transcript_hash: String,
    pub(in crate::core::secure_mesh_pairwise) dh_epoch: u64,
    pub(in crate::core::secure_mesh_pairwise) receiving_ratchet_epoch: u64,
    pub(in crate::core::secure_mesh_pairwise) sending_chain_index: u64,
    pub(in crate::core::secure_mesh_pairwise) receiving_chain_index: u64,
    pub(in crate::core::secure_mesh_pairwise) previous_chain_length: u64,
    pub(in crate::core::secure_mesh_pairwise) skipped_keys: Vec<PersistedSkippedMessageKeyPublic>,
    pub(in crate::core::secure_mesh_pairwise) received_message_ids: Vec<String>,
    #[serde(default)]
    pub(in crate::core::secure_mesh_pairwise) pending_sending_ratchet: bool,
    pub(in crate::core::secure_mesh_pairwise) initiator_key_confirmed: bool,
    pub(in crate::core::secure_mesh_pairwise) local_capability_proof: SignedCapabilityProof,
    pub(in crate::core::secure_mesh_pairwise) capability_binding:
        Option<NegotiatedCapabilityBinding>,
    pub(in crate::core::secure_mesh_pairwise) capability_projection:
        Option<ClientCapabilityProjection>,
    pub(in crate::core::secure_mesh_pairwise) revoked: bool,
}

#[derive(Clone, Serialize, Deserialize)]
pub(in crate::core::secure_mesh_pairwise) struct PersistedSkippedMessageKeyPublic {
    pub(in crate::core::secure_mesh_pairwise) message_id: String,
    pub(in crate::core::secure_mesh_pairwise) dh_epoch: u64,
    pub(in crate::core::secure_mesh_pairwise) chain_index: u64,
    pub(in crate::core::secure_mesh_pairwise) sender_ratchet_public_key: String,
}

impl From<&SkippedMessageKey> for PersistedSkippedMessageKeyPublic {
    fn from(value: &SkippedMessageKey) -> Self {
        Self {
            message_id: value.message_id.clone(),
            dh_epoch: value.dh_epoch,
            chain_index: value.chain_index,
            sender_ratchet_public_key: encode_secret(&value.sender_ratchet_public_key),
        }
    }
}
