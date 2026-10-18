//! Read-only conversation queries, model discovery, filters, and pagination.

use super::*;
use crate::domain::conversation::parameters::text_param;

impl HistoryScanConfig {
    pub(crate) fn from_params(params: &Value) -> Self {
        Self {
            archive_mode: param_bool(params, "archiveMode").unwrap_or(false),
            session_ids: string_list_param(params, &["sessionIds", "sessionId"]),
            match_terms: string_list_param(params, &["matchTerms", "matchTerm"]),
            match_project_paths: string_list_param(
                params,
                &["matchProjectPaths", "matchProjectPath"],
            ),
            page: HistoryPageConfig::from_params(params),
        }
    }

    pub(super) fn matches_session(&self, session: &Value) -> bool {
        if !self.session_ids.is_empty() {
            let projected_id = session
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let native_id = session
                .get("nativeSessionId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !self
                .session_ids
                .iter()
                .any(|session_id| session_id == projected_id || session_id == native_id)
            {
                return false;
            }
        }
        if self.match_terms.is_empty() && self.match_project_paths.is_empty() {
            return true;
        }
        let text = history_match_text(session);
        let normalized = normalize_history_match_text(&text);
        let text_matches = self.match_terms.iter().any(|term| {
            let normalized_term = normalize_history_match_text(term);
            !normalized_term.is_empty()
                && normalized_contains_history_term(&normalized, &normalized_term)
        });
        if text_matches {
            return true;
        }
        if self.match_project_paths.is_empty() {
            return false;
        }
        let path_text = history_match_path_text(session);
        let normalized_path = normalize_history_match_text(&path_text);
        self.match_project_paths.iter().any(|term| {
            let normalized_term = normalize_history_match_text(term);
            path_text.contains(term)
                || (!normalized_term.is_empty()
                    && normalized_contains_history_term(&normalized_path, &normalized_term))
        })
    }

    pub(super) fn has_match_filters(&self) -> bool {
        !self.match_terms.is_empty() || !self.match_project_paths.is_empty()
    }

    pub(crate) fn has_single_session_filter(&self) -> bool {
        self.session_ids.len() == 1
    }

    /// The one requested session identity, when the caller asked for exactly one
    /// session. Parsers use it to skip every conversation the caller did not ask
    /// for instead of materializing a whole agent store.
    pub(crate) fn single_session_id(&self) -> Option<&str> {
        self.has_single_session_filter()
            .then(|| self.session_ids[0].as_str())
    }

    pub(crate) fn discovery_options(&self) -> HistoryDiscoveryOptions {
        HistoryDiscoveryOptions {
            archive_mode: self.archive_mode,
            exact_session_ids: self
                .has_single_session_filter()
                .then(|| self.session_ids.clone())
                .unwrap_or_default(),
        }
    }

    pub(super) fn matched_terms(&self, session: &Value) -> Vec<String> {
        let text = history_match_text(session);
        let path_text = history_match_path_text(session);
        self.matched_terms_in_text_and_path(&text, &path_text)
    }

    pub(super) fn matched_terms_in_text_and_path(
        &self,
        text: &str,
        path_text: &str,
    ) -> Vec<String> {
        let normalized = normalize_history_match_text(text);
        let normalized_path = normalize_history_match_text(path_text);
        let mut matched = Vec::<String>::new();
        for term in self
            .match_terms
            .iter()
            .chain(self.match_project_paths.iter())
        {
            let normalized_term = normalize_history_match_text(term);
            if normalized_term.is_empty() {
                continue;
            }
            let text_match = normalized_contains_history_term(&normalized, &normalized_term);
            let path_match = path_text.contains(term)
                || normalized_contains_history_term(&normalized_path, &normalized_term);
            if text_match || path_match {
                matched.push(term.clone());
            }
        }
        matched.sort();
        matched.dedup();
        matched
    }

    pub(super) fn compact_session_for_archive_discovery(&self, mut session: Value) -> Value {
        if !self.has_match_filters() || source_path_is_sqlite(&session) {
            return session;
        }
        let matched_terms = self.matched_terms(&session);
        let has_conversation = history_session_has_real_conversation(&session);
        if let Some(object) = session.as_object_mut() {
            object.insert(
                "archiveDiscoveryHasConversation".to_string(),
                json!(has_conversation),
            );
            object.insert(
                "archiveDiscoveryMatchedTerms".to_string(),
                json!(matched_terms),
            );
            if let Some(messages) = object.get("messages").and_then(Value::as_array) {
                let preview = messages
                    .iter()
                    .filter(|message| history_message_is_matchable(message))
                    .take(12)
                    .cloned()
                    .collect::<Vec<_>>();
                object.insert("messages".to_string(), Value::Array(preview));
                object.insert(
                    "messagesTruncatedForArchiveDiscovery".to_string(),
                    json!(true),
                );
            }
        }
        session
    }
}

impl HistoryPageConfig {
    pub(super) fn from_params(params: &Value) -> Self {
        Self {
            offset: number_param(params, "offset").unwrap_or(0) as usize,
            limit: number_param(params, "limit")
                .map(|value| (value as usize).clamp(1, MAX_HISTORY_PAGE_LIMIT)),
        }
    }

