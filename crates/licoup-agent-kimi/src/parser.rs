//! Kimi Code's ACP frame interpretation, once below the adapter port.
//!
//! Kimi Code speaks the published ACP v1 profile over stdio (ADR-0008), so the
//! wire vocabulary is shared with the other ACP Agents and lives in
//! `licoup_foundation::core::acp`. What is Kimi's is which of those frames this
//! Agent answers with, and what one of them means for a turn: that is stated
//! here, exactly once, and the shared ACP reducer never re-reads a raw line.
//!
//! The module is the interpreter, not the engine. It starts no process, keeps
//! no session state, settles no turn and writes nothing; the shared transport in
//! `licoup-agent-drivers` owns all of that and reads this module through
//! [`crate::dialect`].

use licoup_agent_adapter_sdk::adapters::AdapterContract;
use licoup_agent_adapter_sdk::{LifecycleStage, Transition, TransitionReducer};
use licoup_foundation::core::acp::{self, AcpSessionUpdate, AcpStopReason};
use serde_json::Value;

/// This Agent's adapter declaration, as composition and the corpus check read it.
pub const CONTRACT: AdapterContract = AdapterContract::new("kimi-code", "lf-ndjson-acp");

/// Kimi Code owns a distinct ACP interpretation even though its physical
/// framing is also LF-delimited JSON.
pub fn decode_frame(line: &[u8]) -> Result<Value, acp::AcpError> {
    acp::decode_json_line(line)
}

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientRequest {
    pub id: Value,
    pub method: String,
    pub session_id: Option<String>,
    pub allow_once_option: Option<String>,
}

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

pub fn is_notification(message: &Value) -> bool {
    message.get("method").is_some() && message.get("id").is_none()
}

pub fn response_id_matches(message: &Value, expected: i64) -> bool {
    message.get("id").and_then(Value::as_i64) == Some(expected)
}

pub fn session_update(
    message: &Value,
    expected_session_id: Option<&str>,
) -> Result<AcpSessionUpdate, acp::AcpError> {
    acp::validate_session_update(message, expected_session_id)
}

pub fn prompt_stop_reason(
    message: &Value,
    request_id: i64,
) -> Result<AcpStopReason, acp::AcpError> {
    acp::validate_prompt_response(message, request_id).map(|response| response.stop_reason)
}

/// The transitions one completed Kimi turn reports.
///
/// The turn is accepted, processing, responding and completed in arrival order,
/// with the Agent's whole reply as the one text unit: an execution outcome is
/// reported after the stream closed, so the reducer is advanced to its terminal
/// stage rather than replayed frame by frame.
pub fn completed_transitions(output: &str) -> Vec<Transition> {
    let mut reducer = TransitionReducer::default();
    let mut transitions = reducer.advance(LifecycleStage::Accepted);
    transitions.extend(reducer.advance(LifecycleStage::Processing));
    transitions.extend(reducer.advance(LifecycleStage::Responding));
    transitions.push(Transition::Text {
        unit_id: "kimi-code:reply".to_owned(),
        text: output.to_owned(),
    });
    transitions.extend(reducer.advance(LifecycleStage::Completed));
    transitions
}

/// The transitions one failed Kimi turn reports, in the arrival order the
/// reducer produces.
pub fn failed_transitions(code: &str, stage: &str, message: &str) -> Vec<Transition> {
    let mut reducer = TransitionReducer::default();
    let mut transitions = reducer.advance(LifecycleStage::Accepted);
    if let Some(failure) = reducer.fail(code, stage, message) {
        transitions.push(failure);
    }
    transitions
}
