//! Verification-only Key Transparency primitives for the client.
//!
//! The release build deliberately contains no log signing key or log authority. A client is
//! constructed with an explicit pin, verifies RFC 9162 append-only proofs, verifies the
//! authenticated sparse directory map, and advances its durable checkpoint with a SQLite CAS.

mod client_state;
mod constants;
mod diagnostics;
mod json_codec;
#[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
mod log;
mod model;
mod persistence;
mod proofs;
mod signature;
mod sparse_map;

pub use client_state::SecureMeshKtClientState;
pub use constants::{
    KT_JSON_SAFE_INTEGER_MAX, KT_PROTOCOL_MAX_FUTURE_SKEW_SECONDS,
    KT_PROTOCOL_MAX_GOSSIP_AGE_SECONDS, KT_PROTOCOL_MAX_STH_AGE_SECONDS,
    SECURE_MESH_DIAGNOSTIC_HASH_CHAIN_STATUS, SECURE_MESH_KT_GOSSIP_CONTENT_TYPE,
    SECURE_MESH_KT_PROTOCOL_VERSION, SECURE_MESH_TRANSPARENCY_STATUS,
};
pub use diagnostics::diagnostic_hash_chain_tree_head;
#[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
pub use log::SecureMeshKtLog;
pub(crate) use model::DirectoryComponentCommitments;
pub use model::{
    SecureMeshKtAuthorizationReceipt, SecureMeshKtCachedCheckpoint, SecureMeshKtConsistencyProof,
    SecureMeshKtGossipPayload, SecureMeshKtInclusionProof, SecureMeshKtMapEntry,
    SecureMeshKtMapProof, SecureMeshKtNonInclusionProof, SecureMeshTransparencyLeafBody,
    directory_scope_commitment, stable_directory_label,
};
pub use persistence::reset_kt_persistent_authority_state;
pub(crate) use proofs::kt_log_leaf_hash;
pub use proofs::{verify_kt_consistency, verify_kt_inclusion};
pub use signature::{
    KtAuthorityProvenance, KtFreshnessPolicy, PinnedKtLogKey, SecureMeshSignedTreeHead,
    VerifiedKtFreshness,
};
pub use sparse_map::{verify_kt_map_inclusion, verify_kt_non_inclusion};

#[cfg(test)]
mod tests;
