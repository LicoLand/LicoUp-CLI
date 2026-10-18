use super::*;

#[cfg(unix)]
#[test]
fn fake_child_e2e_streams_final_and_drains_stderr() {
    install_package_dialect();
    let root = unique_temp_dir("hermes-acp-e2e");
    let _portable_data = PortableDataDirGuard::isolate_under(&root);
    let executable = root.join("fake-hermes");
    write_executable(
        &executable,
        r#"#!/bin/sh
if [ "$1" = "acp" ] && [ "$2" = "--check" ]; then
  printf '%s\n' 'Hermes ACP check OK'
  exit 0
fi
if [ "$1" = "acp" ] && [ "$2" = "--version" ]; then
  printf '%s\n' 'Hermes test-version'
  exit 0
fi
if [ "$#" -ne 1 ] || [ "$1" != "acp" ]; then
  exit 40
fi
dd if=/dev/zero bs=1024 count=128 2>/dev/null | tr '\000' x >&2 &
IFS= read -r init
case "$init" in *private-hermes-prompt*|*workspace/project*) exit 41;; esac
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"agentInfo":{"name":"hermes-agent","version":"test"}}}'
IFS= read -r session
case "$session" in *private-hermes-prompt*) exit 42;; esac
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"native-hermes-session","models":{"currentModelId":"native-model","availableModels":[]},"modes":{"currentModeId":"default","availableModes":[]}}}'
IFS= read -r model
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{}}'
IFS= read -r prompt
case "$prompt" in *private-hermes-prompt*) :;; *) exit 43;; esac
printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"native-hermes-session","update":{"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"hidden thought"}}}}'
printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"native-hermes-session","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"native answer"}}}}'
printf '%s\n' '{"jsonrpc":"2.0","id":4,"result":{"stopReason":"end_turn"}}'
wait
"#,
    );
    let result = execute(
        executable.to_str().unwrap(),
        &json!({"model": "requested-model"}),
        "private-hermes-prompt",
        "",
        Some(&root),
        5_000,
        Some(128 * 1024),
        8 * 1024,
    );
    assert!(
        result.ok,
        "Hermes fake child failed: {:?}",
        result
            .error
            .as_ref()
            .map(|failure| (failure.code, failure.stage))
    );
    assert_eq!(result.output, "native answer");
    assert_eq!(result.session_id, "native-hermes-session");
    assert_eq!(result.turn_status, "end_turn");
    assert_eq!(result.events.len(), 2);
    assert_eq!(result.effective.model.as_deref(), Some("requested-model"));
    assert_eq!(result.effective.approval_policy, Some(json!("default")));
    assert_eq!(
        cleanup_session(&result.session_id),
        ControlDisposition::Accepted
    );

    let probe = probe_driver(executable.to_str().unwrap(), 10_000, 16 * 1024);
    assert!(probe.available);
    assert!(probe.supported);
    assert!(probe.supports_streaming);
    assert!(probe.supports_model_override);
    assert!(!probe.supports_reasoning_override);
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn hermes_zero_timeout_and_default_event_projection_are_unbounded() {
    install_package_dialect();
    let root = unique_temp_dir("hermes-acp-zero-timeout");
    let _portable_data = PortableDataDirGuard::isolate_under(&root);
    let executable = root.join("fake-hermes-unbounded");
    write_executable(
        &executable,
        r#"#!/bin/sh
if [ "$#" -ne 1 ] || [ "$1" != "acp" ]; then
  exit 40
fi
IFS= read -r init
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"agentInfo":{"name":"hermes-agent","version":"test"}}}'
IFS= read -r session
case "$session" in *'"method":"session/new"'*) :;; *) exit 41;; esac
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"unbounded-hermes-session"}}'
IFS= read -r prompt
sleep 0.3
i=0
while [ "$i" -lt 4200 ]; do
  printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"unbounded-hermes-session","update":{"sessionUpdate":"tool_call","toolCallId":"opaque-tool-id","title":"shell","kind":"execute"}}}'
  i=$((i + 1))
