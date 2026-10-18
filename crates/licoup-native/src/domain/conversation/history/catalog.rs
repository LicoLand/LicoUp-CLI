//! Tiered, metadata-only session catalogs for browse-mode conversation lists.
//!
//! Browse lists never parse conversation content up front. Each adapter contributes
//! native catalog metadata (agent-owned sqlite state, per-session state files, or
//! plain file metadata) without an age or terminal-count window. Only sessions
//! that land in the
//! returned page are hydrated from their content files. Explicit searches
//! (match terms), archive discovery, single-session readback, and root overrides
//! keep the legacy full-scan path in `query.rs`.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::Connection;
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::codex::{
    CodexRuntimeObservation, parse_codex_rollout_bounded_sessions, rollout_session_id_from_filename,
};
use super::cursor_openagent::codec::{open_read_only_connection, sqlite_table_exists};
use super::cursor_openagent::{
    CopilotChatSessionsReadError, copilot_chat_sessions_document, cursor_composer_catalog,
};
use super::delegated_transcripts::{
    CURSOR_TRANSCRIPTS_DIRECTORY, delegated_file_is_transcript, delegated_task_label,
    delegated_task_prompt_text, delegated_transcript_lineage, transcript_conversation_id,
    transcript_is_delegated,
};
use super::project_workspace::bounded_project_workspace;
use super::projection_cache::{HistoryProjectionCache, ProjectionCacheKey, SourceFingerprint};
use super::query_filter::{epoch_number_to_rfc3339, session_id, system_time, title_from_text};
use super::session_metadata::{
    extract_conversation_title, meaningful_explicit_title, session_from_messages_with_title,
};

/// Native sessions the user archived through a Lico conversation are
/// visibility-filtered here; the record lives in the Canonical store and the
/// agent's own files are never touched. Read-only and fail-open by design: a
/// missing or unreadable mark table filters nothing. Unit tests without a
/// portable data directory override never open the developer's store.
fn archived_native_sessions(agent_id: &str) -> std::collections::BTreeSet<String> {
    #[cfg(test)]
    if licoup_foundation::platform::paths::portable_data_dir_override_path().is_none() {
        return Default::default();
    }
    let Ok(root) = licoup_foundation::platform::paths::portable_data_dir_read_only() else {
        return Default::default();
    };
    licoup_conversation::store::ConversationStore::archived_native_session_ids(&root, agent_id)
        .unwrap_or_default()
}
use super::{CONVERSATION_SCHEMA_VERSION, HistoryScanConfig, finalize_history_sessions};
use crate::domain::conversation::adapter_dispatch::parse_history_file;
use crate::domain::conversation::history_discovery::{
    HistoryDiscoveryOptions, HistoryFileCandidate, discover_history_files,
};
use crate::domain::conversation::source_catalog::{HistoryAdapter, HistoryRoot, history_roots};

const CURSOR_IDE_STORE_FILE: &str = "state.vscdb";
const CURSOR_IDE_STORE_WALK_DEPTH: usize = 2;
const MAX_TITLE_PROBE_LINES: usize = 200;
const MAX_TITLE_PROBE_LINE_BYTES: usize = 1024 * 1024;
const MAX_TITLE_PROBE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct CatalogSession {
    pub(crate) native_session_id: String,
    pub(crate) source_path: PathBuf,
    pub(crate) source_kind: String,
    pub(crate) title: Option<String>,
    pub(crate) created_at: Option<SystemTime>,
    pub(crate) updated_at: Option<SystemTime>,
    pub(crate) working_directory: Option<String>,
    pub(crate) message_count: Option<usize>,
    pub(crate) model: Option<String>,
    pub(crate) running: bool,
    pub(crate) hydrate: CatalogHydration,
}

#[derive(Clone, Debug)]
pub(crate) enum CatalogHydration {
    /// Parse this content file when the session lands in the returned page.
    File(PathBuf),
    /// Parse every `agents/*/wire.jsonl` below this session directory.
    KimiWireDirectory(PathBuf),
    /// Parse one conversation transcript together with the delegated task
    /// transcripts recorded for it, so delegated work folds into the
    /// conversation instead of surfacing as separate conversations.
    TranscriptWithDelegatedTasks {
        transcript: PathBuf,
        delegated: Vec<PathBuf>,
    },
    /// Metadata-only entry; the page keeps a stub session.
    None,
}

#[derive(Default)]
pub(crate) struct SessionCatalog {
    pub(crate) sessions: Vec<CatalogSession>,
    pub(crate) files_seen: usize,
    pub(crate) directory_entries_seen: usize,
    pub(crate) skipped: Vec<Value>,
}

impl CatalogSession {
    fn recency(&self) -> SystemTime {
        self.updated_at.or(self.created_at).unwrap_or(UNIX_EPOCH)
    }
}

/// Browse-mode list entry point: complete catalog metadata,
/// sorted by last activity, paginated, with content hydration for the page only.
pub(crate) fn conversation_list_from_catalog(
    adapter: HistoryAdapter,
    agent_id: &str,
    params: &Value,
    scan_config: &HistoryScanConfig,
) -> Result<Value> {
    Ok(conversation_list_from_catalog_inner(adapter, agent_id, params, scan_config).0)
}

/// Browse-mode list with bounded-work diagnostics. Tests assert sharp bounds
/// on cache entries/bytes and tail reads through the returned counters; the
/// public DTO is identical to [`conversation_list_from_catalog`].
pub(crate) fn conversation_list_from_catalog_inner(
    adapter: HistoryAdapter,
    agent_id: &str,
    params: &Value,
    scan_config: &HistoryScanConfig,
) -> (Value, BrowseWorkCounters) {
    let mut catalog = load_session_catalog(adapter, params, SystemTime::now());
    let archived = archived_native_sessions(agent_id);
    if !archived.is_empty() {
        // Lico-archived sessions stay on disk under the agent's own history;
        // they only stop surfacing in browse listings.
        catalog
            .sessions
            .retain(|session| !archived.contains(session.native_session_id.as_str()));
    }
    let total_sessions = catalog.sessions.len();
    let offset = scan_config.page.offset;
    let end = scan_config
        .page
        .end()
        .map(|end| end.min(total_sessions))
        .unwrap_or(total_sessions);
    let mut page_entries: Vec<CatalogSession> = if offset >= total_sessions {
        Vec::new()
    } else {
        catalog.sessions[offset..end].to_vec()
    };
    if adapter == HistoryAdapter::Codex {
        let observation = CodexRuntimeObservation::capture();
        for entry in &mut page_entries {
            entry.running = catalog_entry_runtime_paths(entry)
                .into_iter()
                .any(|path| observation.is_running(path));
        }
    }
    let mut cache = HistoryProjectionCache::open(params);
    let mut counters = BrowseWorkCounters::default();
    let mut hydration_skipped = Vec::new();
    let sessions = hydrate_catalog_page(
        adapter,
        &page_entries,
        &mut hydration_skipped,
        &mut cache,
        &mut counters,
        scan_config,
        params,
    );
    counters.cache_entries = cache.entry_count();
    counters.cache_bytes = cache.byte_count();
    counters.cache_discards = counters.cache_discards.saturating_add(cache.discard_count);
    cache.save();
    let mut skipped = catalog.skipped;
    skipped.append(&mut hydration_skipped);
    let returned_sessions = sessions.len();
    let has_more = scan_config.page.has_more(total_sessions);

    (
        json!({
            "ok": true,
            "schemaVersion": CONVERSATION_SCHEMA_VERSION,
            "mode": "native-history",
            "scanMode": "browse",
            "importMode": "precise-adapter",
            "readOnly": true,
            "agentId": agent_id,
            "adapterId": adapter.id(),
            "adapterLabel": adapter.label(),
            "sessions": sessions,
            "page": {
                "offset": offset,
                "limit": scan_config.page.limit,
                "returned": returned_sessions,
                "totalSessions": total_sessions,
                "hasMore": has_more
            },
            "sources": {
                "filesSeen": catalog.files_seen,
                "directoryEntriesSeen": catalog.directory_entries_seen,
                "skipped": skipped
            }
        }),
        counters,
    )
}

pub(crate) fn load_session_catalog(
    adapter: HistoryAdapter,
    params: &Value,
    _now: SystemTime,
) -> SessionCatalog {
    let cutoff = UNIX_EPOCH;
    let roots = history_roots(adapter, params);
    let mut catalog = SessionCatalog::default();
    match adapter {
        HistoryAdapter::Codex => codex_catalog(&roots, cutoff, &mut catalog),
        HistoryAdapter::OpenCode | HistoryAdapter::KiloCode => {
            openagent_catalog(adapter, &roots, cutoff, &mut catalog);
        }
        HistoryAdapter::Copilot => copilot_catalog(&roots, cutoff, &mut catalog),
        HistoryAdapter::KimiCode => kimi_code_catalog(&roots, cutoff, &mut catalog),
        HistoryAdapter::Cursor => cursor_catalog(&roots, cutoff, &mut catalog),
        HistoryAdapter::ClaudeCode => claude_catalog(&roots, cutoff, &mut catalog),
        HistoryAdapter::Pi | HistoryAdapter::LicoAgent => {
            pi_catalog(&roots, cutoff, &mut catalog);
        }
        HistoryAdapter::Antigravity => antigravity_catalog(&roots, cutoff, &mut catalog),
        _ => generic_file_catalog(adapter, &roots, cutoff, &mut catalog),
    }
    sort_and_dedupe_catalog(adapter, &mut catalog.sessions);
    catalog
}

/// Resolve a task's recorded layout owner before directory pruning. This reads
/// catalog metadata only and keeps the eventual exact parse in one conversation.
pub(crate) fn delegated_transcript_owner_ids(
    adapter: HistoryAdapter,
    params: &Value,
    requested: &str,
) -> Vec<String> {
    let catalog = load_session_catalog(adapter, params, SystemTime::now());
    catalog
        .sessions
        .into_iter()
        .flat_map(|entry| {
            let paths = match entry.hydrate {
                CatalogHydration::TranscriptWithDelegatedTasks { delegated, .. } => delegated,
                CatalogHydration::File(path) => vec![path],
                _ => Vec::new(),
            };
            paths.into_iter().filter_map(|path| {
                let (child, parent) = delegated_transcript_lineage(&path)?;
                (child == requested).then_some(parent)
            })
        })
        .collect()
}

fn sort_and_dedupe_catalog(adapter: HistoryAdapter, sessions: &mut Vec<CatalogSession>) {
    sessions.sort_by(|left, right| {
        catalog_content_rank(right)
            .cmp(&catalog_content_rank(left))
            .then_with(|| right.recency().cmp(&left.recency()))
            .then_with(|| left.source_path.cmp(&right.source_path))
            .then_with(|| left.native_session_id.cmp(&right.native_session_id))
    });
    let mut kept = BTreeMap::<String, usize>::new();
    let mut merged = Vec::<CatalogSession>::with_capacity(sessions.len());
    for entry in sessions.drain(..) {
        let key = catalog_dedupe_key(adapter, &entry);
        match kept.get(&key).copied() {
            Some(index) => absorb_duplicate_catalog_entry(&mut merged[index], entry),
            None => {
                kept.insert(key, merged.len());
                merged.push(entry);
            }
        }
    }
    merged.sort_by(|left, right| {
        right
            .recency()
            .cmp(&left.recency())
            .then_with(|| left.source_path.cmp(&right.source_path))
            .then_with(|| left.native_session_id.cmp(&right.native_session_id))
    });
    *sessions = merged;
}

