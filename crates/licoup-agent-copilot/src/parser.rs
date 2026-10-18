//! Copilot's ACP frame protocol, classified once below the adapter port.
//!
//! Copilot speaks the shared ACP profile: JSON-RPC 2.0 frames, one per LF
//! delimited line, over stdio. *Which* frames those are is Copilot's protocol
//! fact and lives here; the framing, the session semantics and the reducer that
//! drives them are shared and live in `licoup-agent-drivers`, which reads this
//! module's answers through its port.
//!
//! The parser owns the whole ingress: [`decode_frame`] is the only place a raw
//! byte line becomes a frame, and every reader below validates the frame
//! exactly once, per ADR-0008. Nothing above the port re-parses a frame and no
//! second decoder exists.
//!
//! What this parser reports is what the client's turn authority acts on: a
//! protocol finish, an end of stream and a confirmed cancellation. It settles
//! no turn, imposes no implicit timeout and hides no content.

use licoup_agent_adapter_sdk::adapters::AdapterContract;
use licoup_agent_adapter_sdk::{LifecycleStage, Transition, TransitionReducer};
use licoup_foundation::core::acp::{self, AcpSessionUpdate, AcpStopReason};
use serde_json::Value;

/// This Agent's adapter declaration, as composition and the corpus check read
/// it.
pub const CONTRACT: AdapterContract = AdapterContract::new("copilot", "lf-ndjson-acp");

/// Copilot ACP's complete parser-owned ingress. The stdio transport passes
/// each raw LF-delimited frame here exactly once and never decodes JSON itself.
pub fn decode_frame(line: &[u8]) -> Result<Value, acp::AcpError> {
    acp::decode_json_line(line)
}

/// Read the initialize response out of a raw byte line, when the line is it.
pub fn initialize_response(
    line: &[u8],
    request_id: i64,
) -> Result<Option<acp::AcpInitializeResponse>, acp::AcpError> {
    let frame = decode_frame(line)?;
    if !response_id_matches(&frame, request_id) {
        return Ok(None);
    }
    acp::validate_initialize_response(&frame, request_id).map(Some)
}

/// One client request Copilot asked this client to answer.
///
/// It is this Agent's shape rather than the shared transport's: the transport
/// reads four facts and the projection onto it lives in [`crate::dialect`],
/// so the parser owns no transport type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientRequest {
    /// The JSON request id this request must be answered on.
    pub id: Value,
    /// The ACP method Copilot asked the client to answer.
    pub method: String,
    /// The session the request belongs to, when the frame names one.
    pub session_id: Option<String>,
    /// The option id the frame offers for a one-time approval, when it offers
    /// exactly one.
    pub allow_once_option: Option<String>,
}

/// Read one client request out of a frame, when the frame is one.
pub fn client_request(message: &Value) -> Option<ClientRequest> {
    let id = message.get("id")?.clone();
    let method = message.get("method")?.as_str()?.to_owned();
    if message.get("result").is_some() || message.get("error").is_some() {
        return None;
    }
    let params = message.get("params");
    Some(ClientRequest {
        id,
        method,
        session_id: params
            .and_then(|params| params.get("sessionId"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        allow_once_option: params
            .and_then(|params| params.get("options"))
            .and_then(Value::as_array)
            .and_then(|options| {
                ["allow_once", "allow"].into_iter().find_map(|expected| {
                    options.iter().find_map(|option| {
                        let kind = option.get("kind")?.as_str()?;
                        let id = option.get("optionId")?.as_str()?.trim();
                        (kind == expected
                            && !id.is_empty()
                            && id.len() <= 256
                            && !id.contains('\0'))
                        .then(|| id.to_owned())
                    })
                })
            }),
    })
}

/// Whether a frame is a notification rather than a response.
pub fn is_notification(message: &Value) -> bool {
    message.get("method").is_some() && message.get("id").is_none()
}

/// Whether a frame is the response to the request carrying `expected`.
pub fn response_id_matches(message: &Value, expected: i64) -> bool {
    message.get("id").and_then(Value::as_i64) == Some(expected)
}

/// Read one session update out of a frame.
pub fn session_update(
    message: &Value,
    expected_session_id: Option<&str>,
) -> Result<AcpSessionUpdate, acp::AcpError> {
    acp::validate_session_update(message, expected_session_id)
}

/// Read one prompt result's stop reason out of a frame.
pub fn prompt_stop_reason(
    message: &Value,
    request_id: i64,
) -> Result<AcpStopReason, acp::AcpError> {
    acp::validate_prompt_response(message, request_id).map(|response| response.stop_reason)
}

/// This Agent's normalized transitions for one completed turn.
pub fn completed_transitions(output: &str) -> Vec<Transition> {
    terminal_transitions("copilot:reply", output, None)
}

/// This Agent's normalized transitions for one failed turn.
pub fn failed_transitions(code: &str, stage: &str, message: &str) -> Vec<Transition> {
    terminal_transitions("copilot:reply", "", Some((code, stage, message)))
}

fn terminal_transitions(
    unit_id: &str,
    output: &str,
    failure: Option<(&str, &str, &str)>,
) -> Vec<Transition> {
    let mut reducer = TransitionReducer::default();
    let mut transitions = reducer.advance(LifecycleStage::Accepted);
    if !output.is_empty() {
        transitions.extend(reducer.advance(LifecycleStage::Processing));
        transitions.extend(reducer.advance(LifecycleStage::Responding));
        transitions.push(Transition::Text {
            unit_id: unit_id.to_owned(),
            text: output.to_owned(),
        });
    }
    if let Some((code, stage, message)) = failure {
        if let Some(failure) = reducer.fail(code, stage, message) {
            transitions.push(failure);
        }
    } else {
        transitions.extend(reducer.advance(LifecycleStage::Completed));
    }
    transitions
}
