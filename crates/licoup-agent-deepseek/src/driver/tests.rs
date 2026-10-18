//! The claims this package's own driver makes about a running Harness.
//!
//! They drive the process half against fake harnesses the test writes from
//! source, so the durable session store, the exact-session cleanup, the
//! bounded frame reader and the streaming projection are established on the
//! process the package actually starts rather than on a description of it.

use super::*;
use serde_json::json;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[test]
fn private_instructions_fail_before_process_launch() {
    let result = execute(
        "definitely-not-a-real-deepseek-harness",
        &json!({"model":"test","privateInstructions":"private sentinel"}),
        "exact user prompt",
        "session",
        Some(std::env::temp_dir().as_path()),
        1_000,
        None,
        1_024,
    );
    assert_eq!(
        result.error.as_ref().map(|error| error.code),
        Some("deepseek_harness_private_instructions_unsupported")
    );
}

#[test]
fn protocol_reader_rejects_an_oversized_line_without_buffering_past_cap() {
    let input = format!("{{\"value\":\"{}\"}}\n", "x".repeat(128));
    let mut reader = BufReader::new(input.as_bytes());
    assert!(matches!(
        read_protocol_frame(&mut reader, Some(32)),
        Err(FrameError::OutputLimit)
    ));
}

#[test]
fn raw_execution_preserves_unparsed_frames_and_encoded_requests() {
    use licoup_foundation::platform::raw_execution::RawExecutionScope;
    let records = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&records);
    let observer = RawExecutionObserver::new(move |_, direction, text| {
        captured.lock().unwrap().push((direction, text.to_owned()));
        Ok(())
    });
    let _scope = RawExecutionScope::enter(Some(observer));
    let binding = RawExecutionBinding::default();
    let _guard = binding.bind_current();
    let raw = b" {\"future\":{\"toolResult\":\"exact\"}} \r\n";
    let reader = RawExecutionReader::new(
        raw.as_slice(),
        binding.clone(),
        "deepseek-harness",
        RawExecutionDirection::Received,
    );
    assert!(
        read_protocol_frame(&mut BufReader::new(reader), None)
            .unwrap()
            .is_some()
    );
    let reader = RawExecutionReader::new(
        b"{invalid\r\n".as_slice(),
        binding,
        "deepseek-harness",
        RawExecutionDirection::Received,
    );
    assert!(matches!(
        read_protocol_frame(&mut BufReader::new(reader), None),
        Err(FrameError::InvalidJson)
    ));
    let mut wire = Vec::new();
    write_frame(&mut wire, &json!({"message":"line one\nline two"})).unwrap();
    let captured = records.lock().unwrap();
    assert_eq!(
        captured[0],
        (
            RawExecutionDirection::Received,
            String::from_utf8(raw.to_vec()).unwrap()
        )
    );
    assert_eq!(
        captured[1],
        (RawExecutionDirection::Received, "{invalid\r\n".to_owned())
    );
    assert_eq!(
        captured[2],
        (
            RawExecutionDirection::Sent,
            String::from_utf8(wire).unwrap()
        )
    );
}

