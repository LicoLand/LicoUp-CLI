use std::path::Path;

use serde_json::json;

use super::super::cursor_projection::{
    cursor_bubble_role, cursor_bubble_usage, cursor_composer_model_from_config,
    cursor_message_from_bubble, normalize_cursor_model_name,
};

#[test]
fn cursor_model_projection_prefers_selected_and_explicit_bubble_models() {
    let composer_model = cursor_composer_model_from_config(&json!({
        "modelConfig": {
            "modelName": "composer-label",
            "selectedModels": [{"modelId": "model-selected"}]
        }
    }));
    assert_eq!(composer_model, "model-selected");
    assert_eq!(normalize_cursor_model_name("default"), "cursor-auto");

    let message = cursor_message_from_bubble(
        &json!({
            "type": 2,
            "text": "Reply",
            "createdAt": 1_773_798_050_000i64,
            "modelInfo": {"modelName": "model-bubble"},
            "tokenCount": {"inputTokens": 12, "outputTokens": 4}
        }),
        &composer_model,
        Path::new("fixture/state.vscdb"),
        0,
    )
    .expect("Cursor bubble message");
    assert_eq!(message["role"], "agent");
    assert_eq!(message["model"], "model-bubble");
    assert_eq!(message["usage"]["totalTokens"], 16);
}

#[test]
fn cursor_role_and_usage_projection_fail_closed_for_unknown_or_empty_values() {
    assert_eq!(cursor_bubble_role(&json!({"type": 1})), Some("user"));
    assert_eq!(
        cursor_bubble_role(&json!({"type": 99, "role": "system"})),
        None
    );
    assert!(cursor_bubble_usage(&json!({"tokenCount": {"inputTokens": 0}}), "model").is_none());
}

#[test]
fn cursor_user_bubble_projects_inner_userquery_text_only() {
    let message = cursor_message_from_bubble(
        &json!({
            "type": 1,
            "text": "<userquery>Keep the real question</userquery>",
            "createdAt": 1_773_798_050_000i64
        }),
        "cursor-auto",
        Path::new("fixture/state.vscdb"),
        0,
    )
    .expect("Cursor user bubble");
    assert_eq!(message["role"], "user");
    assert_eq!(message["text"], "Keep the real question");
    let text = message["text"].as_str().expect("text");
    assert!(!text.contains("<userquery>"));
    assert!(!text.contains("</userquery>"));
}

#[test]
fn cursor_user_bubble_missing_userquery_close_fails_closed_without_tags() {
    let message = cursor_message_from_bubble(
        &json!({
            "type": 1,
            "text": "<userquery>Keep the visible question"
        }),
        "cursor-auto",
        Path::new("fixture/state.vscdb"),
        0,
    )
    .expect("Cursor user bubble");
    assert_eq!(message["text"], "Keep the visible question");
    let text = message["text"].as_str().expect("text");
    assert!(!text.contains("<userquery>"));
    assert!(!text.contains("</userquery>"));
}

#[test]
fn cursor_composer_context_occupancy_is_not_token_consumption() {
    let config = json!({
        "modelConfig": {
            "modelName": "composer-2.5-fast",
            "selectedModels": [{"modelId": "grok-4.5"}]
        },
        "promptTokenBreakdown": {
            "totalUsedTokens": 158628,
            "maxTokens": 300000
        },
        "contextTokensUsed": 158628
    });
    assert!(crate::domain::conversation::usage::extract_token_usage(&config).is_none());
}

#[test]
fn cursor_attachment_only_user_bubble_remains_visible() {
    let attachment = json!({
        "type": "file",
        "name": "synthetic.txt",
        "mimeType": "text/plain"
    });
    let message = cursor_message_from_bubble(
        &json!({
            "type": 1,
            "text": "",
            "attachments": [attachment.clone()]
        }),
        "cursor-auto",
        Path::new("fixture/state.vscdb"),
        7,
    )
    .expect("attachment-only bubble");
    assert_eq!(message["role"], "user");
    assert_eq!(message["text"], "");
    assert_eq!(message["attachments"], json!([attachment]));
}
