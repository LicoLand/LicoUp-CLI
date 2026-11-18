//! The package's own claims.
//!
//! A claim that names OpenCode — which `serve` frame it classifies, which part
//! becomes the Agent's reply, what a completed or failed turn reduces to, what
//! readiness the endpoint reports — belongs here, beside the parser that makes
//! it. The claims that name no Agent live in `licoup-agent-adapter-sdk`'s own
//! test module, and the recorded-transcript parity claim lives in the package's
//! own `tests/replay_corpus.rs`, where the SDK's harness drives it.

use licoup_agent_adapter_sdk::adapters::AdapterContract;
use licoup_agent_adapter_sdk::{LifecycleStage, Transition};
use serde_json::json;

use crate::parser;
use crate::port::execution;
use crate::registration;

/// The declaration this package reports is the one its composition injects.
#[test]
fn the_package_declares_one_adapter_and_reports_it_through_the_sdk() {
    assert_eq!(
        registration::CONTRACT,
        AdapterContract::new("opencode", "http-sse")
    );
    assert_eq!(registration::ADAPTER_ID, "opencode");
    assert_eq!(registration::FRAMING, "http-sse");
    assert_eq!(registration::PROTOCOL_FORMAT, "opencode.serve-http.v1");
    assert_eq!(parser::CONTRACT.id, registration::ADAPTER_ID);
    assert_eq!(parser::CONTRACT.framing, registration::FRAMING);
    assert_eq!(registration::registrations().len(), 1);
    assert_eq!(registration::contract(), Some(registration::CONTRACT));
    let set = registration::parser_set();
    assert_eq!(set.registered_ids(), ["opencode"]);
    assert_eq!(set.contract("opencode"), Some(registration::CONTRACT));
    assert!(set.contract("codex").is_none());
    // The transitions one OpenCode execution reports travel with the parser's
    // own result, so the shared query stays declared and unanswered rather than
    // answering a second, invented list.
    assert_eq!(
        registration::parser_set().execution_transitions(
            "opencode",
            &licoup_agent_adapter_sdk::port::ExecutionOutcome {
                output: "answer",
                failure: None,
            },
        ),
        Some(Vec::new())
    );
    assert_eq!(
        registration::parser_set().execution_transitions(
            "codex",
            &licoup_agent_adapter_sdk::port::ExecutionOutcome {
                output: "answer",
                failure: None,
            },
        ),
        None,
        "a program that composes no Codex parser answers no Codex question"
    );
}

/// The three `serve` document kinds decode only here, and only through this
/// package's own entry points.
#[test]
fn serve_frames_decode_only_inside_the_opencode_package() {
    assert!(parser::health_ready(&json!({"healthy": true})));
    assert!(!parser::health_ready(&json!({"healthy": false})));
    assert_eq!(parser::session_id(&json!({"id": "open-1"})), Some("open-1"));
    assert_eq!(
        parser::session_id(&json!({"sessionID": "open-2"})),
        Some("open-2")
    );
    assert_eq!(parser::session_id(&json!({"id": ""})), None);
    assert!(parser::session_collection(&json!([])));
    assert!(!parser::session_collection(&json!({})));
    let parsed = parser::message(&json!({"parts": [
        {"type": "reasoning", "text": "hidden"},
        {"type": "text", "text": "answer"}
    ]}))
    .expect("a document with assistant text is a message");
    assert_eq!(parsed.output, "answer");
    assert!(parser::message(&json!({"parts": []})).is_none());
}

/// The stream parser reports this session's assistant parts and nothing else,
/// and a frame that is not JSON is the stream's own framing failure.
#[test]
fn serve_event_parser_is_exact_session_and_assistant_only() {
    let mut projection = parser::ServeEventParser::new("open-1");
    let assistant = json!({
        "type": "message.updated",
        "properties": {"info": {
            "id": "agent", "role": "assistant", "sessionID": "open-1"
        }}
    });
    assert_eq!(projection.observe(&assistant.to_string()), Ok(None));
    let part = json!({
        "type": "message.part.updated",
        "properties": {
            "sessionId": "open-1",
            "part": {"messageID": "agent", "type": "text", "text": "delta"}
        }
    });
    assert_eq!(
        projection.observe(&part.to_string()),
        Ok(Some("delta".into()))
    );
    // A different session, a user's own part and a non-text part are all absent
    // answers rather than projections of somebody else's content.
    for excluded in [
        json!({
            "type": "message.part.updated",
            "properties": {
                "sessionId": "open-2",
                "part": {"messageID": "agent", "type": "text", "text": "other"}
            }
        }),
        json!({
            "type": "message.part.updated",
            "properties": {
                "sessionId": "open-1",
                "part": {"messageID": "user", "type": "text", "text": "private"}
            }
        }),
        json!({
            "type": "message.part.updated",
            "properties": {
                "sessionId": "open-1",
                "part": {"messageID": "agent", "type": "reasoning", "text": "hidden"}
            }
        }),
        json!({"type": "tool.updated", "properties": {"sessionId": "open-1"}}),
    ] {
        assert_eq!(projection.observe(&excluded.to_string()), Ok(None));
    }
    assert_eq!(
        projection.observe("{"),
        Err(parser::ServeEventFailure::InvalidJson)
    );
}

