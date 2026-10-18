//! Metadata-first, change-driven usage aggregation for non-Codex histories.
//!
//! The refresh pipeline is two-phase: discovery and parsing build an
//! immutable typed plan with no database lease held, then one short
//! immediate transaction revalidates identities and applies the planned
//! compaction, rollup, source, watermark, and scan-marker actions with
//! prepared statements.

mod cache;
mod cursor;
mod deepseek;
mod files;
mod identity_refresh;
#[cfg(test)]
mod migration_tests;
mod models;
mod openclaw;
mod parser;
pub(super) mod runtime;
mod snapshot_cursor;
mod watermark;

use crate::domain::conversation::history_discovery::{
    HistoryDiscoveryOptions, discover_history_files,
};
use crate::domain::conversation::parameters::param_bool;
use crate::domain::conversation::source_catalog::{
    HistoryAdapter, history_roots, usage_adapter_for_agent,
};
use anyhow::{Context, Result};
use cache::{
    RefreshStatements, aggregate_usage, cache_has_baseline, cache_is_fresh, cache_path,
    load_compaction_targets, load_sources, migration_counts, reclaim_space,
};
use files::{
    append_guard, append_guard_matches, is_append_format, is_usage_source, roots_fingerprint,
    source_is_closed, source_key, source_metadata, usage_roots,
};
use licoup_statistics::contract::{AgentDef, HistoryUsageSummary};
use licoup_statistics::persistence::client_state_store;
use licoup_statistics::window::UsageWindow;
use models::{
    CachedSource, ParseResult, PlannedSource, PlannedSourceAction, RefreshPlan, ScanStats,
    SourceMetadata,
};
use parser::{parse_append_prefix, parse_append_source, parse_snapshot_source};
use runtime::{CacheLease, CacheRuntime};
use rusqlite::TransactionBehavior;
use serde_json::{Value, json};
use snapshot_cursor::SnapshotCursor;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use watermark::{
    WatermarkProjection, apply_cumulative_watermarks, cursor_covers_old_sessions,
    initialize_cumulative_cursor,
};

pub(super) const PARSER_REVISION: &str = "native-metadata-first-daily-rollups-v11";
const CACHE_REFRESH_INTERVAL_MS: u64 = 60_000;
const SNAPSHOT_CACHE_REFRESH_INTERVAL_MS: u64 = 10 * 60_000;

struct SourceEntry {
    path: PathBuf,
    source_kind: String,
    metadata: SourceMetadata,
}

struct StableParse {
    parsed: ParseResult,
    metadata: SourceMetadata,
    append: bool,
}

pub(super) fn summarize(
    agent: &AgentDef,
    scan_params: &Value,
    window: &UsageWindow,
    warnings: &mut Vec<Value>,
    runtime: &CacheRuntime,
) -> Option<HistoryUsageSummary> {
    if agent.id == "openclaw" {
        return Some(openclaw::summarize(window, warnings));
    }
    // Cursor's billing ledger is hosted: the local stores it writes are not a
    // request ledger, so the file pipeline is never consulted for it.
    if agent.id == "cursor" {
        return Some(cursor::summarize(scan_params, window, warnings));
    }
    match summarize_inner(agent, scan_params, window, warnings, runtime) {
        Ok(summary) => Some(summary),
        Err(error) => {
            warnings.push(if let Some(failure) = error.downcast_ref::<deepseek::ReadFailure>() {
                json!({"code":"native_usage_source_read_failed", "agentId":agent.id, "stage":failure.0})
            } else if let Some(version) = error.downcast_ref::<cache::UnsupportedSchemaVersion>() {
                json!({"code":"native_usage_cache_schema_unsupported", "agentId":agent.id, "schemaVersion":version.0})
            } else { json!({
                "code": "native_usage_cache_failed", "agentId": agent.id
            }) });
            None
        }
    }
}