#[test]
#[cfg(unix)]
fn process_restart_reclaims_a_durably_recorded_session() {
    let root = std::env::temp_dir().join(format!("lico-dsh-test-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    let executable = root.join("fake-dsh");
    let log = root.join("prompts.log");
    let store = root.join("sessions.store");
    // Emulates the SDK runtime's durable session store: a session identity
    // recorded by one process is refused by any later process.
    let source = format!(
        r#"#!/bin/sh
[ "$#" -eq 2 ] && [ "$1" = '--profile' ] && [ "$2" = 'sdk' ] || exit 9
while IFS= read -r line; do
 case "$line" in
  *'"method":"initialize"'*) printf '%s\n' '{{"jsonrpc":"2.0","id":"initialize","result":{{"serverInfo":{{"name":"deepseek-harness-sdk-runtime"}}}}}}' ;;
  *'"method":"session/prompt"'*) request_id=$(printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p'); session_id=$(printf '%s' "$line" | sed -n 's/.*"sessionId":"\([^"]*\)".*/\1/p'); if grep -q " ${{session_id}}\$" '{store}' 2>/dev/null && ! grep -q "^$$ ${{session_id}}\$" '{store}'; then printf '{{"jsonrpc":"2.0","id":"%s","error":{{"code":-32603,"message":"session \"%s\" already exists"}}}}\n' "$request_id" "$session_id"; else grep -q "^$$ ${{session_id}}\$" '{store}' 2>/dev/null || printf '%s %s\n' "$$" "$session_id" >> '{store}'; count=$(grep -c '^prompt ' '{log}' 2>/dev/null || true); count=$((count + 1)); id="message-$count"; printf 'prompt %s %s %s\n' "$$" "$session_id" "$count" >> '{log}'; printf '{{"jsonrpc":"2.0","id":"%s","result":{{"messageId":"%s"}}}}\n' "$request_id" "$id"; printf '{{"jsonrpc":"2.0","method":"session.event","params":{{"sessionId":"%s","event":{{"type":"agent/inbox/spliced","data":{{"inserted":[{{"id":"%s"}}]}}}}}}}}\n' "$session_id" "$id"; printf '{{"jsonrpc":"2.0","method":"session.event","params":{{"sessionId":"%s","event":{{"type":"assistant/message","data":{{"message":{{"content":[{{"type":"text","text":"turn-%s"}}]}}}}}}}}}}\n' "$session_id" "$count"; printf '{{"jsonrpc":"2.0","method":"session.status","params":{{"sessionId":"%s","status":"idle"}}}}\n' "$session_id"; fi ;;
  *'"method":"shutdown"'*) exit 0 ;;
 esac
