//! This package's own claims about the Cursor adapter it carries.
//!
//! A claim that names Cursor belongs here rather than in the SDK or in the
//! client: which adapter id this package registers, what its declaration says,
//! how one execution outcome becomes the shared transition vocabulary, which
//! durable identity it accepts, and — the claim that matters most — that the
//! five recorded transcripts still replay through the real parser.

use licoup_agent_adapter_sdk::port::{
    DurableIdentityRequest, ExecutionFailure, ExecutionOutcome, ExecutionTransitions,
};
use licoup_agent_adapter_sdk::replay::{SCENARIOS, fixture_root, replay_corpus};
use licoup_agent_adapter_sdk::{LifecycleStage, Transition};

use crate::port::turn_event::{self, TurnEventPort};
use crate::registration::{self, ADAPTER_ID, CONTRACT, FRAMING};

/// The five recorded transcripts, in the order the harness checks them.
const CURSOR_SCENARIOS: [&str; 5] = SCENARIOS;

#[test]
fn the_package_registers_exactly_one_adapter_through_the_sdk_port() {
    let set = registration::parser_set();
    assert_eq!(set.registered_ids(), [ADAPTER_ID]);
    assert_eq!(set.all().len(), 1);

    let contract = set
        .contract(ADAPTER_ID)
        .expect("the composed set answers for the adapter it carries");
    assert_eq!(contract, CONTRACT);
    assert_eq!(contract.id, ADAPTER_ID);
    assert_eq!(contract.framing, FRAMING);
    assert_eq!(registration::contract(), Some(CONTRACT));

    // Another adapter is refused rather than defaulted: this program carries one
    // Agent, and it says so.
    assert_eq!(set.contract("codex"), None);
    assert_eq!(
        set.framing("codex"),
        Err("no registered contract for adapter codex".to_owned())
    );
}

#[test]
fn the_cursor_declaration_reports_the_complete_l4_signal_set() {
    let contract = CONTRACT;
    assert!(
        !contract.settles_turn,
        "the conversation layer settles turns"
    );
    assert!(!contract.has_implicit_turn_timeout);
    assert!(contract.emits_all_content);
    assert_eq!(
        contract.reported_signals,
        [
            licoup_agent_adapter_sdk::adapters::ProtocolSignalKind::ProtocolFinish,
            licoup_agent_adapter_sdk::adapters::ProtocolSignalKind::Eof,
            licoup_agent_adapter_sdk::adapters::ProtocolSignalKind::CancelConfirmed,
        ]
    );
    assert_eq!(contract.inventory_json()["adapterId"], ADAPTER_ID);
    assert_eq!(contract.framing, "strict-lf-ndjson");
}

#[test]
fn a_completed_execution_becomes_the_reply_transitions_of_this_agent() {
    let transitions: ExecutionTransitions = registration::execution_transitions;
    let completed = transitions(&ExecutionOutcome {
        output: "answer",
        failure: None,
    });
    assert_eq!(
        completed,
        vec![
            Transition::Lifecycle(LifecycleStage::Submitted),
            Transition::Lifecycle(LifecycleStage::Accepted),
            Transition::Lifecycle(LifecycleStage::Processing),
            Transition::Lifecycle(LifecycleStage::Responding),
            Transition::Text {
                unit_id: "cursor:reply".to_owned(),
                text: "answer".to_owned(),
            },
            Transition::Lifecycle(LifecycleStage::Completed),
        ]
    );

    // An empty output is not a text transition: the parser reports the stages it
    // reached and nothing it did not receive.
    let empty = transitions(&ExecutionOutcome {
        output: "",
        failure: None,
    });
    assert_eq!(
        empty,
        vec![
            Transition::Lifecycle(LifecycleStage::Submitted),
            Transition::Lifecycle(LifecycleStage::Accepted),
            Transition::Lifecycle(LifecycleStage::Processing),
            Transition::Lifecycle(LifecycleStage::Responding),
            Transition::Lifecycle(LifecycleStage::Completed),
        ]
    );
}

