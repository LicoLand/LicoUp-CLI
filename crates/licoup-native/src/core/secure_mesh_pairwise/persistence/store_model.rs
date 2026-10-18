use std::sync::Arc;

use rusqlite::Connection;

use crate::core::secure_mesh_secret_store::SecureMeshSecretStore;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshPairwiseDurableRecord {
    pub session_id: String,
    pub local_endpoint_id: String,
    pub remote_endpoint_id: String,
    pub state_version: u64,
    pub dh_epoch: u64,
    pub sent_count: u64,
    pub received_count: u64,
    pub revoked_at: Option<String>,
    pub updated_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshRemotePreKeyUse {
    pub session_id: String,
    pub local_endpoint_id: String,
    pub remote_endpoint_id: String,
    pub remote_identity_fingerprint: String,
    pub signed_prekey_id: String,
    pub one_time_prekey_id: String,
    pub one_time_prekey_public_key_hash: String,
    pub one_time_mlkem1024_prekey_id: String,
    pub one_time_mlkem1024_prekey_public_key_hash: String,
    pub directory_authorization_digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshLocalPreKeyUse {
    pub local_endpoint_id: String,
    pub local_identity_fingerprint: String,
    pub one_time_prekey_id: String,
    pub one_time_prekey_public_key_hash: String,
    pub one_time_mlkem1024_prekey_id: String,
    pub one_time_mlkem1024_prekey_public_key_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshPairwisePendingDelivery {
    pub delivery_kind: String,
    pub envelope_id: String,
    pub expires_at: String,
    pub envelope_json: String,
    pub binding_json: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecureMeshPairwiseReceivedPayload {
    pub receipt_id: String,
    pub binding_digest: String,
    pub mailbox_id: String,
    pub payload_json: String,
    pub received_at: String,
}

pub struct SecureMeshPairwiseDurableStore {
    pub(super) connection: Connection,
    pub(super) secret_store: Arc<dyn SecureMeshSecretStore>,
    pub(super) secret_store_namespace: String,
}
