//! External approval for Claude Code permission requests.
//!
//! When Claude Code emits a `control_request` with subtype
//! `permission_request`, the driver must not fail the turn: it parks the
//! request in the shared approval registry, emits `agent.approval.needed`,
//! and suspends the turn until the client resolves the park (allow/deny).
//! The decision is written back as a `permission_response` control response
//! so the CLI continues or aborts the turn itself.

use super::io::write_message;
use super::transport::PersistentTransport;
use crate::protocol::parser::permission_response;
use crate::protocol::{PermissionRequest, ProtocolFailure};
use serde_json::json;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use uuid::Uuid;

pub(super) struct PendingApproval {
    token: String,
    request_id: String,
    tool_use_id: Option<String>,
    decision_rx: Receiver<bool>,
}

impl PendingApproval {
    /// Poll one unbounded approval route without blocking native transport
    /// supervision. `None` means the user has not decided yet.
    pub(super) fn try_resolve(
        &self,
        transport: &mut PersistentTransport,
    ) -> Result<Option<bool>, ProtocolFailure> {
        let allow = match self.decision_rx.try_recv() {
            Ok(allow) => allow,
            Err(TryRecvError::Empty) => return Ok(None),
            Err(TryRecvError::Disconnected) => {
                return Err(ProtocolFailure::new(
                    "claude_code_approval_park_disconnected",
                    "Claude Code approval park channel disconnected.",
                    "server/request",
                ));
            }
        };
        write_message(
            &mut transport.stdin,
            &permission_response(&self.request_id, self.tool_use_id.as_deref(), allow),
        )
        .map_err(|_| {
            ProtocolFailure::new(
                "claude_code_write_failed",
                "Claude Code stopped accepting the approval response.",
                "protocol/write",
            )
        })?;
        Ok(Some(allow))
    }
}

impl Drop for PendingApproval {
    fn drop(&mut self) {
        licoup_foundation::platform::native_agent_interaction::abandon(&self.token);
    }
}

/// Park the turn until the client resolves the approval. The caller continues
/// polling native transport/control events, so the wait has no elapsed-time
/// expiry while process exit and cancellation remain observable.
pub(super) fn park_external_approval(
    session_id: &str,
    turn_id: &str,
    request: &PermissionRequest,
) -> Result<PendingApproval, ProtocolFailure> {
    let (decision_tx, decision_rx) = mpsc::sync_channel(1);
    let token = Uuid::new_v4().to_string();
    let tools = request.tool_name.clone().into_iter().collect::<Vec<_>>();
    if let Err(failure) = licoup_agent_drivers::acp_session_transport::register_park_and_inbox(
        &token,
        session_id,
        turn_id,
        "claude-code",
        &json!({ "request_id": request.request_id }),
        &request.summary,
        None,
        &tools,
        decision_tx,
    ) {
        let mut converted = ProtocolFailure::new(failure.code, failure.message, failure.stage);
        if let Some(turn_id) = failure.turn_id.as_deref() {
            converted = converted.with_turn(turn_id);
        }
        return Err(converted);
    }
    Ok(PendingApproval {
        token,
        request_id: request.request_id.clone(),
        tool_use_id: request.tool_use_id.clone(),
        decision_rx,
    })
}
