//! Hermes' persistent-ACP parser: one raw stdio line in, one classified frame
//! out.
//!
//! The parser is the sole ingress for Hermes frames, per ADR-0008: a byte line
//! becomes the ACP envelope here, is validated against the request it answers,
//! and is never decoded again above this port. The two facts that belong to no
//! other Agent live beside it:
//!
//! * [`permission_request`] reads Hermes' own permission question — the one ACP
//!   frame shape that carries a display summary and a requested-tool list.
//! * [`completed_transitions`] and [`failed_transitions`] word one Hermes turn
//!   in the shared transition vocabulary. Hermes' driver reports no transition
//!   list of its own, so these are the answer the host's normalization reads
//!   through the SDK's protocol-agnostic query rather than from an execution
//!   result.
//!
//! Everything else delegates to `licoup_foundation::core::acp`, because the ACP
//! envelope is a published contract and not one Agent's protocol.

use licoup_agent_adapter_sdk::adapters::AdapterContract;
use licoup_agent_adapter_sdk::{LifecycleStage, Transition, TransitionReducer};
use licoup_foundation::core::acp::{self, AcpSessionUpdate, AcpStopReason};
use serde_json::Value;

/// The adapter declaration Hermes' parser reports.
pub const CONTRACT: AdapterContract = AdapterContract::new("hermes", "stdio-jsonrpc-acp");

/// Decode one raw byte line into a frame, or report why it is not one.
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

/// One permission question Hermes asks the client to answer.
///
/// It is Hermes' own frame shape: the ACP persistent profile asks on a request
/// id and may carry the tool calls it is asking about, which is more than the
/// shared ACP profile's client request states.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionRequest {
    /// The JSON request id this question must be answered on.
    pub id: Value,
    /// The ACP method Hermes asked the client to answer.
    pub method: String,
    /// The session the question belongs to, when the frame names one.
    pub session_id: Option<String>,
    /// The redacted summary shown to the user.
    pub display_summary: String,
    /// The single option id the frame offers for a one-time approval.
    pub option_id: Option<String>,
    /// The tools Hermes is asking permission to use, bounded and deduplicated by
    /// position rather than by content.
    pub requested_tools: Vec<String>,
}

/// Read one permission question out of a frame, when the frame is one.
///
/// A frame that answers a request (`result` or `error`) is never a question, and
/// the summary and the tool list are derived here rather than above the port.
pub fn permission_request(message: &Value) -> Option<PermissionRequest> {
    let id = message.get("id")?.clone();
    let method = message.get("method")?.as_str()?.to_owned();
    if message.get("result").is_some() || message.get("error").is_some() {
        return None;
    }
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let mut requested_tools = params
        .get("toolCalls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|call| {
            call.get("title")
                .or_else(|| call.get("kind"))
                .or_else(|| call.pointer("/toolCall/title"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(|name| name.chars().take(64).collect::<String>())
        })
        .take(8)
        .collect::<Vec<_>>();
    requested_tools.shrink_to_fit();
    let option_id = params
        .get("options")
        .and_then(Value::as_array)
        .and_then(|options| {
            options
                .iter()
                .find(|option| {
                    matches!(
                        option.get("kind").and_then(Value::as_str),
                        Some("allow_once" | "allow_always" | "allow")
                    ) || option
                        .get("optionId")
                        .and_then(Value::as_str)
                        .is_some_and(|id| id.contains("allow"))
                })
                .or_else(|| options.first())
        })
        .and_then(|option| option.get("optionId"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let display_summary = if requested_tools.is_empty() {
        "Hermes Agent requests permission to continue.".to_owned()
    } else {
        format!(
            "Hermes Agent requests permission for: {}",
            requested_tools.join(", ")
        )
    };
    Some(PermissionRequest {
        id,
        method,
        session_id: params
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_owned),
        display_summary,
        option_id,
        requested_tools,
    })
}

/// Whether a frame is a notification rather than a response.
pub fn is_notification(message: &Value) -> bool {
    message.get("method").is_some() && message.get("id").is_none()
}

/// Whether a frame is the response to the request carrying `expected`.
pub fn response_id_matches(message: &Value, expected: i64) -> bool {
    message.get("id").is_some_and(|id| {
        id.as_i64() == Some(expected)
            || id
                .as_str()
                .is_some_and(|value| value == expected.to_string())
    })
}

/// Whether a frame reports a protocol-level error.
pub fn response_is_error(message: &Value) -> bool {
    message.get("error").is_some()
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

/// Hermes' normalized transitions for one completed turn.
///
/// Hermes reports no transition list with its execution result, so this is the
/// answer the host reads through the SDK's protocol-agnostic query: the turn is
/// walked from acceptance to completion, and the reply is Hermes' own unit id.
pub fn completed_transitions(output: &str) -> Vec<Transition> {
    let mut reducer = TransitionReducer::default();
    let mut transitions = reducer.advance(LifecycleStage::Accepted);
    transitions.extend(reducer.advance(LifecycleStage::Processing));
    transitions.extend(reducer.advance(LifecycleStage::Responding));
    transitions.push(Transition::Text {
        unit_id: "hermes:reply".to_owned(),
        text: output.to_owned(),
    });
    transitions.extend(reducer.advance(LifecycleStage::Completed));
    transitions
}

/// Hermes' normalized transitions for one failed turn.
///
/// The failure carries the protocol's own code, stage and redacted message, and
/// the first failure is write-once in the shared reducer.
pub fn failed_transitions(code: &str, stage: &str, message: &str) -> Vec<Transition> {
    let mut reducer = TransitionReducer::default();
    let mut transitions = reducer.advance(LifecycleStage::Accepted);
    if let Some(failure) = reducer.fail(code, stage, message) {
        transitions.push(failure);
    }
    transitions
}
