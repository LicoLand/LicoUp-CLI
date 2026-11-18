//! The host's progressive turn-event emission, as one installed port.
//!
//! One OpenCode turn produces structured, redacted events as its stream
//! arrives: a bound turn, an assistant message chunk and a completed message.
//! *What* the events are is this package's answer; *where they go* is the
//! host's, because the host owns the consumer — a CLI `--stream-events` reader,
//! the Flutter NDJSON transport, or no consumer at all.
//!
//! The port is one installed function set rather than a second sink, so the
//! package never keeps a thread-local the host also keeps: a process installs
//! [`install`] once, and every event this package emits reaches the same
//! consumer the host's own emitters reach. Before installation the port is
//! fail-closed and emits nothing, which is the honest answer for a package
//! running outside the client.

use std::sync::OnceLock;

use serde_json::Value;

/// One progressive event, in the shape the host's consumers already read.
pub type TurnEventSink = fn(kind: &str, session_id: &str, turn_id: &str, payload: Value);

/// The host facilities one OpenCode turn's events need.
#[derive(Clone, Copy)]
pub struct TurnEventPort {
    /// Emit one named event with an explicit payload.
    pub emit_turn_event: TurnEventSink,
    /// Emit one streamed assistant message chunk.
    pub emit_agent_message_chunk: fn(session_id: &str, turn_id: &str, text: &str),
    /// Emit one completed assistant message.
    pub emit_agent_message_completed: fn(session_id: &str, turn_id: &str, text: &str),
}

static PORT: OnceLock<TurnEventPort> = OnceLock::new();

/// Install the host's emission once per process.
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

pub(crate) fn emit_agent_message_completed(session_id: &str, turn_id: &str, text: &str) {
    if let Some(port) = PORT.get() {
        (port.emit_agent_message_completed)(session_id, turn_id, text);
    }
}
