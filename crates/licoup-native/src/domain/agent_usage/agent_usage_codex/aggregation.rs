use super::models::ScanStats;
use super::rollup::{DailyRollup, collect_detail_rollups, normalized_model};
use super::utils::from_i64;
use anyhow::{Context, Result};
use licoup_statistics::contract::{HistoryUsageSummary, UsageVariant};
use licoup_statistics::window::UsageWindow;
use rusqlite::{Connection, TransactionBehavior, params};
use std::collections::BTreeSet;

fn apply_rollup(
    summary: &mut HistoryUsageSummary,
    sessions: &mut BTreeSet<String>,
    day: String,
    rollup: DailyRollup,
) {
    summary.explicit_prompt_tokens = summary
        .explicit_prompt_tokens
        .saturating_add(rollup.explicit_prompt);
    summary.explicit_cached_input_tokens = summary
        .explicit_cached_input_tokens
        .saturating_add(rollup.explicit_cached.min(rollup.explicit_prompt));
    summary.explicit_completion_tokens = summary
        .explicit_completion_tokens
        .saturating_add(rollup.explicit_completion);
    summary.explicit_total_tokens = summary.explicit_total_tokens.saturating_add(
        rollup
            .explicit_prompt
            .saturating_add(rollup.explicit_completion),
    );
    summary.explicit_records = summary
        .explicit_records
        .saturating_add(rollup.explicit_records);
    summary.message_count = summary.message_count.saturating_add(rollup.message_count);
    sessions.extend(rollup.sessions);

    let daily = summary.daily_usage.entry(day).or_default();
    daily.prompt_tokens = daily.prompt_tokens.saturating_add(rollup.explicit_prompt);
    daily.cached_input_tokens = daily
        .cached_input_tokens
        .saturating_add(rollup.explicit_cached.min(rollup.explicit_prompt));
    daily.completion_tokens = daily
        .completion_tokens
        .saturating_add(rollup.explicit_completion);
    daily.total_tokens = daily.total_tokens.saturating_add(
        rollup
            .explicit_prompt
            .saturating_add(rollup.explicit_completion),
    );
    daily.message_count = daily.message_count.saturating_add(rollup.message_count);
    daily.explicit_records = daily
        .explicit_records
        .saturating_add(rollup.explicit_records);
    for ((model, variant), usage) in rollup.models {
        daily.add_model_usage_with_variant(
            normalized_model(&model),
            variant,
            usage.prompt,
            usage.cached,
            usage.completion,
            usage.total,
        );
    }
}

pub(super) fn aggregate_cached_usage(
    connection: &mut Connection,
    root_key: &str,
    window: &UsageWindow,
    stats: ScanStats,
) -> Result<HistoryUsageSummary> {
    let snapshot = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .context("agent usage cache snapshot transaction failed")?;
    let mut summary = HistoryUsageSummary {
        source: Some("codex-local-token-events"),
        scan_cache: Some(stats.to_json()),
        ..HistoryUsageSummary::default()
    };
    let mut sessions = BTreeSet::<String>::new();
    let mut historical = std::collections::BTreeMap::<String, DailyRollup>::new();
    {
        let mut statement = snapshot.prepare(
            "SELECT day, explicit_prompt, explicit_cached, explicit_completion,
                    explicit_records, message_count
             FROM usage_daily_totals
             WHERE root_key=?1 AND day>=?2 AND day<=?3
             ORDER BY day",
        )?;
        let rows = statement.query_map(params![root_key, &window.start, &window.end], |row| {
            Ok((
                row.get::<_, String>(0)?,
                DailyRollup {
                    explicit_prompt: from_i64(row.get(1)?),
                    explicit_cached: from_i64(row.get(2)?),
                    explicit_completion: from_i64(row.get(3)?),
                    explicit_records: from_i64(row.get(4)?),
                    message_count: from_i64(row.get(5)?),
                    ..DailyRollup::default()
                },
            ))
        })?;
        for row in rows {
            let (day, rollup) = row?;
            historical.insert(day, rollup);
        }
    }
    {
        let mut statement = snapshot.prepare(
            "SELECT day, model, prompt_tokens, cached_input_tokens,
                    completion_tokens, total_tokens,effort,fast
             FROM usage_daily_models
             WHERE root_key=?1 AND day>=?2 AND day<=?3
             ORDER BY day, model",
        )?;
        let rows = statement.query_map(params![root_key, &window.start, &window.end], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                super::rollup::ModelRollup {
                    prompt: from_i64(row.get(2)?),
                    cached: from_i64(row.get(3)?),
                    completion: from_i64(row.get(4)?),
                    total: from_i64(row.get(5)?),
                },
                UsageVariant {
                    effort: Some(row.get::<_, String>(6)?).filter(|effort| !effort.is_empty()),
                    fast: match row.get::<_, i64>(7)? {
                        0 => Some(false),
                        1 => Some(true),
                        _ => None,
                    },
                },
            ))
        })?;
        for row in rows {
            let (day, model, usage, variant) = row?;
            historical
                .entry(day)
                .or_default()
                .models
                .insert((model, variant), usage);
        }
    }
    {
        let mut statement = snapshot.prepare(
            "SELECT day, session_key
             FROM usage_daily_sessions
             WHERE root_key=?1 AND day>=?2 AND day<=?3",
        )?;
        let rows = statement.query_map(params![root_key, &window.start, &window.end], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (day, session) = row?;
            historical.entry(day).or_default().sessions.insert(session);
        }
    }
    for (day, rollup) in historical {
        apply_rollup(&mut summary, &mut sessions, day, rollup);
    }
    let details = collect_detail_rollups(&snapshot, root_key, window, &window.end, true)?;
    for (day, rollup) in details {
        apply_rollup(&mut summary, &mut sessions, day, rollup);
    }
    snapshot
        .commit()
        .context("agent usage cache snapshot commit failed")?;
    summary.session_count = sessions.len() as u64;
    summary.source = (summary.explicit_records > 0).then_some("codex-local-token-events");
    if summary.explicit_records > 0 {
        summary
            .source_paths
            .insert("codex-local-usage-store".to_string());
    }
    Ok(summary)
}
