use super::support::{
    completed_outcome, config, failed_effect, initialize, open_thread, start_turn,
};
use crate::parser::CodexParser;
use crate::port::turn_event::{TestSinkGuard, install_test_sink};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[test]
fn matching_completion_uses_last_agent_message_and_thread_authority() {
    let mut protocol = CodexParser::new(config(
        json!({"model": "explicit-model", "reasoningEffort": "high"}),
        "hello",
        "",
    ));
    initialize(&mut protocol);
    open_thread(&mut protocol);
    start_turn(&mut protocol);

    assert!(
        protocol
            .handle_message(json!({
                "method": "turn/completed",
                "params": {
                    "threadId": "another-thread",
                    "turn": {"id": "turn-1", "status": "completed", "items": []}
                }
            }))
            .is_empty()
    );

    let outcome = completed_outcome(protocol.handle_message(json!({
        "method": "turn/completed",
        "params": {
            "threadId": "thread-1",
            "turn": {
                "id": "turn-1",
                "status": "completed",
                "items": [
                    {"id": "agent-1", "type": "agentMessage", "text": "draft"},
                    {"id": "reasoning-1", "type": "reasoning", "summary": []},
                    {"id": "agent-2", "type": "agentMessage", "text": "final answer"}
                ]
            }
        }
    })));
    assert_eq!(outcome.output, "final answer");
    assert_eq!(outcome.session_id, "thread-1");
    assert_eq!(outcome.thread_id, "thread-1");
    assert_eq!(outcome.turn_id, "turn-1");
    assert_eq!(outcome.turn_status, "completed");
    assert_eq!(outcome.effective.model.as_deref(), Some("explicit-model"));
    assert_eq!(outcome.effective.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(outcome.effective.cwd.as_deref(), Some("/workspace/project"));
    assert_eq!(outcome.effective.approval_policy, Some(json!("on-request")));
}

#[test]
fn native_item_started_emits_redacted_processing_receipt() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let sink_target = Arc::clone(&captured);
    install_test_sink(Box::new(move |event| {
        sink_target.lock().unwrap().push(event);
    }));
    let _guard = TestSinkGuard;
    let mut protocol = CodexParser::new(config(json!({}), "hello", ""));
    initialize(&mut protocol);
    open_thread(&mut protocol);
    start_turn(&mut protocol);

    protocol.handle_message(json!({
        "method": "item/started",
        "params": {
            "threadId": "thread-1",
            "turnId": "turn-1",
            "item": {
                "id": "private-item-id",
                "type": "reasoning",
                "summary": [{"text": "private chain of thought"}]
            }
        }
    }));

    let events = captured.lock().unwrap().clone();
    assert!(events.iter().any(|event| {
        event["event"] == "agent.turn.processing"
            && event["payload"]
                == json!({
                    "evidenceKind": "reasoning",
                    "lifecyclePrefix": ["submitted", "accepted", "processing"]
                })
    }));
    let encoded = serde_json::to_string(&events).unwrap();
    assert!(!encoded.contains("private-item-id"));
    assert!(!encoded.contains("private chain of thought"));
}

#[test]
fn mcp_tool_call_projects_only_the_tool_name() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let sink_target = Arc::clone(&captured);
    install_test_sink(Box::new(move |event| {
        sink_target.lock().unwrap().push(event);
    }));
    let _guard = TestSinkGuard;
    let mut protocol = CodexParser::new(config(json!({}), "hello", ""));
    initialize(&mut protocol);
    open_thread(&mut protocol);
    start_turn(&mut protocol);

    protocol.handle_message(json!({
        "method": "item/started",
        "params": {
            "threadId": "thread-1",
            "turnId": "turn-1",
            "item": {
                "id": "mcp-1",
                "type": "mcpToolCall",
                "server": "land.lico.licoup.subagents",
                "tool": "lico_subagent_cancel",
                "arguments": {"conversationId": "private", "membershipId": "private"},
                "status": "inProgress"
            }
        }
    }));

    let events = captured.lock().unwrap().clone();
    assert!(events.iter().any(|event| {
        event["event"] == "agent.turn.processing"
            && event["payload"]["evidenceKind"] == "tool"
            && event["payload"]["toolName"] == "lico_subagent_cancel"
    }));
    let encoded = serde_json::to_string(&events).unwrap();
    assert!(!encoded.contains("private"));
    assert!(!encoded.contains("conversationId"));
}

