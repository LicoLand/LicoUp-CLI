use anyhow::{Result, anyhow, ensure};
use serde_json::Value;

pub(super) fn json_optional_text(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

pub(super) fn json_bool(value: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter().find_map(|key| {
        let value = value.get(*key)?;
        if let Some(flag) = value.as_bool() {
            return Some(flag);
        }
        match value.as_str()?.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Some(true),
            "0" | "false" | "no" | "off" => Some(false),
            _ => None,
        }
    })
}

pub(super) fn json_text(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Result<String> {
    let value = keys
        .iter()
        .find_map(|key| object.get(*key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    ensure!(
        !value.is_empty(),
        "secure mesh file manifest text field is required"
    );
    Ok(value)
}

pub(super) fn json_u64(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Result<u64> {
    let value = keys
        .iter()
        .find_map(|key| object.get(*key))
        .ok_or_else(|| anyhow!("secure mesh file manifest integer field is required"))?;
    if let Some(number) = value.as_u64() {
        return Ok(number);
    }
    value
        .as_str()
        .unwrap_or_default()
        .trim()
        .parse::<u64>()
        .map_err(|_| anyhow!("secure mesh file manifest integer field is invalid"))
}

pub(super) fn json_u32(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Result<u32> {
    let value = json_u64(object, keys)?;
    u32::try_from(value)
        .map_err(|_| anyhow!("secure mesh file manifest integer field is too large"))
}