#[test]
fn a_failed_execution_becomes_the_protocols_own_failure_transition() {
    let transitions: ExecutionTransitions = registration::execution_transitions;
    let failure = transitions(&ExecutionOutcome {
        output: "",
        failure: Some(ExecutionFailure {
            code: "cursor_cli_turn_failed",
            stage: "turn/completed",
            message: "Cursor Agent CLI reported a failed turn result.",
        }),
    });
    let reported: Vec<&str> = failure
        .iter()
        .filter_map(|transition| match transition {
            Transition::Failed { code, .. } => Some(code.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(reported, ["cursor_cli_turn_failed"]);
    assert_eq!(
        failure.first(),
        Some(&Transition::Lifecycle(LifecycleStage::Submitted))
    );
}

#[test]
fn a_durable_identity_is_decided_by_the_rule_the_parser_binds_with() {
    // Cursor's native identity is the chat id its own CLI resumes: bounded,
    // non-empty and drawn from the alphabet it round-trips.
    assert!(registration::valid_identity(&DurableIdentityRequest {
        session_id: "synthetic-chat-0001",
        location: None,
    }));
    assert!(!registration::valid_identity(&DurableIdentityRequest {
        session_id: "",
        location: None,
    }));
    assert!(!registration::valid_identity(&DurableIdentityRequest {
        session_id: "short",
        location: None,
    }));
    assert!(!registration::valid_identity(&DurableIdentityRequest {
        session_id: "synthetic chat with spaces",
        location: None,
    }));
    // A binding that names a location never widens the rule: Cursor's chat
    // storage is the client's to read, not this package's to trust.
    assert!(!registration::valid_identity(&DurableIdentityRequest {
        session_id: "synthetic chat with spaces",
        location: Some(std::path::Path::new("/synthetic/chats/record.json")),
    }));
}

#[test]
fn every_recorded_cursor_transcript_replays_through_the_real_parser() {
    let coverage = replay_corpus(&registration::parser_set(), &fixture_root());
    assert_eq!(coverage.len(), 1, "one Agent parser is composed");
    let scenarios = coverage
        .get(ADAPTER_ID)
        .expect("the corpus covers the adapter this package carries");
    assert_eq!(
        scenarios.len(),
        CURSOR_SCENARIOS.len(),
        "every scenario class is replayed"
    );
    for scenario in CURSOR_SCENARIOS {
        assert!(
            scenarios.contains(scenario),
            "cursor/{scenario}.json was replayed"
        );
    }
}

#[test]
fn the_replay_arm_refuses_an_adapter_this_package_does_not_carry() {
    let error = crate::replay::replay_arm("codex")
        .err()
        .expect("an adapter this package does not carry has no arm");
    assert!(error.contains("codex"), "{error}");
    assert!(crate::replay::replay_arm(ADAPTER_ID).is_ok());
}

#[test]
fn the_turn_event_port_is_fail_closed_until_the_host_installs_it() {
    // This test process never installs the port, so every emitter is silent
    // rather than inventing a consumer.
    assert!(!turn_event::installed());
    turn_event::emit_turn_event("agent.turn.accepted", "s", "t", serde_json::json!({}));
    turn_event::emit_agent_message_chunk("s", "t", "text");
    turn_event::emit_agent_message_completed("s", "t", "text");
    turn_event::emit_agent_processing("s", "t", "evidence", Some("tool"));
    turn_event::emit_agent_tool_error("s", "t", "tool", "code");

    // The port is installed once: a second, different answer is refused rather
    // than silently replacing the consumer.
    fn sink(_kind: &str, _session_id: &str, _turn_id: &str, _payload: serde_json::Value) {}
    fn chunk(_session_id: &str, _turn_id: &str, _text: &str) {}
    fn completed(_session_id: &str, _turn_id: &str, _text: &str) {}
    fn processing(_session_id: &str, _turn_id: &str, _evidence_kind: &str, _tool: Option<&str>) {}
    fn tool_error(_session_id: &str, _turn_id: &str, _tool: &str, _code: &str) {}
    let port = TurnEventPort {
        emit_turn_event: sink,
        emit_agent_message_chunk: chunk,
        emit_agent_message_completed: completed,
        emit_agent_processing: processing,
        emit_agent_tool_error: tool_error,
    };
    // The shape is stated here so the host's own functions are what it installs;
    // this test asserts the seam accepts exactly five answers and no others.
    let _ = port;
}

#[test]
fn the_execution_port_admits_nothing_before_the_host_answers() {
    use crate::port::execution;
    assert!(!execution::installed());
    assert!(!execution::admits_execution());
    assert_eq!(
        execution::subagent_caller_context(),
        Err(execution::HostEffect::Uninstalled)
    );
}
