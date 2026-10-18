use crate::core::secure_mesh_transparency::KT_JSON_SAFE_INTEGER_MAX;
use crate::domain::mobile_relay::endpoint_trust::{
    build_local_directory_claim, configured_directory_scope_commitment, configured_kt_pin,
    derive_local_publication_purpose, ensure_mobile_relay_endpoint_material,
    validate_canonical_sha256_hex,
};
use crate::domain::mobile_relay::key_transparency::config::{
    CONFIG_SCHEMA_VERSION, load_config_with_runtime_secret_context,
    save_config_with_runtime_secret_context,
};
use crate::domain::mobile_relay::support::{ensure_only_known_params, text_param};
use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};

/// Prepare the exact public directory claim for the preconfigured authority.
/// A real MLS KeyPackage publication must already exist.
pub(super) fn key_transparency_publication_request(params: &Value) -> Result<Value> {
    ensure_only_known_params(
        params,
        &[
            "endpointKind",
            "allowInteraction",
            "secretOverrideTransport",
            "secretOverrides",
        ],
        "secure mesh KT publication request",
    )?;
    let (mut config, mut secret_context) = load_config_with_runtime_secret_context(params)?;
    let endpoint_kind = text_param(params, &["endpointKind"])
        .or_else(|| {
            config
                .get("mobileRelayE2ee")
                .and_then(|state| state.get("endpointKind"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "desktop_sidecar".to_string());
    let scope = configured_directory_scope_commitment(&config)?.to_string();
    let _ = configured_kt_pin(&config)?;
    ensure_mobile_relay_endpoint_material(
        &mut config,
        &mut secret_context.material,
        &endpoint_kind,
    )?;
    let state = config
        .get("mobileRelayE2ee")
        .ok_or_else(|| anyhow!("mobile relay E2EE endpoint state is missing"))?;
    let current_directory_version = state
        .get("directoryVersion")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let directory_version = current_directory_version
        .checked_add(1)
        .ok_or_else(|| anyhow!("secure mesh directory publication version overflow"))?;
    ensure!(
        directory_version <= KT_JSON_SAFE_INTEGER_MAX,
        "secure mesh directory publication version exceeds the cross-language safe range"
    );
    let mls_key_package_version = state
        .get("mlsKeyPackageVersion")
        .and_then(Value::as_u64)
        .filter(|version| *version > 0)
        .ok_or_else(|| anyhow!("secure mesh real MLS KeyPackage publication is required"))?;
    let mls_key_package_digest = state
        .get("mlsKeyPackageDigest")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("secure mesh real MLS KeyPackage digest is required"))?;
    validate_canonical_sha256_hex(mls_key_package_digest, "MLS KeyPackage digest")?;
    ensure!(
        mls_key_package_digest
            != "0000000000000000000000000000000000000000000000000000000000000000",
        "secure mesh MLS KeyPackage digest cannot be a sentinel"
    );
    let claim = build_local_directory_claim(
        &config,
        &scope,
        directory_version,
        "active",
        mls_key_package_digest,
        mls_key_package_version,
    )?;
    let purpose = derive_local_publication_purpose(&config, &claim)?;
    config["mobileRelayE2ee"]["pendingKeyTransparencyClaim"] = serde_json::to_value(&claim)?;
    config["mobileRelayE2ee"]["pendingKeyTransparencyPurpose"] = json!(purpose.stable_code());
    save_config_with_runtime_secret_context(&mut config, &mut secret_context)?;
    Ok(json!({
        "ok": true,
        "schemaVersion": CONFIG_SCHEMA_VERSION,
        "authorityOperation": "publish-directory-claim",
        "claim": claim,
        "derivedPurpose": purpose.stable_code(),
        "authorityRequired": true
    }))
}
