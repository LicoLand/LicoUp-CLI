//! This package's own claims about the Codex adapter it carries.
//!
//! A claim that names Codex belongs here rather than in the SDK or in the
//! client: which adapter id this package registers, what its declaration says,
//! how one execution outcome becomes the shared transition vocabulary, which
//! durable identity it accepts, and — the claim that matters most — that the
//! five recorded transcripts still replay through the real parser.

use std::io::Write;

use licoup_agent_adapter_sdk::port::{
    DurableIdentityRequest, ExecutionFailure, ExecutionOutcome, ExecutionTransitions,
};
use licoup_agent_adapter_sdk::replay::{SCENARIOS, fixture_root, replay_corpus};
use licoup_agent_adapter_sdk::{LifecycleStage, Transition};

use crate::port::turn_event::{self, TurnEventPort};
use crate::registration::{self, ADAPTER_ID, CONTRACT, FRAMING};

/// The five recorded transcripts, in the order the harness checks them.
const CODEX_SCENARIOS: [&str; 5] = SCENARIOS;

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
fn the_codex_declaration_reports_the_complete_l4_signal_set() {
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
                unit_id: "codex:reply".to_owned(),
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
            code: "codex_app_server_start_failed",
            stage: "protocol/start",
            message: "Codex app-server could not be started.",
        }),
    });
    let reported: Vec<&str> = failure
        .iter()
        .filter_map(|transition| match transition {
            Transition::Failed { code, .. } => Some(code.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(reported, ["codex_app_server_start_failed"]);
}

#[test]
fn a_durable_identity_is_decided_by_the_rollout_record_it_names() {
    let directory =
        std::env::temp_dir().join(format!("lico-codex-identity-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("a disposable directory");
    let rollout = directory.join("rollout-2026-01-01T00-00-00-synthetic.jsonl");
    let mut file = std::fs::File::create(&rollout).expect("a synthetic rollout record");
    writeln!(
        file,
        r#"{{"type":"session_meta","payload":{{"id":"synthetic-session"}}}}"#
    )
    .expect("the metadata line is written");
    drop(file);

    let valid = registration::valid_identity(&DurableIdentityRequest {
        session_id: "synthetic-session",
        location: Some(rollout.as_path()),
    });
    assert!(valid, "the record itself names this identity");

    // The file name is a locator and never authorizes a resume.
    let mismatched = registration::valid_identity(&DurableIdentityRequest {
        session_id: "rollout-2026-01-01T00-00-00-synthetic",
        location: Some(rollout.as_path()),
    });
    assert!(!mismatched);

    let absent = registration::valid_identity(&DurableIdentityRequest {
        session_id: "synthetic-session",
        location: Some(directory.join("absent.jsonl").as_path()),
    });
    assert!(!absent, "an unreadable record is not a valid identity");

    // No location: there is nothing to read, so the identity is judged on its
    // own shape exactly as this host's exact-identity resolution judges it.
    assert!(registration::valid_identity(&DurableIdentityRequest {
        session_id: "synthetic-session",
        location: None,
    }));
    assert!(!registration::valid_identity(&DurableIdentityRequest {
        session_id: "",
        location: None,
    }));
    assert!(!registration::valid_identity(&DurableIdentityRequest {
        session_id: "control\u{7}character",
        location: None,
    }));

    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn every_recorded_codex_transcript_replays_through_the_real_parser() {
    let coverage = replay_corpus(&registration::parser_set(), &fixture_root());
    assert_eq!(coverage.len(), 1, "one Agent parser is composed");
    let scenarios = coverage
        .get(ADAPTER_ID)
        .expect("the corpus covers the adapter this package carries");
    assert_eq!(
        scenarios.len(),
        CODEX_SCENARIOS.len(),
        "every scenario class is replayed"
    );
    for scenario in CODEX_SCENARIOS {
        assert!(
            scenarios.contains(scenario),
            "codex/{scenario}.json was replayed"
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
    turn_event::emit_agent_tool_error("s", "t", "tool", "code");

    // The port answers one question — where an event goes — because what an
    // event is belongs to this package. The host installs its own emitter; this
    // test asserts the seam has that one answer and no second one.
    fn sink(_kind: &str, _session_id: &str, _turn_id: &str, _payload: serde_json::Value) {}
    let port = TurnEventPort { emit: sink };
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
