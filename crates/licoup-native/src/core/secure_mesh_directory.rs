//! Typed Key Transparency directory authority.
//!
//! Directory models, verified authority state transitions, and proof
//! transcript construction are separated behind this stable facade.

use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

use crate::core::secure_mesh_transparency::{
    DirectoryComponentCommitments, KT_JSON_SAFE_INTEGER_MAX, KtAuthorityProvenance,
    KtFreshnessPolicy, PinnedKtLogKey, SecureMeshKtAuthorizationReceipt,
    SecureMeshKtCachedCheckpoint, SecureMeshKtClientState, SecureMeshKtConsistencyProof,
    SecureMeshKtGossipPayload, SecureMeshKtInclusionProof, SecureMeshKtMapProof,
    SecureMeshSignedTreeHead, SecureMeshTransparencyLeafBody, VerifiedKtFreshness,
    kt_log_leaf_hash,
};
use crate::core::secure_mesh_trust::DeviceTrustPublicIdentity;

mod authority;
mod model;
mod proof;

pub use authority::SecureMeshDirectoryAuthority;
use authority::require_claim_device_identity;
pub use model::{
    AuthorizedDirectoryAbsence, AuthorizedDirectoryLeaf, DirectoryAuthorizationPurpose,
    DirectoryAuthorizationRequest, PinnedKtLogConfiguration,
    SecureMeshDirectoryKeyMaterialCommitment, SecureMeshDirectoryLeafClaim,
    SecureMeshKtVerifierConfiguration, UntrustedDirectoryAbsenceResponse,
    UntrustedDirectoryResponse,
};
use proof::*;

pub const SECURE_MESH_DIRECTORY_CLAIM_VERSION: &str = "licomesh.secure-mesh.directory-claim.v1";
pub const SECURE_MESH_DIRECTORY_AUTHORITY_STATUS: &str =
    "typed_identity_pairwise_prekeys_mls_keypackage_pinned_kt_latest_map_authority";

const DIRECTORY_CLAIM_DOMAIN: &[u8] = b"LCOSM-KT-DIRECTORY-CLAIM-v1";
const DIRECTORY_IDENTITY_COMMITMENT_DOMAIN: &[u8] = b"LCOSM-KT-DIRECTORY-IDENTITY-COMMITMENT-v1";
const AUTHORIZED_LEAF_DOMAIN: &[u8] = b"LCOSM-KT-AUTHORIZED-LEAF-v1";
const AUTHORIZED_LEAF_TRANSCRIPT_BINDING_DOMAIN: &[u8] =
    b"LCOSM-KT-AUTHORIZED-LEAF-TRANSCRIPT-BINDING-v1";
const AUTHORIZED_ABSENCE_DOMAIN: &[u8] = b"LCOSM-KT-AUTHORIZED-ABSENCE-v1";
const HASH_HEX_LEN: usize = 64;
#[allow(dead_code)]
const MAX_DIRECTORY_PROOF_HASHES: usize = 256;

#[cfg(test)]
mod tests;
