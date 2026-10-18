use crate::core::secure_mesh_directory::{
    AuthorizedDirectoryLeaf, DirectoryAuthorizationPurpose, DirectoryAuthorizationRequest,
    UntrustedDirectoryResponse,
};
use crate::core::secure_mesh_prekey::{one_time_prekey_batch_digest, signed_prekey_bundle_digest};
use crate::domain::mobile_relay::endpoint_trust::{
    clear_mobile_relay_pairing_state, configured_directory_scope_commitment,
    current_secure_mesh_kt_gate_epoch_seconds, local_endpoint_state,
    open_mobile_relay_directory_authority, validate_canonical_sha256_hex,
};
use crate::domain::mobile_relay::key_transparency::config::{
    CONFIG_SCHEMA_VERSION, load_config_with_runtime_secret_context,
    save_config_with_runtime_secret_context,
};
use crate::domain::mobile_relay::support::ensure_only_known_params;
use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};

pub(super) fn key_transparency_self_monitor(params: &Value) -> Result<Value> {
    ensure_only_known_params(
        params,
        &[
            "response",
            "allowInteraction",
            "secretOverrideTransport",
            "secretOverrides",
        ],
        "secure mesh KT self monitor",
    )?;
    let (mut config, mut secret_context) = load_config_with_runtime_secret_context(params)?;
    let response_value = params
        .get("response")
        .filter(|value| value.is_object())
        .cloned()
        .ok_or_else(|| anyhow!("secure mesh KT self-monitor response is required"))?;
    let response: UntrustedDirectoryResponse = serde_json::from_value(response_value.clone())
        .map_err(|_| anyhow!("secure mesh KT self-monitor response is invalid"))?;
    let local = local_endpoint_state(&config, &secret_context.material)?;
    let identity = local.device_identity()?;
    let bundle = local.pairwise_prekey_bundle()?;
    let scope = configured_directory_scope_commitment(&config)?;
    ensure!(
        response.claim.endpoint.directory_scope_commitment == scope,
        "secure mesh KT self-monitor response scope differs from local authority"
    );
    let purpose = if response.claim.revoked() {
        DirectoryAuthorizationPurpose::Revocation
    } else {
        DirectoryAuthorizationPurpose::SelfMonitor
    };
    let signed_prekey_digest = signed_prekey_bundle_digest(&bundle)?;
    let one_time_prekey_digest = one_time_prekey_batch_digest(&bundle)?;
    let local_mls_key_package_digest = config
        .get("mobileRelayE2ee")
        .and_then(|state| state.get("mlsKeyPackageDigest"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("secure mesh local MLS KeyPackage digest is required"))?;
    validate_canonical_sha256_hex(&local_mls_key_package_digest, "local MLS KeyPackage digest")?;
    let local_mls_key_package_version = config
        .get("mobileRelayE2ee")
        .and_then(|state| state.get("mlsKeyPackageVersion"))
        .and_then(Value::as_u64)
        .filter(|version| *version > 0)
        .ok_or_else(|| anyhow!("secure mesh local MLS KeyPackage version is required"))?;
    let mut authority = open_mobile_relay_directory_authority(&config, &local.endpoint_id)?;
    let now_epoch_seconds = current_secure_mesh_kt_gate_epoch_seconds()?;
    #[cfg(test)]
    if config
        .get("secureMeshKeyTransparency")
        .and_then(|settings| settings.get("pin"))
        .and_then(|pin| pin.get("provenance"))
        .and_then(Value::as_str)
        == Some("local-acceptance-mock")
    {
        authority.observe_response_gossip_for_test(&response, now_epoch_seconds)?;
    }
    let authorized = authority.authorize_request(
        response.clone(),
        DirectoryAuthorizationRequest::for_full_subject(
            purpose,
            scope,
            &identity,
            response.claim.directory_version,
            &signed_prekey_digest,
            &one_time_prekey_digest,
            bundle.prekey_publication_version,
            &local_mls_key_package_digest,
            local_mls_key_package_version,
        ),
        now_epoch_seconds,
    )?;
    let mls_key_package_authorized = if authorized.claim().revoked() {
        None
    } else {
        Some(authority.authorize_request(
            response.clone(),
            DirectoryAuthorizationRequest::for_mls(
                DirectoryAuthorizationPurpose::MlsKeyPackage,
                scope,
                &identity,
                response.claim.directory_version,
                &local_mls_key_package_digest,
                local_mls_key_package_version,
            ),
            now_epoch_seconds,
        )?)
    };
    config["mobileRelayE2ee"]["keyTransparencyResponse"] = response_value;
    config["mobileRelayE2ee"]["keyTransparencyAuthorization"] = json!({
        "provenance": authorized.provenance().stable_code(),
        "productionAuthority": authorized.provenance().production_service_claim_allowed(),
        "selfMonitorDigest": authorized.authorization_digest(),
        "mlsKeyPackageDigest": mls_key_package_authorized
            .as_ref()
            .map(AuthorizedDirectoryLeaf::authorization_digest),
        "purpose": purpose.stable_code(),
        "treeSize": authorized.signed_tree_head().tree_size,
        "issuedAtEpochSeconds": authorized.signed_tree_head().issued_at_epoch_seconds,
        "observedAtEpochSeconds": authorized.freshness().observed_at_epoch_seconds
    });
    if authorized.claim().revoked() {
        config["mobileRelayE2ee"]["localDirectoryState"] = json!("revoked");
        clear_mobile_relay_pairing_state(&mut config)?;
    } else {
        config["mobileRelayE2ee"]["localDirectoryState"] = json!("active");
    }
    save_config_with_runtime_secret_context(&mut config, &mut secret_context)?;
    Ok(json!({
        "ok": true,
        "schemaVersion": CONFIG_SCHEMA_VERSION,
        "purpose": purpose.stable_code(),
        "directoryState": if authorized.claim().revoked() { "revoked" } else { "active" },
        "treeSize": authorized.signed_tree_head().tree_size,
        "issuedAtEpochSeconds": authorized.signed_tree_head().issued_at_epoch_seconds,
        "observedAtEpochSeconds": authorized.freshness().observed_at_epoch_seconds,
        "authorizationDigest": authorized.authorization_digest(),
        "mlsKeyPackageAuthorizationDigest": mls_key_package_authorized
            .as_ref()
            .map(AuthorizedDirectoryLeaf::authorization_digest)
    }))
}
