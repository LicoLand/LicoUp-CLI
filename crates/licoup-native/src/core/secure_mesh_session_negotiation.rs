use anyhow::{Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

use crate::core::secure_mesh_capability::{
    CapabilityScope, SecurityCapability, capability_catalog, custody_selection_from_enabled,
};
use crate::core::secure_mesh_capability_proof::{
    CLIENT_CAPABILITY_PROJECTION_SCHEMA_VERSION, CapabilityProofVerificationContext,
    CapabilitySetProjection, ClientCapabilityProjection, SignedCapabilityProof,
    VerifiedCapabilityProof, append_len_prefixed_bytes, decode_sha256_digest, encode_sha256_digest,
    verify_capability_proof,
};
use crate::core::secure_mesh_trust::DeviceTrustPublicIdentity;

pub const SESSION_CAPABILITY_NEGOTIATION_SCHEMA_VERSION: u32 = 1;
const SESSION_CAPABILITY_TRANSCRIPT_MAGIC: &[u8] = b"LICO-SM-SESSION-CAPABILITY-TRANSCRIPT";
const DEFAULT_REPLAY_GUARD_CAPACITY: usize = 4096;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecureMeshSessionKind {
    Pairwise,
    Mls,
}

impl SecureMeshSessionKind {
    fn id(self) -> &'static str {
        match self {
            Self::Pairwise => "pairwise",
            Self::Mls => "mls",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct NegotiatedCapabilityBinding {
    pub schema_version: u32,
    pub session_kind: SecureMeshSessionKind,
    pub base_transcript_digest: String,
    pub capability_proof_digests: Vec<String>,
    pub negotiated_protocol_capabilities: BTreeSet<SecurityCapability>,
    pub transcript_digest: String,
}

#[derive(Clone, Debug)]
pub struct VerifiedSessionNegotiation {
    binding: NegotiatedCapabilityBinding,
    projection: ClientCapabilityProjection,
}

impl VerifiedSessionNegotiation {
    pub fn binding(&self) -> &NegotiatedCapabilityBinding {
        &self.binding
    }

    pub fn projection(&self) -> &ClientCapabilityProjection {
        &self.projection
    }
}

pub(crate) fn restore_verified_pairwise_session_negotiation(
    binding: NegotiatedCapabilityBinding,
    projection: ClientCapabilityProjection,
) -> Result<VerifiedSessionNegotiation> {
    validate_restored_negotiation(SecureMeshSessionKind::Pairwise, &binding, &projection)?;
    Ok(VerifiedSessionNegotiation {
        binding,
        projection,
    })
}

#[derive(Clone, Copy)]
pub struct CapabilityProofPeer<'a> {
    pub identity: &'a DeviceTrustPublicIdentity,
    pub proof: &'a SignedCapabilityProof,
    pub verification_context: &'a CapabilityProofVerificationContext,
}

#[derive(Clone, Debug)]
pub struct CapabilityProofReplayGuard {
    max_unexpired_proofs: usize,
    consumed: BTreeMap<[u8; 32], i64>,
    expiry_index: BTreeSet<(i64, [u8; 32])>,
}

impl Default for CapabilityProofReplayGuard {
    fn default() -> Self {
        Self {
            max_unexpired_proofs: DEFAULT_REPLAY_GUARD_CAPACITY,
            consumed: BTreeMap::new(),
            expiry_index: BTreeSet::new(),
        }
    }
}

impl CapabilityProofReplayGuard {
    pub fn with_capacity(max_unexpired_proofs: usize) -> Result<Self> {
        ensure!(
            max_unexpired_proofs >= 2,
            "secure mesh capability proof replay capacity is too small"
        );
        Ok(Self {
            max_unexpired_proofs,
            consumed: BTreeMap::new(),
            expiry_index: BTreeSet::new(),
        })
    }

    fn consume_pair(
        &mut self,
        first: &VerifiedCapabilityProof,
        second: &VerifiedCapabilityProof,
        now_unix_seconds: i64,
    ) -> Result<()> {
        self.prune_expired(now_unix_seconds);
        let first_digest = first.proof_digest_bytes();
        let second_digest = second.proof_digest_bytes();
        ensure!(
            first_digest != second_digest,
            "secure mesh session requires two distinct capability proofs"
        );
        ensure!(
            !self.consumed.contains_key(&first_digest)
                && !self.consumed.contains_key(&second_digest),
            "secure mesh capability proof replay rejected"
        );
        ensure!(
            self.consumed.len().saturating_add(2) <= self.max_unexpired_proofs,
            "secure mesh capability proof replay guard is at capacity"
        );
        let first_expiry = first.claims().expires_at_unix_seconds;
        let second_expiry = second.claims().expires_at_unix_seconds;
        self.consumed.insert(first_digest, first_expiry);
        self.consumed.insert(second_digest, second_expiry);
        self.expiry_index.insert((first_expiry, first_digest));
        self.expiry_index.insert((second_expiry, second_digest));
        Ok(())
    }

    fn prune_expired(&mut self, now_unix_seconds: i64) {
        while self
            .expiry_index
            .first()
            .is_some_and(|(expires_at, _)| *expires_at < now_unix_seconds)
        {
            let Some((expires_at, digest)) = self.expiry_index.pop_first() else {
                break;
            };
            if self.consumed.get(&digest) == Some(&expires_at) {
                self.consumed.remove(&digest);
            }
        }
    }
}

pub fn create_pairwise_capability_binding(
    initiator: &VerifiedCapabilityProof,
    responder: &VerifiedCapabilityProof,
    pairwise_transcript_digest: &str,
) -> Result<NegotiatedCapabilityBinding> {
    create_capability_binding(
        SecureMeshSessionKind::Pairwise,
        initiator,
        responder,
        pairwise_transcript_digest,
    )
}

pub fn create_mls_capability_binding(
    committer: &VerifiedCapabilityProof,
    member: &VerifiedCapabilityProof,
    mls_group_context_digest: &str,
) -> Result<NegotiatedCapabilityBinding> {
    create_capability_binding(
        SecureMeshSessionKind::Mls,
        committer,
        member,
        mls_group_context_digest,
    )
}

pub fn accept_pairwise_capability_binding(
    local: CapabilityProofPeer<'_>,
    peer: CapabilityProofPeer<'_>,
    pairwise_transcript_digest: &str,
    binding: &NegotiatedCapabilityBinding,
    replay_guard: &mut CapabilityProofReplayGuard,
) -> Result<VerifiedSessionNegotiation> {
    accept_capability_binding(
        SecureMeshSessionKind::Pairwise,
        local,
        peer,
        pairwise_transcript_digest,
        binding,
        replay_guard,
    )
}

pub fn accept_mls_capability_binding(
    local: CapabilityProofPeer<'_>,
    peer: CapabilityProofPeer<'_>,
    mls_group_context_digest: &str,
    binding: &NegotiatedCapabilityBinding,
    replay_guard: &mut CapabilityProofReplayGuard,
) -> Result<VerifiedSessionNegotiation> {
    accept_capability_binding(
        SecureMeshSessionKind::Mls,
        local,
        peer,
        mls_group_context_digest,
        binding,
        replay_guard,
    )
}

fn create_capability_binding(
    session_kind: SecureMeshSessionKind,
    first: &VerifiedCapabilityProof,
    second: &VerifiedCapabilityProof,
    base_transcript_digest: &str,
) -> Result<NegotiatedCapabilityBinding> {
    decode_sha256_digest(base_transcript_digest, "base session transcript digest")?;
    validate_peer_proof_pair(first, second)?;
    let negotiated_protocol_capabilities = protocol_intersection(first, second)?;
    require_mandatory_protocol_capabilities(&negotiated_protocol_capabilities)?;
    let capability_proof_digests = sorted_proof_digests(first, second)?;
    let transcript_digest = negotiation_transcript_digest(
        session_kind,
        base_transcript_digest,
        &capability_proof_digests,
        &negotiated_protocol_capabilities,
    )?;
    Ok(NegotiatedCapabilityBinding {
        schema_version: SESSION_CAPABILITY_NEGOTIATION_SCHEMA_VERSION,
        session_kind,
        base_transcript_digest: base_transcript_digest.to_string(),
        capability_proof_digests,
        negotiated_protocol_capabilities,
        transcript_digest,
    })
}

fn accept_capability_binding(
    session_kind: SecureMeshSessionKind,
    local: CapabilityProofPeer<'_>,
    peer: CapabilityProofPeer<'_>,
    base_transcript_digest: &str,
    binding: &NegotiatedCapabilityBinding,
    replay_guard: &mut CapabilityProofReplayGuard,
) -> Result<VerifiedSessionNegotiation> {
    let local_verified =
        verify_capability_proof(local.identity, local.proof, local.verification_context)?;
    let peer_verified =
        verify_capability_proof(peer.identity, peer.proof, peer.verification_context)?;
    let expected = create_capability_binding(
        session_kind,
        &local_verified,
        &peer_verified,
        base_transcript_digest,
    )?;
    ensure!(
        binding == &expected,
        "secure mesh negotiated capability transcript binding failed"
    );
    let now = local
        .verification_context
        .now_unix_seconds
        .max(peer.verification_context.now_unix_seconds);
    replay_guard.consume_pair(&local_verified, &peer_verified, now)?;
    Ok(VerifiedSessionNegotiation {
        binding: binding.clone(),
        projection: ClientCapabilityProjection {
            schema_version: CLIENT_CAPABILITY_PROJECTION_SCHEMA_VERSION,
            local: local_verified.projection().clone(),
            peer: Some(peer_verified.projection().clone()),
            negotiated_protocol_capabilities: binding.negotiated_protocol_capabilities.clone(),
            reasons: BTreeMap::new(),
        },
    })
}

fn validate_restored_negotiation(
    expected_kind: SecureMeshSessionKind,
    binding: &NegotiatedCapabilityBinding,
    projection: &ClientCapabilityProjection,
) -> Result<()> {
    ensure!(
        binding.schema_version == SESSION_CAPABILITY_NEGOTIATION_SCHEMA_VERSION
            && binding.session_kind == expected_kind,
        "secure mesh restored capability negotiation schema is invalid"
    );
    decode_sha256_digest(
        &binding.base_transcript_digest,
        "restored base session transcript digest",
    )?;
    ensure!(
        binding.capability_proof_digests.len() == 2
            && binding.capability_proof_digests[0] < binding.capability_proof_digests[1],
        "secure mesh restored capability proof digest set is invalid"
    );
    for digest in &binding.capability_proof_digests {
        decode_sha256_digest(digest, "restored capability proof digest")?;
    }
    let expected_transcript_digest = negotiation_transcript_digest(
        expected_kind,
        &binding.base_transcript_digest,
        &binding.capability_proof_digests,
        &binding.negotiated_protocol_capabilities,
    )?;
    ensure!(
        binding.transcript_digest == expected_transcript_digest,
        "secure mesh restored capability transcript binding failed"
    );

    ensure!(
        projection.schema_version == CLIENT_CAPABILITY_PROJECTION_SCHEMA_VERSION
            && projection.reasons.is_empty(),
        "secure mesh restored client capability projection schema is invalid"
    );
    validate_restored_capability_set(&projection.local)?;
    let peer = projection
        .peer
        .as_ref()
        .ok_or_else(|| anyhow!("secure mesh restored peer capability projection is missing"))?;
    validate_restored_capability_set(peer)?;
    ensure!(
        projection.local.catalog_digest == peer.catalog_digest,
        "secure mesh restored capability catalog bindings differ"
    );
    let negotiated = protocol_projection_intersection(&projection.local, peer)?;
    require_mandatory_protocol_capabilities(&negotiated)?;
    ensure!(
        negotiated == binding.negotiated_protocol_capabilities
            && projection.negotiated_protocol_capabilities
                == binding.negotiated_protocol_capabilities,
        "secure mesh restored negotiated capability set is invalid"
    );
    Ok(())
}

fn validate_restored_capability_set(projection: &CapabilitySetProjection) -> Result<()> {
    ensure!(
        projection.schema_version == CLIENT_CAPABILITY_PROJECTION_SCHEMA_VERSION,
        "secure mesh restored capability set schema is invalid"
    );
    let catalog = capability_catalog()?;
    ensure!(
        projection.catalog_digest == catalog.digest(),
        "secure mesh restored capability catalog binding is invalid"
    );
    let all = catalog
        .definitions()
        .map(|definition| definition.capability)
        .collect::<BTreeSet<_>>();
    let mut partition = BTreeSet::new();
    for set in [
        &projection.available,
        &projection.unavailable,
        &projection.unverified,
    ] {
        ensure!(
            set.is_subset(&all),
            "secure mesh restored capability set contains an unknown capability"
        );
        for capability in set.iter().copied() {
            ensure!(
                partition.insert(capability),
                "secure mesh restored capability exact sets overlap"
            );
        }
    }
    ensure!(
        partition == all && projection.enabled.is_subset(&projection.available),
        "secure mesh restored capability exact sets are invalid"
    );
    let missing_mandatory = catalog
        .definitions()
        .filter(|definition| definition.mandatory)
        .map(|definition| definition.capability)
        .filter(|capability| !projection.enabled.contains(capability))
        .collect::<BTreeSet<_>>();
    ensure!(
        projection.missing_mandatory == missing_mandatory
            && projection.mandatory_foundation_complete == missing_mandatory.is_empty(),
        "secure mesh restored mandatory capability set is invalid"
    );
    let expected_reasons = all
        .difference(&projection.enabled)
        .map(|capability| capability.id().to_string())
        .collect::<BTreeSet<_>>();
    ensure!(
        projection.reasons.keys().cloned().collect::<BTreeSet<_>>() == expected_reasons,
        "secure mesh restored capability reason coverage is invalid"
    );
    ensure!(
        projection.custody == custody_selection_from_enabled(&projection.enabled),
        "secure mesh restored custody projection is invalid"
    );
    Ok(())
}

fn protocol_projection_intersection(
    first: &CapabilitySetProjection,
    second: &CapabilitySetProjection,
) -> Result<BTreeSet<SecurityCapability>> {
    let catalog = capability_catalog()?;
    Ok(first
        .enabled
        .intersection(&second.enabled)
        .copied()
        .filter(|capability| {
            catalog
                .definition(*capability)
                .is_some_and(|definition| definition.scope == CapabilityScope::ProtocolSession)
        })
        .collect())
}

fn validate_peer_proof_pair(
    first: &VerifiedCapabilityProof,
    second: &VerifiedCapabilityProof,
) -> Result<()> {
    ensure!(
        first.claims().endpoint_identity_fingerprint
            != second.claims().endpoint_identity_fingerprint,
        "secure mesh session capability proofs must identify distinct endpoints"
    );
    ensure!(
        first.challenge_bytes()? == second.challenge_bytes()?,
        "secure mesh session capability proof challenges differ"
    );
    ensure!(
        first.claims().policy_revision == second.claims().policy_revision,
        "secure mesh session capability proof policy revisions differ"
    );
    Ok(())
}

fn protocol_intersection(
    first: &VerifiedCapabilityProof,
    second: &VerifiedCapabilityProof,
) -> Result<BTreeSet<SecurityCapability>> {
    let catalog = capability_catalog()?;
    Ok(first
        .claims()
        .enabled
        .intersection(&second.claims().enabled)
        .copied()
        .filter(|capability| {
            catalog
                .definition(*capability)
                .map(|definition| definition.scope == CapabilityScope::ProtocolSession)
                .unwrap_or(false)
        })
        .collect())
}

fn require_mandatory_protocol_capabilities(
    negotiated: &BTreeSet<SecurityCapability>,
) -> Result<()> {
    let catalog = capability_catalog()?;
    let missing = catalog
        .definitions()
        .filter(|definition| definition.mandatory)
        .map(|definition| definition.capability)
        .any(|capability| !negotiated.contains(&capability));
    ensure!(
        !missing,
        "secure mesh session mandatory capability negotiation failed"
    );
    Ok(())
}

fn sorted_proof_digests(
    first: &VerifiedCapabilityProof,
    second: &VerifiedCapabilityProof,
) -> Result<Vec<String>> {
    let mut digests = vec![first.proof_digest(), second.proof_digest()];
    digests.sort_unstable();
    ensure!(
        digests[0] != digests[1],
        "secure mesh session requires two distinct capability proofs"
    );
    Ok(digests)
}

fn negotiation_transcript_digest(
    session_kind: SecureMeshSessionKind,
    base_transcript_digest: &str,
    proof_digests: &[String],
    negotiated: &BTreeSet<SecurityCapability>,
) -> Result<String> {
    ensure!(
        proof_digests.len() == 2 && proof_digests[0] < proof_digests[1],
        "secure mesh session capability proof digest set is invalid"
    );
    let mut transcript = Vec::new();
    transcript.extend_from_slice(SESSION_CAPABILITY_TRANSCRIPT_MAGIC);
    transcript.extend_from_slice(&SESSION_CAPABILITY_NEGOTIATION_SCHEMA_VERSION.to_be_bytes());
    append_len_prefixed_bytes(&mut transcript, session_kind.id().as_bytes())?;
    append_len_prefixed_bytes(
        &mut transcript,
        &decode_sha256_digest(base_transcript_digest, "base session transcript digest")?,
    )?;
    transcript.extend_from_slice(&(proof_digests.len() as u32).to_be_bytes());
    for digest in proof_digests {
        append_len_prefixed_bytes(
            &mut transcript,
            &decode_sha256_digest(digest, "capability proof digest")?,
        )?;
    }
    let mut capability_ids = negotiated
        .iter()
        .map(|capability| capability.id())
        .collect::<Vec<_>>();
    capability_ids.sort_unstable();
    let capability_count = u32::try_from(capability_ids.len())
        .map_err(|_| anyhow!("secure mesh negotiated capability set is too large"))?;
    transcript.extend_from_slice(&capability_count.to_be_bytes());
    for id in capability_ids {
        append_len_prefixed_bytes(&mut transcript, id.as_bytes())?;
    }
    let digest: [u8; 32] = Sha256::digest(transcript).into();
    Ok(encode_sha256_digest(&digest))
}

#[cfg(test)]
mod tests;
