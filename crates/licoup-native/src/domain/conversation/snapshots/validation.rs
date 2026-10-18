//! Collection verification, content integrity, and baseline coverage.

use super::*;

pub(crate) fn archive_verify(params: &Value) -> Result<Value> {
    if text_param(params, &["collectionPath", "collection", "path"])
        .filter(|value| !value.trim().is_empty())
        .is_some()
    {
        return archive_verify_collection_path(params);
    }
    let store = client_state_store(params)?;
    let profile = load_archive_profile(&store, params)?;
    let root = archive_root_for_profile(&store, params, &profile)?;
    let collection_dir = collection_dir_for_profile(&root, &profile);
    let index_records = read_index_records(&collection_dir.join(CONVERSATION_INDEX_JSONL))?;
    let validation = validate_archive_collection(&collection_dir, &index_records, &profile)?;
    atomic_write_json(&collection_dir.join(VALIDATION_JSON), &validation)?;
    Ok(json!({
        "ok": true,
        "schemaVersion": COLLECTION_SCHEMA_VERSION,
        "mode": "conversation-archive-verify",
        "profileId": profile.profile_id,
        "collectionPathSegments": profile.collection_path_segments,
        "collectionPath": display_path(&collection_dir.join(COLLECTION_JSON)),
        "validation": validation
    }))
}

pub(super) fn archive_verify_collection_path(params: &Value) -> Result<Value> {
    let raw_path = text_param(params, &["collectionPath", "collection", "path"])
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("archive verify requires --collection-path"))?;
    let requested = expand_home(&raw_path);
    let collection_path = if requested.is_dir() {
        requested.join(COLLECTION_JSON)
    } else {
        requested.clone()
    };
    let collection_dir = collection_path
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("archive collection path has no parent directory"))?;
    let collection = read_json_or_default(&collection_path, || json!({}))?;
    let profile_value = collection
        .get("archiveProfile")
        .cloned()
        .ok_or_else(|| anyhow!("archive collection is missing archiveProfile"))?;
    let profile = parse_archive_profile(&profile_value)?;
    let index_records = read_index_records(&collection_dir.join(CONVERSATION_INDEX_JSONL))?;
    let validation = validate_archive_collection(&collection_dir, &index_records, &profile)?;
    atomic_write_json(&collection_dir.join(VALIDATION_JSON), &validation)?;
    let mut updated_collection = collection.as_object().cloned().unwrap_or_default();
    updated_collection.insert("archiveHealth".to_string(), validation.clone());
    updated_collection.insert("refreshedAt".to_string(), json!(timestamp_rfc3339()));
    atomic_write_json(&collection_path, &Value::Object(updated_collection))?;
    Ok(json!({
        "ok": true,
        "schemaVersion": COLLECTION_SCHEMA_VERSION,
        "mode": "conversation-archive-verify",
        "profileId": profile.profile_id,
        "collectionPathSegments": profile.collection_path_segments,
        "collectionPath": display_path(&collection_path),
        "validation": validation
    }))
}

pub(super) fn validate_archive_collection(
    collection_dir: &Path,
    index_records: &[Value],
    profile: &ArchiveProfile,
) -> Result<Value> {
    validate_archive_collection_with_options(collection_dir, index_records, profile, None, None)
}

pub(super) fn validate_archive_collection_for_relocation(
    collection_dir: &Path,
    index_records: &[Value],
    profile: &ArchiveProfile,
    data_root: &Path,
    logical_root: &Path,
    preserved_external_baseline: Option<Value>,
) -> Result<Value> {
    validate_archive_collection_with_options(
        collection_dir,
        index_records,
        profile,
        Some((data_root, logical_root)),
        preserved_external_baseline,
    )
}

