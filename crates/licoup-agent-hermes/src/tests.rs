//! The package's own claims.
//!
//! A claim that names Hermes — what its parser decodes, which permission
//! question it reads, what a completed or failed turn reduces to, which answers
//! its registration gives — belongs here, beside the parser that makes it. The
//! claims that name no Agent live in `licoup-agent-adapter-sdk`'s own test
//! module, and the claims about the committed release documents live in
//! `tests/package_artifact.rs`.

use licoup_agent_adapter_sdk::adapters::AdapterContract;
use licoup_agent_adapter_sdk::port::{DurableIdentityRequest, ExecutionFailure, ExecutionOutcome};
use licoup_agent_adapter_sdk::{LifecycleStage, Transition};
use serde_json::json;

use crate::dialect;
use crate::parser;
use crate::registration;

/// The declaration this package reports is the one its composition injects.
#[test]
fn the_package_declares_one_adapter_and_reports_it_through_the_sdk() {
    assert_eq!(
        registration::CONTRACT,
        AdapterContract::new("hermes", "stdio-jsonrpc-acp")
    );
    assert_eq!(registration::ADAPTER_ID, "hermes");
    assert_eq!(registration::FRAMING, "stdio-jsonrpc-acp");
    assert_eq!(parser::CONTRACT.id, registration::ADAPTER_ID);
    assert_eq!(parser::CONTRACT.framing, registration::FRAMING);
    assert_eq!(registration::registrations().len(), 1);
    assert_eq!(registration::contract(), Some(registration::CONTRACT));
    let set = registration::parser_set();
    assert_eq!(set.registered_ids(), ["hermes"]);
    assert_eq!(set.contract("hermes"), Some(registration::CONTRACT));
    assert!(set.contract("codex").is_none());
}

/// The dialect answers for the identity the transport pools Hermes sessions by,
/// and it publishes the vendor wire its release declaration names.
#[test]
fn the_dialect_answers_for_the_declared_driver_identity() {
    let registration = dialect::registration();
    assert_eq!(registration.driver_id, dialect::DRIVER_ID);
    assert_eq!(dialect::DRIVER_ID, "hermes-acp");
    assert_eq!(dialect::PROTOCOL_FORMAT, "hermes.acp.v1");
}

/// Hermes' normalized transitions are Hermes' answer, not an empty declaration:
/// the host reads them through this query because the driver reports none.
#[test]
fn a_completed_execution_reduces_to_the_hermes_reply_transitions() {
    let outcome = ExecutionOutcome {
        output: "the recorded reply",
        failure: None,
    };
    let transitions = registration::execution_transitions(&outcome);
    // The whole sequence, in arrival order: the stages Hermes reached, the reply
    // under Hermes' own unit id, and the terminal stage. Hermes reports no
    // transition list of its own, so this answer is the one the host's Hermes
    // normalization reads — an empty or re-worded answer moves this vector.
    assert_eq!(
        transitions,
        vec![
            Transition::Lifecycle(LifecycleStage::Submitted),
            Transition::Lifecycle(LifecycleStage::Accepted),
            Transition::Lifecycle(LifecycleStage::Processing),
            Transition::Lifecycle(LifecycleStage::Responding),
            Transition::Text {
                unit_id: "hermes:reply".to_owned(),
                text: "the recorded reply".to_owned(),
            },
            Transition::Lifecycle(LifecycleStage::Completed),
        ]
    );
}

/// A failed execution reports the protocol's own failure, and it never also
/// reports a completed turn.
#[test]
fn a_failed_execution_reduces_to_the_reported_failure_transition() {
    let outcome = ExecutionOutcome {
        output: "",
        failure: Some(ExecutionFailure {
            code: "hermes_acp_prompt_failed",
            stage: "prompt",
            message: "Hermes Agent rejected the prompt.",
        }),
    };
    let transitions = registration::execution_transitions(&outcome);
    assert_eq!(
        transitions
            .iter()
            .filter(|transition| matches!(transition, Transition::Failed { .. }))
            .count(),
        1,
        "one first failure is reported: {transitions:?}"
    );
    assert!(transitions.iter().any(|transition| matches!(
        transition,
        Transition::Failed { code, stage, .. }
            if code == "hermes_acp_prompt_failed" && stage == "prompt"
    )));
    assert!(
        !transitions.iter().any(|transition| matches!(
            transition,
            Transition::Lifecycle(LifecycleStage::Completed)
        )),
        "a failed turn is never also completed: {transitions:?}"
    );
}

