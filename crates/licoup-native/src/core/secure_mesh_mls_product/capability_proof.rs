use super::constants::{MAX_ROSTER, SECURE_MESH_MLS_CAPABILITY_POLICY_REVISION};
use super::helpers::append_len_prefixed;
use anyhow::{Result, anyhow, ensure};
use ed25519_dalek::SigningKey;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use time::OffsetDateTime;

use crate::core::secure_mesh::SECURE_MESH_PROTOCOL_BUILD_REVISION;
use crate::core::secure_mesh_capability::{
    CapabilityEvaluation, CapabilityScope, SecurityCapability, capability_catalog,
};
use crate::core::secure_mesh_capability_proof::{
    CAPABILITY_PROOF_MAX_CLOCK_SKEW_SECONDS, CAPABILITY_PROOF_MAX_LIFETIME_SECONDS,
    CapabilityProofRequest, CapabilityProofVerificationContext, SignedCapabilityProof,
    sign_capability_proof, signed_capability_proof_challenge,
};
use crate::core::secure_mesh_mls::{
    SECURE_MESH_GROUP_MLS_PROTOCOL_VERSION, SECURE_MESH_MLS_CIPHER_SUITE,
    SecureMeshMlsCapabilityExtension, SecureMeshMlsKeyPackage, SecureMeshMlsMemberCapabilityProof,
    SecureMeshMlsRosterTransition,
};
use crate::core::secure_mesh_session_negotiation::{
    CapabilityProofPeer, CapabilityProofReplayGuard, SecureMeshSessionKind,
    accept_mls_capability_binding,
};
use crate::core::secure_mesh_trust::DeviceTrustPublicIdentity;

pub fn secure_mesh_mls_build_protocol_digest() -> Result<String> {
    secure_mesh_mls_build_protocol_digest_for_revision(SECURE_MESH_PROTOCOL_BUILD_REVISION)
}

pub(super) fn secure_mesh_mls_build_protocol_digest_for_revision(
    profile_revision: u64,
) -> Result<String> {
    let mut transcript = Vec::new();
    transcript.extend_from_slice(b"LICO-SM-MLS-BUILD-PROTOCOL-v1");
    append_len_prefixed(
        &mut transcript,
        SECURE_MESH_GROUP_MLS_PROTOCOL_VERSION.as_bytes(),
    )?;
    append_len_prefixed(&mut transcript, SECURE_MESH_MLS_CIPHER_SUITE.as_bytes())?;
    transcript.extend_from_slice(&profile_revision.to_be_bytes());
    transcript.extend_from_slice(&SECURE_MESH_MLS_CAPABILITY_POLICY_REVISION.to_be_bytes());
    append_len_prefixed(&mut transcript, capability_catalog()?.digest().as_bytes())?;
    let digest: [u8; 32] = Sha256::digest(transcript).into();
    Ok(crate::core::secure_mesh_capability_proof::encode_sha256_digest(&digest))
}

pub(super) fn mls_key_package_capability_challenge(
    key_package: &SecureMeshMlsKeyPackage,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"LICO-SM-MLS-KEYPACKAGE-CAPABILITY-CHALLENGE-v1");
    hasher.update(key_package.as_public_bytes());
    hasher.finalize().into()
}

pub(super) fn mls_capability_proof_request(
    challenge: [u8; 32],
    now: OffsetDateTime,
) -> Result<CapabilityProofRequest> {
    let issued_at_unix_seconds = now.unix_timestamp();
    let expires_at_unix_seconds = issued_at_unix_seconds
        .checked_add(CAPABILITY_PROOF_MAX_LIFETIME_SECONDS)
        .ok_or_else(|| anyhow!("secure mesh MLS capability proof time is invalid"))?;
    Ok(CapabilityProofRequest {
        build_protocol_digest: secure_mesh_mls_build_protocol_digest()?,
        policy_revision: SECURE_MESH_MLS_CAPABILITY_POLICY_REVISION,
        challenge,
        issued_at_unix_seconds,
        expires_at_unix_seconds,
    })
}

pub(super) fn mls_capability_verification_context(
    challenge: [u8; 32],
    now: OffsetDateTime,
) -> Result<CapabilityProofVerificationContext> {
    Ok(CapabilityProofVerificationContext {
        expected_build_protocol_digest: secure_mesh_mls_build_protocol_digest()?,
        expected_policy_revision: SECURE_MESH_MLS_CAPABILITY_POLICY_REVISION,
        expected_challenge: challenge,
        now_unix_seconds: now.unix_timestamp(),
    })
}

