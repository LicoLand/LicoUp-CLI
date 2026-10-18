use super::*;
use std::sync::{Arc, Mutex};

#[test]
fn live_process_continuation_cancel_cleanup_and_redaction_close_end_to_end() {
    let _serial = process_local_test_guard();
    let (directory, executable) = compile_fake_claude("lico-claude-execution");
    let executable_text = executable.to_string_lossy().to_string();
    let params = json!({
        "model": "fake-model",
        "reasoningEffort": "high",
        "permissionMode": "plan"
    });
    let first = execute(
        &executable_text,
        &params,
        "fake-claude-private-prompt-1",
        "",
        Some(&directory),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(first.ok, "first turn failed: {:?}", first.error);
    assert_eq!(first.output, "fake Claude final answer 1");
    assert!(matches!(
        first.transitions.last(),
        Some(licoup_agent_adapter_sdk::Transition::Lifecycle(
            licoup_agent_adapter_sdk::LifecycleStage::Completed
        ))
    ));
    assert_eq!(first.session_id, "fake-claude-session");
    assert!(has_live_session(&first.session_id));
    let second = execute(
        &executable_text,
        &json!({}),
        "fake-claude-private-prompt-2",
        &first.session_id,
        Some(&directory),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(second.ok, "second turn failed: {:?}", second.error);
    assert_eq!(second.output, "fake Claude final answer 2");
    assert_eq!(second.session_id, first.session_id);
    // The process-local transcript projection is Claude Code's own: the
    // package's `history` reads the complete transcript of the live transport
    // this turn bound, pages it backward and reports the scope, the exact
    // native session and its byte count. The host's lane adds its own `ok`
    // envelope and its own parameter parsing around this answer, and asserts
    // that side of the fold in the host's suite.
    let transcript = history(&second.session_id, None, 50)
        .expect("the live transport projects its own transcript");
    assert_eq!(transcript["continuityScope"], "process-local");
    assert_eq!(transcript["nativeSessionId"], second.session_id);
    assert_eq!(transcript["turnCount"], 2);
    assert_eq!(transcript["turns"].as_array().unwrap().len(), 2);
    assert_eq!(
        transcript
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        [
            "byteCount",
            "continuityScope",
            "hasMore",
            "nativeSessionId",
            "nextBefore",
            "turnCount",
            "turns"
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    );
    for turn in transcript["turns"].as_array().unwrap() {
        assert_eq!(
            turn.as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            ["events", "output", "prompt", "turnId"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );
    }
    assert_eq!(transcript["turns"][0]["turnId"], first.turn_id);
    assert_eq!(transcript["turns"][1]["turnId"], second.turn_id);
    assert_eq!(
        transcript["turns"][0]["output"],
        "fake Claude final answer 1"
    );
    assert_eq!(
        transcript["turns"][1]["output"],
        "fake Claude final answer 2"
    );
    assert_eq!(
        transcript["turns"][0]["prompt"],
        "fake-claude-private-prompt-1"
    );
    assert_eq!(
        transcript["turns"][1]["prompt"],
        "fake-claude-private-prompt-2"
    );
    assert_eq!(transcript["hasMore"], false);
    assert!(transcript["nextBefore"].is_null());
    let projected_bytes = transcript["turns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|turn| turn["output"].as_str().unwrap().len())
        .sum::<usize>();
    assert_eq!(transcript["byteCount"], json!(projected_bytes));
    // A conversation no live process owns has no process-local transcript. The
    // host's lane turns this `None` into its own
    // `claude_code_session_unavailable` refusal, which the host asserts.
    assert!(history("fake-claude-session-forged", None, 50).is_none());
    assert_eq!(
        cleanup_session(&second.session_id),
        ControlDisposition::Accepted
    );
    // Cleanup released the transport and cleared its transcript, so the exact
    // conversation is no longer projected.
    assert!(history(&second.session_id, None, 50).is_none());
    // After cleanup no process-local transport owns the conversation: a fresh
    // Claude Code process resumes the persisted transcript via --resume.
    let resumed = execute(
        &executable_text,
        &json!({
            "model": "fake-model",
            "reasoningEffort": "high",
            "permissionMode": "plan"
        }),
        "fake-claude-private-prompt-1",
        &second.session_id,
        Some(&directory),
        5_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(
        resumed.ok,
        "resume after cleanup failed: {:?}",
        resumed.error
    );
    assert_eq!(resumed.session_id, second.session_id);
    assert_eq!(resumed.output, "fake Claude final answer 1");
    assert!(has_live_session(&resumed.session_id));
    // Release the resumed transport so later fixture turns bind the shared
    // fixture session to their own fresh process.
    assert_eq!(
        cleanup_session(&resumed.session_id),
        ControlDisposition::Accepted
    );
    // An unknown conversation fails closed with the CLI's resume error surface.
    let missing = execute(
        &executable_text,
        &json!({
            "model": "fake-model",
            "reasoningEffort": "high",
            "permissionMode": "plan"
        }),
        "fake-claude-private-prompt-1",
        "missing-conversation",
        Some(&directory),
        5_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(!missing.ok);
    assert_eq!(missing.error.unwrap().code, "claude_code_exited");

    let working_dir = directory.clone();
    let executable_for_cancel = executable_text.clone();
    let run = thread::spawn(move || {
        execute(
            &executable_for_cancel,
            &json!({
                "model": "fake-model",
                "reasoningEffort": "high",
                "permissionMode": "plan"
            }),
            "fake-claude-cancel-prompt",
            "",
            Some(&working_dir),
            10_000,
            Some(1024 * 1024),
            1024,
        )
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let disposition = loop {
        let disposition = cancel("fake-claude-session");
        if disposition == ControlDisposition::Accepted || Instant::now() >= deadline {
            break disposition;
        }
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(disposition, ControlDisposition::Accepted);
    let cancelled = run.join().unwrap();
    let failure = cancelled.error.unwrap();
    assert_eq!(failure.code, "claude_code_turn_cancelled");
    assert_eq!(failure.stage, "turn/cancelled");
    assert_eq!(cancelled.turn_status, "cancelled");
    // A cancelled turn records no transcript turn while its transport is still
    // live.
    let cancelled_transcript = history("fake-claude-session", None, 50)
        .expect("a cancelled turn leaves its live transport in place");
    assert_eq!(cancelled_transcript["turnCount"], 0);
    assert_eq!(
        cleanup_session("fake-claude-session"),
        ControlDisposition::Accepted
    );
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn permission_denials_render_honestly_without_failing_the_turn() {
    let _serial = process_local_test_guard();
    let (directory, executable) = compile_fake_claude("lico-claude-denied");
    let executable_text = executable.to_string_lossy().to_string();
    let captured: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let events: Arc<Mutex<Vec<Value>>> = Arc::clone(&captured);
    licoup_foundation::platform::turn_event_emit::install_stream_sink(Box::new(move |event| {
        events.lock().unwrap().push(event);
    }));
    let params = json!({
        "model": "fake-model",
        "reasoningEffort": "high",
        "permissionMode": "plan"
    });
    let turn = execute(
        &executable_text,
        &params,
        "fake-claude-denied-prompt-1",
        "",
        Some(&directory),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    licoup_foundation::platform::turn_event_emit::clear_stream_sink();
    // The denied turn completes honestly instead of failing the client.
    assert!(turn.ok, "denied turn failed: {:?}", turn.error);
    let denied = captured
        .lock()
        .unwrap()
        .iter()
        .find(|event| event.get("event").and_then(Value::as_str) == Some("permission.denied"))
        .cloned()
        .expect("permission.denied event never emitted");
    assert_eq!(denied["payload"]["toolName"], "Bash");
    assert_eq!(denied["payload"]["toolUseId"], "toolu_denied");
    assert_eq!(
        cleanup_session(&turn.session_id),
        ControlDisposition::Accepted
    );
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn configuration_change_resumes_in_a_fresh_process_instead_of_failing() {
    let _serial = process_local_test_guard();
    let (directory, executable) = compile_fake_claude("lico-claude-config-switch");
    let executable_text = executable.to_string_lossy().to_string();
    let working_dir = directory.clone();
    let first = execute(
        &executable_text,
        &json!({
            "model": "fake-model",
            "reasoningEffort": "high",
            "permissionMode": "plan"
        }),
        "fake-claude-private-prompt-1",
        "",
        Some(&working_dir),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(first.ok, "first turn failed: {:?}", first.error);
    // Switching the reasoning effort is a launch-configuration change: the
    // pinned live process is released and a fresh process resumes the same
    // conversation with the new settings instead of failing the turn.
    let resumed = execute(
        &executable_text,
        &json!({
            "model": "fake-model",
            "reasoningEffort": "max",
            "permissionMode": "plan"
        }),
        "fake-claude-private-prompt-1",
        &first.session_id,
        Some(&directory),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(
        resumed.ok,
        "configuration switch failed: {:?}",
        resumed.error
    );
    assert_eq!(resumed.session_id, first.session_id);
    // A follow-up turn with the new configuration reuses the fresh process.
    let follow_up = execute(
        &executable_text,
        &json!({
            "model": "fake-model",
            "reasoningEffort": "max",
            "permissionMode": "plan"
        }),
        "fake-claude-private-prompt-2",
        &first.session_id,
        Some(&directory),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(follow_up.ok, "follow-up failed: {:?}", follow_up.error);
    assert_eq!(follow_up.session_id, first.session_id);
    assert_eq!(
        cleanup_session(&first.session_id),
        ControlDisposition::Accepted
    );
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn error_during_execution_with_reply_stays_successful() {
    let _serial = process_local_test_guard();
    let (directory, executable) = compile_fake_claude("lico-claude-error-execution");
    let executable_text = executable.to_string_lossy().to_string();
    let params = json!({
        "model": "fake-model",
        "reasoningEffort": "high",
        "permissionMode": "plan"
    });
    let turn = execute(
        &executable_text,
        &params,
        "fake-claude-error-execution-prompt-1",
        "",
        Some(&directory),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(turn.ok, "error_during_execution failed: {:?}", turn.error);
    assert_eq!(turn.output, "Reply despite a tool error");
    assert_eq!(
        cleanup_session(&turn.session_id),
        ControlDisposition::Accepted
    );
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn whole_assistant_messages_stream_progress_chunks() {
    let _serial = process_local_test_guard();
    let (directory, executable) = compile_fake_claude("lico-claude-whole-assistant");
    let executable_text = executable.to_string_lossy().to_string();
    let captured: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let events: Arc<Mutex<Vec<Value>>> = Arc::clone(&captured);
    licoup_foundation::platform::turn_event_emit::install_stream_sink(Box::new(move |event| {
        events.lock().unwrap().push(event);
    }));
    let params = json!({
        "model": "fake-model",
        "reasoningEffort": "high",
        "permissionMode": "plan"
    });
    let turn = execute(
        &executable_text,
        &params,
        "fake-claude-whole-assistant-prompt-1",
        "",
        Some(&directory),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    licoup_foundation::platform::turn_event_emit::clear_stream_sink();
    assert!(turn.ok, "whole-assistant turn failed: {:?}", turn.error);
    let chunks = captured
        .lock()
        .unwrap()
        .iter()
        .filter(|event| event.get("event").and_then(Value::as_str) == Some("agent.message.chunk"))
        .map(|event| {
            event
                .get("payload")
                .and_then(|payload| payload.get("text"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect::<Vec<_>>();
    assert!(
        chunks.contains(&"First round answer".to_string()),
        "first round text never chunked: {chunks:?}"
    );
    assert!(
        chunks.contains(&"Final round answer".to_string()),
        "final round text never chunked: {chunks:?}"
    );
    assert_eq!(
        cleanup_session(&turn.session_id),
        ControlDisposition::Accepted
    );
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn whole_assistant_messages_keep_distinct_units_without_replayed_text() {
    let _serial = process_local_test_guard();
    let (directory, executable) = compile_fake_claude("lico-claude-segmented-assistant");
    let executable_text = executable.to_string_lossy().to_string();
    let captured: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let events: Arc<Mutex<Vec<Value>>> = Arc::clone(&captured);
    licoup_foundation::platform::turn_event_emit::install_stream_sink(Box::new(move |event| {
        events.lock().unwrap().push(event);
    }));
    let params = json!({
        "model": "fake-model",
        "reasoningEffort": "high",
        "permissionMode": "plan"
    });
    let turn = execute(
        &executable_text,
        &params,
        "fake-claude-segmented-assistant-prompt-1",
        "",
        Some(&directory),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    licoup_foundation::platform::turn_event_emit::clear_stream_sink();
    assert!(turn.ok, "segmented assistant turn failed: {:?}", turn.error);
    let events = captured.lock().unwrap();
    let chunks = events
        .iter()
        .filter(|event| event["event"] == "agent.message.chunk")
        .map(|event| {
            (
                event["payload"]["messageUnit"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                event["payload"]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        chunks,
        vec![
            ("1".to_owned(), "第一段".to_owned()),
            ("2".to_owned(), "第二段".to_owned()),
            ("3".to_owned(), "第三段".to_owned()),
        ]
    );
    let completed = events
        .iter()
        .find(|event| event["event"] == "agent.message.completed")
        .expect("completed message event");
    assert_eq!(completed["payload"]["messageUnit"], "3");
    assert_eq!(completed["payload"]["text"], "第三段");
    drop(events);
    assert_eq!(
        cleanup_session(&turn.session_id),
        ControlDisposition::Accepted
    );
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn terminal_only_final_message_gets_its_own_unit() {
    let _serial = process_local_test_guard();
    let (directory, executable) = compile_fake_claude("lico-claude-terminal-segment");
    let executable_text = executable.to_string_lossy().to_string();
    let captured: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let events: Arc<Mutex<Vec<Value>>> = Arc::clone(&captured);
    licoup_foundation::platform::turn_event_emit::install_stream_sink(Box::new(move |event| {
        events.lock().unwrap().push(event);
    }));
    let turn = execute(
        &executable_text,
        &json!({"model": "fake-model", "permissionMode": "plan"}),
        "fake-claude-terminal-segment-prompt-1",
        "",
        Some(&directory),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    licoup_foundation::platform::turn_event_emit::clear_stream_sink();
    assert!(turn.ok, "terminal segment turn failed: {:?}", turn.error);
    let events = captured.lock().unwrap();
    let chunk = events
        .iter()
        .find(|event| event["event"] == "agent.message.chunk")
        .expect("first segment chunk");
    assert_eq!(chunk["payload"]["messageUnit"], "1");
    assert_eq!(chunk["payload"]["text"], "First segment");
    let completed = events
        .iter()
        .find(|event| event["event"] == "agent.message.completed")
        .expect("terminal segment completed");
    assert_eq!(completed["payload"]["messageUnit"], "2");
    assert_eq!(completed["payload"]["text"], "Final segment");
    drop(events);
    assert_eq!(
        cleanup_session(&turn.session_id),
        ControlDisposition::Accepted
    );
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn native_stream_input_accepts_guidance_during_the_active_turn() {
    let _serial = process_local_test_guard();
    let (directory, executable) = compile_fake_claude("lico-claude-native-steer");
    let executable = executable.to_string_lossy().to_string();
    let working_directory = directory.clone();
    let run = thread::spawn(move || {
        execute(
            &executable,
            &json!({
                "model": "fake-model",
                "reasoningEffort": "high",
                "permissionMode": "plan"
            }),
            "fake-claude-steer-prompt",
            "",
            Some(&working_directory),
            10_000,
            Some(1024 * 1024),
            1024,
        )
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let disposition = loop {
        let disposition = steer("fake-claude-session", "fake-claude-steer-guidance");
        if disposition == ControlDisposition::Accepted || Instant::now() >= deadline {
            break disposition;
        }
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(disposition, ControlDisposition::Accepted);
    let result = run.join().unwrap();
    assert!(result.ok, "native steer turn failed: {:?}", result.error);
    assert_eq!(result.output, "fake Claude guided answer");
    assert_eq!(
        cleanup_session(&result.session_id),
        ControlDisposition::Accepted
    );
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn output_overflow_fails_closed_without_recording_a_successful_turn() {
    let _serial = process_local_test_guard();
    let (directory, executable) = compile_fake_claude("lico-claude-output-overflow");
    let result = execute(
        &executable.to_string_lossy(),
        &json!({
            "model": "fake-model",
            "reasoningEffort": "high",
            "permissionMode": "plan"
        }),
        "fake-claude-private-prompt-1",
        "",
        Some(&directory),
        10_000,
        Some(32),
        1024,
    );
    assert!(!result.ok);
    assert!(result.stdout_truncated);
    assert_eq!(result.error.unwrap().code, "claude_code_output_limit");
    assert!(!has_live_session("fake-claude-session"));
    // The overflow released the transport, so no process-local transcript is
    // projected for it.
    assert!(history("fake-claude-session", None, 50).is_none());
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn successful_utf8_output_history_reports_exact_encoded_byte_count() {
    let _serial = process_local_test_guard();
    let (directory, executable) = compile_fake_claude("lico-claude-utf8-history");
    let result = execute(
        &executable.to_string_lossy(),
        &json!({
            "model": "fake-model",
            "reasoningEffort": "high",
            "permissionMode": "plan"
        }),
        "fake-claude-private-prompt-1 fake-claude-utf8-output",
        "",
        Some(&directory),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    let expected = "多字节🙂";
    assert!(result.ok, "UTF-8 fixture turn failed: {:?}", result.error);
    assert_eq!(result.output, expected);
    assert_ne!(expected.as_bytes().len(), expected.chars().count());

    let transcript = history(&result.session_id, None, 50)
        .expect("the live transport projects its own transcript");
    assert_eq!(transcript["turnCount"], 1);
    assert_eq!(transcript["turns"][0]["turnId"], result.turn_id);
    assert_eq!(transcript["turns"][0]["output"], expected);
    assert_eq!(transcript["byteCount"], json!(expected.as_bytes().len()));
    assert_eq!(
        cleanup_session(result.session_id.as_str()),
        ControlDisposition::Accepted
    );
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn authentication_failure_is_a_stable_redacted_blocker_and_closes_the_transport() {
    let _serial = process_local_test_guard();
    let (directory, executable) = compile_fake_claude("lico-claude-authentication");
    let result = execute(
        &executable.to_string_lossy(),
        &json!({
            "model": "fake-model",
            "reasoningEffort": "high",
            "permissionMode": "plan"
        }),
        "fake-claude-auth-prompt",
        "",
        Some(&directory),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(!result.ok);
    let failure = result.error.unwrap();
    assert_eq!(failure.code, "claude_code_authentication_required");
    assert!(
        !failure
            .message
            .contains("synthetic private authentication detail")
    );
    let directory_text = directory.to_string_lossy();
    assert!(!failure.message.contains(directory_text.as_ref()));
    assert!(!has_live_session("fake-claude-session"));
    let _ = fs::remove_dir_all(directory);
}
