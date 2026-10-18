use anyhow::{Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

use super::manifest::{ValidatedManifest, ValidatedServerRunner};

pub(super) const OFFICIAL_SERVER_RUNNER_IDENTITY: &str = "licomesh.official-local-server-runner.v1";

pub(super) fn parse_public_key(value: &str) -> Result<VerifyingKey> {
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| anyhow!("collaboration_plugin_runner_trust_key_invalid"))?;
    let array: [u8; 32] = bytes
        .try_into()
        .map_err(|_| anyhow!("collaboration_plugin_runner_trust_key_invalid"))?;
    VerifyingKey::from_bytes(&array)
        .map_err(|_| anyhow!("collaboration_plugin_runner_trust_key_invalid"))
}

pub(super) fn public_key_fingerprint(value: &str) -> Result<String> {
    let key = parse_public_key(value)?;
    Ok(format!("{:x}", Sha256::digest(key.as_bytes())))
}

pub(super) fn validate_key_id(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'),
        "collaboration_plugin_runner_trust_key_id_invalid"
    );
    Ok(())
}

pub(super) fn verify_runner_signature(
    manifest: &ValidatedManifest,
    runner: &ValidatedServerRunner,
    public_key_base64url: &str,
) -> Result<()> {
    let key = parse_public_key(public_key_base64url)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(&runner.signature_base64url)
        .map_err(|_| anyhow!("collaboration_plugin_server_runner_signature_invalid"))?;
    let signature = Signature::from_slice(&bytes)
        .map_err(|_| anyhow!("collaboration_plugin_server_runner_signature_invalid"))?;
    key.verify(&runner_signature_payload(manifest, runner)?, &signature)
        .map_err(|_| anyhow!("collaboration_plugin_server_runner_signature_untrusted"))
}

fn runner_signature_payload(
    manifest: &ValidatedManifest,
    runner: &ValidatedServerRunner,
) -> Result<Vec<u8>> {
    let relative_path = super::manifest::normalized_relative_protocol_path(&runner.relative_path)?;
    let fields = [
        OFFICIAL_SERVER_RUNNER_IDENTITY,
        manifest.plugin_id.as_str(),
        manifest.version.as_str(),
        runner.source_url.as_str(),
        runner.source_commit_oid.as_str(),
        runner.platform.as_str(),
        runner.architecture.as_str(),
        relative_path.as_str(),
        runner.digest_sha256.as_str(),
        runner.runner_contract_version.as_str(),
        runner.health_contract_version.as_str(),
        runner.capabilities_contract_version.as_str(),
        manifest.signed_package_inventory_digest_sha256.as_str(),
    ];
    let mut payload = b"LICOUP-SERVER-RUNNER-SIGNATURE-V1\0".to_vec();
    for field in fields {
        payload.extend_from_slice(&(field.len() as u64).to_be_bytes());
        payload.extend_from_slice(field.as_bytes());
    }
    Ok(payload)
}

#[cfg(test)]
pub(super) fn test_trust() -> (String, String, String) {
    use ed25519_dalek::SigningKey;
    let signing = SigningKey::from_bytes(&[37u8; 32]);
    let public = URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes());
    let fingerprint = public_key_fingerprint(&public).unwrap();
    ("licoup-test-runner-key".to_owned(), public, fingerprint)
}

#[cfg(test)]
pub(super) fn sign_runner_for_test(
    manifest: &ValidatedManifest,
    runner: &ValidatedServerRunner,
) -> String {
    use ed25519_dalek::{Signer, SigningKey};
    let signing = SigningKey::from_bytes(&[37u8; 32]);
    URL_SAFE_NO_PAD.encode(
        signing
            .sign(&runner_signature_payload(manifest, runner).unwrap())
            .to_bytes(),
    )
}
