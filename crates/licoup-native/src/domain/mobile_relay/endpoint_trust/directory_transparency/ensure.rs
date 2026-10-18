use super::claim::local_pairwise_prekey_bundle_from_config;
use super::clock::current_secure_mesh_kt_gate_epoch_seconds;
use super::config::validate_canonical_sha256_hex;
#[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
use super::test_support::provision_mobile_relay_test_key_transparency;
use super::verifier::{authorize_mls_directory_response, authorize_pairwise_directory_response};
use crate::core::secure_mesh_directory::{AuthorizedDirectoryLeaf, DirectoryAuthorizationPurpose};
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use time::OffsetDateTime;

pub(in crate::domain::mobile_relay) fn ensure_mobile_relay_key_transparency(
    config: &mut Value,
) -> Result<()> {
    #[cfg(any(test, feature = "secure-mesh-acceptance-mock-kt"))]
    provision_mobile_relay_test_key_transparency(config)?;

    let bundle = local_pairwise_prekey_bundle_from_config(config)?;
    let response = config
        .get("mobileRelayE2ee")
        .and_then(|state| state.get("keyTransparencyResponse"))
        .filter(|value| value.is_object())
        .cloned()
        .ok_or_else(|| {
            anyhow!("mobile relay endpoint has no externally provisioned key transparency response")
        })?;
    let now = OffsetDateTime::now_utc();
    let self_monitor = authorize_pairwise_directory_response(
        config,
        &bundle,
        response.clone(),
        now,
        DirectoryAuthorizationPurpose::SelfMonitor,
    )?;
    let signed_prekey = authorize_pairwise_directory_response(
        config,
        &bundle,
        response.clone(),
        now,
        DirectoryAuthorizationPurpose::PairwiseSignedPrekey,
    )?;
    let one_time_prekey = authorize_pairwise_directory_response(
        config,
        &bundle,
        response.clone(),
        now,
        DirectoryAuthorizationPurpose::PairwiseOneTimePrekey,
    )?;
    let mls_key_package = match (
        config
            .get("mobileRelayE2ee")
            .and_then(|state| state.get("mlsKeyPackageDigest"))
            .and_then(Value::as_str),
        config
            .get("mobileRelayE2ee")
            .and_then(|state| state.get("mlsKeyPackageVersion"))
            .and_then(Value::as_u64)
            .filter(|version| *version > 0),
    ) {
        (Some(digest), Some(version)) => {
            validate_canonical_sha256_hex(digest, "MLS KeyPackage digest")?;
            Some(authorize_mls_directory_response(
                config,
                &bundle,
                response,
                current_secure_mesh_kt_gate_epoch_seconds()?,
                digest,
                version,
            )?)
        }
        _ => None,
    };
    config["mobileRelayE2ee"]["keyTransparencyAuthorization"] = json!({
        "provenance": self_monitor.provenance().stable_code(),
        "productionAuthority": self_monitor.provenance().production_service_claim_allowed(),
        "selfMonitorDigest": self_monitor.authorization_digest(),
        "signedPrekeyDigest": signed_prekey.authorization_digest(),
        "oneTimePrekeyDigest": one_time_prekey.authorization_digest(),
        "mlsKeyPackageDigest": mls_key_package
            .as_ref()
            .map(AuthorizedDirectoryLeaf::authorization_digest)
    });
    Ok(())
}
