//! Archive reports, Markdown indexes, summaries, and workflow diagnostics.

use super::*;

pub(crate) fn archive_report(params: &Value) -> Result<Value> {
    let store = client_state_store(params)?;
    let profile = load_archive_profile(&store, params)?;
    let root = archive_root_for_profile(&store, params, &profile)?;
    let collection_dir = collection_dir_for_profile(&root, &profile);
    let collection = read_json_or_default(&collection_dir.join(COLLECTION_JSON), || json!({}))?;
    let validation = read_json_or_default(&collection_dir.join(VALIDATION_JSON), || json!({}))?;
    let sources = read_json_or_default(&collection_dir.join(SOURCES_JSON), || json!({}))?;
    let index_records = read_index_records(&collection_dir.join(CONVERSATION_INDEX_JSONL))?;
    Ok(json!({
        "ok": true,
        "schemaVersion": COLLECTION_SCHEMA_VERSION,
        "mode": "conversation-archive-report",
        "profileId": profile.profile_id,
        "collectionPathSegments": profile.collection_path_segments,
        "displayName": profile.display_name,
        "snapshotRoot": display_path(&root),
        "collectionPath": display_path(&collection_dir.join(COLLECTION_JSON)),
        "conversationIndexPath": display_path(&collection_dir.join(CONVERSATION_INDEX_JSONL)),
        "summaryPath": display_path(&collection_dir.join(SUMMARY_MD)),
        "indexCount": index_records.len(),
        "collection": collection_summary(&collection, &collection_dir.join(COLLECTION_JSON)),
        "validation": validation,
        "sources": sources
    }))
}

pub(super) fn conversation_index_markdown(
    profile: &ArchiveProfile,
    index_records: &[Value],
    validation: &Value,
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# Conversation Index: {}\n\n",
        profile.display_name
    ));
    out.push_str(&format!(
        "- Profile: `{}`\n- Records: {}\n- Health: `{}`\n\n",
        profile.profile_id,
        index_records.len(),
        validation
            .get("healthStatus")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    ));
    out.push_str("| Title | Source | Adapter | Confidence | Status | Semantic |\n");
    out.push_str("| --- | --- | --- | --- | --- | --- |\n");
    for record in index_records {
        out.push_str(&format!(
            "| {} | `{}` | `{}` | `{}` | `{}` | `{}` |\n",
            markdown_cell(
                record
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        record
                            .get("archive_key")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                    })
            ),
            record
                .get("source_label")
                .or_else(|| record.get("source_tool"))
                .and_then(Value::as_str)
                .unwrap_or(""),
            record
                .get("adapter_id")
                .and_then(Value::as_str)
                .unwrap_or(""),
            record
                .get("confidence")
                .and_then(Value::as_str)
                .unwrap_or(""),
            record
                .get("archive_status")
                .and_then(Value::as_str)
                .unwrap_or(""),
            record
                .get("semantic_markdown_path")
                .or_else(|| record.get("semantic_document_path"))
                .and_then(Value::as_str)
                .unwrap_or("")
        ));
    }
    out
}

pub(super) fn markdown_cell(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('|', "\\|")
}

pub(super) fn archive_summary_markdown(
    profile: &ArchiveProfile,
    root: &Path,
    candidate_count: usize,
    source_summaries: &[Value],
    index_records: &[Value],
    validation: &Value,
) -> String {
    let mut by_source = BTreeMap::<String, usize>::new();
    for record in index_records {
        let source = record
            .get("source_label")
            .or_else(|| record.get("source_tool"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        *by_source.entry(source).or_insert(0) += 1;
    }
    let mut out = String::new();
    out.push_str(&format!(
        "# Conversation Archive Summary: {}\n\n",
        profile.display_name
    ));
    out.push_str(&format!(
        "- Profile: `{}`\n- Archive root: `{}`\n- Candidates scanned: {}\n- Archived records: {}\n- Health: `{}`\n\n",
        profile.profile_id,
        display_path(root),
        candidate_count,
        index_records.len(),
        validation.get("healthStatus").and_then(Value::as_str).unwrap_or("unknown")
    ));
    out.push_str("## By Source\n\n");
    for (source, count) in by_source {
        out.push_str(&format!("- `{}`: {}\n", source, count));
    }
    out.push_str("\n## Source Coverage\n\n");
    for source in source_summaries {
        out.push_str(&format!(
            "- `{}` `{}`: {} sessions, {} files seen\n",
            source.get("agentId").and_then(Value::as_str).unwrap_or(""),
            source.get("scope").and_then(Value::as_str).unwrap_or(""),
            source
                .get("sessionCount")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            source.get("filesSeen").and_then(Value::as_u64).unwrap_or(0)
        ));
    }
    out
}

pub(super) fn archive_workflow_diagnostics(
    discovery: &DiscoveryResult,
    validation: &Value,
    index_count: usize,
) -> Vec<Value> {
    let mut diagnostics = discovery.diagnostics.clone();
    diagnostics.push(json!({
        "stage": "archive_profile_matching",
        "status": "completed",
        "indexCount": index_count,
        "candidateCount": discovery.candidates.len()
    }));
    diagnostics.push(json!({
        "stage": "archive_validation",
        "status": validation.get("healthStatus").cloned().unwrap_or_else(|| json!("unknown")),
        "errorCount": validation.get("errorCount").cloned().unwrap_or_else(|| json!(0)),
        "warningCount": validation.get("warningCount").cloned().unwrap_or_else(|| json!(0))
    }));
    diagnostics
}
