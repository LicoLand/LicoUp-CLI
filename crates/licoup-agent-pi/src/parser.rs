use crate::driver::errors::ProtocolFailure;
use licoup_agent_adapter_sdk::adapters::AdapterContract;
use licoup_agent_adapter_sdk::{LifecycleStage, Transition, TransitionReducer};
use serde_json::Value;

mod events;
mod protocol;

pub use events::{processing_evidence_kind, sanitized_event};
pub use protocol::{PendingInteraction, PiProtocol, ProtocolEffect, ProtocolOutcome};

pub const CONTRACT: AdapterContract = AdapterContract::new("pi", "lf-jsonl-rpc");

/// Decode one Pi JSONL frame at the parser boundary. The stdio transport owns
/// line acquisition only and never interprets vendor payloads.
pub fn decode_jsonl_line(line: &str) -> Result<Option<Value>, ()> {
    let line = line.strip_suffix('\n').unwrap_or(line);
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.is_empty() {
        return Ok(None);
    }
    serde_json::from_str(line).map(Some).map_err(|_| ())
}

pub fn session_header_has_id(line: &str, expected_id: &str) -> bool {
    serde_json::from_str::<Value>(line.trim_end_matches('\r'))
        .ok()
        .is_some_and(|header| {
            header.get("type").and_then(Value::as_str) == Some("session")
                && header.get("id").and_then(Value::as_str) == Some(expected_id)
        })
}

pub struct SteerAcknowledgement {
    pub request_id: String,
    pub accepted: bool,
}

pub fn classify_steer_response(message: &Value) -> Option<SteerAcknowledgement> {
    let request_id = message.get("id").and_then(Value::as_str)?;
    if !request_id.starts_with("lico-pi-steer-") {
        return None;
    }
    Some(SteerAcknowledgement {
        request_id: request_id.to_string(),
        accepted: message.get("type").and_then(Value::as_str) == Some("response")
            && message.get("success").and_then(Value::as_bool) == Some(true),
    })
}

pub fn encode_steer(text: String) -> (String, Value) {
    let request_id = format!("lico-pi-steer-{}", uuid::Uuid::new_v4().simple());
    let message = serde_json::json!({
        "id": request_id,
        "type": "steer",
        "message": text,
    });
    (request_id, message)
}

pub fn completed_transitions(output: &str) -> Vec<Transition> {
    let mut reducer = TransitionReducer::default();
    let mut transitions = reducer.advance(LifecycleStage::Accepted);
    transitions.extend(reducer.advance(LifecycleStage::Processing));
    transitions.extend(reducer.advance(LifecycleStage::Responding));
    transitions.push(Transition::Text {
        unit_id: "pi:reply".to_string(),
        text: output.to_string(),
    });
    transitions.extend(reducer.advance(LifecycleStage::Completed));
    transitions
}

pub fn failed_transitions(failure: &ProtocolFailure) -> Vec<Transition> {
    let mut reducer = TransitionReducer::default();
    let mut transitions = reducer.advance(LifecycleStage::Accepted);
    if let Some(failed) = reducer.fail(failure.code, failure.stage, failure.message) {
        transitions.push(failed);
    }
    transitions
}
