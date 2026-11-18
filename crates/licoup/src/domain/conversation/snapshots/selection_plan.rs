//! Preview and execution for the two user-facing backup selections.

use super::*;

pub(super) const ALL_SELECTION: &str = "all";
pub(super) const EXACT_KEYWORD_SELECTION: &str = "exact-keyword";

pub(super) fn archive_selection_mode(params: &Value) -> Result<&'static str> {
    match text_param(params, &["selectionMode", "selection"])
        .unwrap_or_else(|| EXACT_KEYWORD_SELECTION.to_string())
        .to_ascii_lowercase()
        .as_str()
    {
        ALL_SELECTION => Ok(ALL_SELECTION),
        EXACT_KEYWORD_SELECTION => Ok(EXACT_KEYWORD_SELECTION),
        _ => Err(anyhow!(
            "conversation archive selectionMode must be all or exact-keyword"
        )),
    }
}

pub(super) fn archive_selection_query(params: &Value, mode: &str) -> Result<String> {
    if mode == ALL_SELECTION {
        return Ok(String::new());
    }
    text_param(params, &["query"])
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("exact-keyword conversation archive requires --query"))
}

fn selection_profile(
    mode: &str,
    query: &str,
    archive_root: &Path,
    agents: &[String],
) -> Result<ArchiveProfile> {
    if mode == ALL_SELECTION {
        return parse_archive_profile(&json!({
            "profileId": "all-conversations",
            "displayName": "All conversations",
            "collectionPathSegments": ["all-conversations"],
            "archiveRoot": display_path(archive_root),
            "canonicalNames": [],
            "aliasNames": [],
            "projectPaths": [],
            "expectedAgents": agents,
            "expectedSources": [],
            "exclusionRules": [],
            "selectionMode": ALL_SELECTION
        }));
    }
    let mut profile = derived_archive_profile(&[query.to_string()], archive_root, agents)?;
    profile.alias_names.clear();
    if let Some(raw) = profile.raw.as_object_mut() {
        raw.insert("aliasNames".to_string(), json!([]));
        raw.insert("selectionMode".to_string(), json!(EXACT_KEYWORD_SELECTION));
    }
    Ok(profile)
}

fn prepared_selection(
    params: &Value,
) -> Result<(
    ClientStateStore,
    PathBuf,
    Value,
    Vec<String>,
    String,
    String,
    ArchiveProfile,
    DiscoveryResult,
)> {
    let store = client_state_store(params)?;
    let archive_root = archive_destination(params)?;
    let target_scan = archive_target_scan(params)?;
    let mut agents = archive_agents_from_target_scan(params, &target_scan);
    agents.sort();
    let mode = archive_selection_mode(params)?.to_string();
    let query = archive_selection_query(params, &mode)?;
    let profile = selection_profile(&mode, &query, &archive_root, &agents)?;
    let run_params = merge_params(
        params,
        json!({
            "archiveRoot": display_path(&archive_root),
            "agents": agents.join(","),
            "targetScan": target_scan
        }),
    );
    let discovery = discover_archive_candidates(&store, &run_params, &profile);
    Ok((
        store,
        archive_root,
        target_scan,
        agents,
        mode,
        query,
        profile,
        discovery,
    ))
}

pub(crate) fn archive_selection_preview(params: &Value) -> Result<Value> {
    let (_, archive_root, target_scan, agents, mode, query, profile, discovery) =
        prepared_selection(params)?;
    let (selected, _) = select_profile_archive_candidates(&profile, &discovery);
    let collection_dir = collection_dir_for_profile_layout(
        &archive_root,
        &profile,
        ArchiveCollectionLayout::DirectKeywordFolders,
    );
    Ok(json!({
        "ok": true,
        "mode": "conversation-archive-preview",
        "selectionMode": mode,
        "source": {
            "kind": "local-native-history",
            "agents": agents
        },
        "query": query,
        "destination": display_path(&archive_root),
        "count": selected.len(),
        "conflict": collection_dir.join(COLLECTION_JSON).exists(),
        "conflictPolicy": "merge-local-archive",
        "collectionKey": profile.profile_id,
        "targetScan": archive_target_scan_summary(&target_scan, &profile.expected_agents)
    }))
}

pub(crate) fn archive_selection_collect(params: &Value) -> Result<Value> {
    let (store, archive_root, target_scan, agents, mode, query, profile, discovery) =
        prepared_selection(params)?;
    if agents.is_empty() {
        return Ok(json!({
            "ok": false,
            "status": "no_supported_clients_detected",
            "mode": "conversation-archive",
            "entry": "selection-archive",
            "selectionMode": mode,
            "query": query,
            "selectedCount": 0,
            "targetScan": archive_target_scan_summary(&target_scan, &[])
        }));
    }
    let mut result = run_archive_with_profile_discovery(
        &store,
        params,
        profile,
        archive_root,
        "selection-archive",
        ArchiveCollectionLayout::DirectKeywordFolders,
        &discovery,
    )?;
    if let Some(object) = result.as_object_mut() {
        object.insert("selectionMode".to_string(), json!(mode));
        object.insert("query".to_string(), json!(query));
        object.insert(
            "targetScan".to_string(),
            archive_target_scan_summary(&target_scan, &agents),
        );
    }
    Ok(result)
}
