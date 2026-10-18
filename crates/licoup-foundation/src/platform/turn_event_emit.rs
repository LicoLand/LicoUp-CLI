//! Progressive turn-event emission for bound-on-send streaming.
//!
//! When a stream sink is installed for the duration of one `send`, drivers emit
//! redacted structured events as chunks arrive. Callers (CLI `--stream-events`,
//! Flutter NDJSON consumers) watch progress without a mid-run inject channel.

use serde_json::{Value, json};
use std::cell::RefCell;

type Sink = Box<dyn Fn(Value) + Send + Sync>;

thread_local! {
    // A stream sink belongs to one synchronous dispatch. Keeping it thread-local
    // prevents concurrent sends and parallel tests from replacing or observing
    // each other's event consumer.
    static SINK_SLOT: RefCell<Option<Sink>> = RefCell::new(None);
}

/// Install a thread-local sink for the duration of one send. Replaces any prior sink
/// on the current thread only.
pub fn install_stream_sink(sink: Sink) {
    SINK_SLOT.with(|slot| *slot.borrow_mut() = Some(sink));
}

/// Clear the current thread's sink.
pub fn clear_stream_sink() {
    SINK_SLOT.with(|slot| *slot.borrow_mut() = None);
}

/// Emit one progressive dispatch event when a sink is installed. No-op otherwise.
pub fn emit_turn_event(kind: &str, session_id: &str, turn_id: &str, payload: Value) {
    let event = json!({
        "event": kind,
        "sessionId": session_id,
        "turnId": turn_id,
        "payload": payload,
    });
    SINK_SLOT.with(|slot| {
        if let Some(sink) = slot.borrow().as_ref() {
            sink(event);
        }
    });
}

/// Emit an agent message chunk for real-time display.
pub fn emit_agent_message_chunk(session_id: &str, turn_id: &str, text: &str) {
    emit_agent_message_chunk_for_unit(session_id, turn_id, "", text);
}

/// Emit one chunk for a stable adapter-owned message unit. The unit is a
/// redacted local ordinal, never a provider message identifier.
pub fn emit_agent_message_chunk_for_unit(
    session_id: &str,
    turn_id: &str,
    message_unit: &str,
    text: &str,
) {
    if text.is_empty() {
        return;
    }
    let mut payload = json!({
        "text": text,
        "lifecyclePrefix": ["submitted", "accepted", "processing", "responding"]
    });
    if !message_unit.is_empty() {
        payload["messageUnit"] = json!(message_unit);
    }
    emit_turn_event("agent.message.chunk", session_id, turn_id, payload);
}

/// Emit a completed agent message item (Codex item/completed path).
pub fn emit_agent_message_completed(session_id: &str, turn_id: &str, text: &str) {
    emit_agent_message_completed_for_unit(session_id, turn_id, "", text);
}

/// Complete one adapter-owned message unit without merging it into an earlier
/// assistant message from the same turn.
pub fn emit_agent_message_completed_for_unit(
    session_id: &str,
    turn_id: &str,
    message_unit: &str,
    text: &str,
) {
    if text.is_empty() {
        return;
    }
    let mut payload = json!({
        "text": text,
        "lifecyclePrefix": ["submitted", "accepted", "processing", "responding", "completed"]
    });
    if !message_unit.is_empty() {
        payload["messageUnit"] = json!(message_unit);
    }
    emit_turn_event("agent.message.completed", session_id, turn_id, payload);
}

