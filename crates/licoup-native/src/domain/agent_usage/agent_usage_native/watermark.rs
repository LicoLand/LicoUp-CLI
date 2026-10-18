//! Persistent cumulative counters for conversations that can resume on a later day.

use super::cache::{RefreshStatements, json_column, variant_from_columns};
use super::models::{CumulativeSnapshot, CumulativeTotals};
use anyhow::Result;
use licoup_statistics::contract::{
    HistoryUsageSummary, MessageUsage, ModelTokenUsageSummary, UNATTRIBUTED_MODEL, UsageVariant,
};
use licoup_statistics::window::UsageWindow;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub(super) struct Watermark {
    pub(super) session_key: String,
    pub(super) model: Option<String>,
    pub(super) variant: UsageVariant,
    pub(super) day: String,
    pub(super) last: CumulativeTotals,
    pub(super) day_total: CumulativeTotals,
    pub(super) day_variants: BTreeMap<(Option<String>, UsageVariant), CumulativeTotals>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum WatermarkProjection {
    AppendDelta,
    ReplaceCurrentDay,
    RebuildAllHistory,
}

/// Old ledger rows have already been sealed before this replaces their cursor.
/// These snapshots establish a boundary; none of their totals are new usage.
pub(super) fn initialize_cumulative_cursor(
    statements: &mut RefreshStatements<'_>,
    scope_key: &str,
    source_key: &str,
    snapshots: &[CumulativeSnapshot],
) -> Result<()> {
    statements.clear_watermarks(scope_key, source_key)?;
    let mut latest = BTreeMap::new();
    for snapshot in snapshots {
        latest.insert(&snapshot.usage_key, snapshot);
    }
    for (key, snapshot) in latest {
        statements.save_watermark(
            scope_key,
            source_key,
            key,
            &Watermark {
                session_key: snapshot.session_key.clone(),
                model: snapshot.model.clone(),
                variant: snapshot.variant.clone(),
                day: snapshot.observed_day.clone(),
                last: snapshot.totals,
                day_total: CumulativeTotals::default(),
                day_variants: BTreeMap::new(),
            },
        )?;
    }
    Ok(())
}

pub(super) fn cursor_covers_old_sessions(
    statements: &RefreshStatements<'_>,
    scope_key: &str,
    source_key: &str,
    snapshots: &[CumulativeSnapshot],
) -> Result<bool> {
    let sessions = snapshots
        .iter()
        .map(|snapshot| snapshot.session_key.as_str())
        .collect::<BTreeSet<_>>();
    let mut query = statements.transaction().prepare(
        "SELECT DISTINCT session_key FROM native_usage_watermarks WHERE scope_key=?1 AND source_key=?2",
    )?;
    let mut rows = query.query(rusqlite::params![scope_key, source_key])?;
    while let Some(row) = rows.next()? {
        if !sessions.contains(row.get::<_, String>(0)?.as_str()) {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(super) fn apply_cumulative_watermarks(
    statements: &mut RefreshStatements<'_>,
    scope_key: &str,
    source_key: &str,
    calendar: &UsageWindow,
    snapshots: &[CumulativeSnapshot],
    projection: WatermarkProjection,
    summary: &mut HistoryUsageSummary,
) -> Result<u64> {
    let mut states = if projection == WatermarkProjection::RebuildAllHistory {
        BTreeMap::new()
    } else {
        load_watermarks_batch(statements.transaction(), scope_key, source_key, snapshots)?
    };
    let (projected_snapshots, suppressed) =
        snapshots_for_projection(snapshots, projection, &states);
    let mut sessions = BTreeSet::new();
    for snapshot in projected_snapshots {
        if !calendar.contains(&snapshot.observed_day) {
            continue;
        }
        let (state, added) = next_watermark(states.get(&snapshot.usage_key), snapshot);
        statements.save_watermark(scope_key, source_key, &snapshot.usage_key, &state)?;
        states.insert(snapshot.usage_key.clone(), state);
        if projection != WatermarkProjection::ReplaceCurrentDay
            && snapshot.projects_usage
            && add_to_summary(summary, snapshot, added)
        {
            sessions.insert(snapshot.session_key.clone());
        }
    }

    if projection != WatermarkProjection::ReplaceCurrentDay {
        return Ok(sessions.len() as u64);
    }
    for (_, state) in states.iter().filter(|(usage_key, state)| {
        calendar.contains(&state.day) && !suppressed.contains(usage_key.as_str())
    }) {
        let mut partitions = 0u64;
        for ((model, variant), totals) in &state.day_variants {
            if add_totals(
                summary,
                *totals,
                model.clone(),
                variant.clone(),
                state.day.clone(),
            ) {
                partitions += 1;
                sessions.insert(state.session_key.clone());
                let model = model
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or(UNATTRIBUTED_MODEL);
                if let Some(day) = summary.daily_usage.get_mut(&state.day) {
                    if let Some(usage) = day.model_usage.get_mut(model) {
                        usage.request_count = usage.request_count.saturating_sub(1);
                    }
                    if let Some(usage) = day
                        .model_variants
                        .get_mut(&(model.to_owned(), variant.clone()))
                    {
                        usage.request_count = usage.request_count.saturating_sub(1);
                    }
                }
            }
        }
        // Variants partition one cumulative record; they do not create extra
        // requests merely because this refresh rebuilds the current day.
        let extra = partitions.saturating_sub(1);
        summary.explicit_records = summary.explicit_records.saturating_sub(extra);
        if let Some(day) = summary.daily_usage.get_mut(&state.day) {
            day.explicit_records = day.explicit_records.saturating_sub(extra);
            day.message_count = day.message_count.saturating_sub(extra);
            day.request_count = day.request_count.saturating_sub(extra);
            if partitions > 0 {
                // The cumulative record does not tell us how many requests
                // belong to each token partition. Retain its one count without
                // assigning it to an effort or fast setting.
                day.add_model_variant_totals(
                    state
                        .model
                        .clone()
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or_else(|| UNATTRIBUTED_MODEL.to_owned()),
                    UsageVariant::default(),
                    ModelTokenUsageSummary {
                        request_count: 1,
                        ..Default::default()
                    },
                );
            }
        }
    }
    Ok(sessions.len() as u64)
}

fn snapshots_for_projection<'a>(
    snapshots: &'a [CumulativeSnapshot],
    projection: WatermarkProjection,
    states: &BTreeMap<String, Watermark>,
) -> (Vec<&'a CumulativeSnapshot>, BTreeSet<&'a str>) {
    if projection != WatermarkProjection::ReplaceCurrentDay {
        return (snapshots.iter().collect(), BTreeSet::new());
    }
    let mut latest = BTreeMap::new();
    for snapshot in snapshots {
        latest.insert(snapshot.usage_key.as_str(), snapshot);
    }
    let suppressed = latest
        .values()
        .filter(|snapshot| !snapshot.projects_usage)
        .map(|snapshot| snapshot.usage_key.as_str())
        .collect();
    let mut floors = BTreeMap::new();
    for (key, snapshot) in &latest {
        match states.get(*key) {
            Some(previous)
                if snapshot.observed_day >= previous.day
                    && snapshot.totals.at_least(previous.last) =>
            {
                floors.insert(*key, previous.last);
            }
            None if snapshot.first_day == snapshot.observed_day => {
                floors.insert(*key, CumulativeTotals::default());
            }
            _ => {}
        }
    }
    let projected = snapshots
        .iter()
        .filter(|snapshot| {
            let final_snapshot = latest[snapshot.usage_key.as_str()];
            if std::ptr::eq(*snapshot, final_snapshot) {
                return true;
            }
            let Some(floor) = floors.get_mut(snapshot.usage_key.as_str()) else {
                return false;
            };
            // Preserve the existing final-counter/reset result. Replayed records
            // and counters above the final value cannot create new token totals.
            if snapshot.observed_day != final_snapshot.observed_day
                || !snapshot.totals.at_least(*floor)
                || !final_snapshot.totals.at_least(snapshot.totals)
                || (!states.contains_key(snapshot.usage_key.as_str())
                    && snapshot.first_day != snapshot.observed_day)
            {
                return false;
            }
            *floor = snapshot.totals;
            true
        })
        .collect();
    (projected, suppressed)
}

fn next_watermark(
    previous: Option<&Watermark>,
    snapshot: &CumulativeSnapshot,
) -> (Watermark, CumulativeTotals) {
    let initial = if snapshot.first_day == snapshot.observed_day {
        snapshot.totals
    } else {
        CumulativeTotals::default()
    };
    let mut next = previous.cloned().unwrap_or_else(|| Watermark {
        session_key: snapshot.session_key.clone(),
        model: snapshot.model.clone(),
        variant: snapshot.variant.clone(),
        day: snapshot.observed_day.clone(),
        last: snapshot.totals,
        day_total: initial,
        day_variants: BTreeMap::new(),
    });
    let mut added = initial;
    if let Some(previous) = previous {
        if snapshot.observed_day > previous.day {
            next.day = snapshot.observed_day.clone();
            next.day_variants.clear();
            next.day_total = if snapshot.totals.at_least(previous.last) {
                snapshot.totals.delta(previous.last)
            } else {
                CumulativeTotals::default()
            };
            added = next.day_total;
        } else if snapshot.observed_day == previous.day && snapshot.totals.at_least(previous.last) {
            added = snapshot.totals.delta(previous.last);
            next.day_total = add(next.day_total, added);
        } else {
            added = CumulativeTotals::default();
        }
        if snapshot.observed_day >= previous.day {
            next.last = snapshot.totals;
            next.session_key = snapshot.session_key.clone();
            next.model = snapshot.model.clone();
            next.variant = snapshot.variant.clone();
        }
    }
    if added.prompt.saturating_add(added.completion) > 0 {
        let partition = next
            .day_variants
            .entry((snapshot.model.clone(), snapshot.variant.clone()))
            .or_default();
        *partition = add(*partition, added);
    }
    (next, added)
}

fn add_to_summary(
    summary: &mut HistoryUsageSummary,
    snapshot: &CumulativeSnapshot,
    totals: CumulativeTotals,
) -> bool {
    add_totals(
        summary,
        totals,
        snapshot.model.clone(),
        snapshot.variant.clone(),
        snapshot.observed_day.clone(),
    )
}

fn add_totals(
    summary: &mut HistoryUsageSummary,
    totals: CumulativeTotals,
    model: Option<String>,
    variant: UsageVariant,
    day: String,
) -> bool {
    let total = totals.prompt.saturating_add(totals.completion);
    if total == 0 {
        return false;
    }
    summary.add(
        MessageUsage {
            prompt_tokens: totals.prompt,
            cached_input_tokens: totals.cached.min(totals.prompt),
            completion_tokens: totals.completion,
            total_tokens: total,
            model,
            variant,
            accuracy: Default::default(),
        },
        Some(day),
    );
    true
}

fn add(left: CumulativeTotals, right: CumulativeTotals) -> CumulativeTotals {
    CumulativeTotals {
        prompt: left.prompt.saturating_add(right.prompt),
        cached: left.cached.saturating_add(right.cached),
        completion: left.completion.saturating_add(right.completion),
    }
}

fn load_watermarks_batch(
    transaction: &rusqlite::Transaction<'_>,
    scope_key: &str,
    source_key: &str,
    snapshots: &[CumulativeSnapshot],
) -> Result<BTreeMap<String, Watermark>> {
    let usage_keys = snapshots
        .iter()
        .map(|snapshot| snapshot.usage_key.as_str())
        .collect::<BTreeSet<_>>();
    if usage_keys.is_empty() {
        return Ok(BTreeMap::new());
    }
    let placeholders = vec!["?"; usage_keys.len()].join(",");
    let mut statement = transaction.prepare(&format!(
        "SELECT usage_key,session_key,model,day,last_prompt,last_cached,last_completion,
                day_prompt,day_cached,day_completion,effort,fast,day_variants
         FROM native_usage_watermarks
         WHERE scope_key=?1 AND source_key=?2 AND usage_key IN ({placeholders})"
    ))?;
    let mut parameters = Vec::with_capacity(usage_keys.len() + 2);
    parameters.push(scope_key);
    parameters.push(source_key);
    parameters.extend(usage_keys.iter().copied());
    let rows = statement.query_map(rusqlite::params_from_iter(parameters), |row| {
        let day_variants: Vec<((Option<String>, UsageVariant), CumulativeTotals)> =
            json_column(row, 12)?;
        Ok((
            row.get::<_, String>(0)?,
            Watermark {
                session_key: row.get(1)?,
                model: row.get(2)?,
                variant: variant_from_columns(row.get(10)?, row.get(11)?),
                day: row.get(3)?,
                last: totals(row.get(4)?, row.get(5)?, row.get(6)?),
                day_total: totals(row.get(7)?, row.get(8)?, row.get(9)?),
                day_variants: day_variants.into_iter().collect(),
            },
        ))
    })?;
    rows.collect::<rusqlite::Result<BTreeMap<_, _>>>()
        .map_err(Into::into)
}

fn totals(prompt: i64, cached: i64, completion: i64) -> CumulativeTotals {
    CumulativeTotals {
        prompt: from_i64(prompt),
        cached: from_i64(cached),
        completion: from_i64(completion),
    }
}

fn from_i64(value: i64) -> u64 {
    value.max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::super::cache::open_cache_database;
    use super::*;
    use serde_json::json;
    use std::path::Path;

    fn snapshot(prompt: u64, effort: Option<&str>, fast: Option<bool>) -> CumulativeSnapshot {
        CumulativeSnapshot {
            usage_key: "synthetic-counter".to_owned(),
            session_key: "synthetic-session".to_owned(),
            model: Some("raw-model".to_owned()),
            variant: UsageVariant {
                effort: effort.map(str::to_owned),
                fast,
            },
            first_day: "2026-07-15".to_owned(),
            observed_day: "2026-07-15".to_owned(),
            totals: CumulativeTotals {
                prompt,
                ..Default::default()
            },
            projects_usage: true,
        }
    }

    #[test]
    fn replacement_preserves_each_actual_variant_and_only_new_cumulative_growth() {
        let mut connection = open_cache_database(Path::new(":memory:")).unwrap();
        let transaction = connection.transaction().unwrap();
        let mut statements = RefreshStatements::prepare(&transaction).unwrap();
        let calendar =
            UsageWindow::from_params(&json!({"now":"2026-07-15T12:00:00Z", "historyDays":1}));
        let high = snapshot(100, Some("high"), Some(false));
        let low = snapshot(130, Some("low"), None);
        let fast = snapshot(150, None, Some(true));
        let mut initial = HistoryUsageSummary::default();
        apply_cumulative_watermarks(
            &mut statements,
            "scope",
            "source",
            &calendar,
            std::slice::from_ref(&high),
            WatermarkProjection::RebuildAllHistory,
            &mut initial,
        )
        .unwrap();
        let mut replaced = HistoryUsageSummary::default();
        assert_eq!(
            apply_cumulative_watermarks(
                &mut statements,
                "scope",
                "source",
                &calendar,
                &[high.clone(), low.clone(), fast.clone()],
                WatermarkProjection::ReplaceCurrentDay,
                &mut replaced
            )
            .unwrap(),
            1
        );
        assert_eq!(replaced.total_tokens(), 150);
        assert_eq!(replaced.explicit_records, 1);
        assert_eq!(replaced.daily_usage["2026-07-15"].message_count, 1);
        let day = &replaced.daily_usage["2026-07-15"];
        assert_eq!(day.request_count, 1);
        assert_eq!(day.model_usage["raw-model"].request_count, 1);
        let models = &day.model_variants;
        assert_eq!(
            models.values().map(|usage| usage.total_tokens).sum::<u64>(),
            day.total_tokens
        );
        assert_eq!(
            models
                .values()
                .map(|usage| usage.request_count)
                .sum::<u64>(),
            1
        );
        let unallocated = models[&("raw-model".to_owned(), UsageVariant::default())];
        assert_eq!(unallocated.total_tokens, 0);
        assert_eq!(unallocated.request_count, 1);
        for (snapshot, expected) in [(&high, 100), (&low, 30), (&fast, 20)] {
            assert_eq!(
                models[&("raw-model".to_owned(), snapshot.variant.clone())].total_tokens,
                expected
            );
            assert_eq!(
                models[&("raw-model".to_owned(), snapshot.variant.clone())].request_count,
                0
            );
        }
        // A reset keeps the existing day total and resets only the last
        // counter, just as the previous latest-only projection did.
        let reset = snapshot(120, None, None);
        let mut after_reset = HistoryUsageSummary::default();
        apply_cumulative_watermarks(
            &mut statements,
            "scope",
            "source",
            &calendar,
            &[
                high.clone(),
                snapshot(160, Some("low"), None),
                reset.clone(),
            ],
            WatermarkProjection::ReplaceCurrentDay,
            &mut after_reset,
        )
        .unwrap();
        assert_eq!(after_reset.total_tokens(), 150);
        let mut resumed = HistoryUsageSummary::default();
        apply_cumulative_watermarks(
            &mut statements,
            "scope",
            "source",
            &calendar,
            &[reset, snapshot(125, Some("high"), Some(false))],
            WatermarkProjection::ReplaceCurrentDay,
            &mut resumed,
        )
        .unwrap();
        assert_eq!(resumed.total_tokens(), 155);
        assert_eq!(
            resumed.daily_usage["2026-07-15"].model_variants
                [&("raw-model".to_owned(), high.variant.clone())]
                .total_tokens,
            105
        );

        let tomorrow =
            UsageWindow::from_params(&json!({"now":"2026-07-16T12:00:00Z", "historyDays":1}));
        let mut later = snapshot(140, Some("high"), Some(false));
        later.observed_day = "2026-07-16".to_owned();
        let mut next_day = HistoryUsageSummary::default();
        apply_cumulative_watermarks(
            &mut statements,
            "scope",
            "source",
            &tomorrow,
            &[later.clone()],
            WatermarkProjection::ReplaceCurrentDay,
            &mut next_day,
        )
        .unwrap();
        assert_eq!(next_day.total_tokens(), 15);
        let saved = load_watermarks_batch(&transaction, "scope", "source", &[later]).unwrap();
        assert_eq!(saved["synthetic-counter"].day_variants.len(), 1);
        assert_eq!(saved["synthetic-counter"].day_total.prompt, 15);
    }
}
