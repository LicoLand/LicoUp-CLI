//! The package's own claims.
//!
//! A claim that names Kimi Code — what its dialect decodes, which client request
//! it reports, what a completed or failed turn reduces to — belongs here, beside
//! the parser that makes it. The claims that name no Agent live in
//! `licoup-agent-adapter-sdk`'s own test module.

use licoup_agent_adapter_sdk::adapters::AdapterContract;
use licoup_agent_adapter_sdk::{LifecycleStage, Transition};
use licoup_foundation::core::acp;
use serde_json::json;

use crate::dialect;
use crate::driver::{DRIVER, PROTOCOL_FORMAT, RUNTIME_PROTOCOL};
use crate::parser;
use crate::port::execution;
use crate::registration;

/// The declaration this package reports is the one its composition injects.
#[test]
fn the_package_declares_one_adapter_and_reports_it_through_the_sdk() {
    assert_eq!(
        registration::CONTRACT,
        AdapterContract::new("kimi-code", "lf-ndjson-acp")
    );
    assert_eq!(registration::ADAPTER_ID, "kimi-code");
    assert_eq!(registration::FRAMING, "lf-ndjson-acp");
    assert_eq!(parser::CONTRACT.id, registration::ADAPTER_ID);
    assert_eq!(parser::CONTRACT.framing, registration::FRAMING);
    assert_eq!(registration::registrations().len(), 1);
    assert_eq!(registration::contract(), Some(registration::CONTRACT));
    let set = registration::parser_set();
    assert_eq!(set.registered_ids(), ["kimi-code"]);
    assert_eq!(set.contract("kimi-code"), Some(registration::CONTRACT));
    assert!(set.contract("codex").is_none());
}

/// The framing the declaration reports is the framing the dialect answers for,
/// and the identity the transport pools by is the one the launch spec carries.
#[test]
fn the_dialect_answers_for_the_declared_driver_identity() {
    let registration = dialect::registration();
    assert_eq!(registration.driver_id, dialect::DRIVER_ID);
    assert_eq!(registration.driver_id, DRIVER.agent_id);
    assert_eq!(DRIVER.runtime_protocol, RUNTIME_PROTOCOL);
    assert_eq!(PROTOCOL_FORMAT, "kimi-code.acp.v1");
}

/// The parser-owned ingress: a raw LF-delimited frame becomes a JSON value here
/// and is never decoded again above.
#[test]
fn one_raw_line_becomes_exactly_one_decoded_frame() {
    let line = br#"{"jsonrpc":"2.0","id":7,"result":{"protocolVersion":1}}"#;
    let frame = parser::decode_frame(line).expect("a well-formed ACP line decodes");
    assert_eq!(frame["id"], json!(7));
    assert!(parser::decode_frame(b"not json").is_err());
}

/// A response is the response to exactly the request it answers.
#[test]
fn a_response_is_matched_by_request_id_and_never_by_position() {
    let frame = json!({"jsonrpc": "2.0", "id": 4, "result": {}});
    assert!(parser::response_id_matches(&frame, 4));
    assert!(!parser::response_id_matches(&frame, 5));
    assert!(!parser::response_id_matches(&json!({"id": "4"}), 4));
    let notification = json!({"jsonrpc": "2.0", "method": "session/update"});
    assert!(parser::is_notification(&notification));
    let request = json!({"jsonrpc": "2.0", "id": 1, "method": "session/request_permission"});
    assert!(!parser::is_notification(&request));
}

/// Kimi's client request carries the four facts the transport answers on, and a
/// frame that is a response rather than a request is not one.
#[test]
fn a_client_request_reports_its_id_method_session_and_approval_option() {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 11,
        "method": "session/request_permission",
        "params": {
            "sessionId": "native-session",
            "options": [
                {"optionId": "reject", "kind": "reject_once"},
                {"optionId": "approve", "kind": "allow_once"},
            ],
        },
    });
    let projected = dialect::client_request(&request).expect("a request is reported");
    assert_eq!(projected.id, json!(11));
    assert_eq!(projected.method, "session/request_permission");
    assert_eq!(projected.session_id.as_deref(), Some("native-session"));
    assert_eq!(projected.allow_once_option.as_deref(), Some("approve"));

    let response = json!({"jsonrpc": "2.0", "id": 11, "result": {}});
    assert!(dialect::client_request(&response).is_none());
    let no_id = json!({"jsonrpc": "2.0", "method": "session/update"});
    assert!(dialect::client_request(&no_id).is_none());
}

