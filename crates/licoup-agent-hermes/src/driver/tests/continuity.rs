use super::*;

#[cfg(unix)]
#[test]
fn fake_child_exact_resume_keeps_native_session_id() {
    use std::sync::{Arc, Mutex};

    install_package_dialect();

    let root = unique_temp_dir("hermes-acp-resume");
    let _portable_data = PortableDataDirGuard::isolate_under(&root);
    let executable = root.join("fake-hermes-resume");
    write_executable(
        &executable,
        r#"#!/bin/sh
if [ "$#" -ne 1 ] || [ "$1" != "acp" ]; then
  exit 40
fi
printf '%s\n' launch >> "$0.launches"
IFS= read -r init
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"agentInfo":{"name":"hermes-agent","version":"test"}}}'
turn=1
while [ "$turn" -le 2 ]; do
  IFS= read -r session
  if [ "$turn" -eq 1 ]; then
case "$session" in *'"method":"session/new"'*) :;; *) exit 44;; esac
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"native-hermes-exact"}}'
  else
case "$session" in *'"method":"session/load"'*native-hermes-exact*) :;; *) exit 45;; esac
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":null}'
  fi
  IFS= read -r prompt
  case "$prompt" in *native-hermes-exact*) :;; *) exit 46;; esac
  printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"native-hermes-exact","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"chunk-a"}}}}'
  sleep 0.05
  printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"native-hermes-exact","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"-chunk-b"}}}}'
  printf '%s\n' '{"jsonrpc":"2.0","id":4,"result":{"stopReason":"end_turn"}}'
  turn=$((turn + 1))
done
sleep 30
"#,
    );

    let first = execute(
        executable.to_str().unwrap(),
        &json!({}),
        "first-turn-canary",
        "",
        Some(&root),
        5_000,
        Some(64 * 1024),
        8 * 1024,
    );
    assert!(
        first.ok,
        "first turn should open a native session: {:?}",
        first
            .error
            .as_ref()
            .map(|failure| (failure.code, failure.stage))
    );
    assert_eq!(first.session_id, "native-hermes-exact");

    let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink_target = Arc::clone(&captured);
    licoup_foundation::platform::turn_event_emit::install_stream_sink(Box::new(move |event| {
        sink_target.lock().unwrap().push(event);
    }));
    let _guard = licoup_foundation::platform::turn_event_emit::StreamSinkGuard;

    let follow_up = execute(
        executable.to_str().unwrap(),
        &json!({}),
        "follow-up-canary",
        &first.session_id,
        Some(&root),
        5_000,
        Some(64 * 1024),
        8 * 1024,
    );
    assert!(follow_up.ok, "exact resume follow-up should succeed");
    assert_eq!(follow_up.session_id, first.session_id);
    assert_eq!(follow_up.output, "chunk-a-chunk-b");

    let events = captured.lock().unwrap().clone();
    let chunk_texts: Vec<&str> = events
        .iter()
        .filter(|event| event["event"] == "agent.message.chunk")
        .filter_map(|event| event["payload"]["text"].as_str())
        .collect();
    assert_eq!(chunk_texts, vec!["chunk-a", "-chunk-b"]);
    assert!(
        events
            .iter()
            .any(|event| event["event"] == "agent.message.completed")
    );
    assert!(
        events
            .iter()
            .all(|event| event["sessionId"] == "native-hermes-exact")
    );
    let launch_receipt = fs::read_to_string(format!("{}.launches", executable.display())).unwrap();
    assert_eq!(launch_receipt.lines().count(), 1);
    assert_eq!(
        cleanup_session(&first.session_id),
        ControlDisposition::Accepted
    );
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn persistent_turn_can_be_cancelled_without_restarting_the_session() {
    install_package_dialect();
    let root = unique_temp_dir("hermes-acp-cancel");
    let _portable_data = PortableDataDirGuard::isolate_under(&root);
    let executable = root.join("fake-hermes-cancel");
    write_executable(
        &executable,
        r#"#!/bin/sh
if [ "$#" -ne 1 ] || [ "$1" != "acp" ]; then
  exit 40
fi
IFS= read -r init
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"agentInfo":{"name":"hermes-agent","version":"test"}}}'
IFS= read -r new_session
case "$new_session" in *'"method":"session/new"'*) :;; *) exit 41;; esac
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"native-hermes-cancel"}}'
IFS= read -r first_prompt
printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"native-hermes-cancel","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"first"}}}}'
printf '%s\n' '{"jsonrpc":"2.0","id":4,"result":{"stopReason":"end_turn"}}'
IFS= read -r load_session
case "$load_session" in *'"method":"session/load"'*native-hermes-cancel*) :;; *) exit 42;; esac
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":null}'
IFS= read -r second_prompt
IFS= read -r cancel
case "$cancel" in *'"method":"session/cancel"'*native-hermes-cancel*) :;; *) exit 43;; esac
printf '%s\n' '{"jsonrpc":"2.0","id":4,"result":{"stopReason":"cancelled"}}'
sleep 30
"#,
    );
    let first = execute(
        executable.to_str().unwrap(),
        &json!({}),
        "first",
        "",
        Some(&root),
        5_000,
        Some(64 * 1024),
        8 * 1024,
    );
    assert!(
        first.ok,
        "first cancellable turn failed: {:?}",
        first
            .error
            .as_ref()
            .map(|failure| (failure.code, failure.stage))
    );
    let executable_for_turn = executable.clone();
    let root_for_turn = root.clone();
    let session_id = first.session_id.clone();
    let session_for_turn = session_id.clone();
    let turn = thread::spawn(move || {
        let _portable_data = PortableDataDirGuard::isolate_under(&root_for_turn);
        execute(
            executable_for_turn.to_str().unwrap(),
            &json!({}),
            "second",
            &session_for_turn,
            Some(&root_for_turn),
            5_000,
            Some(64 * 1024),
            8 * 1024,
        )
    });
    let mut disposition = ControlDisposition::NoActiveTurn;
    for _ in 0..100 {
        disposition = cancel(&session_id);
        if disposition == ControlDisposition::Accepted {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(disposition, ControlDisposition::Accepted);
    let cancelled = turn.join().unwrap();
    assert!(!cancelled.ok);
    let failure = cancelled.error.unwrap();
    assert_eq!(failure.code, "hermes_acp_turn_not_completed");
    assert_eq!(failure.turn_status.as_deref(), Some("cancelled"));
    assert_eq!(failure.session_id.as_deref(), Some(session_id.as_str()));
    assert_eq!(cleanup_session(&session_id), ControlDisposition::Accepted);
    let _ = fs::remove_dir_all(root);
}
