//! End-to-end routing for the manual-stop and force-stop control lane.
//!
//! These cases send the exact wire frames the desktop client's work-control
//! gateway sends and assert that the stdio server answers them from the real
//! native owner. A control method that stops being routed here falls through to
//! the ordinary conversation CLI path, which is the defect this proves absent:
//! the whole work-control surface would answer `unavailable` in the client
//! while the protocol still declares the method.

use super::*;
use std::io::Cursor;

fn control_frame(method: &str, params: Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(&json!({
        "protocol": STDIO_RPC_PROTOCOL,
        "id": "request-1",
        "workflowId": "workflow-1",
        "method": method,
        "params": params,
    }))
    .expect("a control frame serializes");
    bytes.push(b'\n');
    bytes
}

/// Serve one control frame with no persistent conversation runtime.
///
/// The three work-control methods are not persistent operations, so a host
/// without a conversation runtime must still reach their owner. The executor
/// panics because a control method is never a CLI command: any routing change
/// that sends one here fails this case instead of silently degrading it.
fn serve_control(method: &str, params: Value) -> Value {
    let output = serve_stdio_rpc(
        Cursor::new(control_frame(method, params)),
        Vec::new(),
        |_, _| panic!("{method} must never fall through to the CLI executor"),
    )
    .expect("the stdio server answers one control frame");
    serde_json::from_slice(&output).expect("the answer is one JSON frame")
}

#[test]
fn force_preview_reaches_the_native_owner() {
    let response = serve_control("agent.conversation.force.preview", json!({}));

    assert_eq!(
        response.get("error"),
        None,
        "the preview owner must answer, got {response}"
    );
    // With no scope chosen the owner names its own candidates rather than
    // letting the client guess which process group the user meant.
    assert_eq!(
        response.pointer("/result/status").and_then(Value::as_str),
        Some("scope-required"),
        "got {response}"
    );
    assert!(
        response.pointer("/result/candidates").is_some(),
        "the preview owner must report its candidate scopes, got {response}"
    );
    assert!(
        response
            .pointer("/result/correlationId")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty()),
        "the preview owner must bound its answer with a correlation id, got {response}"
    );
}

#[test]
fn force_preview_for_an_unknown_scope_is_refused_by_the_owner() {
    let response = serve_control(
        "agent.conversation.force.preview",
        json!({"scopeId": "scope-that-does-not-exist"}),
    );

    assert_eq!(
        response.pointer("/result/status").and_then(Value::as_str),
        Some("scope-unavailable"),
        "got {response}"
    );
    assert_eq!(
        response
            .pointer("/result/error/code")
            .and_then(Value::as_str),
        Some("force_stop_scope_unavailable"),
        "got {response}"
    );
}

#[test]
fn force_confirm_without_a_decision_signals_nothing() {
    // The owner decides the decline before it ever resolves a target, so a
    // request that never carried `confirmed: true` cannot signal anything even
    // when it names a scope.
    let response = serve_control(
        "agent.conversation.force.confirm",
        json!({"scopeId": "scope-that-does-not-exist", "confirmationToken": "token-1"}),
    );

    assert_eq!(
        response.pointer("/result/status").and_then(Value::as_str),
        Some("declined"),
        "got {response}"
    );
    assert_eq!(
        response
            .pointer("/result/signalled")
            .and_then(Value::as_bool),
        Some(false),
        "an unconfirmed force stop must signal nothing, got {response}"
    );
}

#[test]
fn force_confirm_for_a_changed_target_is_refused_after_the_decision() {
    // A confirmed request still re-verifies the exact scope revision at
    // execution time: a target that no longer matches its token is refused and
    // the new process is never signalled.
    let response = serve_control(
        "agent.conversation.force.confirm",
        json!({
            "scopeId": "scope-that-does-not-exist",
            "confirmationToken": "token-1",
            "confirmed": true
        }),
    );

    assert_eq!(
        response.pointer("/result/status").and_then(Value::as_str),
        Some("unconfirmed"),
        "got {response}"
    );
    assert_eq!(
        response
            .pointer("/result/signalled")
            .and_then(Value::as_bool),
        Some(false),
        "a changed target must signal nothing, got {response}"
    );
    assert_eq!(
        response
            .pointer("/result/error/code")
            .and_then(Value::as_str),
        Some("force_stop_confirmed_target_changed"),
        "got {response}"
    );
}

#[test]
fn manual_stop_for_an_unknown_owner_is_answered_not_routed_to_the_cli() {
    // A stop names one durable identity the host resolves itself. An identity
    // the host does not own is answered with a bounded refusal; it never
    // becomes a CLI command.
    let response = serve_control(
        "agent.conversation.stop",
        json!({"turnHandle": "turn-that-does-not-exist"}),
    );

    assert_eq!(
        response.get("error"),
        None,
        "the stop owner must answer, got {response}"
    );
    assert_eq!(
        response.get("ok"),
        Some(&Value::Bool(true)),
        "the stop owner answers inside the bounded result shape, got {response}"
    );
    assert_eq!(
        response.pointer("/result/ok").and_then(Value::as_bool),
        Some(false),
        "a stop the host cannot resolve is never reported as accepted, got {response}"
    );
}
