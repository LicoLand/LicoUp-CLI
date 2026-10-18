use super::{AcpError, DEFAULT_MAX_MESSAGE_BYTES};
use serde_json::{Map, Value};

pub(super) const MAX_SESSION_ID_BYTES: usize = 1024;
/// Third-party display strings (command names, descriptions, hints) are far
/// larger than identifiers: real agents advertise skill catalogues whose
/// entries carry multi-kilobyte descriptions. Bounded, but not identifier-tight.
pub(super) const MAX_DISPLAY_TEXT_BYTES: usize = 8192;
pub(super) const MAX_CURSOR_BYTES: usize = 4096;

pub(super) fn validated_session_id(session_id: &str) -> Result<&str, AcpError> {
    normalized_text(session_id, MAX_SESSION_ID_BYTES, AcpError::SessionIdInvalid)?;
    Ok(session_id)
}

pub(super) fn normalized_text(
    value: &str,
    max_bytes: usize,
    error: AcpError,
) -> Result<(), AcpError> {
    if value.is_empty() || value.len() > max_bytes || value.trim() != value {
        return Err(error);
    }
    Ok(())
}

pub(super) fn ensure_message_limit(message: &Value) -> Result<(), AcpError> {
    let bytes = serde_json::to_vec(message).map_err(|_| AcpError::ResponseEnvelopeInvalid)?;
    if bytes.len() > DEFAULT_MAX_MESSAGE_BYTES {
        return Err(AcpError::MessageTooLarge);
    }
    Ok(())
}

pub(super) fn validate_optional_meta(
    value: Option<&Value>,
    error: AcpError,
) -> Result<(), AcpError> {
    match value {
        None | Some(Value::Null) | Some(Value::Object(_)) => Ok(()),
        Some(_) => Err(error),
    }
}

pub(super) fn optional_object<'a>(
    value: Option<&'a Value>,
    error: AcpError,
) -> Result<Option<&'a Map<String, Value>>, AcpError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(value)) => Ok(Some(value)),
        Some(_) => Err(error),
    }
}