#[test]
fn mcp_tool_call_ignores_a_server_name_and_keeps_a_frozen_suffix() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let sink_target = Arc::clone(&captured);
    install_test_sink(Box::new(move |event| {
        sink_target.lock().unwrap().push(event);
    }));
    let _guard = TestSinkGuard;
    let mut protocol = CodexParser::new(config(json!({}), "hello", ""));
    initialize(&mut protocol);
    open_thread(&mut protocol);
    start_turn(&mut protocol);

    protocol.handle_message(json!({
        "method": "item/started",
        "params": {
            "threadId": "thread-1",
            "turnId": "turn-1",
            "item": {
                "id": "mcp-server-name",
                "type": "mcpToolCall",
                "name": "land.lico.licoup.subagents",
                "status": "inProgress"
            }
        }
    }));
    protocol.handle_message(json!({
        "method": "item/started",
        "params": {
            "threadId": "thread-1",
            "turnId": "turn-1",
            "item": {
                "id": "mcp-fqn",
                "type": "mcpToolCall",
                "name": "land.lico.licoup.subagents-lico_subagent_delegate",
                "status": "inProgress"
            }
        }
    }));

    let events = captured.lock().unwrap().clone();
    assert!(events.iter().any(|event| {
        event["event"] == "agent.turn.processing"
            && event["payload"]["evidenceKind"] == "tool"
            && event["payload"].get("toolName").is_none()
    }));
    assert!(events.iter().any(|event| {
        event["event"] == "agent.turn.processing"
            && event["payload"]["toolName"] == "land.lico.licoup.subagents-lico_subagent_delegate"
    }));
    let encoded = serde_json::to_string(&events).unwrap();
    assert!(!encoded.contains("\"toolName\":\"land.lico.licoup.subagents\""));
}

#[test]
fn mcp_tool_completion_projects_only_allowlisted_application_codes() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let sink_target = Arc::clone(&captured);
    install_test_sink(Box::new(move |event| {
        sink_target.lock().unwrap().push(event);
    }));
    let _guard = TestSinkGuard;
    let mut protocol = CodexParser::new(config(json!({}), "hello", ""));
    initialize(&mut protocol);
    open_thread(&mut protocol);
    start_turn(&mut protocol);

    protocol.handle_message(json!({
        "method": "item/completed",
        "params": {
            "threadId": "thread-1",
            "turnId": "turn-1",
            "item": {
                "id": "mcp-2",
                "type": "mcpToolCall",
                "server": "land.lico.licoup.subagents",
                "tool": "lico_subagent_delegate",
                "arguments": {"prompt": "private-prompt"},
                "status": "failed",
                "result": {
                    "content": [{
                        "type": "text",
                        "text": "caller_membership_not_authorized private provider result"
                    }]
                }
            }
        }
    }));

    let events = captured.lock().unwrap().clone();
    assert!(events.iter().any(|event| {
        event["event"] == "agent.turn.processing"
            && event["payload"]["toolName"] == "lico_subagent_delegate"
    }));
    assert!(events.iter().any(|event| {
        event["event"] == "agent.tool.result"
            && event["payload"]["toolName"] == "lico_subagent_delegate"
            && event["payload"]["text"] == "caller_membership_not_authorized"
    }));
    let encoded = serde_json::to_string(&events).unwrap();
    assert!(!encoded.contains("private-prompt"));
    assert!(!encoded.contains("private provider result"));
}

#[test]
fn native_item_started_and_completed_emit_one_processing_receipt() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let sink_target = Arc::clone(&captured);
    install_test_sink(Box::new(move |event| {
        sink_target.lock().unwrap().push(event);
    }));
    let _guard = TestSinkGuard;
    let mut protocol = CodexParser::new(config(json!({}), "hello", ""));
    initialize(&mut protocol);
    open_thread(&mut protocol);
    start_turn(&mut protocol);
    let item = json!({"id": "reasoning-1", "type": "reasoning", "summary": []});

    for method in ["item/started", "item/completed"] {
        protocol.handle_message(json!({
            "method": method,
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "item": item.clone()
            }
        }));
    }

    let events = captured.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event["event"] == "agent.turn.processing")
            .count(),
        1
    );
}

#[test]
fn idless_completed_item_emits_once_with_or_without_started_receipt() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let sink_target = Arc::clone(&captured);
    install_test_sink(Box::new(move |event| {
        sink_target.lock().unwrap().push(event);
    }));
    let _guard = TestSinkGuard;
    let mut protocol = CodexParser::new(config(json!({}), "hello", ""));
    initialize(&mut protocol);
    open_thread(&mut protocol);
    start_turn(&mut protocol);
    let item = json!({"type": "reasoning", "summary": []});

    for method in ["item/started", "item/completed", "item/completed"] {
        protocol.handle_message(json!({
            "method": method,
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "item": item.clone()
            }
        }));
    }

    assert_eq!(
        captured
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event["event"] == "agent.turn.processing")
            .count(),
        2
    );
}

