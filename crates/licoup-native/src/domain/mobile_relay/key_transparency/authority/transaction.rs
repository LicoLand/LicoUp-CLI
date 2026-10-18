use super::challenge::{
    KtAuthorityChallengeState, complete_kt_authority_challenge, stage_kt_authority_challenge,
    verify_kt_authority_challenge,
};
use super::proposal::{KtAuthorityProposal, parse_kt_authority_proposal};
use super::reset::reset_authority_state_if_required;
use crate::core::secure_mesh_transparency::KT_JSON_SAFE_INTEGER_MAX;
use crate::domain::mobile_relay::key_transparency::config::{
    AUTHORITY_GENERATION_FIELD, CONFIG_SCHEMA_VERSION, complete_kt_authority_reset,
    config_contains_native_store_secret_material, config_generation, kt_authority_reset_failpoint,
    kt_authority_reset_in_progress, load_config_with_runtime_secret_context_for_authority_reset,
    load_config_without_persistence, read_persisted_config, save_config_raw,
    save_config_with_runtime_secret_context,
    save_config_with_runtime_secret_context_for_authority_reset,
};
use crate::domain::mobile_relay::key_transparency::projection::authority_configuration_response;
use crate::domain::mobile_relay::support::{bool_param, ensure_only_known_params, text_param};
use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};

pub(in crate::domain::mobile_relay) fn key_transparency_configure_authority(
    params: &Value,
) -> Result<Value> {
    ensure_only_known_params(
        params,
        &[
            "operation",
            "authorityChallengeId",
            "confirmAuthorityConfiguration",
            "confirmSecurityReset",
            "directoryScopeCommitment",
            "pin",
            "maxSthAgeSeconds",
            "maxFutureSkewSeconds",
            "allowInteraction",
            "secretOverrideTransport",
            "secretOverrides",
        ],
        "secure mesh KT authority configuration",
    )?;
    let proposal = parse_kt_authority_proposal(params)?;
    let operation = text_param(params, &["operation"]).unwrap_or_else(|| "prepare".to_string());
    if operation == "prepare" {
        return prepare_authority_configuration(params, &proposal);
    }
    ensure!(
        operation == "confirm",
        "secure mesh KT authority configuration operation must be prepare or confirm"
    );
    confirm_authority_configuration(params, proposal)
}

fn prepare_authority_configuration(
    params: &Value,
    proposal: &KtAuthorityProposal,
) -> Result<Value> {
    ensure!(
        bool_param(params, &["confirmAuthorityConfiguration"]) != Some(true)
            && params.get("authorityChallengeId").is_none(),
        "secure mesh KT authority preparation cannot confirm its own challenge"
    );
    let mut config = load_config_without_persistence()?;
    let persisted = read_persisted_config()?;
    if persisted.as_ref() != Some(&config) {
        ensure!(
            !config_contains_native_store_secret_material(&config),
            "secure mesh KT authority preparation requires prior authorized secret migration"
        );
        save_config_raw(&mut config)?;
    }
    stage_kt_authority_challenge(&config, proposal)
}

fn confirm_authority_configuration(params: &Value, proposal: KtAuthorityProposal) -> Result<Value> {
    ensure!(
        bool_param(params, &["confirmAuthorityConfiguration"]) == Some(true),
        "secure mesh KT authority configuration requires explicit user confirmation"
    );
    ensure!(
        bool_param(params, &["allowInteraction"]) == Some(true),
        "secure mesh KT authority confirmation requires foreground user interaction"
    );
    let challenge_id = text_param(params, &["authorityChallengeId"])
        .ok_or_else(|| anyhow!("secure mesh KT authority confirmation challenge id is required"))?;
    let (mut config, mut secret_context) =
        load_config_with_runtime_secret_context_for_authority_reset(params)?;
    let challenge_state = verify_kt_authority_challenge(&config, &proposal, &challenge_id)?;
    if let KtAuthorityChallengeState::AlreadyCommitted {
        required_security_reset,
    } = challenge_state
    {
        if kt_authority_reset_in_progress()? {
            complete_kt_authority_reset()?;
        }
        complete_kt_authority_challenge()?;
        return Ok(authority_configuration_response(
            CONFIG_SCHEMA_VERSION,
            proposal.pin.provenance().stable_code(),
            proposal.pin.provenance().is_mock(),
            proposal.pin.provenance().production_service_claim_allowed(),
            required_security_reset,
            true,
        ));
    }
    let KtAuthorityChallengeState::Pending {
        requires_security_reset,
    } = challenge_state
    else {
        unreachable!("committed challenge returned above")
    };

    let existing = config
        .get("secureMeshKeyTransparency")
        .filter(|value| value.is_object())
        .cloned();
    let existing_scope = config
        .get("secureMeshDirectoryScopeCommitment")
        .and_then(Value::as_str);
    let has_existing_authority_root = existing.is_some() || existing_scope.is_some();
    let authority_changed = has_existing_authority_root
        && (existing.as_ref().and_then(|settings| settings.get("pin"))
            != Some(&proposal.pin_value)
            || existing_scope != Some(proposal.scope.as_str()));
    let reset_in_progress = kt_authority_reset_in_progress()?;
    ensure!(
        requires_security_reset == (authority_changed || reset_in_progress),
        "secure mesh KT authority challenge reset binding mismatch"
    );
    reset_authority_state_if_required(
        &mut config,
        &mut secret_context,
        params,
        authority_changed,
        reset_in_progress,
    )?;

    config["secureMeshDirectoryScopeCommitment"] = json!(proposal.scope);
    config["secureMeshKeyTransparency"] = json!({
        "pin": proposal.pin_value,
        "maxSthAgeSeconds": proposal.max_sth_age_seconds,
        "maxFutureSkewSeconds": proposal.max_future_skew_seconds
    });
    if authority_changed {
        let next_authority_generation = config_generation(&config, AUTHORITY_GENERATION_FIELD)?
            .checked_add(1)
            .filter(|generation| *generation <= KT_JSON_SAFE_INTEGER_MAX)
            .ok_or_else(|| anyhow!("mobile relay authority generation overflow"))?;
        config[AUTHORITY_GENERATION_FIELD] = json!(next_authority_generation);
    }
    if authority_changed || reset_in_progress {
        save_config_with_runtime_secret_context_for_authority_reset(
            &mut config,
            &mut secret_context,
        )?;
        kt_authority_reset_failpoint("after_replacement_config_persisted")?;
        complete_kt_authority_reset()?;
    } else {
        save_config_with_runtime_secret_context(&mut config, &mut secret_context)?;
    }
    complete_kt_authority_challenge()?;
    Ok(authority_configuration_response(
        CONFIG_SCHEMA_VERSION,
        proposal.pin.provenance().stable_code(),
        proposal.pin.provenance().is_mock(),
        proposal.pin.provenance().production_service_claim_allowed(),
        authority_changed || reset_in_progress,
        false,
    ))
}
