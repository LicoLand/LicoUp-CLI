use anyhow::{Result, anyhow};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use super::super::support::{
    SECURE_MESH_PAIRWISE_CAPABILITY_POLICY_REVISION, SECURE_MESH_PAIRWISE_CIPHER_SUITE,
    append_len_prefixed_bytes,
};
use crate::core::secure_mesh::{SECURE_MESH_PROTOCOL_BUILD_REVISION, SECURE_MESH_PROTOCOL_VERSION};
#[cfg(test)]
use crate::core::secure_mesh_capability::CapabilityEvaluation;
use crate::core::secure_mesh_capability::capability_catalog;
use crate::core::secure_mesh_capability_proof::{
    CAPABILITY_PROOF_MAX_LIFETIME_SECONDS, CapabilityProofRequest,
    CapabilityProofVerificationContext,
};

pub fn secure_mesh_pairwise_build_protocol_digest() -> Result<String> {
    secure_mesh_pairwise_build_protocol_digest_for_revision(SECURE_MESH_PROTOCOL_BUILD_REVISION)
}

pub(in crate::core::secure_mesh_pairwise) fn secure_mesh_pairwise_build_protocol_digest_for_revision(
    profile_revision: u64,
) -> Result<String> {
    let mut transcript = Vec::new();
    transcript.extend_from_slice(b"LICO-SM-PAIRWISE-BUILD-PROTOCOL-v1");
    append_len_prefixed_bytes(&mut transcript, SECURE_MESH_PROTOCOL_VERSION.as_bytes())?;
    append_len_prefixed_bytes(
        &mut transcript,
        SECURE_MESH_PAIRWISE_CIPHER_SUITE.as_bytes(),
    )?;
    transcript.extend_from_slice(&profile_revision.to_be_bytes());
    transcript.extend_from_slice(&SECURE_MESH_PAIRWISE_CAPABILITY_POLICY_REVISION.to_be_bytes());
    append_len_prefixed_bytes(&mut transcript, capability_catalog()?.digest().as_bytes())?;
    let digest: [u8; 32] = Sha256::digest(transcript).into();
    Ok(crate::core::secure_mesh_capability_proof::encode_sha256_digest(&digest))
}

#[cfg(test)]
pub(crate) fn secure_mesh_pairwise_test_capability_evaluation() -> Result<CapabilityEvaluation> {
    let facts = crate::core::secure_mesh_capability::mandatory_protocol_facts(
        crate::core::secure_mesh_capability::CapabilityEvidenceKind::TestFixture,
    )?;
    capability_catalog()?.evaluate(&facts)
}

pub(in crate::core::secure_mesh_pairwise) fn capability_proof_request(
    challenge: [u8; 32],
    now: OffsetDateTime,
) -> Result<CapabilityProofRequest> {
    let issued_at_unix_seconds = now.unix_timestamp();
    let expires_at_unix_seconds = issued_at_unix_seconds
        .checked_add(CAPABILITY_PROOF_MAX_LIFETIME_SECONDS)
        .ok_or_else(|| anyhow!("secure mesh pairwise capability proof time is invalid"))?;
    Ok(CapabilityProofRequest {
        build_protocol_digest: secure_mesh_pairwise_build_protocol_digest()?,
        policy_revision: SECURE_MESH_PAIRWISE_CAPABILITY_POLICY_REVISION,
        challenge,
        issued_at_unix_seconds,
        expires_at_unix_seconds,
    })
}

pub(in crate::core::secure_mesh_pairwise) fn capability_verification_context(
    challenge: [u8; 32],
    now: OffsetDateTime,
) -> Result<CapabilityProofVerificationContext> {
    Ok(CapabilityProofVerificationContext {
        expected_build_protocol_digest: secure_mesh_pairwise_build_protocol_digest()?,
        expected_policy_revision: SECURE_MESH_PAIRWISE_CAPABILITY_POLICY_REVISION,
        expected_challenge: challenge,
        now_unix_seconds: now.unix_timestamp(),
    })
}
