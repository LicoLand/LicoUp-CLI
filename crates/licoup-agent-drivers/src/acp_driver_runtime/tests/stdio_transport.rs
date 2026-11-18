use super::super::stdio_transport::run_protocol_loop;
use super::*;
use std::time::{Duration, Instant};

pub fn compile_fake_agent(label: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("lico-acp-{label}-{}", timestamp()));
    fs::create_dir_all(&dir).unwrap();
    let source = dir.join("fake_agent.rs");
    let executable = dir.join(format!("fake-agent{}", std::env::consts::EXE_SUFFIX));
    fs::write(&source, FAKE_AGENT_SOURCE).unwrap();
    let status = Command::new("rustc")
        .args(["--edition", "2021"])
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .status()
        .unwrap();
    assert!(status.success());
    (dir, executable)
}

fn protocol_awaiting_prompt_response() -> AcpProtocol {
    let mut protocol = new_protocol(json!({}), "private", "");
    let effects = protocol.handle_message(initialize_response(true, true));
    assert!(matches!(effects[0], ProtocolEffect::Send(_)));
    let effects = protocol.handle_message(json!({
        "jsonrpc": "2.0",
        "id": SESSION_REQUEST_ID,
        "result": {"sessionId": "native-session", "configOptions": []}
    }));
    let ProtocolEffect::Send(prompt) = &effects[0] else {
        panic!("expected prompt request")
    };
    assert_eq!(prompt["method"], "session/prompt");
    assert_eq!(protocol.phase, ProtocolPhase::AwaitPrompt);
    protocol
}

fn prompt_response() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": PROMPT_REQUEST_ID,
        "result": {"stopReason": "end_turn"}
    })
}

fn agent_chunk(content: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": "native-session",
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": content
            }
        }
    })
}

