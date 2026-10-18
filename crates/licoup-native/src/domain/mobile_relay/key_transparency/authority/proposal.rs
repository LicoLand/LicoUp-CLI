use crate::core::secure_mesh_directory::PinnedKtLogConfiguration;
use crate::core::secure_mesh_transparency::{KtFreshnessPolicy, PinnedKtLogKey};
use crate::domain::mobile_relay::endpoint_trust::{descriptor_sha256_hex, stable_json_sha256};
use anyhow::{Result, anyhow};
use serde_json::{Value, json};

pub(in crate::domain::mobile_relay) struct KtAuthorityProposal {
    pub(super) pin_value: Value,
    pub(super) pin: PinnedKtLogKey,
    pub(super) scope: String,
    pub(super) max_sth_age_seconds: u64,
    pub(super) max_future_skew_seconds: u64,
    pub(super) digest: String,
}

pub(in crate::domain::mobile_relay) fn parse_kt_authority_proposal(
    params: &Value,
) -> Result<KtAuthorityProposal> {
    let pin_value = params
        .get("pin")
        .filter(|value| value.is_object())
        .cloned()
        .ok_or_else(|| anyhow!("secure mesh KT explicit pinned log is required"))?;
    let pin_configuration: PinnedKtLogConfiguration = serde_json::from_value(pin_value.clone())
        .map_err(|_| anyhow!("secure mesh KT explicit pinned log is invalid"))?;
    let pin = pin_configuration.into_pin()?;
    let scope = descriptor_sha256_hex(params, "directoryScopeCommitment")?;
    let max_sth_age_seconds = params
        .get("maxSthAgeSeconds")
        .and_then(Value::as_u64)
        .unwrap_or(3600);
    let max_future_skew_seconds = params
        .get("maxFutureSkewSeconds")
        .and_then(Value::as_u64)
        .unwrap_or(300);
    KtFreshnessPolicy::strict(max_sth_age_seconds, max_future_skew_seconds)?;
    let digest = stable_json_sha256(&json!({
        "pin": pin_value,
        "directoryScopeCommitment": scope,
        "maxSthAgeSeconds": max_sth_age_seconds,
        "maxFutureSkewSeconds": max_future_skew_seconds,
    }));
    Ok(KtAuthorityProposal {
        pin_value,
        pin,
        scope,
        max_sth_age_seconds,
        max_future_skew_seconds,
        digest,
    })
}

pub(in crate::domain::mobile_relay) fn authority_configuration_matches(
    config: &Value,
    proposal: &KtAuthorityProposal,
) -> bool {
    config
        .get("secureMeshKeyTransparency")
        .and_then(|settings| settings.get("pin"))
        == Some(&proposal.pin_value)
        && config
            .get("secureMeshDirectoryScopeCommitment")
            .and_then(Value::as_str)
            == Some(proposal.scope.as_str())
        && config
            .get("secureMeshKeyTransparency")
            .and_then(|settings| settings.get("maxSthAgeSeconds"))
            .and_then(Value::as_u64)
            == Some(proposal.max_sth_age_seconds)
        && config
            .get("secureMeshKeyTransparency")
            .and_then(|settings| settings.get("maxFutureSkewSeconds"))
            .and_then(Value::as_u64)
            == Some(proposal.max_future_skew_seconds)
}

pub(super) fn authority_change_requires_reset(
    config: &Value,
    proposal: &KtAuthorityProposal,
) -> bool {
    let existing = config
        .get("secureMeshKeyTransparency")
        .filter(|value| value.is_object());
    let existing_scope = config
        .get("secureMeshDirectoryScopeCommitment")
        .and_then(Value::as_str);
    (existing.is_some() || existing_scope.is_some())
        && !authority_configuration_matches(config, proposal)
}