/// A metadata-only entry never wins over an entry that can hydrate content, so
/// the same conversation recorded in several agent stores keeps the richest
/// source and only borrows metadata from the others.
///
/// For Cursor the CLI project transcript is the richest of the three stores: it
/// keeps the tool trace and the delegated-task transcripts, while the IDE store
/// keeps only conversation bubbles and the chat store keeps no content the
/// catalog can hydrate.
fn catalog_content_rank(entry: &CatalogSession) -> u8 {
    if matches!(entry.hydrate, CatalogHydration::None) {
        return 0;
    }
    match entry.source_kind.as_str() {
        "cursor-cli-projects" => 2,
        "vscode-copilot-workspace-storage" | "vscode-copilot-global-storage"
            if entry
                .source_path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| matches!(extension, "jsonl" | "ndjson")) =>
        {
            2
        }
        _ => 1,
    }
}

/// Carry metadata a duplicate entry knows and the kept entry does not.
///
/// Cursor records one conversation in the IDE store, the CLI chat store, and the
/// CLI project tree; only some of them know the project directory, the title, or
/// the model. Dropping the duplicate outright loses that metadata, which is how
/// a conversation ends up with no working directory at all.
fn absorb_duplicate_catalog_entry(kept: &mut CatalogSession, duplicate: CatalogSession) {
    if kept.working_directory.is_none() {
        kept.working_directory = duplicate.working_directory;
    }
    if kept.title.is_none() {
        kept.title = duplicate.title;
    }
    if kept.model.is_none() {
        kept.model = duplicate.model;
    }
    if kept.created_at.is_none() {
        kept.created_at = duplicate.created_at;
    }
    kept.updated_at = match (kept.updated_at, duplicate.updated_at) {
        (Some(kept_at), Some(other)) => Some(kept_at.max(other)),
        (kept_at, other) => kept_at.or(other),
    };
    if kept.message_count.is_none() {
        kept.message_count = duplicate.message_count;
    }
    kept.running |= duplicate.running;
}

fn catalog_dedupe_key(adapter: HistoryAdapter, entry: &CatalogSession) -> String {
    // Native identity is the conversation key; source paths are hydration
    // locations. Only records without identity fall back to a source locator.
    if !entry.native_session_id.is_empty() {
        return format!("{}\n{}", adapter.id(), entry.native_session_id);
    }
    format!(
        "{}\n{}\n{}",
        adapter.id(),
        entry.source_path.display(),
        entry.native_session_id
    )
}

// ---------------------------------------------------------------------------
// Page hydration: parse content files for the returned page only.
// ---------------------------------------------------------------------------

/// Browse pages return a newest-message preview with exact continuation facts.
/// Message text is never rewritten; selecting a row enters exact paging.
const CATALOG_HYDRATED_MESSAGE_CAP: usize = super::DEFAULT_HISTORY_MESSAGE_LIMIT;

fn hydrate_catalog_page(
    adapter: HistoryAdapter,
    page: &[CatalogSession],
    skipped: &mut Vec<Value>,
    cache: &mut HistoryProjectionCache,
    counters: &mut BrowseWorkCounters,
    scan_config: &HistoryScanConfig,
    params: &Value,
) -> Vec<Value> {
    let mut units = BTreeMap::<PathBuf, (String, CatalogHydration, Vec<usize>)>::new();
    for (index, entry) in page.iter().enumerate() {
        match &entry.hydrate {
            CatalogHydration::File(path) => {
                let unit = units.entry(path.clone()).or_insert_with(|| {
                    (
                        entry.source_kind.clone(),
                        CatalogHydration::File(path.clone()),
                        Vec::new(),
                    )
                });
                unit.2.push(index);
            }
            CatalogHydration::KimiWireDirectory(directory) => {
                let unit = units.entry(directory.clone()).or_insert_with(|| {
                    (
                        entry.source_kind.clone(),
                        CatalogHydration::KimiWireDirectory(directory.clone()),
                        Vec::new(),
                    )
                });
                unit.2.push(index);
            }
            CatalogHydration::TranscriptWithDelegatedTasks {
                transcript,
                delegated,
            } => {
                let unit = units.entry(transcript.clone()).or_insert_with(|| {
                    (
                        entry.source_kind.clone(),
                        CatalogHydration::TranscriptWithDelegatedTasks {
                            transcript: transcript.clone(),
                            delegated: delegated.clone(),
                        },
                        Vec::new(),
                    )
                });
                unit.2.push(index);
            }
            CatalogHydration::None => {}
        }
    }

    let mut resolved = BTreeMap::<usize, Value>::new();
    let mut units_with_content = HashSet::<&PathBuf>::new();
    for (unit_path, (source_kind, hydration, indexes)) in &units {
        // A hydration unit is shared by every page entry that names it.
        let key = projection_cache_key(adapter, source_kind, hydration, params);
        let sessions = match key.as_ref().and_then(|key| cache.get(key)) {
            Some(cached) => {
                counters.cache_hits += 1;
                cached
            }
            None => {
                if key.is_some() {
                    counters.cache_misses += 1;
                }
                let parsed = parse_catalog_unit(
                    adapter,
                    unit_path,
                    source_kind,
                    hydration,
                    params,
                    scan_config,
                );
                let parsed = catalog_cache_projection(parsed);
                if let Some(key) = key.as_ref().filter(|_| !parsed.is_empty()) {
                    cache.insert(key.clone(), parsed.clone());
                }
                parsed
            }
        };
        if !sessions.is_empty() {
            units_with_content.insert(unit_path);
        }
        for index in indexes {
            let wanted = page[*index].native_session_id.as_str();
            let Some(session) = sessions.iter().find(|session| {
                session
                    .get("nativeSessionId")
                    .and_then(Value::as_str)
                    .is_some_and(|native_id| native_id == wanted)
            }) else {
                continue;
            };
            let mut session = session.clone();
            if let Some(object) = session.as_object_mut() {
                // Native catalog metadata wins over content-derived projection.
                if let Some(title) = page[*index]
                    .title
                    .as_deref()
                    .filter(|title| meaningful_explicit_title(title))
                {
                    object.insert("title".to_string(), json!(title_from_text(title)));
                }
                if let Some(working_directory) = page[*index]
                    .working_directory
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                {
                    object.insert("workingDirectory".to_string(), json!(working_directory));
                }
                if let Some(model) = page[*index]
                    .model
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                {
                    object.insert("model".to_string(), json!(model));
                }
                if adapter == HistoryAdapter::Copilot {
                    if let Some(created_at) = page[*index].created_at {
                        object.insert("createdAt".to_string(), json!(system_time(created_at)));
                    }
                    if let Some(updated_at) = page[*index].updated_at {
                        object.insert("updatedAt".to_string(), json!(system_time(updated_at)));
                    }
                }
                if page[*index].running {
                    object.insert("running".to_string(), json!(true));
                } else {
                    object.remove("running");
                }
                let recorded_message_count = object
                    .get("messageCount")
                    .and_then(Value::as_u64)
                    .map(|value| value as usize);
                if let Some(messages) = object.get_mut("messages").and_then(Value::as_array_mut) {
                    let total = recorded_message_count
                        .unwrap_or(messages.len())
                        .max(messages.len());
                    trim_browse_messages(messages);
                    let returned = messages.len();
                    let start = total.saturating_sub(returned);
                    let next_before = (start > 0)
                        .then(|| messages.first())
                        .flatten()
                        .and_then(|message| message.get("id"))
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    object.insert("messageCount".to_string(), json!(total));
                    object.insert("sourceMessageCount".to_string(), json!(total));
                    object.insert(
                        "messagePage".to_string(),
                        json!({
                            "start": start,
                            "endExclusive": total,
                            "returned": returned,
                            "total": total,
                            "hasEarlier": start > 0,
                            "nextBefore": next_before
                        }),
                    );
                }
                super::query::retain_message_page_semantics(object);
            }
            resolved.insert(*index, session);
        }
    }

    page.iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            if let Some(session) = resolved.remove(&index) {
                return Some(session);
            }
            // A source that parsed into conversations, none of which is this
            // entry, means the entry identity does not name a conversation in it
            // — an index key, a config record, or workflow bookkeeping. A stub
            // there would put an empty row in the browse list.
            if catalog_entry_content_source(entry)
                .is_some_and(|unit| units_with_content.contains(&unit))
            {
                skipped.push(json!({
                    "path": entry.source_path.to_string_lossy(),
                    "reason": "catalog_entry_is_not_a_conversation"
                }));
                return None;
            }
            catalog_session_stub(adapter, entry).or_else(|| {
                skipped.push(json!({
                    "path": entry.source_path.to_string_lossy(),
                    "reason": "catalog_metadata_unavailable"
                }));
                None
            })
        })
        .collect()
}

