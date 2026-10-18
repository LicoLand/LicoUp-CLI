use serde_json::json;

use super::super::{
    SEMANTIC_KIND, SemanticAuditInput, build_semantic_conversation,
    timeline_messages_from_semantic, validate_semantic_conversation,
};

#[test]
fn public_facade_builds_valid_semantic_documents_and_timeline_messages() {
    let semantic = build_semantic_conversation(
        &[json!({
            "id": "thread-1",
            "layer": "thread",
            "role": "user",
            "text": "Hello",
            "createdAt": "2026-01-01T00:00:00Z"
        })],
        SemanticAuditInput {
            adapter_id: "codex",
            adapter_label: "Codex",
            host_app: "codex",
            host_app_label: "Codex",
            source_client: "codex",
            source_kind: "jsonl",
            native_session_id: "session-1",
            path_ref: "fixture://semantic/session.jsonl",
            content_hash: "digest",
            byte_length: 64,
            parse_warnings: &[],
            redaction_status: "applied",
            validation_status: "ok",
            created_at: "2026-01-01T00:00:00Z",
            updated_at: "2026-01-01T00:00:00Z",
        },
    )
    .expect("semantic build through public facade");

    validate_semantic_conversation(&semantic).expect("public validation");
    assert_eq!(semantic["kind"], SEMANTIC_KIND);
    let timeline = timeline_messages_from_semantic(&semantic);
    assert_eq!(timeline.len(), 1);
    assert_eq!(timeline[0]["role"], "user");
}
