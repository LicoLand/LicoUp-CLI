use super::support::{
    completed_outcome, config, failed_effect, initialize, open_thread, sent_messages, start_turn,
};
use crate::app_server::config::ProtocolConfig;
use crate::app_server::limits::{
    ACCOUNT_RATE_LIMITS_REQUEST_ID, INITIALIZE_REQUEST_ID, THREAD_REQUEST_ID,
    THREAD_UNARCHIVE_REQUEST_ID,
};
use crate::app_server::model::ProtocolEffect;
use crate::parser::CodexParser;
use crate::port::turn_event::{TestSinkGuard, install_test_sink};
use serde_json::{Map, Value, json};
use std::path::Path;
use std::sync::{Arc, Mutex};

fn resume_protocol(thread_id: &str) -> CodexParser {
    let mut protocol = CodexParser::new(config(json!({}), "hello", thread_id));
    let messages = sent_messages(initialize(&mut protocol));
    assert_eq!(messages[1]["method"], "thread/resume");
    assert_eq!(messages[1]["params"]["threadId"], thread_id);
    protocol
}

fn thread_open_response(thread_id: &str) -> Value {
    json!({
        "id": THREAD_REQUEST_ID,
        "result": {
            "thread": {"id": thread_id, "cwd": "/workspace/project"},
            "cwd": "/workspace/project"
        }
    })
}

fn initialize_response() -> Value {
    json!({
        "id": INITIALIZE_REQUEST_ID,
        "result": {
            "userAgent": "codex-test",
            "platformFamily": "test",
            "platformOs": "test",
            "codexHome": "/redacted"
        }
    })
}

fn complete_current_turn(protocol: &mut CodexParser, thread_id: &str) {
    start_turn(protocol);
    let outcome = completed_outcome(protocol.handle_message(json!({
        "method": "turn/completed",
        "params": {
            "threadId": thread_id,
            "turn": {
                "id": "turn-1",
                "status": "completed",
                "items": [{"id": "agent-1", "type": "agentMessage", "text": "done"}]
            }
        }
    })));
    assert_eq!(outcome.session_id, thread_id);
    assert_eq!(outcome.thread_id, thread_id);
}

#[test]
fn empty_prompt_resume_completes_without_starting_a_turn() {
    let mut protocol = CodexParser::new(config(json!({}), "", "exact-thread"));
    let messages = sent_messages(initialize(&mut protocol));
    assert_eq!(messages[1]["method"], "thread/resume");
    assert_eq!(messages[1]["params"]["threadId"], "exact-thread");
    let outcome = completed_outcome(protocol.handle_message(thread_open_response("exact-thread")));
    assert_eq!(outcome.session_id, "exact-thread");
    assert_eq!(outcome.thread_id, "exact-thread");
    assert_eq!(outcome.turn_status, "resumed");
    assert!(outcome.turn_id.is_empty());
}

#[test]
fn new_thread_sends_prompt_only_in_turn_start_stdio_message() {
    let prompt = "private prompt that must not enter argv";
    let mut protocol = CodexParser::new(config(
        json!({"model": "explicit-model", "reasoningEffort": "high"}),
        prompt,
        "",
    ));

    let initialize_request = protocol.initial_request();
    assert_eq!(initialize_request["method"], "initialize");
    assert!(!initialize_request.to_string().contains(prompt));

    let thread_messages = sent_messages(initialize(&mut protocol));
    assert_eq!(thread_messages[0], json!({"method": "initialized"}));
    assert_eq!(thread_messages[1]["method"], "thread/start");
    assert!(!thread_messages[1].to_string().contains(prompt));
    assert!(thread_messages[1]["params"].get("sandbox").is_none());

    let turn_messages = sent_messages(open_thread(&mut protocol));
    assert_eq!(turn_messages.len(), 1);
    assert_eq!(turn_messages[0]["method"], "turn/start");
    assert_eq!(turn_messages[0]["params"]["threadId"], "thread-1");
    assert_eq!(turn_messages[0]["params"]["input"][0]["type"], "text");
    assert_eq!(turn_messages[0]["params"]["input"][0]["text"], prompt);
    assert_eq!(turn_messages[0]["params"]["model"], "explicit-model");
    assert_eq!(turn_messages[0]["params"]["effort"], "high");
}