fn catalog_cache_projection(mut sessions: Vec<Value>) -> Vec<Value> {
    for session in &mut sessions {
        let Some(object) = session.as_object_mut() else {
            continue;
        };
        let total = object
            .get("messageCount")
            .and_then(Value::as_u64)
            .map(|value| value as usize)
            .or_else(|| {
                object
                    .get("messages")
                    .and_then(Value::as_array)
                    .map(Vec::len)
            })
            .unwrap_or(0);
        let Some(messages) = object.get_mut("messages").and_then(Value::as_array_mut) else {
            continue;
        };
        trim_browse_messages(messages);
        let returned = messages.len();
        let start = total.saturating_sub(returned);
        let next_before = (start > 0)
            .then(|| messages.first())
            .flatten()
            .and_then(|message| message.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        object.insert("messageCount".to_string(), json!(total));
        object.insert("sourceMessageCount".to_string(), json!(total));
        object.insert(
            "messagePage".to_string(),
            json!({
                "start": start,
                "endExclusive": total,
                "returned": returned,
                "total": total,
                "hasEarlier": start > 0,
                "nextBefore": next_before
            }),
        );
        super::query::retain_message_page_semantics(object);
    }
    sessions
}

fn trim_browse_messages(messages: &mut Vec<Value>) {
    let overflow = messages.len().saturating_sub(CATALOG_HYDRATED_MESSAGE_CAP);
    if overflow > 0 {
        messages.drain(0..overflow);
    }
}

/// Hydration unit key of one catalog entry, when it has content to parse.
fn catalog_entry_content_source(entry: &CatalogSession) -> Option<&PathBuf> {
    match &entry.hydrate {
        CatalogHydration::File(path) => Some(path),
        CatalogHydration::KimiWireDirectory(directory) => Some(directory),
        CatalogHydration::TranscriptWithDelegatedTasks { transcript, .. } => Some(transcript),
        CatalogHydration::None => None,
    }
}

fn catalog_entry_runtime_paths(entry: &CatalogSession) -> Vec<&Path> {
    match &entry.hydrate {
        CatalogHydration::File(path) => vec![path.as_path()],
        CatalogHydration::TranscriptWithDelegatedTasks {
            transcript,
            delegated,
            ..
        } => std::iter::once(transcript.as_path())
            .chain(delegated.iter().map(PathBuf::as_path))
            .collect(),
        CatalogHydration::KimiWireDirectory(_) | CatalogHydration::None => Vec::new(),
    }
}

fn parse_catalog_unit(
    adapter: HistoryAdapter,
    unit_path: &Path,
    source_kind: &str,
    hydration: &CatalogHydration,
    params: &Value,
    scan_config: &HistoryScanConfig,
) -> Vec<Value> {
    let mut sessions = Vec::<Value>::new();
    match hydration {
        CatalogHydration::File(path) => {
            if let Ok(metadata) = fs::metadata(path) {
                sessions.extend(parse_catalog_history_file(
                    adapter,
                    path,
                    source_kind,
                    &metadata,
                    scan_config,
                ));
            }
        }
        CatalogHydration::KimiWireDirectory(directory) => {
            for wire in kimi_wire_files(directory) {
                if let Ok(metadata) = fs::metadata(&wire) {
                    sessions.extend(parse_history_file(
                        adapter,
                        &wire,
                        source_kind,
                        &metadata,
                        scan_config.clone(),
                    ));
                }
            }
        }
        CatalogHydration::TranscriptWithDelegatedTasks {
            transcript,
            delegated,
        } => {
            if let Ok(metadata) = fs::metadata(transcript) {
                sessions.extend(parse_history_file(
                    adapter,
                    transcript,
                    source_kind,
                    &metadata,
                    scan_config.clone(),
                ));
            }
            for child in delegated {
                let Ok(metadata) = fs::metadata(child) else {
                    continue;
                };
                let child_sessions =
                    parse_catalog_history_file(adapter, child, source_kind, &metadata, scan_config);
                sessions.extend(child_sessions);
            }
        }
        CatalogHydration::None => {
            let _ = unit_path;
        }
    }
    if sessions.is_empty() {
        return sessions;
    }
    if adapter == HistoryAdapter::Codex {
        let parents = codex_database_spawn_lineage(params);
        apply_codex_spawn_lineage(params, &parents, &mut sessions);
    }
    if matches!(adapter, HistoryAdapter::OpenCode | HistoryAdapter::KiloCode) {
        apply_openagent_parent_lineage(adapter, params, &mut sessions);
    }
    finalize_history_sessions(sessions, scan_config)
}

fn parse_catalog_history_file(
    adapter: HistoryAdapter,
    path: &Path,
    source_kind: &str,
    metadata: &fs::Metadata,
    scan_config: &HistoryScanConfig,
) -> Vec<Value> {
    if adapter == HistoryAdapter::Codex {
        return parse_codex_rollout_bounded_sessions(
            path,
            source_kind,
            metadata,
            scan_config.clone(),
            CATALOG_HYDRATED_MESSAGE_CAP,
            None,
        )
        .unwrap_or_default();
    }
    parse_history_file(adapter, path, source_kind, metadata, scan_config.clone())
}

/// Task label from the instruction the conversation handed the delegated agent.
fn declared_delegated_title(session: &Value) -> Option<String> {
    delegated_task_prompt_text(session)
        .and_then(delegated_task_label)
        .or_else(|| {
            session
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn kimi_wire_files(session_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(session_dir.join("agents")) else {
        return Vec::new();
    };
    let mut wires = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path().join("wire.jsonl"))
        .filter(|path| {
            fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
        })
        .collect::<Vec<_>>();
    wires.sort();
    wires
}

/// Build the full session JSON shape from catalog metadata alone, with an empty
/// message list; content stays deferred until the session is expanded or searched.
fn catalog_session_stub(adapter: HistoryAdapter, entry: &CatalogSession) -> Option<Value> {
    let metadata = fs::metadata(&entry.source_path)
        .or_else(|_| {
            entry
                .source_path
                .parent()
                .map_or_else(|| fs::metadata("."), fs::metadata)
        })
        .or_else(|_| fs::metadata(std::env::temp_dir()))
        .ok()?;
    let mut session = session_from_messages_with_title(
        adapter,
        &entry.source_path,
        &metadata,
        &entry.source_kind,
        entry.native_session_id.clone(),
        Vec::new(),
        entry.title.clone(),
    );
    if let Some(object) = session.as_object_mut() {
        if let Some(updated_at) = entry.updated_at {
            object.insert("updatedAt".to_string(), json!(system_time(updated_at)));
        }
        if let Some(created_at) = entry.created_at {
            object.insert("createdAt".to_string(), json!(system_time(created_at)));
        }
        if let Some(working_directory) = entry
            .working_directory
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            object.insert("workingDirectory".to_string(), json!(working_directory));
        }
        if let Some(model) = entry
            .model
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            object.insert("model".to_string(), json!(model));
        }
        if entry.running {
            object.insert("running".to_string(), json!(true));
        }
        object.insert(
            "messageCount".to_string(),
            json!(entry.message_count.unwrap_or(0)),
        );
        let total = entry.message_count.unwrap_or(0);
        object.insert("sourceMessageCount".to_string(), json!(total));
        object.insert(
            "messagePage".to_string(),
            json!({
                "start": total,
                "endExclusive": total,
                "returned": 0,
                "total": total,
                "hasEarlier": false,
                "nextBefore": Value::Null
            }),
        );
    }
    Some(session)
}

/// Bounded-work counters for one browse pass. Internal diagnostics only: the
/// public catalog DTO is unchanged, and tests assert cache bounds.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct BrowseWorkCounters {
    pub(crate) cache_hits: usize,
    pub(crate) cache_misses: usize,
    pub(crate) cache_entries: usize,
    pub(crate) cache_bytes: usize,
    pub(crate) cache_discards: usize,
}

/// Cache identity of one hydration unit: every source file that shaped the
/// projection, plus the Codex thread database the parser consulted for
/// delegated labels. A missing or unreadable source makes the key unavailable
/// so the unit is parsed fresh instead of trusting an ambiguous cache state.
fn projection_cache_key(
    adapter: HistoryAdapter,
    source_kind: &str,
    hydration: &CatalogHydration,
    params: &Value,
) -> Option<ProjectionCacheKey> {
    let adapter_id = adapter.id().to_string();
    let source_kind = source_kind.to_string();
    match hydration {
        CatalogHydration::File(path) => Some(ProjectionCacheKey {
            adapter_id,
            source_kind,
            kind: "file".to_string(),
            sources: catalog_source_fingerprints(path)?,
            authority: None,
        }),
        CatalogHydration::KimiWireDirectory(directory) => {
            let sources = kimi_wire_files(directory)
                .into_iter()
                .filter_map(|path| SourceFingerprint::from_path(&path))
                .collect::<Vec<_>>();
            if sources.is_empty() {
                return None;
            }
            Some(ProjectionCacheKey {
                adapter_id,
                source_kind,
                kind: "kimi-wire-directory".to_string(),
                sources,
                authority: None,
            })
        }
        CatalogHydration::TranscriptWithDelegatedTasks {
            transcript,
            delegated,
            ..
        } => {
            let mut sources = Vec::with_capacity(1 + delegated.len());
            sources.push(SourceFingerprint::from_path(transcript)?);
            for child in delegated {
                sources.push(SourceFingerprint::from_path(child)?);
            }
            let authority = (adapter == HistoryAdapter::Codex)
                .then(|| codex_state_database(params))
                .flatten()
                .and_then(|path| {
                    let mut fingerprints = catalog_source_fingerprints(&path)?;
                    let database = fingerprints.remove(0);
                    sources.extend(fingerprints);
                    Some(database)
                });
            Some(ProjectionCacheKey {
                adapter_id,
                source_kind,
                kind: "transcript-delegated".to_string(),
                sources,
                authority,
            })
        }
        CatalogHydration::None => None,
    }
}

fn catalog_source_fingerprints(path: &Path) -> Option<Vec<SourceFingerprint>> {
    let mut sources = vec![SourceFingerprint::from_path(path)?];
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "db" | "sqlite" | "sqlite3" | "vscdb"))
    {
        // A committed SQLite write can live entirely in WAL while the main
        // database's size and modification time stay unchanged.
        let mut wal = path.as_os_str().to_os_string();
        wal.push("-wal");
        if let Some(fingerprint) = SourceFingerprint::from_path(Path::new(&wal)) {
            sources.push(fingerprint);
        }
    }
    Some(sources)
}

// ---------------------------------------------------------------------------
// Codex: state_*.sqlite threads (fail-closed) + fresh rollout supplement.
// ---------------------------------------------------------------------------

fn codex_catalog(roots: &[HistoryRoot], cutoff: SystemTime, catalog: &mut SessionCatalog) {
    let Some(sessions_root) = roots
        .iter()
        .find(|root| root.source_kind == "codex-session-store")
    else {
        return;
    };
    let sessions_dir = sessions_root.path.clone();
    let mut known_ids = HashSet::<String>::new();
    let mut parents_by_child = newest_codex_state_database(&sessions_dir)
        .and_then(|path| read_codex_spawn_edges(&path))
        .unwrap_or_default();
    if let Some(state_db) = newest_codex_state_database(&sessions_dir) {
        match read_codex_state_threads(&state_db, cutoff) {
            Ok(entries) => {
                catalog.files_seen += 1;
                // Every indexed thread counts as known, including a delegated
                // thread folded into its conversation. Otherwise the rollout scan
                // below re-adds it as its own row.
                known_ids.extend(entries.iter().map(|entry| entry.native_session_id.clone()));
                catalog.sessions.extend(entries);
            }
            Err(skip) => catalog.skipped.push(skip),
        }
    }
    // Rollouts the state database has not indexed yet (including every rollout
    // when the schema was rejected) still surface from the session store itself.
    // The archived session store is intentionally left out of browse lists.
    let discovery = discover_history_files(
        HistoryAdapter::Codex,
        &[HistoryRoot {
            path: sessions_dir,
            source_kind: sessions_root.source_kind.clone(),
            explicitly_selected: sessions_root.explicitly_selected,
        }],
        HistoryDiscoveryOptions::default(),
    );
    catalog.files_seen += discovery.files_seen;
    catalog.directory_entries_seen += discovery.directory_entries_seen;
    catalog.skipped.extend(discovery.skipped);
    for candidate in discovery.candidates {
        if candidate.modified_at < cutoff {
            continue;
        }
        let (recorded_identity, working_directory, parent) =
            codex_rollout_header_facts(&candidate.path);
        // The rollout header owns native identity. The filename UUID is only
        // the layout fallback for a header that records none, so a rollout
        // whose record id differs from its file stem still opens itself.
        let Some(session_id) =
            recorded_identity.or_else(|| rollout_session_id_from_filename(&candidate.path))
        else {
            continue;
        };
        if let Some(parent) = parent {
            parents_by_child.entry(session_id.clone()).or_insert(parent);
        }
        if !known_ids.insert(session_id.clone()) {
            continue;
        }
        catalog.sessions.push(CatalogSession {
            native_session_id: session_id,
            source_path: candidate.path.clone(),
            source_kind: candidate.source_kind,
            title: None,
            created_at: None,
            updated_at: Some(candidate.modified_at),
            working_directory,
            message_count: None,
            model: None,
            running: false,
            hydrate: CatalogHydration::File(candidate.path),
        });
    }
    catalog.sessions =
        fold_codex_delegated_threads(&parents_by_child, std::mem::take(&mut catalog.sessions));
}

