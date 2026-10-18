//! Supported-agent selection, target-scan projection, and local history discovery.

use super::*;

pub(super) fn collect_agent_ids(params: &Value) -> Vec<String> {
    if let Some(agent) = text_param(params, &["agent", "agentId", "target"]) {
        if !agent.trim().is_empty() {
            return vec![agent];
        }
    }
    if let Some(agents) = params.get("agents").and_then(Value::as_str) {
        let list = agents
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        if !list.is_empty() {
            return list;
        }
    }
    SUPPORTED_AGENTS
        .iter()
        .map(|value| value.to_string())
        .collect()
}

pub(super) fn explicit_agent_ids(params: &Value) -> Vec<String> {
    if text_param(params, &["agent", "agentId", "target"])
        .filter(|agent| !agent.trim().is_empty())
        .is_some()
        || params
            .get("agents")
            .and_then(Value::as_str)
            .filter(|agents| !agents.trim().is_empty())
            .is_some()
    {
        return collect_agent_ids(params)
            .into_iter()
            .map(|agent| normalize_agent_alias(&agent))
            .filter(|agent| SUPPORTED_AGENTS.contains(&agent.as_str()))
            .collect();
    }
    Vec::new()
}

pub(super) fn archive_target_scan(params: &Value) -> Result<Value> {
    if let Some(value) = params.get("targetScan").filter(|value| value.is_object()) {
        return Ok(value.clone());
    }
    if let Some(raw) = text_param(params, &["targetScanJson"]) {
        return Ok(serde_json::from_str(&raw)?);
    }
    if let Some(path) = text_param(params, &["targetScanFile"]) {
        let path = expand_home(&path);
        return read_json_or_default(&path, || json!({ "candidates": [] }));
    }
    targets::scan_targets_with_params(&crate::target_port::agent_target_port(), params)
}

pub(super) fn archive_agents_from_target_scan(params: &Value, target_scan: &Value) -> Vec<String> {
    let explicit = explicit_agent_ids(params);
    if !explicit.is_empty() {
        return unique_agents(explicit);
    }
    let mut agents = Vec::<String>::new();
    for candidate in target_scan
        .get("candidates")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let target = candidate
            .get("target")
            .and_then(Value::as_str)
            .map(normalize_agent_alias)
            .unwrap_or_default();
        if !SUPPORTED_AGENTS.contains(&target.as_str()) {
            continue;
        }
        let status = candidate
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let has_history_roots = candidate
            .get("historyRoots")
            .and_then(Value::as_array)
            .map(|items| !items.is_empty())
            .unwrap_or(false);
        if status == "configured" || status == "detected" || status == "manual" || has_history_roots
        {
            agents.push(target);
        }
    }
    unique_agents(agents)
}

pub(super) fn unique_agents(agents: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::<String>::new();
    let mut unique = Vec::<String>::new();
    for agent in agents {
        let normalized = normalize_agent_alias(&agent);
        if SUPPORTED_AGENTS.contains(&normalized.as_str()) && seen.insert(normalized.clone()) {
            unique.push(normalized);
        }
    }
    unique
}

pub(super) fn archive_target_scan_summary(
    target_scan: &Value,
    selected_agents: &[String],
) -> Value {
    let selected = selected_agents.iter().cloned().collect::<BTreeSet<_>>();
    let mut clients = Vec::<Value>::new();
    for candidate in target_scan
        .get("candidates")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let target = candidate
            .get("target")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let normalized = normalize_agent_alias(&target);
        let included = selected.contains(&normalized);
        clients.push(json!({
            "target": target,
            "label": candidate.get("label").cloned().unwrap_or_else(|| json!("")),
            "status": candidate.get("status").cloned().unwrap_or_else(|| json!("")),
            "included": included
        }));
    }
    json!({
        "source": target_scan.get("source").cloned().unwrap_or_else(|| json!("target-adapters")),
        "clientCount": clients.len(),
        "includedAgents": selected_agents,
        "clients": clients
    })
}