#[test]
fn luna_reserve_capability_read_routes_an_eligible_turn_without_changing_effective_settings() {
    let mut protocol = CodexParser::new(config(
        json!({"model": "gpt-5.6-luna", "reasoningEffort": "high"}),
        "hello",
        "",
    ));

    let initialized = sent_messages(protocol.handle_message(initialize_response()));
    assert_eq!(initialized.len(), 2);
    assert_eq!(initialized[0], json!({"method": "initialized"}));
    assert_eq!(initialized[1]["method"], "account/rateLimits/read");
    assert_eq!(initialized[1]["params"]["supportsLunaReserve"], true);

    let thread_messages = sent_messages(protocol.handle_message(json!({
        "id": ACCOUNT_RATE_LIMITS_REQUEST_ID,
        "result": {
            "ordinaryUsageAllowed": false,
            "rateLimits": {"primary": {"usedPercent": 100.0}},
            "rateLimitsByLimitId": {
                "base_model_inference": {
                    "limitId": "base_model_inference",
                    "limitName": "gpt-reserve",
                    "normalModelSlug": "gpt-5.6-luna",
                    "primary": {"usedPercent": 48.0}
                }
            },
            "rateLimitUpsell": {
                "banner_type": "luna_reserve",
                "blocked_model_slug": "gpt-5.6-luna"
            }
        }
    })));
    assert_eq!(thread_messages[0]["method"], "thread/start");

    let turn_messages = sent_messages(open_thread(&mut protocol));
    assert_eq!(turn_messages[0]["params"]["model"], "gpt-reserve");
    assert_eq!(turn_messages[0]["params"]["effort"], "high");

    start_turn(&mut protocol);
    let outcome = completed_outcome(protocol.handle_message(json!({
        "method": "turn/completed",
        "params": {
            "threadId": "thread-1",
            "turn": {
                "id": "turn-1",
                "status": "completed",
                "items": [{"id": "agent-1", "type": "agentMessage", "text": "done"}]
            }
        }
    })));
    assert_eq!(outcome.effective.model.as_deref(), Some("gpt-5.6-luna"));
    assert_eq!(outcome.effective.reasoning_effort.as_deref(), Some("high"));
}

#[test]
fn luna_reserve_capability_read_accepts_legacy_response_without_ordinary_usage_permission() {
    let mut protocol = CodexParser::new(config(json!({"model": "gpt-5.6-luna"}), "hello", ""));
    sent_messages(protocol.handle_message(initialize_response()));

    let thread_messages = sent_messages(protocol.handle_message(json!({
        "id": ACCOUNT_RATE_LIMITS_REQUEST_ID,
        "result": {
            "rateLimits": {
                "primary": {"rateLimitReachedType": "rate_limit_reached"}
            },
            "rateLimitsByLimitId": {
                "base_model_inference": {
                    "limitName": "gpt-reserve",
                    "primary": {"usedPercent": 0.0}
                }
            },
            "rateLimitUpsell": {"banner_type": "luna_reserve"}
        }
    })));
    assert_eq!(thread_messages[0]["method"], "thread/start");

    let turn_messages = sent_messages(open_thread(&mut protocol));
    assert_eq!(turn_messages[0]["params"]["model"], "gpt-reserve");
}

