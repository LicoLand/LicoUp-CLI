use crate::core::secure_mesh_directory::{
    DirectoryAuthorizationPurpose, SecureMeshDirectoryLeafClaim, SecureMeshKtVerifierConfiguration,
    UntrustedDirectoryResponse,
};
use crate::core::secure_mesh_transparency::PinnedKtLogKey;
use crate::domain::mobile_relay::secret_custody::ensure_no_kt_authority_reset_in_progress;
use anyhow::{Result, anyhow, ensure};
use serde_json::Value;

pub(in crate::domain::mobile_relay) fn configured_kt_verifier(
    config: &Value,
) -> Result<SecureMeshKtVerifierConfiguration> {
    ensure_no_kt_authority_reset_in_progress()?;
    let settings = config
        .get("secureMeshKeyTransparency")
        .filter(|value| value.is_object())
        .cloned()
        .ok_or_else(|| anyhow!("secure mesh KT authority must be configured before publication"))?;
    let configuration: SecureMeshKtVerifierConfiguration = serde_json::from_value(settings)
        .map_err(|_| anyhow!("secure mesh KT local verifier configuration is invalid"))?;
    configuration.validate()?;
    Ok(configuration)
}

pub(in crate::domain::mobile_relay) fn configured_kt_pin(config: &Value) -> Result<PinnedKtLogKey> {
    configured_kt_verifier(config)?.pin.into_pin()
}

pub(in crate::domain::mobile_relay) fn configured_directory_scope_commitment(
    config: &Value,
) -> Result<&str> {
    let scope = config
        .get("secureMeshDirectoryScopeCommitment")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            anyhow!("secure mesh opaque directory scope commitment is not configured")
        })?;
    validate_canonical_sha256_hex(scope, "directory scope commitment")?;
    Ok(scope)
}

pub(in crate::domain::mobile_relay) fn derive_local_publication_purpose(
    config: &Value,
    pending: &SecureMeshDirectoryLeafClaim,
) -> Result<DirectoryAuthorizationPurpose> {
    let Some(response_value) = config
        .get("mobileRelayE2ee")
        .and_then(|state| state.get("keyTransparencyResponse"))
        .filter(|value| value.is_object())
    else {
        return Ok(DirectoryAuthorizationPurpose::SelfMonitor);
    };
    let current: UntrustedDirectoryResponse = serde_json::from_value(response_value.clone())
        .map_err(|_| anyhow!("secure mesh current KT directory response is invalid"))?;
    ensure!(
        !current.claim.revoked(),
        "secure mesh revoked directory identity cannot be republished as active"
    );
    ensure!(
        current.claim.endpoint.endpoint_id == pending.endpoint.endpoint_id
            && current.claim.endpoint.directory_scope_commitment
                == pending.endpoint.directory_scope_commitment,
        "secure mesh pending directory identity scope differs from current authority"
    );
    let identity_changed = current.claim.endpoint.identity_public_key
        != pending.endpoint.identity_public_key
        || current.claim.endpoint.signing_public_key != pending.endpoint.signing_public_key
        || current.claim.endpoint.fingerprint != pending.endpoint.fingerprint
        || current.claim.endpoint.rotation_epoch != pending.endpoint.rotation_epoch;
    if identity_changed {
        return Ok(DirectoryAuthorizationPurpose::IdentityKeyChange);
    }
    if current.claim.key_material.mls_key_package_digest
        != pending.key_material.mls_key_package_digest
        || current.claim.key_material.mls_key_package_version
            != pending.key_material.mls_key_package_version
    {
        return Ok(DirectoryAuthorizationPurpose::MlsKeyPackage);
    }
    Ok(DirectoryAuthorizationPurpose::SelfMonitor)
}

pub(in crate::domain::mobile_relay) fn parse_local_directory_authorization_purpose(
    value: &str,
) -> Result<DirectoryAuthorizationPurpose> {
    match value.trim() {
        "self-monitor" => Ok(DirectoryAuthorizationPurpose::SelfMonitor),
        "identity-key-change" => Ok(DirectoryAuthorizationPurpose::IdentityKeyChange),
        "revocation" => Ok(DirectoryAuthorizationPurpose::Revocation),
        "mls-key-package" => Ok(DirectoryAuthorizationPurpose::MlsKeyPackage),
        "pairwise-signed-prekey" => Ok(DirectoryAuthorizationPurpose::PairwiseSignedPrekey),
        "pairwise-one-time-prekey" => Ok(DirectoryAuthorizationPurpose::PairwiseOneTimePrekey),
        _ => Err(anyhow!(
            "secure mesh local directory authorization purpose is unsupported"
        )),
    }
}

pub(in crate::domain::mobile_relay) fn validate_canonical_sha256_hex(
    value: &str,
    label: &str,
) -> Result<()> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "secure mesh {label} must be canonical lowercase SHA-256 hex"
    );
    Ok(())
}