fn validate_archive_collection_with_options(
    collection_dir: &Path,
    index_records: &[Value],
    profile: &ArchiveProfile,
    data_root: Option<(&Path, &Path)>,
    baseline_override: Option<Value>,
) -> Result<Value> {
    let mut issues = Vec::<Value>::new();
    let mut archive_keys = BTreeMap::<String, usize>::new();
    let mut fingerprints = BTreeMap::<String, usize>::new();
    let mut indexed_snapshot_paths = BTreeSet::<String>::new();
    let mut total_bytes = 0u64;

    for record in index_records {
        let archive_key = record
            .get("archive_key")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if archive_key.is_empty() {
            issues.push(json!({"type": "missing_archive_key", "severity": "error"}));
        } else {
            *archive_keys.entry(archive_key).or_insert(0) += 1;
        }
        let fingerprint = record
            .get("content_fingerprint")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if fingerprint.is_empty() {
            issues.push(json!({
                "type": "missing_fingerprint",
                "severity": "error",
                "archive_key": record.get("archive_key").cloned().unwrap_or_else(|| json!(""))
            }));
        } else {
            *fingerprints.entry(fingerprint.clone()).or_insert(0) += 1;
        }
        for key in [
            "snapshot_path",
            "raw_content_path",
            "semantic_document_path",
            "semantic_markdown_path",
        ] {
            let path = record.get(key).and_then(Value::as_str).unwrap_or_default();
            if path.is_empty() || validation_file_path(path, data_root).is_none() {
                issues.push(json!({
                    "type": if key.starts_with("semantic_") {
                        "missing_semantic_document"
                    } else {
                        "missing_file"
                    },
                    "severity": "error",
                    "field": key,
                    "path": path,
                    "archive_key": record.get("archive_key").cloned().unwrap_or_else(|| json!(""))
                }));
            }
            if key == "snapshot_path" && !path.is_empty() {
                indexed_snapshot_paths.insert(path.to_string());
            }
        }
        if let Some(semantic_hash) = record
            .get("semantic_content_hash")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            let semantic_path = record
                .get("semantic_document_path")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if let Some(semantic_file) = validation_file_path(semantic_path, data_root) {
                let content = fs::read_to_string(semantic_file)?;
                let actual = hash_text(&content.trim_end_matches('\n'));
                // materialize writes pretty JSON + trailing newline; compare both forms.
                let actual_raw = hash_text(&content);
                if actual != semantic_hash && actual_raw != semantic_hash {
                    issues.push(json!({
                        "type": "stale_semantic_hash",
                        "severity": "error",
                        "archive_key": record.get("archive_key").cloned().unwrap_or_else(|| json!("")),
                        "path": semantic_path,
                        "expectedFingerprint": semantic_hash,
                        "actualFingerprint": actual_raw
                    }));
                }
                if let Ok(semantic_value) = serde_json::from_str::<Value>(&content) {
                    if let Err(error) =
                        crate::domain::conversation_semantic::validate_semantic_conversation(
                            &semantic_value,
                        )
                    {
                        issues.push(json!({
                            "type": "invalid_semantic_document",
                            "severity": "error",
                            "archive_key": record.get("archive_key").cloned().unwrap_or_else(|| json!("")),
                            "path": semantic_path,
                            "detail": error.to_string()
                        }));
                    }
                    let thread_empty = semantic_value
                        .get("thread")
                        .and_then(Value::as_array)
                        .map(|items| items.is_empty())
                        .unwrap_or(true);
                    let execution_empty = semantic_value
                        .get("execution")
                        .and_then(Value::as_array)
                        .map(|items| items.is_empty())
                        .unwrap_or(true);
                    if thread_empty
                        && execution_empty
                        && record
                            .get("match_reason")
                            .and_then(Value::as_str)
                            .is_some_and(|reason| reason.contains("metadata"))
                    {
                        issues.push(json!({
                            "type": "metadata_only_false_positive",
                            "severity": "warning",
                            "archive_key": record.get("archive_key").cloned().unwrap_or_else(|| json!("")),
                            "path": semantic_path
                        }));
                    }
                }
            }
        }
        if let Some(bytes) = record.get("raw_content_bytes").and_then(Value::as_u64) {
            total_bytes += bytes;
            let raw_path = record
                .get("raw_content_path")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if let Some(raw_file) = validation_file_path(raw_path, data_root) {
                let actual_bytes = fs::metadata(raw_file)?.len();
                if actual_bytes != bytes {
                    issues.push(json!({
                        "type": "raw_content_size_mismatch",
                        "severity": "error",
                        "archive_key": record.get("archive_key").cloned().unwrap_or_else(|| json!("")),
                        "path": raw_path,
                        "expectedBytes": bytes,
                        "actualBytes": actual_bytes
                    }));
                }
            }
        }
        if !fingerprint.is_empty() {
            let raw_path = record
                .get("raw_content_path")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if let Some(raw_file) = validation_file_path(raw_path, data_root) {
                let content = fs::read(raw_file)?;
                let actual_fingerprint = hash_bytes(&content);
                if actual_fingerprint != fingerprint {
                    issues.push(json!({
                        "type": "raw_content_fingerprint_mismatch",
                        "severity": "error",
                        "archive_key": record.get("archive_key").cloned().unwrap_or_else(|| json!("")),
                        "path": raw_path,
                        "expectedFingerprint": fingerprint,
                        "actualFingerprint": actual_fingerprint
                    }));
                }
            }
        }
    }

    for (key, count) in archive_keys {
        if count > 1 {
            issues.push(json!({
                "type": "duplicate_archive_key",
                "severity": "error",
                "archive_key": key,
                "count": count
            }));
        }
    }
    for (fingerprint, count) in fingerprints {
        if count > 1 {
            issues.push(json!({
                "type": "duplicate_content_fingerprint",
                "severity": "warning",
                "content_fingerprint": fingerprint,
                "count": count
            }));
        }
    }
    let conversations_dir = collection_dir.join("conversations");
    if conversations_dir.exists() {
        for entry in fs::read_dir(&conversations_dir)? {
            let entry = entry?;
            let snapshot_path = entry.path().join(SNAPSHOT_JSON);
            let snapshot_path = data_root
                .and_then(|(physical, logical)| {
                    snapshot_path
                        .strip_prefix(physical)
                        .ok()
                        .map(|relative| logical.join(relative))
                })
                .unwrap_or(snapshot_path);
            if validation_file_path(&display_path(&snapshot_path), data_root).is_some()
                && !indexed_snapshot_paths.contains(display_path(&snapshot_path).as_str())
            {
                issues.push(json!({
                    "type": "stale_unindexed",
                    "severity": "warning",
                    "snapshot_path": display_path(&snapshot_path)
                }));
            }
        }
    }
    let baseline = if let Some(baseline) = baseline_override {
        baseline
    } else if let Some(data_root) = data_root {
        baseline_coverage_with_root(profile, index_records, total_bytes, data_root)?
    } else {
        baseline_coverage(profile, index_records, total_bytes)?
    };
    let error_count = issues
        .iter()
        .filter(|issue| issue.get("severity").and_then(Value::as_str) == Some("error"))
        .count();
    let warning_count = issues.len().saturating_sub(error_count);
    let health_status = if error_count > 0 {
        "failed"
    } else if warning_count > 0 {
        "warning"
    } else {
        "ok"
    };
    Ok(json!({
        "schemaVersion": COLLECTION_SCHEMA_VERSION,
        "profileId": profile.profile_id,
        "checkedAt": timestamp_rfc3339(),
        "healthStatus": health_status,
        "recordCount": index_records.len(),
        "rawContentBytes": total_bytes,
        "errorCount": error_count,
        "warningCount": warning_count,
        "issues": issues,
        "baseline": baseline
    }))
}

