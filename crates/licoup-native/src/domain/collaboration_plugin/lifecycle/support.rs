use anyhow::{Result, anyhow, ensure};
use serde_json::Value;
use uuid::Uuid;

pub(super) fn required_plan_id(params: &Value) -> Result<String> {
    let value = required_text(params, &["planId"], "collaboration_plugin_plan_id_required")?;
    let parsed =
        Uuid::parse_str(value).map_err(|_| anyhow!("collaboration_plugin_plan_id_invalid"))?;
    let normalized = parsed.to_string();
    ensure!(value == normalized, "collaboration_plugin_plan_id_invalid");
    Ok(normalized)
}

pub(super) fn required_digest(params: &Value, key: &str) -> Result<String> {
    let value = required_text(params, &[key], "collaboration_plugin_digest_required")?;
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')),
        "collaboration_plugin_digest_invalid"
    );
    Ok(value.to_owned())
}

pub(super) fn require_direct_confirmation(params: &Value, code: &'static str) -> Result<()> {
    let confirmed = params
        .get("confirmed")
        .or_else(|| params.get("directUserConfirmation"))
        .and_then(|value| match value {
            Value::Bool(value) => Some(*value),
            Value::String(value) => Some(value == "true"),
            _ => None,
        })
        .unwrap_or(false);
    ensure!(confirmed, code);
    Ok(())
}

pub(super) fn require_direct_request(params: &Value) -> Result<()> {
    ensure!(
        params.get("requestOrigin").and_then(Value::as_str) == Some("direct-user"),
        "collaboration_plugin_direct_user_origin_required"
    );
    ensure!(
        ["agentTriggered", "scheduled", "startupTriggered"]
            .iter()
            .all(|key| params.get(*key).and_then(Value::as_bool) != Some(true)),
        "collaboration_plugin_automatic_trigger_forbidden"
    );
    Ok(())
}

fn required_text<'a>(params: &'a Value, keys: &[&str], code: &'static str) -> Result<&'a str> {
    let value = keys
        .iter()
        .find_map(|key| params.get(*key).and_then(Value::as_str))
        .ok_or_else(|| anyhow!(code))?;
    ensure!(value == value.trim() && !value.is_empty(), code);
    Ok(value)
}
