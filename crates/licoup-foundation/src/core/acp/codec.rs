use super::validation::ensure_message_limit;
use super::{AcpError, MAX_JSON_LINE_BYTES};
use serde_json::Value;

pub fn encode_json_line(message: &Value) -> Result<Vec<u8>, AcpError> {
    if !message.is_object() {
        return Err(AcpError::JsonLineInvalid);
    }
    ensure_message_limit(message)?;
    let mut bytes = serde_json::to_vec(message).map_err(|_| AcpError::JsonLineInvalid)?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub fn decode_json_line(line: &[u8]) -> Result<Value, AcpError> {
    if line.len() > MAX_JSON_LINE_BYTES {
        return Err(AcpError::MessageTooLarge);
    }
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    if line.is_empty() || line.contains(&b'\n') || line.contains(&b'\r') {
        return Err(AcpError::JsonLineInvalid);
    }
    let message = serde_json::from_slice::<Value>(line).map_err(|_| AcpError::JsonLineInvalid)?;
    if !message.is_object() {
        return Err(AcpError::JsonLineInvalid);
    }
    ensure_message_limit(&message)?;
    Ok(message)
}
