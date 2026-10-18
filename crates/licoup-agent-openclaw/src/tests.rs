//! This package's own claims about the OpenClaw adapter it carries.
//!
//! A claim that names OpenClaw belongs here rather than in the SDK or in the
//! client: which adapter id this package registers, what its declaration says,
//! which request shape it refuses and with which code, that the Gateway
//! endpoint pair is one fact, that the ports are fail-closed without a host, and
//! — the claim that matters most — that the five recorded transcripts still
//! replay through the real state machine.

use licoup_agent_adapter_sdk::Transition;
use licoup_agent_adapter_sdk::replay::{SCENARIOS, fixture_root, replay_corpus};
use serde_json::{Value, json};

use crate::gateway::GatewayEndpoint;
use crate::gateway_acp::errors::ProtocolFailure;
use crate::gateway_acp::params::ProtocolConfig;
use crate::parser::codec::{INITIALIZE_REQUEST_ID, PROMPT_REQUEST_ID, SESSION_REQUEST_ID};
use crate::parser::protocol::{OpenClawProtocol, ProtocolEffect, ProtocolPhase};
use crate::port::{execution, gateway, turn_event};
use crate::registration::{self, ADAPTER_ID, CONTRACT, FRAMING};

/// The five recorded transcripts, in the order the harness checks them.
const OPENCLAW_SCENARIOS: [&str; 5] = SCENARIOS;

fn config(params: Value, prompt: &str, session_id: &str) -> ProtocolConfig {
    // Protocol fixtures do not exercise installation-backed MCP registration.
    ProtocolConfig::from_params_without_local_mcp(
        &params,
        prompt,
        session_id,
        Some(std::path::Path::new("/workspace/synthetic-project")),
    )
    .expect("the synthetic request is accepted")
}

fn initialize(protocol: &mut OpenClawProtocol) -> Vec<ProtocolEffect> {
    protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": INITIALIZE_REQUEST_ID,
        "result": {
            "protocolVersion": licoup_foundation::core::acp::PROTOCOL_VERSION,
            "agentCapabilities": {"loadSession": true, "sessionCapabilities": {"resume": {}}},
            "agentInfo": {"name": "openclaw-acp", "version": "test"}
        }
    }))
}

