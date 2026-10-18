//! The ACP frame dialect this crate's suites drive the transport through, and
//! the installer that puts it in place.
//!
//! The transport engines read one driver's dialect through the port the
//! composition above this crate answers. A unit-test binary is not that
//! composition, so it installs a dialect here. Installing one is not optional:
//! with no dialect installed the transport answers fail-closed, and every
//! assertion about a state transition becomes an assertion about a refusal.
//!
//! This module is behind `test-support` rather than `cfg(test)` because **two**
//! builds need it and only one of them is this crate's own test build. The
//! host's Hermes and replay suites drive the moved reducers directly, and the
//! host links this crate as a dependency — so a `cfg(test)` dialect would be
//! compiled out of exactly the build that calls it. The host enables
//! `licoup-agent-drivers/test-support` from its own `test-support` feature.
//!
//! The dialect is **not** a stub. Every member delegates to the same shared ACP
//! semantics the production per-Agent parsers delegate to —
//! `licoup_foundation::core::acp`'s codec, envelope validation and value types —
//! so a test frame is decoded, classified and validated exactly as a production
//! frame is, and the only thing under test is a reducer's own state machine.
//! What differs from a production parser is the transition vocabulary: the
//! production ACP parsers word a completed or failed turn in their own Agent's
//! unit id, and a change to one Agent's wording must not be able to move a
//! transport test. The suites here assert the transition *sequence*, so the
//! neutral unit id used here is what they read.

use licoup_agent_adapter_sdk::{LifecycleStage, Transition, TransitionReducer};
use licoup_foundation::core::acp::{self, AcpInitializeResponse, AcpSessionUpdate, AcpStopReason};
use serde_json::Value;

use crate::acp_driver_runtime::parser_port::{
    self, AcpParserRegistration, ProtocolClientRequest, ProtocolPermissionRequest,
};

/// The driver identities the transport suites build a protocol for.
///
/// They are driver identities rather than Agent names, because that is what the
/// transport is keyed on: the suites exercise the shared ACP profile and the
/// persistent ACP profile, and the port answers by identity.
pub const COPILOT_ACP: &str = "copilot-acp";
pub const HERMES_ACP: &str = "hermes-acp";
/// The identity the transport suites build an ordinary driver spec with when
/// the test is not about which Agent is reached. `AcpDriverSpec::new` defaults
/// its identity to this, so it is what a spec that names no Agent carries.
pub const TEST_ACP: &str = "test-acp";

/// The option kind the shared ACP profile marks a one-shot approval with.
const ALLOW_ONCE_KIND: &str = "allow_once";

/// The unit id this dialect attributes a turn to. See the module note: it is
/// neutral on purpose so a per-Agent wording change cannot move a transport
/// test.
const TRANSPORT_UNIT: &str = "acp-transport:reply";

/// Install the transport dialect into a build that has none.
///
/// It is idempotent and it never displaces an installed composition: a host
/// that composed its real per-Agent dialects keeps them, and only a build that
/// installed nothing reaches this. The reducers call it at construction, so a
/// suite cannot forget it and silently assert against refusals.
pub fn ensure() {
    static ENSURED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    ENSURED.get_or_init(|| parser_port::install(dialects()));
}

/// Whether the dialect the suites need is the one that is installed. A suite
/// that reached a fail-closed dialect would otherwise assert against refusals.
pub fn installed_driver_ids() -> Vec<&'static str> {
    parser_port::installed()
        .iter()
        .map(|registration| registration.driver_id)
        .collect()
}

/// The dialect set a build with no composition reads.
pub fn dialects() -> Vec<AcpParserRegistration> {
    vec![
        AcpParserRegistration {
            driver_id: COPILOT_ACP,
            ..shared_profile()
        },
        AcpParserRegistration {
            driver_id: TEST_ACP,
            ..shared_profile()
        },
        AcpParserRegistration {
            driver_id: HERMES_ACP,
            ..shared_profile()
        },
    ]
}

/// The shared ACP frame readers, as `licoup_foundation::core::acp` states them.
///
/// Only the frame readers: a dialect's transition builders belong to the Agent
/// whose unit id they carry, so this set leaves them empty and the reducers
/// that read them are driven by the host's real per-Agent registrations.
fn shared_profile() -> AcpParserRegistration {
    AcpParserRegistration {
        driver_id: "",
        decode_frame: acp::decode_json_line,
        is_notification,
        response_id_matches,
        response_is_error,
        session_update,
        prompt_stop_reason,
        initialize_response,
        client_request,
        permission_request,
        completed_transitions: transport_completed_transitions,
        failed_transitions: failed_transitions,
    }
}