fn summarize_inner(
    agent: &AgentDef,
    scan_params: &Value,
    window: &UsageWindow,
    warnings: &mut Vec<Value>,
    runtime: &CacheRuntime,
) -> Result<HistoryUsageSummary> {
    let adapter = usage_adapter_for_agent(agent.id)
        .with_context(|| format!("unsupported usage adapter: {}", agent.id))?;
    let roots = usage_roots(adapter, history_roots(adapter, scan_params));
    let root_paths = roots
        .iter()
        .map(|root| root.path.clone())
        .collect::<Vec<_>>();
    let scope_key = roots_fingerprint(agent.id, &root_paths, &window.cache_timezone_key());
    let state_store = client_state_store(scan_params)?;
    let database_path = cache_path(state_store.root());
    let force_refresh = param_bool(scan_params, "forceRefresh").unwrap_or(false);
    let now_ms = unix_millis();
    let mut stats = ScanStats::default();
    let refresh_scope = runtime.begin_refresh(&scope_key, &database_path, now_ms)?;
    stats.opened_connections = refresh_scope.opened_connections();

    let (has_baseline, previous, compaction_targets, identity_targets) = {
        let mut lease = runtime.lease(&scope_key, &database_path, now_ms)?;
        stats.leases = stats.leases.saturating_add(lease.stats().leases);
        stats.opened_connections = stats
            .opened_connections
            .saturating_add(lease.stats().opened);
        if !force_refresh
            && cache_is_fresh(
                lease.connection(0),
                &scope_key,
                now_ms,
                cache_refresh_interval_ms(adapter),
            )?
        {
            let mut summary = aggregate_usage(lease.connection(1), &scope_key, window)?;
            report_migration_status(lease.connection(0), &scope_key, agent.id, warnings)?;
            apply_source(adapter, &mut summary);
            summary.scan_cache = Some(
                ScanStats {
                    cache_fresh: true,
                    opened_connections: stats.opened_connections,
                    leases: stats.leases,
                    ..ScanStats::default()
                }
                .to_json(),
            );
            return Ok(summary);
        }
        let has_baseline = cache_has_baseline(lease.connection(0), &scope_key)?;
        let previous = load_sources(lease.connection(0), &scope_key)?;
        let compaction_targets =
            load_compaction_targets(lease.connection(0), &scope_key, &window.end)?;
        let identity_targets = if force_refresh {
            identity_refresh::targets(lease.connection(0), &scope_key, &window.end)?
        } else {
            BTreeSet::new()
        };
        (has_baseline, previous, compaction_targets, identity_targets)
    };
    // The lease is released here: discovery, guard hashing, and parsing run
    // without holding any database connection.

    let parse_window = if has_baseline {
        window.today_only()
    } else {
        window.all_history()
    };

    let discovery = discover_history_files(adapter, &roots, HistoryDiscoveryOptions::default());
    for skipped in discovery
        .skipped
        .iter()
        .filter(|item| item.get("reason").and_then(Value::as_str) != Some("not_present"))
    {
        warnings.push(json!({
            "code": skipped
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("native_usage_source_skipped"),
            "agentId": agent.id
        }));
    }
    let unique_candidates = discovery
        .candidates
        .into_iter()
        .filter(|candidate| is_usage_source(adapter, &candidate.path, &candidate.source_kind))
        .fold(
            BTreeMap::<PathBuf, String>::new(),
            |mut unique, candidate| {
                unique
                    .entry(candidate.path)
                    .or_insert(candidate.source_kind);
                unique
            },
        );
    let unique_candidates = if adapter == HistoryAdapter::DeepSeekHarness {
        deepseek::canonical_sources(unique_candidates)
    } else {
        unique_candidates
    };
    let entries = unique_candidates
        .into_iter()
        .filter_map(|(path, source_kind)| {
            let metadata = source_metadata(&path)?;
            Some(SourceEntry {
                metadata,
                path,
                source_kind,
            })
        })
        .collect::<Vec<_>>();
    stats.discovered_sources = entries.len() as u64;

    // Day rollover is independent of source discovery. The plan replicates
    // the post-compaction session state so unchanged-source decisions match
    // the single-transaction behavior exactly.
    let mut planned_sources = previous.clone();
    for (key, source) in &mut planned_sources {
        if compaction_targets.contains(key) {
            source.session_count = 0;
        }
    }
    let compactions = previous
        .iter()
        .map(|(key, source)| (key.clone(), source.session_count))
        .collect::<Vec<_>>();

    let mut sources = Vec::new();
    let mut deepseek_reader = deepseek::Reader::default();
    for entry in entries {
        let identity_path = if adapter == HistoryAdapter::DeepSeekHarness {
            entry.path.parent().unwrap_or(&entry.path)
        } else {
            &entry.path
        };
        let key = source_key(&scope_key, identity_path);
        let previous_source = planned_sources.get(&key).cloned();
        let (action, metadata) = if let Some(previous) = &previous_source
            && (previous.migration_state == 1
                || (matches!(previous.migration_state, 2 | 3)
                    && (!source_unchanged(previous, &entry.metadata)
                        || (force_refresh
                            && !append_guard_matches(
                                &entry.path,
                                previous.size,
                                &previous.append_guard,
                            )))
                    && !can_append(adapter, &entry.path, previous, &entry.metadata))
                || (matches!(previous.migration_state, 4 | 5)
                    && previous.file_id != entry.metadata.file_id))
        {
            let migration = plan_source_migration(
                adapter,
                &entry.path,
                &entry.source_kind,
                previous,
                &parse_window,
                &mut deepseek_reader,
            )?;
            stats.replaced_sources = stats.replaced_sources.saturating_add(1);
            stats.parsed_bytes = stats.parsed_bytes.saturating_add(migration.1.size);
            migration
        } else if let Some(previous) = &previous_source
            && identity_targets.contains(&key)
            && source_unchanged(previous, &entry.metadata)
            && (!is_append_source(adapter, &entry.path)
                || append_guard_matches(&entry.path, previous.size, &previous.append_guard))
        {
            let parsed = if !previous.sealed
                && previous.migration_state == 0
                && previous.file_id.is_some()
                && previous.parsed_bytes <= previous.size
            {
                let parsed = if is_append_source(adapter, &entry.path) {
                    parse_append_prefix(adapter, &entry.path, previous.parsed_bytes, &parse_window)
                } else {
                    parse_stable_source(
                        adapter,
                        &entry.path,
                        &entry.source_kind,
                        None,
                        0,
                        &parse_window,
                        &mut deepseek_reader,
                    )
                    .map(|stable| stable.parsed)
                };
                parsed.ok().and_then(|parsed| {
                    stats.parsed_bytes = stats.parsed_bytes.saturating_add(parsed.parsed_bytes);
                    (parsed.parsed_bytes == previous.parsed_bytes
                        && parsed.cumulative_snapshots.is_empty()
                        && (!is_append_source(adapter, &entry.path)
                            || append_guard_matches(
                                &entry.path,
                                previous.size,
                                &previous.append_guard,
                            )))
                    .then(|| Box::new(parsed))
                })
            } else {
                None
            };
            (
                PlannedSourceAction::RefreshIdentity { parsed },
                entry.metadata.clone(),
            )
        } else if let Some(previous) = &previous_source
            && source_unchanged(previous, &entry.metadata)
            && (!force_refresh
                || (is_append_source(adapter, &entry.path)
                    && append_guard_matches(&entry.path, previous.size, &previous.append_guard)))
        {
            if !previous.sealed && source_is_closed(&entry.metadata, window) {
                stats.sealed_sources = stats.sealed_sources.saturating_add(1);
                (
                    PlannedSourceAction::ReuseSeal {
                        session_count: previous.session_count,
                    },
                    entry.metadata.clone(),
                )
            } else {
                stats.reused_sources = stats.reused_sources.saturating_add(1);
                (PlannedSourceAction::Reuse, entry.metadata.clone())
            }
        } else {
            let append_format = is_append_source(adapter, &entry.path);
            let previous_session_count = previous_source
                .as_ref()
                .map_or(0, |source| source.session_count);
            let stable = parse_stable_source(
                adapter,
                &entry.path,
                &entry.source_kind,
                previous_source.as_ref(),
                previous_session_count,
                &parse_window,
                &mut deepseek_reader,
            )?;
            let projection = if stable.append {
                let previous = previous_source
                    .as_ref()
                    .expect("stable append requires a previous source");
                stats.appended_sources = stats.appended_sources.saturating_add(1);
                stats.parsed_bytes = stats.parsed_bytes.saturating_add(
                    stable
                        .parsed
                        .parsed_bytes
                        .saturating_sub(previous.parsed_bytes),
                );
                WatermarkProjection::AppendDelta
            } else {
                stats.replaced_sources = stats.replaced_sources.saturating_add(1);
                stats.parsed_bytes = stats
                    .parsed_bytes
                    .saturating_add(stable.parsed.parsed_bytes);
                if has_baseline {
                    WatermarkProjection::ReplaceCurrentDay
                } else {
                    WatermarkProjection::RebuildAllHistory
                }
            };
            let append_guard = if append_format {
                append_guard(&entry.path, stable.metadata.size)?
            } else {
                String::new()
            };
            (
                PlannedSourceAction::Refresh {
                    append: stable.append,
                    append_format,
                    previous_session_count,
                    parsed: Box::new(stable.parsed),
                    projection,
                    append_guard,
                },
                stable.metadata,
            )
        };
        sources.push(PlannedSource {
            key,
            path: entry.path,
            metadata,
            action,
        });
    }

    let plan = RefreshPlan {
        previous,
        compaction_targets,
        compactions,
        sources,
    };
    {
        let mut lease = runtime.lease(&scope_key, &database_path, now_ms)?;
        stats.leases = stats.leases.saturating_add(lease.stats().leases);
        stats.opened_connections = stats
            .opened_connections
            .saturating_add(lease.stats().opened);
        apply_refresh_plan(
            &mut lease,
            &scope_key,
            &parse_window,
            window,
            now_ms,
            plan,
            &mut stats,
        )?;
    }
    // The apply lease is released before aggregation.

    let mut summary = {
        let mut lease = runtime.lease(&scope_key, &database_path, now_ms)?;
        stats.leases = stats.leases.saturating_add(lease.stats().leases);
        stats.opened_connections = stats
            .opened_connections
            .saturating_add(lease.stats().opened);
        report_migration_status(lease.connection(0), &scope_key, agent.id, warnings)?;
        aggregate_usage(lease.connection(1), &scope_key, window)?
    };
    apply_source(adapter, &mut summary);
    summary.scan_cache = Some(stats.to_json());
    Ok(summary)
}

