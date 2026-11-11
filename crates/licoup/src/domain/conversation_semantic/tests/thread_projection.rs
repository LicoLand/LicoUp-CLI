use serde_json::json;

use super::super::thread_projection::{append_timeline_messages, thread_wire_message_from_tagged};

#[test]
fn thread_wire_projection_normalizes_roles_and_preserves_accounting_metadata() {
    let wire = thread_wire_message_from_tagged(&json!({
        "id": "thread-1",
        "role": "assistant",
        "text": "Visible reply",
        "createdAt": "2026-01-01T00:00:00Z",
        "images": [{"mediaType": "image/png", "path": "/fixture-root/screenshot.png"}],
        "usage": {"inputTokens": 2},
        "model": "model-1"
    }))
    .expect("thread wire message");
    assert_eq!(wire["role"], "agent");
    assert_eq!(wire["eventKind"], "assistant-message");
    assert_eq!(wire["usage"]["inputTokens"], 2);
    assert_eq!(wire["model"], "model-1");
    assert_eq!(wire["images"][0]["mediaType"], "image/png");
    assert_eq!(wire["images"][0]["path"], "/fixture-root/screenshot.png");
}

#[test]
fn thread_timeline_projection_keeps_order_and_native_wire_role() {
    let semantic = json!({
        "thread": [
            {"id": "one", "role": "user", "text": "First", "createdAt": "", "eventKind": "user-message"},
            {"id": "two", "role": "assistant", "text": "Second", "createdAt": "", "eventKind": "assistant-message"}
        ]
    });
    let mut messages = Vec::new();
    append_timeline_messages(&semantic, &mut messages);
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[1]["role"], "agent");
}
