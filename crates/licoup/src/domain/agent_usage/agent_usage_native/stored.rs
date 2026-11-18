use super::cache::RefreshStatements;
use super::models::{CachedSource, SourceMetadata};
use super::snapshot_cursor::SnapshotCursor;
use crate::domain::conversation::history::acop_codec_for;
use crate::domain::conversation::source_catalog::HistoryAdapter;
use acop::store::SqliteUsageSpec;
use acop_usage_source::stored::StoredSource;
use agent_statistics::contract::{
    HistoryUsageSummary, ModelTokenUsageSummary, UNATTRIBUTED_MODEL, UsageVariant,
};
use agent_statistics::window::UsageWindow;
use anyhow::{Context, Result, ensure};
use metera_sdk::store::{
    AdvanceInput, AdvanceKind, Counts, SessionUsage, UsageAllocation, UsageCursor,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

/// The persisted cursor never names an Agent session: each entry is keyed by
/// the SHA-256 of the session id and the stored usage cursor carries the same
/// digest as its `id`, so the cache stores no raw identifier.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub(super) struct Cursor {
    sessions: BTreeMap<String, Session>,
    time_inferred: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
struct Session {
    value: UsageCursor,
    observed_day: String,
    present: bool,
    counted: bool,
}

#[derive(Clone, Debug)]
pub(super) struct Projection {
    pub(super) summary: HistoryUsageSummary,
    pub(super) cursor: Cursor,
    pub(super) gap: bool,
    pub(super) rebase: bool,
    pub(super) session_count: u64,
}

fn session_key(id: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(id.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(super) fn declaration(adapter: HistoryAdapter) -> Option<&'static SqliteUsageSpec> {
    acop_codec_for(adapter)?.stored_usage()
}

pub(super) fn accepts(spec: &SqliteUsageSpec, path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            spec.file_names
                .iter()
                .any(|declared| name.eq_ignore_ascii_case(declared))
        })
}

pub(super) fn ready(source: &CachedSource) -> bool {
    matches!(source.migration_state, 4 | 5)
        && source
            .snapshot_cursor
            .as_ref()
            .and_then(|cursor| cursor.stored.as_ref())
            .is_some()
}

pub(super) fn read(
    reader: &mut StoredSource,
    path: &Path,
    previous: Option<&CachedSource>,
    metadata: &SourceMetadata,
    calendar: &UsageWindow,
) -> Result<Projection> {
    let readings = reader.collect(path)?;
    project(reader, &readings, previous, metadata, calendar)
}

