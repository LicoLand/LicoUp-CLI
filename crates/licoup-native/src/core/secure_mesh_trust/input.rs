use super::PUBLIC_KEY_LEN;
use super::identity::DeviceTrustPublicIdentity;
use super::model::DeviceTrustState;
use anyhow::{Result, anyhow, ensure};
use base64::{Engine as _, engine::general_purpose};
use serde_json::Value;

pub(super) fn device_identity_from_json(value: &Value) -> Result<DeviceTrustPublicIdentity> {
    let object = value
        .as_object()
        .ok_or_else(|| anyhow!("secure mesh device trust identity must be an object"))?;
    DeviceTrustPublicIdentity::new(
        read_text_field(object, &["endpointId", "endpoint_id"])?,
        read_public_key_field(object, &["identityPublicKey", "identity_public_key"])?,
        read_public_key_field(object, &["signingPublicKey", "signing_public_key"])?,
        read_u64_field(object, &["rotationEpoch", "rotation_epoch"], 0)?,
    )
}

pub(super) fn device_identity_param(
    params: &Value,
    keys: &[&str],
) -> Result<DeviceTrustPublicIdentity> {
    let value = keys
        .iter()
        .find_map(|key| params.get(*key))
        .ok_or_else(|| anyhow!("secure mesh device trust identity is required"))?;
    device_identity_from_json(value)
}

pub(super) fn provided_sas_text(params: &Value) -> Option<String> {
    let value = params.get("sas").or_else(|| params.get("sasCode"))?;
    if let Some(text) = value.as_str() {
        return Some(text.trim().replace(' ', "-"));
    }
    Some(
        value
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|chunk| !chunk.is_empty())
            .collect::<Vec<_>>()
            .join("-"),
    )
}

pub(super) fn read_text_field(
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
) -> Result<String> {
    let value = keys
        .iter()
        .find_map(|key| object.get(*key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    ensure!(
        !value.is_empty(),
        "secure mesh device trust text field is required"
    );
    Ok(value)
}

pub(super) fn read_u64_field(
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
    default_value: u64,
) -> Result<u64> {
    let Some(value) = keys.iter().find_map(|key| object.get(*key)) else {
        return Ok(default_value);
    };
    if let Some(number) = value.as_u64() {
        return Ok(number);
    }
    value
        .as_str()
        .unwrap_or_default()
        .trim()
        .parse::<u64>()
        .map_err(|_| anyhow!("secure mesh device trust integer field is invalid"))
}

pub(super) fn read_public_key_field(
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
) -> Result<[u8; PUBLIC_KEY_LEN]> {
    let raw = keys
        .iter()
        .find_map(|key| object.get(*key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    ensure!(
        !raw.is_empty(),
        "secure mesh device trust public key is required"
    );
    let bytes = decode_public_key(raw)?;
    bytes
        .try_into()
        .map_err(|_| anyhow!("secure mesh device trust public key must be 32 bytes"))
}

pub(super) fn decode_public_key(raw: &str) -> Result<Vec<u8>> {
    general_purpose::URL_SAFE_NO_PAD
        .decode(raw)
        .or_else(|_| general_purpose::STANDARD.decode(raw))
        .or_else(|_| decode_hex(raw))
        .map_err(|_| anyhow!("secure mesh device trust public key is not base64url or hex"))
}

pub(super) fn decode_hex(raw: &str) -> Result<Vec<u8>, ()> {
    let normalized = raw
        .chars()
        .filter(|ch| !matches!(ch, ':' | '-' | ' '))
        .collect::<String>();
    if normalized.len() % 2 != 0 {
        return Err(());
    }
    let mut out = Vec::with_capacity(normalized.len() / 2);
    for index in (0..normalized.len()).step_by(2) {
        let byte = u8::from_str_radix(&normalized[index..index + 2], 16).map_err(|_| ())?;
        out.push(byte);
    }
    Ok(out)
}

pub(super) fn trust_state_from_json(params: &Value) -> Result<DeviceTrustState> {
    let value = params
        .get("trustState")
        .or_else(|| params.get("trust_state"))
        .or_else(|| {
            params
                .get("identity")
                .and_then(|identity| identity.get("trustState"))
        })
        .and_then(Value::as_str)
        .unwrap_or("unverified");
    trust_state_from_label(value)
}

pub(super) fn trust_state_from_label(value: &str) -> Result<DeviceTrustState> {
    match value.trim() {
        "unverified" => Ok(DeviceTrustState::Unverified),
        "verified" => Ok(DeviceTrustState::Verified),
        "cross_signed" | "crossSigned" => Ok(DeviceTrustState::CrossSigned),
        "key_changed" | "keyChanged" => Ok(DeviceTrustState::KeyChanged),
        "revoked" => Ok(DeviceTrustState::Revoked),
        _ => Err(anyhow!("secure mesh device trust state is unsupported")),
    }
}

pub(super) fn trust_state_label(value: &DeviceTrustState) -> &'static str {
    match value {
        DeviceTrustState::Unverified => "unverified",
        DeviceTrustState::Verified => "verified",
        DeviceTrustState::CrossSigned => "cross_signed",
        DeviceTrustState::KeyChanged => "key_changed",
        DeviceTrustState::Revoked => "revoked",
    }
}

pub(super) fn read_bool(params: &Value, key: &str, default_value: bool) -> bool {
    match params.get(key) {
        Some(Value::Bool(value)) => *value,
        Some(Value::String(value)) => match value.trim() {
            "true" | "1" | "yes" => true,
            "false" | "0" | "no" => false,
            _ => default_value,
        },
        _ => default_value,
    }
}