    pub(crate) fn end(&self) -> Option<usize> {
        self.limit.map(|limit| self.offset.saturating_add(limit))
    }

    pub(crate) fn has_more(&self, total: usize) -> bool {
        self.end().map(|end| total > end).unwrap_or(false)
    }
}

pub fn conversation_list(params: &Value) -> Result<Value> {
    if crate::platform::remote_acp_history::has_runtime_connection(params) {
        return crate::platform::remote_acp_history::conversation_list(params);
    }
    let agent_id = agent_param(params)?;
    let adapter = adapter_for_agent(&agent_id)
        .ok_or_else(|| anyhow!("unsupported native history adapter: {}", agent_id))?;
    let scan_config = HistoryScanConfig::from_params(params);
    validate_message_page_request(params, &scan_config)?;
    if browse_catalog_applies(params, &scan_config) {
        return super::catalog::conversation_list_from_catalog(
            adapter,
            &agent_id,
            params,
            &scan_config,
        );
    }
    let roots = history_roots(adapter, params);
    let mut sessions = Vec::<Value>::new();
    let mut discovery_options = scan_config.discovery_options();
    let codex_lineage = (adapter == HistoryAdapter::Codex)
        .then(|| super::catalog::codex_spawn_lineage(params))
        .unwrap_or_default();
    // Codex runs each delegated task as its own thread in its own rollout, so a
    // single-conversation read must pull those rollouts into scope or the
    // conversation shows none of the work it delegated.
    if adapter == HistoryAdapter::Codex && !discovery_options.exact_session_ids.is_empty() {
        let delegated = super::catalog::codex_delegated_thread_ids(
            &codex_lineage,
            &discovery_options.exact_session_ids,
        );
        discovery_options.exact_session_ids.extend(delegated);
    }
    if let Some(requested) = scan_config.single_session_id() {
        if matches!(adapter, HistoryAdapter::Cursor | HistoryAdapter::ClaudeCode) {
            discovery_options.exact_session_ids.extend(
                super::catalog::delegated_transcript_owner_ids(adapter, params, requested),
            );
        }
        if adapter == HistoryAdapter::KimiCode {
            discovery_options
                .exact_session_ids
                .push(super::kimi::kimi_code_conversation_id(requested).to_string());
        }
    }
    let discovery = discover_history_files(adapter, &roots, discovery_options);
    let mut skipped = discovery.skipped;
    let files_seen = discovery.files_seen;
    let directory_entries_seen = discovery.directory_entries_seen;
    let requested = scan_config.single_session_id();
    let mut mismatched_exact_hydration = false;
    for candidate in discovery.candidates {
        let metadata = match fs::metadata(&candidate.path) {
            Ok(metadata) => metadata,
            Err(error) => {
                skipped.push(json!({
                    "path": display_path(&candidate.path),
                    "reason": "metadata_failed",
                    "error": error.to_string()
                }));
                continue;
            }
        };
        let parsed = (adapter == HistoryAdapter::Codex && requested.is_some())
            .then(|| {
                super::codex::parse_codex_rollout_bounded_sessions(
                    &candidate.path,
                    &candidate.source_kind,
                    &metadata,
                    scan_config.clone(),
                    DEFAULT_HISTORY_MESSAGE_LIMIT,
                    requested,
                )
            })
            .flatten()
            .unwrap_or_else(|| {
                parse_history_file(
                    adapter,
                    &candidate.path,
                    &candidate.source_kind,
                    &metadata,
                    scan_config.clone(),
                )
            });
        if let Some(requested) = requested
            && exact_hydration_locator_matches(&candidate.path, requested)
            && parsed.iter().any(|session| {
                session.get("nativeSessionId").and_then(Value::as_str) != Some(requested)
            })
        {
            mismatched_exact_hydration = true;
        }
        sessions.extend(parsed);
    }
    if adapter == HistoryAdapter::Codex {
        let observation = CodexRuntimeObservation::capture();
        mark_codex_runtime_activity(&mut sessions, &observation);
    }
    if adapter == HistoryAdapter::Codex && !scan_config.has_single_session_filter() {
        apply_codex_session_index_titles(params, &mut sessions);
    }
    // Preserve database lineage alongside explicit rollout-header lineage.
    if adapter == HistoryAdapter::Codex {
        super::catalog::apply_codex_spawn_lineage(params, &codex_lineage, &mut sessions);
    }
    if matches!(adapter, HistoryAdapter::OpenCode | HistoryAdapter::KiloCode) {
        super::catalog::apply_openagent_parent_lineage(adapter, params, &mut sessions);
    }
    // Native identity owns dedupe for every adapter. Collapse before lineage so
    // delegated work attaches to the richest copy rather than a source-path copy
    // that a later pass would discard.
    sessions = collapse_sessions_by_native_identity(sessions);
    let observed_native_ids = sessions
        .iter()
        .filter_map(|session| session.get("nativeSessionId").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    let mut sessions = dedupe_history_sessions(finalize_history_sessions(sessions, &scan_config));
    sort_sessions_by_updated_at(&mut sessions);
    if let Some(requested) = scan_config.single_session_id() {
        ensure_exact_session_identity(
            requested,
            &observed_native_ids,
            &sessions,
            mismatched_exact_hydration,
        )?;
        apply_exact_message_page(&mut sessions[0], params)?;
        super::execution_provenance::annotate_selected_session(&agent_id, &mut sessions[0])?;
    }
    let total_sessions = sessions.len();
    let exact = scan_config.has_single_session_filter();
    let has_more = !exact && scan_config.page.has_more(total_sessions);
    let sessions = if exact {
        sessions
    } else {
        paged_history_sessions(sessions, &scan_config.page)
    };
    let returned_sessions = sessions.len();

    Ok(json!({
        "ok": true,
        "schemaVersion": CONVERSATION_SCHEMA_VERSION,
        "mode": "native-history",
        "scanMode": if scan_config.archive_mode { "archive" } else { "browse" },
        "importMode": "precise-adapter",
        "readOnly": true,
        "agentId": agent_id,
        "adapterId": adapter.id(),
        "adapterLabel": adapter.label(),
        "sessions": sessions,
        "page": {
            "offset": scan_config.page.offset,
            "limit": scan_config.page.limit,
            "returned": returned_sessions,
            "totalSessions": total_sessions,
            "hasMore": has_more
        },
        "sources": {
            "filesSeen": files_seen,
            "directoryEntriesSeen": directory_entries_seen,
            "skipped": skipped
        }
    }))
}

/// Whether a single-record hydration unit was selected by the requested
/// identity. The parsed record still owns native identity; this locator is used
/// only to distinguish a missing conversation from a selected unit that
/// returned a different identity.
fn exact_hydration_locator_matches(path: &Path, requested: &str) -> bool {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .any(|component| component == requested)
        || path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| stem == requested || stem.ends_with(&format!("-{requested}")))
}

fn validate_message_page_request(params: &Value, scan_config: &HistoryScanConfig) -> Result<()> {
    let has_message_page_parameter =
        params.get("messageBefore").is_some() || params.get("messageLimit").is_some();
    if has_message_page_parameter && !scan_config.has_single_session_filter() {
        return Err(anyhow!(
            "native_history_message_page_requires_exact_session"
        ));
    }
    if let Some(raw) = params.get("messageBefore")
        && raw
            .as_str()
            .map(str::trim)
            .is_none_or(|value| value.is_empty())
    {
        return Err(anyhow!("native_history_message_before_invalid"));
    }
    if let Some(raw) = params.get("messageLimit") {
        let Some(limit) = raw
            .as_u64()
            .or_else(|| raw.as_str().and_then(|value| value.parse::<u64>().ok()))
        else {
            return Err(anyhow!("native_history_message_limit_invalid"));
        };
        if !(1..=100).contains(&limit) {
            return Err(anyhow!("native_history_message_limit_invalid"));
        }
    }
    Ok(())
}

fn ensure_exact_session_identity(
    requested: &str,
    observed_native_ids: &BTreeSet<String>,
    sessions: &[Value],
    mismatched_exact_hydration: bool,
) -> Result<()> {
    if !observed_native_ids.contains(requested) {
        return if mismatched_exact_hydration {
            Err(anyhow!("native_history_session_identity_mismatch"))
        } else {
            Err(anyhow!("native_history_session_not_found"))
        };
    }
    if sessions.len() != 1 {
        return Err(anyhow!("native_history_session_not_found"));
    }
    let returned = sessions[0]
        .get("nativeSessionId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if returned != requested {
        return Err(anyhow!("native_history_session_identity_mismatch"));
    }
    Ok(())
}

fn apply_exact_message_page(session: &mut Value, params: &Value) -> Result<()> {
    let object = session
        .as_object_mut()
        .ok_or_else(|| anyhow!("native_history_session_invalid"))?;
    let messages = object
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("native_history_session_invalid"))?;
    let mut positions = std::collections::BTreeMap::<String, usize>::new();
    for (index, message) in messages.iter().enumerate() {
        let id = message
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| anyhow!("native_history_message_identity_missing"))?;
        if positions.insert(id.to_string(), index).is_some() {
            return Err(anyhow!("native_history_message_identity_duplicate"));
        }
    }
    let total = messages.len();
    let message_before = text_param(params, &["messageBefore"])
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let end = match message_before.as_deref() {
        Some(anchor) => positions
            .get(anchor)
            .copied()
            .ok_or_else(|| anyhow!("native_history_message_continuation_stale"))?,
        None => total,
    };
    let limit = number_param(params, "messageLimit")
        .map(|value| value as usize)
        .unwrap_or(DEFAULT_HISTORY_MESSAGE_LIMIT);
    let start = end.saturating_sub(limit);
    let page = messages[start..end].to_vec();
    let next_before = (start > 0)
        .then(|| page.first())
        .flatten()
        .and_then(|message| message.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    object.insert("messages".to_string(), Value::Array(page));
    retain_message_page_semantics(object);
    object.insert("messageCount".to_string(), json!(total));
    object.insert("sourceMessageCount".to_string(), json!(total));
    object.insert(
        "messagePage".to_string(),
        json!({
            "start": start,
            "endExclusive": end,
            "returned": end.saturating_sub(start),
            "total": total,
            "hasEarlier": start > 0,
            "nextBefore": next_before
        }),
    );
    Ok(())
}