fn apply_refresh_plan(
    lease: &mut CacheLease<'_>,
    scope_key: &str,
    parse_window: &UsageWindow,
    window: &UsageWindow,
    now_ms: u64,
    plan: RefreshPlan,
    stats: &mut ScanStats,
) -> Result<()> {
    let started = Instant::now();
    let connection = lease.connection(0);
    let needs_reclaim = {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("native usage cache transaction failed")?;
        stats.statements = stats.statements.saturating_add(1);
        let current = load_sources(&transaction, scope_key)?;
        if current != plan.previous {
            anyhow::bail!("native usage cache changed during scan");
        }
        stats.statements = stats.statements.saturating_add(1);
        let targets = load_compaction_targets(&transaction, scope_key, &window.end)?;
        if targets != plan.compaction_targets {
            anyhow::bail!("native usage cache changed during scan");
        }
        for source in &plan.sources {
            let observed = source_metadata(&source.path).with_context(|| {
                format!(
                    "native usage source recheck failed: {}",
                    source.path.display()
                )
            })?;
            if observed != source.metadata {
                anyhow::bail!("native usage source changed during scan");
            }
        }
        let mut statements = RefreshStatements::prepare(&transaction)?;
        for (key, session_count) in &plan.compactions {
            let compacted = statements.compact(scope_key, key, &window.end, *session_count)?;
            stats.compacted_days = stats.compacted_days.saturating_add(compacted);
        }
        for source in plan.sources {
            match source.action {
                PlannedSourceAction::Migrate {
                    baseline,
                    tail,
                    complete,
                    append_format,
                    append_guard,
                } => {
                    let mut previous = plan.previous[&source.key].clone();
                    if plan.compaction_targets.contains(&source.key) {
                        previous.session_count = 0;
                    }
                    apply_source_migration(
                        &mut statements,
                        scope_key,
                        &source.key,
                        &source.metadata,
                        &previous,
                        *baseline,
                        tail.map(|parsed| *parsed),
                        complete,
                        append_format,
                        &append_guard,
                        parse_window,
                    )?;
                }
                PlannedSourceAction::Reuse => {}
                PlannedSourceAction::RefreshIdentity { parsed } => {
                    let refreshed = match parsed {
                        Some(parsed) => identity_refresh::apply(
                            &mut statements,
                            scope_key,
                            &source.key,
                            &window.end,
                            &parsed.summary,
                        )?,
                        None => false,
                    };
                    if refreshed {
                        stats.identity_refreshed_sources += 1;
                    } else {
                        stats.identity_skipped_sources += 1;
                    }
                }
                PlannedSourceAction::ReuseSeal { session_count } => {
                    statements.seal(scope_key, &source.key, session_count)?;
                }
                PlannedSourceAction::Refresh {
                    append,
                    append_format,
                    previous_session_count,
                    mut parsed,
                    projection,
                    append_guard,
                } => {
                    let previous = plan.previous.get(&source.key);
                    let snapshot_mode =
                        previous.is_some_and(|source| matches!(source.migration_state, 4 | 5));
                    let mut snapshot_gap = false;
                    let snapshot_cursor = if snapshot_mode {
                        let cursor = snapshot_cursor_for(&parsed, parse_window);
                        let old = previous
                            .and_then(|source| source.snapshot_cursor.as_ref())
                            .context("native snapshot migration cursor missing")?;
                        (parsed.summary, snapshot_gap) = cursor.delta(old, parse_window);
                        Some(cursor)
                    } else {
                        None
                    };
                    let snapshots = std::mem::take(&mut parsed.cumulative_snapshots);
                    let watermarked_sessions = if snapshot_gap {
                        initialize_cumulative_cursor(
                            &mut statements,
                            scope_key,
                            &source.key,
                            &snapshots,
                        )?;
                        0
                    } else {
                        apply_cumulative_watermarks(
                            &mut statements,
                            scope_key,
                            &source.key,
                            parse_window,
                            &snapshots,
                            if snapshot_mode {
                                WatermarkProjection::AppendDelta
                            } else {
                                projection
                            },
                            &mut parsed.summary,
                        )?
                    };
                    if !snapshot_mode {
                        parsed.summary.session_count = parsed
                            .summary
                            .session_count
                            .saturating_add(watermarked_sessions);
                    }
                    let session_count = if previous
                        .is_some_and(|source| has_sealed_session_today(source, parse_window))
                    {
                        previous_session_count
                    } else if snapshot_mode {
                        previous_session_count.saturating_add(parsed.summary.session_count)
                    } else if append_format {
                        if append {
                            previous_session_count.saturating_add(if previous_session_count == 0 {
                                parsed.summary.session_count
                            } else {
                                0
                            })
                        } else {
                            previous_session_count.max(parsed.summary.session_count)
                        }
                    } else {
                        parsed.summary.session_count
                    };
                    if append || snapshot_mode {
                        statements.add_rollup(scope_key, &source.key, &parsed.summary)?;
                    } else {
                        statements.replace_rollup(scope_key, &source.key, &parsed.summary)?;
                    }
                    statements.save_source(
                        scope_key,
                        &source.key,
                        &source.metadata,
                        parsed.parsed_bytes,
                        &append_guard,
                        session_count,
                        &parsed.request_context,
                    )?;
                    if let Some(cursor) = snapshot_cursor {
                        statements.save_snapshot_cursor(scope_key, &source.key, &cursor)?;
                        if snapshot_gap {
                            statements.save_migration_state(scope_key, &source.key, 5)?;
                        }
                    }
                    let compacted =
                        statements.compact(scope_key, &source.key, &window.end, session_count)?;
                    stats.compacted_days = stats.compacted_days.saturating_add(compacted);
                    if source_is_closed(&source.metadata, window) {
                        statements.seal(
                            scope_key,
                            &source.key,
                            if compacted > 0 { 0 } else { session_count },
                        )?;
                        stats.sealed_sources = stats.sealed_sources.saturating_add(1);
                    }
                }
            }
        }
        statements.mark_scan(scope_key, now_ms)?;
        let executed = statements.executed();
        let sealed_sources = stats.sealed_sources;
        let compacted_days = stats.compacted_days;
        let rebuilt = stats.rebuilt;
        drop(statements);
        transaction.commit()?;
        stats.statements = stats.statements.saturating_add(executed);
        stats.transaction_millis = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        sealed_sources > 0 || compacted_days > 0 || rebuilt
    };
    if needs_reclaim {
        reclaim_space(connection)?;
    }
    Ok(())
}

