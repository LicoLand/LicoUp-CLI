//! This package's own claims about the Kilo Code adapter it carries.
//!
//! A claim that names Kilo Code belongs here rather than in the SDK or in the
//! client: which adapter id this package registers, what its declaration says,
//! how one execution outcome becomes the shared transition vocabulary, which
//! durable identity it accepts, what its endpoint contract declares, and — the
//! claim that matters most — that the five recorded transcripts still replay
//! through the real parser.

use licoup_agent_adapter_sdk::port::{
    DurableIdentityRequest, ExecutionFailure, ExecutionOutcome, ExecutionTransitions,
};
use licoup_agent_adapter_sdk::replay::{SCENARIOS, fixture_root, replay_corpus};
use licoup_agent_adapter_sdk::{LifecycleStage, Transition};

use crate::parser;
use crate::policy;
use crate::port::turn_event::{self, TurnEventPort};
use crate::registration::{self, ADAPTER_ID, CONTRACT};

/// The five recorded transcripts, in the order the harness checks them.
const KILO_SCENARIOS: [&str; 5] = SCENARIOS;

#[test]
fn the_package_registers_exactly_one_adapter_through_the_sdk_port() {
    let set = registration::parser_set();
    assert_eq!(set.registered_ids(), [ADAPTER_ID]);
    assert_eq!(set.all().len(), 1);

    let contract = set
        .contract(ADAPTER_ID)
        .expect("the composed set answers for the adapter it carries");
    assert_eq!(contract, CONTRACT);
    assert_eq!(contract.id, "kilo-code");
    assert_eq!(contract.framing, "http-sse");
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
fn the_kilo_declaration_reports_the_complete_l4_signal_set() {
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
}

#[test]
fn both_registration_queries_are_answered_rather_than_fail_closed() {
    let set = registration::parser_set();
    let transitions: ExecutionTransitions = registration::execution_transitions;
    let completed = set
        .execution_transitions(
            ADAPTER_ID,
            &ExecutionOutcome {
                output: "answer",
                failure: None,
            },
        )
        .expect("the composed set answers for the adapter it carries");
    assert_eq!(
        completed,
        transitions(&ExecutionOutcome {
            output: "answer",
            failure: None,
        })
    );
    assert!(matches!(
        completed.last(),
        Some(Transition::Lifecycle(LifecycleStage::Completed))
    ));

    let failed = set
        .execution_transitions(
            ADAPTER_ID,
            &ExecutionOutcome {
                output: "",
                failure: Some(ExecutionFailure {
                    code: "kilo_code_serve_sse_closed",
                    stage: "serve/sse",
                    message: "safe",
                }),
            },
        )
        .unwrap();
    assert!(matches!(
        failed.last(),
        Some(Transition::Failed { code, .. }) if code == "kilo_code_serve_sse_closed"
    ));

    // The identity query is answered from the Agent's own rule, not refused.
    assert_eq!(
        set.valid_identity(
            ADAPTER_ID,
            &DurableIdentityRequest {
                session_id: "kilo-1",
                location: None,
            }
        ),
        Some(true)
    );
}

#[test]
fn the_five_recorded_transcripts_replay_through_the_real_parser() {
    assert_eq!(KILO_SCENARIOS, SCENARIOS);
    assert_eq!(
        fixture_root().join("kilo-code").is_dir(),
        true,
        "the kilo-code corpus must exist for its own claims to be checkable"
    );
    let coverage = replay_corpus(&registration::parser_set(), &fixture_root());
    let scenarios = coverage
        .get(ADAPTER_ID)
        .expect("the corpus covers the adapter this package carries");
    assert_eq!(
        scenarios.len(),
        KILO_SCENARIOS.len(),
        "every recorded scenario must replay: replayed {scenarios:?}"
    );
    for scenario in KILO_SCENARIOS {
        assert!(
            scenarios.contains(scenario),
            "{scenario} must replay through this package's parser"
        );
    }
}

#[test]
fn the_endpoint_contract_is_this_agents_own_and_names_no_other() {
    let spec = policy::SPEC;
    assert_eq!(spec.identity, "kilo_code_serve");
    assert_eq!(spec.default_executable, "kilo");
    assert_eq!(
        spec.executable_environment,
        &["KILO_BIN", "KILO_PATH", "KILOCODE_PATH"]
    );
    // The Agent's own paths, not another serve-family Agent's.
    assert_eq!(spec.health_path, "/global/health");
    assert_eq!(spec.session_probe_path, "/session");
    assert_ne!(spec.identity, "opencode_serve");
    // Every failure code the engine may report is namespaced to this Agent, so
    // a reader never has to guess which Agent failed.
    for code in [
        spec.errors.executable_missing,
        spec.errors.port_exhausted,
        spec.errors.start_failed,
        spec.errors.health_failed,
        spec.errors.attach_probe_failed,
        spec.errors.not_found,
        spec.errors.request_failed,
        spec.errors.invalid_json,
        spec.errors.invalid_state,
        spec.errors.stop_failed,
    ] {
        assert!(
            code.starts_with("kilo"),
            "{code} must name the Agent it belongs to"
        );
    }
}

#[test]
fn the_endpoint_url_and_the_route_shapes_are_one_contract() {
    assert_eq!(
        policy::endpoint_url("http://127.0.0.1:4097", policy::SPEC.health_path),
        "http://127.0.0.1:4097/global/health"
    );
    // The readiness reader and the turn bind the same field spellings, so a
    // document that satisfies one cannot be unreadable to the other.
    let identity = serde_json::json!({"sessionID": "kilo-1"});
    assert_eq!(parser::session_id(&identity), Some("kilo-1"));
    assert_eq!(
        parser::session_id(&serde_json::json!({"id": "kilo-1"})),
        Some("kilo-1")
    );
    assert_eq!(parser::session_id(&serde_json::json!({"id": ""})), None);
}

#[test]
fn a_turn_event_reaches_the_host_and_is_silent_without_one() {
    use serde_json::json;
    use std::sync::Mutex;

    // The port is installed once per process, so this test asserts the
    // fail-closed half first and then the answer half. A second installation is
    // refused, which is itself the claim that a process has one consumer.
    static EVENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());
    fn emit_turn_event(kind: &str, session_id: &str, turn_id: &str, payload: serde_json::Value) {
        EVENTS
            .lock()
            .unwrap()
            .push(format!("{kind}|{session_id}|{turn_id}|{payload}"));
    }
    fn emit_agent_message_chunk(session_id: &str, turn_id: &str, text: &str) {
        EVENTS
            .lock()
            .unwrap()
            .push(format!("chunk|{session_id}|{turn_id}|{text}"));
    }
    fn emit_agent_message_completed(session_id: &str, turn_id: &str, text: &str) {
        EVENTS
            .lock()
            .unwrap()
            .push(format!("done|{session_id}|{turn_id}|{text}"));
    }

    assert!(!turn_event::installed());
    turn_event::emit_agent_message_chunk("kilo-1", "turn-1", "before");
    assert!(EVENTS.lock().unwrap().is_empty());

    turn_event::install(TurnEventPort {
        emit_turn_event,
        emit_agent_message_chunk,
        emit_agent_message_completed,
    })
    .unwrap();
    assert!(turn_event::installed());
    assert!(
        turn_event::install(TurnEventPort {
            emit_turn_event,
            emit_agent_message_chunk,
            emit_agent_message_completed,
        })
        .is_err(),
        "a second consumer for one process is refused"
    );

    turn_event::emit_turn_event("dispatch.turn.bound", "kilo-1", "turn-1", json!({}));
    turn_event::emit_agent_message_chunk("kilo-1", "turn-1", "delta");
    turn_event::emit_agent_message_completed("kilo-1", "turn-1", "answer");
    let events = EVENTS.lock().unwrap().clone();
    assert_eq!(events.len(), 3);
    assert!(events[0].starts_with("dispatch.turn.bound|kilo-1|turn-1|"));
    assert_eq!(events[1], "chunk|kilo-1|turn-1|delta");
    assert_eq!(events[2], "done|kilo-1|turn-1|answer");
}
