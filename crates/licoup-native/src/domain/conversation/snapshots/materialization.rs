//! Parallel snapshot materialization, semantic artifacts, and incremental indexes.

use super::*;

pub(super) fn materialize_archive_work_items_parallel(
    collection_dir: &Path,
    topic: &str,
    topic_key: &str,
    refreshed_at: &str,
    work_items: Vec<ArchiveMaterializeWorkItem>,
    parallelism: usize,
) -> Result<Vec<ArchiveMaterializeResult>> {
    if work_items.is_empty() {
        return Ok(Vec::new());
    }
    let worker_count = parallelism.max(1).min(work_items.len());
    let chunk_size = work_items.len().div_ceil(worker_count);
    let mut results = Vec::<ArchiveMaterializeResult>::with_capacity(work_items.len());

    thread::scope(|scope| -> Result<()> {
        let mut handles = Vec::new();
        for chunk in work_items.chunks(chunk_size) {
            let chunk = chunk.to_vec();
            handles.push(
                scope.spawn(move || -> Result<Vec<ArchiveMaterializeResult>> {
                    let mut chunk_results =
                        Vec::<ArchiveMaterializeResult>::with_capacity(chunk.len());
                    for item in chunk {
                        let archive_key = archive_key_for_session(&item.selected.session);
                        let record = materialize_snapshot(
                            collection_dir,
                            topic,
                            topic_key,
                            &item.selected,
                            refreshed_at,
                        )?;
                        chunk_results.push(ArchiveMaterializeResult {
                            position: item.position,
                            archive_key,
                            session: item.selected.session,
                            profile_match: item.profile_match,
                            record,
                        });
                    }
                    Ok(chunk_results)
                }),
            );
        }
        for handle in handles {
            match handle.join() {
                Ok(Ok(mut chunk_results)) => results.append(&mut chunk_results),
                Ok(Err(error)) => return Err(error),
                Err(_) => return Err(anyhow!("archive materialize worker panicked")),
            }
        }
        Ok(())
    })?;

    Ok(results)
}

pub(super) fn archive_parallelism(params: &Value) -> usize {
    if let Some(value) = usize_param(params, &["archiveParallelism", "parallelism"]) {
        return value.max(1);
    }
    thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(4)
        .clamp(1, 8)
}

pub(super) fn materialize_snapshot(
    collection_dir: &Path,
    topic: &str,
    topic_key: &str,
    selected: &SelectedCandidate,
    refreshed_at: &str,
) -> Result<Value> {
    let session = &selected.session;
    let metadata = snapshot_source_metadata(session);
    let snapshot_hash = hash_parts(&[&metadata.adapter_id, &metadata.native_identity]);
    let snapshot_id = format!("native-conversation-{}", &snapshot_hash[..24]);
    let conversation_dir = collection_dir.join("conversations").join(&snapshot_hash);
    fs::create_dir_all(&conversation_dir)?;

    let raw = export_raw_content(session)?;
    let raw_path = conversation_dir.join(&raw.file_name);
    atomic_write_text(&raw_path, &raw.content)?;
    let raw_hash = hash_text(&raw.content);
    let semantic_document =
        project_archive_semantic_document(session, &raw, &snapshot_hash, refreshed_at, &metadata);
    let (semantic_json_path, semantic_md_path, semantic_hash) =
        crate::domain::conversation_semantic::materialize_semantic_documents(
            &conversation_dir,
            &semantic_document,
        )?;
    let snapshot_path = conversation_dir.join(SNAPSHOT_JSON);
    let snapshot = json!({
        "schemaVersion": COLLECTION_SCHEMA_VERSION,
        "kind": "native-conversation-snapshot",
        "snapshotId": snapshot_id,
        "topic": topic,
        "topicKey": topic_key,
        "agentAdapterId": metadata.adapter_id,
        "sourceClient": metadata.source_client,
        "sourceClientLabel": metadata.source_client_label,
        "hostApp": metadata.host_app,
        "hostAppLabel": metadata.host_app_label,
        "sourceLabel": metadata.source_label,
        "nativeConversationIdentity": metadata.native_identity,
        "sourcePath": session.get("sourcePath").cloned().unwrap_or_else(|| json!("")),
        "sourceKind": session.get("sourceKind").cloned().unwrap_or_else(|| json!("")),
        "title": session.get("title").cloned().unwrap_or_else(|| json!("Native agent history")),
        "messageCount": session.get("messageCount").cloned().unwrap_or_else(|| json!(0)),
        "selection": {
            "mode": selected.selection_mode,
            "reason": selected.reason,
            "labels": selected.labels,
            "group": selected.group,
            "summary": selected.summary
        },
        "semanticDocumentFile": SEMANTIC_JSON,
        "semanticDocumentPath": display_path(&semantic_json_path),
        "semanticMarkdownFile": SEMANTIC_MD,
        "semanticMarkdownPath": display_path(&semantic_md_path),
        "semanticContentHash": semantic_hash,
        "semantic": semantic_document,
        "rawContentFile": raw.file_name,
        "rawContentPath": display_path(&raw_path),
        "rawContentHash": raw_hash,
        "rawContentBytes": raw.content.len(),
        "rawExportKind": raw.export_kind,
        "diagnostics": raw.diagnostics,
        "createdAt": existing_created_at(&snapshot_path).unwrap_or_else(|| refreshed_at.to_string()),
        "refreshedAt": refreshed_at
    });
    atomic_write_json(&snapshot_path, &snapshot)?;
    Ok(json!({
        "snapshotId": snapshot.get("snapshotId").cloned().unwrap_or_else(|| json!("")),
        "agentAdapterId": snapshot.get("agentAdapterId").cloned().unwrap_or_else(|| json!("")),
        "sourceClient": snapshot.get("sourceClient").cloned().unwrap_or_else(|| json!("")),
        "sourceClientLabel": snapshot.get("sourceClientLabel").cloned().unwrap_or_else(|| json!("")),
        "hostApp": snapshot.get("hostApp").cloned().unwrap_or_else(|| json!("")),
        "hostAppLabel": snapshot.get("hostAppLabel").cloned().unwrap_or_else(|| json!("")),
        "sourceLabel": snapshot.get("sourceLabel").cloned().unwrap_or_else(|| json!("")),
        "nativeConversationIdentity": snapshot.get("nativeConversationIdentity").cloned().unwrap_or_else(|| json!("")),
        "title": snapshot.get("title").cloned().unwrap_or_else(|| json!("")),
        "sourcePath": snapshot.get("sourcePath").cloned().unwrap_or_else(|| json!("")),
        "messageCount": snapshot.get("messageCount").cloned().unwrap_or_else(|| json!(0)),
        "snapshotPath": display_path(&snapshot_path),
        "semanticDocumentPath": display_path(&semantic_json_path),
        "semanticMarkdownPath": display_path(&semantic_md_path),
        "semanticContentHash": snapshot.get("semanticContentHash").cloned().unwrap_or_else(|| json!("")),
        "rawContentPath": display_path(&raw_path),
        "rawContentHash": snapshot.get("rawContentHash").cloned().unwrap_or_else(|| json!("")),
        "rawContentBytes": snapshot.get("rawContentBytes").cloned().unwrap_or_else(|| json!(0)),
        "rawExportKind": snapshot.get("rawExportKind").cloned().unwrap_or_else(|| json!("")),
        "selection": snapshot.get("selection").cloned().unwrap_or_else(|| json!({})),
        "createdAt": snapshot.get("createdAt").cloned().unwrap_or_else(|| json!("")),
        "refreshedAt": refreshed_at
    }))
}

