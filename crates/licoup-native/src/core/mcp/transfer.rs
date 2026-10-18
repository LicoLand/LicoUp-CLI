use super::{
    DEFAULT_MAX_MESSAGE_BYTES, DEFAULT_TRANSFER_APPROVAL_TTL, MAX_TRANSFER_APPROVAL_TTL,
    McpMessage, encode_http_body,
};
use anyhow::{Result, anyhow, ensure};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum McpTransferDirection {
    Request,
    Response,
}

#[derive(Clone)]
pub(super) struct TransferApproval {
    pub(super) direction: McpTransferDirection,
    pub(super) destination: String,
    pub(super) purpose: String,
    pub(super) body_sha256: [u8; 32],
    pub(super) expires_at: Instant,
}

/// One-shot gate for sending a request or relaying a response outside the
/// local MCP transport.
///
/// Callers record approval only after a direct user confirmation for this exact
/// action, direction, destination, purpose, and encoded message. Approval is
/// short-lived, and every transfer attempt consumes the matching action ID.
#[derive(Default)]
pub struct McpExternalTransferGate {
    pub(super) approvals: Mutex<HashMap<String, TransferApproval>>,
}

impl fmt::Debug for McpExternalTransferGate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpExternalTransferGate")
            .field("pending_approval_count", &self.pending_approval_count())
            .finish()
    }
}

pub struct McpTransferPacket {
    direction: McpTransferDirection,
    destination: String,
    purpose: String,
    body: Vec<u8>,
}

impl fmt::Debug for McpTransferPacket {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpTransferPacket")
            .field("direction", &self.direction)
            .field("destination", &"<redacted>")
            .field("purpose", &"<redacted>")
            .field("body_bytes", &self.body.len())
            .finish()
    }
}

impl McpTransferPacket {
    pub fn direction(&self) -> McpTransferDirection {
        self.direction
    }

    pub fn destination(&self) -> &str {
        &self.destination
    }

    pub fn purpose(&self) -> &str {
        &self.purpose
    }

    pub fn body(&self) -> &[u8] {
        &self.body
    }
}

impl McpExternalTransferGate {
    pub fn record_direct_user_approval(
        &self,
        action_id: impl Into<String>,
        direction: McpTransferDirection,
        destination: impl Into<String>,
        purpose: impl Into<String>,
        message: &McpMessage,
    ) -> Result<()> {
        self.record_direct_user_approval_with_ttl(
            action_id,
            direction,
            destination,
            purpose,
            message,
            DEFAULT_TRANSFER_APPROVAL_TTL,
        )
    }

    pub fn record_direct_user_approval_with_ttl(
        &self,
        action_id: impl Into<String>,
        direction: McpTransferDirection,
        destination: impl Into<String>,
        purpose: impl Into<String>,
        message: &McpMessage,
        ttl: Duration,
    ) -> Result<()> {
        ensure!(
            !ttl.is_zero() && ttl <= MAX_TRANSFER_APPROVAL_TTL,
            "mcp_transfer_approval_ttl_invalid"
        );
        let expires_at = Instant::now()
            .checked_add(ttl)
            .ok_or_else(|| anyhow!("mcp_transfer_approval_ttl_invalid"))?;
        self.record_direct_user_approval_until(
            action_id.into(),
            direction,
            destination.into(),
            purpose.into(),
            message,
            expires_at,
        )
    }

    pub(super) fn record_direct_user_approval_until(
        &self,
        action_id: String,
        direction: McpTransferDirection,
        destination: String,
        purpose: String,
        message: &McpMessage,
        expires_at: Instant,
    ) -> Result<()> {
        validate_transfer_message(direction, message)?;
        let action_id = non_empty_scope(action_id, "mcp_transfer_action_id_invalid")?;
        let destination = non_empty_scope(destination, "mcp_transfer_destination_invalid")?;
        let purpose = non_empty_scope(purpose, "mcp_transfer_purpose_invalid")?;
        ensure!(expires_at > Instant::now(), "mcp_transfer_approval_expired");
        let body = encode_bounded_transfer_body(message)?;
        let mut approvals = self
            .approvals
            .lock()
            .map_err(|_| anyhow!("mcp_transfer_gate_unavailable"))?;
        ensure!(
            !approvals.contains_key(&action_id),
            "mcp_transfer_approval_duplicate"
        );
        approvals.insert(
            action_id,
            TransferApproval {
                direction,
                destination,
                purpose,
                body_sha256: message_digest(&body),
                expires_at,
            },
        );
        Ok(())
    }

