//! The host's progressive turn-event emission, as one installed port.
//!
//! One Codex turn produces structured, redacted events as its frames arrive:
//! a message chunk, a completed message, a processed item and a failed tool
//! call. *What* the events are is this package's answer, so the kind and the
//! payload are built here; *where they go* is the host's, because the host owns
//! the consumer — a CLI `--stream-events` reader, the Flutter NDJSON transport,
//! or no consumer at all. The port is therefore one sink: the host states where
//! an event goes and never re-derives what it is.
//!
//! The port is one installed function rather than a second sink, so the package
//! never keeps a thread-local the host also keeps: a process installs
//! [`install`] once, and every event this package emits reaches the same
//! consumer the host's own emitters reach. Before installation the port is
//! fail-closed and emits nothing, which is the honest answer for a package
//! running outside the client.

use std::sync::OnceLock;

use serde_json::{Value, json};

/// One progressive event, in the shape the host's consumers already read.
///
/// It is the host's own event shape, named here so the seam is a value rather
/// than a convention: `kind` is the event name, `session_id` and `turn_id`
/// locate it, and `payload` is the redacted body this package built.
pub type TurnEventSink = fn(kind: &str, session_id: &str, turn_id: &str, payload: Value);

/// The host facility one Codex turn's events need: where an event goes.
#[derive(Clone, Copy)]
pub struct TurnEventPort {
    pub emit: TurnEventSink,
}

static PORT: OnceLock<TurnEventPort> = OnceLock::new();

/// Install the host's emission once per process.
///
/// A second installation is refused rather than silently replacing the first:
/// the consumer belongs to one process, and a second answer would mean two
/// consumers for one turn.
pub fn install(port: TurnEventPort) -> Result<(), &'static str> {
    PORT.set(port)
        .map_err(|_| "the turn-event port is already installed")
}

/// Whether the host has installed its emission.
pub fn installed() -> bool {
    PORT.get().is_some()
}

pub(crate) fn emit_turn_event(kind: &str, session_id: &str, turn_id: &str, payload: Value) {
    dispatch(kind, session_id, turn_id, payload);
}

pub(crate) fn emit_agent_message_chunk(session_id: &str, turn_id: &str, text: &str) {
    if text.is_empty() {
        return;
    }
    emit_turn_event(
        "agent.message.chunk",
        session_id,
        turn_id,
        json!({
            "text": text,
            "lifecyclePrefix": ["submitted", "accepted", "processing", "responding"]
        }),
    );
}

pub(crate) fn emit_agent_message_completed(session_id: &str, turn_id: &str, text: &str) {
    if text.is_empty() {
        return;
    }
    emit_turn_event(
        "agent.message.completed",
        session_id,
        turn_id,
        json!({
            "text": text,
            "lifecyclePrefix": [
                "submitted",
                "accepted",
                "processing",
                "responding",
                "completed"
            ]
        }),
    );
}

/// Emit a redacted native-work receipt. The evidence kind is a fixed
/// classification such as `reasoning`, `plan` or `tool`; provider payloads and
/// model-authored reasoning never cross this boundary.
pub(crate) fn emit_agent_processing(
    session_id: &str,
    turn_id: &str,
    evidence_kind: &str,
    tool_name: Option<&str>,
) {
    let evidence_kind = match evidence_kind {
        "reasoning" => "reasoning",
        "plan" => "plan",
        "tool" => "tool",
        "progress" => "progress",
        _ => "activity",
    };
    let mut payload = json!({
        "evidenceKind": evidence_kind,
        "lifecyclePrefix": ["submitted", "accepted", "processing"]
    });
    if let Some(tool_name) = tool_name.filter(|name| !name.trim().is_empty()) {
        payload["toolName"] = json!(tool_name);
    }
    emit_turn_event("agent.turn.processing", session_id, turn_id, payload);
}

/// Emit only a fixed application error code for a completed native tool call.
/// Provider results, arguments, identifiers and model-authored text never
/// cross this projection boundary.
pub(crate) fn emit_agent_tool_error(
    session_id: &str,
    turn_id: &str,
    tool_name: &str,
    error_code: &str,
) {
    emit_turn_event(
        "agent.tool.result",
        session_id,
        turn_id,
        json!({
            "text": error_code,
            "toolName": tool_name,
            "status": "error",
            "lifecyclePrefix": ["submitted", "accepted", "processing"]
        }),
    );
}

fn dispatch(kind: &str, session_id: &str, turn_id: &str, payload: Value) {
    #[cfg(test)]
    if record_for_test(kind, session_id, turn_id, &payload) {
        return;
    }
    if let Some(port) = PORT.get() {
        (port.emit)(kind, session_id, turn_id, payload);
    }
}

#[cfg(test)]
thread_local! {
    static TEST_SINK: std::cell::RefCell<Option<Box<dyn Fn(Value)>>> =
        const { std::cell::RefCell::new(None) };
}

/// Capture this thread's progressive events instead of the installed port.
///
/// One process installs the port once, so a test that needs to read what a
/// turn emitted takes a thread-local capture rather than a second port. The
/// captured value is the consumer-shaped envelope the host's own readers see.
/// The capture is installed by [`install_test_sink`] and cleared by the guard's
/// own drop, exactly as the host's own stream sink is.
#[cfg(test)]
pub struct TestSinkGuard;

#[cfg(test)]
pub fn install_test_sink(sink: Box<dyn Fn(Value)>) {
    TEST_SINK.with(|cell| *cell.borrow_mut() = Some(sink));
}

#[cfg(test)]
impl Drop for TestSinkGuard {
    fn drop(&mut self) {
        TEST_SINK.with(|cell| *cell.borrow_mut() = None);
    }
}

#[cfg(test)]
fn record_for_test(kind: &str, session_id: &str, turn_id: &str, payload: &Value) -> bool {
    if !TEST_SINK.with(|cell| cell.borrow().is_some()) {
        return false;
    }
    let envelope = json!({
        "event": kind,
        "sessionId": session_id,
        "turnId": turn_id,
        "payload": payload,
    });
    TEST_SINK.with(|cell| {
        if let Some(sink) = cell.borrow().as_ref() {
            sink(envelope);
        }
    });
    true
}
