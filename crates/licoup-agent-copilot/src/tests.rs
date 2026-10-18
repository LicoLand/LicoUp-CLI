//! This package's own claims about the Copilot adapter it carries.
//!
//! A claim that names Copilot belongs here rather than in the SDK, in the
//! shared ACP engine or in the client: which adapter id this package registers,
//! what its declaration says, which frames its dialect answers with, what
//! Copilot's launch declaration is, and — the claim that matters most — that the
//! five recorded transcripts still replay through the real parser and the shared
//! reducer.

use licoup_agent_adapter_sdk::replay::{SCENARIOS, fixture_root, replay_corpus};
use serde_json::json;

use crate::dialect;
use crate::driver::{DRIVER, DRIVER_ID, LAUNCH_ARGS, RUNTIME_PROTOCOL};
use crate::registration::{self, ADAPTER_ID, CONTRACT, DIALECT, FRAMING};

/// The five recorded transcripts, in the order the harness checks them.
const COPILOT_SCENARIOS: [&str; 5] = SCENARIOS;

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
    assert_eq!(set.contract("kimi-code"), None);
    assert_eq!(
        set.framing("kimi-code"),
        Err("no registered contract for adapter kimi-code".to_owned())
    );
}

#[test]
fn the_copilot_declaration_reports_the_complete_l4_signal_set() {
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

    // The dialect is keyed by the driver identity the transport pools and
    // cancels Copilot sessions by, so the composition can install it, and the
    // framing the corpus records is the framing the contract declares.
    assert_eq!(DIALECT.driver_id, DRIVER_ID);
    assert_eq!(DRIVER.runtime_protocol, RUNTIME_PROTOCOL);
    assert_eq!(DRIVER.agent_id, DRIVER_ID);
    assert_eq!(FRAMING, "lf-ndjson-acp");
}

#[test]
fn copilot_launch_arguments_are_fixed_and_private_values_use_acp_stdin() {
    assert_eq!(LAUNCH_ARGS, &["--acp", "--stdio", "--no-auto-update"]);
    assert_eq!(DRIVER.launch_args, LAUNCH_ARGS);
    assert!(
        !DRIVER
            .launch_args
            .iter()
            .any(|arg| *arg == "private-prompt" || *arg == concat!("/", "private", "/workspace")),
        "no private value is carried on a command line"
    );
}

#[test]
fn the_dialect_projects_this_agents_own_client_request() {
    let message = json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "session/request_permission",
        "params": {
            "sessionId": "synthetic-session",
            "options": [{"kind": "allow_once", "optionId": "once"}],
        },
    });
    let projected = (DIALECT.client_request)(&message).expect("a client request frame");
    assert_eq!(projected.id, json!(7));
    assert_eq!(projected.method, "session/request_permission");
    assert_eq!(projected.session_id.as_deref(), Some("synthetic-session"));
    assert_eq!(projected.allow_once_option.as_deref(), Some("once"));

    // A response is not a client request, and this profile never asks a
    // permission question: neither answer is invented.
    assert_eq!(
        (DIALECT.client_request)(&json!({"jsonrpc": "2.0", "id": 7, "result": {}})),
        None
    );
    assert_eq!((DIALECT.permission_request)(&message), None);
    assert!((DIALECT.response_is_error)(&json!({
        "jsonrpc": "2.0",
        "id": 7,
        "error": {"code": -32601, "message": "method not found"},
    })));
    assert!(!(DIALECT.response_is_error)(&json!({
        "jsonrpc": "2.0",
        "id": 7,
        "result": {},
    })));
}

#[test]
fn a_completed_execution_becomes_the_reply_transitions_of_this_agent() {
    let completed = crate::parser::completed_transitions("answer");
    assert_eq!(
        completed,
        vec![
            licoup_agent_adapter_sdk::Transition::Lifecycle(
                licoup_agent_adapter_sdk::LifecycleStage::Submitted
            ),
            licoup_agent_adapter_sdk::Transition::Lifecycle(
                licoup_agent_adapter_sdk::LifecycleStage::Accepted
            ),
            licoup_agent_adapter_sdk::Transition::Lifecycle(
                licoup_agent_adapter_sdk::LifecycleStage::Processing
            ),
            licoup_agent_adapter_sdk::Transition::Lifecycle(
                licoup_agent_adapter_sdk::LifecycleStage::Responding
            ),
            licoup_agent_adapter_sdk::Transition::Text {
                unit_id: "copilot:reply".to_owned(),
                text: "answer".to_owned(),
            },
            licoup_agent_adapter_sdk::Transition::Lifecycle(
                licoup_agent_adapter_sdk::LifecycleStage::Completed
            ),
        ]
    );

    let failed = crate::parser::failed_transitions(
        "acp_protocol_failed",
        "protocol/prompt",
        "Copilot ACP turn failed.",
    );
    let reported: Vec<&str> = failed
        .iter()
        .filter_map(|transition| match transition {
            licoup_agent_adapter_sdk::Transition::Failed { code, .. } => Some(code.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(reported, ["acp_protocol_failed"]);
}

#[test]
fn every_recorded_copilot_transcript_replays_through_the_real_parser() {
    let coverage = replay_corpus(&registration::parser_set(), &fixture_root());
    assert_eq!(coverage.len(), 1, "one Agent parser is composed");
    let scenarios = coverage
        .get(ADAPTER_ID)
        .expect("the corpus covers the adapter this package carries");
    assert_eq!(
        scenarios.len(),
        COPILOT_SCENARIOS.len(),
        "every scenario class is replayed"
    );
    for scenario in COPILOT_SCENARIOS {
        assert!(
            scenarios.contains(scenario),
            "copilot/{scenario}.json was replayed"
        );
    }
}

#[test]
fn the_replay_arm_refuses_an_adapter_this_package_does_not_carry() {
    let error = crate::replay::replay_arm("kimi-code")
        .err()
        .expect("an adapter this package does not carry has no arm");
    assert!(error.contains("kimi-code"), "{error}");
    assert!(crate::replay::replay_arm(ADAPTER_ID).is_ok());
}

#[test]
fn the_package_answers_the_sdk_queries_fail_closed_rather_than_borrowing() {
    let set = registration::parser_set();
    let outcome = licoup_agent_adapter_sdk::port::ExecutionOutcome {
        output: "answer",
        failure: None,
    };
    // Copilot's driver carries its own transition list, so the shared query
    // stays declared and unanswered: the empty answer is the honest one.
    assert_eq!(
        set.execution_transitions(ADAPTER_ID, &outcome),
        Some(Vec::new())
    );
    assert_eq!(
        set.valid_identity(
            ADAPTER_ID,
            &licoup_agent_adapter_sdk::port::DurableIdentityRequest {
                session_id: "synthetic-session",
                location: None,
            },
        ),
        Some(false)
    );
    assert_eq!(dialect::no_permission_request(&json!({})), None);
}
