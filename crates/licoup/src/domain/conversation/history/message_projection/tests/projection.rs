use std::path::Path;

use serde_json::json;

use super::super::generated_context::normalize_generated_metadata_message;
use super::super::projection::{
    clean_native_message_text, plain_history_message, structured_history_message,
};
use super::super::semantic::HistoryMessageKind;
use crate::domain::conversation::source_catalog::HistoryAdapter;

#[test]
fn plain_projection_keeps_thread_semantics_and_filters_generated_context() {
    let message = plain_history_message(
        HistoryAdapter::Codex,
        Path::new("fixture/session.jsonl"),
        1,
        0,
        "user",
        "<environment_context>hidden</environment_context>Visible prompt",
        Some("2026-01-01T00:00:00Z".to_string()),
    )
    .unwrap();
    assert_eq!(message["text"], "Visible prompt");
    assert_eq!(message["layer"], "thread");
    assert!(
        clean_native_message_text(
            HistoryAdapter::Codex,
            "system",
            "<local-command-output>hidden</local-command-output>"
        )
        .is_none()
    );
}

#[test]
fn generated_agent_notifications_project_as_collapsed_metadata() {
    let mut message = json!({
        "id": "synthetic-message",
        "role": "user",
        "text": "<task-notification>\n<task-id>synthetic-task</task-id>\n<output-file>/fixture/private.output</output-file>\n<status>failed</status>\n<summary>Synthetic background action failed</summary>\n</task-notification>",
        "createdAt": "2026-01-01T00:00:00Z",
        "layer": "thread"
    });

    assert!(normalize_generated_metadata_message(&mut message));

    assert_eq!(message["role"], "metadata");
    assert_eq!(message["layer"], "execution");
    assert_eq!(message["cardType"], "metadata");
    assert_eq!(message["cardTitle"], "Task notification");
    assert_eq!(message["collapsed"], true);
    assert!(!message.to_string().contains("private.output"));
}

#[test]
fn plain_projection_keeps_generated_wrapper_image_as_typed_attachment() {
    let message = plain_history_message(
        HistoryAdapter::Codex,
        Path::new("fixture/session.jsonl"),
        1,
        0,
        "user",
        "# Files mentioned by the user:\n\n## screenshot.webp: /fixture-root/screenshot.webp\n\n## My request:\n\n<image name=[Image #1] path=\"/fixture-root/screenshot.webp\">\nprivate image metadata\n</image>",
        Some("2026-01-01T00:00:00Z".to_string()),
    )
    .unwrap();

    assert_eq!(message["text"], "");
    assert_eq!(message["images"][0]["mediaType"], "image/webp");
    assert_eq!(
        message["images"][0]["path"],
        "/fixture-root/screenshot.webp"
    );
}

#[test]
fn structured_projection_preserves_execution_card_without_sensitive_detail() {
    let private_path = format!("/{}", ["Users", "sample-user", "private.txt"].join("/"));
    let message = structured_history_message(
        HistoryAdapter::ClaudeCode,
        Path::new("fixture/session.jsonl"),
        2,
        0,
        HistoryMessageKind::ToolCall,
        "tool_use",
        &json!({
            "name": "Read",
            "input": {"path": private_path.clone()}
        }),
        Some("2026-01-01T00:00:00Z".to_string()),
    );
    assert_eq!(message["cardTitle"], "Read");
    assert_eq!(message["text"], "path: [local path hidden]");
    assert_eq!(message["layer"], "execution");
    assert!(!message.to_string().contains(&private_path));
}

#[test]
fn reasoning_projection_prefers_recorded_thinking_over_provider_summary() {
    let message = structured_history_message(
        HistoryAdapter::Codex,
        Path::new("fixture/session.jsonl"),
        3,
        0,
        HistoryMessageKind::Reasoning,
        "reasoning",
        &json!({
            "summary": {"type": "summary_text", "text": "Provider summary line"},
            "text": "Private chain of thought"
        }),
        Some("2026-01-01T00:00:00Z".to_string()),
    );
    assert_eq!(message["text"], "Private chain of thought");
    assert!(message.get("providerSummary").is_none());
    assert_eq!(message["cardSubtitle"], "Provider summary line");
}

#[test]
fn reasoning_projection_uses_provider_summary_when_no_thinking_recorded() {
    let message = structured_history_message(
        HistoryAdapter::ClaudeCode,
        Path::new("fixture/session.jsonl"),
        3,
        0,
        HistoryMessageKind::Reasoning,
        "reasoning",
        &json!({"summary": "Provider summary line"}),
        Some("2026-01-01T00:00:00Z".to_string()),
    );
    assert_eq!(message["text"], "Provider summary line");
    assert_eq!(message["providerSummary"], true);
    assert_eq!(message["cardSubtitle"], "Reasoning summary");
}
