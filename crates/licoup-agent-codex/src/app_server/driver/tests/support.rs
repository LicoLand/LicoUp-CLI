use crate::app_server::config::ProtocolConfig;
use crate::app_server::limits::{
    ACCOUNT_RATE_LIMITS_REQUEST_ID, INITIALIZE_REQUEST_ID, THREAD_REQUEST_ID, TURN_REQUEST_ID,
};
use crate::app_server::model::{ProtocolEffect, ProtocolFailure, ProtocolOutcome};
use crate::parser::CodexParser;
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn config(params: Value, prompt: &str, session_id: &str) -> ProtocolConfig {
    ProtocolConfig::from_params(
        &params,
        prompt,
        session_id,
        Some(Path::new("/workspace/project")),
    )
    .unwrap()
}

pub(super) fn initialize(protocol: &mut CodexParser) -> Vec<ProtocolEffect> {
    let mut effects = protocol.handle_message(json!({
        "id": INITIALIZE_REQUEST_ID,
        "result": {
            "userAgent": "codex-test",
            "platformFamily": "test",
            "platformOs": "test",
            "codexHome": "/redacted"
        }
    }));
    // Default/Luna configurations now perform the account capability read before opening a
    // thread. Feeding a normal response here keeps existing protocol fixtures focused on their
    // thread behavior while dedicated tests can inspect the preflight request directly.
    effects.extend(protocol.handle_message(json!({
        "id": ACCOUNT_RATE_LIMITS_REQUEST_ID,
        "result": {"rateLimits": {"primary": {"usedPercent": 0.0}}}
    })));
    effects.retain(|effect| match effect {
        ProtocolEffect::Send(message) => {
            message.get("method").and_then(Value::as_str) != Some("account/rateLimits/read")
        }
        ProtocolEffect::Complete(_) | ProtocolEffect::Fail(_) => true,
    });
    effects
}

pub(super) fn open_thread(protocol: &mut CodexParser) -> Vec<ProtocolEffect> {
    protocol.handle_message(json!({
        "id": THREAD_REQUEST_ID,
        "result": {
            "thread": {
                "id": "thread-1",
                "sessionId": "non-authoritative-session",
                "cwd": "/workspace/project"
            },
            "cwd": "/workspace/project",
            "model": "non-authoritative-default-model",
            "reasoningEffort": "non-authoritative-medium",
            "sandbox": {"type": "workspaceWrite", "writableRoots": []},
            "approvalPolicy": "on-request"
        }
    }))
}

pub(super) fn start_turn(protocol: &mut CodexParser) {
    let effects = protocol.handle_message(json!({
        "id": TURN_REQUEST_ID,
        "result": {
            "turn": {"id": "turn-1", "status": "inProgress", "items": []}
        }
    }));
    assert!(effects.is_empty());
}

pub(super) fn sent_messages(effects: Vec<ProtocolEffect>) -> Vec<Value> {
    effects
        .into_iter()
        .filter_map(|effect| match effect {
            ProtocolEffect::Send(message) => Some(message),
            ProtocolEffect::Complete(_) | ProtocolEffect::Fail(_) => None,
        })
        .collect()
}

pub(super) fn completed_outcome(effects: Vec<ProtocolEffect>) -> ProtocolOutcome {
    effects
        .into_iter()
        .find_map(|effect| match effect {
            ProtocolEffect::Complete(outcome) => Some(*outcome),
            ProtocolEffect::Send(_) | ProtocolEffect::Fail(_) => None,
        })
        .expect("matching completion should finish the protocol")
}

pub(super) fn failed_effect(effects: Vec<ProtocolEffect>) -> ProtocolFailure {
    effects
        .into_iter()
        .find_map(|effect| match effect {
            ProtocolEffect::Fail(failure) => Some(failure),
            ProtocolEffect::Send(_) | ProtocolEffect::Complete(_) => None,
        })
        .expect("matching failure should finish the protocol")
}