fn project(
    reader: &mut StoredSource,
    readings: &[SessionUsage],
    previous: Option<&CachedSource>,
    metadata: &SourceMetadata,
    calendar: &UsageWindow,
) -> Result<Projection> {
    let rebase = previous
        .is_some_and(|old| !ready(old) || old.file_id.is_none() || old.file_id != metadata.file_id);
    let old = previous
        .and_then(|source| source.snapshot_cursor.as_ref())
        .and_then(|cursor| cursor.stored.as_ref());
    let mut cursor = old.cloned().unwrap_or_default();
    for (key, session) in cursor.sessions.iter_mut() {
        session.present = false;
        // A cursor persisted before the digest normalization kept the raw
        // session id inside `value.id`; rewrite it to the digest key so the
        // next save stores no raw identifier either.
        session.value.id.clone_from(key);
    }
    let inputs = readings
        .iter()
        .map(|reading| {
            let old = old.and_then(|cursor| cursor.sessions.get(&session_key(&reading.id)));
            // The cursor is persisted; the raw session id never is, so the
            // SDK cursor carries the digest as its identity.
            let mut current = UsageCursor::from(reading);
            current.id = session_key(&reading.id);
            AdvanceInput {
                previous: if rebase {
                    None
                } else {
                    old.filter(|session| session.present)
                        .map(|session| session.value.clone())
                },
                current,
                count_initial: !rebase && old.is_none(),
            }
        })
        .collect::<Vec<_>>();
    let advances = reader.advance(&inputs)?;
    ensure!(
        advances.len() == readings.len(),
        "stored_usage_response_incomplete"
    );
    let seen = readings
        .iter()
        .map(|reading| session_key(&reading.id))
        .collect::<BTreeSet<_>>();
    let mut gap = rebase
        || previous.is_some_and(|source| source.migration_state == 5)
        || old.is_some_and(|cursor| {
            cursor
                .sessions
                .iter()
                .any(|(key, session)| session.present && !seen.contains(key))
        });
    let mut summary = HistoryUsageSummary::default();
    let mut session_count = previous
        .filter(|source| !rebase && !source.sealed)
        .map_or(0, |source| source.session_count);
    for ((reading, input), advance) in readings.iter().zip(inputs).zip(advances) {
        let old = old.and_then(|cursor| cursor.sessions.get(&session_key(&reading.id)));
        let mut counted = rebase || old.is_some_and(|session| session.counted);
        gap |= advance.attribution_gap || advance.kind != AdvanceKind::Delta;
        let mut projected = false;
        let current_counters = reading
            .counters
            .iter()
            .map(|counter| (counter.key.as_str(), counter))
            .collect::<BTreeMap<_, _>>();
        let mut allocations_by_counter = BTreeMap::<&str, Vec<&UsageAllocation>>::new();
        for allocation in &reading.allocations {
            allocations_by_counter
                .entry(&allocation.counter_key)
                .or_default()
                .push(allocation);
        }
        for counter in advance.counters {
            let current = current_counters
                .get(counter.key.as_str())
                .context("stored_usage_counter_missing")?;
            let allocations = allocations_by_counter
                .get(counter.key.as_str())
                .context("stored_usage_allocation_missing")?;
            ensure!(!allocations.is_empty(), "stored_usage_allocation_missing");
            if counter.delta == current.counts {
                for allocation in allocations {
                    cursor.time_inferred |= allocation.time_inferred;
                    if let Some(day) = allocation_day(allocation, calendar)? {
                        add(
                            &mut summary,
                            &day,
                            allocation.model.as_deref(),
                            allocation.counts,
                        )?;
                        projected = true;
                    } else {
                        gap = true;
                    }
                }
                continue;
            }
            for model in counter.models {
                let candidates = allocations
                    .iter()
                    .copied()
                    .filter(|allocation| counter.attribution_gap || allocation.model == model.model)
                    .collect::<Vec<_>>();
                let mut days = BTreeSet::new();
                let mut definite = !candidates.is_empty();
                let mut current_activity = false;
                for allocation in candidates {
                    cursor.time_inferred |= allocation.time_inferred;
                    current_activity |= allocation.time_inferred
                        || epoch_day(allocation.last_seen, calendar)? == calendar.end;
                    if let Some(day) = allocation_day(allocation, calendar)? {
                        days.insert(day);
                    } else {
                        definite = false;
                    }
                }
                let day = if definite && days.len() == 1 {
                    days.into_iter().next()
                } else if current_activity
                    && old.is_some_and(|session| {
                        session.present && session.observed_day == calendar.end
                    })
                {
                    cursor.time_inferred = true;
                    Some(calendar.end.clone())
                } else {
                    None
                };
                if let Some(day) = day {
                    add(&mut summary, &day, model.model.as_deref(), model.counts)?;
                    projected = true;
                } else {
                    gap = true;
                }
            }
        }
        if projected && !counted {
            add_integer(&mut session_count, 1)?;
            add_integer(&mut summary.session_count, 1)?;
            counted = true;
        }
        cursor.sessions.insert(
            session_key(&reading.id),
            Session {
                value: input.current,
                observed_day: calendar.end.clone(),
                present: true,
                counted,
            },
        );
    }
    Ok(Projection {
        summary,
        cursor,
        gap,
        rebase,
        session_count,
    })
}