/// A completed turn reduces to the shared vocabulary in arrival order, and a
/// native tool interaction is reported as a control rather than hidden.
#[test]
fn serve_execution_produces_closed_typed_transitions() {
    let transitions = parser::completed_transitions("answer");
    // The projection is prefix closed from the declared initial stage and ends
    // at the terminal one, so a reader never sees a stage without the stages
    // that must precede it.
    assert!(matches!(
        transitions.first(),
        Some(Transition::Lifecycle(LifecycleStage::Submitted))
    ));
    assert!(matches!(
        transitions.last(),
        Some(Transition::Lifecycle(LifecycleStage::Completed))
    ));
    let reported = transitions
        .iter()
        .filter(
            |transition| matches!(transition, Transition::Text { text, .. } if text == "answer"),
        )
        .count();
    assert_eq!(reported, 1, "the reply is reported exactly once");
    let position = transitions
        .iter()
        .position(|transition| matches!(transition, Transition::Text { .. }))
        .expect("the reply is projected");
    assert!(
        position + 1 < transitions.len(),
        "the reply precedes the terminal stage"
    );
    let with_tool = parser::message(&json!({"parts": [
        {"type": "tool", "tool": "bash"},
        {"type": "text", "text": "answer"}
    ]}))
    .expect("a message");
    assert!(with_tool.transitions.iter().any(|transition| matches!(
        transition,
        Transition::Control { method, .. } if method == "bash"
    )));
    let failed = parser::failure_transitions("code", "serve/sse", "safe");
    assert!(matches!(
        failed.first(),
        Some(Transition::Lifecycle(LifecycleStage::Submitted))
    ));
    assert!(matches!(
        failed.last(),
        Some(Transition::Failed { code, .. }) if code == "code"
    ));
}

/// Readiness is a conjunction of protocol facts and a selected model, taken from
/// the configuration first and the provider's own default second — never from a
/// session's stale recorded model.
#[test]
fn readiness_uses_config_then_provider_order_and_never_session_models() {
    let providers = json!({
        "all": [
            {"id": "anthropic", "models": {"claude-current": {}}},
            {"id": "openai", "models": {"gpt-current": {}}}
        ],
        "default": {"anthropic": "claude-current", "openai": "gpt-current"}
    });
    let configured = parser::readiness(
        &json!({"healthy": true, "version": "2.0.0"}),
        &json!([{"id": "old", "model": "stale-session-model"}]),
        &json!({"model": "openai/gpt-current"}),
        &providers,
    )
    .expect("a healthy endpoint with a selected model is ready");
    assert_eq!(configured.catalog.current.selector(), "openai/gpt-current");
    let defaulted = parser::readiness(
        &json!({"healthy": true, "version": "2.0.0"}),
        &json!([]),
        &json!({}),
        &providers,
    )
    .expect("the provider default is a selection");
    assert_eq!(
        defaulted.catalog.current.selector(),
        "anthropic/claude-current"
    );
    // An endpoint that is not healthy, or that answers no version, is not ready
    // and reports no catalogue rather than an empty one.
    assert!(
        parser::readiness(
            &json!({"healthy": false, "version": "2.0.0"}),
            &json!([]),
            &json!({}),
            &providers,
        )
        .is_none()
    );
    assert!(
        parser::readiness(
            &json!({"healthy": true}),
            &json!([]),
            &json!({}),
            &providers,
        )
        .is_none()
    );
}

/// The package cannot start work the host has not admitted, and the first
/// answer the host installs is the one that stands.
#[test]
fn the_execution_port_admits_only_what_the_host_answered() {
    // No host has answered yet: the failure modes are a refusal, not an
    // invented admission.
    assert!(!execution::installed());
    assert!(!execution::admits_execution());

    assert!(
        execution::install(execution::ExecutionPort {
            admits_execution: || true,
        })
        .is_ok()
    );
    assert!(execution::installed());
    assert!(execution::admits_execution());
    // A second installation is refused rather than replacing the first answer.
    assert!(
        execution::install(execution::ExecutionPort {
            admits_execution: || false,
        })
        .is_err()
    );
    assert!(execution::admits_execution(), "the first answer stands");
}