#[test]
fn fake_child_transport_emits_ordered_chunks_before_protocol_finish() {
    use licoup_foundation::platform::raw_execution::{
        RawExecutionDirection, RawExecutionObserver, RawExecutionScope,
    };
    use std::sync::{Arc, Mutex};

    let (dir, executable) = compile_fake_agent("response-first");
    let records = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&records);
    let _scope = RawExecutionScope::enter(Some(RawExecutionObserver::new(
        move |source, direction, text| {
            sink.lock()
                .unwrap()
                .push((source.to_owned(), direction, text.to_owned()));
            Ok(())
        },
    )));
    let acp_driver = AcpDriverSpec::new("test-acp", &["acp"]).with_identity("test-acp", "acp");
    let result = execute_acp(
        acp_driver,
        executable.to_string_lossy().as_ref(),
        &json!({}),
        "private-stdin-prompt",
        "",
        Some(dir.as_path()),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(result.ok, "fake ACP failure: {:?}", result.error);
    assert_eq!(result.output, "fake final");
    assert_eq!(result.session_id, "native-fake-session");
    assert_eq!(result.turn_status, "end_turn");
    assert_eq!(result.capabilities.protocol_version, Some(1));
    assert!(matches!(
        result.transitions.last(),
        Some(licoup_agent_adapter_sdk::Transition::Lifecycle(
            licoup_agent_adapter_sdk::LifecycleStage::Completed
        ))
    ));
    assert!(result.stderr_truncated);
    let records = records.lock().unwrap();
    assert!(
        records
            .iter()
            .any(|(source, direction, text)| source == "acp"
                && *direction == RawExecutionDirection::Sent
                && text.contains("private-stdin-prompt")
                && text.ends_with('\n'))
    );
    let received: String = records
        .iter()
        .filter(|(_, direction, _)| *direction == RawExecutionDirection::Received)
        .map(|(_, _, text)| text.as_str())
        .collect();
    assert!(received.contains("native-fake-session") && received.ends_with('\n'));
    assert!(
        records
            .iter()
            .filter(|(_, direction, _)| *direction == RawExecutionDirection::Stderr)
            .map(|(_, _, text)| text.len())
            .sum::<usize>()
            > 1024
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn production_protocol_loop_finishes_only_from_the_prompt_response_signal() {
    let start = Instant::now();
    let hard_deadline = start + Duration::from_millis(350);
    let mut protocol = protocol_awaiting_prompt_response();
    let mut transport = ScriptedProtocolLoopTransport::messages(
        start,
        vec![
            (
                Duration::ZERO,
                agent_chunk(json!({"type": "text", "text": "first "})),
            ),
            (
                Duration::from_millis(90),
                agent_chunk(json!({"type": "text", "text": "second"})),
            ),
            (Duration::from_millis(180), prompt_response()),
        ],
    );

    let (outcome, failure, status_code, stdout_truncated) =
        run_protocol_loop(&mut transport, &mut protocol, Some(hard_deadline));

    let outcome = outcome.expect("valid scripted prompt drain must complete");
    assert!(failure.is_none());
    assert_eq!(status_code, None);
    assert!(!stdout_truncated);
    assert_eq!(outcome.output, "first second");
    assert!(outcome.transitions.iter().any(|transition| matches!(
        transition,
        licoup_agent_adapter_sdk::Transition::Text { text, .. }
            if text == "first second"
    )));
    assert_eq!(transport.now(), start + Duration::from_millis(180));
    assert_eq!(transport.remaining_events(), 0);
    assert!(transport.writes().is_empty());
}

#[test]
fn production_protocol_loop_never_extends_the_explicit_hard_cap() {
    let start = Instant::now();
    let original_request_deadline = start + Duration::from_millis(250);
    let delayed_prompt_response_at = Duration::from_millis(80);
    let mut protocol = protocol_awaiting_prompt_response();
    let mut transport = ScriptedProtocolLoopTransport::messages(
        start,
        vec![
            (
                delayed_prompt_response_at,
                agent_chunk(json!({"type": "text", "text": "one "})),
            ),
            (
                Duration::from_millis(230),
                agent_chunk(json!({"type": "text", "text": "two "})),
            ),
            (
                Duration::from_millis(245),
                agent_chunk(json!({"type": "text", "text": "three"})),
            ),
            (Duration::from_millis(260), prompt_response()),
        ],
    );

    let (outcome, failure, status_code, stdout_truncated) = run_protocol_loop(
        &mut transport,
        &mut protocol,
        Some(original_request_deadline),
    );

    assert!(outcome.is_none());
    assert_eq!(
        failure.as_ref().map(|failure| failure.code.as_str()),
        Some("acp_protocol_timeout")
    );
    assert_eq!(status_code, None);
    assert!(!stdout_truncated);
    assert!(delayed_prompt_response_at > Duration::ZERO);
    assert_eq!(transport.now(), original_request_deadline);
    assert_eq!(transport.remaining_events(), 1);
    assert_eq!(protocol.output, "one two three");
}

#[test]
fn malformed_canary_content_fails_immediately_before_protocol_finish() {
    const MALFORMED_CONTENT_CANARY: &str = "MALFORMED-CONTENT-CANARY";
    let start = Instant::now();
    let hard_deadline = start + Duration::from_millis(300);
    let mut protocol = protocol_awaiting_prompt_response();
    let mut transport = ScriptedProtocolLoopTransport::messages(
        start,
        vec![
            (
                Duration::from_millis(40),
                agent_chunk(json!({"type": "text", "text": "safe"})),
            ),
            (
                Duration::from_millis(80),
                agent_chunk(json!({
                    "type": "text",
                    "text": {"canary": MALFORMED_CONTENT_CANARY}
                })),
            ),
            (
                Duration::from_millis(150),
                agent_chunk(json!({"type": "text", "text": "must-not-run"})),
            ),
        ],
    );

    let (outcome, failure, _, _) =
        run_protocol_loop(&mut transport, &mut protocol, Some(hard_deadline));

    assert!(outcome.is_none());
    assert_eq!(
        failure.as_ref().map(|failure| failure.code.as_str()),
        Some("acp_session_update_invalid")
    );
    assert_eq!(transport.now(), start + Duration::from_millis(80));
    assert_eq!(transport.remaining_events(), 1);
    assert_eq!(protocol.output, "safe");
    assert!(
        !serde_json::to_string(&protocol.events)
            .unwrap()
            .contains(MALFORMED_CONTENT_CANARY)
    );
}

#[test]
fn malformed_notification_before_prompt_response_stops_before_later_response_and_chunk() {
    const MALFORMED_BEFORE_RESPONSE_CANARY: &str = "MALFORMED-BEFORE-RESPONSE-CANARY";
    const UNCONSUMED_VALID_CHUNK: &str = "must-remain-unconsumed";
    let start = Instant::now();
    let hard_deadline = start + Duration::from_millis(300);
    let mut protocol = protocol_awaiting_prompt_response();
    let later_response = prompt_response();
    let later_chunk = agent_chunk(json!({"type": "text", "text": UNCONSUMED_VALID_CHUNK}));
    let mut transport = ScriptedProtocolLoopTransport::messages(
        start,
        vec![
            (
                Duration::from_millis(10),
                agent_chunk(json!({
                    "type": "text",
                    "text": {"canary": MALFORMED_BEFORE_RESPONSE_CANARY}
                })),
            ),
            (Duration::from_millis(20), later_response.clone()),
            (Duration::from_millis(30), later_chunk.clone()),
        ],
    );

    let (outcome, failure, _, _) =
        run_protocol_loop(&mut transport, &mut protocol, Some(hard_deadline));

    assert!(outcome.is_none());
    assert_eq!(
        failure.as_ref().map(|failure| failure.code.as_str()),
        Some("acp_session_update_invalid")
    );
    assert_eq!(transport.now(), start + Duration::from_millis(10));
    assert_eq!(transport.remaining_events(), 2);
    let remaining_messages = transport.remaining_messages();
    assert_eq!(remaining_messages, vec![later_response, later_chunk]);
    assert!(protocol.output.is_empty());
    assert!(protocol.events.is_empty());
}

#[test]
fn protocol_signals_fail_closed_on_output_limit_process_loss_and_hard_deadline() {
    let (dir, executable) = compile_fake_agent("drain-negatives");
    let acp_driver = AcpDriverSpec::new("test-acp", &["acp"]).with_identity("test-acp", "acp");

    let empty_output = execute_acp(
        acp_driver,
        executable.to_string_lossy().as_ref(),
        &json!({}),
        "private-stdin-prompt SELFTEST_EMPTY_OUTPUT",
        "",
        Some(dir.as_path()),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(!empty_output.ok);
    assert_eq!(
        empty_output
            .error
            .as_ref()
            .map(|failure| failure.code.as_str()),
        Some("acp_final_message_missing")
    );

    let output_limit = execute_acp(
        acp_driver,
        executable.to_string_lossy().as_ref(),
        &json!({}),
        "private-stdin-prompt SELFTEST_OUTPUT_LIMIT",
        "",
        Some(dir.as_path()),
        10_000,
        Some(4 * 1024),
        1024,
    );
    assert!(!output_limit.ok);
    assert_eq!(
        output_limit
            .error
            .as_ref()
            .map(|failure| failure.code.as_str()),
        Some("acp_protocol_output_limit")
    );
    assert_eq!(output_limit.session_id, "native-fake-session");
    assert_eq!(output_limit.thread_id, "native-fake-session");
    assert!(output_limit.stdout_truncated);

    let process_loss = execute_acp(
        acp_driver,
        executable.to_string_lossy().as_ref(),
        &json!({}),
        "private-stdin-prompt SELFTEST_PROCESS_LOSS",
        "",
        Some(dir.as_path()),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(!process_loss.ok);
    assert_eq!(
        process_loss
            .error
            .as_ref()
            .map(|failure| failure.code.as_str()),
        Some("acp_process_exited")
    );

    let hard_deadline = execute_acp(
        acp_driver,
        executable.to_string_lossy().as_ref(),
        &json!({}),
        "private-stdin-prompt SELFTEST_HARD_DEADLINE",
        "",
        Some(dir.as_path()),
        1_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(!hard_deadline.ok);
    assert_eq!(
        hard_deadline
            .error
            .as_ref()
            .map(|failure| failure.code.as_str()),
        Some("acp_protocol_timeout")
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn timeout_zero_dispatch_opts_out_of_the_turn_deadline() {
    // dispatch.rs declares timeoutMs 0 as "no turn deadline": the protocol loop
    // must not treat it as an immediately expired window (regression for the
    // execute_acp deadline computation).
    let (dir, executable) = compile_fake_agent("timeout-zero");
    let acp_driver = AcpDriverSpec::new("test-acp", &["acp"]).with_identity("test-acp", "acp");
    let result = execute_acp(
        acp_driver,
        executable.to_string_lossy().as_ref(),
        &json!({}),
        "private-stdin-prompt",
        "",
        Some(dir.as_path()),
        0,
        Some(1024 * 1024),
        1024,
    );
    assert!(result.ok, "timeoutMs 0 ACP failure: {:?}", result.error);
    assert_eq!(result.output, "fake final");
    assert_eq!(result.session_id, "native-fake-session");
    assert_eq!(result.turn_status, "end_turn");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn flood_above_queue_capacity_delivers_complete_ordered_output() {
    let (dir, executable) = compile_fake_agent("flood");
    let acp_driver = AcpDriverSpec::new("test-acp", &["acp"]).with_identity("test-acp", "acp");
    let result = execute_acp(
        acp_driver,
        executable.to_string_lossy().as_ref(),
        &json!({}),
        "private-stdin-prompt SELFTEST_FLOOD",
        "",
        Some(dir.as_path()),
        10_000,
        Some(1024 * 1024),
        1024,
    );
    assert!(result.ok, "flood ACP failure: {:?}", result.error);
    let expected: String = (0..70).map(|index| format!("chunk-{index}")).collect();
    assert_eq!(result.output, expected);
    assert!(result.transitions.iter().any(|transition| matches!(
        transition,
        licoup_agent_adapter_sdk::Transition::Text { text, .. }
            if text == &expected
    )));
    assert_eq!(result.turn_status, "end_turn");
    let _ = fs::remove_dir_all(dir);
}