/// Parses a source with metadata verification before and after the read.
/// One change retries the parse; a second change fails the whole scan so no
/// cache row or watermark can partially apply.
fn parse_stable_source(
    adapter: HistoryAdapter,
    path: &Path,
    source_kind: &str,
    previous_source: Option<&CachedSource>,
    previous_session_count: u64,
    parse_window: &UsageWindow,
    deepseek_reader: &mut deepseek::Reader,
) -> Result<StableParse> {
    let append_format = is_append_source(adapter, path);
    for _ in 0..2 {
        let before = source_metadata(path)
            .with_context(|| format!("native usage source metadata failed: {}", path.display()))?;
        // Recompute append eligibility for the exact snapshot about to be
        // parsed. A replacement between discovery and either retry must fall
        // back to a full parse instead of applying an obsolete append plan.
        let append =
            previous_source.is_some_and(|previous| can_append(adapter, path, previous, &before));
        let parsed = if adapter == HistoryAdapter::DeepSeekHarness {
            deepseek_reader.parse(path, before.size, parse_window)?
        } else if append {
            let previous = previous_source.expect("append source checked above");
            parse_append_source(
                adapter,
                path,
                previous.parsed_bytes,
                parse_window,
                previous_session_count > 0 || has_sealed_session_today(previous, parse_window),
                previous.request_context.clone(),
            )?
        } else if append_format {
            parse_append_source(
                adapter,
                path,
                0,
                parse_window,
                previous_session_count > 0,
                Default::default(),
            )?
        } else {
            let metadata = fs::metadata(path)
                .with_context(|| format!("native usage source stat failed: {}", path.display()))?;
            parse_snapshot_source(adapter, path, source_kind, &metadata, parse_window)?
        };
        let after = source_metadata(path)
            .with_context(|| format!("native usage source recheck failed: {}", path.display()))?;
        if after == before {
            return Ok(StableParse {
                parsed,
                metadata: after,
                append,
            });
        }
    }
    anyhow::bail!(
        "native usage source changed while parsing: {}",
        path.display()
    )
}

