use anyhow::{Result, anyhow, ensure};
use serde_json::Value;

pub const SECURE_MESH_LIFECYCLE_STATUS: &str = "ttl_delete_screenshot_resend_ack_purge_typing_read_receipt_service_actions_redacted_policy_available_pairwise_mls_envelope_required";
pub const SECURE_MESH_LIFECYCLE_CONTENT_TYPE: &str =
    "application/vnd.licomesh.secure-mesh.lifecycle-service-action+json";

pub(super) const MAX_TTL_SECONDS: u64 = 31 * 24 * 60 * 60;
pub(super) const MAX_MISSING_MESSAGE_IDS: usize = 64;
const MAX_TEXT_BYTES: usize = 255;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LifecycleServiceActionKind {
    MessageTtlSet,
    MessageDelete,
    ScreenshotDetected,
    ResendRequest,
    TypingState,
    ReadReceipt,
    AckPurge,
}

impl LifecycleServiceActionKind {
    pub(super) fn parse(params: &Value) -> Result<Self> {
        match read_required_text(
            params,
            &["actionKind", "kind", "serviceAction", "service_action"],
        )?
        .as_str()
        {
            "message_ttl_set" => Ok(Self::MessageTtlSet),
            "message_delete" => Ok(Self::MessageDelete),
            "screenshot_detected" => Ok(Self::ScreenshotDetected),
            "resend_request" => Ok(Self::ResendRequest),
            "typing_state" => Ok(Self::TypingState),
            "read_receipt" => Ok(Self::ReadReceipt),
            "ack_purge" => Ok(Self::AckPurge),
            _ => Err(anyhow!(
                "secure mesh lifecycle service action is unsupported"
            )),
        }
    }

    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::MessageTtlSet => "message_ttl_set",
            Self::MessageDelete => "message_delete",
            Self::ScreenshotDetected => "screenshot_detected",
            Self::ResendRequest => "resend_request",
            Self::TypingState => "typing_state",
            Self::ReadReceipt => "read_receipt",
            Self::AckPurge => "ack_purge",
        }
    }
}

pub(super) fn read_required_text(params: &Value, keys: &[&str]) -> Result<String> {
    let value = keys
        .iter()
        .find_map(|key| params.get(*key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    validate_text(&value, "service action text field")?;
    Ok(value)
}

pub(super) fn read_u64(params: &Value, keys: &[&str]) -> Result<u64> {
    let value = keys
        .iter()
        .find_map(|key| params.get(*key))
        .ok_or_else(|| anyhow!("secure mesh lifecycle integer field is required"))?;
    if let Some(number) = value.as_u64() {
        return Ok(number);
    }
    value
        .as_str()
        .unwrap_or_default()
        .trim()
        .parse::<u64>()
        .map_err(|_| anyhow!("secure mesh lifecycle integer field is invalid"))
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

pub(super) fn validate_text(value: &str, label: &str) -> Result<()> {
    ensure!(!value.is_empty(), "secure mesh {label} is required");
    ensure!(
        value.len() <= MAX_TEXT_BYTES,
        "secure mesh {label} is too large"
    );
    Ok(())
}
