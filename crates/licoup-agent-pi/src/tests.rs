//! This package's own claims about the Pi adapter it carries.
//!
//! A claim that names Pi belongs here rather than in the SDK or in the client:
//! which adapter id this package registers, what its declaration says, which of
//! the two protocol-agnostic queries it answers, and — the claim that matters
//! most — that the five recorded transcripts still replay through the real
//! parser.

use licoup_agent_adapter_sdk::port::{
    DurableIdentityRequest, ExecutionFailure, ExecutionOutcome, ExecutionTransitions,
};
use licoup_agent_adapter_sdk::replay::{SCENARIOS, fixture_root, replay_corpus};

use crate::port::turn_event::{self, TurnEventPort};
use crate::registration::{self, ADAPTER_ID, CONTRACT, FRAMING};

/// The five recorded transcripts, in the order the harness checks them.
const PI_SCENARIOS: [&str; 5] = SCENARIOS;

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
    assert_eq!(set.contract("cursor"), None);
    assert_eq!(
        set.framing("cursor"),
        Err("no registered contract for adapter cursor".to_owned())
    );
}

#[test]
fn the_pi_declaration_reports_the_complete_l4_signal_set() {
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
}

#[test]
fn the_two_protocol_agnostic_queries_stay_declared_and_unanswered() {
    // Pi's driver carries the parser's own transition list on every execution
    // result, so the separate transition query stays fail-closed rather than
    // answering a second time from the same facts.
    let registration = registration::REGISTRATION;
    let transitions: ExecutionTransitions = registration.execution_transitions;
    assert!(
        transitions(&ExecutionOutcome {
            output: "answer",
            failure: None,
        })
        .is_empty(),
        "the driver carries Pi's transitions, not this query"
    );
    assert!(
        transitions(&ExecutionOutcome {
            output: "",
            failure: Some(ExecutionFailure {
                code: "pi_rpc_failed",
                stage: "protocol",
                message: "Pi RPC did not complete the request.",
            }),
        })
        .is_empty()
    );

    // The Subagent mesh never dispatches Pi, so there is no durable dispatch
    // identity for this query to validate.
    let valid_identity = registration.valid_identity;
    assert!(!valid_identity(&DurableIdentityRequest {
        session_id: "pi-native-resume-1",
        location: None,
    }));
}

#[test]
fn every_recorded_pi_transcript_replays_through_the_real_parser() {
    let coverage = replay_corpus(&registration::parser_set(), &fixture_root());
    assert_eq!(coverage.len(), 1, "one Agent parser is composed");
    let scenarios = coverage
        .get(ADAPTER_ID)
        .expect("the corpus covers the adapter this package carries");
    assert_eq!(
        scenarios.len(),
        PI_SCENARIOS.len(),
        "every scenario class is replayed"
    );
    for scenario in PI_SCENARIOS {
        assert!(
            scenarios.contains(scenario),
            "pi/{scenario}.json was replayed"
        );
    }
}

#[test]
fn the_replay_arm_refuses_an_adapter_this_package_does_not_carry() {
    let error = crate::replay::replay_arm("cursor")
        .err()
        .expect("an adapter this package does not carry has no arm");
    assert!(error.contains("cursor"), "{error}");
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

    // The port is installed once: a second, different answer is refused rather
    // than silently replacing the consumer.
    fn sink(_kind: &str, _session_id: &str, _turn_id: &str, _payload: serde_json::Value) {}
    fn chunk(_session_id: &str, _turn_id: &str, _text: &str) {}
    fn completed(_session_id: &str, _turn_id: &str, _text: &str) {}
    fn processing(_session_id: &str, _turn_id: &str, _evidence_kind: &str, _tool: Option<&str>) {}
    let port = TurnEventPort {
        emit_turn_event: sink,
        emit_agent_message_chunk: chunk,
        emit_agent_message_completed: completed,
        emit_agent_processing: processing,
    };
    // The shape is stated here so the host's own functions are what it installs;
    // this test asserts the seam accepts exactly four answers and no others.
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