#[test]
fn failed_turn_classifies_closed_codex_error_without_leaking_details() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let sink_target = Arc::clone(&captured);
    install_test_sink(Box::new(move |event| {
        sink_target.lock().unwrap().push(event);
    }));
    let _guard = TestSinkGuard;
    let mut protocol = CodexParser::new(config(json!({}), "hello", ""));
    initialize(&mut protocol);
    open_thread(&mut protocol);
    start_turn(&mut protocol);

    let failure = failed_effect(protocol.handle_message(json!({
        "method": "turn/completed",
        "params": {
            "threadId": "thread-1",
            "turn": {
                "id": "turn-1",
                "status": "failed",
                "items": [],
                "error": {
                    "message": "turn-fixture/secret.txt is unreadable",
                    "codexErrorInfo": "Unauthorized",
                    "additionalDetails": "turn-fixture/secret.txt"
                }
            }
        }
    })));
    assert_eq!(failure.code, "codex_turn_not_completed");
    assert_eq!(failure.turn_status.as_deref(), Some("failed/Unauthorized"));
    assert_eq!(failure.message, "Codex rejected the turn as unauthorized.");
    let encoded = format!("{failure:?}");
    assert!(!encoded.contains("secret.txt"));
    assert!(!encoded.contains("turn-fixture"));
    let events = captured.lock().unwrap().clone();
    let failed = events
        .iter()
        .find(|event| event["event"] == "dispatch.turn.failed")
        .expect("failed turn emits dispatch.turn.failed");
    assert_eq!(failed["payload"]["turnStatus"], "failed/Unauthorized");
    assert_eq!(failed["payload"]["code"], "codex_turn_not_completed");
    let payload = serde_json::to_string(&failed["payload"]).unwrap();
    assert!(!payload.contains("secret.txt"));
    assert!(!payload.contains("turn-fixture"));
}

#[test]
fn interrupted_turn_is_not_completed() {
    let mut protocol = CodexParser::new(config(json!({}), "hello", ""));
    initialize(&mut protocol);
    open_thread(&mut protocol);
    start_turn(&mut protocol);

    let failure = failed_effect(protocol.handle_message(json!({
        "method": "turn/completed",
        "params": {
            "threadId": "thread-1",
            "turn": {"id": "turn-1", "status": "interrupted", "items": []}
        }
    })));
    assert_eq!(failure.code, "codex_turn_not_completed");
    assert_eq!(failure.turn_status.as_deref(), Some("interrupted"));
    assert_eq!(failure.message, "Codex interrupted the requested turn.");
}

#[test]
fn failed_turn_classifies_current_codex_error_shapes() {
    let cases = [
        (
            json!("serverOverloaded"),
            "codex_turn_not_completed",
            "failed/ServerOverloaded",
            "Codex is temporarily overloaded.",
            None,
        ),
        (
            json!({"responseStreamDisconnected": {"httpStatusCode": 502}}),
            "codex_turn_not_completed",
            "failed/ResponseStreamDisconnected",
            "Codex response stream disconnected.",
            None,
        ),
        (
            json!({"usageLimitExceeded": {"resetAt": "redacted fixture"}}),
            "codex_usage_limit_exceeded",
            "failed/UsageLimitExceeded",
            "Codex usage limit exceeded.",
            Some((
                "native_cli",
                false,
                "select_available_model_or_wait_for_quota_reset",
            )),
        ),
    ];
    for (error_info, expected_code, expected_status, expected_message, resolution) in cases {
        let mut protocol = CodexParser::new(config(json!({}), "hello", ""));
        initialize(&mut protocol);
        open_thread(&mut protocol);
        start_turn(&mut protocol);
        let failure = failed_effect(protocol.handle_message(json!({
            "method": "turn/completed",
            "params": {
                "threadId": "thread-1",
                "turn": {
                    "id": "turn-1",
                    "status": "failed",
                    "items": [],
                    "error": {
                        "message": "private fixture detail",
                        "codexErrorInfo": error_info,
                    }
                }
            }
        })));
        assert_eq!(failure.code, expected_code);
        assert_eq!(failure.turn_status.as_deref(), Some(expected_status));
        assert_eq!(failure.message, expected_message);
        match resolution {
            Some((component, retryable, recovery)) => {
                assert_eq!(failure.component, Some(component));
                assert_eq!(failure.retryable, Some(retryable));
                assert_eq!(failure.recovery, Some(recovery));
            }
            None => {
                assert_eq!(failure.component, None);
                assert_eq!(failure.retryable, None);
                assert_eq!(failure.recovery, None);
            }
        }
        assert!(!format!("{failure:?}").contains("private fixture detail"));
        assert!(!format!("{failure:?}").contains("redacted fixture"));
    }
}

#[test]
fn missing_final_message_fails_without_a_completed_lifecycle_event() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let sink_target = Arc::clone(&captured);
    install_test_sink(Box::new(move |event| {
        sink_target.lock().unwrap().push(event);
    }));
    let _guard = TestSinkGuard;
    let mut protocol = CodexParser::new(config(json!({}), "hello", ""));
    initialize(&mut protocol);
    open_thread(&mut protocol);
    start_turn(&mut protocol);

    let failure = failed_effect(protocol.handle_message(json!({
        "method": "turn/completed",
        "params": {
            "threadId": "thread-1",
            "turn": {
                "id": "turn-1",
                "status": "completed",
                "items": [{"type": "agentMessage", "text": "  "}],
            }
        }
    })));
    assert_eq!(failure.code, "codex_final_message_missing");
    let events = captured.lock().unwrap();
    assert!(events.iter().any(|event| {
        event["event"] == "dispatch.turn.failed"
            && event["payload"]["code"] == "codex_final_message_missing"
    }));
    assert!(
        !events
            .iter()
            .any(|event| event["event"] == "dispatch.turn.completed")
    );
}