/// An approval option the transport cannot safely answer on is reported as
/// absent rather than invented: an empty or oversized option id is not one.
#[test]
fn an_unusable_approval_option_is_absent_rather_than_guessed() {
    for options in [
        json!([{"optionId": "   ", "kind": "allow_once"}]),
        json!([{"optionId": "x".repeat(257), "kind": "allow_once"}]),
        json!([{"optionId": "a\u{0}b", "kind": "allow_once"}]),
        json!([{"optionId": "approve", "kind": "reject_once"}]),
    ] {
        let request = json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "session/request_permission",
            "params": {"options": options},
        });
        let projected = dialect::client_request(&request).expect("the request is still reported");
        assert_eq!(projected.allow_once_option, None);
    }
}

/// Kimi asks no permission question over this profile, so the dialect reports
/// the absence rather than inheriting another Agent's reader.
#[test]
fn this_dialect_never_invents_a_permission_request() {
    let frame = json!({"jsonrpc": "2.0", "id": 1, "method": "session/request_permission"});
    assert_eq!(dialect::permission_request(&frame), None);
}

/// The error test is the shared ACP envelope rule: a frame carrying `error` is
/// the outcome the envelope rejects.
#[test]
fn the_error_test_is_the_shared_envelope_rule() {
    assert!(dialect::response_is_error(
        &json!({"id": 1, "error": {"code": -32601}})
    ));
    assert!(!dialect::response_is_error(&json!({"id": 1, "result": {}})));
}

/// One completed turn reduces to the arrival-ordered lifecycle with the Agent's
/// whole reply as its one text unit. The reducer starts before acceptance, so
/// the sequence opens with the submitted stage every parser reports.
#[test]
fn a_completed_turn_reduces_to_the_shared_transition_vocabulary() {
    let transitions = parser::completed_transitions("synthetic reply");
    assert_eq!(
        transitions,
        vec![
            Transition::Lifecycle(LifecycleStage::Submitted),
            Transition::Lifecycle(LifecycleStage::Accepted),
            Transition::Lifecycle(LifecycleStage::Processing),
            Transition::Lifecycle(LifecycleStage::Responding),
            Transition::Text {
                unit_id: "kimi-code:reply".to_owned(),
                text: "synthetic reply".to_owned(),
            },
            Transition::Lifecycle(LifecycleStage::Completed),
        ]
    );
}

/// One failed turn reduces to the open stages followed by the shared failure,
/// and the failure reports the protocol's own code, stage and redacted message.
#[test]
fn a_failed_turn_reduces_to_acceptance_and_the_shared_failure() {
    let transitions = parser::failed_transitions(
        "kimi_code_acp_working_directory_invalid",
        "params",
        "redacted",
    );
    assert_eq!(
        transitions.len(),
        3,
        "an unaccepted turn reports the open stages and one failure: {transitions:?}"
    );
    assert_eq!(
        transitions[0],
        Transition::Lifecycle(LifecycleStage::Submitted)
    );
    match transitions.last().expect("a reported failure") {
        Transition::Failed {
            code,
            stage,
            message,
        } => {
            assert_eq!(code, "kimi_code_acp_working_directory_invalid");
            assert_eq!(stage, "params");
            assert_eq!(message, "redacted");
        }
        other => panic!("a failed turn reports the shared failure, got {other:?}"),
    }
}

/// A session update the ACP envelope rejects is refused rather than read, so a
/// malformed frame cannot become a turn fact.
#[test]
fn a_malformed_session_update_is_refused() {
    let update = json!({"jsonrpc": "2.0", "method": "session/update", "params": {}});
    assert!(parser::session_update(&update, Some("native-session")).is_err());
    assert!(parser::prompt_stop_reason(&json!({"id": 1, "result": {}}), 1).is_err());
}

