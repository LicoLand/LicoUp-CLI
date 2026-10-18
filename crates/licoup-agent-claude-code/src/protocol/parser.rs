//! The Claude Code CLI's `stream-json` dialect, and the parser that reads it.
//!
//! The parser is the sole ingress for this Agent's bytes: one line becomes one
//! [`ClaudeEffect`] here and is never re-parsed above, per ADR-0008. It reports
//! protocol finishes; it settles no turn, imposes no implicit timeout and hides
//! no content, so the conversation layer stays the sole turn authority.
//!
//! It also writes the frames the client sends *to* the CLI — the prompt, a
//! steer, an interrupt and a permission decision — because the two directions
//! are one dialect and splitting them would give each side half a protocol.

use licoup_agent_adapter_sdk::adapters::AdapterContract;
use licoup_agent_adapter_sdk::{LifecycleStage, Transition, TransitionReducer};
use serde_json::Value;
use std::io;

mod adapter;
pub mod events;
mod state;

pub use adapter::ClaudeCodeParser;
pub use state::{ClaudeCodeStateMachine, ProtocolFinishReport};

pub use crate::protocol::control::PermissionRequest;
pub use crate::protocol::params::DriverConfig;

/// This Agent's adapter declaration: how it frames bytes and what it reports.
pub const CONTRACT: AdapterContract = AdapterContract::new("claude-code", "lf-ndjson");

/// One line of this Agent's framing: a JSON document terminated by `\n`.
pub fn encode_message(message: &Value) -> io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(message).map_err(io::Error::other)?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// The prompt frame, in the dialect the CLI's streaming-input lane reads.
pub fn user_message(prompt: &str) -> Value {
    serde_json::json!({
        "type": "user",
        "message": {"role": "user", "content": [{"type": "text", "text": prompt}]}
    })
}

/// A user-initiated interrupt, with its own correlation identity.
pub fn interrupt_request() -> Value {
    serde_json::json!({
        "type": "control_request",
        "request_id": uuid::Uuid::new_v4().to_string(),
        "request": {"subtype": "interrupt"}
    })
}

/// A steer, or `None` when the text carries nothing to send.
pub fn steer_message(text: &str) -> Option<Value> {
    (!text.trim().is_empty()).then(|| user_message(text))
}

/// The answer the client writes back for one parked permission request.
pub fn permission_response(request_id: &str, tool_use_id: Option<&str>, allow: bool) -> Value {
    let mut response = serde_json::Map::new();
    response.insert(
        "subtype".to_owned(),
        serde_json::json!("permission_response"),
    );
    response.insert("request_id".to_owned(), serde_json::json!(request_id));
    if let Some(tool_use_id) = tool_use_id {
        response.insert("tool_use_id".to_owned(), serde_json::json!(tool_use_id));
    }
    response.insert(
        "response".to_owned(),
        serde_json::json!(if allow { "allow" } else { "deny" }),
    );
    serde_json::json!({"type": "control_response", "response": response})
}

/// The refusal this client writes back for one control request it does not
/// implement. Unimplemented interaction is declined explicitly rather than
/// silently dropped, and the native turn keeps running.
pub(crate) fn denied_control_response(request_id: &str) -> Value {
    serde_json::json!({
        "type": "control_response",
        "response": {
            "subtype": "error",
            "request_id": request_id,
            "error": "Client interaction is unavailable."
        }
    })
}

/// The approval one permission request asks the user for, or `None` when the
/// control request is not a permission request.
pub fn permission_request_details(message: &Value) -> Option<PermissionRequest> {
    let request_id = message.get("request_id").and_then(Value::as_str)?;
    let request = message.get("request")?;
    let subtype = request
        .get("subtype")
        .or_else(|| request.get("type"))
        .and_then(Value::as_str)?;
    if subtype != "permission_request" {
        return None;
    }
    let tool_use = request.get("toolUse").or_else(|| request.get("tool_use"));
    let tool_use_id = tool_use
        .and_then(|value| value.get("id"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let tool_name = tool_use
        .and_then(|value| value.get("name"))
        .and_then(Value::as_str)
        .map(|name| name.chars().take(64).collect::<String>());
    let prompt = request
        .get("prompt")
        .or_else(|| request.get("message"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let summary = prompt
        .or_else(|| {
            tool_name
                .as_ref()
                .map(|name| format!("Claude Code requests permission for: {name}"))
        })
        .unwrap_or_else(|| "Claude Code requests permission to continue.".to_owned());
    Some(PermissionRequest {
        request_id: request_id.to_owned(),
        tool_use_id,
        tool_name,
        summary,
    })
}

/// This Agent's normalized transitions for one completed execution.
pub fn completed_transitions(output: &str) -> Vec<Transition> {
    terminal_transitions("claude-code:reply", output)
}

/// This Agent's normalized transitions for one failed execution.
pub fn failure_transitions(code: &str, stage: &str, message: &str) -> Vec<Transition> {
    let mut reducer = TransitionReducer::default();
    let mut transitions = reducer.advance(LifecycleStage::Submitted);
    if let Some(failure) = reducer.fail(code, stage, message) {
        transitions.push(failure);
    }
    transitions
}

fn terminal_transitions(unit_id: &str, output: &str) -> Vec<Transition> {
    let mut reducer = TransitionReducer::default();
    let mut transitions = reducer.advance(LifecycleStage::Responding);
    if !output.is_empty() {
        transitions.push(Transition::Text {
            unit_id: unit_id.to_owned(),
            text: output.to_owned(),
        });
    }
    transitions.extend(reducer.advance(LifecycleStage::Completed));
    transitions
}

/// One effect a parsed Claude Code frame produces.
///
/// It carries only what the frame said. Every identity the parser bound earlier
/// is already on the report or the failure, so an effect never re-derives it.
pub enum ClaudeEffect {
    /// The CLI is asking the user to decide about one tool call.
    Permission(PermissionRequest),
    /// The CLI sent a control request this client does not implement, and the
    /// refusal to write back (or `None` when the request is uncorrelatable).
    Control { response: Option<Value> },
    /// The frame advanced the turn without finishing it.
    Progress { session_id: Option<String> },
    /// The turn's terminal result.
    ProtocolFinished(ProtocolFinishReport),
}
