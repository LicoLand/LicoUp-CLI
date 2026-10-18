//! Archive collection orchestration and bounded keyword fan-out.

use super::*;

pub(crate) fn archive_run(params: &Value) -> Result<Value> {
    let store = client_state_store(params)?;
    let profile = load_archive_profile(&store, params)?;
    let root = archive_root_for_profile(&store, params, &profile)?;
    run_archive_with_profile(&store, params, profile, root, "archive")
}

pub(super) fn run_archive_with_profile(
    store: &ClientStateStore,
    params: &Value,
    profile: ArchiveProfile,
    root: PathBuf,
    entry: &str,
) -> Result<Value> {
    run_archive_with_profile_layout(
        store,
        params,
        profile,
        root,
        entry,
        ArchiveCollectionLayout::CollectionsSubdir,
    )
}

pub(super) fn run_archive_with_profile_layout(
    store: &ClientStateStore,
    params: &Value,
    profile: ArchiveProfile,
    root: PathBuf,
    entry: &str,
    layout: ArchiveCollectionLayout,
) -> Result<Value> {
    let discovery = discover_archive_candidates(store, params, &profile);
    run_archive_with_profile_discovery(store, params, profile, root, entry, layout, &discovery)
}

pub(super) fn run_archive_with_profile_discovery(
    store: &ClientStateStore,
    params: &Value,
    profile: ArchiveProfile,
    root: PathBuf,
    entry: &str,
    layout: ArchiveCollectionLayout,
    discovery: &DiscoveryResult,
) -> Result<Value> {
    ensure_snapshot_root(&root)?;
    let collection_dir = collection_dir_for_profile_layout(&root, &profile, layout);
    fs::create_dir_all(collection_dir.join("conversations"))?;
    let collection_path = collection_dir.join(COLLECTION_JSON);
    let existing = read_json_or_default(&collection_path, || {
        empty_collection(
            &profile.display_name,
            &profile.profile_id,
            &root,
            &timestamp_rfc3339(),
        )
    })?;
    let (selected, matches_by_id) = select_profile_archive_candidates(&profile, discovery);
    let refreshed_at = timestamp_rfc3339();
    let mut conversations = existing_conversations(&existing);
    let previous_index = read_index_records(&collection_dir.join(CONVERSATION_INDEX_JSONL))?;
    let previous_by_key = index_records_by_archive_key(&previous_index);
    let mut current_keys = BTreeSet::<String>::new();
    let mut index_records = Vec::<Value>::new();
    let mut match_records = Vec::<Value>::new();
    let mut written = Vec::<Value>::new();

    let work_items = selected
        .into_iter()
        .enumerate()
        .map(|(position, selected_candidate)| {
            let candidate_id = candidate_id(&selected_candidate.session).unwrap_or_default();
            let profile_match = matches_by_id
                .get(&candidate_id)
                .cloned()
                .unwrap_or(ProfileMatch {
                    matched_terms: vec!["deterministic".to_string()],
                    confidence: "medium".to_string(),
                    reason: "deterministic profile match selected this candidate".to_string(),
                });
            ArchiveMaterializeWorkItem {
                position,
                selected: selected_candidate,
                profile_match,
            }
        })
        .collect::<Vec<_>>();
    let mut materialized = materialize_archive_work_items_parallel(
        &collection_dir,
        &profile.display_name,
        &profile.profile_id,
        &refreshed_at,
        work_items,
        archive_parallelism(params),
    )?;

    materialized.sort_by_key(|item| item.position);
    for item in materialized {
        let profile_match = item.profile_match;
        let record = item.record;
        let archive_key = item.archive_key;
        current_keys.insert(archive_key.clone());
        let status = archive_status_for(
            previous_by_key.get(&archive_key),
            record
                .get("rawContentHash")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
        let index_record = archive_index_record(
            &archive_key,
            &item.session,
            &record,
            &profile_match,
            &status,
            &refreshed_at,
        );
        match_records.push(archive_match_record(
            &archive_key,
            &item.session,
            &profile_match,
        ));
        index_records.push(index_record);
        upsert_conversation_record(&mut conversations, record.clone());
        written.push(record);
    }

    append_preserved_index_records(&previous_index, &current_keys, &mut index_records);
    index_records.sort_by(|left, right| {
        left.get("archive_key")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(
                right
                    .get("archive_key")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
    });
    write_jsonl(
        &collection_dir.join(CONVERSATION_INDEX_JSONL),
        &index_records,
    )?;
    conversations.retain(|record| {
        !record
            .get("sourcePath")
            .and_then(Value::as_str)
            .map(excluded_archive_source_path)
            .unwrap_or(false)
    });
    prune_excluded_unindexed_snapshots(&collection_dir, &index_records)?;
    write_jsonl(&collection_dir.join(MATCHES_JSONL), &match_records)?;
    atomic_write_json(
        &collection_dir.join(SOURCES_JSON),
        &json!({
            "schemaVersion": COLLECTION_SCHEMA_VERSION,
            "profileId": profile.profile_id,
            "collectionPathSegments": profile.collection_path_segments,
            "generatedAt": refreshed_at,
            "agents": discovery.agents,
            "sources": discovery.source_summaries,
            "diagnostics": discovery.diagnostics
        }),
    )?;

    let validation = validate_archive_collection(&collection_dir, &index_records, &profile)?;
    atomic_write_json(&collection_dir.join(VALIDATION_JSON), &validation)?;
    atomic_write_text(
        &collection_dir.join(CONVERSATION_INDEX_MD),
        &conversation_index_markdown(&profile, &index_records, &validation),
    )?;
    atomic_write_text(
        &collection_dir.join(SUMMARY_MD),
        &archive_summary_markdown(
            &profile,
            &root,
            discovery.candidates.len(),
            &discovery.source_summaries,
            &index_records,
            &validation,
        ),
    )?;

    let workflow_diagnostics =
        archive_workflow_diagnostics(&discovery, &validation, index_records.len());
    let status = if index_records.is_empty() {
        "empty"
    } else {
        "materialized"
    };
    let collection = build_collection(
        &existing,
        &profile.display_name,
        &profile.profile_id,
        &root,
        status,
        conversations,
        &refreshed_at,
        &discovery.source_summaries,
        &workflow_diagnostics,
        written.len(),
        discovery.candidates.len(),
    );
    let mut collection_object = collection.as_object().cloned().unwrap_or_default();
    collection_object.insert("kind".to_string(), json!("native-conversation-archive"));
    collection_object.insert(
        "archiveProfile".to_string(),
        archive_profile_value(&profile),
    );
    collection_object.insert("archiveHealth".to_string(), validation.clone());
    atomic_write_json(&collection_path, &Value::Object(collection_object))?;

    let trigger = text_param(params, &["trigger"]).unwrap_or_else(|| "manual".to_string());
    let activity = store.activity_log().append(
        "conversation_snapshots.archive_run",
        json!({
            "target": "conversation-snapshots",
            "entry": entry,
            "trigger": trigger,
            "profileId": profile.profile_id,
            "collectionPathSegments": profile.collection_path_segments,
            "snapshotRoot": display_path(&root),
            "collectionPath": display_path(&collection_path),
            "candidateCount": discovery.candidates.len(),
            "selectedCount": written.len(),
            "indexCount": index_records.len(),
            "healthStatus": validation.get("healthStatus").cloned().unwrap_or_else(|| json!("unknown")),
            "status": status
        }),
    )?;
    Ok(json!({
        "ok": true,
        "status": status,
        "schemaVersion": COLLECTION_SCHEMA_VERSION,
        "mode": "conversation-archive",
        "entry": entry,
        "profileId": profile.profile_id,
        "collectionPathSegments": profile.collection_path_segments,
        "displayName": profile.display_name,
        "snapshotRoot": display_path(&root),
        "collectionPath": display_path(&collection_path),
        "candidateCount": discovery.candidates.len(),
        "selectedCount": written.len(),
        "indexCount": index_records.len(),
        "written": written,
        "sourcesPath": display_path(&collection_dir.join(SOURCES_JSON)),
        "conversationIndexPath": display_path(&collection_dir.join(CONVERSATION_INDEX_JSONL)),
        "summaryPath": display_path(&collection_dir.join(SUMMARY_MD)),
        "diagnostics": discovery.diagnostics,
        "validation": validation,
        "activity": activity
    }))
}

pub(crate) fn archive_collect(params: &Value) -> Result<Value> {
    let store = client_state_store(params)?;
    let keywords = archive_keywords(params)?;
    let archive_root = archive_destination(params)?;
    let target_scan = archive_target_scan(params)?;
    let agents = archive_agents_from_target_scan(params, &target_scan);
    if agents.is_empty() {
        return Ok(json!({
            "ok": false,
            "status": "no_supported_clients_detected",
            "mode": "conversation-archive",
            "entry": "keyword-archive",
            "keywords": keywords,
            "archiveRoot": display_path(&archive_root),
            "message": "No supported local agent clients were detected for conversation archive.",
            "targetScan": archive_target_scan_summary(&target_scan, &[])
        }));
    }
    let profiles = derived_keyword_archive_profiles(&keywords, &archive_root, &agents)?;
    let run_params = merge_params(
        params,
        json!({
            "archiveRoot": display_path(&archive_root),
            "agents": agents.join(","),
            "targetScan": target_scan
        }),
    );
    let mut keyword_runs =
        run_keyword_archives_parallel(&store, &run_params, &keywords, profiles, &archive_root)?;
    keyword_runs.sort_by_key(|item| item.position);
    let mut archives = Vec::<Value>::new();
    let mut total_index_count = 0_u64;
    let mut total_selected_count = 0_u64;
    let mut failed_count = 0_u64;
    for keyword_run in keyword_runs {
        let run = keyword_run.run;
        let folder_name = run
            .get("collectionPathSegments")
            .and_then(Value::as_array)
            .and_then(|segments| segments.first())
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                run.get("profileId")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let validation = run.get("validation").cloned().unwrap_or_else(|| json!({}));
        let health = validation
            .get("healthStatus")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let index_count = run.get("indexCount").and_then(Value::as_u64).unwrap_or(0);
        let selected_count = run
            .get("selectedCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        total_index_count += index_count;
        total_selected_count += selected_count;
        if health == "failed" {
            failed_count += 1;
        }
        let archive_status = if health == "failed" {
            "failed"
        } else if index_count == 0 {
            "empty"
        } else {
            "archived"
        };
        archives.push(json!({
            "keyword": keyword_run.keyword,
            "folderName": folder_name,
            "status": archive_status,
            "profileId": run.get("profileId").cloned().unwrap_or_else(|| json!("")),
            "collectionPathSegments": run.get("collectionPathSegments").cloned().unwrap_or_else(|| json!([])),
            "folderPath": run.get("collectionPath")
                .and_then(Value::as_str)
                .and_then(|path| Path::new(path).parent().map(display_path))
                .map(Value::String)
                .unwrap_or_else(|| json!("")),
            "collectionPath": run.get("collectionPath").cloned().unwrap_or_else(|| json!("")),
            "documentCount": index_count,
            "selectedCount": selected_count,
            "documents": {
                "summary": run.get("summaryPath").cloned().unwrap_or_else(|| json!("")),
                "conversationIndex": run.get("conversationIndexPath").cloned().unwrap_or_else(|| json!("")),
                "collection": run.get("collectionPath").cloned().unwrap_or_else(|| json!(""))
            },
            "healthStatus": health,
            "validation": validation
        }));
    }
    let ok = failed_count == 0;
    let status = if !ok {
        if total_index_count == 0 {
            "failed"
        } else {
            "partial_failed"
        }
    } else if total_index_count == 0 {
        "empty"
    } else {
        "archived"
    };
    let first_archive = archives.first().cloned().unwrap_or_else(|| json!({}));
    let collection_path = if archives.len() == 1 {
        first_archive
            .get("collectionPath")
            .cloned()
            .unwrap_or_else(|| json!(""))
    } else {
        json!(display_path(&archive_root))
    };
    let archive_count = archives.len();
    let collection_path_segments = if archive_count == 1 {
        first_archive
            .get("collectionPathSegments")
            .cloned()
            .unwrap_or_else(|| json!([]))
    } else {
        Value::Array(
            archives
                .iter()
                .filter_map(|archive| archive.get("collectionPathSegments").cloned())
                .collect::<Vec<_>>(),
        )
    };
    let documents = if archive_count == 1 {
        first_archive
            .get("documents")
            .cloned()
            .unwrap_or_else(|| json!({}))
    } else {
        json!({
            "root": display_path(&archive_root)
        })
    };
    Ok(json!({
        "ok": ok,
        "status": status,
        "mode": "conversation-archive",
        "entry": "keyword-archive",
        "keywords": keywords,
        "keywordCount": archive_count,
        "collectionPathSegments": collection_path_segments,
        "archiveRoot": display_path(&archive_root),
        "collectionPath": collection_path,
        "documentCount": total_index_count,
        "selectedCount": total_selected_count,
        "archives": archives,
        "documents": documents,
        "message": if ok {
            format!(
                "Archived {} native conversations into {} keyword folders.",
                total_selected_count,
                archive_count
            )
        } else {
            "Conversation archive validation failed.".to_string()
        },
        "targetScan": archive_target_scan_summary(&target_scan, &agents)
    }))
}

pub(crate) fn collect(params: &Value) -> Result<Value> {
    let store = client_state_store(params)?;
    let topic = text_param(params, &["topic", "project", "intent"])
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("snapshots collect requires --topic"))?;
    let topic_key = topic_key(&topic)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("snapshot topic is empty after normalization"))?;
    let root = snapshot_root(&store, params)?;
    let mut raw = Map::<String, Value>::new();
    raw.insert("profileId".to_string(), json!(topic_key));
    raw.insert("displayName".to_string(), json!(topic));
    raw.insert("canonicalNames".to_string(), json!([topic.clone()]));
    raw.insert("aliasNames".to_string(), json!([]));
    raw.insert("projectPaths".to_string(), json!([]));
    raw.insert(
        "expectedAgents".to_string(),
        json!(collect_agent_ids(params)),
    );
    raw.insert("expectedSources".to_string(), json!([]));
    raw.insert("exclusionRules".to_string(), json!([]));
    let profile = parse_archive_profile(&Value::Object(raw))?;
    let mut result = run_archive_with_profile(&store, params, profile, root.path, "collect")?;
    if let Some(object) = result.as_object_mut() {
        object.insert("topic".to_string(), json!(topic));
        object.insert("topicKey".to_string(), json!(topic_key));
    }
    Ok(result)
}

pub(super) fn run_keyword_archives_parallel(
    store: &ClientStateStore,
    params: &Value,
    keywords: &[String],
    profiles: Vec<ArchiveProfile>,
    archive_root: &Path,
) -> Result<Vec<KeywordArchiveRun>> {
    if profiles.is_empty() {
        return Ok(Vec::new());
    }
    let worker_count = archive_parallelism(params).max(1).min(profiles.len());
    let mut items = keywords
        .iter()
        .cloned()
        .zip(profiles)
        .enumerate()
        .collect::<Vec<_>>();
    let chunk_size = items.len().div_ceil(worker_count);
    let mut runs = Vec::<KeywordArchiveRun>::with_capacity(items.len());

    thread::scope(|scope| -> Result<()> {
        let mut handles = Vec::new();
        for chunk in items.chunks_mut(chunk_size) {
            let local_items = chunk.to_vec();
            let local_store = store.clone();
            let local_params = params.clone();
            let local_archive_root = archive_root.to_path_buf();
            handles.push(scope.spawn(move || -> Result<Vec<KeywordArchiveRun>> {
                let mut local_runs = Vec::<KeywordArchiveRun>::with_capacity(local_items.len());
                for (position, (keyword, profile)) in local_items {
                    let run = run_archive_with_profile_layout(
                        &local_store,
                        &local_params,
                        profile,
                        local_archive_root.clone(),
                        "keyword-archive",
                        ArchiveCollectionLayout::DirectKeywordFolders,
                    )?;
                    local_runs.push(KeywordArchiveRun {
                        position,
                        keyword,
                        run,
                    });
                }
                Ok(local_runs)
            }));
        }
        for handle in handles {
            let mut local_runs = handle
                .join()
                .map_err(|_| anyhow!("keyword archive worker panicked"))??;
            runs.append(&mut local_runs);
        }
        Ok(())
    })?;

    Ok(runs)
}