#[test]
fn ordinary_account_response_keeps_luna_on_the_normal_route() {
    let mut protocol = CodexParser::new(config(
        json!({"model": "gpt-5.6-luna", "reasoningEffort": "medium"}),
        "hello",
        "",
    ));
    sent_messages(protocol.handle_message(initialize_response()));
    let thread_messages = sent_messages(protocol.handle_message(json!({
        "id": ACCOUNT_RATE_LIMITS_REQUEST_ID,
        "result": {
            "ordinaryUsageAllowed": true,
            "rateLimits": {"primary": {"usedPercent": 12.0}}
        }
    })));
    assert_eq!(thread_messages[0]["method"], "thread/start");
    let turn_messages = sent_messages(open_thread(&mut protocol));
    assert_eq!(turn_messages[0]["params"]["model"], "gpt-5.6-luna");
}

#[test]
fn older_app_server_rejecting_capability_params_is_retried_with_null_params() {
    let mut protocol = CodexParser::new(config(json!({"model": "gpt-5.6-luna"}), "hello", ""));
    sent_messages(protocol.handle_message(initialize_response()));

    let retry = sent_messages(protocol.handle_message(json!({
        "id": ACCOUNT_RATE_LIMITS_REQUEST_ID,
        "error": {"code": -32602, "message": "invalid params"}
    })));
    assert_eq!(retry.len(), 1);
    assert_eq!(retry[0]["method"], "account/rateLimits/read");
    assert!(retry[0]["params"].is_null());

    let thread_messages = sent_messages(protocol.handle_message(json!({
        "id": ACCOUNT_RATE_LIMITS_REQUEST_ID,
        "result": {
            "ordinaryUsageAllowed": false,
            "rateLimits": {"primary": {"usedPercent": 100.0}}
        }
    })));
    assert_eq!(thread_messages[0]["method"], "thread/start");
    let turn_messages = sent_messages(open_thread(&mut protocol));
    assert_eq!(turn_messages[0]["params"]["model"], "gpt-5.6-luna");
}

#[test]
fn private_instructions_use_native_developer_channel_without_changing_prompt() {
    let prompt = "exact user prompt";
    let private = "synthetic private instruction";
    let mut protocol =
        CodexParser::new(config(json!({"privateInstructions": private}), prompt, ""));
    let thread_messages = sent_messages(initialize(&mut protocol));
    assert_eq!(
        thread_messages[1]["params"]["developerInstructions"],
        private
    );
    assert!(!thread_messages[1].to_string().contains(prompt));
    let turn_messages = sent_messages(open_thread(&mut protocol));
    assert_eq!(turn_messages[0]["params"]["input"][0]["text"], prompt);
    assert!(!turn_messages[0].to_string().contains(private));
}

#[test]
fn archived_thread_unarchives_then_resumes_the_same_native_identity() {
    let thread_id = "archived-thread-id";
    let mut protocol = resume_protocol(thread_id);

    let effects = protocol.handle_message(json!({
        "id": THREAD_REQUEST_ID,
        "error": {
            "code": -32600,
            "message": "session archived-thread-id is archived"
        }
    }));
    let unarchive = sent_messages(effects);
    assert_eq!(unarchive.len(), 1);
    assert_eq!(unarchive[0]["id"], THREAD_UNARCHIVE_REQUEST_ID);
    assert_eq!(unarchive[0]["method"], "thread/unarchive");
    assert_eq!(unarchive[0]["params"]["threadId"], thread_id);

    let resumed = sent_messages(protocol.handle_message(json!({
        "id": THREAD_UNARCHIVE_REQUEST_ID,
        "result": {"thread": {"id": thread_id}}
    })));
    assert_eq!(resumed.len(), 1);
    assert_eq!(resumed[0]["method"], "thread/resume");
    assert_eq!(resumed[0]["params"]["threadId"], thread_id);

    let turn_messages = sent_messages(protocol.handle_message(thread_open_response(thread_id)));
    assert_eq!(turn_messages.len(), 1);
    assert_eq!(turn_messages[0]["method"], "turn/start");
    assert_eq!(turn_messages[0]["params"]["threadId"], thread_id);
    complete_current_turn(&mut protocol, thread_id);
}

