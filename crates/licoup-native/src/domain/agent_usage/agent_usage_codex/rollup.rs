use super::utils::{from_i64, to_i64};
use anyhow::Result;
use licoup_statistics::contract::{UNATTRIBUTED_MODEL, UsageVariant};
use licoup_statistics::window::UsageWindow;
use rusqlite::{Transaction, params};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default)]
pub(super) struct ModelRollup {
    pub(super) prompt: u64,
    pub(super) cached: u64,
    pub(super) completion: u64,
    pub(super) total: u64,
}

#[derive(Clone, Debug, Default)]
pub(super) struct DailyRollup {
    pub(super) explicit_prompt: u64,
    pub(super) explicit_cached: u64,
    pub(super) explicit_completion: u64,
    pub(super) explicit_records: u64,
    pub(super) message_count: u64,
    pub(super) models: BTreeMap<(String, UsageVariant), ModelRollup>,
    pub(super) sessions: BTreeSet<String>,
}

impl DailyRollup {
    fn add_model(
        &mut self,
        model: String,
        variant: UsageVariant,
        prompt: u64,
        cached: u64,
        completion: u64,
    ) {
        let usage = self.models.entry((model, variant)).or_default();
        usage.prompt = usage.prompt.saturating_add(prompt);
        usage.cached = usage.cached.saturating_add(cached.min(prompt));
        usage.completion = usage.completion.saturating_add(completion);
        usage.total = usage
            .total
            .saturating_add(prompt.saturating_add(completion));
    }
}

pub(super) fn collect_detail_rollups(
    snapshot: &Transaction<'_>,
    root_key: &str,
    window: &UsageWindow,
    end: &str,
    include_end: bool,
) -> Result<BTreeMap<String, DailyRollup>> {
    let mut rollups = BTreeMap::<String, DailyRollup>::new();
    {
        let mut statement = snapshot.prepare(
            "SELECT r.source_key, r.session_id, r.day, r.model,
                    r.input_tokens, r.cached_input_tokens, r.output_tokens,r.effort,r.fast
             FROM usage_rows r
             INNER JOIN usage_files f
               ON f.root_key=r.root_key AND f.source_key=r.source_key
             WHERE r.root_key=?1 AND r.day>=?2
               AND (r.day<?3 OR (?4=1 AND r.day=?3))
               AND NOT EXISTS (
                 SELECT 1
                 FROM usage_rows prior
                 INNER JOIN usage_files prior_file
                   ON prior_file.root_key=prior.root_key
                  AND prior_file.source_key=prior.source_key
                 WHERE prior.root_key=r.root_key
                   AND prior.event_identity=r.event_identity
                   AND prior_file.lineage_scope=f.lineage_scope
                   AND (
                     CASE WHEN prior_file.forked_from_id IS NULL THEN 0 ELSE 1 END,
                     prior.day, prior.source_key, prior.event_index
                   ) < (
                     CASE WHEN f.forked_from_id IS NULL THEN 0 ELSE 1 END,
                     r.day, r.source_key, r.event_index
                   )
               )
             ORDER BY r.day, r.source_key, r.event_index",
        )?;
        let rows = statement.query_map(
            params![root_key, &window.start, end, i64::from(include_end)],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    from_i64(row.get(4)?),
                    from_i64(row.get(5)?),
                    from_i64(row.get(6)?),
                    UsageVariant {
                        effort: row.get(7)?,
                        fast: row.get(8)?,
                    },
                ))
            },
        )?;
        for row in rows {
            let (source, session, day, model, prompt, cached, completion, variant) = row?;
            let session_key = session.unwrap_or(source);
            let model = normalized_model(model.as_deref().unwrap_or_default());
            let rollup = rollups.entry(day).or_default();
            rollup.sessions.insert(session_key);
            rollup.explicit_prompt = rollup.explicit_prompt.saturating_add(prompt);
            rollup.explicit_cached = rollup.explicit_cached.saturating_add(cached.min(prompt));
            rollup.explicit_completion = rollup.explicit_completion.saturating_add(completion);
            rollup.explicit_records = rollup.explicit_records.saturating_add(1);
            rollup.message_count = rollup.message_count.saturating_add(1);
            rollup.add_model(model, variant, prompt, cached, completion);
        }
    }
    Ok(rollups)
}

pub(super) fn compact_historical_details(
    transaction: &Transaction<'_>,
    root_key: &str,
    window: &UsageWindow,
) -> Result<usize> {
    let rollups = collect_detail_rollups(transaction, root_key, window, &window.end, false)?;
    let mut insert_totals = transaction.prepare(
        "INSERT INTO usage_daily_totals VALUES(
           ?1,?2,?3,?4,?5,?6,?7
         ) ON CONFLICT(root_key,day) DO UPDATE SET
           explicit_prompt=explicit_prompt+excluded.explicit_prompt,
           explicit_cached=explicit_cached+excluded.explicit_cached,
           explicit_completion=explicit_completion+excluded.explicit_completion,
           explicit_records=explicit_records+excluded.explicit_records,
           message_count=message_count+excluded.message_count",
    )?;
    let mut insert_model = transaction.prepare(
        "INSERT INTO usage_daily_models VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
         ON CONFLICT(root_key,day,model,effort,fast) DO UPDATE SET
           prompt_tokens=prompt_tokens+excluded.prompt_tokens,
           cached_input_tokens=cached_input_tokens+excluded.cached_input_tokens,
           completion_tokens=completion_tokens+excluded.completion_tokens,
           total_tokens=total_tokens+excluded.total_tokens",
    )?;
    let mut insert_session = transaction.prepare(
        "INSERT OR IGNORE INTO usage_daily_sessions(root_key,day,session_key)
         VALUES(?1,?2,?3)",
    )?;
    for (day, rollup) in rollups {
        insert_totals.execute(params![
            root_key,
            day,
            to_i64(rollup.explicit_prompt),
            to_i64(rollup.explicit_cached),
            to_i64(rollup.explicit_completion),
            to_i64(rollup.explicit_records),
            to_i64(rollup.message_count),
        ])?;
        for ((model, variant), usage) in rollup.models {
            insert_model.execute(params![
                root_key,
                day,
                model,
                to_i64(usage.prompt),
                to_i64(usage.cached),
                to_i64(usage.completion),
                to_i64(usage.total),
                variant.effort.unwrap_or_default(),
                variant.fast.map(i64::from).unwrap_or(-1),
            ])?;
        }
        for session in rollup.sessions {
            insert_session.execute(params![root_key, day, session])?;
        }
    }
    drop(insert_totals);
    drop(insert_model);
    drop(insert_session);
    let deleted = transaction.execute(
        "DELETE FROM usage_rows WHERE root_key=?1 AND day<?2",
        params![root_key, &window.end],
    )?;
    Ok(deleted)
}

pub(super) fn normalized_model(model: &str) -> String {
    let model = model.trim();
    if model.is_empty() {
        UNATTRIBUTED_MODEL.to_string()
    } else {
        model.to_string()
    }
}
