use super::super::sse::{SseFailure, parse_stream};
use std::io::Cursor;
use std::sync::atomic::AtomicBool;

#[test]
fn parser_combines_data_lines_and_ignores_non_data_fields() {
    let input = b"event: update\ndata: {\"a\":\ndata: 1}\n\n";
    let stop = AtomicBool::new(false);
    let mut events = Vec::new();
    parse_stream(Cursor::new(input), &stop, |data, _| {
        events.push(data.to_string());
        true
    })
    .unwrap();
    assert_eq!(events, vec!["{\"a\":\n1}"]);
}

#[test]
fn parser_rejects_an_unbounded_line_before_allocating_a_frame() {
    let input = vec![b'x'; super::super::bounds::MAX_SSE_LINE_BYTES + 1];
    let stop = AtomicBool::new(false);
    let failure = parse_stream(Cursor::new(input), &stop, |_, _| true).unwrap_err();
    assert_eq!(failure, SseFailure::LineTooLarge);
}

#[test]
fn raw_frames_preserve_all_fields_and_line_endings_before_session_selection() {
    let target = ": trace\r\nid: frame-1\r\nevent: tool.updated\r\ndata: {\"type\":\"tool.updated\",\r\ndata: \"properties\":{\"sessionID\":\"target\",\"unknown\":[1,2]}}\r\n\r\n";
    let other = "data: {\"type\":\"tool.updated\",\"properties\":{\"sessionID\":\"other\"}}\n\n";
    let input = format!("{other}{target}");
    let mut captured = Vec::new();
    parse_stream(
        Cursor::new(input.as_bytes()),
        &AtomicBool::new(false),
        |data, raw| {
            if super::super::sse::frame_belongs_to_session(data, "target") {
                captured.push(raw.to_vec());
            }
            true
        },
    )
    .unwrap();
    assert_eq!(captured, [target.as_bytes()]);
    for data in [
        r#"{"type":"server.heartbeat","properties":{}}"#,
        r#"{"type":"message.updated","properties":{"sessionID":"target","info":{"sessionID":"other"}}}"#,
        r#"{"type":"message.updated","properties":{"info":{"id":"target"}}}"#,
    ] {
        assert!(!super::super::sse::frame_belongs_to_session(data, "target"));
    }
    assert!(super::super::sse::frame_belongs_to_session(
        r#"{"type":"session.updated","properties":{"info":{"id":"target"}}}"#,
        "target"
    ));
}
