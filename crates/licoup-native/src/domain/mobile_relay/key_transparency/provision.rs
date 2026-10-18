use crate::core::secure_mesh_directory::{
    DirectoryAuthorizationPurpose, SecureMeshDirectoryLeafClaim, UntrustedDirectoryResponse,
};
use crate::domain::mobile_relay::endpoint_trust::{
    authorize_exact_local_directory_response, configured_directory_scope_commitment,
    configured_kt_pin, ensure_mobile_relay_key_transparency,
    parse_local_directory_authorization_purpose,
};
use crate::domain::mobile_relay::key_transparency::config::{
    CONFIG_SCHEMA_VERSION, load_config_with_runtime_secret_context,
    save_config_with_runtime_secret_context,
};
use crate::domain::mobile_relay::support::ensure_only_known_params;
use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};
use time::OffsetDateTime;

/// Verify a service response against the persisted pending claim. Transport
/// input cannot select purpose or replace authority roots.
pub(super) fn key_transparency_provision(params: &Value) -> Result<Value> {
    ensure_only_known_params(
        params,
        &[
            "response",
            "allowInteraction",
            "secretOverrideTransport",
            "secretOverrides",
        ],
        "secure mesh KT provision",
    )?;
    for forbidden in ["pin", "directoryScopeCommitment", "authorizationPurpose"] {
        ensure!(
            params.get(forbidden).is_none(),
            "secure mesh KT provision cannot replace local authority configuration or purpose"
        );
    }
    let (mut config, mut secret_context) = load_config_with_runtime_secret_context(params)?;
    let scope = configured_directory_scope_commitment(&config)?.to_string();
    let pin = configured_kt_pin(&config)?;
    let response_value = params
        .get("response")
        .filter(|value| value.is_object())
        .cloned()
        .ok_or_else(|| anyhow!("secure mesh KT directory response is required"))?;
    let response: UntrustedDirectoryResponse = serde_json::from_value(response_value.clone())
        .map_err(|_| anyhow!("secure mesh KT directory response is invalid"))?;
    let state = config
        .get("mobileRelayE2ee")
        .ok_or_else(|| anyhow!("mobile relay E2EE endpoint state is missing"))?;
    let pending: SecureMeshDirectoryLeafClaim = serde_json::from_value(
        state
            .get("pendingKeyTransparencyClaim")
            .cloned()
            .ok_or_else(|| anyhow!("secure mesh pending KT publication claim is required"))?,
    )
    .map_err(|_| anyhow!("secure mesh pending KT publication claim is invalid"))?;
    let purpose = state
        .get("pendingKeyTransparencyPurpose")
        .and_then(Value::as_str)
        .map(parse_local_directory_authorization_purpose)
        .transpose()?
        .ok_or_else(|| anyhow!("secure mesh pending KT publication purpose is missing"))?;
    ensure!(
        pending.endpoint.directory_scope_commitment == scope
            && response.claim == pending
            && response.claim.leaf_hash()? == pending.leaf_hash()?,
        "secure mesh KT service response does not match the exact pending local claim"
    );
    config["mobileRelayE2ee"]["keyTransparencyResponse"] = response_value.clone();
    let authorized = authorize_exact_local_directory_response(
        &config,
        response_value.clone(),
        &pending,
        OffsetDateTime::now_utc(),
        purpose,
    )?;
    let mls_key_package_authorized = if pending.revoked() {
        ensure!(
            purpose == DirectoryAuthorizationPurpose::Revocation,
            "secure mesh revoked local claim requires revocation authorization"
        );
        None
    } else {
        let mls_authorized = authorize_exact_local_directory_response(
            &config,
            response_value,
            &pending,
            OffsetDateTime::now_utc(),
            DirectoryAuthorizationPurpose::MlsKeyPackage,
        )?;
        ensure_mobile_relay_key_transparency(&mut config)?;
        Some(mls_authorized.authorization_digest().to_string())
    };
    config["mobileRelayE2ee"]["directoryVersion"] = json!(pending.directory_version);
    config["mobileRelayE2ee"]["mlsKeyPackageVersion"] =
        json!(pending.key_material.mls_key_package_version);
    config["mobileRelayE2ee"]["mlsKeyPackageDigest"] =
        json!(pending.key_material.mls_key_package_digest);
    if let Some(e2ee) = config
        .get_mut("mobileRelayE2ee")
        .and_then(Value::as_object_mut)
    {
        e2ee.remove("pendingKeyTransparencyClaim");
        e2ee.remove("pendingKeyTransparencyPurpose");
    }
    save_config_with_runtime_secret_context(&mut config, &mut secret_context)?;
    Ok(json!({
        "ok": true,
        "schemaVersion": CONFIG_SCHEMA_VERSION,
        "authorityProvenance": pin.provenance().stable_code(),
        "mock": pin.provenance().is_mock(),
        "productionAuthority": pin.provenance().production_service_claim_allowed(),
        "purpose": purpose.stable_code(),
        "treeSize": authorized.signed_tree_head().tree_size,
        "authorizationDigest": authorized.authorization_digest(),
        "mlsKeyPackageAuthorizationDigest": mls_key_package_authorized
    }))
}