pub fn sign_mls_keypackage_capability_proof(
    identity: &DeviceTrustPublicIdentity,
    signing_key: &SigningKey,
    evaluation: &CapabilityEvaluation,
    key_package: &SecureMeshMlsKeyPackage,
    now: OffsetDateTime,
) -> Result<SignedCapabilityProof> {
    sign_capability_proof(
        identity,
        signing_key,
        evaluation,
        &mls_capability_proof_request(mls_key_package_capability_challenge(key_package), now)?,
    )
}

pub(super) fn verify_active_mls_capability_extension(
    extension: &SecureMeshMlsCapabilityExtension,
    committer_identity: &DeviceTrustPublicIdentity,
    added_member_identity: &DeviceTrustPublicIdentity,
    now: OffsetDateTime,
) -> Result<BTreeSet<SecurityCapability>> {
    let SecureMeshMlsCapabilityExtension::Active {
        committer_endpoint_id,
        roster_transition,
        member_capability_proofs,
        group_negotiated_protocol_capabilities,
        ..
    } = extension
    else {
        return Err(anyhow!(
            "secure mesh MLS member capability negotiation is incomplete"
        ));
    };
    let SecureMeshMlsRosterTransition::MemberAdded {
        member_endpoint_id: added_member_endpoint_id,
        pair_binding,
    } = roster_transition.as_ref()
    else {
        return Err(anyhow!(
            "secure mesh MLS capability extension is not a member-add transition"
        ));
    };
    ensure!(
        committer_endpoint_id == &committer_identity.endpoint_id
            && added_member_endpoint_id == &added_member_identity.endpoint_id,
        "secure mesh MLS capability extension pair identity is invalid"
    );
    let committer_record = member_capability_proofs
        .get(committer_endpoint_id)
        .ok_or_else(|| anyhow!("secure mesh MLS committer capability proof is missing"))?;
    let added_member_record = member_capability_proofs
        .get(added_member_endpoint_id)
        .ok_or_else(|| anyhow!("secure mesh MLS added member capability proof is missing"))?;
    ensure!(
        pair_binding.session_kind == SecureMeshSessionKind::Mls,
        "secure mesh MLS capability binding has the wrong session kind"
    );
    let challenge = signed_capability_proof_challenge(&added_member_record.proof)?;
    let context = mls_capability_verification_context(challenge, now)?;
    let mut verification_guard = CapabilityProofReplayGuard::default();
    let negotiation = accept_mls_capability_binding(
        CapabilityProofPeer {
            identity: committer_identity,
            proof: &committer_record.proof,
            verification_context: &context,
        },
        CapabilityProofPeer {
            identity: added_member_identity,
            proof: &added_member_record.proof,
            verification_context: &context,
        },
        &pair_binding.base_transcript_digest,
        pair_binding,
        &mut verification_guard,
    )?;
    ensure!(
        group_negotiated_protocol_capabilities
            .is_subset(&negotiation.binding().negotiated_protocol_capabilities),
        "secure mesh MLS group capability extension overclaims the added pair"
    );
    Ok(negotiation
        .binding()
        .negotiated_protocol_capabilities
        .clone())
}

pub(super) fn active_pair_capability_proofs(
    extension: &SecureMeshMlsCapabilityExtension,
) -> Result<(&SignedCapabilityProof, &SignedCapabilityProof)> {
    let SecureMeshMlsCapabilityExtension::Active {
        committer_endpoint_id,
        roster_transition,
        member_capability_proofs,
        ..
    } = extension
    else {
        return Err(anyhow!(
            "secure mesh MLS member capability negotiation is incomplete"
        ));
    };
    let SecureMeshMlsRosterTransition::MemberAdded {
        member_endpoint_id: added_member_endpoint_id,
        ..
    } = roster_transition.as_ref()
    else {
        return Err(anyhow!(
            "secure mesh MLS capability extension is not a member-add transition"
        ));
    };
    let committer = member_capability_proofs
        .get(committer_endpoint_id)
        .ok_or_else(|| anyhow!("secure mesh MLS committer capability proof is missing"))?;
    let added_member = member_capability_proofs
        .get(added_member_endpoint_id)
        .ok_or_else(|| anyhow!("secure mesh MLS added member capability proof is missing"))?;
    Ok((&committer.proof, &added_member.proof))
}

