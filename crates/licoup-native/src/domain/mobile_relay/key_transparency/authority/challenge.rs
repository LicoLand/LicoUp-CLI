use super::proposal::{
    KtAuthorityProposal, authority_change_requires_reset, authority_configuration_matches,
};
use crate::domain::mobile_relay::endpoint_trust::{
    current_secure_mesh_kt_gate_epoch_seconds, random_base64url,
};
use crate::domain::mobile_relay::key_transparency::config::{
    AUTHORITY_GENERATION_FIELD, CONFIG_GENERATION_FIELD, CONFIG_SCHEMA_VERSION, config_generation,
    kt_authority_reset_in_progress,
};
use crate::domain::mobile_relay::key_transparency::persistence::{
    create_authority_challenge_marker, read_authority_challenge_marker,
    remove_authority_challenge_marker,
};
use crate::domain::mobile_relay::key_transparency::projection::authority_challenge_response;
use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};

const KT_AUTHORITY_CHALLENGE_SCHEMA_VERSION: u64 = 1;
const KT_AUTHORITY_CHALLENGE_TTL_SECONDS: u64 = 5 * 60;

pub(super) enum KtAuthorityChallengeState {
    Pending { requires_security_reset: bool },
    AlreadyCommitted { required_security_reset: bool },
}

pub(in crate::domain::mobile_relay) fn read_kt_authority_challenge() -> Result<Option<Value>> {
    let Some(raw) = read_authority_challenge_marker()? else {
        return Ok(None);
    };
    let challenge: Value = serde_json::from_slice(&raw)
        .map_err(|_| anyhow!("secure mesh KT authority challenge is invalid"))?;
    ensure!(
        challenge.get("schemaVersion").and_then(Value::as_u64)
            == Some(KT_AUTHORITY_CHALLENGE_SCHEMA_VERSION)
            && challenge
                .get("challengeId")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.trim().is_empty())
            && challenge
                .get("proposalDigest")
                .and_then(Value::as_str)
                .is_some_and(|value| value.len() == 64)
            && challenge
                .get("configGeneration")
                .and_then(Value::as_u64)
                .is_some()
            && challenge
                .get("authorityGeneration")
                .and_then(Value::as_u64)
                .is_some()
            && challenge
                .get("expiresAtEpochSeconds")
                .and_then(Value::as_u64)
                .is_some()
            && challenge
                .get("requiresSecurityReset")
                .and_then(Value::as_bool)
                .is_some(),
        "secure mesh KT authority challenge is invalid"
    );
    Ok(Some(challenge))
}

pub(super) fn stage_kt_authority_challenge(
    config: &Value,
    proposal: &KtAuthorityProposal,
) -> Result<Value> {
    let now = current_secure_mesh_kt_gate_epoch_seconds()?;
    let mesh_config_generation = config_generation(config, CONFIG_GENERATION_FIELD)?;
    let authority_generation = config_generation(config, AUTHORITY_GENERATION_FIELD)?;
    if let Some(existing) = read_kt_authority_challenge()? {
        let unexpired = existing["expiresAtEpochSeconds"]
            .as_u64()
            .is_some_and(|expires_at| now <= expires_at);
        let same_proposal = existing["proposalDigest"].as_str() == Some(proposal.digest.as_str())
            && existing["configGeneration"].as_u64() == Some(mesh_config_generation)
            && existing["authorityGeneration"].as_u64() == Some(authority_generation);
        if unexpired && same_proposal {
            return Ok(authority_challenge_response(
                &existing,
                CONFIG_SCHEMA_VERSION,
            ));
        }
        if unexpired {
            return Err(anyhow!(
                "a different secure mesh KT authority challenge is already pending"
            ));
        }
        ensure!(
            remove_authority_challenge_marker()?,
            "expired secure mesh KT authority challenge could not be removed"
        );
    }
    let challenge = json!({
        "schemaVersion": KT_AUTHORITY_CHALLENGE_SCHEMA_VERSION,
        "challengeId": random_base64url(24),
        "proposalDigest": proposal.digest,
        "configGeneration": mesh_config_generation,
        "authorityGeneration": authority_generation,
        "expiresAtEpochSeconds": now.saturating_add(KT_AUTHORITY_CHALLENGE_TTL_SECONDS),
        "requiresSecurityReset": authority_change_requires_reset(config, proposal)
            || kt_authority_reset_in_progress()?,
    });
    create_authority_challenge_marker(&serde_json::to_vec(&challenge)?)?;
    Ok(authority_challenge_response(
        &challenge,
        CONFIG_SCHEMA_VERSION,
    ))
}

pub(super) fn verify_kt_authority_challenge(
    config: &Value,
    proposal: &KtAuthorityProposal,
    challenge_id: &str,
) -> Result<KtAuthorityChallengeState> {
    let challenge = read_kt_authority_challenge()?
        .ok_or_else(|| anyhow!("secure mesh KT authority challenge is missing"))?;
    ensure!(
        challenge["challengeId"].as_str() == Some(challenge_id),
        "secure mesh KT authority challenge id mismatch"
    );
    ensure!(
        challenge["proposalDigest"].as_str() == Some(proposal.digest.as_str()),
        "secure mesh KT authority challenge proposal mismatch"
    );
    let prepared_config_generation = challenge["configGeneration"]
        .as_u64()
        .ok_or_else(|| anyhow!("secure mesh KT authority challenge is invalid"))?;
    let prepared_authority_generation = challenge["authorityGeneration"]
        .as_u64()
        .ok_or_else(|| anyhow!("secure mesh KT authority challenge is invalid"))?;
    let requires_security_reset = challenge["requiresSecurityReset"]
        .as_bool()
        .ok_or_else(|| anyhow!("secure mesh KT authority challenge is invalid"))?;
    let current_config_generation = config_generation(config, CONFIG_GENERATION_FIELD)?;
    let current_authority_generation = config_generation(config, AUTHORITY_GENERATION_FIELD)?;
    let committed_authority_generation =
        prepared_authority_generation.saturating_add(u64::from(requires_security_reset));
    if current_config_generation > prepared_config_generation
        && current_authority_generation == committed_authority_generation
        && authority_configuration_matches(config, proposal)
    {
        return Ok(KtAuthorityChallengeState::AlreadyCommitted {
            required_security_reset: requires_security_reset,
        });
    }
    ensure!(
        current_config_generation == prepared_config_generation
            && current_authority_generation == prepared_authority_generation,
        "secure mesh KT authority challenge generation is stale"
    );
    ensure!(
        current_secure_mesh_kt_gate_epoch_seconds()?
            <= challenge["expiresAtEpochSeconds"].as_u64().unwrap_or(0),
        "secure mesh KT authority challenge has expired"
    );
    Ok(KtAuthorityChallengeState::Pending {
        requires_security_reset,
    })
}

pub(super) fn complete_kt_authority_challenge() -> Result<()> {
    ensure!(
        remove_authority_challenge_marker()?,
        "secure mesh KT authority challenge is missing"
    );
    Ok(())
}
