use super::approval_store::register_park_and_inbox;
use super::capabilities::APPROVAL_POLL_INTERVAL;
use super::continuity::handle_control_requests;
use super::errors::ProtocolFailure;
use super::io::{write_cancel_notification, write_message};
use super::protocol::SessionProtocol;
use super::supervision::PersistentTransport;
use serde_json::{Value, json};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Instant;
use uuid::Uuid;

#[derive(Debug)]
pub enum ApprovalWaitOutcome {
    Allowed,
    Denied,
}

pub fn await_external_approval(
    transport: &mut PersistentTransport,
    protocol: &mut SessionProtocol,
    request_id: &Value,
    display_summary: &str,
    option_id: Option<&str>,
    requested_tools: &[String],
    _deadline: Option<Instant>,
) -> Result<ApprovalWaitOutcome, ProtocolFailure> {
    let (decision_tx, decision_rx) = mpsc::sync_channel(1);
    let token = Uuid::new_v4().to_string();
    let session_id = protocol.session_id.clone().ok_or_else(|| {
        ProtocolFailure::user_interaction(
            "session/request_permission",
            None,
            Some(&protocol.config.turn_id),
        )
    })?;
    if let Err(failure) = register_park_and_inbox(
        &token,
        &session_id,
        &protocol.config.turn_id,
        "hermes",
        request_id,
        display_summary,
        option_id,
        requested_tools,
        decision_tx,
    ) {
        // Fail closed: cancel the permission and surface interaction required.
        let _ = write_message(
            &mut transport.stdin,
            &json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {"outcome": {"outcome": "cancelled"}}
            }),
        );
        if let Some(session_id) = protocol.session_id.as_deref() {
            let _ = write_cancel_notification(&mut transport.stdin, session_id);
        }
        protocol.interaction_failure = Some(failure.clone());
        return Err(failure);
    }
    loop {
        if let Some(failure) = handle_control_requests(transport, protocol) {
            licoup_foundation::platform::native_agent_interaction::abandon(&token);
            let _ = write_message(
                &mut transport.stdin,
                &json!({
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "result": {"outcome": {"outcome": "cancelled"}}
                }),
            );
            return Err(failure);
        }
        match decision_rx.recv_timeout(APPROVAL_POLL_INTERVAL) {
            Ok(true) => {
                let outcome = if let Some(option_id) = option_id {
                    json!({
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "result": {
                            "outcome": {
                                "outcome": "selected",
                                "optionId": option_id
                            }
                        }
                    })
                } else {
                    json!({
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "result": {
                            "outcome": {
                                "outcome": "selected",
                                "optionId": "allow"
                            }
                        }
                    })
                };
                if write_message(&mut transport.stdin, &outcome).is_err() {
                    return Err(protocol.failure_with_ids(
                        "hermes_acp_write_failed",
                        "Hermes ACP stopped accepting protocol messages.",
                        "protocol/write",
                    ));
                }
                return Ok(ApprovalWaitOutcome::Allowed);
            }
            Ok(false) => {
                if write_message(
                    &mut transport.stdin,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "result": {"outcome": {"outcome": "cancelled"}}
                    }),
                )
                .is_err()
                {
                    return Err(protocol.failure_with_ids(
                        "hermes_acp_write_failed",
                        "Hermes ACP stopped accepting protocol messages.",
                        "protocol/write",
                    ));
                }
                if let Some(session_id) = protocol.session_id.as_deref() {
                    let _ = write_cancel_notification(&mut transport.stdin, session_id);
                }
                protocol.interaction_failure = Some(ProtocolFailure::user_interaction(
                    "session/request_permission",
                    protocol.session_id.as_deref(),
                    Some(&protocol.config.turn_id),
                ));
                return Ok(ApprovalWaitOutcome::Denied);
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                let _ = write_message(
                    &mut transport.stdin,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "result": {"outcome": {"outcome": "cancelled"}}
                    }),
                );
                return Err(ProtocolFailure::new(
                    "native_interaction_transport_closed",
                    "The native interaction route closed before a response was delivered.",
                    "server/request",
                ));
            }
        }
    }
}
