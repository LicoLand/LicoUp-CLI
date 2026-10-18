use crate::core::secure_mesh_directory::{
    SecureMeshDirectoryAuthority, SecureMeshKtVerifierConfiguration,
};
use crate::core::secure_mesh_transparency::KtFreshnessPolicy;
use crate::domain::mobile_relay::endpoint_trust::secure_mesh_kt_authority_path;
use crate::domain::mobile_relay::key_transparency::config::{
    CONFIG_SCHEMA_VERSION, kt_authority_reset_in_progress, load_config_without_persistence,
};
use crate::domain::mobile_relay::support::ensure_only_known_params;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};

pub(super) fn key_transparency_status(params: &Value) -> Result<Value> {
    ensure_only_known_params(params, &[], "secure mesh KT status")?;
    let config = load_config_without_persistence()?;
    let reset_guard = kt_authority_reset_in_progress();
    let reset_in_progress = reset_guard.as_ref().copied().unwrap_or(true);
    let guard_valid = reset_guard.is_ok();
    let settings = config
        .get("secureMeshKeyTransparency")
        .filter(|value| value.is_object())
        .cloned()
        .map(serde_json::from_value::<SecureMeshKtVerifierConfiguration>)
        .transpose()
        .map_err(|_| anyhow!("secure mesh KT local verifier configuration is invalid"))?;
    let pin = settings
        .as_ref()
        .map(|settings| settings.pin.clone().into_pin())
        .transpose()?;
    let endpoint_id = config
        .get("mobileRelayE2ee")
        .and_then(|state| state.get("endpointId"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    let (checkpoint, security_blocked) = if !reset_in_progress {
        if let (Some(endpoint_id), Some(settings)) = (endpoint_id, settings.as_ref()) {
            let authority = SecureMeshDirectoryAuthority::open(
                secure_mesh_kt_authority_path(endpoint_id)?,
                settings.pin.clone().into_pin()?,
                KtFreshnessPolicy::strict(
                    settings.max_sth_age_seconds,
                    settings.max_future_skew_seconds,
                )?,
            )?;
            (
                authority.latest_checkpoint()?,
                authority.security_blocked()?,
            )
        } else {
            (None, false)
        }
    } else {
        (None, true)
    };
    Ok(json!({
        "ok": guard_valid,
        "schemaVersion": CONFIG_SCHEMA_VERSION,
        "configured": settings.is_some(),
        "authorityProvenance": pin.as_ref().map(|pin| pin.provenance().stable_code()),
        "mock": pin.as_ref().is_some_and(|pin| pin.provenance().is_mock()),
        "productionAuthority": pin.as_ref().is_some_and(|pin| pin.provenance().production_service_claim_allowed()),
        "directoryScopeCommitted": config
            .get("secureMeshDirectoryScopeCommitment")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty()),
        "resetInProgress": reset_in_progress,
        "guardValid": guard_valid,
        "securityBlocked": security_blocked,
        "latestCheckpoint": checkpoint.map(|checkpoint| json!({
            "treeSize": checkpoint.tree_size,
            "issuedAtEpochSeconds": checkpoint.issued_at_epoch_seconds,
            "rootCommitted": true,
            "mapRootCommitted": true
        }))
    }))
}
