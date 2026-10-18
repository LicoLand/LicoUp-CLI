//! The `lico-agent` RPC protocol below the adapter port.
//!
//! Lico Agent is LicoUp's own runtime, not a third-party CLI, and the interface
//! between this host and the packaged `lico-agent` program is nonetheless a
//! protocol with its own shapes: the `lf-jsonl-jsonrpc` channel, one
//! LF-delimited JSON document per frame, carrying the readiness handshake, the
//! prompt request the host writes and the handshake, text, progress, control and
//! terminal frames the program answers with.
//!
//! This module is the only place that interprets those frames. One line becomes
//! one [`RpcEffect`] here and is never re-parsed above, per ADR-0008, and the
//! request envelopes the host writes are built here for the same reason: a
//! second copy of an envelope is a second protocol.
//!
//! The parser settles no turn. It reports what the frame said — including
//! [`RpcEffect::Ignored`] for a frame this protocol does not carry a fact for —
//! and [`success_transitions`] / [`failure_transitions`] project one execution
//! outcome onto the shared transition vocabulary. The conversation layer
//! remains the sole turn authority.

use licoup_agent_adapter_sdk::adapters::{AdapterContract, NativeLineParser};
use licoup_agent_adapter_sdk::{LifecycleStage, Transition, TransitionReducer};
use serde_json::{Value, json};

/// The runtime protocol id this Agent's turns run under.
///
/// It is the id the execution surface reports for the packaged program's stdio
/// JSONL RPC, and it is this package's fact rather than the host's: a reader
/// that sees it named knows which protocol's parser interpreted the turn.
pub const RUNTIME_PROTOCOL: &str = "lico-agent-rpc-stdio-jsonl";

/// The wire format this protocol's frames are recorded and converted under.
///
/// It is the format name the package's release declaration publishes as its
/// inbound format, and it is the same protocol [`RUNTIME_PROTOCOL`] names, so
/// the declaration and the runtime cannot describe two different wires.
pub const PROTOCOL_FORMAT: &str = "lico-agent.rpc-stdio-jsonl.v1";

/// This Agent's adapter declaration: the id dispatch names and the framing this
/// protocol really speaks.
///
/// The framing string is the channel the recorded corpus carries, so a
/// transcript recorded on another channel cannot pass against this parser.
pub const CONTRACT: AdapterContract = AdapterContract::new("lico-agent", "lf-jsonl-jsonrpc");

/// The readiness handshake request this host writes first.
///
/// The request id is part of the protocol: the packaged program answers it with
/// a `response` frame, and the parser classifies that answer as the handshake.
pub fn readiness_request() -> Value {
    json!({"id": "lico-1", "type": "get_state"})
}

/// The prompt request this host writes once the handshake has been accepted.
///
/// The prompt text is carried verbatim and is never trimmed, escaped or
/// re-framed here; the protocol's own JSON encoding is the only transform.
pub fn prompt_request(message: &str) -> Value {
    json!({"id": "lico-2", "type": "prompt", "message": message})
}

/// What one `lico-agent` frame reported.
///
/// The variants are the protocol's own; a reader matches on them rather than on
/// the raw frame, so a field name is read in exactly one place.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RpcEffect {
    /// A `response` frame. `accepted` is the protocol's own success flag and
    /// `session_id` is present only when the answer carried one.
    Handshake {
        accepted: bool,
        session_id: Option<String>,
    },
    /// One streamed assistant text delta.
    Text { delta: String },
    /// A frame that reports the turn is still working.
    Processing,
    /// A frame that asks the client for an explicit answer.
    Control { method: String },
    /// The turn's terminal frame.
    Completed,
    /// A terminal failure frame, with the program's own code when it sent one.
    Failed { code: Option<String> },
    /// A frame this protocol carries no fact for. It is reported rather than
    /// dropped, so a reader can tell "nothing to project" from "never seen".
    Ignored,
}

/// Why one line could not be read as a frame of this protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameError {
    /// The line carried no frame at all.
    Empty,
    /// The line was not one JSON document.
    InvalidJson,
}

/// Encode one request as the bytes this protocol writes: the JSON document and
/// its LF terminator.
pub fn encode_request(value: &Value) -> Result<Vec<u8>, serde_json::Error> {
    let mut encoded = serde_json::to_vec(value)?;
    encoded.push(b'\n');
    Ok(encoded)
}

/// The one frame parser this protocol has.
///
/// It carries no per-turn state: every classification is a function of the line
/// alone, so replaying a recorded transcript constructs exactly the parser a
/// live turn drives.
#[derive(Default)]
pub struct RpcParser;

impl NativeLineParser for RpcParser {
    type Report = RpcEffect;
    type Error = FrameError;

    fn parse_line(&mut self, line: &[u8]) -> Result<Self::Report, Self::Error> {
        let line = std::str::from_utf8(line)
            .map_err(|_| FrameError::InvalidJson)?
            .trim();
        if line.is_empty() {
            return Err(FrameError::Empty);
        }
        let event: Value = serde_json::from_str(line).map_err(|_| FrameError::InvalidJson)?;
        if event.get("type").and_then(Value::as_str) == Some("response") {
            return Ok(RpcEffect::Handshake {
                accepted: event.get("success").and_then(Value::as_bool) == Some(true),
                session_id: event
                    .pointer("/data/sessionId")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            });
        }
        if let Some(delta) = event
            .pointer("/assistantMessageEvent/delta")
            .and_then(Value::as_str)
        {
            return Ok(RpcEffect::Text {
                delta: delta.to_owned(),
            });
        }
        match event.get("type").and_then(Value::as_str) {
            Some("agent.event" | "agent.progress" | "agent.tool") => Ok(RpcEffect::Processing),
            Some("agent.interaction") => Ok(RpcEffect::Control {
                method: "agent.interaction".to_owned(),
            }),
            Some("agent_end") => Ok(RpcEffect::Completed),
            Some("error") => Ok(RpcEffect::Failed {
                code: event
                    .get("code")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            }),
            _ => Ok(RpcEffect::Ignored),
        }
    }
}

/// The shared transition vocabulary for one accepted `lico-agent` turn.
pub fn success_transitions(
    output: &str,
    saw_processing: bool,
    controls: &[String],
) -> Vec<Transition> {
    terminal_transitions(output, saw_processing, controls, None)
}

/// The shared transition vocabulary for one refused or failed `lico-agent`
/// turn, reported with the program's own code, stage and message.
pub fn failure_transitions(code: &str, stage: &str, message: &str) -> Vec<Transition> {
    terminal_transitions("", false, &[], Some((code, stage, message)))
}

/// The prefix-closed walk the reducer owns: accepted, then the stages this turn
/// really reached, in arrival order, and one terminal entry.
fn terminal_transitions(
    output: &str,
    saw_processing: bool,
    controls: &[String],
    failure: Option<(&str, &str, &str)>,
) -> Vec<Transition> {
    let mut reducer = TransitionReducer::default();
    let mut transitions = reducer.advance(LifecycleStage::Accepted);
    if saw_processing {
        transitions.extend(reducer.advance(LifecycleStage::Processing));
    }
    transitions.extend(controls.iter().map(|method| Transition::Control {
        method: method.clone(),
        summary: "Native agent interaction requires an explicit client response.".to_owned(),
    }));
    if !output.is_empty() {
        transitions.extend(reducer.advance(LifecycleStage::Responding));
        transitions.push(Transition::Text {
            unit_id: "lico-agent:reply".to_owned(),
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