#[test]
fn exact_missing_rollout_fails_without_starting_a_replacement_thread() {
    let stale_thread_id = "stale-thread-id";
    let mut protocol = resume_protocol(stale_thread_id);

    let effects = protocol.handle_message(json!({
        "id": THREAD_REQUEST_ID,
        "error": {
            "code": -32600,
            "message": "no rollout found for thread id stale-thread-id"
        }
    }));
    assert!(
        effects
            .iter()
            .all(|effect| !matches!(effect, ProtocolEffect::Send(_)))
    );
    let failure = failed_effect(effects);
    assert_eq!(failure.code, "codex_thread_open_failed");
    assert_eq!(failure.stage, "thread/resume");
    assert_eq!(failure.session_id.as_deref(), Some(stale_thread_id));
}

#[test]
fn unrelated_resume_errors_never_trigger_recovery_requests() {
    for error_message in [
        "thread not found: existing-thread-id",
        "unknown thread existing-thread-id",
        "no such thread existing-thread-id",
        "thread existing-thread-id does not exist",
        "missing thread existing-thread-id",
        "no rollout found for thread id existing-thread-id; retry later",
        "session another-thread is archived. Run `codex unarchive another-thread` to unarchive it first.",
        "approval policy rejected",
    ] {
        let mut protocol = resume_protocol("existing-thread-id");
        let effects = protocol.handle_message(json!({
            "id": THREAD_REQUEST_ID,
            "error": {"code": -32001, "message": error_message}
        }));
        assert!(
            sent_messages(effects).is_empty(),
            "unexpected recovery for {error_message}"
        );

        let mut protocol = resume_protocol("existing-thread-id");
        let failure = failed_effect(protocol.handle_message(json!({
            "id": THREAD_REQUEST_ID,
            "error": {"code": -32001, "message": error_message}
        })));
        assert_eq!(failure.code, "codex_thread_open_failed");
        assert_eq!(failure.stage, "thread/resume");
        assert_eq!(failure.session_id.as_deref(), Some("existing-thread-id"));
        assert_eq!(failure.thread_id.as_deref(), Some("existing-thread-id"));
    }
}

#[test]
fn rejected_unarchive_keeps_the_binding_and_sends_no_extra_request() {
    let mut protocol = resume_protocol("archived-thread-id");
    sent_messages(protocol.handle_message(json!({
        "id": THREAD_REQUEST_ID,
        "error": {
            "code": -32600,
            "message": "session archived-thread-id is archived. Run `codex unarchive archived-thread-id` to unarchive it first."
        }
    })));

    let effects = protocol.handle_message(json!({
        "id": THREAD_UNARCHIVE_REQUEST_ID,
        "error": {"code": -32601, "message": "Method not found"}
    }));
    assert!(sent_messages(effects).is_empty());

    let mut protocol = resume_protocol("archived-thread-id");
    sent_messages(protocol.handle_message(json!({
        "id": THREAD_REQUEST_ID,
        "error": {
            "code": -32600,
            "message": "session archived-thread-id is archived. Run `codex unarchive archived-thread-id` to unarchive it first."
        }
    })));
    let failure = failed_effect(protocol.handle_message(json!({
        "id": THREAD_UNARCHIVE_REQUEST_ID,
        "error": {"code": -32601, "message": "Method not found"}
    })));
    assert_eq!(failure.code, "codex_thread_unarchive_failed");
    assert_eq!(failure.stage, "thread/unarchive");
    assert_eq!(failure.session_id.as_deref(), Some("archived-thread-id"));
    assert_eq!(failure.thread_id.as_deref(), Some("archived-thread-id"));
}