pub(super) fn read_index_records(path: &Path) -> Result<Vec<Value>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(path)?;
    let mut records = Vec::<Value>::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        records.push(serde_json::from_str(trimmed)?);
    }
    Ok(records)
}

pub(super) fn index_records_by_archive_key(records: &[Value]) -> BTreeMap<String, Value> {
    records
        .iter()
        .filter_map(|record| {
            record
                .get("archive_key")
                .and_then(Value::as_str)
                .map(|key| (key.to_string(), record.clone()))
        })
        .collect()
}

pub(super) fn append_preserved_index_records(
    previous_index: &[Value],
    current_keys: &BTreeSet<String>,
    index_records: &mut Vec<Value>,
) {
    for record in previous_index {
        let archive_key = record
            .get("archive_key")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if archive_key.is_empty() || current_keys.contains(archive_key) {
            continue;
        }
        if record
            .get("source_path")
            .and_then(Value::as_str)
            .map(excluded_archive_source_path)
            .unwrap_or(false)
        {
            continue;
        }
        let mut preserved = record.as_object().cloned().unwrap_or_default();
        preserved.insert("archive_status".to_string(), json!("preserved"));
        preserved.insert("preserved_at".to_string(), json!(timestamp_rfc3339()));
        if let Some(source_path) = preserved.get("source_path").and_then(Value::as_str) {
            if !source_path.is_empty() && !Path::new(source_path).exists() {
                preserved.insert("source_status".to_string(), json!("missing_source"));
            }
        }
        index_records.push(Value::Object(preserved));
    }
}

pub(super) fn excluded_archive_source_path(source_path: &str) -> bool {
    Path::new(source_path)
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .any(|name| {
            matches!(
                name,
                "node_modules" | ".git" | "target" | "build" | "dist" | ".next"
            )
        })
}

pub(super) fn prune_excluded_unindexed_snapshots(
    collection_dir: &Path,
    index_records: &[Value],
) -> Result<()> {
    let indexed_snapshot_paths = index_records
        .iter()
        .filter_map(|record| record.get("snapshot_path").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    let conversations_dir = collection_dir.join("conversations");
    if !conversations_dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(&conversations_dir)? {
        let entry = entry?;
        let snapshot_path = entry.path().join(SNAPSHOT_JSON);
        if !snapshot_path.exists()
            || indexed_snapshot_paths.contains(display_path(&snapshot_path).as_str())
        {
            continue;
        }
        let snapshot = read_json_or_default(&snapshot_path, || json!({}))?;
        if snapshot
            .get("sourcePath")
            .and_then(Value::as_str)
            .map(excluded_archive_source_path)
            .unwrap_or(false)
        {
            fs::remove_dir_all(entry.path())?;
        }
    }
    Ok(())
}

pub(super) fn write_jsonl(path: &Path, records: &[Value]) -> Result<()> {
    let mut lines = Vec::<String>::new();
    for record in records {
        lines.push(serde_json::to_string(record)?);
    }
    let content = if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    };
    atomic_write_text(path, &content)
}