pub(super) fn discover_candidates(store: &ClientStateStore, params: &Value) -> DiscoveryResult {
    let agents = collect_agent_ids(params);
    let mut candidates = Vec::<Value>::new();
    let mut source_summaries = Vec::<Value>::new();
    let mut diagnostics = Vec::<Value>::new();
    let mut seen_candidates = BTreeSet::<String>::new();
    for agent in &agents {
        let mut history_params = Map::<String, Value>::new();
        history_params.insert("agent".to_string(), json!(agent));
        if let Some(home_dir) = text_param(params, &["homeDir"]) {
            history_params.insert("homeDir".to_string(), json!(home_dir));
        }
        if let Some(archive_mode) = params.get("archiveMode") {
            history_params.insert("archiveMode".to_string(), archive_mode.clone());
        }
        match conversations::conversation_list(&Value::Object(history_params.clone())) {
            Ok(history) => {
                let sessions = history
                    .get("sessions")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                extend_unique_candidates(&mut candidates, &mut seen_candidates, sessions);
                source_summaries.push(json!({
                    "agentId": agent,
                    "scope": "adapter-defaults",
                    "adapterId": history.get("adapterId").cloned().unwrap_or_else(|| json!(agent)),
                    "sessionCount": history.get("sessions").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
                    "filesSeen": history.pointer("/sources/filesSeen").cloned().unwrap_or_else(|| json!(0)),
                    "skipped": history.pointer("/sources/skipped").cloned().unwrap_or_else(|| json!([]))
                }));
            }
            Err(error) => diagnostics.push(json!({
                "stage": "discovery",
                "agentId": agent,
                "status": "failed",
                "message": error.to_string()
            })),
        }

        match manual_history_roots(store, params, agent) {
            Ok(roots) => {
                for history_root in roots {
                    let mut manual_params = history_params.clone();
                    manual_params.insert(
                        "historyRoot".to_string(),
                        json!(display_path(&history_root)),
                    );
                    manual_params.insert(
                        "historyRootKind".to_string(),
                        json!("manual-target-history-root"),
                    );
                    match conversations::conversation_list(&Value::Object(manual_params)) {
                        Ok(history) => {
                            let sessions = history
                                .get("sessions")
                                .and_then(Value::as_array)
                                .cloned()
                                .unwrap_or_default();
                            let before = candidates.len();
                            extend_unique_candidates(
                                &mut candidates,
                                &mut seen_candidates,
                                sessions,
                            );
                            source_summaries.push(json!({
                                "agentId": agent,
                                "scope": "manual-target-history-root",
                                "historyRoot": display_path(&history_root),
                                "adapterId": history.get("adapterId").cloned().unwrap_or_else(|| json!(agent)),
                                "sessionCount": candidates.len().saturating_sub(before),
                                "filesSeen": history.pointer("/sources/filesSeen").cloned().unwrap_or_else(|| json!(0)),
                                "skipped": history.pointer("/sources/skipped").cloned().unwrap_or_else(|| json!([]))
                            }));
                        }
                        Err(error) => diagnostics.push(json!({
                            "stage": "discovery",
                            "agentId": agent,
                            "scope": "manual-target-history-root",
                            "historyRoot": display_path(&history_root),
                            "status": "failed",
                            "message": error.to_string()
                        })),
                    }
                }
            }
            Err(error) => diagnostics.push(json!({
                "stage": "discovery",
                "agentId": agent,
                "scope": "manual-target-history-root",
                "status": "failed",
                "message": error.to_string()
            })),
        }
    }
    DiscoveryResult {
        agents,
        candidates,
        source_summaries,
        diagnostics,
    }
}

pub(super) fn discover_archive_candidates(
    store: &ClientStateStore,
    params: &Value,
    profile: &ArchiveProfile,
) -> DiscoveryResult {
    let agent_list = if profile.expected_agents.is_empty() {
        collect_agent_ids(params)
    } else {
        profile.expected_agents.clone()
    };
    let exact_keyword =
        profile.raw.get("selectionMode").and_then(Value::as_str) == Some(EXACT_KEYWORD_SELECTION);
    let match_terms = if exact_keyword {
        Vec::new()
    } else {
        profile
            .canonical_names
            .iter()
            .chain(profile.alias_names.iter())
            .cloned()
            .collect::<Vec<_>>()
    };
    let archive_params = merge_params(
        params,
        json!({
            "archiveMode": true,
            "agents": agent_list.join(","),
            "matchTerms": match_terms,
            "matchProjectPaths": profile.project_paths.clone()
        }),
    );
    discover_candidates(store, &archive_params)
}

pub(super) fn extend_unique_candidates(
    candidates: &mut Vec<Value>,
    seen: &mut BTreeSet<String>,
    sessions: Vec<Value>,
) {
    for session in sessions {
        let id = candidate_id(&session).unwrap_or_else(|| {
            hash_text(
                &serde_json::to_string(&session)
                    .unwrap_or_else(|_| "unknown-native-session".to_string()),
            )
        });
        if seen.insert(id) {
            candidates.push(session);
        }
    }
}

pub(super) fn manual_history_roots(
    store: &ClientStateStore,
    params: &Value,
    agent: &str,
) -> Result<Vec<PathBuf>> {
    if params.get("targetScan").is_some() {
        return Ok(target_scan_history_roots(params, agent));
    }
    let document = store.read_collection(TARGETS_COLLECTION)?;
    let items = document
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut roots = Vec::<PathBuf>::new();
    let mut seen = BTreeSet::<String>::new();
    let wanted = normalize_agent_alias(agent);
    for item in items {
        let item_target = item
            .get("target")
            .and_then(Value::as_str)
            .map(normalize_agent_alias)
            .unwrap_or_default();
        if item_target != wanted {
            continue;
        }
        for root in history_roots_from_value(item.get("historyRoots"))
            .into_iter()
            .chain(history_roots_from_value(item.get("historyRoot")))
        {
            let key = display_path(&root);
            if seen.insert(key) {
                roots.push(root);
            }
        }
    }
    Ok(roots)
}

pub(super) fn target_scan_history_roots(params: &Value, agent: &str) -> Vec<PathBuf> {
    let wanted = normalize_agent_alias(agent);
    let mut roots = Vec::<PathBuf>::new();
    let mut seen = BTreeSet::<String>::new();
    for candidate in params
        .get("targetScan")
        .and_then(|scan| scan.get("candidates"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let candidate_agent = candidate
            .get("target")
            .and_then(Value::as_str)
            .map(normalize_agent_alias)
            .unwrap_or_default();
        if candidate_agent != wanted {
            continue;
        }
        for root in history_roots_from_value(candidate.get("historyRoots"))
            .into_iter()
            .chain(history_roots_from_value(candidate.get("historyRoot")))
        {
            let key = display_path(&root);
            if seen.insert(key) {
                roots.push(root);
            }
        }
    }
    roots
}
