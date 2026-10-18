use serde_json::{Map, Value, json};

use super::super::semantic::HistoryMessageKind;
use super::super::structured_privacy::{
    looks_like_raw_structured_payload, sanitize_structured_event_text, structured_event_text,
    structured_metadata_detail, structured_reasoning_detail, structured_reasoning_summary,
    structured_tool_call_detail,
};
use super::drop_json_iteratively;

#[test]
fn structured_text_redacts_credentials_paths_and_opaque_values() {
    let opaque = "A".repeat(48);
    let private_path = format!("/{}", ["home", "sample-user", "private.txt"].join("/"));
    let text = format!("failed at {private_path} with access_token=private and Bearer {opaque}");
    let sanitized = sanitize_structured_event_text(&text).unwrap();
    assert!(!sanitized.contains(&private_path));
    assert!(!sanitized.contains("private"));
    assert!(!sanitized.contains(&opaque));
    assert!(sanitized.contains("[redacted]"));
    assert!(sanitized.contains("[local path hidden]"));
}

#[test]
fn raw_whole_document_payloads_stay_hidden_but_detail_channels_survive() {
    assert!(looks_like_raw_structured_payload(r#"{"secret":"value"}"#));
    assert!(!looks_like_raw_structured_payload(
        r#"command: {"secret": "value"}"#
    ));
    assert_eq!(
        structured_event_text(
            HistoryMessageKind::Reasoning,
            &json!({"thinking": "private reasoning"}),
            "Reasoning details are redacted.",
        ),
        "private reasoning"
    );
    assert_eq!(
        structured_reasoning_summary(&json!({
            "summary": {"type": "summary_text", "text": "Provider summary"},
            "thinking": "private reasoning"
        }))
        .as_deref(),
        Some("Provider summary")
    );
}

#[test]
fn reasoning_detail_returns_only_recorded_thinking_text() {
    assert_eq!(
        structured_reasoning_detail(&json!({"text": "CoT", "summary": "Summary"})).as_deref(),
        Some("CoT")
    );
    assert_eq!(
        structured_reasoning_detail(&json!({"think": "  Claude thinking  "})).as_deref(),
        Some("Claude thinking")
    );
    assert_eq!(
        structured_reasoning_detail(&json!({"summary": "Summary only"})),
        None
    );
}

#[test]
fn tool_call_detail_formats_arguments_and_redacts_secrets() {
    let detail = structured_tool_call_detail(&json!({
        "type": "tool_use",
        "name": "Bash",
        "input": {"command": "ls /fixture-root/private", "access_token": "secret-value"}
    }))
    .unwrap();
    assert_eq!(
        detail,
        "access_token: secret-value\ncommand: ls /fixture-root/private"
    );
    assert_eq!(
        structured_event_text(
            HistoryMessageKind::ToolCall,
            &json!({
                "type": "tool_use",
                "name": "Bash",
                "input": {"command": "ls /fixture-root/private", "access_token": "secret-value"}
            }),
            "Invocation details are hidden.",
        ),
        "access_token: [redacted]\ncommand: ls [local path hidden]"
    );
    assert_eq!(
        structured_tool_call_detail(&json!({"type": "tool", "tool": "bash", "input": {}})),
        None
    );
}

#[test]
fn metadata_detail_unfolds_json_content_and_keeps_key_values() {
    let detail = structured_metadata_detail(&json!({
        "type": "metadata",
        "content": r#"{"cwd": "/workspace/project", "access_token": "fixture-credential-canary"}"#
    }))
    .unwrap();
    assert_eq!(
        detail,
        "access_token: fixture-credential-canary\ncwd: /workspace/project"
    );
    assert_eq!(
        structured_event_text(
            HistoryMessageKind::Metadata,
            &json!({
                "type": "metadata",
                "content": r#"{"cwd": "/workspace/project", "access_token": "fixture-credential-canary"}"#
            }),
            "Sensitive native metadata is hidden.",
        ),
        "access_token: [redacted]\ncwd: [local path hidden]"
    );
    assert_eq!(
        structured_metadata_detail(&json!({
            "type": "token-count",
            "usage": {"input_tokens": 120},
            "model": "test-model"
        }))
        .unwrap(),
        "model: test-model\nusage: {\"input_tokens\":120}"
    );
}

#[test]
fn policy_allowed_structured_text_and_deep_summaries_are_complete() {
    let long = format!("begin {} end", "allowed segment ".repeat(200));
    assert_eq!(
        sanitize_structured_event_text(&long).as_deref(),
        Some(long.as_str())
    );

    let mut nested = json!("deep-summary-canary");
    for _ in 0..4_097 {
        nested = Value::Array(vec![nested]);
    }
    let mut wrapper = Map::new();
    wrapper.insert("summary".to_string(), nested);
    let value = Value::Object(wrapper);
    assert_eq!(
        structured_reasoning_summary(&value).as_deref(),
        Some("deep-summary-canary")
    );
    drop_json_iteratively(value);
}
