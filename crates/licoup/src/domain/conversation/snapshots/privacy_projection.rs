//! Privacy-bounded searchable projections for archive candidate matching.

use super::*;

#[derive(Clone, Debug)]
pub(super) struct SnapshotSourceMetadata {
    pub(super) adapter_id: String,
    pub(super) adapter_label: String,
    pub(super) source_client: String,
    pub(super) source_client_label: String,
    pub(super) host_app: String,
    pub(super) host_app_label: String,
    pub(super) source_label: String,
    pub(super) source_kind: String,
    pub(super) source_path: String,
    pub(super) native_identity: String,
}

pub(super) fn snapshot_source_metadata(session: &Value) -> SnapshotSourceMetadata {
    let adapter_id = text_value(session, "adapterId")
        .or_else(|| text_value(session, "agentId"))
        .unwrap_or_else(|| "unknown".to_string());
    let source_client = text_value(session, "sourceClient")
        .or_else(|| text_value(session, "sourceTool"))
        .unwrap_or_else(|| adapter_id.clone());
    SnapshotSourceMetadata {
        adapter_label: text_value(session, "adapterLabel").unwrap_or_else(|| adapter_id.clone()),
        source_client_label: text_value(session, "sourceClientLabel").unwrap_or_default(),
        host_app: text_value(session, "hostApp").unwrap_or_default(),
        host_app_label: text_value(session, "hostAppLabel").unwrap_or_default(),
        source_label: text_value(session, "sourceLabel").unwrap_or_else(|| source_client.clone()),
        source_kind: text_value(session, "sourceKind").unwrap_or_else(|| "unknown".to_string()),
        source_path: text_value(session, "sourcePath").unwrap_or_default(),
        native_identity: native_identity(session),
        adapter_id,
        source_client,
    }
}

pub(super) fn project_archive_semantic_document(
    session: &Value,
    raw: &RawExport,
    snapshot_hash: &str,
    refreshed_at: &str,
    metadata: &SnapshotSourceMetadata,
) -> Value {
    let raw_hash = hash_text(&raw.content);
    let semantic = session.get("semantic").cloned().unwrap_or_else(|| {
        crate::domain::conversation_semantic::build_semantic_conversation(
            session
                .get("messages")
                .and_then(Value::as_array)
                .map(|messages| messages.as_slice())
                .unwrap_or(&[]),
            crate::domain::conversation_semantic::SemanticAuditInput {
                adapter_id: &metadata.adapter_id,
                adapter_label: &metadata.adapter_label,
                host_app: &metadata.host_app,
                host_app_label: &metadata.host_app_label,
                source_client: &metadata.source_client,
                source_kind: &metadata.source_kind,
                native_session_id: &metadata.native_identity,
                path_ref: &metadata.source_path,
                content_hash: &raw_hash,
                byte_length: raw.content.len() as u64,
                parse_warnings: &[],
                redaction_status: "applied",
                validation_status: "unchecked",
                created_at: refreshed_at,
                updated_at: refreshed_at,
            },
        )
        .unwrap_or_else(|_| json!({}))
    });
    let mut semantic_document = semantic;
    if let Some(object) = semantic_document.as_object_mut() {
        let mut artifacts = object
            .get("artifacts")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        artifacts.extend([
            json!({
                "id": format!("artifact-summary-{snapshot_hash}"),
                "layer": "artifacts",
                "kind": "summary",
                "label": "Archive summary",
                "ref": SUMMARY_MD
            }),
            json!({
                "id": format!("artifact-index-{snapshot_hash}"),
                "layer": "artifacts",
                "kind": "index",
                "label": "Conversation index",
                "ref": CONVERSATION_INDEX_MD
            }),
            json!({
                "id": format!("artifact-validation-{snapshot_hash}"),
                "layer": "artifacts",
                "kind": "validation",
                "label": "Archive validation",
                "ref": VALIDATION_JSON
            }),
            json!({
                "id": format!("artifact-raw-{snapshot_hash}"),
                "layer": "artifacts",
                "kind": "archive-path",
                "label": "Raw source export",
                "ref": raw.file_name,
                "contentHash": raw_hash
            }),
        ]);
        object.insert("artifacts".to_string(), Value::Array(artifacts));
        let evidence = json!({
            "kind": crate::domain::conversation_semantic::evidence_kind_from_source(
                metadata.source_kind.as_str()
            ),
            "pathRef": raw.file_name,
            "contentHash": raw_hash,
            "byteLength": raw.content.len()
        });
        if let Some(raw_block) = object.get_mut("raw").and_then(Value::as_object_mut) {
            raw_block.insert("evidenceRefs".to_string(), json!([evidence.clone()]));
        }
        if let Some(audit) = object.get_mut("audit").and_then(Value::as_object_mut) {
            audit.insert("validationStatus".to_string(), json!("ok"));
            audit.insert("sourceEvidence".to_string(), evidence);
        }
    }
    let _ =
        crate::domain::conversation_semantic::validate_semantic_conversation(&semantic_document);
    semantic_document
}