/// Fold Codex delegated threads into the conversation that spawned them.
///
/// Codex runs each delegated task as its own thread and records the edge in
/// `thread_spawn_edges`. Without reading that graph every delegated task occupies
/// its own browse row and its conversation shows none of the work it delegated.
fn fold_codex_delegated_threads(
    parents_by_child: &BTreeMap<String, String>,
    entries: Vec<CatalogSession>,
) -> Vec<CatalogSession> {
    if parents_by_child.is_empty() {
        return entries;
    }
    let rollout_by_id = entries
        .iter()
        .map(|entry| (entry.native_session_id.clone(), entry.source_path.clone()))
        .collect::<BTreeMap<_, _>>();
    let present = rollout_by_id.keys().cloned().collect::<HashSet<_>>();
    let mut roots_by_child = BTreeMap::<String, String>::new();
    let mut delegated_by_parent = BTreeMap::<String, Vec<PathBuf>>::new();
    for child in rollout_by_id.keys() {
        let root = explicit_lineage_root(child, parents_by_child, &present);
        if root == *child || !rollout_by_id.contains_key(&root) {
            continue;
        }
        roots_by_child.insert(child.clone(), root.clone());
        if let Some(rollout) = rollout_by_id.get(child) {
            delegated_by_parent
                .entry(root)
                .or_default()
                .push(rollout.clone());
        }
    }
    entries
        .into_iter()
        .filter(|entry| !roots_by_child.contains_key(&entry.native_session_id))
        .map(|mut entry| {
            let Some(mut delegated) = delegated_by_parent.remove(&entry.native_session_id) else {
                return entry;
            };
            delegated.sort();
            entry.hydrate = CatalogHydration::TranscriptWithDelegatedTasks {
                transcript: entry.source_path.clone(),
                delegated,
            };
            entry
        })
        .collect()
}

fn explicit_lineage_root(
    session_id: &str,
    parents_by_child: &BTreeMap<String, String>,
    present: &HashSet<String>,
) -> String {
    let mut current = session_id.to_string();
    let mut visited = HashSet::new();
    while visited.insert(current.clone()) {
        let Some(parent) = parents_by_child.get(&current) else {
            return current;
        };
        if !present.contains(parent) {
            return current;
        }
        current.clone_from(parent);
    }
    // A cycle has no authoritative root. Leave every member standalone.
    session_id.to_string()
}

/// Mark every parsed Codex session the thread database records as delegated, so
/// the shared merge folds it into its conversation on read paths that build no
/// catalog.
pub(crate) fn apply_codex_spawn_lineage(
    params: &Value,
    parents_by_child: &BTreeMap<String, String>,
    sessions: &mut [Value],
) {
    if parents_by_child.is_empty() {
        return;
    }
    let labels = codex_delegated_labels(params);
    for session in sessions.iter_mut() {
        let Some(own_id) = session
            .get("nativeSessionId")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            continue;
        };
        let Some(parent) = parents_by_child.get(&own_id) else {
            continue;
        };
        let declared = labels.get(&own_id).cloned().unwrap_or_default();
        let title = declared.title.or_else(|| declared_delegated_title(session));
        let Some(object) = session.as_object_mut() else {
            continue;
        };
        object.insert("delegatedSubagent".to_string(), json!(true));
        object.insert("parentSessionId".to_string(), json!(parent));
        if let Some(title) = title {
            object
                .entry("subagentTitle".to_string())
                .or_insert_with(|| json!(title));
        }
        if let Some(role) = declared.role {
            object
                .entry("subagentType".to_string())
                .or_insert_with(|| json!(role));
        }
    }
}

/// Delegated thread ids the requested Codex conversations spawned, so a
/// single-conversation read can pull their rollouts into scope.
pub(crate) fn codex_delegated_thread_ids(
    parents_by_child: &BTreeMap<String, String>,
    requested: &[String],
) -> Vec<String> {
    let mut children_by_parent = BTreeMap::<&str, Vec<&str>>::new();
    for (child, parent) in parents_by_child {
        children_by_parent.entry(parent).or_default().push(child);
    }
    let mut wanted = requested.iter().map(String::as_str).collect::<HashSet<_>>();
    let mut pending = requested
        .iter()
        .map(String::as_str)
        .collect::<VecDeque<_>>();
    let mut children = Vec::new();
    while let Some(parent) = pending.pop_front() {
        for child in children_by_parent.get(parent).into_iter().flatten() {
            if wanted.insert(child) {
                pending.push_back(child);
                children.push((*child).to_string());
            }
        }
    }
    children
}

/// Parent thread of each Codex conversation, for read paths that carry no
/// catalog. Database edges take precedence over explicit rollout-header edges.
pub(crate) fn codex_spawn_lineage(params: &Value) -> BTreeMap<String, String> {
    let mut parents = codex_database_spawn_lineage(params);
    let roots = history_roots(HistoryAdapter::Codex, params);
    let discovery = discover_history_files(
        HistoryAdapter::Codex,
        &roots,
        HistoryDiscoveryOptions::default(),
    );
    for candidate in discovery.candidates {
        let (id, _, parent) = codex_rollout_header_facts(&candidate.path);
        if let (Some(id), Some(parent)) = (id, parent) {
            parents.entry(id).or_insert(parent);
        }
    }
    parents
}

fn codex_database_spawn_lineage(params: &Value) -> BTreeMap<String, String> {
    codex_state_database(params)
        .and_then(|state_db| read_codex_spawn_edges(&state_db))
        .unwrap_or_default()
}

fn codex_state_database(params: &Value) -> Option<PathBuf> {
    history_roots(HistoryAdapter::Codex, params)
        .iter()
        .find(|root| root.source_kind == "codex-session-store")
        .and_then(|root| newest_codex_state_database(&root.path))
}

/// How Codex names one delegated thread.
///
/// Codex gives each delegated agent a nickname and a role and keeps them in the
/// thread database, not in the rollout. They are the only curated labels the
/// store has for delegated work, so a card that ignores them falls back to
/// whatever the first prompt line happens to be.
#[derive(Clone, Debug, Default)]
pub(crate) struct CodexDelegatedLabel {
    pub(crate) title: Option<String>,
    pub(crate) role: Option<String>,
}

pub(crate) fn codex_delegated_labels(params: &Value) -> BTreeMap<String, CodexDelegatedLabel> {
    let Some(state_db) = codex_state_database(params) else {
        return BTreeMap::new();
    };
    let Some(connection) = open_read_only_connection(&state_db) else {
        return BTreeMap::new();
    };
    if !sqlite_table_exists(&connection, "threads") {
        return BTreeMap::new();
    }
    let Ok(columns) = sqlite_columns(&connection, "threads") else {
        return BTreeMap::new();
    };
    let optional = |name: &str| {
        if columns.contains(name) {
            format!("\"{name}\"")
        } else {
            "NULL".to_string()
        }
    };
    let sql = format!(
        "SELECT id, {}, {}, {} FROM threads",
        optional("agent_nickname"),
        optional("agent_role"),
        optional("first_user_message"),
    );
    let Ok(mut statement) = connection.prepare(&sql) else {
        return BTreeMap::new();
    };
    let Ok(rows) = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    }) else {
        return BTreeMap::new();
    };
    let mut labels = BTreeMap::new();
    for (id, nickname, role, first_message) in rows.flatten() {
        let clean = |value: Option<String>| {
            value
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        };
        let title = clean(first_message)
            .and_then(|message| delegated_task_label(&message))
            .or_else(|| clean(nickname));
        let role = clean(role);
        if title.is_some() || role.is_some() {
            labels.insert(id, CodexDelegatedLabel { title, role });
        }
    }
    labels
}

/// Child thread to parent thread, from the Codex thread database. Returns `None`
/// when the table is absent so an older database simply keeps flat rows.
fn read_codex_spawn_edges(state_db: &Path) -> Option<BTreeMap<String, String>> {
    let connection = open_read_only_connection(state_db)?;
    if !sqlite_table_exists(&connection, "thread_spawn_edges") {
        return None;
    }
    let columns = sqlite_columns(&connection, "thread_spawn_edges").ok()?;
    if !columns.contains("parent_thread_id") || !columns.contains("child_thread_id") {
        return None;
    }
    let mut statement = connection
        .prepare("SELECT child_thread_id, parent_thread_id FROM thread_spawn_edges")
        .ok()?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .ok()?;
    let mut edges = BTreeMap::<String, String>::new();
    for (child, parent) in rows.flatten() {
        if child.trim().is_empty() || parent.trim().is_empty() || child == parent {
            continue;
        }
        edges.insert(child, parent);
    }
    Some(edges)
}

/// Identity and project directory one Codex rollout records in its
/// `session_meta` header. Record identity is authoritative here: the filename
/// UUID is only a layout fallback for a header that names no identity. Only
/// the bounded head of the file is read; the header is always the first
/// record.
fn codex_rollout_header_facts(path: &Path) -> (Option<String>, Option<String>, Option<String>) {
    (|| {
        let file = fs::File::open(path).ok()?;
        let mut head = Vec::new();
        BufReader::new(file.take(MAX_TITLE_PROBE_BYTES))
            .read_until(b'\n', &mut head)
            .ok()?;
        let head = String::from_utf8_lossy(&head);
        let line = head.lines().next()?;
        let value = serde_json::from_str::<Value>(line).ok()?;
        let payload = value.get("payload").unwrap_or(&value);
        let identity = (value.get("type").and_then(Value::as_str) == Some("session_meta"))
            .then(|| {
                payload
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .map(str::to_string)
            })
            .flatten();
        let cwd = payload
            .get("cwd")
            .and_then(Value::as_str)
            .and_then(bounded_project_workspace);
        let parent = identity.as_deref().and_then(|id| {
            let (parent, delegated, _) = super::codex::codex_rollout_lineage(payload, id);
            delegated.then_some(parent).flatten()
        });
        Some((identity, cwd, parent))
    })()
    .unwrap_or((None, None, None))
}

fn newest_codex_state_database(sessions_dir: &Path) -> Option<PathBuf> {
    let codex_dir = sessions_dir.parent()?;
    fs::read_dir(codex_dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            let version = name
                .strip_prefix("state_")?
                .strip_suffix(".sqlite")?
                .parse::<u64>()
                .ok()?;
            Some((version, entry.path()))
        })
        .max_by_key(|(version, _)| *version)
        .map(|(_, path)| path)
}

