//! This package's own claims about the Claude Code adapter it carries.
//!
//! A claim that names Claude Code belongs here rather than in the SDK or in the
//! client: which adapter id this package registers, what its declaration says,
//! how one execution outcome becomes the shared transition vocabulary, which
//! durable identity it accepts, that the two ports it declares are fail-closed
//! until a host answers them, and — the claim that matters most — that the five
//! recorded transcripts still replay through the real parser.

use licoup_agent_adapter_sdk::port::{
    DurableIdentityRequest, ExecutionFailure, ExecutionOutcome, ExecutionTransitions,
};
use licoup_agent_adapter_sdk::replay::{SCENARIOS, fixture_root, replay_corpus};
use licoup_agent_adapter_sdk::{LifecycleStage, Transition};

use crate::port::execution;
use crate::registration::{self, ADAPTER_ID, CONTRACT, FRAMING};

/// The five recorded transcripts, in the order the harness checks them.
const CLAUDE_CODE_SCENARIOS: [&str; 5] = SCENARIOS;

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
fn the_claude_code_declaration_reports_the_complete_l4_signal_set() {
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
    assert_eq!(contract.inventory_json()["framing"], "lf-ndjson");
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
                unit_id: "claude-code:reply".to_owned(),
                text: "answer".to_owned(),
            },
            Transition::Lifecycle(LifecycleStage::Completed),
        ]
    );

    // An empty reply is a completion with no text rather than a synthesized one.
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
    let failed = transitions(&ExecutionOutcome {
        output: "",
        failure: Some(ExecutionFailure {
            code: "claude_code_turn_failed",
            stage: "turn/completed",
            message: "Claude Code reported that the requested turn failed.",
        }),
    });
    let reported: Vec<&str> = failed
        .iter()
        .filter_map(|transition| match transition {
            Transition::Failed { code, .. } => Some(code.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(reported, ["claude_code_turn_failed"]);
    match failed.last() {
        Some(Transition::Failed {
            code,
            stage,
            message,
        }) => {
            assert_eq!(code, "claude_code_turn_failed");
            assert_eq!(stage, "turn/completed");
            assert!(message.contains("requested turn failed"));
        }
        other => panic!("the failure transition ends the walk, found {other:?}"),
    }
}

#[test]
fn a_durable_identity_is_judged_on_the_identity_the_cli_reported() {
    let valid = registration::valid_identity;

    // The CLI persists its own transcript and reports the identity on the
    // stream this package parses, so there is no client-readable record: the
    // identity is judged on its own shape.
    assert!(valid(&DurableIdentityRequest {
        session_id: "a-real-native-conversation",
        location: None,
    }));
    assert!(!valid(&DurableIdentityRequest {
        session_id: "",
        location: None,
    }));
    assert!(!valid(&DurableIdentityRequest {
        session_id: "carries\u{7}a-control-character",
        location: None,
    }));
    let bounded = "a".repeat(512);
    assert!(valid(&DurableIdentityRequest {
        session_id: &bounded,
        location: None,
    }));
    let over_bounded = "a".repeat(513);
    assert!(!valid(&DurableIdentityRequest {
        session_id: &over_bounded,
        location: None,
    }));

    // A recorded location is a locator and is never read as proof: the answer
    // stays the identity's own shape rather than a claim about a file.
    let location = std::path::Path::new("/tmp/synthetic/claude-code-session.jsonl");
    assert!(valid(&DurableIdentityRequest {
        session_id: "a-real-native-conversation",
        location: Some(location),
    }));
    assert!(!valid(&DurableIdentityRequest {
        session_id: "",
        location: Some(location),
    }));
}

#[test]
fn every_recorded_claude_code_transcript_replays_through_the_real_parser() {
    let coverage = replay_corpus(&registration::parser_set(), &fixture_root());
    assert_eq!(coverage.len(), 1, "one Agent parser is composed");
    let scenarios = coverage
        .get(ADAPTER_ID)
        .expect("the corpus covers the adapter this package carries");
    assert_eq!(
        scenarios.len(),
        CLAUDE_CODE_SCENARIOS.len(),
        "every scenario class is replayed"
    );
    for scenario in CLAUDE_CODE_SCENARIOS {
        assert!(
            scenarios.contains(scenario),
            "claude-code/{scenario}.json was replayed"
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
fn the_execution_port_admits_nothing_before_the_host_answers() {
    // A package started outside its host cannot invent an admission or a
    // caller context: both answers are fail-closed until the host installs one.
    if execution::installed() {
        // A host that installed its answers states them; this assertion only
        // runs in a process that has none.
        return;
    }
    assert_eq!(
        execution::subagent_caller_context(),
        Err(execution::HostEffect::Uninstalled)
    );
    assert!(!execution::admits_execution());
}

#[test]
fn the_declared_package_documents_are_the_ones_this_crate_publishes() {
    // The documents the release tool packages are read here as data, so a
    // renaming cannot leave the manifest describing another program.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("package");
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("manifest.json")).expect("the manifest is readable"),
    )
    .expect("the manifest is JSON");
    assert_eq!(manifest["id"], "org.licoland.adapter.claude-code");
    assert_eq!(manifest["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(manifest["runtime"]["entry"], "bin/lico-agent-claude-code");
    assert_eq!(manifest["runtime"]["mode"], "process");
    assert!(
        manifest["runtime"].get("runtimeRef").is_none(),
        "the declared runtime is the program itself, not a reference"
    );

    let release: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("package-release.json"))
            .expect("the release declaration is readable"),
    )
    .expect("the release declaration is JSON");
    assert_eq!(release["packageId"], manifest["id"]);
    assert_eq!(release["packageVersion"], manifest["version"]);
    assert_eq!(release["converter"]["entry"], manifest["runtime"]["entry"]);
    assert_eq!(
        release["converter"]["sourceFormat"],
        crate::protocol::PROTOCOL_FORMAT
    );
    assert_eq!(
        release["converter"]["targetFormat"],
        "licoup.conversation.v1"
    );
    assert_eq!(
        release["clientCompatibility"]["range"],
        manifest["compatibility"]["clientVersions"][0]
    );
}