/// Semantic thread/execution events are alternate projections of these same
/// message identities. A page must not carry a second complete transcript.
pub(super) fn retain_message_page_semantics(object: &mut serde_json::Map<String, Value>) {
    let ids = object
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|message| message.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<std::collections::HashSet<_>>();
    if let Some(semantic) = object.get_mut("semantic").and_then(Value::as_object_mut) {
        for key in ["thread", "execution"] {
            if let Some(events) = semantic.get_mut(key).and_then(Value::as_array_mut) {
                events.retain(|event| {
                    event
                        .get("id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| ids.contains(id))
                });
            }
        }
    }
}

/// Browse-mode lists (no search terms, no explicit session selection, no root
/// override, no archive discovery) load through the complete metadata catalog
/// instead of parsing every history file up front.
///
/// The Flutter client consumes `conversations stream`, not `list`. Stream must
/// use this same gate so catalog metadata such as `workingDirectory` reaches
/// the composer bind path.
pub(crate) fn browse_catalog_applies(params: &Value, scan_config: &HistoryScanConfig) -> bool {
    !scan_config.archive_mode
        && scan_config.session_ids.is_empty()
        && !scan_config.has_match_filters()
        && text_param(params, &["root", "historyRoot"]).is_none_or(|value| value.trim().is_empty())
}

pub fn model_catalog(params: &Value) -> Result<Value> {
    let agent_id = agent_param(params)?;
    let adapter = adapter_for_agent(&agent_id)
        .ok_or_else(|| anyhow!("unsupported native history adapter: {}", agent_id))?;
    let scan_config = HistoryScanConfig::from_params(params);
    let roots = history_roots(adapter, params);
    let discovery = discover_history_files(adapter, &roots, scan_config.discovery_options());
    let mut candidates = discovery.candidates;
    let skipped = discovery.skipped;
    let files_seen = discovery.files_seen;
    candidates.sort_by(|left, right| {
        right
            .modified_at
            .cmp(&left.modified_at)
            .then_with(|| left.path.cmp(&right.path))
    });
    let file_limit = number_param(params, "historyModelCatalogFileLimit")
        .unwrap_or(80)
        .clamp(1, 500) as usize;
    let mut names = BTreeSet::<String>::new();
    for candidate in candidates.into_iter().take(file_limit) {
        let Ok(metadata) = fs::metadata(&candidate.path) else {
            continue;
        };
        let sessions = parse_history_file(
            adapter,
            &candidate.path,
            &candidate.source_kind,
            &metadata,
            scan_config.clone(),
        );
        for session in sessions {
            collect_history_model_names(&session, &mut names, 0);
        }
    }
    let models = names
        .into_iter()
        .map(|name| {
            json!({
                "name": name,
                "source": "history",
                "sources": ["history"],
                "reasoningEfforts": []
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "ok": true,
        "status": if models.is_empty() { "empty" } else { "available" },
        "source": "history",
        "models": models,
        "sources": {
            "filesSeen": files_seen,
            "skippedCount": skipped.len()
        }
    }))
}

pub fn conversation_stream(params: &Value) -> Result<()> {
    crate::domain::conversation::streaming::conversation_stream(params)
}

pub fn conversation_append(_params: &Value) -> Result<Value> {
    Err(anyhow!(
        "native agent history is read-only; LicoUp does not create synthetic history entries"
    ))
}

pub fn conversation_delete(_params: &Value) -> Result<Value> {
    Err(anyhow!(
        "native agent history is read-only; LicoUp does not delete source-agent history"
    ))
}