fn is_append_source(adapter: HistoryAdapter, path: &Path) -> bool {
    adapter != HistoryAdapter::DeepSeekHarness && is_append_format(path)
}

fn can_append(
    adapter: HistoryAdapter,
    path: &Path,
    previous: &CachedSource,
    metadata: &SourceMetadata,
) -> bool {
    is_append_source(adapter, path)
        && previous.file_id.is_some()
        && previous.file_id == metadata.file_id
        && metadata.size > previous.size
        && previous.parsed_bytes <= previous.size
        && append_guard_matches(path, previous.size, &previous.append_guard)
}

fn has_sealed_session_today(source: &CachedSource, calendar: &UsageWindow) -> bool {
    matches!(source.migration_state, 2 | 3)
        && source
            .snapshot_cursor
            .as_ref()
            .is_some_and(|cursor| cursor.day == calendar.end && cursor.session_count > 0)
}

fn plan_source_migration(
    adapter: HistoryAdapter,
    path: &Path,
    source_kind: &str,
    previous: &CachedSource,
    calendar: &UsageWindow,
    deepseek_reader: &mut deepseek::Reader,
) -> Result<(PlannedSourceAction, SourceMetadata)> {
    let append_format = is_append_source(adapter, path);
    for _ in 0..2 {
        let before =
            source_metadata(path).context("native usage migration source metadata failed")?;
        let same_file = previous.file_id.is_some() && previous.file_id == before.file_id;
        let prefix = if append_format
            && same_file
            && previous.parsed_bytes <= previous.size
            && before.size >= previous.size
            && append_guard_matches(path, previous.size, &previous.append_guard)
        {
            parse_append_prefix(
                adapter,
                path,
                previous.parsed_bytes,
                &calendar.all_history(),
            )
            .ok()
        } else {
            None
        };
        let complete = if append_format {
            prefix.is_some()
        } else {
            same_file && source_unchanged(previous, &before)
        };
        let (baseline, tail) = if let Some(prefix) = prefix {
            let tail = parse_append_source(
                adapter,
                path,
                previous.parsed_bytes,
                calendar,
                previous.session_count > 0 || has_sealed_session_today(previous, calendar),
                prefix.request_context.clone(),
            )?;
            (prefix, Some(Box::new(tail)))
        } else if append_format {
            (
                parse_append_source(
                    adapter,
                    path,
                    0,
                    &calendar.all_history(),
                    true,
                    Default::default(),
                )?,
                None,
            )
        } else {
            let metadata = fs::metadata(path)?;
            (
                if adapter == HistoryAdapter::DeepSeekHarness {
                    deepseek_reader.parse(path, metadata.len(), calendar)?
                } else {
                    parse_snapshot_source(adapter, path, source_kind, &metadata, calendar)?
                },
                None,
            )
        };
        let guard = if append_format {
            append_guard(path, before.size)?
        } else {
            String::new()
        };
        if source_metadata(path).as_ref() == Some(&before) {
            return Ok((
                PlannedSourceAction::Migrate {
                    baseline: Box::new(baseline),
                    tail,
                    complete,
                    append_format,
                    append_guard: guard,
                },
                before,
            ));
        }
    }
    anyhow::bail!("native usage source changed while migrating")
}