/// Read the codex app-server thread catalog. The database is codex-internal and
/// versioned by filename, so drift in the identity or ordering columns is
/// rejected as a whole and the caller falls back to rollout files instead of
/// trusting partial rows. Presentation columns are additive and project as NULL
/// when an older state schema omits them.
fn read_codex_state_threads(
    db_path: &Path,
    cutoff: SystemTime,
) -> std::result::Result<Vec<CatalogSession>, Value> {
    let fail = |reason: &str| json!({"path": db_path.to_string_lossy(), "reason": reason});
    let connection =
        open_read_only_connection(db_path).ok_or_else(|| fail("codex_state_open_failed"))?;
    if !sqlite_table_exists(&connection, "threads") {
        return Err(fail("codex_state_schema_unrecognized"));
    }
    let columns = sqlite_columns(&connection, "threads")
        .map_err(|_| fail("codex_state_schema_unrecognized"))?;
    for required in ["id", "rollout_path", "created_at", "updated_at"] {
        if !columns.contains(required) {
            return Err(fail("codex_state_schema_unrecognized"));
        }
    }
    // Codex's state database is an internal, versioned store. Identity and
    // ordering columns are required for the catalog; presentation columns are
    // additive and must not make the whole catalog disappear when an older
    // state schema omits them.
    let optional_column = |name: &str| {
        if columns.contains(name) {
            name.to_string()
        } else {
            "NULL".to_string()
        }
    };
    let cutoff_seconds = cutoff
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    let mut statement = connection
        .prepare(&format!(
            "SELECT id, rollout_path, created_at, updated_at, {}, {}, {} \
             FROM threads WHERE COALESCE({}, 0) = 0 AND updated_at >= ?1 \
             ORDER BY updated_at DESC, id ASC",
            optional_column("title"),
            optional_column("cwd"),
            optional_column("model"),
            optional_column("archived"),
        ))
        .map_err(|_| fail("codex_state_schema_unrecognized"))?;
    let rows = statement
        .query_map([cutoff_seconds], |row| {
            let rollout_path: String = row.get(1)?;
            Ok(CatalogSession {
                native_session_id: row.get(0)?,
                source_path: PathBuf::from(&rollout_path),
                source_kind: "codex-session-store".to_string(),
                title: row
                    .get::<_, Option<String>>(4)?
                    .filter(|value| !value.trim().is_empty()),
                created_at: row.get::<_, Option<i64>>(2)?.and_then(epoch_to_system_time),
                updated_at: row.get::<_, Option<i64>>(3)?.and_then(epoch_to_system_time),
                working_directory: row
                    .get::<_, Option<String>>(5)?
                    .as_deref()
                    .and_then(bounded_project_workspace),
                message_count: None,
                model: row
                    .get::<_, Option<String>>(6)?
                    .filter(|value| !value.trim().is_empty()),
                running: false,
                hydrate: CatalogHydration::File(PathBuf::from(rollout_path)),
            })
        })
        .map_err(|_| fail("codex_state_read_failed"))?;
    let mut entries = Vec::new();
    for row in rows {
        entries.push(row.map_err(|_| fail("codex_state_read_failed"))?);
    }
    Ok(entries)
}

// ---------------------------------------------------------------------------
// OpenCode / Kilo Code: native session table with archive and parent filters.
// ---------------------------------------------------------------------------

fn openagent_catalog(
    adapter: HistoryAdapter,
    roots: &[HistoryRoot],
    cutoff: SystemTime,
    catalog: &mut SessionCatalog,
) {
    for root in roots {
        let db_path = match (adapter, root.source_kind.as_str()) {
            (HistoryAdapter::KiloCode, "kilo-session-database") => root.path.clone(),
            (HistoryAdapter::OpenCode, "opencode-data") => root.path.join("opencode.db"),
            _ => continue,
        };
        match read_openagent_sessions(adapter, &db_path, cutoff) {
            Ok(entries) => {
                catalog.files_seen += 1;
                catalog.sessions.extend(entries);
            }
            Err(skip) => {
                if db_path.exists() {
                    catalog.skipped.push(skip);
                }
            }
        }
    }
}

