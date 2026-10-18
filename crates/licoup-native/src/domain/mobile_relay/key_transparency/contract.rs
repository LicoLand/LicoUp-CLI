use crate::core::secure_mesh_transparency::SecureMeshKtGossipPayload;
use serde::{Deserialize, Serialize};

pub(super) const SECURE_MESH_KT_GOSSIP_CONTROL_TYPE: &str = "secure_mesh.kt.gossip";

pub const SECURE_MESH_KT_NATIVE_ACTIONS: &[&str] = &[
    "secure_mesh.kt.configureAuthority",
    "secure_mesh.kt.publicationRequest",
    "secure_mesh.kt.revocationRequest",
    "secure_mesh.kt.provision",
    "secure_mesh.kt.gossip",
    "secure_mesh.kt.selfMonitor",
    "secure_mesh.kt.status",
];

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct SecureMeshKtGossipControlMessage {
    pub(super) message_type: String,
    pub(super) gossip: SecureMeshKtGossipPayload,
}
