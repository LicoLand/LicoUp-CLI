use super::super::adapter::adapter_for_agent;
use super::super::{RuntimeAdapter, RuntimeAdapterError};
use serde_json::json;

#[test]
fn remaining_packaged_driver_registry_is_bound_to_one_exact_native_lane_each() {
    super::compose();
    let matrix = [
        (
            "claude-code",
            "claude-code-stream-json",
            "claude-code-cli-stream-json",
        ),
        ("copilot", "copilot-acp", "copilot-acp-v1-stdio-ndjson"),
        (
            "kimi-code",
            "kimi-code-acp",
            "kimi-code-acp-v1-stdio-ndjson",
        ),
        ("openclaw", "openclaw-acp", "openclaw-acp-stdio-jsonrpc"),
        ("pi", "pi-rpc", "pi-rpc-stdio-jsonl"),
        (
            "deepseek-harness",
            "deepseek-harness-sdk-jsonrpc",
            "deepseek-harness-sdk-stdio-jsonrpc",
        ),
    ];

    for (agent_id, driver_id, runtime_protocol) in matrix {
        let adapter = adapter_for_agent(agent_id).expect("packaged adapter");
        assert_eq!(adapter.id(), agent_id);
        assert_eq!(adapter.driver_id(), driver_id);
        assert_eq!(adapter.runtime_protocol(), runtime_protocol);
    }
}

#[test]
fn legacy_launch_values_are_rejected_before_selected_executable_admission() {
    for legacy in [
        json!({"command":"alternate"}),
        json!({"args":["alternate"]}),
    ] {
        let mut request = json!({
            "agent": RuntimeAdapter::ClaudeCode.id(),
            "text": "synthetic prompt",
            "binary": "/runtime/must-not-launch"
        });
        request
            .as_object_mut()
            .unwrap()
            .extend(legacy.as_object().unwrap().clone());
        assert_eq!(
            super::super::dispatch::send_message(
                &crate::target_port::agent_target_port(),
                &request,
            )
            .unwrap_err(),
            RuntimeAdapterError::LegacyLaunchConfiguration
        );
    }
}
