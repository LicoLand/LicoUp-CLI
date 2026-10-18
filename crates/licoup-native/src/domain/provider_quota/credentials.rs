//! Bounded credential-artifact reads for provider quota sources.
//!
//! Every credential read is funneled through these helpers: read the specific
//! provider auth artifact into memory with a hard byte bound, build the
//! request, drop. Credential material is never persisted, logged, or included
//! in diagnostics; only quota metrics and identity labels leave a source.

use anyhow::{Result, anyhow, ensure};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::Value;
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Provider auth artifacts are small JSON documents; anything larger is
/// refused instead of being truncated into a misleading credential.
pub(super) const MAX_CREDENTIAL_BYTES: u64 = 64 * 1024;

pub(super) fn read_bounded_utf8(path: &Path) -> Result<String> {
    let file = File::open(path).map_err(|_| anyhow!("credential artifact is not readable"))?;
    let mut handle = file.take(MAX_CREDENTIAL_BYTES.saturating_add(1));
    let mut bytes = Vec::with_capacity(4096);
    handle
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow!("credential artifact is not readable"))?;
    ensure!(
        bytes.len() as u64 <= MAX_CREDENTIAL_BYTES,
        "credential artifact exceeds its bounded size"
    );
    String::from_utf8(bytes).map_err(|_| anyhow!("credential artifact is not valid UTF-8"))
}

/// Decode the payload claims of a JSON Web Token without trusting the
/// signature. The token is the user's own local credential; claims are used
/// only for expiry checks and identity labels, never for authorization.
pub(super) fn decode_jwt_payload(token: &str) -> Option<Value> {
    let mut segments = token.trim().split('.');
    let _header = segments.next()?;
    let payload = segments.next()?;
    if segments.next().is_none() {
        return None;
    }
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// JWT `exp` claim as epoch seconds; tokens without a numeric expiry are not
/// treated as expiring here.
pub(super) fn jwt_expiry_epoch_seconds(token: &str) -> Option<i64> {
    let payload = decode_jwt_payload(token)?;
    let exp = payload.get("exp")?;
    exp.as_i64()
        .or_else(|| exp.as_f64().map(|value| value as i64))
}

/// JWT string claim (for example `sub` or `email`) used to derive session
/// material and identity labels from the user's own local token.
pub(super) fn jwt_string_claim(token: &str, claim: &str) -> Option<String> {
    let payload = decode_jwt_payload(token)?;
    payload.get(claim)?.as_str().map(str::to_owned)
}
