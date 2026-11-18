use super::*;
use crate::parser::processing_evidence_kind;

#[test]
fn event_projection_drops_raw_delta_arguments_and_extension_details() {
    let private_argument = ["private", "argument"].join("-");
    let projected = sanitized_event(&json!({
        "type": "message_update",
        "assistantMessageEvent": {
            "type": "text_delta",
            "delta": "private-message"
        },
        "arguments": {"secret": private_argument.clone()}
    }))
    .unwrap();
    assert_eq!(
        projected,
        json!({"type": "message_update", "deltaType": "text_delta"})
    );
    assert!(!projected.to_string().contains("private"));
    assert!(!projected.to_string().contains(&private_argument));

    let extension = sanitized_event(&json!({
        "type": "extension_error",
        "message": "private-extension-detail"
    }))
    .unwrap();
    assert_eq!(extension, json!({"type": "extension_error"}));
}

#[test]
fn processing_evidence_distinguishes_reasoning_tools_and_terminal_events() {
    assert_eq!(
        processing_evidence_kind(&json!({"type": "tool_execution_start"})),
        Some("tool")
    );
    assert_eq!(
        processing_evidence_kind(&json!({
            "type": "message_update",
            "assistantMessageEvent": {"type": "thinking_delta"}
        })),
        None
    );
    assert_eq!(
        processing_evidence_kind(&json!({
            "type": "message_update",
            "assistantMessageEvent": {"type": "thinking_start"}
        })),
        Some("reasoning")
    );
    assert_eq!(
        processing_evidence_kind(&json!({"type": "tool_execution_end"})),
        None
    );
    assert_eq!(
        processing_evidence_kind(&json!({"type": "agent_settled"})),
        None
    );
}
