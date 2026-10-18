use super::*;
use crate::protocol::parser::events::processing_evidence_kind;

#[test]
fn parser_extracts_only_the_text_delta() {
    let message = json!({
        "type": "stream_event",
        "event": {
            "type": "content_block_delta",
            "delta": {"type": "text_delta", "text": "visible"}
        }
    });
    assert_eq!(partial_text_delta(&message), Some("visible"));
}

#[test]
fn processing_evidence_uses_native_thinking_and_tool_blocks_only() {
    assert_eq!(
        processing_evidence_kind(&json!({
            "type": "stream_event",
            "event": {
                "type": "content_block_delta",
                "delta": {"type": "thinking_delta", "thinking": "private"}
            }
        })),
        None
    );
    assert_eq!(
        processing_evidence_kind(&json!({
            "type": "stream_event",
            "event": {
                "type": "content_block_start",
                "content_block": {"type": "thinking"}
            }
        })),
        Some("reasoning")
    );
    assert_eq!(
        processing_evidence_kind(&json!({
            "type": "stream_event",
            "event": {
                "type": "content_block_delta",
                "content_block": {"type": "thinking"},
                "delta": {"type": "thinking_delta", "thinking": "private"}
            }
        })),
        None
    );
    assert_eq!(
        processing_evidence_kind(&json!({
            "type": "assistant",
            "message": {"content": [{"type": "tool_use", "input": {"secret": true}}]}
        })),
        Some("tool")
    );
    assert_eq!(
        processing_evidence_kind(&json!({
            "type": "stream_event",
            "event": {
                "type": "content_block_delta",
                "delta": {"type": "text_delta", "text": "answer"}
            }
        })),
        None
    );
}