fn read_openagent_sessions(
    adapter: HistoryAdapter,
    db_path: &Path,
    cutoff: SystemTime,
) -> std::result::Result<Vec<CatalogSession>, Value> {
    let fail = |reason: &str| json!({"path": db_path.to_string_lossy(), "reason": reason});
    let connection =
        open_read_only_connection(db_path).ok_or_else(|| fail("openagent_state_open_failed"))?;
    if !sqlite_table_exists(&connection, "session") {
        return Err(fail("openagent_state_schema_unrecognized"));
    }
    let columns = sqlite_columns(&connection, "session")
        .map_err(|_| fail("openagent_state_schema_unrecognized"))?;
    for required in ["id", "time_updated"] {
        if !columns.contains(required) {
            return Err(fail("openagent_state_schema_unrecognized"));
        }
    }
    let optional_column = |name: &str| {
        if columns.contains(name) {
            name.to_string()
        } else {
            "NULL".to_string()
        }
    };
    let mut sql = format!(
        "SELECT id, {}, {}, {}, {}, time_updated, {} FROM session WHERE time_updated >= ?1",
        optional_column("title"),
        optional_column("directory"),
        optional_column("model"),
        optional_column("time_created"),
        optional_column("parent_id"),
    );
    if columns.contains("time_archived") {
        sql.push_str(" AND (time_archived IS NULL OR time_archived = 0)");
    }
    sql.push_str(" ORDER BY time_updated DESC, id ASC");
    let cutoff_millis = cutoff
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0);
    let mut statement = connection
        .prepare(&sql)
        .map_err(|_| fail("openagent_state_schema_unrecognized"))?;
    let rows = statement
        .query_map([cutoff_millis], |row| {
            Ok((
                CatalogSession {
                    native_session_id: row.get(0)?,
                    source_path: db_path.to_path_buf(),
                    source_kind: match adapter {
                        HistoryAdapter::KiloCode => "kilo-session-database".to_string(),
                        _ => "opencode-session-database".to_string(),
                    },
                    title: row
                        .get::<_, Option<String>>(1)?
                        .filter(|value| !value.trim().is_empty()),
                    created_at: row.get::<_, Option<i64>>(4)?.and_then(epoch_to_system_time),
                    updated_at: row.get::<_, Option<i64>>(5)?.and_then(epoch_to_system_time),
                    working_directory: row
                        .get::<_, Option<String>>(2)?
                        .as_deref()
                        .and_then(bounded_project_workspace),
                    message_count: None,
                    model: row
                        .get::<_, Option<String>>(3)?
                        .filter(|value| !value.trim().is_empty()),
                    // The session store holds the conversation too, so the browse row
                    // carries its messages instead of rendering as an empty row.
                    running: false,
                    hydrate: CatalogHydration::File(db_path.to_path_buf()),
                },
                row.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(|_| fail("openagent_state_read_failed"))?;
    let mut entries = Vec::new();
    for row in rows {
        entries.push(row.map_err(|_| fail("openagent_state_read_failed"))?);
    }
    let parents = entries
        .iter()
        .filter_map(|(entry, parent)| {
            parent
                .as_ref()
                .filter(|parent| !parent.trim().is_empty())
                .map(|parent| (entry.native_session_id.clone(), parent.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    let present = entries
        .iter()
        .map(|(entry, _)| entry.native_session_id.clone())
        .collect::<HashSet<_>>();
    Ok(entries
        .into_iter()
        .filter_map(|(entry, _)| {
            (explicit_lineage_root(&entry.native_session_id, &parents, &present)
                == entry.native_session_id)
                .then_some(entry)
        })
        .collect())
}

/// Apply the same explicit `session.parent_id` relation used by browse catalog
/// filtering to exact and search projections. Prompt text never participates.
pub(crate) fn apply_openagent_parent_lineage(
    adapter: HistoryAdapter,
    params: &Value,
    sessions: &mut [Value],
) {
    let mut parents = BTreeMap::<String, String>::new();
    for root in history_roots(adapter, params) {
        let database = match (adapter, root.source_kind.as_str()) {
            (HistoryAdapter::KiloCode, "kilo-session-database") => root.path,
            (HistoryAdapter::OpenCode, "opencode-data") => root.path.join("opencode.db"),
            _ => continue,
        };
        let Some(connection) = open_read_only_connection(&database) else {
            continue;
        };
        if !sqlite_table_exists(&connection, "session") {
            continue;
        }
        let Ok(columns) = sqlite_columns(&connection, "session") else {
            continue;
        };
        if !columns.contains("id") || !columns.contains("parent_id") {
            continue;
        }
        let Ok(mut statement) = connection.prepare(
            "SELECT id, parent_id FROM session WHERE parent_id IS NOT NULL ORDER BY id ASC",
        ) else {
            continue;
        };
        let Ok(rows) = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }) else {
            continue;
        };
        for (child, parent) in rows.flatten() {
            if !child.trim().is_empty() && !parent.trim().is_empty() && child != parent {
                parents.insert(child, parent);
            }
        }
    }
    if parents.is_empty() {
        return;
    }
    for session in sessions.iter_mut() {
        let Some(id) = session
            .get("nativeSessionId")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            continue;
        };
        let Some(parent) = parents.get(&id) else {
            continue;
        };
        if let Some(object) = session.as_object_mut() {
            object.insert("delegatedSubagent".to_string(), json!(true));
            object.insert("parentSessionId".to_string(), json!(parent));
        }
    }
}

// ---------------------------------------------------------------------------
// Copilot: CLI session-store rows plus precise VS Code chat stores/transcripts.
// ---------------------------------------------------------------------------

fn copilot_catalog(roots: &[HistoryRoot], cutoff: SystemTime, catalog: &mut SessionCatalog) {
    for root in roots {
        match root.source_kind.as_str() {
            "copilot-cli-session-store" => copilot_sqlite_catalog(root, cutoff, catalog),
            "vscode-copilot-workspace-storage" => {
                copilot_vscode_workspace_catalog(root, cutoff, catalog)
            }
            "vscode-copilot-global-storage" => {
                copilot_vscode_store_catalog(&root.path.join("state.vscdb"), root, cutoff, catalog);
                copilot_vscode_transcript_catalog(&root.path, root, cutoff, catalog);
            }
            _ => generic_file_root_catalog(HistoryAdapter::Copilot, root, cutoff, catalog),
        }
    }
}

fn copilot_vscode_workspace_catalog(
    root: &HistoryRoot,
    cutoff: SystemTime,
    catalog: &mut SessionCatalog,
) {
    let Ok(entries) = fs::read_dir(&root.path) else {
        return;
    };
    let mut workspaces = Vec::<PathBuf>::new();
    for entry in entries.flatten() {
        catalog.directory_entries_seen = catalog.directory_entries_seen.saturating_add(1);
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            workspaces.push(entry.path());
        }
    }
    workspaces.sort();
    for workspace in workspaces {
        copilot_vscode_store_catalog(&workspace.join("state.vscdb"), root, cutoff, catalog);
        copilot_vscode_transcript_catalog(&workspace, root, cutoff, catalog);
    }
}

fn copilot_vscode_transcript_catalog(
    parent: &Path,
    root: &HistoryRoot,
    cutoff: SystemTime,
    catalog: &mut SessionCatalog,
) {
    let transcript_root = parent.join("GitHub.copilot-chat/transcripts");
    if !transcript_root.is_dir() {
        return;
    }
    let discovery = discover_history_files(
        HistoryAdapter::Copilot,
        std::slice::from_ref(&HistoryRoot {
            path: transcript_root,
            source_kind: root.source_kind.clone(),
            explicitly_selected: root.explicitly_selected,
        }),
        HistoryDiscoveryOptions::default(),
    );
    catalog.files_seen += discovery.files_seen;
    catalog.directory_entries_seen += discovery.directory_entries_seen;
    catalog.skipped.extend(discovery.skipped);
    for candidate in discovery.candidates {
        let extension = candidate
            .path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default();
        if matches!(extension, "jsonl" | "ndjson") {
            push_generic_file_catalog_session(HistoryAdapter::Copilot, candidate, cutoff, catalog);
        }
    }
}

fn copilot_vscode_store_catalog(
    store: &Path,
    root: &HistoryRoot,
    cutoff: SystemTime,
    catalog: &mut SessionCatalog,
) {
    let Ok(metadata) = fs::metadata(store) else {
        return;
    };
    let store_recency = metadata.modified().unwrap_or(UNIX_EPOCH);
    if store_recency < cutoff {
        return;
    }
    catalog.files_seen += 1;
    let Some(connection) = open_read_only_connection(store) else {
        catalog.skipped.push(json!({
            "path": store.to_string_lossy(),
            "reason": "copilot_vscode_store_unreadable"
        }));
        return;
    };
    let document = match copilot_chat_sessions_document(&connection) {
        Ok(Some(document)) => document,
        Ok(None) => {
            catalog.skipped.push(json!({
                "path": store.to_string_lossy(),
                "reason": "copilot_vscode_store_has_no_conversations"
            }));
            return;
        }
        Err(error) => {
            let reason = match error {
                CopilotChatSessionsReadError::SchemaUnrecognized => {
                    "copilot_vscode_store_schema_unrecognized"
                }
                CopilotChatSessionsReadError::ReadFailed => "copilot_vscode_store_read_failed",
                CopilotChatSessionsReadError::InvalidPayload => {
                    "copilot_vscode_store_payload_invalid"
                }
            };
            catalog.skipped.push(json!({
                "path": store.to_string_lossy(),
                "reason": reason
            }));
            return;
        }
    };
    let Some(items) = document.get("chatSessions").and_then(Value::as_array) else {
        catalog.skipped.push(json!({
            "path": store.to_string_lossy(),
            "reason": "copilot_vscode_store_schema_unrecognized"
        }));
        return;
    };
    for (index, item) in items.iter().enumerate() {
        let messages = item
            .get("messages")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let native_session_id =
            crate::domain::conversation::snapshot_identity::extract_native_session_id(item)
                .unwrap_or_else(|| {
                    session_id(
                        HistoryAdapter::Copilot.id(),
                        store,
                        &format!("chatSessions-{index}"),
                    )
                });
        let created_at = copilot_session_time(item, &["createdAt", "created_at"])
            .or_else(|| messages.iter().filter_map(copilot_message_time).min());
        let updated_at = copilot_session_time(
            item,
            &[
                "updatedAt",
                "updated_at",
                "lastActivity",
                "lastActivityAt",
                "timestamp",
                "time",
            ],
        )
        .or_else(|| messages.iter().filter_map(copilot_message_time).max())
        .or(created_at)
        .unwrap_or(store_recency);
        if updated_at < cutoff {
            continue;
        }
        let working_directory = ["cwd", "workingDirectory", "projectPath"]
            .iter()
            .find_map(|key| item.get(*key).and_then(Value::as_str))
            .and_then(bounded_project_workspace);
        catalog.sessions.push(CatalogSession {
            native_session_id,
            source_path: store.to_path_buf(),
            source_kind: root.source_kind.clone(),
            title: extract_conversation_title(item),
            created_at,
            updated_at: Some(updated_at),
            working_directory,
            message_count: None,
            model: None,
            running: false,
            hydrate: CatalogHydration::File(store.to_path_buf()),
        });
    }
}

fn copilot_session_time(value: &Value, keys: &[&str]) -> Option<SystemTime> {
    let object = value.as_object()?;
    keys.iter()
        .find_map(|key| object.get(*key).and_then(copilot_time_value))
}

fn copilot_message_time(value: &Value) -> Option<SystemTime> {
    copilot_session_time(
        value,
        &[
            "updatedAt",
            "updated_at",
            "createdAt",
            "created_at",
            "timestamp",
            "time",
            "date",
        ],
    )
}

fn copilot_time_value(value: &Value) -> Option<SystemTime> {
    if let Some(text) = value.as_str() {
        return rfc3339_to_system_time(text)
            .or_else(|| text.parse::<i64>().ok().and_then(epoch_to_system_time));
    }
    value.as_i64().and_then(epoch_to_system_time)
}

fn copilot_sqlite_catalog(root: &HistoryRoot, cutoff: SystemTime, catalog: &mut SessionCatalog) {
    let session_state_dir = &root.path;
    let Some(copilot_home) = session_state_dir.parent() else {
        return;
    };
    let db_path = copilot_home.join("session-store.db");
    if !db_path.is_file() {
        return;
    }
    match read_copilot_sessions(&db_path, session_state_dir, cutoff) {
        Ok(entries) => {
            catalog.files_seen += 1;
            catalog.sessions.extend(entries);
        }
        Err(skip) => catalog.skipped.push(skip),
    }
}

fn read_copilot_sessions(
    db_path: &Path,
    session_state_dir: &Path,
    cutoff: SystemTime,
) -> std::result::Result<Vec<CatalogSession>, Value> {
    let fail = |reason: &str| json!({"path": db_path.to_string_lossy(), "reason": reason});
    let connection =
        open_read_only_connection(db_path).ok_or_else(|| fail("copilot_state_open_failed"))?;
    if !sqlite_table_exists(&connection, "sessions") {
        return Err(fail("copilot_state_schema_unrecognized"));
    }
    let columns = sqlite_columns(&connection, "sessions")
        .map_err(|_| fail("copilot_state_schema_unrecognized"))?;
    for required in ["id", "updated_at"] {
        if !columns.contains(required) {
            return Err(fail("copilot_state_schema_unrecognized"));
        }
    }
    let optional_column = |name: &str| {
        if columns.contains(name) {
            name.to_string()
        } else {
            "NULL".to_string()
        }
    };
    let sql = format!(
        "SELECT id, {}, {}, {}, updated_at FROM sessions WHERE updated_at >= ?1 ORDER BY updated_at DESC, id ASC",
        optional_column("cwd"),
        optional_column("summary"),
        optional_column("created_at"),
    );
    let cutoff_text = system_time(cutoff);
    let mut statement = connection
        .prepare(&sql)
        .map_err(|_| fail("copilot_state_schema_unrecognized"))?;
    let rows = statement
        .query_map([cutoff_text], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(|_| fail("copilot_state_read_failed"))?;
    let mut entries = Vec::new();
    for row in rows {
        let (id, cwd, summary, created_at, updated_at) =
            row.map_err(|_| fail("copilot_state_read_failed"))?;
        // The store can hold sessions whose on-disk state is gone (for example
        // remote sessions); only locally resumable sessions are listed.
        let state_dir = session_state_dir.join(&id);
        if !state_dir.is_dir() {
            continue;
        }
        let title = summary
            .filter(|value| !value.trim().is_empty())
            .or_else(|| workspace_yaml_title(&state_dir.join("workspace.yaml")));
        entries.push(CatalogSession {
            native_session_id: id,
            source_path: state_dir,
            source_kind: "copilot-cli-session-store".to_string(),
            title,
            created_at: created_at.as_deref().and_then(rfc3339_to_system_time),
            updated_at: updated_at.as_deref().and_then(rfc3339_to_system_time),
            working_directory: cwd.filter(|value| !value.trim().is_empty()),
            message_count: None,
            model: None,
            running: false,
            hydrate: CatalogHydration::None,
        });
    }
    Ok(entries)
}

fn workspace_yaml_title(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    for line in raw.lines().take(64) {
        let trimmed = line.trim();
        for key in ["name:", "title:"] {
            if let Some(value) = trimmed.strip_prefix(key) {
                let value = value.trim().trim_matches('"').trim_matches('\'');
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Kimi Code: one state.json per session directory; wire files only for pages.
// ---------------------------------------------------------------------------

fn kimi_code_catalog(roots: &[HistoryRoot], cutoff: SystemTime, catalog: &mut SessionCatalog) {
    for root in roots {
        if root.source_kind != "kimi-code-session-store" {
            continue;
        }
        let mut state_files = Vec::<PathBuf>::new();
        collect_named_files(&root.path, "state.json", 0, catalog, &mut state_files);
        for path in state_files {
            catalog.files_seen += 1;
            match read_kimi_state(&path) {
                Some(entry) => {
                    if entry.recency() >= cutoff {
                        catalog.sessions.push(entry);
                    }
                }
                None => catalog.skipped.push(json!({
                    "path": path.to_string_lossy(),
                    "reason": "kimi_state_unreadable"
                })),
            }
        }
    }
}

fn read_kimi_state(path: &Path) -> Option<CatalogSession> {
    let raw = fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<Value>(&raw).ok()?;
    let session_dir = path.parent()?.to_path_buf();
    let native_session_id = session_dir.file_name()?.to_str()?.to_string();
    let wire = kimi_wire_files(&session_dir).into_iter().next();
    Some(CatalogSession {
        native_session_id,
        source_path: wire.unwrap_or_else(|| path.to_path_buf()),
        source_kind: "kimi-code-session-store".to_string(),
        title: value
            .get("title")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string),
        created_at: value
            .get("createdAt")
            .and_then(Value::as_str)
            .and_then(rfc3339_to_system_time),
        updated_at: value
            .get("updatedAt")
            .and_then(Value::as_str)
            .and_then(rfc3339_to_system_time),
        working_directory: value
            .get("workDir")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string),
        message_count: None,
        model: None,
        running: false,
        hydrate: CatalogHydration::KimiWireDirectory(session_dir),
    })
}

// ---------------------------------------------------------------------------
// Cursor CLI: per-chat meta.json catalogs (content blobs stay deferred).
// ---------------------------------------------------------------------------

fn cursor_catalog(roots: &[HistoryRoot], cutoff: SystemTime, catalog: &mut SessionCatalog) {
    // Prefer roots that already know the project path before walking IDE
    // storage trees that share the catalog directory-entry budget.
    for root in roots {
        match root.source_kind.as_str() {
            "cursor-cli-chats" => cursor_cli_meta_catalog(root, cutoff, catalog),
            "cursor-cli-projects" => cursor_cli_projects_catalog(root, cutoff, catalog),
            _ => {}
        }
    }
    for root in roots {
        match root.source_kind.as_str() {
            "cursor-workspace-storage" | "cursor-global-storage" => {
                cursor_ide_store_catalog(root, cutoff, catalog);
            }
            _ => {}
        }
    }
}

/// Cursor keeps every IDE conversation inside one `state.vscdb`, so a
/// file-metadata catalog only ever sees the database itself and no conversation
/// in it. This walks the IDE storage tree for those databases and lists their
/// composers directly, which is what makes IDE conversations reachable in browse
/// mode at all.
fn cursor_ide_store_catalog(root: &HistoryRoot, cutoff: SystemTime, catalog: &mut SessionCatalog) {
    // `globalStorage/state.vscdb` and `workspaceStorage/<id>/state.vscdb` are the
    // only two shapes. Walking deeper would descend into the bundled agent-CLI
    // installs that share these trees and cost thousands of directory entries.
    let mut stores = Vec::<PathBuf>::new();
    collect_named_files_bounded(
        &root.path,
        CURSOR_IDE_STORE_FILE,
        0,
        CURSOR_IDE_STORE_WALK_DEPTH,
        catalog,
        &mut stores,
    );
    stores.sort();
    for store in stores {
        catalog.files_seen += 1;
        let Some(connection) = open_read_only_connection(&store) else {
            catalog.skipped.push(json!({
                "path": store.to_string_lossy(),
                "reason": "cursor_ide_store_unreadable"
            }));
            continue;
        };
        if !sqlite_table_exists(&connection, "cursorDiskKV") {
            continue;
        }
        let store_recency = fs::metadata(&store)
            .ok()
            .and_then(|metadata| metadata.modified().ok());
        for entry in cursor_composer_catalog(&connection) {
            if entry.parent_composer_id.is_some() {
                // Delegated work folds into its parent conversation.
                continue;
            }
            let created_at = entry.created_at.as_deref().and_then(rfc3339_to_system_time);
            let updated_at = entry
                .updated_at
                .as_deref()
                .and_then(rfc3339_to_system_time)
                .or(created_at)
                .or(store_recency);
            if updated_at.or(created_at).unwrap_or(UNIX_EPOCH) < cutoff {
                continue;
            }
            catalog.sessions.push(CatalogSession {
                native_session_id: entry.composer_id,
                source_path: store.clone(),
                source_kind: root.source_kind.clone(),
                title: entry.title,
                created_at,
                updated_at,
                working_directory: entry.working_directory,
                message_count: Some(entry.message_count),
                model: entry.model,
                running: false,
                hydrate: CatalogHydration::File(store.clone()),
            });
        }
    }
}

/// Cursor CLI project trees hold far more than conversations: `mcps/` tool
/// descriptors, `agent-tools/`, `canvases/`, `terminals/`, and `assets/` are all
/// JSON under the same root. Only `agent-transcripts/` holds conversations, so
/// the catalog walks that subtree per project instead of the whole project root.
///
/// Inside it the layout is
/// `agent-transcripts/<sessionId>/<sessionId>.jsonl` for the conversation and
/// `agent-transcripts/<sessionId>/subagents/<childId>.jsonl` for each delegated
/// task. Child transcripts are attached to their parent's hydration unit rather
/// than listed as their own conversations.
fn cursor_cli_projects_catalog(
    root: &HistoryRoot,
    cutoff: SystemTime,
    catalog: &mut SessionCatalog,
) {
    for project in cursor_project_directories(&root.path, catalog) {
        let transcripts_root = project.join(CURSOR_TRANSCRIPTS_DIRECTORY);
        if !transcripts_root.is_dir() {
            continue;
        }
        let working_directory = cursor_project_workspace_path(&project);
        let discovery = discover_history_files(
            HistoryAdapter::Cursor,
            std::slice::from_ref(&HistoryRoot {
                path: transcripts_root,
                source_kind: root.source_kind.clone(),
                explicitly_selected: root.explicitly_selected,
            }),
            HistoryDiscoveryOptions::default(),
        );
        catalog.files_seen += discovery.files_seen;
        catalog.directory_entries_seen += discovery.directory_entries_seen;
        catalog.skipped.extend(discovery.skipped);
        push_delegated_transcript_units(
            group_delegated_transcripts(discovery.candidates),
            working_directory.as_deref(),
            cutoff,
            catalog,
        );
    }
}

/// Group discovered transcripts by the conversation they belong to, so each
/// conversation carries its delegated task transcripts instead of letting them
/// occupy their own browse rows.
fn group_delegated_transcripts(
    candidates: Vec<super::super::history_discovery::HistoryFileCandidate>,
) -> BTreeMap<String, DelegatedTranscriptUnit> {
    let mut transcripts = BTreeMap::<String, DelegatedTranscriptUnit>::new();
    for candidate in candidates {
        let Some(session_id) = transcript_conversation_id(&candidate.path) else {
            continue;
        };
        if transcript_is_delegated(&candidate.path)
            && !delegated_file_is_transcript(&candidate.path)
        {
            // Workflow bookkeeping beside a delegated task is not a conversation.
            continue;
        }
        let unit = transcripts.entry(session_id).or_default();
        if transcript_is_delegated(&candidate.path) {
            unit.delegated
                .push((candidate.modified_at, candidate.path.clone()));
        } else {
            unit.transcript = Some(candidate.path.clone());
        }
        unit.modified_at = unit.modified_at.max(candidate.modified_at);
        unit.source_kind = candidate.source_kind;
    }
    for unit in transcripts.values_mut() {
        // Newest delegated work first; every explicitly related child remains
        // part of the hydration unit.
        unit.delegated
            .sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    }
    transcripts
}

fn push_delegated_transcript_units(
    units: BTreeMap<String, DelegatedTranscriptUnit>,
    working_directory: Option<&str>,
    cutoff: SystemTime,
    catalog: &mut SessionCatalog,
) {
    for (native_session_id, unit) in units {
        if unit.modified_at < cutoff {
            continue;
        }
        let Some(transcript) = unit.transcript else {
            // Every orphan keeps its task identity. The missing parent's id
            // cannot name any of these separate hydration units.
            for (updated_at, orphan) in unit.delegated {
                let Some((task_id, _)) = delegated_transcript_lineage(&orphan) else {
                    continue;
                };
                catalog.sessions.push(CatalogSession {
                    native_session_id: task_id,
                    source_path: orphan.clone(),
                    source_kind: unit.source_kind.clone(),
                    title: None,
                    created_at: None,
                    updated_at: Some(updated_at),
                    working_directory: working_directory.map(str::to_string),
                    message_count: None,
                    model: None,
                    running: false,
                    hydrate: CatalogHydration::File(orphan),
                });
            }
            continue;
        };
        catalog.sessions.push(CatalogSession {
            native_session_id,
            source_path: transcript.clone(),
            source_kind: unit.source_kind,
            title: None,
            created_at: None,
            updated_at: Some(unit.modified_at),
            working_directory: working_directory.map(str::to_string),
            message_count: None,
            model: None,
            running: false,
            hydrate: CatalogHydration::TranscriptWithDelegatedTasks {
                transcript,
                delegated: unit.delegated.into_iter().map(|(_, path)| path).collect(),
            },
        });
    }
}

struct DelegatedTranscriptUnit {
    transcript: Option<PathBuf>,
    /// Delegated transcripts with their last-modified time, newest first after
    /// grouping so the browse bound keeps the most recent work.
    delegated: Vec<(SystemTime, PathBuf)>,
    modified_at: SystemTime,
    source_kind: String,
}

impl Default for DelegatedTranscriptUnit {
    fn default() -> Self {
        Self {
            transcript: None,
            delegated: Vec::new(),
            modified_at: UNIX_EPOCH,
            source_kind: String::new(),
        }
    }
}

/// Immediate project directories below `~/.cursor/projects`.
fn cursor_project_directories(root: &Path, catalog: &mut SessionCatalog) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut projects = Vec::new();
    for entry in entries.flatten() {
        catalog.directory_entries_seen += 1;
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            projects.push(path);
        }
    }
    projects.sort();
    projects
}

/// Cursor CLI project trees record the trusted workspace in
/// `.workspace-trusted` at the project root. Only that exact file is read: a
/// marker further up (`~/.cursor/projects/.workspace-trusted` is written with
/// `workspacePath: "/"`) belongs to a different trust decision and would hand
/// every conversation the filesystem root.
fn cursor_project_workspace_path(project: &Path) -> Option<String> {
    let trusted = project.join(".workspace-trusted");
    if !trusted.is_file() {
        return None;
    }
    let raw = fs::read_to_string(&trusted).ok()?;
    let value = serde_json::from_str::<Value>(&raw).ok()?;
    bounded_project_workspace(value.get("workspacePath").and_then(Value::as_str)?)
}

fn cursor_cli_meta_catalog(root: &HistoryRoot, cutoff: SystemTime, catalog: &mut SessionCatalog) {
    let mut meta_files = Vec::<PathBuf>::new();
    collect_named_files(&root.path, "meta.json", 0, catalog, &mut meta_files);
    for path in meta_files {
        catalog.files_seen += 1;
        let Some(entry) = read_cursor_cli_meta(&path) else {
            catalog.skipped.push(json!({
                "path": path.to_string_lossy(),
                "reason": "cursor_cli_meta_unreadable"
            }));
            continue;
        };
        if entry.recency() >= cutoff {
            catalog.sessions.push(entry);
        }
    }
}

fn read_cursor_cli_meta(path: &Path) -> Option<CatalogSession> {
    let raw = fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<Value>(&raw).ok()?;
    if value.get("hasConversation").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    let native_session_id = path.parent()?.file_name()?.to_str()?.to_string();
    Some(CatalogSession {
        native_session_id,
        source_path: path.to_path_buf(),
        source_kind: "cursor-cli-chats".to_string(),
        title: None,
        created_at: value
            .get("createdAtMs")
            .and_then(Value::as_i64)
            .and_then(epoch_to_system_time),
        updated_at: value
            .get("updatedAtMs")
            .and_then(Value::as_i64)
            .and_then(epoch_to_system_time),
        working_directory: value
            .get("cwd")
            .and_then(Value::as_str)
            .and_then(bounded_project_workspace),
        message_count: None,
        model: None,
        running: false,
        hydrate: CatalogHydration::None,
    })
}

// ---------------------------------------------------------------------------
// Antigravity: one conversation per brain directory, project path in the
// trajectory database beside it.
// ---------------------------------------------------------------------------

const ANTIGRAVITY_BRAIN_DIRECTORY: &str = "brain";
const ANTIGRAVITY_TRAJECTORY_DIRECTORY: &str = "conversations";
const ANTIGRAVITY_TRANSCRIPT: &str = ".system_generated/logs/transcript.jsonl";
/// The trajectory metadata record is a small protobuf; a conversation that grew
/// a large one is not describing a project directory.
const MAX_ANTIGRAVITY_TRAJECTORY_METADATA_BYTES: usize = 64 * 1024;

/// Antigravity keeps one conversation per directory under
/// `~/.gemini/antigravity/brain/<conversationId>/` and its readable transcript at
/// `.system_generated/logs/transcript.jsonl`.
///
/// The same tree also holds the CLI's own rotating logs, crash reports, skill
/// bundles, and knowledge files. Cataloguing files generically turned thousands
/// of log lines into conversations and exhausted the discovery budget before any
/// real conversation was reached, so the browse list held no Antigravity
/// conversation at all and none of them had a project directory.
fn antigravity_catalog(roots: &[HistoryRoot], cutoff: SystemTime, catalog: &mut SessionCatalog) {
    for root in roots {
        if root.source_kind != "antigravity-bridge" {
            // IDE state and CLI logs hold no conversation transcript.
            continue;
        }
        let brain = root.path.join(ANTIGRAVITY_BRAIN_DIRECTORY);
        let Ok(entries) = fs::read_dir(&brain) else {
            continue;
        };
        let trajectories = root.path.join(ANTIGRAVITY_TRAJECTORY_DIRECTORY);
        let mut conversations = Vec::<PathBuf>::new();
        for entry in entries.flatten() {
            catalog.directory_entries_seen += 1;
            let path = entry.path();
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                conversations.push(path);
            }
        }
        conversations.sort();
        for conversation in conversations {
            let Some(conversation_id) = conversation
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
            else {
                continue;
            };
            let transcript = conversation.join(ANTIGRAVITY_TRANSCRIPT);
            let Ok(metadata) = fs::metadata(&transcript) else {
                continue;
            };
            catalog.files_seen += 1;
            let modified_at = metadata.modified().unwrap_or(UNIX_EPOCH);
            if modified_at < cutoff {
                continue;
            }
            catalog.sessions.push(CatalogSession {
                native_session_id: conversation_id.to_string(),
                source_path: transcript.clone(),
                source_kind: root.source_kind.clone(),
                title: None,
                created_at: None,
                updated_at: Some(modified_at),
                working_directory: antigravity_trajectory_workspace(&trajectories, conversation_id),
                message_count: None,
                model: None,
                running: false,
                hydrate: CatalogHydration::File(transcript),
            });
        }
    }
}

/// Project directory of one Antigravity conversation.
///
/// Antigravity records the workspace as a `file://` URI inside the trajectory
/// metadata record of `conversations/<conversationId>.db`. The record is a
/// protobuf with no published schema, so the URI is recovered by scanning the
/// bytes rather than by decoding fields, and only a bounded absolute project
/// directory is accepted.
fn antigravity_trajectory_workspace(trajectories: &Path, conversation_id: &str) -> Option<String> {
    let database = trajectories.join(format!("{conversation_id}.db"));
    if !database.is_file() {
        return None;
    }
    let connection = open_read_only_connection(&database)?;
    if !sqlite_table_exists(&connection, "trajectory_metadata_blob") {
        return None;
    }
    let mut statement = connection
        .prepare("SELECT data FROM trajectory_metadata_blob LIMIT 4")
        .ok()?;
    let rows = statement
        .query_map([], |row| row.get::<_, Vec<u8>>(0))
        .ok()?;
    for record in rows.flatten() {
        if record.len() > MAX_ANTIGRAVITY_TRAJECTORY_METADATA_BYTES {
            continue;
        }
        if let Some(workspace) = first_file_uri_workspace(&record) {
            return Some(workspace);
        }
    }
    None
}

/// First `file:///…` path embedded in an opaque record, as a bounded project
/// directory. Scanning stops at the first byte that cannot belong to a path so a
/// neighbouring protobuf field is never absorbed into it.
fn first_file_uri_workspace(record: &[u8]) -> Option<String> {
    const PREFIX: &[u8] = b"file:///";
    let start = record
        .windows(PREFIX.len())
        .position(|window| window == PREFIX)?
        + PREFIX.len()
        - 1;
    let end = record[start..]
        .iter()
        .position(|byte| {
            !matches!(byte, 0x20..=0x7e) || matches!(byte, b'"' | b'<' | b'>' | b'|' | b'?' | b'*')
        })
        .map(|offset| start + offset)
        .unwrap_or(record.len());
    let candidate = std::str::from_utf8(&record[start..end]).ok()?;
    bounded_project_workspace(candidate)
}

// ---------------------------------------------------------------------------
// Claude Code: one transcript per session; titles from a bounded head probe.
// ---------------------------------------------------------------------------

fn claude_catalog(roots: &[HistoryRoot], cutoff: SystemTime, catalog: &mut SessionCatalog) {
    for root in roots {
        if root.source_kind != "claude-project-transcripts" {
            // `~/.claude.json` is client configuration and prompt history, not a
            // conversation store.
            continue;
        }
        let discovery = discover_history_files(
            HistoryAdapter::ClaudeCode,
            std::slice::from_ref(root),
            HistoryDiscoveryOptions::default(),
        );
        catalog.files_seen += discovery.files_seen;
        catalog.directory_entries_seen += discovery.directory_entries_seen;
        catalog.skipped.extend(discovery.skipped);
        // Claude Code stores each delegated task under
        // `<sessionId>/subagents/agent-<taskId>.jsonl`, so grouping is what keeps
        // sidechain work inside its conversation. Tool results, workflow records,
        // and other per-session artifacts share the tree and are not
        // conversations.
        let candidates = discovery
            .candidates
            .into_iter()
            .filter(|candidate| claude_transcript_candidate(&candidate.path))
            .collect::<Vec<_>>();
        let units = group_delegated_transcripts(candidates);
        let mut grouped = SessionCatalog::default();
        push_delegated_transcript_units(units, None, cutoff, &mut grouped);
        for mut entry in grouped.sessions {
            entry.title = claude_head_title(&entry.source_path);
            catalog.sessions.push(entry);
        }
    }
}

/// Whether one file in the Claude Code project tree is a conversation transcript.
///
/// The tree also holds `tool-results/<id>.txt`, `workflows/<id>.json`, and other
/// per-session artifacts. None of them is a conversation, and listing them fills
/// the browse list with empty rows.
fn claude_transcript_candidate(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(extension.as_str(), "jsonl" | "ndjson") {
        return false;
    }
    if transcript_is_delegated(path) {
        return delegated_file_is_transcript(path);
    }
    !path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .any(|component| matches!(component, "tool-results" | "workflows"))
}

/// Cheap title probe: scan only a bounded head prefix of the transcript for the
/// first summary or user line instead of parsing the session content.
fn claude_head_title(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut head = Vec::new();
    reader
        .by_ref()
        .take(MAX_TITLE_PROBE_BYTES)
        .read_to_end(&mut head)
        .ok()?;
    let head = String::from_utf8_lossy(&head);
    for line in head.lines().take(MAX_TITLE_PROBE_LINES) {
        if line.len() > MAX_TITLE_PROBE_LINE_BYTES {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("summary") => {
                if let Some(summary) = value
                    .get("summary")
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                {
                    return Some(title_from_text(summary));
                }
            }
            Some("user") => {
                let message = value.get("message")?;
                let text = match message.get("content") {
                    Some(Value::String(text)) => Some(text.clone()),
                    Some(Value::Array(blocks)) => blocks.iter().find_map(|block| {
                        (block.get("type").and_then(Value::as_str) == Some("text"))
                            .then(|| block.get("text").and_then(Value::as_str))
                            .flatten()
                            .map(str::to_string)
                    }),
                    _ => None,
                };
                if let Some(text) = text.filter(|value| !value.trim().is_empty()) {
                    return Some(title_from_text(&text));
                }
            }
            _ => {}
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Pi: first-line session header plus file mtime.
// ---------------------------------------------------------------------------

fn pi_catalog(roots: &[HistoryRoot], cutoff: SystemTime, catalog: &mut SessionCatalog) {
    for root in roots {
        let discovery = discover_history_files(
            HistoryAdapter::Pi,
            std::slice::from_ref(root),
            HistoryDiscoveryOptions::default(),
        );
        catalog.files_seen += discovery.files_seen;
        catalog.directory_entries_seen += discovery.directory_entries_seen;
        catalog.skipped.extend(discovery.skipped);
        for candidate in discovery.candidates {
            if candidate.modified_at < cutoff {
                continue;
            }
            let (native_session_id, created_at, working_directory) = pi_header(&candidate.path);
            catalog.sessions.push(CatalogSession {
                native_session_id,
                source_path: candidate.path.clone(),
                source_kind: candidate.source_kind,
                title: None,
                created_at,
                updated_at: Some(candidate.modified_at),
                working_directory,
                message_count: None,
                model: None,
                running: false,
                hydrate: CatalogHydration::File(candidate.path),
            });
        }
    }
}

fn pi_header(path: &Path) -> (String, Option<SystemTime>, Option<String>) {
    let header = super::pi_copilot::pi_session_header(path);
    (
        header.native_session_id,
        header
            .created_at
            .as_deref()
            .and_then(rfc3339_to_system_time),
        header.working_directory,
    )
}

// ---------------------------------------------------------------------------
// Generic file-metadata catalog for adapters without a native index.
// ---------------------------------------------------------------------------

fn generic_file_catalog(
    adapter: HistoryAdapter,
    roots: &[HistoryRoot],
    cutoff: SystemTime,
    catalog: &mut SessionCatalog,
) {
    for root in roots {
        generic_file_root_catalog(adapter, root, cutoff, catalog);
    }
}

fn generic_file_root_catalog(
    adapter: HistoryAdapter,
    root: &HistoryRoot,
    cutoff: SystemTime,
    catalog: &mut SessionCatalog,
) {
    let discovery = discover_history_files(
        adapter,
        std::slice::from_ref(root),
        HistoryDiscoveryOptions::default(),
    );
    catalog.files_seen += discovery.files_seen;
    catalog.directory_entries_seen += discovery.directory_entries_seen;
    catalog.skipped.extend(discovery.skipped);
    for candidate in discovery.candidates {
        push_generic_file_catalog_session(adapter, candidate, cutoff, catalog);
    }
}

fn push_generic_file_catalog_session(
    adapter: HistoryAdapter,
    candidate: HistoryFileCandidate,
    cutoff: SystemTime,
    catalog: &mut SessionCatalog,
) {
    if candidate.modified_at < cutoff {
        return;
    }
    let Ok(metadata) = fs::metadata(&candidate.path) else {
        catalog.skipped.push(json!({
            "path": candidate.path.to_string_lossy(),
            "reason": "catalog_source_metadata_failed"
        }));
        return;
    };
    let scan_config = HistoryScanConfig::from_params(&json!({}));
    let parsed = parse_history_file(
        adapter,
        &candidate.path,
        &candidate.source_kind,
        &metadata,
        scan_config.clone(),
    );
    let parsed = finalize_history_sessions(parsed, &scan_config);
    if parsed.is_empty() {
        catalog.skipped.push(json!({
            "path": candidate.path.to_string_lossy(),
            "reason": "catalog_source_has_no_conversation"
        }));
        return;
    }
    for session in parsed {
        let Some(native_session_id) = session
            .get("nativeSessionId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
        else {
            catalog.skipped.push(json!({
                "path": candidate.path.to_string_lossy(),
                "reason": "catalog_session_identity_missing"
            }));
            continue;
        };
        let timestamp = |key: &str| {
            session
                .get(key)
                .and_then(Value::as_str)
                .and_then(rfc3339_to_system_time)
        };
        catalog.sessions.push(CatalogSession {
            native_session_id,
            source_path: candidate.path.clone(),
            source_kind: candidate.source_kind.clone(),
            title: session
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string),
            created_at: timestamp("createdAt"),
            updated_at: timestamp("updatedAt").or(Some(candidate.modified_at)),
            working_directory: session
                .get("workingDirectory")
                .and_then(Value::as_str)
                .map(str::to_string),
            message_count: session
                .get("messageCount")
                .and_then(Value::as_u64)
                .map(|value| value as usize)
                .or_else(|| {
                    session
                        .get("messages")
                        .and_then(Value::as_array)
                        .map(Vec::len)
                }),
            model: session
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string),
            running: session.get("running").and_then(Value::as_bool) == Some(true),
            hydrate: CatalogHydration::File(candidate.path.clone()),
        });
    }
}

// ---------------------------------------------------------------------------
// Shared helpers.
// ---------------------------------------------------------------------------

fn collect_named_files(
    dir: &Path,
    name: &str,
    _depth: usize,
    catalog: &mut SessionCatalog,
    out: &mut Vec<PathBuf>,
) {
    let mut pending = vec![dir.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        let mut children = Vec::new();
        for entry in entries.flatten() {
            catalog.directory_entries_seen = catalog.directory_entries_seen.saturating_add(1);
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                children.push(path);
            } else if entry.file_name().to_str() == Some(name) {
                out.push(path);
            }
        }
        children.sort_by(|left, right| right.cmp(left));
        pending.extend(children);
    }
}

fn collect_named_files_bounded(
    dir: &Path,
    name: &str,
    depth: usize,
    max_depth: usize,
    catalog: &mut SessionCatalog,
    out: &mut Vec<PathBuf>,
) {
    if depth >= max_depth {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        catalog.directory_entries_seen += 1;
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            collect_named_files_bounded(&path, name, depth + 1, max_depth, catalog, out);
        } else if entry.file_name().to_str() == Some(name) {
            out.push(path);
        }
    }
}

fn sqlite_columns(
    connection: &Connection,
    table: &str,
) -> std::result::Result<HashSet<String>, rusqlite::Error> {
    // Table names are compile-time constants at every call site.
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect()
}

fn rfc3339_to_system_time(value: &str) -> Option<SystemTime> {
    OffsetDateTime::parse(value.trim(), &Rfc3339)
        .ok()
        .map(SystemTime::from)
}

fn epoch_to_system_time(value: i64) -> Option<SystemTime> {
    epoch_number_to_rfc3339(value).and_then(|text| rfc3339_to_system_time(&text))
}