/// The initialize response is read only out of the line that answers the
/// request, and only when the envelope is the one the profile publishes.
#[test]
fn an_initialize_response_is_read_from_its_own_line() {
    let other =
        br#"{"jsonrpc":"2.0","id":2,"result":{"protocolVersion":1,"agentCapabilities":{}}}"#;
    assert_eq!(
        parser::initialize_response(other, 1).expect("decodes"),
        None,
        "a response to another request is not this Agent's initialize answer"
    );

    let answering = br#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"authMethods":[]}}"#;
    let response = parser::initialize_response(answering, 1)
        .expect("decodes")
        .expect("the answering line is read");
    assert_eq!(response.protocol_version, 1);
    assert!(response.capabilities.load_session);

    for refused in [
        // A protocol generation this profile does not publish.
        br#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":99}}"#.as_slice(),
        // A result that is not the object the envelope requires.
        br#"{"jsonrpc":"2.0","id":1,"result":[]}"#.as_slice(),
        // A capability value of the wrong shape.
        br#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"authMethods":{}}}"#.as_slice(),
        b"".as_slice(),
    ] {
        assert!(
            parser::initialize_response(refused, 1).is_err(),
            "an unusable initialize answer must be refused: {}",
            String::from_utf8_lossy(refused)
        );
    }
    assert_eq!(acp::AcpError::ResultInvalid, acp::AcpError::ResultInvalid);
}

/// The execution port is fail-closed until its host installs it: a package
/// running outside the client reports the refusal rather than running.
#[test]
fn the_execution_port_is_fail_closed_before_its_host_answers() {
    assert!(!execution::installed());
    assert!(!execution::admits_execution());

    let failure = crate::driver::capability_probe(
        "unused",
        std::path::Path::new("relative"),
        10,
        Some(1024),
        1024,
    )
    .expect_err("an uninstalled host admits no execution");
    assert_eq!(failure.code, "kimi_code_acp_execution_not_admitted");

    // A second installation is refused rather than replacing the first answer.
    assert!(
        execution::install(execution::ExecutionPort {
            admits_execution: || true,
        })
        .is_ok()
    );
    assert!(execution::installed());
    assert!(execution::admits_execution());
    assert!(
        execution::install(execution::ExecutionPort {
            admits_execution: || false,
        })
        .is_err()
    );
    assert!(execution::admits_execution(), "the first answer stands");
}

/// The launch metadata this package declares is the whole of what the shared
/// engine runs, so the host cannot describe a different Kimi than the package
/// publishes: the single official ACP entrypoint, its model and reasoning
/// settings, and the one flag that preserves an explicit `allowAll` request.
#[test]
fn canonical_driver_is_only_official_acp_entrypoint() {
    assert_eq!(RUNTIME_PROTOCOL, "kimi-code-acp-v1-stdio-ndjson");
    assert_eq!(DRIVER.runtime_protocol, RUNTIME_PROTOCOL);
    assert_eq!(DRIVER.agent_id, "kimi-code-acp");
    assert_eq!(DRIVER.error_prefix, "kimi_code_acp");
    assert_eq!(DRIVER.launch_args, &["acp"]);
    assert_eq!(DRIVER.launch_model_arg, Some("--model"));
    assert_eq!(
        DRIVER.launch_reasoning_env,
        Some("KIMI_MODEL_THINKING_EFFORT")
    );
    assert_eq!(DRIVER.launch_reasoning_values, &["low", "high", "max"]);
    assert_eq!(DRIVER.launch_allow_all_arg, Some("--auto"));
}

/// A relative working directory is refused before any process is started, and
/// the refusal carries this Agent's namespaced code rather than a privacy leak.
#[test]
fn launch_arguments_cannot_disclose_prompt_or_native_session() {
    assert_eq!(DRIVER.launch_args.len(), 1);
    assert!(
        !DRIVER
            .launch_args
            .iter()
            .any(|argument| argument.contains("prompt") || argument.contains("session"))
    );
    let result = crate::driver::execute(
        "unused",
        &json!({}),
        "private-prompt",
        "private-session",
        Some(std::path::Path::new("relative")),
        10,
        Some(1024),
        1024,
    );
    assert!(!result.ok);
    assert_eq!(result.driver_id, dialect::DRIVER_ID);
    assert_eq!(result.runtime_protocol, RUNTIME_PROTOCOL);
    let failure = result.error.expect("a structured ACP failure");
    assert!(!failure.message.contains("private"));
}

/// Cancelling a session this process never started reports the shared
/// disposition rather than inventing an accepted cancellation.
#[test]
fn cancelling_an_unknown_session_never_reports_an_accepted_cancel() {
    let disposition = crate::driver::cancel("never-started-session");
    assert_ne!(
        disposition,
        licoup_agent_drivers::acp_driver_runtime::ControlDisposition::Accepted
    );
}