fn apply_source_migration(
    statements: &mut RefreshStatements<'_>,
    scope_key: &str,
    source_key: &str,
    metadata: &SourceMetadata,
    previous: &CachedSource,
    mut baseline: ParseResult,
    mut tail: Option<ParseResult>,
    complete: bool,
    append_format: bool,
    append_guard: &str,
    calendar: &UsageWindow,
) -> Result<()> {
    let complete = complete
        && cursor_covers_old_sessions(
            statements,
            scope_key,
            source_key,
            &baseline.cumulative_snapshots,
        )?;
    if !previous.sealed {
        statements.seal(scope_key, source_key, previous.session_count)?;
    }
    if !complete && let Some(tail) = tail.take() {
        baseline
            .cumulative_snapshots
            .extend(tail.cumulative_snapshots);
        baseline.parsed_bytes = tail.parsed_bytes;
        baseline.request_context = tail.request_context;
    }
    initialize_cumulative_cursor(
        statements,
        scope_key,
        source_key,
        &baseline.cumulative_snapshots,
    )?;
    let snapshot_cursor = if append_format {
        SnapshotCursor::capture(
            &HistoryUsageSummary {
                session_count: previous.session_count,
                ..Default::default()
            },
            calendar,
        )
    } else {
        snapshot_cursor_for(&baseline, calendar)
    };
    let mut session_count = 0;
    if let Some(mut tail) = tail {
        let watermarked_sessions = apply_cumulative_watermarks(
            statements,
            scope_key,
            source_key,
            calendar,
            &tail.cumulative_snapshots,
            WatermarkProjection::AppendDelta,
            &mut tail.summary,
        )?;
        if previous.session_count == 0 && !has_sealed_session_today(previous, calendar) {
            session_count = tail
                .summary
                .session_count
                .saturating_add(watermarked_sessions);
        }
        statements.add_rollup(scope_key, source_key, &tail.summary)?;
        baseline.parsed_bytes = tail.parsed_bytes;
        baseline.request_context = tail.request_context;
    }
    statements.save_source(
        scope_key,
        source_key,
        metadata,
        baseline.parsed_bytes,
        append_guard,
        session_count,
        &baseline.request_context,
    )?;
    statements.save_snapshot_cursor(scope_key, source_key, &snapshot_cursor)?;
    let gap = !complete || matches!(previous.migration_state, 3 | 5);
    statements.save_migration_state(
        scope_key,
        source_key,
        if append_format {
            if gap { 3 } else { 2 }
        } else if gap {
            5
        } else {
            4
        },
    )?;
    if source_is_closed(metadata, calendar) {
        statements.seal(scope_key, source_key, session_count)?;
    }
    Ok(())
}