#[test]
fn mismatched_unarchive_identity_fails_without_resume_or_start() {
    let mut protocol = resume_protocol("archived-thread-id");
    sent_messages(protocol.handle_message(json!({
        "id": THREAD_REQUEST_ID,
        "error": {
            "code": -32600,
            "message": "session archived-thread-id is archived. Run `codex unarchive archived-thread-id` to unarchive it first."
        }
    })));

    let effects = protocol.handle_message(json!({
        "id": THREAD_UNARCHIVE_REQUEST_ID,
        "result": {"thread": {"id": "different-thread-id"}}
    }));
    let sends = effects
        .iter()
        .filter(|effect| matches!(effect, ProtocolEffect::Send(_)))
        .count();
    assert_eq!(sends, 0);
    let failure = failed_effect(effects);
    assert_eq!(failure.code, "codex_thread_unarchive_identity_mismatch");
    assert_eq!(failure.stage, "thread/unarchive");
}

#[test]
fn archived_resume_recovery_is_attempted_only_once() {
    let thread_id = "archived-thread-id";
    let archived_error = json!({
        "id": THREAD_REQUEST_ID,
        "error": {
            "code": -32600,
            "message": "session archived-thread-id is archived"
        }
    });
    let mut protocol = resume_protocol(thread_id);
    assert_eq!(
        sent_messages(protocol.handle_message(archived_error.clone()))[0]["method"],
        "thread/unarchive"
    );
    assert_eq!(
        sent_messages(protocol.handle_message(json!({
            "id": THREAD_UNARCHIVE_REQUEST_ID,
            "result": {"thread": {"id": thread_id}}
        })))[0]["method"],
        "thread/resume"
    );

    let effects = protocol.handle_message(archived_error);
    let sends = effects
        .iter()
        .filter(|effect| matches!(effect, ProtocolEffect::Send(_)))
        .count();
    assert_eq!(sends, 0);
    let failure = failed_effect(effects);
    assert_eq!(failure.code, "codex_thread_open_failed");
    assert_eq!(failure.stage, "thread/resume");
}

#[test]
fn resumed_thread_must_return_the_requested_native_identity() {
    let mut protocol = resume_protocol("requested-thread-id");
    let effects = protocol.handle_message(thread_open_response("different-thread-id"));
    let sends = effects
        .iter()
        .filter(|effect| matches!(effect, ProtocolEffect::Send(_)))
        .count();
    assert_eq!(sends, 0);
    let failure = failed_effect(effects);
    assert_eq!(failure.code, "codex_thread_resume_identity_mismatch");
    assert_eq!(failure.stage, "thread/resume");
    assert_eq!(failure.session_id.as_deref(), Some("requested-thread-id"));
    assert_eq!(failure.thread_id.as_deref(), Some("requested-thread-id"));
}

