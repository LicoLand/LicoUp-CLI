//! Reproject attributable current-day model rows without changing the usage ledger.

use super::cache::RefreshStatements;
use anyhow::Result;
use licoup_statistics::contract::{HistoryUsageSummary, ModelTokenUsageSummary, UsageVariant};
use licoup_statistics::variant::{is_placeholder_id, model_selection};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn targets(connection: &Connection, scope: &str, day: &str) -> Result<BTreeSet<String>> {
    let mut query = connection.prepare(
        "SELECT DISTINCT source_key,model FROM native_usage_source_models WHERE scope_key=?1 AND day=?2",
    )?;
    let rows = query.query_map(params![scope, day], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut targets = BTreeSet::new();
    for row in rows {
        let (source, raw) = row?;
        if is_placeholder_id(&model_selection(&raw).id) {
            targets.insert(source);
        }
    }
    Ok(targets)
}

pub(super) fn apply(
    statements: &mut RefreshStatements<'_>,
    scope: &str,
    source: &str,
    day: &str,
    parsed: &HistoryUsageSummary,
) -> Result<bool> {
    let Some(usage) = parsed.daily_usage.get(day) else {
        return Ok(false);
    };
    let connection = statements.transaction();
    let cumulative: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM native_usage_watermarks WHERE scope_key=?1 AND source_key=?2)",
        params![scope, source],
        |row| row.get(0),
    )?;
    if cumulative {
        return Ok(false);
    }
    let expected = [
        usage.prompt_tokens,
        usage.cached_input_tokens,
        usage.completion_tokens,
        usage.estimated_prompt_tokens,
        usage.estimated_completion_tokens,
        usage.explicit_records,
        usage.estimated_records,
        usage.message_count,
        usage.request_count,
        usage.token_unavailable_requests,
    ];
    let stored = connection
        .query_row(
            "SELECT prompt_tokens,cached_input_tokens,completion_tokens,estimated_prompt_tokens,
                estimated_completion_tokens,explicit_records,estimated_records,message_count,
                request_count,token_unavailable_requests FROM native_usage_source_days
         WHERE scope_key=?1 AND source_key=?2 AND day=?3",
            params![scope, source, day],
            |row| {
                let mut counts = [0u64; 10];
                for (index, count) in counts.iter_mut().enumerate() {
                    *count = row.get(index)?;
                }
                Ok(counts)
            },
        )
        .optional()?;
    if stored != Some(expected) {
        return Ok(false);
    }
    // Preserve the complete component vector for every observed variant too;
    // this operation is only a model redistribution, never a counter repair.
    let mut query = connection.prepare(
        "SELECT model,effort,fast,prompt_tokens,cached_input_tokens,completion_tokens,
                total_tokens,estimated_prompt_tokens,estimated_completion_tokens,
                request_count,token_unavailable_requests
         FROM native_usage_source_models WHERE scope_key=?1 AND source_key=?2 AND day=?3",
    )?;
    let old = query
        .query_map(params![scope, source, day], |row| {
            let fast: i64 = row.get(2)?;
            let effort: String = row.get(1)?;
            Ok((
                (
                    row.get::<_, String>(0)?,
                    UsageVariant {
                        effort: (!effort.is_empty()).then_some(effort),
                        fast: (fast >= 0).then_some(fast != 0),
                    },
                ),
                ModelTokenUsageSummary {
                    prompt_tokens: row.get(3)?,
                    cached_input_tokens: row.get(4)?,
                    completion_tokens: row.get(5)?,
                    total_tokens: row.get(6)?,
                    estimated_prompt_tokens: row.get(7)?,
                    estimated_completion_tokens: row.get(8)?,
                    request_count: row.get(9)?,
                    token_unavailable_requests: row.get(10)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
    let by_variant = |models: &BTreeMap<(String, UsageVariant), ModelTokenUsageSummary>| {
        let mut totals = BTreeMap::<UsageVariant, ModelTokenUsageSummary>::new();
        for ((_, variant), usage) in models {
            let total = totals.entry(variant.clone()).or_default();
            let cached = total
                .cached_input_tokens
                .saturating_add(usage.cached_input_tokens);
            total.merge(*usage);
            // Compare stored components verbatim; summary merging normally
            // clamps cached input, which must not hide an inconsistent row.
            total.cached_input_tokens = cached;
        }
        totals
    };
    if old == usage.model_variants || by_variant(&old) != by_variant(&usage.model_variants) {
        return Ok(false);
    }
    drop(query);
    statements.replace_model_rows(scope, source, day, usage)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::super::{cache, runtime::CacheRuntime, summarize_inner};
    use super::*;
    use licoup_statistics::{contract::AgentDef, window::UsageWindow};
    use serde_json::{Value, json};
    use std::{fs, path::PathBuf};

    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fixture(cumulative: bool) -> (Fixture, Value, UsageWindow) {
        let root =
            std::env::temp_dir().join(format!("native-identity-refresh-{}", uuid::Uuid::new_v4()));
        let history = root.join("history");
        fs::create_dir_all(&history).unwrap();
        let event = if cumulative {
            json!({"type":"session_usage","sessionId":"synthetic-session","timestamp":"2026-07-15T10:00:00Z",
                "model":"actual-model","usage":{"input_tokens":10,"cached_input_tokens":4,"output_tokens":2}})
        } else {
            json!({"type":"assistant","timestamp":"2026-07-15T10:00:00Z","model":"Default",
                "message":{"model":"actual-model","usage":{"input_tokens":10,"cache_read_input_tokens":4,"output_tokens":2}}})
        };
        fs::write(history.join("synthetic.jsonl"), format!("{event}\n")).unwrap();
        let params = json!({"root":history,"stateRoot":root.join("state"),"forceRefresh":true,
            "now":"2026-07-15T12:00:00Z","historyDays":2});
        let window = UsageWindow::from_params(&params);
        (Fixture(root), params, window)
    }

    fn scan(params: &Value, window: &UsageWindow) -> HistoryUsageSummary {
        summarize_inner(
            &AgentDef {
                id: "claude-code",
                label: "Claude Code",
            },
            params,
            window,
            &mut Vec::new(),
            &CacheRuntime::new(),
        )
        .unwrap()
    }

    fn open(params: &Value) -> Connection {
        cache::open_cache_database(&cache::cache_path(std::path::Path::new(
            params["stateRoot"].as_str().unwrap(),
        )))
        .unwrap()
    }

    fn ledger(connection: &Connection) -> (String, String) {
        (connection.query_row("SELECT json_array(modified_ns,size,file_id,parsed_bytes,append_guard,session_count,sealed,request_context,migration_state,snapshot_cursor) FROM native_usage_sources", [], |r| r.get(0)).unwrap(),
         connection.query_row("SELECT json_array(prompt_tokens,cached_input_tokens,completion_tokens,estimated_prompt_tokens,estimated_completion_tokens,explicit_records,estimated_records,message_count,request_count,token_unavailable_requests) FROM native_usage_source_days", [], |r| r.get(0)).unwrap())
    }

    #[test]
    fn force_refresh_reprojects_current_models_without_recounting_or_touching_sealed_days() {
        let (_fixture, params, window) = fixture(false);
        assert!(scan(&params, &window).total_tokens() > 0);
        let connection = open(&params);
        connection
            .execute(
                "UPDATE native_usage_source_models SET model='relay/default'",
                [],
            )
            .unwrap();
        connection.execute_batch(
            "INSERT INTO native_usage_daily_totals SELECT scope_key,'2026-07-14',prompt_tokens,cached_input_tokens,
                completion_tokens,estimated_prompt_tokens,estimated_completion_tokens,explicit_records,estimated_records,
                message_count,1,request_count,token_unavailable_requests FROM native_usage_source_days;
             INSERT INTO native_usage_daily_models SELECT scope_key,'2026-07-14',model,prompt_tokens,cached_input_tokens,
                completion_tokens,total_tokens,estimated_prompt_tokens,estimated_completion_tokens,effort,fast,
                request_count,token_unavailable_requests FROM native_usage_source_models;"
        ).unwrap();
        let before = ledger(&connection);
        let refreshed = scan(&params, &window);
        assert_eq!(
            refreshed.scan_cache.as_ref().unwrap()["identityRefreshedSources"],
            1
        );
        assert_eq!(ledger(&connection), before);
        let current: String = connection
            .query_row("SELECT model FROM native_usage_source_models", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(current, "actual-model");
        let sealed: String = connection
            .query_row("SELECT model FROM native_usage_daily_models", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(sealed, "relay/default");
        let repeated = scan(&params, &window);
        assert_eq!(repeated.total_tokens(), refreshed.total_tokens());
        assert_eq!(
            repeated.scan_cache.as_ref().unwrap()["identityRefreshedSources"],
            0
        );
        assert_eq!(ledger(&connection), before);
    }

    #[test]
    fn identity_targets_use_shared_placeholder_rules_for_qualified_and_structured_selectors() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch("CREATE TABLE native_usage_source_models(scope_key TEXT, source_key TEXT, day TEXT, model TEXT);").unwrap();
        for (source, raw, day) in [
            ("bare", "Default", "today"),
            ("qualified", "relay/default", "today"),
            (
                "structured",
                r#"{"id":"default","providerID":"relay"}"#,
                "today",
            ),
            ("malformed", r#"{"id":"default""#, "today"),
            (
                "known",
                r#"{"id":"actual-model","providerID":"relay"}"#,
                "today",
            ),
            ("sealed", "Default", "yesterday"),
        ] {
            connection
                .execute(
                    "INSERT INTO native_usage_source_models VALUES('scope',?1,?2,?3)",
                    params![source, day, raw],
                )
                .unwrap();
        }
        assert_eq!(
            targets(&connection, "scope", "today").unwrap(),
            ["bare", "qualified", "structured", "malformed"]
                .map(str::to_owned)
                .into_iter()
                .collect()
        );
    }

    #[test]
    fn force_refresh_preserves_unverifiable_counters_and_cumulative_or_migrated_cursors() {
        for reason in [
            "tokens",
            "requests",
            "model_cached",
            "variant",
            "cumulative",
            "migration",
        ] {
            let (_fixture, params, window) = fixture(reason == "cumulative");
            assert!(scan(&params, &window).total_tokens() > 0);
            let connection = open(&params);
            connection
                .execute("UPDATE native_usage_source_models SET model='Default'", [])
                .unwrap();
            match reason {
                "tokens" => {
                    connection
                        .execute(
                            "UPDATE native_usage_source_days SET prompt_tokens=prompt_tokens+1",
                            [],
                        )
                        .unwrap();
                }
                "requests" => {
                    connection
                        .execute(
                            "UPDATE native_usage_source_days SET request_count=request_count+1",
                            [],
                        )
                        .unwrap();
                }
                "model_cached" => {
                    connection.execute("UPDATE native_usage_source_models SET cached_input_tokens=cached_input_tokens+1", []).unwrap();
                }
                "variant" => {
                    connection
                        .execute("UPDATE native_usage_source_models SET effort='high'", [])
                        .unwrap();
                }
                "migration" => {
                    connection
                        .execute("UPDATE native_usage_sources SET migration_state=2", [])
                        .unwrap();
                }
                _ => {}
            }
            let before = ledger(&connection);
            let refreshed = scan(&params, &window);
            assert_eq!(
                refreshed.scan_cache.as_ref().unwrap()["identitySkippedSources"],
                1,
                "{reason}"
            );
            assert_eq!(ledger(&connection), before, "{reason}");
            let model: String = connection
                .query_row("SELECT model FROM native_usage_source_models", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(model, "Default", "{reason}");
        }
    }
}