fn snapshot_cursor_for(parsed: &ParseResult, calendar: &UsageWindow) -> SnapshotCursor {
    let mut cursor = SnapshotCursor::capture(&parsed.summary, calendar);
    let cumulative_sessions = parsed
        .cumulative_snapshots
        .iter()
        .filter(|snapshot| snapshot.projects_usage && calendar.contains(&snapshot.observed_day))
        .map(|snapshot| snapshot.session_key.as_str())
        .collect::<BTreeSet<_>>();
    cursor.session_count = cursor
        .session_count
        .saturating_add(cumulative_sessions.len() as u64);
    cursor
}

fn report_migration_status(
    connection: &rusqlite::Connection,
    scope_key: &str,
    agent_id: &str,
    warnings: &mut Vec<Value>,
) -> Result<()> {
    let (pending, gaps) = migration_counts(connection, scope_key)?;
    if pending > 0 || gaps > 0 {
        warnings.push(
            json!({"code":"native_usage_source_migration_incomplete", "agentId":agent_id,
            "pendingSources":pending, "gapSources":gaps}),
        );
    }
    Ok(())
}

fn apply_source(adapter: HistoryAdapter, summary: &mut HistoryUsageSummary) {
    if adapter == HistoryAdapter::DeepSeekHarness {
        summary.source = Some("deepseek-harness-session-usage");
    }
    if adapter == HistoryAdapter::Hermes {
        summary.source = Some("hermes-gateway-usage-database");
    }
}