#[test]
fn resume_accepts_session_path_aliases_only_with_record_identity() {
    let dir = std::env::temp_dir().join(format!("codex-session-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rollout-filename-identity.jsonl");
    std::fs::write(
        &path,
        r#"{"type":"session_meta","payload":{"id":"record-identity"}}
"#,
    )
    .unwrap();
    for key in ["sessionPath", "sourcePath"] {
        let mut params = Map::new();
        params.insert(key.to_string(), json!(path));
        let mut protocol = CodexParser::new(config(Value::Object(params), "hello", ""));
        let thread_messages = sent_messages(initialize(&mut protocol));
        let resume = &thread_messages[1];
        assert_eq!(resume["method"], "thread/resume");
        assert_eq!(resume["params"]["threadId"], "record-identity");
        assert!(resume["params"]["path"].as_str().is_some());
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn turn_start_ack_emits_accepted_lifecycle_receipt() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let sink_target = Arc::clone(&captured);
    install_test_sink(Box::new(move |event| {
        sink_target.lock().unwrap().push(event);
    }));
    let _guard = TestSinkGuard;
    let mut protocol = CodexParser::new(config(json!({}), "hello", ""));
    initialize(&mut protocol);
    open_thread(&mut protocol);
    super::support::start_turn(&mut protocol);

    let events = captured.lock().unwrap().clone();
    assert!(events.iter().any(|event| {
        event["event"] == "agent.turn.accepted"
            && event["sessionId"] == "thread-1"
            && event["turnId"] == "turn-1"
    }));
}

#[test]
fn turn_start_emits_ordered_text_then_local_image_inputs() {
    let prompt = "describe the attachments";
    let mut protocol = CodexParser::new(config(
        json!({
            "attachments": [
                {
                    "id": "sel-1",
                    "name": "first.png",
                    "mediaType": "image/png",
                    "path": "attachment-fixtures/first.png"
                },
                {
                    "id": "sel-2",
                    "name": "second.jpg",
                    "mediaType": "image/jpeg",
                    "path": "attachment-fixtures/second.jpg"
                }
            ]
        }),
        prompt,
        "",
    ));

    let thread_messages = sent_messages(initialize(&mut protocol));
    assert_eq!(thread_messages[0], json!({"method": "initialized"}));
    assert_eq!(thread_messages[1]["method"], "thread/start");

    let turn_messages = sent_messages(open_thread(&mut protocol));
    assert_eq!(turn_messages.len(), 1);
    assert_eq!(turn_messages[0]["method"], "turn/start");
    let input = turn_messages[0]["params"]["input"].as_array().unwrap();
    assert_eq!(input.len(), 3);
    assert_eq!(input[0], json!({"type": "text", "text": prompt}));
    assert_eq!(
        input[1],
        json!({
            "type": "localImage",
            "path": "attachment-fixtures/first.png",
            "mediaType": "image/png",
            "name": "first.png"
        })
    );
    assert_eq!(
        input[2],
        json!({
            "type": "localImage",
            "path": "attachment-fixtures/second.jpg",
            "mediaType": "image/jpeg",
            "name": "second.jpg"
        })
    );
}

#[test]
fn attachment_only_turn_start_contains_no_text_item() {
    let mut protocol = CodexParser::new(config(
        json!({
            "attachments": [
                {
                    "id": "sel-1",
                    "name": "first.png",
                    "mediaType": "image/png",
                    "path": "attachment-fixtures/first.png"
                }
            ]
        }),
        "",
        "",
    ));

    initialize(&mut protocol);
    let turn_messages = sent_messages(open_thread(&mut protocol));
    let input = turn_messages[0]["params"]["input"].as_array().unwrap();
    assert_eq!(input.len(), 1);
    assert_eq!(
        input[0],
        json!({
            "type": "localImage",
            "path": "attachment-fixtures/first.png",
            "mediaType": "image/png",
            "name": "first.png"
        })
    );
}

#[test]
fn invalid_attachment_config_fails_closed_without_paths() {
    for params in [
        json!({"attachments": "not-an-array"}),
        json!({"attachments": [{"id": "1", "name": "a.png", "mediaType": "image/png"}]}),
        json!({
            "attachments": [{
                "id": "1",
                "name": "a.png",
                "mediaType": "image/png",
                "path": "attachment-fixtures/a.png",
                "extra": true
            }]
        }),
        json!({
            "attachments": [{
                "id": "1",
                "name": "a.heic",
                "mediaType": "image/heic",
                "path": "attachment-fixtures/a.heic"
            }]
        }),
        json!({
            "attachments": [
                {"id": "1", "name": "a.png", "mediaType": "image/png", "path": "attachment-fixtures/a.png"},
                {"id": "2", "name": "b.png", "mediaType": "image/png", "path": "attachment-fixtures/b.png"},
                {"id": "3", "name": "c.png", "mediaType": "image/png", "path": "attachment-fixtures/c.png"},
                {"id": "4", "name": "d.png", "mediaType": "image/png", "path": "attachment-fixtures/d.png"},
                {"id": "5", "name": "e.png", "mediaType": "image/png", "path": "attachment-fixtures/e.png"}
            ]
        }),
    ] {
        let failure = ProtocolConfig::from_params(
            &params,
            "hello",
            "",
            Some(Path::new("/workspace/project")),
        )
        .expect_err("invalid attachment config must fail closed");
        assert_eq!(failure.code, "codex_invalid_local_image");
        assert!(!failure.message.contains("attachment-fixtures"));
    }
}
