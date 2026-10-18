use super::aggregation::aggregate_cached_usage;
use super::append_guard::{
    append_guard, append_guard_matches, content_guard_digest, content_guard_state,
    extend_content_guard,
};
use super::cache::{
    cache_is_fresh, cache_state, cached_source_keys, open_cache_database, sqlite_is_busy,
};
use super::cache_batch::CacheBatch;
use super::cache_cleanup::reclaim_cache_space;
use super::constants::CACHE_DATABASE_PREFIX;
use super::file_collection::{collect_usage_files, file_metadata};
use super::lineage::reconcile_lineage_scopes;
use super::models::{ParserState, ScanStats};
use super::parser::ParserBatch;
use super::rollup::compact_historical_details;
use super::scan_params::{bool_param, roots_fingerprint, source_key, usage_roots};
use super::utils::{to_i64, unix_millis};
use anyhow::{Context, Result};
use licoup_statistics::contract::HistoryUsageSummary;
use licoup_statistics::persistence::client_state_store;
use licoup_statistics::window::UsageWindow;
use rusqlite::{TransactionBehavior, params};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::time::Duration;

pub(super) fn summarize(
    scan_params: &Value,
    window: &UsageWindow,
    warnings: &mut Vec<Value>,
) -> Option<HistoryUsageSummary> {
    match summarize_inner(scan_params, window) {
        Ok(summary) if summary.explicit_records > 0 => Some(summary),
        Ok(_) => None,
        Err(_) => {
            warnings.push(json!({
                "code": "codex_local_token_event_scan_failed",
                "agentId": "codex"
            }));
            None
        }
    }
}

fn summarize_inner(scan_params: &Value, window: &UsageWindow) -> Result<HistoryUsageSummary> {
    let roots = usage_roots(scan_params);
    if roots.is_empty() {
        return Ok(HistoryUsageSummary::default());
    }
    let root_key = roots_fingerprint(&roots, &window.cache_timezone_key());
    let store = client_state_store(scan_params)?;
    let database_path = store
        .root()
        .join(format!("{CACHE_DATABASE_PREFIX}-{root_key}.sqlite3"));
    let mut connection = open_cache_database(&database_path)?;
    let force_refresh = bool_param(scan_params, "forceRefresh").unwrap_or(false);
    let now_ms = unix_millis();
    if !force_refresh && cache_is_fresh(&connection, &root_key, now_ms)? {
        let stats = ScanStats {
            cache_fresh: true,
            ..ScanStats::default()
        };
        return aggregate_cached_usage(&mut connection, &root_key, window, stats);
    }

    let files = collect_usage_files(&roots);
    let mut stats = ScanStats {
        discovered_files: files.len() as u64,
        ..ScanStats::default()
    };
    let (has_cached_snapshot, has_baseline) = cache_state(&connection, &root_key)?;
    let rollup_window = window.all_history();
    let parse_window = if has_baseline {
        window.today_only()
    } else {
        window.all_history()
    };
    connection.busy_timeout(if has_cached_snapshot {
        Duration::from_millis(500)
    } else {
        Duration::from_secs(30)
    })?;
    let transaction_result = connection.transaction_with_behavior(TransactionBehavior::Immediate);
    let refresh_deferred = matches!(
        &transaction_result,
        Err(error) if has_cached_snapshot && sqlite_is_busy(error)
    );
    if refresh_deferred {
        drop(transaction_result);
        stats.refresh_deferred = true;
        return aggregate_cached_usage(&mut connection, &root_key, window, stats);
    }
    let transaction = transaction_result.context("agent usage cache transaction failed")?;
    // Finalize previous local days before a rewrite or schema migration can
    // replace today's rows. Preserved scan markers keep archived days sealed.
    let mut deleted_rows = compact_historical_details(&transaction, &root_key, &rollup_window)?;
    let cached_keys = cached_source_keys(&transaction, &root_key)?;
    let mut seen_source_keys = BTreeSet::<String>::new();
    {
        let mut cache_batch = CacheBatch::new(&transaction)?;
        let mut parser_batch = ParserBatch::new(&transaction)?;
        for path in files {
            let Some(metadata) = file_metadata(&path) else {
                continue;
            };
            let source_key = source_key(&root_key, &path);
            seen_source_keys.insert(source_key.clone());
            let cached = cache_batch.load(&root_key, &source_key)?;
            if let Some(cached) = &cached
                && cached.modified_ns == metadata.modified_ns
                && cached.size == metadata.size
                && cached.file_id == metadata.file_id
                && (!force_refresh || append_guard_matches(&path, cached))
            {
                stats.reused_files += 1;
                if force_refresh
                    && super::identity_refresh::needed(
                        &transaction,
                        &root_key,
                        &source_key,
                        &window.end,
                    )?
                {
                    if super::identity_refresh::apply(
                        &transaction,
                        &root_key,
                        &source_key,
                        &path,
                        cached,
                        &parse_window,
                    )? {
                        stats.identity_refreshed_files += 1;
                    } else {
                        stats.identity_skipped_files += 1;
                    }
                }
                continue;
            }

            let append_state = cached.as_ref().and_then(|cached| {
                if cached.file_id.is_none()
                    || cached.file_id != metadata.file_id
                    || metadata.size <= cached.size
                    || cached.parsed_bytes > cached.size
                {
                    return None;
                }
                let guard_state = content_guard_state(&path, cached.size).ok()?;
                (content_guard_digest(&guard_state) == cached.append_guard)
                    .then_some((guard_state, cached.size))
            });
            let (start_offset, mut state, append_state) = if let Some(append_state) = append_state {
                stats.appended_files += 1;
                let cached = cached.expect("append cache checked above");
                (cached.parsed_bytes, cached.state, Some(append_state))
            } else {
                stats.rescanned_files += 1;
                cache_batch.reset_parsed_source(&root_key, &source_key)?;
                (0, ParserState::default(), None)
            };

            let parsed_bytes = parser_batch.parse_file(
                &root_key,
                &source_key,
                &path,
                start_offset,
                &parse_window,
                &mut state,
            )?;
            stats.parsed_bytes = stats
                .parsed_bytes
                .saturating_add(parsed_bytes.saturating_sub(start_offset));
            let append_guard = if let Some((mut guard_state, guarded_bytes)) = append_state {
                extend_content_guard(&path, guarded_bytes, metadata.size, &mut guard_state)?;
                content_guard_digest(&guard_state)
            } else {
                append_guard(&path, metadata.size)?
            };
            cache_batch.save(
                &root_key,
                &source_key,
                &metadata,
                parsed_bytes,
                &append_guard,
                &state,
            )?;
        }

        for source_key in cached_keys.difference(&seen_source_keys) {
            cache_batch.delete_source(&root_key, source_key)?;
        }
    }
    reconcile_lineage_scopes(&transaction, &root_key)?;
    deleted_rows = deleted_rows.saturating_add(compact_historical_details(
        &transaction,
        &root_key,
        &rollup_window,
    )?);
    transaction.execute(
        "INSERT INTO usage_scans(root_key, last_scan_ms) VALUES(?1, ?2)
         ON CONFLICT(root_key) DO UPDATE SET last_scan_ms=excluded.last_scan_ms",
        params![root_key, to_i64(now_ms)],
    )?;
    transaction.commit()?;
    if deleted_rows > 0 {
        reclaim_cache_space(&connection)?;
    }

    aggregate_cached_usage(&mut connection, &root_key, window, stats)
}