done
printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"unbounded-hermes-session","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"native answer"}}}}'
printf '%s\n' '{"jsonrpc":"2.0","id":4,"result":{"stopReason":"end_turn"}}'
sleep 30
"#,
    );
    let result = execute(
        executable.to_str().unwrap(),
        &json!({}),
        "unbounded-turn",
        "",
        Some(&root),
        0,
        None,
        8 * 1024,
    );
    assert!(
        result.ok,
        "a zero-timeout turn must run to the native terminal signal: {:?}",
        result
            .error
            .as_ref()
            .map(|failure| (failure.code, failure.stage))
    );
    assert_eq!(result.session_id, "unbounded-hermes-session");
    assert_eq!(result.turn_status, "end_turn");
    assert_eq!(result.output, "native answer");
    let tool_updates = result
        .events
        .iter()
        .filter(|event| event["sessionUpdate"] == "tool_call")
        .count();
    assert_eq!(
        tool_updates, 4_200,
        "every tool event must survive the default projection"
    );
    assert!(
        result.events.len() > 4_096,
        "the former unconditional 4,096-event terminal must be gone"
    );
    assert_eq!(
        cleanup_session(&result.session_id),
        ControlDisposition::Accepted
    );
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn bounded_timeout_keeps_the_named_failure() {
    install_package_dialect();
    let root = unique_temp_dir("hermes-acp-bounded-timeout");
    let _portable_data = PortableDataDirGuard::isolate_under(&root);
    let executable = root.join("fake-hermes-slow");
    write_executable(
        &executable,
        r#"#!/bin/sh
if [ "$#" -ne 1 ] || [ "$1" != "acp" ]; then
  exit 40
fi
IFS= read -r init
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"agentInfo":{"name":"hermes-agent","version":"test"}}}'
IFS= read -r session
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"slow-hermes-session"}}'
IFS= read -r prompt
sleep 10
printf '%s\n' '{"jsonrpc":"2.0","id":4,"result":{"stopReason":"end_turn"}}'
sleep 30
"#,
    );
    let result = execute(
        executable.to_str().unwrap(),
        &json!({}),
        "slow-turn",
        "",
        Some(&root),
        5_000,
        Some(64 * 1024),
        8 * 1024,
    );
    assert!(
        !result.ok,
        "a finite deadline must still fail the slow turn"
    );
    let failure = result.error.unwrap();
    assert_eq!(failure.code, "hermes_acp_timeout");
    assert_eq!(failure.turn_status.as_deref(), Some("timeout"));
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn explicit_output_bound_keeps_the_named_failure() {
    install_package_dialect();
    let root = unique_temp_dir("hermes-acp-output-bound");
    let _portable_data = PortableDataDirGuard::isolate_under(&root);
    let executable = root.join("fake-hermes-noisy");
    write_executable(
        &executable,
        r#"#!/bin/sh
if [ "$#" -ne 1 ] || [ "$1" != "acp" ]; then
  exit 40
fi
IFS= read -r init
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true},"agentInfo":{"name":"hermes-agent","version":"test"}}}'
IFS= read -r session
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"noisy-hermes-session"}}'
IFS= read -r prompt
i=0
while [ "$i" -lt 4200 ]; do
  printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"noisy-hermes-session","update":{"sessionUpdate":"tool_call","toolCallId":"opaque-tool-id","title":"shell","kind":"execute"}}}'
  i=$((i + 1))
done
printf '%s\n' '{"jsonrpc":"2.0","id":4,"result":{"stopReason":"end_turn"}}'
sleep 30
"#,
    );
    let result = execute(
        executable.to_str().unwrap(),
        &json!({}),
        "noisy-turn",
        "",
        Some(&root),
        5_000,
        Some(8 * 1024),
        8 * 1024,
    );
    assert!(!result.ok, "an explicit output bound must still fail");
    let failure = result.error.unwrap();
    assert_eq!(failure.code, "hermes_acp_output_limit");
    let _ = fs::remove_dir_all(root);
}
