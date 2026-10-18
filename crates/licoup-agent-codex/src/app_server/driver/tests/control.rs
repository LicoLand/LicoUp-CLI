use super::support::{
    completed_outcome, config, initialize, open_thread, sent_messages, start_turn,
};
use crate::app_server::model::ProtocolEffect;
use crate::parser::CodexParser;
use serde_json::{Value, json};

fn active_turn() -> CodexParser {
    let mut protocol = CodexParser::new(config(json!({}), "hello", ""));
    initialize(&mut protocol);
    open_thread(&mut protocol);
    start_turn(&mut protocol);
    protocol
}

fn complete_canary(protocol: &mut CodexParser) -> String {
    completed_outcome(protocol.handle_message(json!({
        "method": "turn/completed",
        "params": {
            "threadId": "thread-1",
            "turn": {
                "id": "turn-1",
                "status": "completed",
                "items": [{"id": "agent-1", "type": "agentMessage", "text": "MKCANARY"}]
            }
        }
    })))
    .output
}

fn decline_only(effects: Vec<ProtocolEffect>) -> Value {
    assert_eq!(
        effects.len(),
        1,
        "unattended decline must not abort the turn"
    );
    match &effects[0] {
        ProtocolEffect::Send(response) => response.clone(),
        ProtocolEffect::Fail(_) => panic!("server request must not abort unattended dispatch"),
        ProtocolEffect::Complete(_) => panic!("server request must not complete the turn"),
    }
}

#[test]
fn command_approval_is_declined_and_the_turn_can_still_complete() {
    let mut protocol = active_turn();
    let response = decline_only(protocol.handle_message(json!({
        "id": "approval-1",
        "method": "item/commandExecution/requestApproval",
        "params": {
            "threadId": "thread-1",
            "turnId": "turn-1",
            "command": "sensitive command"
        }
    })));
    assert_eq!(response["id"], "approval-1");
    assert_eq!(response["result"]["decision"], "decline");
    assert!(response.get("error").is_none());
    assert_eq!(complete_canary(&mut protocol), "MKCANARY");
}

#[test]
fn mcp_elicitation_is_declined_without_a_target_diagnostic() {
    let mut protocol = active_turn();
    let response = decline_only(protocol.handle_message(json!({
        "id": "elicit-1",
        "method": "mcpServer/elicitation/request",
        "params": {
            "threadId": "thread-1",
            "turnId": "turn-1",
            "serverName": "land.lico.licoup.subagents",
            "mode": "form"
        }
    })));
    assert_eq!(response["result"]["action"], "decline");
    assert!(response["result"]["content"].is_null());
    assert_eq!(complete_canary(&mut protocol), "MKCANARY");
}

#[test]
fn unknown_server_request_is_rejected_without_aborting() {
    let mut protocol = active_turn();
    let response = decline_only(protocol.handle_message(json!({
        "id": "auth-refresh-1",
        "method": "account/chatgptAuthTokens/refresh",
        "params": { "threadId": "thread-1" }
    })));
    assert_eq!(response["id"], "auth-refresh-1");
    assert_eq!(response["error"]["code"], -32001);
    assert!(response.get("result").is_none());
    assert_eq!(complete_canary(&mut protocol), "MKCANARY");
}

#[test]
fn file_change_and_permission_requests_are_declined_in_band() {
    let mut protocol = active_turn();
    let file_change = decline_only(protocol.handle_message(json!({
        "id": "file-1",
        "method": "item/fileChange/requestApproval",
        "params": { "threadId": "thread-1", "turnId": "turn-1" }
    })));
    assert_eq!(file_change["result"]["decision"], "decline");
    let permissions = decline_only(protocol.handle_message(json!({
        "id": "perm-1",
        "method": "item/permissions/requestApproval",
        "params": { "threadId": "thread-1", "turnId": "turn-1" }
    })));
    assert_eq!(permissions["result"]["permissions"], json!([]));
    assert_eq!(complete_canary(&mut protocol), "MKCANARY");
}

#[test]
fn notification_methods_are_not_treated_as_server_requests() {
    let mut protocol = active_turn();
    assert!(
        sent_messages(protocol.handle_message(json!({
            "method": "item/started",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "item": { "id": "reason-1", "type": "reasoning", "summary": [] }
            }
        })))
        .is_empty()
    );
    assert_eq!(complete_canary(&mut protocol), "MKCANARY");
}
