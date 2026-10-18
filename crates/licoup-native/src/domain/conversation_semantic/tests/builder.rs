use serde_json::json;

use super::super::builder::{build_semantic_conversation, timeline_messages_from_semantic};
use super::super::model::SemanticAuditInput;

fn audit_input<'a>(path_ref: &'a str) -> SemanticAuditInput<'a> {
    SemanticAuditInput {
        adapter_id: "codex",
        adapter_label: "Codex",
        host_app: "codex",
        host_app_label: "Codex",
        source_client: "codex",
        source_kind: "jsonl",
        native_session_id: "session-1",
        path_ref,
        content_hash: "digest",
        byte_length: 128,
        parse_warnings: &[],
        redaction_status: "applied",
        validation_status: "ok",
        created_at: "2026-01-01T00:00:00Z",
        updated_at: "2026-01-01T00:00:01Z",
    }
}

#[test]
fn builder_separates_layers_sanitizes_defaults_and_redacts_evidence_paths() {
    let token = format!("{}{}", "sk", "-private");
    let path_ref = format!("{}/{}", concat!("/", "Users"), "person/session.jsonl");
    let messages = vec![
        json!({
            "id": "thread-1", "layer": "thread", "role": "user",
            "text": format!("Hello {token}"), "createdAt": "2026-01-01T00:00:00Z"
        }),
        json!({
            "id": "execution-1", "layer": "execution", "role": "tool_call",
            "cardType": "tool-call", "cardTitle": "Read", "text": "Details hidden",
            "createdAt": "2026-01-01T00:00:01Z", "collapsed": true,
            "sourceItemType": "tool-use"
        }),
        json!({
            "id": "artifact-1", "layer": "artifacts", "cardTitle": "Report",
            "sourcePath": path_ref
        }),
    ];
    let semantic =
        build_semantic_conversation(&messages, audit_input(&path_ref)).expect("semantic build");
    assert_eq!(semantic["thread"].as_array().unwrap().len(), 1);
    assert_eq!(semantic["execution"].as_array().unwrap().len(), 1);
    assert_eq!(semantic["artifacts"].as_array().unwrap().len(), 1);
    assert!(
        !semantic["thread"][0]["text"]
            .as_str()
            .unwrap()
            .contains(&token)
    );
    assert_eq!(semantic["artifacts"][0]["ref"], "session.jsonl");
    assert_eq!(
        semantic["audit"]["sourceEvidence"]["pathRef"],
        "session.jsonl"
    );

    let timeline = timeline_messages_from_semantic(&semantic);
    assert_eq!(timeline.len(), 2);
    assert_eq!(timeline[0]["layer"], "thread");
    assert_eq!(timeline[1]["layer"], "execution");
}

#[test]
fn builder_records_excluded_and_unknown_layers_without_projecting_them() {
    let semantic = build_semantic_conversation(
        &[
            json!({"id": "audit-1", "layer": "audit"}),
            json!({"id": "unknown-1", "layer": "unexpected"}),
        ],
        audit_input("fixture://semantic/session.jsonl"),
    )
    .expect("semantic build");
    assert!(semantic["thread"].as_array().unwrap().is_empty());
    assert!(semantic["execution"].as_array().unwrap().is_empty());
    assert_eq!(
        semantic["audit"]["parseWarnings"].as_array().unwrap().len(),
        2
    );
}