fn sent_messages(effects: Vec<ProtocolEffect>) -> Vec<Value> {
    effects
        .into_iter()
        .filter_map(|effect| match effect {
            ProtocolEffect::Send(message) => Some(message),
            ProtocolEffect::Complete(_) | ProtocolEffect::Fail(_) => None,
        })
        .collect()
}

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
fn the_openclaw_declaration_reports_the_complete_l4_signal_set() {
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
fn both_protocol_agnostic_queries_stay_fail_closed_and_say_why() {
    let set = registration::parser_set();
    let outcome = licoup_agent_adapter_sdk::port::ExecutionOutcome {
        output: "answer",
        failure: None,
    };
    // The driver reports the parser's own transition list on its run result, so
    // the query is not answered a second time through the port.
    assert_eq!(
        set.execution_transitions(ADAPTER_ID, &outcome),
        Some(Vec::new())
    );
    // A resumable OpenClaw identity is a bound Gateway session key rather than a
    // location, so no validator is claimed for it.
    let request = licoup_agent_adapter_sdk::port::DurableIdentityRequest {
        session_id: "agent:main:acp:native-session",
        location: None,
    };
    assert_eq!(set.valid_identity(ADAPTER_ID, &request), Some(false));
}

#[test]
fn every_recorded_openclaw_transcript_replays_through_the_real_state_machine() {
    let coverage = replay_corpus(&registration::parser_set(), &fixture_root());
    let replayed = coverage
        .get(ADAPTER_ID)
        .expect("the openclaw corpus was replayed");
    assert_eq!(
        replayed.len(),
        OPENCLAW_SCENARIOS.len(),
        "every scenario class is replayed"
    );
    for scenario in OPENCLAW_SCENARIOS {
        assert!(
            replayed.contains(scenario),
            "openclaw/{scenario}.json was replayed"
        );
    }
    // The corpus covers exactly this package's adapter and no other.
    assert_eq!(coverage.keys().collect::<Vec<_>>(), [ADAPTER_ID]);
}

#[test]
fn the_replay_arm_refuses_an_adapter_this_package_does_not_carry() {
    let Err(error) = crate::replay::replay_arm("codex") else {
        panic!("another Agent's corpus must not replay here")
    };
    assert!(error.contains("codex"), "unexpected refusal: {error}");
    assert!(crate::replay::replay_arm(ADAPTER_ID).is_ok());
}

#[test]
fn a_frame_is_classified_once_and_private_metadata_never_leaves_the_parser() {
    // A fresh OpenClaw send must name the runtime Agent the Gateway will bind a
    // conversation key to; without one this Agent refuses to prompt at all, as
    // the test above states.
    let mut protocol = OpenClawProtocol::new(config(
        json!({"openclawAgentId": "ops"}),
        "private prompt",
        "",
    ));
    initialize(&mut protocol);

    // The Gateway session key arrives on the opening update, before the session
    // response: it is captured by the binding the parser owns and is never
    // reported as an effect of its own.
    let private_key = ["private", "session", "key"].join("-");
    let opening = protocol.handle_frame(
        format!(
            r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"protocol-session","update":{{"sessionUpdate":"session_info_update","_meta":{{"sessionKey":"{private_key}"}}}}}}}}"#
        )
        .as_bytes(),
    );
    assert!(
        opening.effects.is_empty(),
        "a session binding is not an observable effect"
    );
    assert_eq!(
        protocol.binding.native_id(),
        Some(private_key.as_str()),
        "the binding is captured exactly once, by the parser that owns it"
    );

    let prompt = sent_messages(protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": SESSION_REQUEST_ID,
        "result": {
            "sessionId": "protocol-session",
            "modes": {"currentModeId": "medium", "availableModes": []}
        }
    })));
    assert_eq!(prompt[0]["method"], "session/prompt");

    // Visible content and provider metadata arrive on the same frame: only the
    // content survives. The `_meta` body is a different private value from the
    // Gateway key, so "the key is the resumable identity" and "provider
    // metadata is never projected" stay two separate claims.
    let private_metadata = ["not", "for", "projection"].join("-");
    let content = protocol.handle_frame(
        format!(
            r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"protocol-session","update":{{"sessionUpdate":"agent_message_chunk","content":{{"type":"text","text":"visible"}},"_meta":{{"secret":"{private_metadata}"}}}}}}}}"#
        )
        .as_bytes(),
    );
    assert!(content.effects.is_empty(), "a text chunk is not a turn end");
    assert_eq!(protocol.output, "visible");

    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": PROMPT_REQUEST_ID,
        "result": {"stopReason": "end_turn"}
    }));
    assert!(!effects.is_empty(), "a finished prompt reports an effect");
    let ProtocolEffect::Complete(outcome) = &effects[0] else {
        panic!("a finished prompt completes the turn")
    };
    assert_eq!(outcome.output, "visible");
    // The reported session identity is the *resumable* Gateway key, which is
    // what the client stores to resume this conversation — not the
    // process-local ACP protocol id, and not any part of the update's metadata.
    assert_eq!(outcome.session_id, private_key);
    let projected = json!({
        "output": outcome.output,
        "sessionId": outcome.session_id,
        "turnStatus": outcome.turn_status,
        "effective": format!("{:?}", outcome.effective),
    });
    assert!(
        !projected.to_string().contains(&private_metadata),
        "provider metadata is never projected: {projected}"
    );
}

#[test]
fn a_malformed_request_is_refused_with_its_own_code_before_the_host_is_asked() {
    let mut asked = false;
    let failure = ProtocolConfig::from_params(
        &json!({"privateInstructions": "private-system-canary"}),
        "exact-user-prompt",
        "",
        Some(std::path::Path::new("/workspace/synthetic-project")),
        || {
            asked = true;
            Ok(Vec::new())
        },
    )
    .expect_err("a private instruction channel is not supported");
    assert_eq!(
        failure.code,
        "openclaw_acp_private_instructions_unsupported"
    );
    assert!(!failure.message.contains("canary"));
    assert!(!failure.message.contains("exact-user-prompt"));
    assert!(
        !asked,
        "a rejected request never consults the client's MCP registration"
    );
}