done
"#,
        store = store.display(),
        log = log.display(),
    );
    fs::write(&executable, source).unwrap();
    let mut permissions = fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&executable, permissions).unwrap();
    let params = json!({"model":"deepseek-test"});
    let run = |prompt: &str| {
        execute(
            executable.to_str().unwrap(),
            &params,
            prompt,
            "restart-session",
            Some(&root),
            2_000,
            None,
            4096,
        )
    };
    let first = run("one");
    let second = run("two");
    assert!(first.ok, "first turn failed: {:?}", first.error);
    assert!(second.ok, "second turn failed: {:?}", second.error);
    assert_eq!(first.output, "turn-1");
    assert_eq!(second.output, "turn-2");
    assert_eq!(
        cleanup_session("restart-session"),
        CleanupDisposition::Accepted
    );
    let third = run("three");
    assert!(third.ok, "restart turn failed: {:?}", third.error);
    assert_eq!(third.output, "turn-3");
    let prompts: Vec<Vec<String>> = fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(|line| {
            line.strip_prefix("prompt ")
                .unwrap()
                .split(' ')
                .map(str::to_owned)
                .collect()
        })
        .collect();
    assert_eq!(prompts.len(), 3);
    let first_identity = &prompts[0][1];
    assert!(first_identity.starts_with("restart-session-"));
    assert_eq!(prompts[1][0], prompts[0][0], "one transport owns two turns");
    assert_eq!(&prompts[1][1], first_identity);
    assert_ne!(prompts[2][0], prompts[0][0], "a restart spawns a process");
    assert_ne!(&prompts[2][1], first_identity);
    assert!(prompts[2][1].starts_with("restart-session-"));
    assert_eq!(
        cleanup_session("restart-session"),
        CleanupDisposition::Accepted
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[cfg(unix)]
fn prompt_rejection_surfaces_its_own_failure_code() {
    let root = std::env::temp_dir().join(format!("lico-dsh-test-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    let executable = root.join("fake-dsh");
    let source = r#"#!/bin/sh
[ "$#" -eq 2 ] && [ "$1" = '--profile' ] && [ "$2" = 'sdk' ] || exit 9
while IFS= read -r line; do
 case "$line" in
  *'"method":"initialize"'*) printf '%s\n' '{"jsonrpc":"2.0","id":"initialize","result":{"serverInfo":{"name":"deepseek-harness-sdk-runtime"}}}' ;;
  *'"method":"session/prompt"'*) request_id=$(printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p'); printf '{"jsonrpc":"2.0","id":"%s","error":{"code":-32603,"message":"session already exists"}}\n' "$request_id" ;;
  *'"method":"shutdown"'*) exit 0 ;;
 esac
done
"#;
    fs::write(&executable, source).unwrap();
    let mut permissions = fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&executable, permissions).unwrap();
    let result = execute(
        executable.to_str().unwrap(),
        &json!({"model":"deepseek-test"}),
        "one",
        "rejected-session",
        Some(&root),
        2_000,
        None,
        4096,
    );
    assert!(!result.ok);
    let error = result.error.unwrap();
    assert_eq!(error.code, "deepseek_harness_prompt_rejected");
    assert_eq!(error.stage, "protocol/prompt");
    assert_eq!(
        cleanup_session("rejected-session"),
        CleanupDisposition::SessionUnavailable
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[cfg(unix)]
fn two_turns_reuse_one_initialized_process_and_cleanup_exact_session() {
    let root = std::env::temp_dir().join(format!("lico-dsh-test-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    let executable = root.join("fake-dsh");
    let log = root.join("protocol.log");
    let source = format!(
        r#"#!/bin/sh
[ "$#" -eq 2 ] && [ "$1" = '--profile' ] && [ "$2" = 'sdk' ] || exit 9
while IFS= read -r line; do
 case "$line" in
  *'"method":"initialize"'*) printf 'initialize %s\n' "$$" >> '{}'; printf '%s\n' '{{"jsonrpc":"2.0","id":"initialize","result":{{"serverInfo":{{"name":"deepseek-harness-sdk-runtime"}}}}}}' ;;
  *'"method":"session/prompt"'*) count=$(grep -c '^prompt ' '{}' 2>/dev/null || true); count=$((count + 1)); id="message-$count"; request_id=$(printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p'); session_id=$(printf '%s' "$line" | sed -n 's/.*"sessionId":"\([^"]*\)".*/\1/p'); printf 'prompt %s %s\n' "$$" "$count" >> '{}'; printf '{{"jsonrpc":"2.0","id":"%s","result":{{"messageId":"%s"}}}}\n' "$request_id" "$id"; printf '{{"jsonrpc":"2.0","method":"session.event","params":{{"sessionId":"%s","event":{{"type":"agent/inbox/spliced","data":{{"inserted":[{{"id":"%s"}}]}}}}}}}}\n' "$session_id" "$id"; printf '{{"jsonrpc":"2.0","method":"session.event","params":{{"sessionId":"%s","event":{{"type":"assistant/message","data":{{"message":{{"content":[{{"type":"text","text":"process-%s-turn-%s"}}]}}}}}}}}}}\n' "$session_id" "$$" "$count"; while [ ! -e '{}/progress-'"$count" ]; do sleep 0.01; done; printf '{{"jsonrpc":"2.0","method":"session.status","params":{{"sessionId":"%s","status":"idle"}}}}\n' "$session_id" ;;
  *'"method":"shutdown"'*) printf 'shutdown %s\n' "$$" >> '{}'; exit 0 ;;
 esac
done
"#,
        log.display(),
        log.display(),
        log.display(),
        root.display(),
        log.display()
    );
    fs::write(&executable, source).unwrap();
    let mut permissions = fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&executable, permissions).unwrap();
    let params = json!({"model":"deepseek-test","reasoningEffort":"high"});
    let public_events = Arc::new(Mutex::new(Vec::<Value>::new()));
    let observed = Arc::clone(&public_events);
    let progress_root = root.clone();
    licoup_foundation::platform::turn_event_emit::install_stream_sink(Box::new(move |event| {
        if event["event"] == "agent.message.completed" {
            let mut observed = observed.lock().unwrap();
            observed.push(event);
            // The child cannot send idle until this live message reaches the
            // public sink. This proves delivery is independent of terminal.
            fs::write(
                progress_root.join(format!("progress-{}", observed.len())),
                b"",
            )
            .unwrap();
        }
    }));
    let _stream_guard = licoup_foundation::platform::turn_event_emit::StreamSinkGuard;
    let captures = Arc::new(Mutex::new([Vec::new(), Vec::new()]));
    let observer_for = |index: usize| {
        let captures = Arc::clone(&captures);
        RawExecutionObserver::new(move |_, _, text| {
            captures.lock().unwrap()[index].push(text.to_owned());
            Ok(())
        })
    };
    let first = {
        let _scope = licoup_foundation::platform::raw_execution::RawExecutionScope::enter(Some(
            observer_for(0),
        ));
        execute(
            executable.to_str().unwrap(),
            &params,
            "one",
            "persistent-session",
            Some(&root),
            2_000,
            None,
            4096,
        )
    };
    let second = {
        let _scope = licoup_foundation::platform::raw_execution::RawExecutionScope::enter(Some(
            observer_for(1),
        ));
        execute(
            executable.to_str().unwrap(),
            &params,
            "two",
            "persistent-session",
            Some(&root),
            2_000,
            None,
            4096,
        )
    };
    assert!(first.ok, "first turn failed: {:?}", first.error);
    assert!(second.ok, "second turn failed: {:?}", second.error);
    assert_eq!(first.effective.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(second.effective.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(
        first.output.split("-turn-").next(),
        second.output.split("-turn-").next()
    );
    assert_eq!(first.output.rsplit('-').next(), Some("1"));
    assert_eq!(second.output.rsplit('-').next(), Some("2"));
    {
        let events = public_events.lock().unwrap();
        assert_eq!(events.len(), 2);
        for (index, event) in events.iter().enumerate() {
            assert_eq!(event["sessionId"], "persistent-session");
            assert_eq!(event["turnId"], format!("message-{}", index + 1));
            assert_eq!(event["payload"]["messageUnit"], "deepseek-harness:reply:1");
        }
        assert_eq!(events[0]["payload"]["text"], first.output);
        assert_eq!(events[1]["payload"]["text"], second.output);
    }
    {
        let captures = captures.lock().unwrap();
        let first_raw = captures[0].concat();
        let second_raw = captures[1].concat();
        assert!(first_raw.contains("\"method\":\"initialize\""));
        assert!(first_raw.contains("\"reasoningEffort\":\"high\""));
        assert!(!second_raw.contains("\"method\":\"initialize\""));
        assert!(second_raw.contains("message-2"));
        assert!(!first_raw.contains("message-2"));
    }
    let drifted = execute(
        executable.to_str().unwrap(),
        &json!({"model":"deepseek-test","reasoningEffort":"max"}),
        "must not run",
        "persistent-session",
        Some(&root),
        2_000,
        None,
        4096,
    );
    assert_eq!(
        drifted.error.unwrap().code,
        "deepseek_harness_session_config_changed"
    );
    assert_eq!(
        cleanup_session("persistent-session"),
        CleanupDisposition::Accepted
    );
    assert_eq!(
        cleanup_session("persistent-session"),
        CleanupDisposition::SessionUnavailable
    );
    let log = fs::read_to_string(&log).unwrap();
    assert_eq!(
        log.lines()
            .filter(|line| line.starts_with("initialize "))
            .count(),
        1
    );
    assert_eq!(
        log.lines()
            .filter(|line| line.starts_with("prompt "))
            .count(),
        2
    );
    assert_eq!(
        log.lines()
            .filter(|line| line.starts_with("shutdown "))
            .count(),
        1
    );
    fs::remove_dir_all(root).unwrap();
}
