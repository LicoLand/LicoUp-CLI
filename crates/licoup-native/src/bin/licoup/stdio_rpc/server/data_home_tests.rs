use super::*;
use std::io::Cursor;

fn frame(method: &str) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(&json!({
        "protocol": STDIO_RPC_PROTOCOL,
        "id": "request-1",
        "workflowId": "workflow-1",
        "method": method,
        "params": {},
    }))
    .unwrap();
    bytes.push(b'\n');
    bytes
}

#[test]
fn ordinary_rpc_cannot_relocate_the_active_root() {
    let output = serve_stdio_rpc(
        Cursor::new(frame("data.home.relocate")),
        Vec::new(),
        |_, _| panic!("relocation must never reach the ordinary command executor"),
    )
    .unwrap();
    let response: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(
        response.pointer("/error/code").and_then(Value::as_str),
        Some("data_home_operation_requires_dedicated_process")
    );
}

#[test]
fn dedicated_root_change_process_rejects_status_and_ordinary_commands() {
    let output = serve_data_home_stdio_rpc(
        Cursor::new(frame("data.home.status")),
        Vec::new(),
        |_, _| panic!("a read-only or ordinary method must not run in the mutation process"),
    )
    .unwrap();
    let response: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(
        response.pointer("/error/code").and_then(Value::as_str),
        Some("data_home_operation_requires_dedicated_process")
    );
}

#[test]
fn data_home_rpc_preserves_allowlisted_codes_and_hides_unrecognized_errors() {
    for (message, expected) in [
        (
            "data_home_destination_exists",
            "data_home_destination_exists",
        ),
        (
            "data_home_previous_root_marker_cleanup_failed",
            "data_home_previous_root_marker_cleanup_failed",
        ),
        (
            "copy failed at /synthetic/private/path",
            "data_home_operation_failed",
        ),
    ] {
        let writer = Arc::new(StdioRpcWriter::new(Vec::new()));
        dispatch_data_home_result(
            &writer,
            StdioRpcFrameLane::Ordered,
            "request-1",
            "workflow-1",
            || Err(anyhow::anyhow!(message)),
        )
        .unwrap();
        let bytes = writer.lock_for_test().clone();
        let response: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            response.pointer("/error/code").and_then(Value::as_str),
            Some(expected)
        );
    }
}