#[test]
fn the_gateway_endpoint_pair_is_one_spelling_of_one_service() {
    let endpoint = GatewayEndpoint::new("127.0.0.1", 24189);
    assert_eq!(endpoint.attach_url, "http://127.0.0.1:24189");
    assert_eq!(endpoint.ws_url, "ws://127.0.0.1:24189");
    assert_eq!(endpoint.host, "127.0.0.1");
    assert_eq!(endpoint.port, 24189);
}

#[test]
fn every_port_is_fail_closed_before_the_host_installs_it() {
    // These ports are process-wide and a host installs them once, so this test
    // states the uninstalled answer rather than racing an installation: the
    // package must never invent an emission, an admission or an endpoint.
    assert!(!turn_event::installed());
    assert!(!execution::installed());
    assert!(!gateway::installed());
    assert!(
        !execution::admits_execution(),
        "a package that cannot ask the host's admission never claims it was admitted"
    );
    // The Gateway answer is the engine's own "unavailable" code rather than a
    // new package code: a package that cannot ask reports the same refusal the
    // transport has always reported for a Gateway nobody could ensure.
    assert_eq!(
        gateway::ensure_attach_endpoint("openclaw").unwrap_err(),
        "openclaw_gateway_unavailable"
    );
    // An emission with no installed port is a no-op rather than a panic or a
    // second sink.
    turn_event::emit_turn_event("dispatch.turn.bound", "session", "turn", json!({}));
    turn_event::emit_agent_message_chunk("session", "turn", "text");
    turn_event::emit_agent_processing("session", "turn", "reasoning", None);
}

#[test]
fn the_reported_transitions_are_the_shared_vocabulary_of_this_agent() {
    let completed = crate::parser::completed_transitions("answer");
    assert!(matches!(
        completed.last(),
        Some(Transition::Lifecycle(
            licoup_agent_adapter_sdk::LifecycleStage::Completed
        ))
    ));
    let failed = crate::parser::failed_transitions("openclaw_acp_probe_failed", "probe", "no");
    assert!(matches!(failed.last(), Some(Transition::Failed { .. })));
}

#[test]
fn a_protocol_failure_carries_its_stage_and_its_required_interaction() {
    let failure = ProtocolFailure::user_interaction(
        "session/request_permission",
        Some("protocol-session"),
        Some("turn-1"),
    );
    assert_eq!(failure.code, "openclaw_user_interaction_required");
    assert!(failure.user_interaction_required);
    assert_eq!(
        failure.request_method.as_deref(),
        Some("session/request_permission")
    );
    let payload = failure.into_payload();
    assert_eq!(payload.stage, "server/request");
    assert_eq!(payload.session_id.as_deref(), Some("protocol-session"));
    assert_eq!(payload.turn_id.as_deref(), Some("turn-1"));
    assert_eq!(payload.turn_status, None);
}

#[test]
fn a_new_session_requires_a_gateway_key_before_it_may_prompt() {
    let mut protocol = OpenClawProtocol::new(config(json!({}), "hello", ""));
    initialize(&mut protocol);
    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": SESSION_REQUEST_ID,
        "result": {"sessionId": "process-local-session"}
    }));
    let ProtocolEffect::Fail(failure) = &effects[0] else {
        panic!("a process-local ACP identity must not be resumable")
    };
    assert_eq!(failure.code, "openclaw_acp_native_session_id_missing");
    assert_eq!(protocol.phase, ProtocolPhase::Finished);
}

#[test]
fn a_private_prompt_never_reaches_the_launch_arguments_or_the_session_request() {
    let mut protocol = OpenClawProtocol::new(config(
        json!({"openclawAgentId": "ops"}),
        "private-openclaw-prompt",
        "",
    ));
    let initial = protocol.initial_request().expect("the handshake opens");
    assert!(!initial.to_string().contains("private-openclaw-prompt"));
    let requests = sent_messages(initialize(&mut protocol));
    assert_eq!(requests[0]["method"], "session/new");
    assert!(!requests[0].to_string().contains("private-openclaw-prompt"));
}
