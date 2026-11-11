use super::snapshot_identity::{candidate_id, native_identity};
use serde_json::{Value, json};
use std::path::Path;

pub(crate) const COLLECTION_SCHEMA_VERSION: &str = "v0.0.1:agent:native-conversation-snapshot-1";

#[derive(Clone, Debug)]
pub(crate) struct ProfileMatch {
    pub(crate) matched_terms: Vec<String>,
    pub(crate) confidence: String,
    pub(crate) reason: String,
}

pub(crate) fn build_collection(
    existing: &Value,
    topic: &str,
    topic_key: &str,
    root: &Path,
    status: &str,
    conversations: Vec<Value>,
    refreshed_at: &str,
    source_summaries: &[Value],
    diagnostics: &[Value],
    selected_count: usize,
    candidate_count: usize,
) -> Value {
    json!({
        "schemaVersion": COLLECTION_SCHEMA_VERSION,
        "kind": "native-conversation-snapshot-collection",
        "topic": topic,
        "displayTitle": existing.get("displayTitle").and_then(Value::as_str).unwrap_or(topic),
        "topicKey": topic_key,
        "snapshotRoot": root.to_string_lossy(),
        "state": status,
        "createdAt": existing.get("createdAt").and_then(Value::as_str).unwrap_or(refreshed_at),
        "refreshedAt": refreshed_at,
        "latestRefreshSummary": {
            "candidateCount": candidate_count,
            "selectedCount": selected_count,
            "sourceCount": source_summaries.len(),
            "selectionMode": "deterministic"
        },
        "sources": source_summaries,
        "diagnostics": diagnostics,
        "conversations": conversations
    })
}

pub(crate) fn empty_collection(topic: &str, topic_key: &str, root: &Path, now: &str) -> Value {
    json!({
        "schemaVersion": COLLECTION_SCHEMA_VERSION,
        "kind": "native-conversation-snapshot-collection",
        "topic": topic,
        "displayTitle": topic,
        "topicKey": topic_key,
        "snapshotRoot": root.to_string_lossy(),
        "state": "empty",
        "createdAt": now,
        "refreshedAt": now,
        "latestRefreshSummary": {},
        "sources": [],
        "diagnostics": [],
        "conversations": []
    })
}