pub(super) fn capability_intersection_from_member_proofs(
    member_capability_proofs: &BTreeMap<String, SecureMeshMlsMemberCapabilityProof>,
) -> Result<BTreeSet<SecurityCapability>> {
    ensure!(
        !member_capability_proofs.is_empty() && member_capability_proofs.len() <= MAX_ROSTER,
        "secure mesh MLS member capability proof map size is invalid"
    );
    let catalog = capability_catalog()?;
    let mut proof_records = member_capability_proofs.iter();
    let (first_endpoint_id, first_record) = proof_records
        .next()
        .ok_or_else(|| anyhow!("secure mesh MLS member capability proof map is empty"))?;
    ensure!(
        first_endpoint_id == &first_record.endpoint_id,
        "secure mesh MLS member capability proof record is invalid"
    );
    validate_capability_proof_acceptance_time(first_record)?;
    let mut intersection = first_record
        .proof
        .claims
        .enabled
        .iter()
        .copied()
        .filter(|capability| {
            catalog
                .definition(*capability)
                .is_some_and(|definition| definition.scope == CapabilityScope::ProtocolSession)
        })
        .collect::<BTreeSet<_>>();
    for (endpoint_id, record) in proof_records {
        ensure!(
            endpoint_id == &record.endpoint_id,
            "secure mesh MLS member capability proof record is invalid"
        );
        validate_capability_proof_acceptance_time(record)?;
        intersection.retain(|capability| record.proof.claims.enabled.contains(capability));
    }
    let missing_mandatory = catalog
        .definitions()
        .filter(|definition| definition.mandatory)
        .any(|definition| !intersection.contains(&definition.capability));
    ensure!(
        !missing_mandatory,
        "secure mesh MLS group mandatory capability intersection failed"
    );
    Ok(intersection)
}

pub(super) fn verify_complete_member_capability_proof_map(
    extension: &SecureMeshMlsCapabilityExtension,
    expected_roster_endpoint_ids: &BTreeSet<String>,
    trusted_roster: &BTreeMap<String, DeviceTrustPublicIdentity>,
) -> Result<()> {
    let SecureMeshMlsCapabilityExtension::Active {
        member_capability_proofs,
        group_negotiated_protocol_capabilities,
        ..
    } = extension
    else {
        return Err(anyhow!(
            "secure mesh MLS member capability negotiation is incomplete"
        ));
    };
    ensure!(
        member_capability_proofs
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>()
            == *expected_roster_endpoint_ids
            && trusted_roster.keys().cloned().collect::<BTreeSet<_>>()
                == *expected_roster_endpoint_ids,
        "secure mesh MLS member capability proof map does not match roster"
    );
    for (endpoint_id, record) in member_capability_proofs {
        let identity = trusted_roster
            .get(endpoint_id)
            .ok_or_else(|| anyhow!("secure mesh MLS member capability identity is not trusted"))?;
        ensure!(
            record.endpoint_id == *endpoint_id,
            "secure mesh MLS member capability proof record is invalid"
        );
        validate_capability_proof_acceptance_time(record)?;
        let context = CapabilityProofVerificationContext {
            expected_build_protocol_digest: secure_mesh_mls_build_protocol_digest()?,
            expected_policy_revision: SECURE_MESH_MLS_CAPABILITY_POLICY_REVISION,
            expected_challenge: signed_capability_proof_challenge(&record.proof)?,
            now_unix_seconds: record.accepted_at_unix_seconds,
        };
        crate::core::secure_mesh_capability_proof::verify_capability_proof(
            identity,
            &record.proof,
            &context,
        )?;
    }
    ensure!(
        group_negotiated_protocol_capabilities
            == &capability_intersection_from_member_proofs(member_capability_proofs)?,
        "secure mesh MLS cumulative capability intersection is invalid"
    );
    Ok(())
}

pub(super) fn validate_capability_proof_acceptance_time(
    record: &SecureMeshMlsMemberCapabilityProof,
) -> Result<()> {
    let latest_acceptable_issue = record
        .accepted_at_unix_seconds
        .checked_add(CAPABILITY_PROOF_MAX_CLOCK_SKEW_SECONDS)
        .ok_or_else(|| anyhow!("secure mesh MLS capability proof acceptance time is invalid"))?;
    ensure!(
        record.proof.claims.issued_at_unix_seconds <= latest_acceptable_issue
            && record.accepted_at_unix_seconds <= record.proof.claims.expires_at_unix_seconds,
        "secure mesh MLS capability proof acceptance time is outside freshness policy"
    );
    Ok(())
}