fn validation_file_path(path: &str, data_root: Option<(&Path, &Path)>) -> Option<PathBuf> {
    if path.is_empty() {
        return None;
    }
    let path = Path::new(path);
    match data_root {
        Some((physical, logical)) => {
            super::relocation::copied_logical_file(physical, logical, path)
        }
        None if path.exists() => Some(path.to_path_buf()),
        None => None,
    }
}

pub(super) fn baseline_coverage(
    profile: &ArchiveProfile,
    index_records: &[Value],
    total_bytes: u64,
) -> Result<Value> {
    let Some(path) = &profile.baseline_index_path else {
        return Ok(json!({"configured": false}));
    };
    if !path.exists() {
        return Ok(json!({
            "configured": true,
            "status": "missing_baseline",
            "baselineIndexPath": display_path(path)
        }));
    }
    let records = read_index_records(path)?;
    let baseline_count = records.len() as u64;
    let baseline_bytes = records.iter().filter_map(record_numeric_bytes).sum::<u64>();
    let current_count = index_records.len() as u64;
    Ok(json!({
        "configured": true,
        "status": "compared",
        "baselineIndexPath": display_path(path),
        "baselineCount": baseline_count,
        "currentCount": current_count,
        "countCoverage": if baseline_count == 0 { 1.0 } else { current_count as f64 / baseline_count as f64 },
        "baselineBytes": baseline_bytes,
        "currentBytes": total_bytes,
        "byteCoverage": if baseline_bytes == 0 { 1.0 } else { total_bytes as f64 / baseline_bytes as f64 }
    }))
}

fn baseline_coverage_with_root(
    profile: &ArchiveProfile,
    index_records: &[Value],
    total_bytes: u64,
    data_root: (&Path, &Path),
) -> Result<Value> {
    let Some(path) = &profile.baseline_index_path else {
        return Ok(json!({"configured": false}));
    };
    let Some(copied_path) = super::relocation::copied_logical_file(data_root.0, data_root.1, path)
    else {
        return Ok(json!({
            "configured": true,
            "status": "missing_baseline",
            "baselineIndexPath": display_path(path)
        }));
    };
    let records = read_index_records(&copied_path)?;
    let baseline_count = records.len() as u64;
    let baseline_bytes = records.iter().filter_map(record_numeric_bytes).sum::<u64>();
    let current_count = index_records.len() as u64;
    Ok(json!({
        "configured": true,
        "status": "compared",
        "baselineIndexPath": display_path(path),
        "baselineCount": baseline_count,
        "currentCount": current_count,
        "countCoverage": if baseline_count == 0 { 1.0 } else { current_count as f64 / baseline_count as f64 },
        "baselineBytes": baseline_bytes,
        "currentBytes": total_bytes,
        "byteCoverage": if baseline_bytes == 0 { 1.0 } else { total_bytes as f64 / baseline_bytes as f64 }
    }))
}

pub(super) fn record_numeric_bytes(record: &Value) -> Option<u64> {
    for key in [
        "raw_content_bytes",
        "rawContentBytes",
        "bytes",
        "contentBytes",
        "conversation_bytes",
    ] {
        if let Some(value) = record.get(key).and_then(Value::as_u64) {
            return Some(value);
        }
    }
    None
}
