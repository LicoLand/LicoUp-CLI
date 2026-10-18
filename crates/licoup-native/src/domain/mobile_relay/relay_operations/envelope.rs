use crate::domain::mobile_relay::support::json_param;
#[cfg(test)]
use anyhow::anyhow;
use anyhow::{Context, Result};
use licoup_protocol_bindings::licoarc_relay::LicoArcRelayEnvelope;
use serde_json::Value;

pub(in crate::domain::mobile_relay) fn secure_envelope_param(params: &Value) -> Option<Value> {
    let envelope = json_param(params, "envelope")?;
    if validate_secure_envelope(&envelope).is_ok() {
        Some(envelope)
    } else {
        None
    }
}

pub(in crate::domain::mobile_relay) fn validate_secure_envelope(envelope: &Value) -> Result<()> {
    let wire = serde_json::to_string(envelope)
        .context("secure mesh relay envelope serialization failed")?;
    LicoArcRelayEnvelope::from_json(&wire)?;
    Ok(())
}

#[cfg(test)]
pub(in crate::domain::mobile_relay) fn validate_envelope_text_field(
    label: &str,
    value: &str,
    max_bytes: usize,
) -> Result<()> {
    if value.trim().is_empty() {
        return Err(anyhow!("secure envelope missing {label}"));
    }
    if value.len() > max_bytes {
        return Err(anyhow!("secure envelope {label} is too large"));
    }
    Ok(())
}

#[cfg(test)]
pub(in crate::domain::mobile_relay) fn encoded_len_limit(decoded_bytes: usize) -> usize {
    decoded_bytes.div_ceil(3) * 4
}

pub(in crate::domain::mobile_relay) fn relay_envelope_from_value(
    value: &Value,
) -> Result<LicoArcRelayEnvelope> {
    let wire = serde_json::to_string(value).context("Lico Arc envelope serialization failed")?;
    LicoArcRelayEnvelope::from_json(&wire)
}