/// Emit a redacted native-work receipt. The evidence kind is a fixed adapter
/// classification such as `reasoning`, `plan`, or `tool`; provider payloads
/// and model-authored reasoning never cross this boundary.
pub fn emit_agent_processing(
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
/// Provider results, arguments, identifiers, and model-authored text never
/// cross this projection boundary.
pub fn emit_agent_tool_error(session_id: &str, turn_id: &str, tool_name: &str, error_code: &str) {
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

/// RAII guard that clears the sink on drop.
pub struct StreamSinkGuard;

impl Drop for StreamSinkGuard {
    fn drop(&mut self) {
        clear_stream_sink();
    }
}

/// Install a stdout NDJSON sink and return a guard that clears it.
pub fn install_stdout_ndjson_sink() -> StreamSinkGuard {
    install_stream_sink(Box::new(|event| {
        let _ = write_stdout_json_line(&event);
    }));
    StreamSinkGuard
}

fn write_stdout_json_line(value: &Value) -> std::io::Result<()> {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, value)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn emit_is_noop_without_sink() {
        clear_stream_sink();
        emit_agent_message_chunk("s", "t", "hello");
    }

    #[test]
    fn emit_delivers_to_installed_sink() {
        let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
        let sink_target = Arc::clone(&captured);
        install_stream_sink(Box::new(move |event| {
            sink_target.lock().unwrap().push(event);
        }));
        let _guard = StreamSinkGuard;
        emit_agent_message_chunk("sess-1", "turn-1", "chunk");
        let events = captured.lock().unwrap().clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["event"], "agent.message.chunk");
        assert_eq!(events[0]["sessionId"], "sess-1");
        assert_eq!(events[0]["payload"]["text"], "chunk");
    }

    #[test]
    fn processing_receipt_exposes_only_bounded_evidence_kind() {
        let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
        let sink_target = Arc::clone(&captured);
        install_stream_sink(Box::new(move |event| {
            sink_target.lock().unwrap().push(event);
        }));
        let _guard = StreamSinkGuard;

        emit_agent_processing("sess-1", "turn-1", "provider-private-value", None);

        let events = captured.lock().unwrap().clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["event"], "agent.turn.processing");
        assert_eq!(
            events[0]["payload"],
            json!({
                "evidenceKind": "activity",
                "lifecyclePrefix": ["submitted", "accepted", "processing"]
            })
        );
        assert!(!events[0].to_string().contains("provider-private-value"));

        captured.lock().unwrap().clear();
        emit_agent_processing("sess-1", "turn-1", "tool", Some("Bash"));
        let events = captured.lock().unwrap().clone();
        assert_eq!(
            events[0]["payload"],
            json!({
                "evidenceKind": "tool",
                "toolName": "Bash",
                "lifecyclePrefix": ["submitted", "accepted", "processing"]
            })
        );
    }

    #[test]
    fn tool_error_receipt_projects_only_the_fixed_code_and_tool_name() {
        let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
        let sink_target = Arc::clone(&captured);
        install_stream_sink(Box::new(move |event| {
            sink_target.lock().unwrap().push(event);
        }));
        let _guard = StreamSinkGuard;

        emit_agent_tool_error(
            "sess-1",
            "turn-1",
            "lico_subagent_delegate",
            "subagent_cross_conversation_rejected",
        );

        let events = captured.lock().unwrap().clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["event"], "agent.tool.result");
        assert_eq!(events[0]["payload"]["toolName"], "lico_subagent_delegate");
        assert_eq!(
            events[0]["payload"]["text"],
            "subagent_cross_conversation_rejected"
        );
    }

    #[test]
    fn concurrent_sinks_are_order_isolated_by_dispatch_thread() {
        let workers = (0..16)
            .map(|index| {
                std::thread::spawn(move || {
                    let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
                    let sink_target = Arc::clone(&captured);
                    install_stream_sink(Box::new(move |event| {
                        sink_target.lock().unwrap().push(event);
                    }));
                    let _guard = StreamSinkGuard;
                    let session_id = format!("session-{index}");
                    emit_agent_message_chunk(&session_id, "turn", "chunk");
                    let events = captured.lock().unwrap().clone();
                    assert_eq!(events.len(), 1);
                    assert_eq!(events[0]["sessionId"], session_id);
                })
            })
            .collect::<Vec<_>>();

        for worker in workers {
            worker.join().unwrap();
        }
    }
}
