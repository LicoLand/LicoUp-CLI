use crate::core::mcp::{
    DEFAULT_MAX_MESSAGE_BYTES, McpMessage, McpTransferDirection,
    OUTBOUND_TRANSFER_PROTOCOL_REVISION, decode_http_body, encode_http_body,
};
use anyhow::{Result, anyhow, ensure};
use serde_json::Value;
use sha2::{Digest, Sha256};

const MAX_SCOPE_TEXT_BYTES: usize = 2 * 1024;
const MAX_SESSION_ID_BYTES: usize = 1024;

pub(super) struct ApprovedTransferScope {
    pub(super) direction: McpTransferDirection,
    pub(super) destination: String,
    pub(super) purpose: String,
    pub(super) message: McpMessage,
    pub(super) body: Vec<u8>,
    pub(super) session_id: Option<String>,
    pub(super) approval_digest: String,
}

pub(super) fn parse_scope(params: &Value) -> Result<ApprovedTransferScope> {
    let direction = match exact_text(params, "direction", MAX_SCOPE_TEXT_BYTES)?.as_str() {
        "request" => McpTransferDirection::Request,
        "response" => McpTransferDirection::Response,
        _ => return Err(anyhow!("mcp_transfer_direction_invalid")),
    };
    let destination = exact_text(params, "destination", MAX_SCOPE_TEXT_BYTES)?;
    let purpose = exact_text(params, "purpose", MAX_SCOPE_TEXT_BYTES)?;
    let protocol_revision = exact_text(params, "protocolVersion", 64)?;
    ensure!(
        protocol_revision == OUTBOUND_TRANSFER_PROTOCOL_REVISION,
        "mcp_protocol_version_unsupported"
    );
    let message_json = exact_text(params, "messageJson", DEFAULT_MAX_MESSAGE_BYTES)?;
    let message = decode_http_body(message_json.as_bytes(), DEFAULT_MAX_MESSAGE_BYTES)?;
    validate_direction(direction, &message)?;
    let body = encode_http_body(&message, DEFAULT_MAX_MESSAGE_BYTES)?;
    let session_id = optional_session_id(params)?;
    let approval_digest = scope_digest(
        direction,
        &destination,
        &purpose,
        &protocol_revision,
        session_id.as_deref(),
        &body,
    );
    Ok(ApprovedTransferScope {
        direction,
        destination,
        purpose,
        message,
        body,
        session_id,
        approval_digest,
    })
}

pub(super) fn require_direct_confirmation(
    params: &Value,
    scope: &ApprovedTransferScope,
) -> Result<()> {
    require_direct_origin(params)?;
    ensure!(
        params.get("confirmed").and_then(bool_param) == Some(true),
        "mcp_transfer_confirmation_required"
    );
    let supplied = exact_text(params, "approvalDigest", 64)?;
    ensure!(
        supplied.len() == 64
            && supplied.bytes().all(|byte| byte.is_ascii_hexdigit())
            && supplied.eq_ignore_ascii_case(&scope.approval_digest),
        "mcp_transfer_approval_scope_mismatch"
    );
    Ok(())
}

pub(super) fn require_direct_origin(params: &Value) -> Result<()> {
    ensure!(
        params.get("requestOrigin").and_then(Value::as_str) == Some("direct-user"),
        "mcp_transfer_direct_user_required"
    );
    Ok(())
}

fn optional_session_id(params: &Value) -> Result<Option<String>> {
    let Some(value) = params.get("sessionId") else {
        return Ok(None);
    };
    let Some(value) = value.as_str() else {
        return Err(anyhow!("mcp_session_id_invalid"));
    };
    if value.is_empty() {
        return Ok(None);
    }
    ensure!(
        value.len() <= MAX_SESSION_ID_BYTES
            && value.bytes().all(|byte| (0x21..=0x7e).contains(&byte)),
        "mcp_session_id_invalid"
    );
    Ok(Some(value.to_owned()))
}

fn exact_text(params: &Value, key: &str, max_bytes: usize) -> Result<String> {
    let value = params
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("mcp_transfer_parameter_invalid"))?;
    ensure!(
        !value.is_empty() && value == value.trim() && value.len() <= max_bytes,
        "mcp_transfer_parameter_invalid"
    );
    Ok(value.to_owned())
}

fn bool_param(value: &Value) -> Option<bool> {
    value.as_bool().or_else(|| match value.as_str() {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    })
}

fn validate_direction(direction: McpTransferDirection, message: &McpMessage) -> Result<()> {
    match (direction, message) {
        (
            McpTransferDirection::Request,
            McpMessage::Request { .. } | McpMessage::Notification { .. },
        )
        | (McpTransferDirection::Response, McpMessage::Response { .. }) => Ok(()),
        _ => Err(anyhow!("mcp_transfer_message_direction_mismatch")),
    }
}

fn scope_digest(
    direction: McpTransferDirection,
    destination: &str,
    purpose: &str,
    protocol_revision: &str,
    session_id: Option<&str>,
    body: &[u8],
) -> String {
    let mut digest = Sha256::new();
    digest.update(match direction {
        McpTransferDirection::Request => b"request".as_slice(),
        McpTransferDirection::Response => b"response".as_slice(),
    });
    for value in [
        destination.as_bytes(),
        purpose.as_bytes(),
        protocol_revision.as_bytes(),
        session_id.unwrap_or_default().as_bytes(),
        body,
    ] {
        digest.update(value.len().to_be_bytes());
        digest.update(value);
    }
    format!("{:x}", digest.finalize())
}
