use super::{MAX_TOOL_NAME_BYTES, MAX_TOOL_NAMES};
use anyhow::{Result, anyhow, ensure};
use serde_json::Value;

pub(super) fn require_text(field: &str, value: &str, max_bytes: usize) -> Result<String> {
    let trimmed = value.trim();
    ensure!(
        !trimmed.is_empty(),
        "secure mesh approval {field} is required"
    );
    ensure!(
        trimmed.len() <= max_bytes,
        "secure mesh approval {field} exceeds the byte limit"
    );
    ensure!(
        !trimmed.contains('\0'),
        "secure mesh approval {field} contains a NUL byte"
    );
    Ok(trimmed.to_string())
}

pub(super) fn optional_text(value: Option<&str>, max_bytes: usize) -> Result<String> {
    match value {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Ok(String::new());
            }
            ensure!(
                trimmed.len() <= max_bytes,
                "secure mesh approval text exceeds the byte limit"
            );
            Ok(trimmed.to_string())
        }
        None => Ok(String::new()),
    }
}

pub(super) fn json_text(params: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(value) = params.get(*key).and_then(Value::as_str) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

pub(super) fn json_string_list(params: &Value, keys: &[&str]) -> Result<Vec<String>> {
    for key in keys {
        if let Some(Value::Array(items)) = params.get(*key) {
            ensure!(
                items.len() <= MAX_TOOL_NAMES,
                "secure mesh approval tool list exceeds the item limit"
            );
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                let text = item
                    .as_str()
                    .ok_or_else(|| anyhow!("secure mesh approval tool name must be a string"))?;
                out.push(require_text("toolName", text, MAX_TOOL_NAME_BYTES)?);
            }
            return Ok(out);
        }
    }
    Ok(Vec::new())
}

pub(super) fn parse_risk_level(value: &str) -> Result<String> {
    let normalized = value.trim();
    ensure!(
        matches!(
            normalized,
            "read_only" | "safe_write" | "local_effect" | "high_risk"
        ),
        "secure mesh approval risk level is unsupported"
    );
    Ok(normalized.to_string())
}

pub(super) fn parse_adapter_style(value: &str) -> Result<String> {
    let normalized = value.trim();
    ensure!(
        matches!(
            normalized,
            "callback" | "polling" | "cli" | "unavailable" | "runtime-owned"
        ),
        "secure mesh approval adapter style is unsupported"
    );
    Ok(normalized.to_string())
}

pub(super) fn looks_like_plaintext_secret(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("authorization:")
        || lower.contains("bearer ")
        || lower.contains("api_key=")
        || lower.contains("-----begin ")
}