pub(crate) fn existing_conversations(collection: &Value) -> Vec<Value> {
    collection
        .get("conversations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

pub(crate) fn upsert_conversation_record(conversations: &mut Vec<Value>, record: Value) {
    let snapshot_id = record
        .get("snapshotId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if let Some(existing) = conversations
        .iter_mut()
        .find(|item| item.get("snapshotId").and_then(Value::as_str) == Some(snapshot_id.as_str()))
    {
        *existing = record;
    } else {
        conversations.push(record);
    }
    conversations.sort_by(|left, right| {
        right
            .get("refreshedAt")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(
                left.get("refreshedAt")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
    });
}

pub(crate) fn collection_summary(collection: &Value, path: &Path) -> Value {
    json!({
        "schemaVersion": collection.get("schemaVersion").cloned().unwrap_or_else(|| json!(COLLECTION_SCHEMA_VERSION)),
        "topic": collection.get("topic").cloned().unwrap_or_else(|| json!("")),
        "displayTitle": collection.get("displayTitle").cloned().unwrap_or_else(|| json!("")),
        "topicKey": collection.get("topicKey").cloned().unwrap_or_else(|| json!("")),
        "state": collection.get("state").cloned().unwrap_or_else(|| json!("empty")),
        "createdAt": collection.get("createdAt").cloned().unwrap_or_else(|| json!("")),
        "refreshedAt": collection.get("refreshedAt").cloned().unwrap_or_else(|| json!("")),
        "conversationCount": collection.get("conversations").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "collectionPath": path.to_string_lossy(),
        "latestRefreshSummary": collection.get("latestRefreshSummary").cloned().unwrap_or_else(|| json!({}))
    })
}

pub(crate) fn archive_key_for_session(session: &Value) -> String {
    native_identity(session)
}

pub(crate) fn archive_status_for(previous: Option<&Value>, fingerprint: &str) -> String {
    match previous {
        None => "new".to_string(),
        Some(record)
            if record
                .get("content_fingerprint")
                .and_then(Value::as_str)
                .unwrap_or_default()
                == fingerprint =>
        {
            "unchanged".to_string()
        }
        Some(_) => "updated".to_string(),
    }
}

pub(crate) fn archive_index_record(
    archive_key: &str,
    session: &Value,
    record: &Value,
    profile_match: &ProfileMatch,
    archive_status: &str,
    refreshed_at: &str,
) -> Value {
    let source_client = record
        .get("sourceClient")
        .or_else(|| session.get("sourceClient"))
        .or_else(|| session.get("sourceTool"))
        .or_else(|| record.get("agentAdapterId"))
        .cloned()
        .unwrap_or_else(|| json!(""));
    json!({
        "archive_key": archive_key,
        "source_tool": source_client.clone(),
        "source_client": source_client,
        "source_client_label": record.get("sourceClientLabel").or_else(|| session.get("sourceClientLabel")).cloned().unwrap_or_else(|| json!("")),
        "host_app": record.get("hostApp").or_else(|| session.get("hostApp")).cloned().unwrap_or_else(|| json!("")),
        "host_app_label": record.get("hostAppLabel").or_else(|| session.get("hostAppLabel")).cloned().unwrap_or_else(|| json!("")),
        "source_label": record.get("sourceLabel").or_else(|| session.get("sourceLabel")).cloned().unwrap_or_else(|| json!("")),
        "adapter_id": record.get("agentAdapterId").or_else(|| session.get("adapterId")).cloned().unwrap_or_else(|| json!("")),
        "title": record.get("title").or_else(|| session.get("title")).cloned().unwrap_or_else(|| json!("")),
        "source_path": record.get("sourcePath").cloned().unwrap_or_else(|| json!("")),
        "native_session_id": session.get("nativeSessionId").cloned().unwrap_or_else(|| json!("")),
        "snapshot_id": record.get("snapshotId").cloned().unwrap_or_else(|| json!("")),
        "snapshot_path": record.get("snapshotPath").cloned().unwrap_or_else(|| json!("")),
        "semantic_document_path": record.get("semanticDocumentPath").cloned().unwrap_or_else(|| json!("")),
        "semantic_markdown_path": record.get("semanticMarkdownPath").cloned().unwrap_or_else(|| json!("")),
        "semantic_content_hash": record.get("semanticContentHash").cloned().unwrap_or_else(|| json!("")),
        "raw_content_path": record.get("rawContentPath").cloned().unwrap_or_else(|| json!("")),
        "raw_export_kind": record.get("rawExportKind").cloned().unwrap_or_else(|| json!("")),
        "content_fingerprint": record.get("rawContentHash").cloned().unwrap_or_else(|| json!("")),
        "raw_content_bytes": record.get("rawContentBytes").cloned().unwrap_or_else(|| json!(0)),
        "matched_terms": profile_match.matched_terms.clone(),
        "confidence": profile_match.confidence,
        "match_reason": profile_match.reason,
        "archive_status": archive_status,
        "source_modified_at": session.get("updatedAt").cloned().unwrap_or_else(|| json!("")),
        "refreshed_at": refreshed_at
    })
}

pub(crate) fn archive_match_record(
    archive_key: &str,
    session: &Value,
    profile_match: &ProfileMatch,
) -> Value {
    let source_client = session
        .get("sourceClient")
        .or_else(|| session.get("sourceTool"))
        .or_else(|| session.get("adapterId"))
        .or_else(|| session.get("agentId"))
        .cloned()
        .unwrap_or_else(|| json!(""));
    json!({
        "archive_key": archive_key,
        "candidate_id": candidate_id(session).unwrap_or_default(),
        "source_tool": source_client.clone(),
        "source_client": source_client,
        "source_client_label": session.get("sourceClientLabel").cloned().unwrap_or_else(|| json!("")),
        "host_app": session.get("hostApp").cloned().unwrap_or_else(|| json!("")),
        "host_app_label": session.get("hostAppLabel").cloned().unwrap_or_else(|| json!("")),
        "source_label": session.get("sourceLabel").cloned().unwrap_or_else(|| json!("")),
        "adapter_id": session.get("adapterId").or_else(|| session.get("agentId")).cloned().unwrap_or_else(|| json!("")),
        "source_path": session.get("sourcePath").cloned().unwrap_or_else(|| json!("")),
        "native_session_id": session.get("nativeSessionId").cloned().unwrap_or_else(|| json!("")),
        "matched_terms": profile_match.matched_terms.clone(),
        "confidence": profile_match.confidence,
        "reason": profile_match.reason
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_replaces_by_snapshot_identity_and_keeps_refresh_order() {
        let mut records = vec![json!({"snapshotId": "a", "refreshedAt": "1", "value": 1})];
        upsert_conversation_record(
            &mut records,
            json!({"snapshotId": "a", "refreshedAt": "3", "value": 2}),
        );
        upsert_conversation_record(
            &mut records,
            json!({"snapshotId": "b", "refreshedAt": "2", "value": 3}),
        );

        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["snapshotId"], "a");
        assert_eq!(records[0]["value"], 2);
        assert_eq!(
            archive_status_for(Some(&records[0]), "different"),
            "updated"
        );
    }
}
