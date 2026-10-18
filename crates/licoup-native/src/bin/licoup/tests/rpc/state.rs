use super::*;
use licoup_native::ffi::generated::client_state::{
    ClientStateCollection, ClientStateGetRequest, ClientStateSetRequest,
};

#[test]
fn stdio_rpc_parses_typed_state_requests_without_cli_arguments() {
    let get = serde_json::to_vec(&json!({
        "protocol": STDIO_RPC_PROTOCOL,
        "id": "request-get",
        "workflowId": "workflow-state",
        "method": "state.get",
        "params": {"collection": "settings"},
    }))
    .unwrap();
    let parsed = parse_stdio_rpc_request(&get).expect("typed state.get must parse");
    match parsed.method {
        StdioRpcMethod::StateGet {
            request,
            portable_data_dir: None,
        } => assert_eq!(
            request,
            ClientStateGetRequest {
                collection: ClientStateCollection::Settings,
            }
        ),
        _ => panic!("expected typed state.get request"),
    }

    let set = serde_json::to_vec(&json!({
        "protocol": STDIO_RPC_PROTOCOL,
        "id": "request-set",
        "workflowId": "workflow-state",
        "method": "state.set",
        "params": {
            "collection": "targets",
            "document": {
                "schemaVersion": "v0.0.1:schema:definition-1",
                "collection": "targets",
                "items": []
            }
        },
    }))
    .unwrap();
    let parsed = parse_stdio_rpc_request(&set).expect("typed state.set must parse");
    match parsed.method {
        StdioRpcMethod::StateSet {
            request:
                ClientStateSetRequest {
                    collection: ClientStateCollection::Targets,
                    document,
                },
            portable_data_dir: None,
        } => assert_eq!(document.collection, ClientStateCollection::Targets),
        _ => panic!("expected typed state.set request"),
    }
}

#[test]
fn stdio_rpc_rejects_invalid_state_without_echoing_private_payload() {
    let private_secret = ["credential", "canary"].join("-");
    let request = serde_json::to_vec(&json!({
        "protocol": STDIO_RPC_PROTOCOL,
        "id": "request-state",
        "workflowId": "workflow-state",
        "method": "state.get",
        "params": {
            "collection": "private-path-canary",
            "secret": private_secret
        },
    }))
    .unwrap();
    let error = parse_stdio_rpc_request(&request).expect_err("invalid collection must fail");
    assert_eq!(error.code, "invalid_collection");
    assert!(!error.code.contains("private-path-canary"));
    assert!(!error.code.contains("credential-canary"));
}