fn source_unchanged(cached: &CachedSource, metadata: &SourceMetadata) -> bool {
    cached.modified_ns == metadata.modified_ns
        && cached.size == metadata.size
        && cached.file_id == metadata.file_id
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn cache_refresh_interval_ms(adapter: HistoryAdapter) -> u64 {
    if matches!(
        adapter,
        HistoryAdapter::Antigravity | HistoryAdapter::Copilot | HistoryAdapter::Cursor
    ) {
        SNAPSHOT_CACHE_REFRESH_INTERVAL_MS
    } else {
        CACHE_REFRESH_INTERVAL_MS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn native_append_recovers_cached_request_options_without_recounting_tokens() {
        use std::io::Write;
        let root =
            std::env::temp_dir().join(format!("lico-usage-context-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let path = root.join("synthetic.jsonl");
        fs::write(
            &path,
            b"{\"type\":\"request\",\"model\":\"raw-model\",\"effort\":\"high\",\"fast\":false}\n",
        )
        .unwrap();
        let calendar =
            UsageWindow::from_params(&json!({"now":"2026-07-15T12:00:00Z", "historyDays":1}));
        let initial = parse_stable_source(
            HistoryAdapter::ClaudeCode,
            &path,
            "jsonl",
            None,
            0,
            &calendar,
            &mut deepseek::Reader::default(),
        )
        .unwrap();
        let cached = |parsed: &StableParse| CachedSource {
            modified_ns: parsed.metadata.modified_ns,
            size: parsed.metadata.size,
            file_id: parsed.metadata.file_id.clone(),
            parsed_bytes: parsed.parsed.parsed_bytes,
            append_guard: append_guard(&path, parsed.metadata.size).unwrap(),
            request_context: parsed.parsed.request_context.clone(),
            ..Default::default()
        };
        let previous = cached(&initial);
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "{}", json!({"type":"assistant", "timestamp":"2026-07-15T10:00:00Z", "message":{"model":"raw-model", "usage":{"input_tokens":10,"output_tokens":2}}})).unwrap();
        let appended = parse_stable_source(
            HistoryAdapter::ClaudeCode,
            &path,
            "jsonl",
            Some(&previous),
            0,
            &calendar,
            &mut deepseek::Reader::default(),
        )
        .unwrap();
        assert!(appended.append);
        assert_eq!(appended.parsed.summary.total_tokens(), 12);
        let known = licoup_statistics::contract::UsageVariant {
            effort: Some("high".to_owned()),
            fast: Some(false),
        };
        assert_eq!(
            appended.parsed.summary.daily_usage["2026-07-15"].model_variants
                [&("raw-model".to_owned(), known)]
                .total_tokens,
            12
        );
        let previous = cached(&appended);
        writeln!(file, "{}", json!({"type":"request","model":"raw-model"})).unwrap();
        writeln!(file, "{}", json!({"type":"assistant", "timestamp":"2026-07-15T11:00:00Z", "message":{"model":"raw-model", "usage":{"input_tokens":5,"output_tokens":1}}})).unwrap();
        let next = parse_stable_source(
            HistoryAdapter::ClaudeCode,
            &path,
            "jsonl",
            Some(&previous),
            1,
            &calendar,
            &mut deepseek::Reader::default(),
        )
        .unwrap();
        assert!(next.append);
        assert_eq!(next.parsed.summary.total_tokens(), 6);
        assert_eq!(
            next.parsed.summary.daily_usage["2026-07-15"].model_variants
                [&("raw-model".to_owned(), Default::default())]
                .total_tokens,
            6
        );
        drop(file);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn snapshot_heavy_adapters_are_throttled_without_affecting_append_adapters() {
        assert_eq!(
            cache_refresh_interval_ms(HistoryAdapter::Antigravity),
            10 * CACHE_REFRESH_INTERVAL_MS
        );
        assert_eq!(
            cache_refresh_interval_ms(HistoryAdapter::Copilot),
            10 * CACHE_REFRESH_INTERVAL_MS
        );
        assert_eq!(
            cache_refresh_interval_ms(HistoryAdapter::Cursor),
            10 * CACHE_REFRESH_INTERVAL_MS
        );
        assert_eq!(
            cache_refresh_interval_ms(HistoryAdapter::Pi),
            CACHE_REFRESH_INTERVAL_MS
        );
    }
}
