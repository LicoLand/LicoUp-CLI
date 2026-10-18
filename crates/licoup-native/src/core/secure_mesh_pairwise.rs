mod codec;
mod key_ratchet;
mod manager_fanout;
mod persistence;
mod runtime_self_test;
mod session_negotiation;
mod support;

pub use codec::{OpenedPairwiseMessage, SecureMeshPairwiseMessage};
pub use key_ratchet::{
    SecureMeshPairwisePrivateKey, SecureMeshPairwiseRole, SecureMeshPairwiseSession,
};
pub use manager_fanout::{SecureMeshSesameDeviceRecord, SecureMeshSesameSessionManager};
pub(crate) use persistence::pairwise_secret_store_namespace;
pub use persistence::{
    SecureMeshLocalPreKeyUse, SecureMeshPairwiseDurableRecord, SecureMeshPairwiseDurableStore,
    SecureMeshPairwisePendingDelivery, SecureMeshPairwiseReceivedPayload,
    SecureMeshRemotePreKeyUse,
};
pub use runtime_self_test::runtime_crypto_self_test;
#[cfg(test)]
pub(crate) use session_negotiation::secure_mesh_pairwise_test_capability_evaluation;
pub use session_negotiation::{
    SecureMeshPairwiseSessionAccepted, SecureMeshPairwiseSessionFinished,
    SecureMeshPairwiseSessionIntro, secure_mesh_pairwise_build_protocol_digest,
};
pub use support::{
    SECURE_MESH_PAIRWISE_CAPABILITY_POLICY_REVISION, SECURE_MESH_PAIRWISE_CIPHER_SUITE,
    SECURE_MESH_PAIRWISE_STATUS,
};

#[cfg(test)]
mod tests;
