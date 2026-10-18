//! The host's progressive turn-event emission, as one installed port.
//!
//! One OpenClaw turn produces structured, redacted events as its Gateway frames
//! arrive: the named dispatch events that mark a Gateway attach and a bound
//! conversation (`dispatch.gateway.attached`, `dispatch.turn.bound`), processing
//! evidence and streamed assistant message chunks. *What* the events are is this
//! package's answer; *where they go* is the host's, because the host owns the
//! consumer — a CLI `--stream-events` reader, the Flutter NDJSON transport, or no
//! consumer at all.
//!
//! The port is one installed function set rather than a second sink, so the
//! package never keeps a thread-local the host also keeps: a process installs
//! [`install`] once, and every event this package emits reaches the same
//! consumer the host's own emitters reach. Before installation the port is
//! fail-closed and emits nothing, which is the honest answer for a package
//! running outside the client.
//!
//! All three emissions are the package's own: the Gateway attach event is
//! produced by the transport this package runs, beside the frames it writes, and
//! a stream reader drops a whole turn when any single event lacks a bound
//! identity — so the package that knows the identity is the one that states it.

use serde_json::Value;
use std::sync::OnceLock;

/// One progressive event, in the shape the host's consumers already read.
///
/// It is the host's own event shape, named here so the seam is a value rather
/// than a convention: `kind` is the event name, `session_id` and `turn_id`
/// locate it, and `payload` is the redacted body.
pub type TurnEventSink = fn(kind: &str, session_id: &str, turn_id: &str, payload: Value);

/// The host facilities one OpenClaw turn's events need.
///
/// The port carries exactly the three emissions this package produces — one
/// named dispatch event, a streamed assistant message chunk and one processing
/// observation — rather than the host's whole event vocabulary: a package cannot
/// ask for an emission it does not make.
#[derive(Clone, Copy)]
pub struct TurnEventPort {
    /// Emit one named event with an explicit payload.
    pub emit_turn_event: TurnEventSink,
    /// Emit one streamed assistant message chunk.
    pub emit_agent_message_chunk: fn(session_id: &str, turn_id: &str, text: &str),
    /// Emit one processing observation with its evidence kind and tool name.
    pub emit_agent_processing:
        fn(session_id: &str, turn_id: &str, evidence_kind: &str, tool_name: Option<&str>),
}

static PORT: OnceLock<TurnEventPort> = OnceLock::new();

/// Install the host's emission once per process.
///
/// A second installation with a different port is refused rather than silently
/// replacing the first: the consumer belongs to one process, and a second
/// answer would mean two consumers for one turn.
pub fn install(port: TurnEventPort) -> Result<(), &'static str> {
    PORT.set(port)
        .map_err(|_| "the turn-event port is already installed")
}

/// Whether the host has installed its emission.
pub fn installed() -> bool {
    PORT.get().is_some()
}

pub(crate) fn emit_turn_event(kind: &str, session_id: &str, turn_id: &str, payload: Value) {
    if let Some(port) = PORT.get() {
        (port.emit_turn_event)(kind, session_id, turn_id, payload);
    }
}

pub(crate) fn emit_agent_message_chunk(session_id: &str, turn_id: &str, text: &str) {
    if let Some(port) = PORT.get() {
        (port.emit_agent_message_chunk)(session_id, turn_id, text);
    }
}

pub(crate) fn emit_agent_processing(
    session_id: &str,
    turn_id: &str,
    evidence_kind: &str,
    tool_name: Option<&str>,
) {
    if let Some(port) = PORT.get() {
        (port.emit_agent_processing)(session_id, turn_id, evidence_kind, tool_name);
    }
}
