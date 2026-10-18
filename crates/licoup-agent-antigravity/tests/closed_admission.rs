//! The execution gate, in a process whose host answers it closed.
//!
//! `execute` asks the package's agent-execution port whether this host currently
//! admits a new execution, and a host that answers *no* must see the turn refused
//! before any vendor process starts. The answer is the host's own — in production
//! it is the close-admission barrier a maintenance switch holds — so the refusal
//! is asserted here, where the answer can be stated exactly, rather than in the
//! lib suite that installs an open answer to drive a turn.
//!
//! The other half of the chain is asserted where the host lives: the host's
//! composition installs an admission answer that reads that barrier (see
//! `crates/licoup-native/src/lib.rs` and the source-bundle contract over it).

use licoup_agent_antigravity::driver;
use licoup_agent_antigravity::port::execution;
use serde_json::json;

#[test]
fn a_refused_admission_stops_the_turn_before_any_process_starts() {
    assert!(
        execution::install(execution::ExecutionPort {
            subagent_caller_context: || None,
            admits_execution: || false,
        })
        .is_ok()
    );
    // The executable does not exist and would fail the launch on its own; the
    // admission refusal is the reported reason, which is what proves the gate is
    // read before the vendor CLI is started.
    let result = driver::execute(
        "definitely-not-a-real-antigravity",
        &json!({}),
        "exact user prompt",
        "",
        Some(std::env::temp_dir().as_path()),
        1_000,
        None,
        1_024,
    );
    assert!(!result.ok);
    assert_eq!(
        result.error.as_ref().map(|error| error.code),
        Some("antigravity_execution_admission_closed")
    );
    assert_eq!(
        result.error.as_ref().map(|error| error.stage),
        Some("turn/execute")
    );
    // The refusal is this turn's whole reported outcome: it carries the agent's
    // own failure transition and no session was bound.
    assert!(!result.transitions.is_empty());
    assert!(result.session_id.is_empty());
    assert!(result.output.is_empty());
}