/// The durable-identity query is declared and answered fail-closed: the Subagent
/// mesh never dispatches Hermes, so no identity is ever accepted — including one
/// that another Agent's rule would accept.
#[test]
fn the_durable_identity_query_stays_fail_closed() {
    let request = DurableIdentityRequest {
        session_id: "native-session",
        location: None,
    };
    assert!(
        !registration::no_identity(&request),
        "Hermes answers no durable identity"
    );
    assert_eq!(
        registration::parser_set().valid_identity("hermes", &request),
        Some(false),
        "the query is declared rather than omitted, and the answer is false"
    );
    // The declaration is not [`ParserRegistration::unanswered`]'s: Hermes owns a
    // real transition answer, so a reader that saw only a fail-closed identity
    // must not conclude the whole registration is unanswered.
    let answered: licoup_agent_adapter_sdk::port::ExecutionTransitions =
        registration::execution_transitions;
    assert!(
        registration::REGISTRATION.execution_transitions as usize == answered as usize,
        "the registration answers with this package's own transition builder"
    );
}

/// The parser-owned ingress: a raw LF-delimited frame becomes a JSON value here
/// and is never decoded again above.
#[test]
fn one_raw_line_becomes_exactly_one_decoded_frame() {
    let line = br#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}"#;
    let frame = parser::decode_frame(line).expect("a well-formed ACP line decodes");
    assert_eq!(frame["id"].clone(), json!(1));
    assert!(parser::decode_frame(b"not json").is_err());
    let notification = json!({"jsonrpc": "2.0", "method": "session/update"});
    assert!(parser::is_notification(&notification));
    let request = json!({"jsonrpc": "2.0", "id": 2, "method": "session/request_permission"});
    assert!(!parser::is_notification(&request));
    assert!(parser::response_id_matches(&json!({"id": 2}), 2));
    assert!(parser::response_id_matches(&json!({"id": "2"}), 2));
    assert!(!parser::response_id_matches(&json!({"id": 2}), 3));
    assert!(parser::response_is_error(
        &json!({"id": 4, "error": {"code": -1}})
    ));
    assert!(!parser::response_is_error(&json!({"id": 4, "result": {}})));
}

/// Hermes' permission question carries the facts the transport answers on, and a
/// summary the user reads rather than a method name.
#[test]
fn a_permission_question_reports_its_summary_option_and_tools() {
    let question = json!({
        "jsonrpc": "2.0",
        "id": 9,
        "method": "session/request_permission",
        "params": {
            "sessionId": "native-session",
            "toolCalls": [
                {"title": "  Write src/main.rs  "},
                {"kind": "read"},
                {"toolCall": {"title": "Bash"}},
            ],
            "options": [
                {"optionId": "reject_once", "kind": "reject_once"},
                {"optionId": "allow_once", "kind": "allow_once"},
            ],
        },
    });
    let request = parser::permission_request(&question).expect("a permission question is one");
    assert_eq!(request.id, json!(9));
    assert_eq!(request.method, "session/request_permission");
    assert_eq!(request.session_id.as_deref(), Some("native-session"));
    assert_eq!(request.option_id.as_deref(), Some("allow_once"));
    assert_eq!(
        request.requested_tools,
        ["Write src/main.rs", "read", "Bash"],
        "the tool names are trimmed and read from all three vendor shapes"
    );
    assert_eq!(
        request.display_summary,
        "Hermes Agent requests permission for: Write src/main.rs, read, Bash"
    );

    // A question that names no tool still reads as a question.
    let bare = parser::permission_request(&json!({
        "id": 10,
        "method": "session/request_permission",
        "params": {"options": [{"optionId": "allow", "kind": "allow"}]},
    }))
    .expect("a question without tools is one");
    assert_eq!(bare.requested_tools, Vec::<String>::new());
    assert_eq!(
        bare.display_summary,
        "Hermes Agent requests permission to continue."
    );
    assert_eq!(bare.option_id.as_deref(), Some("allow"));

    // A response is never a question, and neither is a notification.
    assert!(
        parser::permission_request(&json!({"id": 9, "result": {}})).is_none(),
        "a response is not a permission question"
    );
    assert!(
        parser::permission_request(&json!({"method": "session/update"})).is_none(),
        "a notification is not a permission question"
    );
}

/// Hermes asks no client request over this profile, and the transport projection
/// states that rather than inheriting another Agent's reader.
#[test]
fn the_dialect_asks_no_client_request_and_projects_its_permission_question() {
    assert!(dialect::no_client_request(&json!({"id": 1, "method": "fs/read_text_file"})).is_none());
    let projected = dialect::permission_request(&json!({
        "id": 7,
        "method": "session/request_permission",
        "params": {"sessionId": "native-session", "toolCalls": [{"title": "Write"}]},
    }))
    .expect("the projection carries Hermes' question");
    assert_eq!(projected.id, json!(7));
    assert_eq!(projected.session_id.as_deref(), Some("native-session"));
    assert_eq!(projected.requested_tools, ["Write"]);
    assert!(projected.display_summary.contains("Write"));
    assert!(dialect::permission_request(&json!({"id": 7, "result": {}})).is_none());
}

/// A recorded transcript of another Agent is refused rather than replayed
/// through Hermes' arm.
#[test]
fn the_replay_arm_refuses_an_adapter_this_package_does_not_carry() {
    assert!(crate::replay::replay_arm("codex").is_err());
    assert!(crate::replay::replay_arm("hermes").is_ok());
}