    pub fn cancel(&self, action_id: &str) -> Result<bool> {
        let mut approvals = self
            .approvals
            .lock()
            .map_err(|_| anyhow!("mcp_transfer_gate_unavailable"))?;
        Ok(approvals.remove(action_id).is_some())
    }

    pub fn send_request_once(
        &self,
        action_id: &str,
        destination: &str,
        purpose: &str,
        request: &McpMessage,
    ) -> Result<McpTransferPacket> {
        self.transfer_once(
            action_id,
            McpTransferDirection::Request,
            destination,
            purpose,
            request,
        )
    }

    pub fn forward_response_once(
        &self,
        action_id: &str,
        destination: &str,
        purpose: &str,
        response: &McpMessage,
    ) -> Result<McpTransferPacket> {
        self.transfer_once(
            action_id,
            McpTransferDirection::Response,
            destination,
            purpose,
            response,
        )
    }

    fn transfer_once(
        &self,
        action_id: &str,
        direction: McpTransferDirection,
        destination: &str,
        purpose: &str,
        message: &McpMessage,
    ) -> Result<McpTransferPacket> {
        validate_transfer_message(direction, message)?;
        let body = encode_bounded_transfer_body(message)?;
        let mut approvals = self
            .approvals
            .lock()
            .map_err(|_| anyhow!("mcp_transfer_gate_unavailable"))?;
        if approvals
            .get(action_id)
            .is_some_and(|approval| approval.expires_at <= Instant::now())
        {
            approvals.remove(action_id);
            return Err(anyhow!("mcp_transfer_approval_expired"));
        }
        let approval = approvals
            .remove(action_id)
            .ok_or_else(|| anyhow!("mcp_transfer_approval_required"))?;
        ensure!(
            approval.direction == direction
                && approval.destination == destination
                && approval.purpose == purpose
                && approval.body_sha256 == message_digest(&body),
            "mcp_transfer_approval_scope_mismatch"
        );
        Ok(McpTransferPacket {
            direction,
            destination: destination.to_owned(),
            purpose: purpose.to_owned(),
            body,
        })
    }

    pub fn pending_approval_count(&self) -> usize {
        self.approvals
            .lock()
            .map(|mut approvals| {
                let now = Instant::now();
                approvals.retain(|_, approval| approval.expires_at > now);
                approvals.len()
            })
            .unwrap_or(0)
    }
}

fn validate_transfer_message(direction: McpTransferDirection, message: &McpMessage) -> Result<()> {
    match (direction, message) {
        (
            McpTransferDirection::Request,
            McpMessage::Request { .. } | McpMessage::Notification { .. },
        )
        | (McpTransferDirection::Response, McpMessage::Response { .. }) => Ok(()),
        (McpTransferDirection::Request, _) => Err(anyhow!("mcp_transfer_request_required")),
        (McpTransferDirection::Response, _) => Err(anyhow!("mcp_transfer_response_required")),
    }
}

pub(super) fn encode_bounded_transfer_body(message: &McpMessage) -> Result<Vec<u8>> {
    encode_http_body(message, DEFAULT_MAX_MESSAGE_BYTES)
}

pub(super) fn message_digest(body: &[u8]) -> [u8; 32] {
    Sha256::digest(body).into()
}

fn non_empty_scope(value: String, code: &'static str) -> Result<String> {
    let normalized = value.trim();
    ensure!(!normalized.is_empty() && normalized == value, code);
    Ok(value)
}
