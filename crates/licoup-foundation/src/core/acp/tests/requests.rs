use super::*;
use serde_json::json;
use std::path::Path;

#[test]
fn initialize_builder_emits_stable_v1_capabilities() {
    let request = initialize_request(1, &client(), AcpClientCapabilities::default()).unwrap();
    assert_eq!(request["jsonrpc"], JSON_RPC_VERSION);
    assert_eq!(request["method"], INITIALIZE_METHOD);
    assert_eq!(request["params"]["protocolVersion"], PROTOCOL_VERSION);
    assert_eq!(
        request["params"]["clientCapabilities"],
        json!({
            "fs": {"readTextFile": false, "writeTextFile": false},
            "terminal": false
        })
    );
    assert!(
        request["params"]["clientCapabilities"]
            .get("auth")
            .is_none()
    );
}

#[test]
fn session_builders_keep_lifecycle_shapes_distinct() {
    let cwd = absolute_test_path();
    let new_request = session_request(
        2,
        AcpSessionMethod::New,
        AcpSessionOptions::new(cwd.as_path()),
    )
    .unwrap();
    assert_eq!(new_request["method"], SESSION_NEW_METHOD);
    assert!(new_request["params"].get("sessionId").is_none());
    assert_eq!(new_request["params"]["mcpServers"], json!([]));

    let load_request = session_request(
        2,
        AcpSessionMethod::Load("session-1"),
        AcpSessionOptions::new(cwd.as_path()),
    )
    .unwrap();
    assert_eq!(load_request["method"], SESSION_LOAD_METHOD);
    assert_eq!(load_request["params"]["sessionId"], "session-1");

    let resume_request = session_request(
        2,
        AcpSessionMethod::Resume("session-1"),
        AcpSessionOptions::new(cwd.as_path()),
    )
    .unwrap();
    assert_eq!(resume_request["method"], SESSION_RESUME_METHOD);
    assert_eq!(resume_request["params"]["sessionId"], "session-1");
    assert_eq!(resume_request["params"]["mcpServers"], json!([]));
}

#[test]
fn session_builder_rejects_unsafe_roots_and_mcp_descriptors() {
    let relative = session_request(
        2,
        AcpSessionMethod::New,
        AcpSessionOptions::new(Path::new("relative")),
    );
    assert_eq!(relative.unwrap_err(), AcpError::WorkingDirectoryInvalid);

    let cwd = absolute_test_path();
    let invalid_mcp = [json!({"command": "server"})];
    let request = session_request(
        2,
        AcpSessionMethod::New,
        AcpSessionOptions::new(cwd.as_path()).mcp_servers(&invalid_mcp),
    );
    assert_eq!(request.unwrap_err(), AcpError::McpServerInvalid);
}

#[test]
fn prompt_cancel_and_close_builders_preserve_distinct_envelopes() {
    let prompt = text_prompt_request(3, "session-1", "hello").unwrap();
    assert_eq!(prompt["method"], SESSION_PROMPT_METHOD);
    assert_eq!(
        prompt["params"]["prompt"],
        json!([{"type": "text", "text": "hello"}])
    );
    let oversized = "x".repeat(DEFAULT_MAX_MESSAGE_BYTES);
    assert_eq!(
        text_prompt_request(3, "session-1", &oversized).unwrap_err(),
        AcpError::MessageTooLarge
    );

    let cancel = cancel_notification("session-1").unwrap();
    assert_eq!(cancel["method"], SESSION_CANCEL_METHOD);
    assert!(cancel.get("id").is_none());

    let close = close_session_request(4, "session-1").unwrap();
    assert_eq!(close["method"], SESSION_CLOSE_METHOD);
    assert_eq!(close["params"]["sessionId"], "session-1");
}

#[test]
fn session_list_builder_keeps_filters_optional_and_cursors_opaque() {
    let minimal = session_list_request(5, None, None).unwrap();
    assert_eq!(minimal["method"], SESSION_LIST_METHOD);
    assert_eq!(minimal["params"], json!({}));

    let filtered =
        session_list_request(6, Some("/workspace/project"), Some("opaque-page-2")).unwrap();
    assert_eq!(filtered["params"]["cwd"], "/workspace/project");
    assert_eq!(filtered["params"]["cursor"], "opaque-page-2");
    assert!(
        session_list_request(7, Some("relative/project"), None).is_err(),
        "session/list cwd must remain absolute"
    );
}