/// The completed-turn transitions the transport suites read, under the neutral
/// unit id this module owns.
fn transport_completed_transitions(output: &str) -> Vec<Transition> {
    completed_transitions(TRANSPORT_UNIT, output)
}

fn is_notification(message: &Value) -> bool {
    message.get("method").is_some() && message.get("id").is_none()
}

fn response_is_error(message: &Value) -> bool {
    message.get("error").is_some()
}

fn response_id_matches(message: &Value, expected: i64) -> bool {
    message.get("id").is_some_and(|id| {
        id.as_i64() == Some(expected)
            || id
                .as_str()
                .and_then(|value| value.parse::<i64>().ok())
                .is_some_and(|value| value == expected)
    })
}

fn session_update(
    message: &Value,
    expected_session_id: Option<&str>,
) -> Result<AcpSessionUpdate, acp::AcpError> {
    acp::validate_session_update(message, expected_session_id)
}

fn prompt_stop_reason(message: &Value, request_id: i64) -> Result<AcpStopReason, acp::AcpError> {
    acp::validate_prompt_response(message, request_id).map(|response| response.stop_reason)
}

fn initialize_response(
    line: &[u8],
    request_id: i64,
) -> Result<Option<AcpInitializeResponse>, acp::AcpError> {
    let frame = acp::decode_json_line(line)?;
    if !response_id_matches(&frame, request_id) {
        return Ok(None);
    }
    acp::validate_initialize_response(&frame, request_id).map(Some)
}

/// A client request, in the shape the shared ACP profile reports one.
///
/// The one-shot approval option is recognised so the interaction suites can
/// drive a permission question end to end.
fn client_request(message: &Value) -> Option<ProtocolClientRequest> {
    let id = message.get("id")?.clone();
    let method = message.get("method")?.as_str()?.to_owned();
    if message.get("result").is_some() || message.get("error").is_some() {
        return None;
    }
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    // The one-shot approval is the option the Agent marked `allow_once`, which
    // is the only option kind a fully autonomous turn may select without asking.
    let allow_once_option = params
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|option| option.get("kind").and_then(Value::as_str) == Some(ALLOW_ONCE_KIND))
        .and_then(|option| option.get("optionId"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    Some(ProtocolClientRequest {
        id,
        method,
        session_id: params
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_owned),
        allow_once_option,
    })
}

/// A permission question, in the shape the persistent ACP profile reports one.
///
/// Hermes states a permission question as its own frame rather than as a client
/// request, which is why its reducer reads this member and the shared profile's
/// does not.
fn permission_request(message: &Value) -> Option<ProtocolPermissionRequest> {
    let request = client_request(message)?;
    Some(ProtocolPermissionRequest {
        id: request.id,
        method: request.method,
        session_id: request.session_id,
        display_summary: format!("Run {REQUESTED_TOOL}"),
        option_id: request.allow_once_option,
        requested_tools: vec![REQUESTED_TOOL.to_owned()],
    })
}

/// The tool a permission question in these suites asks about.
const REQUESTED_TOOL: &str = "run_command";

/// The transition sequence one completed turn reports, for `unit_id`.
///
/// It is the sequence both production ACP parsers report — accepted, processing,
/// responding, one text unit, completed — parameterised by the unit id, which
/// is the one thing they do not share.
pub fn completed_transitions(unit_id: &str, output: &str) -> Vec<Transition> {
    let mut reducer = TransitionReducer::default();
    let mut transitions = reducer.advance(LifecycleStage::Accepted);
    transitions.extend(reducer.advance(LifecycleStage::Processing));
    transitions.extend(reducer.advance(LifecycleStage::Responding));
    transitions.push(Transition::Text {
        unit_id: unit_id.to_owned(),
        text: output.to_owned(),
    });
    transitions.extend(reducer.advance(LifecycleStage::Completed));
    transitions
}

/// The transition sequence one failed turn reports.
pub fn failed_transitions(code: &str, stage: &str, message: &str) -> Vec<Transition> {
    let mut reducer = TransitionReducer::default();
    let mut transitions = reducer.advance(LifecycleStage::Accepted);
    transitions.extend(reducer.advance(LifecycleStage::Processing));
    if let Some(failure) = reducer.fail(code, stage, message) {
        transitions.push(failure);
    }
    transitions
}

/// The unit id the transport suites' own completed turns carry.
pub const TRANSPORT_UNIT_ID: &str = TRANSPORT_UNIT;
