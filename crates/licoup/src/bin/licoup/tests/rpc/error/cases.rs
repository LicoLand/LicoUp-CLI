use serde_json::{Value, json};

pub(super) fn expected_errors() -> [Value; 6] {
    [
        json!({
            "code": "invalid_request",
            "stage": "request/validation",
            "component": "stdio_rpc",
            "retryable": false,
            "recovery": "correct_request",
            "presentationArgs": {"field": "method"}
        }),
        json!({
            "code": "agent_runtime_unsupported",
            "stage": "discovery/adapter",
            "component": "runtime_adapter",
            "retryable": false,
            "recovery": "select_supported_adapter",
            "presentationArgs": {"agentLabel": "Fixture Agent"}
        }),
        json!({
            "code": "native_agent_executable_unavailable",
            "stage": "process/launch",
            "component": "runtime_process",
            "retryable": true,
            "recovery": "install_or_retry_runtime",
            "presentationArgs": {"runtimeLabel": "Fixture Runtime"}
        }),
        json!({
            "code": "agent_conversation_dispatch_failed",
            "stage": "conversation/dispatch",
            "component": "conversation_runtime",
            "retryable": true,
            "recovery": "preserve_draft_and_retry",
            "presentationArgs": {"agentLabel": "Fixture Agent"}
        }),
        json!({
            "code": "stream_protocol_failed",
            "stage": "conversation/stream_receive",
            "component": "stdio_rpc",
            "retryable": true,
            "recovery": "preserve_draft_and_retry",
            "presentationArgs": {"sequence": "7"}
        }),
        json!({
            "code": "terminal_result_invalid",
            "stage": "conversation/terminal_result",
            "component": "conversation_runtime",
            "retryable": false,
            "recovery": "review_terminal_result",
            "presentationArgs": {"resultKind": "terminal"}
        }),
    ]
}
