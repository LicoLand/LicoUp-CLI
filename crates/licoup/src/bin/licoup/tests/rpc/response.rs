use super::*;

fn response_lines(bytes: Vec<u8>) -> Vec<Value> {
    String::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn stdio_rpc_replaces_oversized_response_with_a_bounded_error() {
    let mut writer = Vec::new();
    write_stdio_rpc_success_with_limit(
        &mut writer,
        "request-1",
        "workflow-1",
        json!({"payload": "x".repeat(4096)}),
        512,
    )
    .unwrap();

    let frames = response_lines(writer);
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["ok"], false);
    assert_eq!(frames[0]["error"]["code"], "response_too_large");
    assert!(frames[0].to_string().len() < 512);
}

#[test]
fn stdio_rpc_progress_frames_are_ordered_before_the_terminal_frame() {
    let writer = Arc::new(StdioRpcWriter::new(Vec::new()));
    write_stdio_rpc_event(
        writer.stream(),
        "request-1",
        "workflow-1",
        1,
        json!({
            "sessionId": "session-1",
            "turnId": "turn-1",
            "event": "delta",
            "text": "bounded",
        }),
    )
    .unwrap();
    write_stdio_rpc_terminal_success(
        writer.stream(),
        "request-1",
        "workflow-1",
        2,
        json!({"status": "complete"}),
    )
    .unwrap();

    let frames = response_lines(recover_stdio_rpc_writer(writer).unwrap());
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["kind"], "event");
    assert_eq!(frames[0]["sequence"], 1);
    assert_eq!(frames[1]["kind"], "terminal");
    assert_eq!(frames[1]["sequence"], 2);
    assert_eq!(frames[1]["ok"], true);
}

#[test]
fn stdio_rpc_accepts_persistent_progress_before_native_ids_are_bound() {
    let writer = Arc::new(StdioRpcWriter::new(Vec::new()));
    write_stdio_rpc_event(
        writer.stream(),
        "request-1",
        "workflow-1",
        1,
        json!({
            "event": "agent.turn.processing",
            "sessionId": "",
            "turnId": "",
            "turnHandle": "dispatch-1",
            "conversationId": "conversation-1",
            "cursor": 1,
        }),
    )
    .unwrap();

    let frames = response_lines(recover_stdio_rpc_writer(writer).unwrap());
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["event"]["event"], "agent.turn.processing");
}

#[test]
fn stdio_rpc_rejects_unstructured_progress_events() {
    let writer = Arc::new(StdioRpcWriter::new(Vec::new()));
    let error = write_stdio_rpc_event(
        writer.stream(),
        "request-1",
        "workflow-1",
        1,
        json!({"event": "delta"}),
    )
    .unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::Other);
    assert!(recover_stdio_rpc_writer(writer).unwrap().is_empty());
}
