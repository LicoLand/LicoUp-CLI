use anyhow::{Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use ed25519_dalek::{Signature, Signer, SigningKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

use crate::core::secure_mesh_capability::{
    CapabilityCatalog, CapabilityEvaluation, CustodySelection, SecurityCapability,
    capability_catalog, custody_selection_from_enabled,
};
use crate::core::secure_mesh_trust::DeviceTrustPublicIdentity;

pub const CAPABILITY_PROOF_SCHEMA_VERSION: u32 = 1;
pub const CLIENT_CAPABILITY_PROJECTION_SCHEMA_VERSION: u32 = 3;
pub const CAPABILITY_PROOF_CHALLENGE_LEN: usize = 32;
pub const CAPABILITY_PROOF_MAX_LIFETIME_SECONDS: i64 = 300;
pub const CAPABILITY_PROOF_MAX_CLOCK_SKEW_SECONDS: i64 = 30;

const CAPABILITY_PROOF_SIGNATURE_MAGIC: &[u8] = b"LICO-SM-CAPABILITY-PROOF-SIGNATURE";
const CAPABILITY_PROOF_DIGEST_MAGIC: &[u8] = b"LICO-SM-CAPABILITY-PROOF-DIGEST";
const CAPABILITY_PROOF_SIGNATURE_LEN: usize = 64;
const CAPABILITY_PROOF_DIGEST_LEN: usize = 32;
const CAPABILITY_PROOF_MAX_JSON_BYTES: usize = 128 * 1024;
const CAPABILITY_PROOF_MAX_REASON_CODE_LEN: usize = 96;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityProofRequest {
    pub build_protocol_digest: String,
    pub policy_revision: u64,
    pub challenge: [u8; CAPABILITY_PROOF_CHALLENGE_LEN],
    pub issued_at_unix_seconds: i64,
    pub expires_at_unix_seconds: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CapabilityProofClaims {
    pub schema_version: u32,
    pub endpoint_identity_fingerprint: String,
    pub capability_catalog_digest: String,
    pub build_protocol_digest: String,
    pub policy_revision: u64,
    pub challenge: String,
    pub issued_at_unix_seconds: i64,
    pub expires_at_unix_seconds: i64,
    pub mandatory_foundation_complete: bool,
    pub enabled: BTreeSet<SecurityCapability>,
    pub available: BTreeSet<SecurityCapability>,
    pub unavailable: BTreeSet<SecurityCapability>,
    pub unverified: BTreeSet<SecurityCapability>,
    pub missing_mandatory: BTreeSet<SecurityCapability>,
    pub reasons: BTreeMap<SecurityCapability, String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SignedCapabilityProof {
    pub claims: CapabilityProofClaims,
    pub signature: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CapabilitySetProjection {
    pub schema_version: u32,
    pub catalog_digest: String,
    pub mandatory_foundation_complete: bool,
    pub enabled: BTreeSet<SecurityCapability>,
    pub available: BTreeSet<SecurityCapability>,
    pub unavailable: BTreeSet<SecurityCapability>,
    pub unverified: BTreeSet<SecurityCapability>,
    pub missing_mandatory: BTreeSet<SecurityCapability>,
    pub reasons: BTreeMap<String, String>,
    pub custody: Option<CustodySelection>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ClientCapabilityProjection {
    pub schema_version: u32,
    pub local: CapabilitySetProjection,
    pub peer: Option<CapabilitySetProjection>,
    pub negotiated_protocol_capabilities: BTreeSet<SecurityCapability>,
    pub reasons: BTreeMap<String, String>,
}

impl ClientCapabilityProjection {
    pub fn local_only(evaluation: &CapabilityEvaluation) -> Self {
        Self {
            schema_version: CLIENT_CAPABILITY_PROJECTION_SCHEMA_VERSION,
            local: capability_projection_from_evaluation(evaluation),
            peer: None,
            negotiated_protocol_capabilities: BTreeSet::new(),
            reasons: BTreeMap::from([
                (
                    "peer".to_string(),
                    "secure_mesh_peer_capability_proof_not_available".to_string(),
                ),
                (
                    "negotiated_protocol_capabilities".to_string(),
                    "secure_mesh_session_not_established".to_string(),
                ),
            ]),
        }
    }
}

#[derive(Clone, Debug)]
pub struct VerifiedCapabilityProof {
    claims: CapabilityProofClaims,
    proof_digest: [u8; CAPABILITY_PROOF_DIGEST_LEN],
    projection: CapabilitySetProjection,
}

impl VerifiedCapabilityProof {
    pub fn claims(&self) -> &CapabilityProofClaims {
        &self.claims
    }

    pub fn proof_digest(&self) -> String {
        encode_sha256_digest(&self.proof_digest)
    }

    pub fn projection(&self) -> &CapabilitySetProjection {
        &self.projection
    }

    pub(crate) fn proof_digest_bytes(&self) -> [u8; CAPABILITY_PROOF_DIGEST_LEN] {
        self.proof_digest
    }

    pub(crate) fn challenge_bytes(&self) -> Result<[u8; CAPABILITY_PROOF_CHALLENGE_LEN]> {
        decode_fixed_base64url::<CAPABILITY_PROOF_CHALLENGE_LEN>(
            &self.claims.challenge,
            "capability proof challenge",
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapabilityProofVerificationContext {
    pub expected_build_protocol_digest: String,
    pub expected_policy_revision: u64,
    pub expected_challenge: [u8; CAPABILITY_PROOF_CHALLENGE_LEN],
    pub now_unix_seconds: i64,
}

pub fn sign_capability_proof(
    identity: &DeviceTrustPublicIdentity,
    signing_key: &SigningKey,
    evaluation: &CapabilityEvaluation,
    request: &CapabilityProofRequest,
) -> Result<SignedCapabilityProof> {
    ensure!(
        signing_key.verifying_key().to_bytes() == identity.signing_public_key,
        "secure mesh capability proof signing key does not match endpoint identity"
    );
    validate_sha256_digest(&request.build_protocol_digest, "build protocol digest")?;
    validate_proof_lifetime(
        request.issued_at_unix_seconds,
        request.expires_at_unix_seconds,
    )?;
    let catalog = capability_catalog()?;
    ensure!(
        evaluation.catalog_digest() == catalog.digest(),
        "secure mesh capability proof catalog binding is invalid"
    );

    let claims = CapabilityProofClaims {
        schema_version: CAPABILITY_PROOF_SCHEMA_VERSION,
        endpoint_identity_fingerprint: identity.fingerprint()?,
        capability_catalog_digest: evaluation.catalog_digest().to_string(),
        build_protocol_digest: request.build_protocol_digest.clone(),
        policy_revision: request.policy_revision,
        challenge: general_purpose::URL_SAFE_NO_PAD.encode(request.challenge),
        issued_at_unix_seconds: request.issued_at_unix_seconds,
        expires_at_unix_seconds: request.expires_at_unix_seconds,
        mandatory_foundation_complete: evaluation.mandatory_foundation_complete(),
        enabled: evaluation.enabled().clone(),
        available: evaluation.available().clone(),
        unavailable: evaluation.unavailable().clone(),
        unverified: evaluation.unverified().clone(),
        missing_mandatory: evaluation.missing_mandatory().clone(),
        reasons: evaluation.reasons().clone(),
    };
    validate_claims(&claims, catalog)?;
    let signature_payload = capability_proof_signature_payload(&claims)?;
    let signature = signing_key.sign(&signature_payload);
    Ok(SignedCapabilityProof {
        claims,
        signature: general_purpose::URL_SAFE_NO_PAD.encode(signature.to_bytes()),
    })
}

pub fn verify_capability_proof(
    identity: &DeviceTrustPublicIdentity,
    proof: &SignedCapabilityProof,
    context: &CapabilityProofVerificationContext,
) -> Result<VerifiedCapabilityProof> {
    let catalog = capability_catalog()?;
    validate_claims(&proof.claims, catalog)?;
    ensure!(
        proof.claims.endpoint_identity_fingerprint == identity.fingerprint()?,
        "secure mesh capability proof endpoint identity binding failed"
    );
    ensure!(
        proof.claims.build_protocol_digest == context.expected_build_protocol_digest,
        "secure mesh capability proof build protocol binding failed"
    );
    validate_sha256_digest(
        &context.expected_build_protocol_digest,
        "expected build protocol digest",
    )?;
    ensure!(
        proof.claims.policy_revision == context.expected_policy_revision,
        "secure mesh capability proof policy revision binding failed"
    );
    ensure!(
        proof.claims.challenge
            == general_purpose::URL_SAFE_NO_PAD.encode(context.expected_challenge),
        "secure mesh capability proof challenge binding failed"
    );
    validate_freshness(&proof.claims, context.now_unix_seconds)?;

    let signature_bytes = decode_fixed_base64url::<CAPABILITY_PROOF_SIGNATURE_LEN>(
        &proof.signature,
        "capability proof signature",
    )?;
    let signature = Signature::from_bytes(&signature_bytes);
    identity
        .signing_verifying_key()?
        .verify_strict(
            &capability_proof_signature_payload(&proof.claims)?,
            &signature,
        )
        .map_err(|_| anyhow!("secure mesh capability proof signature verification failed"))?;

    let proof_digest = capability_proof_digest_bytes(proof)?;
    Ok(VerifiedCapabilityProof {
        claims: proof.claims.clone(),
        proof_digest,
        projection: projection_from_claims(&proof.claims),
    })
}

pub fn decode_signed_capability_proof_json(source: &[u8]) -> Result<SignedCapabilityProof> {
    ensure!(
        source.len() <= CAPABILITY_PROOF_MAX_JSON_BYTES,
        "secure mesh capability proof document is too large"
    );
    serde_json::from_slice(source)
        .map_err(|_| anyhow!("secure mesh capability proof document is invalid"))
}

pub fn encode_signed_capability_proof_json(proof: &SignedCapabilityProof) -> Result<Vec<u8>> {
    let encoded = serde_json::to_vec(proof)
        .map_err(|_| anyhow!("secure mesh capability proof document encoding failed"))?;
    ensure!(
        encoded.len() <= CAPABILITY_PROOF_MAX_JSON_BYTES,
        "secure mesh capability proof document is too large"
    );
    Ok(encoded)
}

pub(crate) fn signed_capability_proof_digest(proof: &SignedCapabilityProof) -> Result<String> {
    Ok(encode_sha256_digest(&capability_proof_digest_bytes(proof)?))
}

pub(crate) fn signed_capability_proof_challenge(
    proof: &SignedCapabilityProof,
) -> Result<[u8; CAPABILITY_PROOF_CHALLENGE_LEN]> {
    decode_fixed_base64url::<CAPABILITY_PROOF_CHALLENGE_LEN>(
        &proof.claims.challenge,
        "capability proof challenge",
    )
}

fn validate_claims(claims: &CapabilityProofClaims, catalog: &CapabilityCatalog) -> Result<()> {
    ensure!(
        claims.schema_version == CAPABILITY_PROOF_SCHEMA_VERSION,
        "secure mesh capability proof schema is unsupported"
    );
    ensure!(
        claims.capability_catalog_digest == catalog.digest(),
        "secure mesh capability proof catalog binding is invalid"
    );
    validate_sha256_digest(
        &claims.endpoint_identity_fingerprint,
        "endpoint identity fingerprint",
    )?;
    validate_sha256_digest(&claims.build_protocol_digest, "build protocol digest")?;
    decode_fixed_base64url::<CAPABILITY_PROOF_CHALLENGE_LEN>(
        &claims.challenge,
        "capability proof challenge",
    )?;
    validate_proof_lifetime(
        claims.issued_at_unix_seconds,
        claims.expires_at_unix_seconds,
    )?;

    let all = catalog
        .definitions()
        .map(|definition| definition.capability)
        .collect::<BTreeSet<_>>();
    let mut partition = BTreeSet::new();
    for set in [&claims.available, &claims.unavailable, &claims.unverified] {
        ensure!(
            set.is_subset(&all),
            "secure mesh capability proof contains an unknown capability"
        );
        for capability in set.iter().copied() {
            ensure!(
                partition.insert(capability),
                "secure mesh capability proof exact sets overlap"
            );
        }
    }
    ensure!(
        partition == all,
        "secure mesh capability proof exact sets are incomplete"
    );
    ensure!(
        claims.enabled.is_subset(&claims.available),
        "secure mesh capability proof enables an unavailable capability"
    );

    let mut expected_enabled = BTreeSet::new();
    for capability in catalog.topological_order() {
        let definition = catalog
            .definition(*capability)
            .ok_or_else(|| anyhow!("secure mesh capability proof catalog is incomplete"))?;
        let dependencies_enabled = definition
            .prerequisites
            .iter()
            .all(|prerequisite| expected_enabled.contains(prerequisite));
        if definition.derived {
            ensure!(
                claims.available.contains(capability) == dependencies_enabled,
                "secure mesh capability proof derived capability claim is invalid"
            );
        }
        if claims.available.contains(capability) && dependencies_enabled {
            expected_enabled.insert(*capability);
        }
    }
    ensure!(
        claims.enabled == expected_enabled,
        "secure mesh capability proof dependency closure is invalid"
    );

    let expected_missing_mandatory = catalog
        .definitions()
        .filter(|definition| definition.mandatory)
        .map(|definition| definition.capability)
        .filter(|capability| !claims.enabled.contains(capability))
        .collect::<BTreeSet<_>>();
    ensure!(
        claims.missing_mandatory == expected_missing_mandatory,
        "secure mesh capability proof mandatory capability set is invalid"
    );
    ensure!(
        claims.mandatory_foundation_complete == expected_missing_mandatory.is_empty(),
        "secure mesh capability proof mandatory foundation claim is invalid"
    );

    let expected_reason_capabilities = all
        .difference(&claims.enabled)
        .copied()
        .collect::<BTreeSet<_>>();
    let reason_capabilities = claims.reasons.keys().copied().collect::<BTreeSet<_>>();
    ensure!(
        reason_capabilities == expected_reason_capabilities,
        "secure mesh capability proof reason coverage is invalid"
    );
    for (capability, reason) in &claims.reasons {
        validate_reason_code(reason)?;
        if claims.available.contains(capability) {
            ensure!(
                reason == "capability_dependency_unmet",
                "secure mesh capability proof dependency reason is invalid"
            );
        }
    }
    Ok(())
}

fn validate_proof_lifetime(issued_at: i64, expires_at: i64) -> Result<()> {
    ensure!(
        expires_at > issued_at,
        "secure mesh capability proof expiry must follow issue time"
    );
    let lifetime = expires_at
        .checked_sub(issued_at)
        .ok_or_else(|| anyhow!("secure mesh capability proof lifetime is invalid"))?;
    ensure!(
        lifetime <= CAPABILITY_PROOF_MAX_LIFETIME_SECONDS,
        "secure mesh capability proof lifetime exceeds policy"
    );
    Ok(())
}

fn validate_freshness(claims: &CapabilityProofClaims, now: i64) -> Result<()> {
    let latest_acceptable_issue = now
        .checked_add(CAPABILITY_PROOF_MAX_CLOCK_SKEW_SECONDS)
        .ok_or_else(|| anyhow!("secure mesh capability proof time is invalid"))?;
    ensure!(
        claims.issued_at_unix_seconds <= latest_acceptable_issue,
        "secure mesh capability proof issue time is in the future"
    );
    ensure!(
        now <= claims.expires_at_unix_seconds,
        "secure mesh capability proof is stale"
    );
    let oldest_acceptable_issue = now
        .checked_sub(
            CAPABILITY_PROOF_MAX_LIFETIME_SECONDS + CAPABILITY_PROOF_MAX_CLOCK_SKEW_SECONDS,
        )
        .ok_or_else(|| anyhow!("secure mesh capability proof time is invalid"))?;
    ensure!(
        claims.issued_at_unix_seconds >= oldest_acceptable_issue,
        "secure mesh capability proof is stale"
    );
    Ok(())
}

fn validate_reason_code(reason: &str) -> Result<()> {
    ensure!(
        !reason.is_empty()
            && reason.len() <= CAPABILITY_PROOF_MAX_REASON_CODE_LEN
            && reason.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'_' | b'-')
            }),
        "secure mesh capability proof reason code is invalid"
    );
    Ok(())
}

fn projection_from_claims(claims: &CapabilityProofClaims) -> CapabilitySetProjection {
    CapabilitySetProjection {
        schema_version: CLIENT_CAPABILITY_PROJECTION_SCHEMA_VERSION,
        catalog_digest: claims.capability_catalog_digest.clone(),
        mandatory_foundation_complete: claims.mandatory_foundation_complete,
        enabled: claims.enabled.clone(),
        available: claims.available.clone(),
        unavailable: claims.unavailable.clone(),
        unverified: claims.unverified.clone(),
        missing_mandatory: claims.missing_mandatory.clone(),
        reasons: claims
            .reasons
            .iter()
            .map(|(capability, reason)| (capability.id().to_string(), reason.clone()))
            .collect(),
        custody: custody_selection_from_enabled(&claims.enabled),
    }
}

pub fn capability_projection_from_evaluation(
    evaluation: &CapabilityEvaluation,
) -> CapabilitySetProjection {
    CapabilitySetProjection {
        schema_version: CLIENT_CAPABILITY_PROJECTION_SCHEMA_VERSION,
        catalog_digest: evaluation.catalog_digest().to_string(),
        mandatory_foundation_complete: evaluation.mandatory_foundation_complete(),
        enabled: evaluation.enabled().clone(),
        available: evaluation.available().clone(),
        unavailable: evaluation.unavailable().clone(),
        unverified: evaluation.unverified().clone(),
        missing_mandatory: evaluation.missing_mandatory().clone(),
        reasons: evaluation
            .reasons()
            .iter()
            .map(|(capability, reason)| (capability.id().to_string(), reason.clone()))
            .collect(),
        custody: evaluation.custody().cloned(),
    }
}

fn capability_proof_signature_payload(claims: &CapabilityProofClaims) -> Result<Vec<u8>> {
    let claims = canonical_claims_bytes(claims)?;
    let mut payload = Vec::with_capacity(CAPABILITY_PROOF_SIGNATURE_MAGIC.len() + claims.len() + 4);
    payload.extend_from_slice(CAPABILITY_PROOF_SIGNATURE_MAGIC);
    append_len_prefixed_bytes(&mut payload, &claims)?;
    Ok(payload)
}

fn capability_proof_digest_bytes(
    proof: &SignedCapabilityProof,
) -> Result<[u8; CAPABILITY_PROOF_DIGEST_LEN]> {
    let claims = canonical_claims_bytes(&proof.claims)?;
    let signature = decode_fixed_base64url::<CAPABILITY_PROOF_SIGNATURE_LEN>(
        &proof.signature,
        "capability proof signature",
    )?;
    let mut transcript = Vec::new();
    transcript.extend_from_slice(CAPABILITY_PROOF_DIGEST_MAGIC);
    append_len_prefixed_bytes(&mut transcript, &claims)?;
    append_len_prefixed_bytes(&mut transcript, &signature)?;
    Ok(Sha256::digest(transcript).into())
}

fn canonical_claims_bytes(claims: &CapabilityProofClaims) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(&claims.schema_version.to_be_bytes());
    append_len_prefixed_bytes(
        &mut out,
        &decode_sha256_digest(
            &claims.endpoint_identity_fingerprint,
            "identity fingerprint",
        )?,
    )?;
    append_len_prefixed_bytes(&mut out, claims.capability_catalog_digest.as_bytes())?;
    append_len_prefixed_bytes(
        &mut out,
        &decode_sha256_digest(&claims.build_protocol_digest, "build protocol digest")?,
    )?;
    out.extend_from_slice(&claims.policy_revision.to_be_bytes());
    append_len_prefixed_bytes(
        &mut out,
        &decode_fixed_base64url::<CAPABILITY_PROOF_CHALLENGE_LEN>(
            &claims.challenge,
            "capability proof challenge",
        )?,
    )?;
    out.extend_from_slice(&claims.issued_at_unix_seconds.to_be_bytes());
    out.extend_from_slice(&claims.expires_at_unix_seconds.to_be_bytes());
    out.push(u8::from(claims.mandatory_foundation_complete));
    append_capability_set(&mut out, &claims.enabled)?;
    append_capability_set(&mut out, &claims.available)?;
    append_capability_set(&mut out, &claims.unavailable)?;
    append_capability_set(&mut out, &claims.unverified)?;
    append_capability_set(&mut out, &claims.missing_mandatory)?;
    append_reason_map(&mut out, &claims.reasons)?;
    Ok(out)
}

fn append_capability_set(
    out: &mut Vec<u8>,
    capabilities: &BTreeSet<SecurityCapability>,
) -> Result<()> {
    let mut ids = capabilities
        .iter()
        .map(|capability| capability.id())
        .collect::<Vec<_>>();
    ids.sort_unstable();
    append_count(out, ids.len())?;
    for id in ids {
        append_len_prefixed_bytes(out, id.as_bytes())?;
    }
    Ok(())
}

fn append_reason_map(
    out: &mut Vec<u8>,
    reasons: &BTreeMap<SecurityCapability, String>,
) -> Result<()> {
    let mut entries = reasons.iter().collect::<Vec<_>>();
    entries.sort_unstable_by_key(|(capability, _)| capability.id());
    append_count(out, entries.len())?;
    for (capability, reason) in entries {
        append_len_prefixed_bytes(out, capability.id().as_bytes())?;
        append_len_prefixed_bytes(out, reason.as_bytes())?;
    }
    Ok(())
}

fn append_count(out: &mut Vec<u8>, count: usize) -> Result<()> {
    let count = u32::try_from(count)
        .map_err(|_| anyhow!("secure mesh capability proof collection is too large"))?;
    out.extend_from_slice(&count.to_be_bytes());
    Ok(())
}

pub(crate) fn append_len_prefixed_bytes(out: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    let len = u32::try_from(value.len())
        .map_err(|_| anyhow!("secure mesh canonical transcript field is too large"))?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value);
    Ok(())
}

pub(crate) fn encode_sha256_digest(digest: &[u8; CAPABILITY_PROOF_DIGEST_LEN]) -> String {
    format!("sha256:{}", general_purpose::URL_SAFE_NO_PAD.encode(digest))
}

pub(crate) fn decode_sha256_digest(
    value: &str,
    label: &str,
) -> Result<[u8; CAPABILITY_PROOF_DIGEST_LEN]> {
    let encoded = value
        .strip_prefix("sha256:")
        .ok_or_else(|| anyhow!("secure mesh {label} format is invalid"))?;
    let decoded = decode_fixed_base64url::<CAPABILITY_PROOF_DIGEST_LEN>(encoded, label)?;
    ensure!(
        encode_sha256_digest(&decoded) == value,
        "secure mesh {label} encoding is not canonical"
    );
    Ok(decoded)
}

fn validate_sha256_digest(value: &str, label: &str) -> Result<()> {
    decode_sha256_digest(value, label).map(|_| ())
}

fn decode_fixed_base64url<const N: usize>(value: &str, label: &str) -> Result<[u8; N]> {
    let decoded = general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| anyhow!("secure mesh {label} encoding is invalid"))?;
    let decoded: [u8; N] = decoded
        .try_into()
        .map_err(|_| anyhow!("secure mesh {label} length is invalid"))?;
    ensure!(
        general_purpose::URL_SAFE_NO_PAD.encode(decoded) == value,
        "secure mesh {label} encoding is not canonical"
    );
    Ok(decoded)
}

#[cfg(test)]
mod tests;
