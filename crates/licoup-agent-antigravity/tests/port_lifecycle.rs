//! The ports' own lifecycle contract, in a process that installs nothing.
//!
//! Every port this package declares is fail-closed before its host answers it,
//! and one answer per process is the whole of what `install` accepts. Both claims
//! are about the process-wide port state, so they are asserted here rather than
//! in the crate's lib suite: that suite's driver tests install the ports to drive
//! a real turn, and a claim about the *uninstalled* state is only observable in a
//! process that installed nothing. The ports themselves are the package's own, so
//! their contract lives beside the package rather than in the host that installs
//! them.

use std::sync::Mutex;

use licoup_agent_antigravity::port::{execution, turn_event};
use serde_json::{Value, json};

/// What the installed turn-event answer below received.
static CAPTURED: Mutex<Vec<Value>> = Mutex::new(Vec::new());

#[test]
fn the_execution_port_is_fail_closed_and_answers_once() {
    // Every port is declared fail-closed: a package running outside the client
    // reports that it could not ask rather than inventing an answer.
    assert!(!execution::installed());
    assert_eq!(
        execution::subagent_caller_context(),
        Err(execution::HostEffect::Uninstalled)
    );
    assert!(
        !execution::admits_execution(),
        "a package that cannot ask the host's admission never claims it was admitted"
    );
    assert!(
        execution::install(execution::ExecutionPort {
            subagent_caller_context: || Some("caller".to_owned()),
            admits_execution: || true,
        })
        .is_ok()
    );
    assert!(execution::installed());
    assert_eq!(
        execution::subagent_caller_context(),
        Ok(Some("caller".to_owned()))
    );
    assert!(execution::admits_execution());
    // One answer per port per process: a second installation is refused rather
    // than silently replacing the first.
    assert_eq!(
        execution::install(execution::ExecutionPort {
            subagent_caller_context: || None,
            admits_execution: || false,
        }),
        Err("the agent-execution port is already installed")
    );
}

#[test]
fn the_turn_event_port_emits_nothing_until_the_host_installs_it() {
    // A package running outside the client emits through the port rather than
    // through a sink of its own, so before an installation there is no consumer
    // and an event reaches none.
    assert!(!turn_event::installed());
    turn_event::emit_turn_event("agent.turn.accepted", "session", "turn", json!({}));
    turn_event::emit_agent_processing("session", "turn", "activity", None);
    turn_event::emit_agent_message_chunk("session", "turn", "text");
    turn_event::emit_agent_message_completed("session", "turn", "text");
    assert!(
        CAPTURED.lock().unwrap().is_empty(),
        "an uninstalled port has no sink to reach"
    );
    assert!(
        !turn_event::installed(),
        "emitting never installs a sink of its own"
    );
    // The host answers once, and the turn reaches the answer the host supplied.
    assert!(
        turn_event::install(turn_event::TurnEventPort {
            emit_turn_event: record_event,
            emit_agent_message_chunk: record_chunk,
            emit_agent_message_completed: record_completed,
            emit_agent_processing: record_processing,
        })
        .is_ok()
    );
    turn_event::emit_agent_message_chunk("session", "turn", "installed");
    assert_eq!(
        CAPTURED.lock().unwrap().as_slice(),
        [json!("installed")],
        "the installed answer is the one the turn reaches"
    );
}

fn record_event(_kind: &str, _session_id: &str, _turn_id: &str, payload: Value) {
    CAPTURED.lock().unwrap().push(payload);
}

fn record_chunk(_session_id: &str, _turn_id: &str, text: &str) {
    CAPTURED.lock().unwrap().push(json!(text));
}

fn record_completed(_session_id: &str, _turn_id: &str, text: &str) {
    CAPTURED.lock().unwrap().push(json!(text));
}

fn record_processing(_session_id: &str, _turn_id: &str, evidence_kind: &str, _tool: Option<&str>) {
    CAPTURED.lock().unwrap().push(json!(evidence_kind));
}
