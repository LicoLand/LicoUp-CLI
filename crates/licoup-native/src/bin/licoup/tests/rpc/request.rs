use super::*;

#[test]
fn stdio_rpc_protocol_matches_the_persistent_host_boundary() {
    assert_eq!(
        STDIO_RPC_PROTOCOL,
        licoup_native::platform::conversation_host_transport::STDIO_RPC_PROTOCOL
    );
}

#[test]
fn stdio_rpc_parses_exact_method_and_absolute_portable_path() {
    let portable = env::temp_dir().join("licoup-rpc-portable");
    let request = serde_json::to_vec(&json!({
        "protocol": STDIO_RPC_PROTOCOL,
        "id": "request-1",
        "workflowId": "workflow-1",
        "method": "catalog.list",
        "params": {"source": "cache"},
        "portableDataDir": portable,
    }))
    .unwrap();

    let parsed = match parse_stdio_rpc_request(&request) {
        Ok(parsed) => parsed,
        Err(error) => panic!("request should parse, error code: {}", error.code),
    };
    assert_eq!(parsed.id, "request-1");
    assert_eq!(parsed.workflow_id, "workflow-1");
    match parsed.method {
        StdioRpcMethod::Catalog {
            operation,
            params,
            portable_data_dir,
        } => {
            assert_eq!(operation, "list");
            assert_eq!(params, json!({"source": "cache"}));
            assert_eq!(portable_data_dir, Some(portable));
        }
        _ => panic!("expected a catalog request"),
    }
}

#[test]
fn stdio_rpc_parses_client_conversation_execute() {
    let request = serde_json::to_vec(&json!({
        "protocol": STDIO_RPC_PROTOCOL,
        "id": "request-1",
        "workflowId": "workflow-1",
        "method": "client.conversation.execute",
        "params": {"action": "conversation.list", "includeArchived": false},
    }))
    .unwrap();

    let parsed = parse_stdio_rpc_request(&request).expect("request should parse");
    match parsed.method {
        StdioRpcMethod::ClientConversation { params, .. } => {
            assert_eq!(
                params,
                json!({"action": "conversation.list", "includeArchived": false})
            );
        }
        _ => panic!("expected a client conversation request"),
    }
}

#[test]
fn stdio_rpc_parses_persistent_background_dispatch() {
    let request = serde_json::to_vec(&json!({
        "protocol": STDIO_RPC_PROTOCOL,
        "id": "request-1",
        "workflowId": "workflow-1",
        "method": "agent.conversation.dispatch",
        "params": {
            "agentId": "codex",
            "text": "bounded prompt",
            "conversationId": "conversation:group",
            "membershipId": "membership:codex"
        },
    }))
    .unwrap();

    let parsed = parse_stdio_rpc_request(&request).expect("request should parse");
    match parsed.method {
        StdioRpcMethod::Conversation {
            operation, params, ..
        } => {
            assert_eq!(operation, "dispatch");
            assert_eq!(params["membershipId"], "membership:codex");
        }
        _ => panic!("expected a persistent Conversation dispatch"),
    }
}

#[test]
fn stdio_rpc_parses_strategy_execute() {
    let request = serde_json::to_vec(&json!({
        "protocol": STDIO_RPC_PROTOCOL,
        "id": "request-1",
        "workflowId": "workflow-1",
        "method": "strategy.execute",
        "params": {
            "action": "strategy.run.active",
            "revisionDigest": "rev",
            "conversationId": "conversation:group"
        },
    }))
    .unwrap();

    let parsed = parse_stdio_rpc_request(&request).expect("request should parse");
    match parsed.method {
        StdioRpcMethod::StrategyExecute { params, .. } => {
            assert_eq!(params["action"], "strategy.run.active");
        }
        _ => panic!("expected a strategy execute request"),
    }
}

#[test]
fn stdio_rpc_rejects_invalid_protocol_and_relative_portable_path() {
    let invalid_protocol = serde_json::to_vec(&json!({
        "protocol": "unsupported",
        "id": "request-1",
        "workflowId": "workflow-1",
        "method": "shutdown",
    }))
    .unwrap();
    let error = match parse_stdio_rpc_request(&invalid_protocol) {
        Ok(_) => panic!("unsupported protocol must be rejected"),
        Err(error) => error,
    };
    assert_eq!(error.code, "invalid_protocol");

    let relative_path = serde_json::to_vec(&json!({
        "protocol": STDIO_RPC_PROTOCOL,
        "id": "request-1",
        "workflowId": "workflow-1",
        "method": "execute",
        "args": ["usage", "summary"],
        "portableDataDir": "relative/path",
    }))
    .unwrap();
    let error = match parse_stdio_rpc_request(&relative_path) {
        Ok(_) => panic!("relative portable path must be rejected"),
        Err(error) => error,
    };
    assert_eq!(error.code, "invalid_portable_data_dir");
}

#[test]
fn stdio_rpc_execute_rejects_commands_that_require_external_private_stdin() {
    let args = vec![
        "mcp".to_string(),
        "bridge".to_string(),
        "--stdin-json".to_string(),
        "true".to_string(),
    ];

    assert!(rpc_command_reads_external_stdin(&args));
    assert!(!rpc_command_writes_external_stdout(&args));
}

#[test]
fn gateway_client_token_is_never_projected_over_stdio_rpc() {
    let args = vec![
        "gateway".to_string(),
        "client-token".to_string(),
        "--agent".to_string(),
        "codex".to_string(),
    ];
    assert!(rpc_command_writes_external_stdout(&args));
}
