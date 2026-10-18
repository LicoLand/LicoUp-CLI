use anyhow::{Context, Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use ed25519_dalek::{Signature, Signer, SigningKey};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

use crate::core::secure_mesh_trust::{DeviceTrustPublicIdentity, DeviceTrustState};

const MAX_SIGNATURE_B64_LEN: usize = 256;
const MAX_PREKEY_CLOCK_SKEW_SECONDS: i64 = 300;

pub(super) fn sign_payload(signer_key: &SigningKey, payload: &[u8]) -> String {
    let signature = signer_key.sign(payload);
    general_purpose::URL_SAFE_NO_PAD.encode(signature.to_bytes())
}

pub(super) fn ensure_active_trust_state(
    trust_state: DeviceTrustState,
    require_verified_device: bool,
) -> Result<()> {
    match trust_state {
        DeviceTrustState::Verified => Ok(()),
        DeviceTrustState::CrossSigned => bail_prekey(
            "secure mesh cross-signed endpoint requires durable epoch and revocation validation",
        ),
        DeviceTrustState::Unverified if !require_verified_device => Ok(()),
        DeviceTrustState::Unverified => {
            bail_prekey("secure mesh endpoint is not verified for prekey use")
        }
        DeviceTrustState::KeyChanged => {
            bail_prekey("secure mesh endpoint identity changed; prekey use is paused")
        }
        DeviceTrustState::Revoked => bail_prekey("secure mesh endpoint is revoked"),
    }
}

pub(super) fn ensure_signature_shape(signature: &str, label: &str) -> Result<()> {
    ensure!(
        !signature.trim().is_empty(),
        "secure mesh {label} signature is required"
    );
    ensure!(
        signature.len() <= MAX_SIGNATURE_B64_LEN,
        "secure mesh {label} signature is too large"
    );
    Ok(())
}

pub(super) fn ensure_not_expired(
    created_at: &str,
    expires_at: &str,
    now: OffsetDateTime,
    label: &str,
) -> Result<()> {
    let created = parse_rfc3339(created_at, label)?;
    let expires = parse_rfc3339(expires_at, label)?;
    ensure!(
        expires > created,
        "secure mesh {label} expiresAt must be after createdAt"
    );
    ensure!(
        created <= now + Duration::seconds(MAX_PREKEY_CLOCK_SKEW_SECONDS),
        "secure mesh {label} createdAt is too far in the future"
    );
    ensure!(expires > now, "secure mesh {label} is expired");
    Ok(())
}

pub(super) fn verify_signature(
    endpoint_identity: &DeviceTrustPublicIdentity,
    payload: &[u8],
    signature: &str,
    label: &str,
) -> Result<()> {
    let signature_bytes = general_purpose::URL_SAFE_NO_PAD
        .decode(signature)
        .with_context(|| format!("secure mesh {label} signature is not base64url"))?;
    let signature = Signature::from_slice(&signature_bytes)
        .map_err(|error| anyhow!("secure mesh {label} signature is invalid: {error:?}"))?;
    endpoint_identity
        .signing_verifying_key()?
        .verify_strict(payload, &signature)
        .map_err(|_| anyhow!("secure mesh {label} signature verification failed"))?;
    Ok(())
}

pub(super) fn parse_rfc3339(value: &str, label: &str) -> Result<OffsetDateTime> {
    OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|error| anyhow!("secure mesh {label} timestamp is not RFC3339: {error}"))
}

pub(super) fn append_len_prefixed_bytes(out: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    let len =
        u32::try_from(value.len()).map_err(|_| anyhow!("secure mesh prekey field is too large"))?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value);
    Ok(())
}

pub(super) fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut encoded, "{byte:02x}").expect("writing to a string must succeed");
    }
    encoded
}

pub(super) fn bail_prekey<T>(message: &str) -> Result<T> {
    Err(anyhow!(message.to_string()))
}