pub(super) fn apply(
    statements: &mut RefreshStatements<'_>,
    scope_key: &str,
    source_key: &str,
    metadata: &SourceMetadata,
    previous: Option<&CachedSource>,
    calendar: &UsageWindow,
    projection: Projection,
) -> Result<u64> {
    if projection.rebase
        && let Some(previous) = previous
    {
        statements.seal(
            scope_key,
            source_key,
            if previous.sealed {
                0
            } else {
                previous.session_count
            },
        )?;
    }
    statements.add_rollup(scope_key, source_key, &projection.summary)?;
    statements.save_source(
        scope_key,
        source_key,
        metadata,
        metadata.size,
        "",
        projection.session_count,
        &Default::default(),
    )?;
    statements.save_snapshot_cursor(
        scope_key,
        source_key,
        &SnapshotCursor::for_stored(projection.cursor, calendar, projection.session_count),
    )?;
    statements.save_migration_state(scope_key, source_key, if projection.gap { 5 } else { 4 })?;
    statements.compact(
        scope_key,
        source_key,
        &calendar.end,
        projection.session_count,
    )
}

fn allocation_day(allocation: &UsageAllocation, calendar: &UsageWindow) -> Result<Option<String>> {
    let first = epoch_day(allocation.first_seen, calendar)?;
    let last = epoch_day(allocation.last_seen, calendar)?;
    Ok((first == last && (!allocation.time_inferred || first == calendar.end)).then_some(first))
}

fn epoch_day(timestamp: f64, calendar: &UsageWindow) -> Result<String> {
    ensure!(timestamp.is_finite(), "stored_usage_timestamp_invalid");
    let seconds =
        i64::try_from(timestamp.floor() as i128).context("stored_usage_timestamp_out_of_range")?;
    let utc = OffsetDateTime::from_unix_timestamp(seconds)
        .context("stored_usage_timestamp_out_of_range")?;
    ensure!(
        utc.checked_add(Duration::days(1)).is_some()
            && utc.checked_sub(Duration::days(1)).is_some(),
        "stored_usage_timestamp_out_of_range"
    );
    calendar
        .date_key(&utc.format(&Rfc3339)?)
        .context("stored_usage_timestamp_invalid")
}

fn add_integer(target: &mut u64, value: u64) -> Result<()> {
    *target = target
        .checked_add(value)
        .filter(|value| i64::try_from(*value).is_ok())
        .context("stored_usage_counter_out_of_range")?;
    Ok(())
}

fn add(
    summary: &mut HistoryUsageSummary,
    day: &str,
    model: Option<&str>,
    value: Counts,
) -> Result<()> {
    let prompt = value.prompt.context("stored_usage_metric_missing")?;
    let cached = value.cached.context("stored_usage_metric_missing")?;
    let completion = value.completion.context("stored_usage_metric_missing")?;
    let total = value.total.context("stored_usage_metric_missing")?;
    let requests = value.requests.context("stored_usage_metric_missing")?;
    add_integer(&mut summary.explicit_prompt_tokens, prompt)?;
    add_integer(&mut summary.explicit_cached_input_tokens, cached)?;
    add_integer(&mut summary.explicit_completion_tokens, completion)?;
    add_integer(&mut summary.explicit_total_tokens, total)?;
    add_integer(&mut summary.explicit_records, requests)?;
    add_integer(&mut summary.message_count, requests)?;
    let daily = summary.daily_usage.entry(day.to_owned()).or_default();
    add_integer(&mut daily.prompt_tokens, prompt)?;
    add_integer(&mut daily.cached_input_tokens, cached)?;
    add_integer(&mut daily.completion_tokens, completion)?;
    add_integer(&mut daily.total_tokens, total)?;
    add_integer(&mut daily.explicit_records, requests)?;
    add_integer(&mut daily.message_count, requests)?;
    add_integer(&mut daily.request_count, requests)?;
    daily.add_model_variant_totals(
        model.unwrap_or(UNATTRIBUTED_MODEL).to_owned(),
        UsageVariant::default(),
        ModelTokenUsageSummary {
            prompt_tokens: prompt,
            cached_input_tokens: cached,
            completion_tokens: completion,
            total_tokens: total,
            request_count: requests,
            ..Default::default()
        },
    );
    Ok(())
}

#[cfg(test)]
#[path = "stored_tests.rs"]
mod tests;
