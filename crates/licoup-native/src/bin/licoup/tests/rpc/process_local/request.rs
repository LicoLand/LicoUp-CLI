use super::*;

#[test]
fn stdio_rpc_accepts_every_process_local_conversation_control() {
    for operation in [
        "open",
        "send",
        "history",
        "cleanup",
        "capabilities",
        "cancel",
    ] {
        let method = format!("agent.conversation.{operation}");
        let request = serde_json::to_vec(&json!({
            "protocol": STDIO_RPC_PROTOCOL,
            "id": format!("request-{operation}"),
            "workflowId": "workflow-process-local",
            "method": method,
            "params": {
                "agent": "claude-code",
                "sessionId": "opaque-native-session",
                "model": "configured-model"
            },
        }))
        .unwrap();

        let parsed = parse_stdio_rpc_request(&request)
            .unwrap_or_else(|error| panic!("{operation} rejected with {}", error.code));
        match parsed.method {
            StdioRpcMethod::Conversation {
                operation: parsed_operation,
                params,
                portable_data_dir,
            } => {
                assert_eq!(parsed_operation, operation);
                assert_eq!(params["model"], "configured-model");
                assert_eq!(params["sessionId"], "opaque-native-session");
                assert!(portable_data_dir.is_none());
            }
            _ => panic!("{operation} did not map to a conversation request"),
        }
    }
}