pub(super) fn candidate_search_text(candidate: &Value) -> String {
    let mut parts = Vec::<String>::new();
    for key in ["title", "nativeSessionId"] {
        if let Some(text) = candidate.get(key).and_then(Value::as_str) {
            parts.push(text.to_string());
        }
    }
    if let Some(items) = candidate
        .get("archiveDiscoveryMatchedTerms")
        .and_then(Value::as_array)
    {
        for item in items {
            if let Some(text) = item.as_str() {
                parts.push(text.to_string());
            }
        }
    }
    if let Some(messages) = candidate.get("messages").and_then(Value::as_array) {
        for message in messages {
            if !message_is_matchable_conversation_text(message) {
                continue;
            }
            if let Some(text) = message.get("text").and_then(Value::as_str) {
                parts.push(text.to_string());
            }
        }
    }
    parts.join("\n")
}

/// Exact-keyword backup searches user-visible conversation text only. Opaque
/// native identifiers, filesystem paths, and generated aliases are not user
/// keywords and cannot select a conversation on their own.
pub(super) fn candidate_exact_keyword_text(candidate: &Value) -> String {
    let mut parts = Vec::<String>::new();
    if let Some(title) = candidate.get("title").and_then(Value::as_str) {
        let projected_id = candidate.get("id").and_then(Value::as_str);
        let native_id = candidate.get("nativeSessionId").and_then(Value::as_str);
        if Some(title) != projected_id && Some(title) != native_id {
            parts.push(title.to_string());
        }
    }
    if let Some(messages) = candidate.get("messages").and_then(Value::as_array) {
        for message in messages {
            if !message_is_matchable_conversation_text(message) {
                continue;
            }
            if let Some(text) = message.get("text").and_then(Value::as_str) {
                parts.push(text.to_string());
            }
        }
    }
    parts.join("\n")
}

pub(super) fn message_is_matchable_conversation_text(message: &Value) -> bool {
    let role = text_value(message, "role")
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        role.as_str(),
        "user" | "human" | "assistant" | "agent" | "model"
    ) || (matches!(role.as_str(), "transcript" | "")
        && message
            .get("text")
            .and_then(Value::as_str)
            .map(looks_like_archive_text_conversation)
            .unwrap_or(false))
        || (role == "record"
            && message
                .get("text")
                .and_then(Value::as_str)
                .map(looks_like_archive_database_record)
                .unwrap_or(false))
}

pub(super) fn candidate_path_text(candidate: &Value) -> String {
    let mut parts = Vec::<String>::new();
    for key in ["sourcePath", "workingDirectory", "cwd", "projectPath"] {
        if let Some(text) = candidate.get(key).and_then(Value::as_str) {
            parts.push(text.to_string());
        }
    }
    if let Some(messages) = candidate.get("messages").and_then(Value::as_array) {
        for message in messages {
            for key in ["sourcePath", "workingDirectory", "cwd", "projectPath"] {
                if let Some(text) = message.get(key).and_then(Value::as_str) {
                    parts.push(text.to_string());
                }
            }
        }
    }
    parts.join("\n")
}
