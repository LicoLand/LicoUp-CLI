use crate::core::secure_mesh_directory::{
    DirectoryAuthorizationPurpose, UntrustedDirectoryResponse,
};
use crate::domain::mobile_relay::endpoint_trust::{
    build_local_directory_claim, configured_directory_scope_commitment, configured_kt_pin,
};
use crate::domain::mobile_relay::key_transparency::config::{
    CONFIG_SCHEMA_VERSION, load_config_with_runtime_secret_context,
    save_config_with_runtime_secret_context,
};
use crate::domain::mobile_relay::support::{bool_param, ensure_only_known_params};
use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};

pub(super) fn key_transparency_revocation_request(params: &Value) -> Result<Value> {
    ensure_only_known_params(
        params,
        &[
            "confirmRevocation",
            "allowInteraction",
            "secretOverrideTransport",
            "secretOverrides",
        ],
        "secure mesh KT revocation request",
    )?;
    ensure!(
        bool_param(params, &["confirmRevocation"]) == Some(true),
        "secure mesh directory revocation requires explicit user confirmation"
    );
    let (mut config, mut secret_context) = load_config_with_runtime_secret_context(params)?;
    let scope = configured_directory_scope_commitment(&config)?.to_string();
    let _ = configured_kt_pin(&config)?;
    let state = config
        .get("mobileRelayE2ee")
        .ok_or_else(|| anyhow!("mobile relay E2EE endpoint state is missing"))?;
    let current: UntrustedDirectoryResponse = serde_json::from_value(
        state
            .get("keyTransparencyResponse")
            .cloned()
            .ok_or_else(|| anyhow!("secure mesh current KT directory response is required"))?,
    )
    .map_err(|_| anyhow!("secure mesh current KT directory response is invalid"))?;
    let directory_version = current
        .claim
        .directory_version
        .checked_add(1)
        .ok_or_else(|| anyhow!("secure mesh directory revocation version overflow"))?;
    let claim = build_local_directory_claim(
        &config,
        &scope,
        directory_version,
        "revoked",
        &current.claim.key_material.mls_key_package_digest,
        current.claim.key_material.mls_key_package_version,
    )?;
    config["mobileRelayE2ee"]["pendingKeyTransparencyClaim"] = serde_json::to_value(&claim)?;
    config["mobileRelayE2ee"]["pendingKeyTransparencyPurpose"] =
        json!(DirectoryAuthorizationPurpose::Revocation.stable_code());
    save_config_with_runtime_secret_context(&mut config, &mut secret_context)?;
    Ok(json!({
        "ok": true,
        "schemaVersion": CONFIG_SCHEMA_VERSION,
        "authorityOperation": "publish-directory-revocation",
        "claim": claim,
        "derivedPurpose": DirectoryAuthorizationPurpose::Revocation.stable_code()
    }))
}
