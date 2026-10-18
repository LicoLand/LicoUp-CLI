use super::{opaque_scope, positive, read_only_connection, sqlite_table_exists, table_columns};
use crate::domain::agent_usage::agent_usage_native::models::{
    CumulativeSnapshot, CumulativeTotals, ParseResult,
};
use crate::domain::conversation::usage::extract_token_usage;
use licoup_statistics::attribution::message_usage;
use licoup_statistics::contract::{HistoryUsageSummary, MessageUsage, UsageVariant};
use licoup_statistics::variant::{
    model_label, model_selection, model_with_provider_fallback, prefer_recorded_model,
};
use licoup_statistics::window::UsageWindow;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

pub(super) fn parse_openagent_usage_database(
    path: &Path,
    calendar: &UsageWindow,
) -> Option<ParseResult> {
    let connection = read_only_connection(path)?;
    parse_openagent_connection(&connection, calendar)
}

fn parse_openagent_connection(
    connection: &rusqlite::Connection,
    calendar: &UsageWindow,
) -> Option<ParseResult> {
    let has_sessions = sqlite_table_exists(connection, "session");
    let has_messages = sqlite_table_exists(connection, "message");
    if !has_sessions && !has_messages {
        return None;
    }
    let mut summary = HistoryUsageSummary::default();
    let mut sessions = BTreeSet::<String>::new();
    let mut cumulative_snapshots = Vec::new();

    if has_sessions {
        collect_session_cumulative_metadata(connection, calendar, &mut cumulative_snapshots)?;
    }
    let mut exact_usage_sessions = BTreeMap::<String, CumulativeTotals>::new();
    if has_messages {
        collect_message_usage_metadata(
            connection,
            calendar,
            &mut summary,
            &mut sessions,
            &mut exact_usage_sessions,
        )?;
    }
    for snapshot in &mut cumulative_snapshots {
        let Some(exact) = exact_usage_sessions.get(&snapshot.session_key) else {
            continue;
        };
        // Message counters identify the actual request variants. Session totals
        // can only fill a same-day gap, never replace those individual facts.
        if snapshot.first_day == snapshot.observed_day && snapshot.totals.at_least(*exact) {
            let residual = snapshot.totals.delta(*exact);
            if residual.cached <= residual.prompt
                && residual.prompt.saturating_add(residual.completion) > 0
            {
                summary.add(
                    MessageUsage {
                        prompt_tokens: residual.prompt,
                        cached_input_tokens: residual.cached,
                        completion_tokens: residual.completion,
                        total_tokens: residual.prompt.saturating_add(residual.completion),
                        model: snapshot.model.clone(),
                        variant: UsageVariant::default(),
                        accuracy: Default::default(),
                    },
                    Some(snapshot.observed_day.clone()),
                );
            }
        }
        // Keep the complete cumulative watermark, but do not add it again.
        // A cross-day gap has no exact date and cannot be assigned to today.
        snapshot.projects_usage = false;
    }
    summary.session_count = sessions.len() as u64;
    summary.message_count = summary.explicit_records;
    Some(ParseResult {
        summary,
        cumulative_snapshots,
        ..ParseResult::default()
    })
}

fn collect_session_cumulative_metadata(
    connection: &rusqlite::Connection,
    calendar: &UsageWindow,
    cumulative_snapshots: &mut Vec<CumulativeSnapshot>,
) -> Option<()> {
    let columns = table_columns(connection, "session");
    let required = [
        "id",
        "time_created",
        "time_updated",
        "tokens_input",
        "tokens_output",
        "tokens_reasoning",
        "tokens_cache_read",
        "tokens_cache_write",
    ];
    if required.iter().any(|column| !columns.contains(*column)) {
        return Some(());
    }
    let time_filter = epoch_numeric_filter(calendar, "time_updated");
    let sql = format!(
        "SELECT id,time_created,time_updated,tokens_input,
                tokens_output,tokens_reasoning,tokens_cache_read,tokens_cache_write
         FROM session{time_filter}"
    );
    let mut statement = connection.prepare(&sql).ok()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                row.get::<_, Option<i64>>(5)?.unwrap_or(0),
                row.get::<_, Option<i64>>(6)?.unwrap_or(0),
                row.get::<_, Option<i64>>(7)?.unwrap_or(0),
            ))
        })
        .ok()?;
    for (id, created, updated, input, output, reasoning, cache_read, cache_write) in rows.flatten()
    {
        let Some(id) = id else {
            continue;
        };
        let (Some(created), Some(updated)) = (created, updated) else {
            continue;
        };
        let (Some(created_day), Some(updated_day)) = (
            calendar.date_key(&created.to_string()),
            calendar.date_key(&updated.to_string()),
        ) else {
            continue;
        };
        if !calendar.contains(&updated_day) {
            continue;
        }
        let prompt = positive(input)
            .saturating_add(positive(cache_read))
            .saturating_add(positive(cache_write));
        let completion = positive(output).saturating_add(positive(reasoning));
        if prompt == 0 && completion == 0 {
            continue;
        }
        let session_key = opaque_scope(&id);
        cumulative_snapshots.push(CumulativeSnapshot {
            usage_key: session_key.clone(),
            session_key,
            // Session counters span model changes; its final selector is not
            // historical per-model attribution.
            model: None,
            variant: UsageVariant::default(),
            first_day: created_day.clone(),
            observed_day: updated_day.clone(),
            totals: CumulativeTotals {
                prompt,
                cached: positive(cache_read).min(prompt),
                completion,
            },
            projects_usage: true,
        });
    }
    Some(())
}

fn collect_message_usage_metadata(
    connection: &rusqlite::Connection,
    calendar: &UsageWindow,
    summary: &mut HistoryUsageSummary,
    sessions: &mut BTreeSet<String>,
    exact_usage_sessions: &mut BTreeMap<String, CumulativeTotals>,
) -> Option<()> {
    let columns = table_columns(connection, "message");
    if ["session_id", "time_created", "data"]
        .iter()
        .any(|column| !columns.contains(*column))
    {
        return Some(());
    }
    let has_id = columns.contains("id");
    let order = if has_id {
        "m.time_created ASC,m.id ASC"
    } else {
        "m.time_created ASC"
    };
    let id_column = if has_id { "m.id" } else { "NULL" };
    let time_filter = epoch_numeric_filter(calendar, "m.time_created");
    let message_metadata = metadata_sql("m");
    // Only an exact parent ID in the same native session can supply missing
    // request options; a session's final model/effort is not a turn context.
    let (parent_join, parent_metadata) = if has_id {
        (
            " LEFT JOIN message p ON p.session_id=m.session_id AND p.id=(
            CASE WHEN json_valid(CAST(m.data AS TEXT)) THEN COALESCE(
                json_extract(CAST(m.data AS TEXT), '$.parentID'),
                json_extract(CAST(m.data AS TEXT), '$.parentId'),
                json_extract(CAST(m.data AS TEXT), '$.parent_id')) END)",
            metadata_sql("p"),
        )
    } else {
        ("", "NULL".to_owned())
    };
    let sql = format!(
        "SELECT m.session_id,
                CAST(COALESCE(
                  CASE WHEN json_valid(CAST(m.data AS TEXT))
                    THEN json_extract(CAST(m.data AS TEXT), '$.time.created') END,
                  m.time_created,
                  CASE WHEN json_valid(CAST(m.data AS TEXT))
                    THEN json_extract(CAST(m.data AS TEXT), '$.createdAt') END,
                  CASE WHEN json_valid(CAST(m.data AS TEXT))
                    THEN json_extract(CAST(m.data AS TEXT), '$.timestamp') END
                ) AS TEXT),
                CASE WHEN json_valid(CAST(m.data AS TEXT)) THEN CAST(COALESCE(
                  json_extract(CAST(m.data AS TEXT), '$.usage'),
                  json_extract(CAST(m.data AS TEXT), '$.tokens'),
                  json_extract(CAST(m.data AS TEXT), '$.tokenUsage'),
                  json_extract(CAST(m.data AS TEXT), '$.token_usage'),
                  json_extract(CAST(m.data AS TEXT), '$.usageMetadata'),
                  json_extract(CAST(m.data AS TEXT), '$.usage_metadata'),
                  json_extract(CAST(m.data AS TEXT), '$.responseUsage'),
                  json_extract(CAST(m.data AS TEXT), '$.response_usage'),
                  json_extract(CAST(m.data AS TEXT), '$.tokenCount'),
                  json_extract(CAST(m.data AS TEXT), '$.\"gen_ai.usage\"')
                ) AS TEXT) END,
                {message_metadata}, {parent_metadata}, {id_column}
         FROM message m{parent_join}{time_filter} ORDER BY {order}"
    );
    let mut statement = connection.prepare(&sql).ok()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })
        .ok()?;
    let mut seen_messages = HashSet::new();
    for (session_id, timestamp, usage_json, metadata, parent_metadata, message_id) in rows.flatten()
    {
        let (Some(timestamp), Some(usage_json)) = (timestamp, usage_json) else {
            continue;
        };
        let Some(day) = calendar.date_key(&timestamp) else {
            continue;
        };
        if !calendar.contains(&day) {
            continue;
        }
        let Ok(raw_usage) = serde_json::from_str::<Value>(&usage_json) else {
            continue;
        };
        let Some(normalized) = extract_token_usage(&raw_usage) else {
            continue;
        };
        let mut metadata = decode_metadata(metadata.as_deref());
        metadata["usage"] = raw_usage;
        let parent_metadata = decode_metadata(parent_metadata.as_deref());
        let recorded_model = selected_model(&metadata);
        let direct_model = prefer_recorded_model(
            selected_model(&metadata["usage"]).map(|model| {
                recorded_model
                    .as_deref()
                    .map(|request| model_with_provider_fallback(&model, request))
                    .unwrap_or(model)
            }),
            recorded_model,
        );
        let parent_model = selected_model(&parent_metadata);
        let variant = request_variant(&metadata, direct_model.as_deref())
            .with_fallback(&request_variant(&parent_metadata, parent_model.as_deref()));
        let model = prefer_recorded_model(direct_model, parent_model.clone()).map(|model| {
            parent_model
                .as_deref()
                .map(|request| model_with_provider_fallback(&model, request))
                .unwrap_or(model)
        });
        let envelope = json!({"model": model, "usage": normalized});
        let Some(mut usage) = message_usage(&envelope, model) else {
            continue;
        };
        usage.variant = variant;
        if let Some(message_id) = message_id
            && !seen_messages.insert((session_id.clone(), message_id))
        {
            continue;
        }
        let session_key = session_id.as_deref().map(opaque_scope);
        if let Some(session_key) = session_key.as_ref() {
            let exact = exact_usage_sessions.entry(session_key.clone()).or_default();
            exact.prompt = exact.prompt.saturating_add(usage.prompt_tokens);
            exact.cached = exact.cached.saturating_add(usage.cached_input_tokens);
            exact.completion = exact.completion.saturating_add(usage.completion_tokens);
        }
        summary.add(usage, Some(day));
        if let Some(session_key) = session_key {
            sessions.insert(session_key);
        }
    }
    Some(())
}

// Extract structured request metadata only. JSON's array form preserves booleans
// and model objects; message text and tool payloads never leave SQLite here.
const METADATA_KEYS: &[&str] = &[
    "model",
    "modelID",
    "modelId",
    "model_id",
    "modelName",
    "providerID",
    "providerId",
    "provider_id",
    "variant",
    "reasoning_effort",
    "reasoningEffort",
    "effort",
    "thinking_level",
    "thinkingLevel",
    "fast",
    "fast_mode",
    "fastMode",
    "service_tier",
    "serviceTier",
    "metadata",
    "options",
    "reasoning",
    "request",
    "generationConfig",
    "output_config",
];

fn metadata_sql(table: &str) -> String {
    let paths = METADATA_KEYS
        .iter()
        .map(|key| format!("'$.{key}'"))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "CASE WHEN json_valid(CAST({table}.data AS TEXT)) THEN json_extract(CAST({table}.data AS TEXT),{paths}) END"
    )
}

fn decode_metadata(raw: Option<&str>) -> Value {
    let values = raw
        .and_then(|raw| serde_json::from_str::<Vec<Value>>(raw).ok())
        .unwrap_or_default();
    Value::Object(
        METADATA_KEYS
            .iter()
            .zip(values)
            .filter(|(_, value)| !value.is_null())
            .map(|(key, value)| ((*key).to_owned(), value))
            .collect(),
    )
}

fn selected_model(metadata: &Value) -> Option<String> {
    model_label(metadata)
}

fn request_variant(metadata: &Value, model: Option<&str>) -> UsageVariant {
    let mut variant = UsageVariant::from_metadata(metadata);
    if let Some(model) = model {
        variant = variant.with_fallback(&model_selection(model).variant);
    }
    variant
}

fn epoch_numeric_filter(calendar: &UsageWindow, column: &str) -> String {
    calendar
        .coarse_epoch_millis_bounds()
        .map(|(lower_ms, upper_ms)| {
            let lower_s = lower_ms / 1_000;
            let upper_s = upper_ms / 1_000;
            let lower_us = lower_ms.saturating_mul(1_000);
            let upper_us = upper_ms.saturating_mul(1_000);
            let lower_ns = lower_ms.saturating_mul(1_000_000);
            let upper_ns = upper_ms.saturating_mul(1_000_000);
            format!(
                " WHERE ({column}>={lower_s} AND {column}<{upper_s})
                    OR ({column}>={lower_ms} AND {column}<{upper_ms})
                    OR ({column}>={lower_us} AND {column}<{upper_us})
                    OR ({column}>={lower_ns} AND {column}<{upper_ns})"
            )
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{Connection, params};

    const TODAY: i64 = 1_784_080_800_000;
    const YESTERDAY: i64 = 1_783_994_400_000;

    fn calendar() -> UsageWindow {
        UsageWindow::from_params(&json!({"now":"2026-07-15T12:00:00Z", "historyDays":1}))
    }

    fn database() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE session (
                id TEXT PRIMARY KEY, model TEXT, time_created INTEGER, time_updated INTEGER,
                tokens_input INTEGER, tokens_output INTEGER, tokens_reasoning INTEGER,
                tokens_cache_read INTEGER, tokens_cache_write INTEGER
             );
             CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT);
             CREATE INDEX message_identity ON message(session_id,id);",
            )
            .unwrap();
        connection
    }

    fn message(connection: &Connection, id: &str, session: &str, time: i64, data: Value) {
        connection
            .execute(
                "INSERT INTO message VALUES(?1,?2,?3,?4)",
                params![id, session, time, data.to_string()],
            )
            .unwrap();
    }

    fn totals(parsed: &ParseResult) -> BTreeMap<(String, UsageVariant), u64> {
        let mut totals = BTreeMap::new();
        for day in parsed.summary.daily_usage.values() {
            for ((model, variant), usage) in &day.model_variants {
                *totals
                    .entry((
                        model_selection(model).unresolved_identity(),
                        variant.clone(),
                    ))
                    .or_default() += usage.total_tokens;
            }
        }
        totals
    }

    #[test]
    fn placeholder_models_use_exact_parent_or_same_usage_record_only() {
        let connection = database();
        message(
            &connection,
            "request",
            "s",
            TODAY,
            json!({"modelID":"actual-parent","variant":"high"}),
        );
        message(
            &connection,
            "one",
            "s",
            TODAY + 1,
            json!({"parentID":"request","modelID":"default","tokens":{"input":10,"output":2}}),
        );
        message(
            &connection,
            "two",
            "s",
            TODAY + 2,
            json!({"modelID":"auto","tokens":{"model":"actual-response","input":6,"output":2}}),
        );
        message(
            &connection,
            "wrong-parent",
            "different-session",
            TODAY,
            json!({"modelID":"unrelated-model"}),
        );
        message(
            &connection,
            "three",
            "s",
            TODAY + 3,
            json!({"parentID":"wrong-parent","modelID":"unknown","tokens":{"input":3,"output":2}}),
        );
        let parsed = parse_openagent_connection(&connection, &calendar()).unwrap();
        assert_eq!(parsed.summary.total_tokens(), 25);
        let models = &parsed
            .summary
            .to_json_for_agent(None, &crate::domain::model_registry::snapshot())["dailyUsage"][0]["modelTokenUsage"];
        assert_eq!(
            models["actual-parent"]["variants"]["High"]["totalTokens"],
            12
        );
        assert_eq!(models["actual-response"]["totalTokens"], 8);
        assert_eq!(models["Others"]["totalTokens"], 5);
        assert!(models.get("unrelated-model").is_none());
    }

    #[test]
    fn same_day_variants_preserve_exact_messages_and_only_add_unattributed_residual() {
        let connection = database();
        let session_model =
            json!({"providerID":"synthetic","modelID":"model-a","variant":"ultra","fast":true});
        connection
            .execute(
                "INSERT INTO session VALUES('s',?1,?2,?2,100,20,0,10,0)",
                params![session_model.to_string(), TODAY],
            )
            .unwrap();
        message(
            &connection,
            "request",
            "s",
            TODAY,
            json!({"model":{"providerID":"synthetic","modelID":"model-a","variant":"high"},"fast":true}),
        );
        message(
            &connection,
            "one",
            "s",
            TODAY + 1,
            json!({"parentID":"request","modelID":"model-a","providerID":"synthetic","tokens":{"input":20,"output":4,"cache":{"read":2}}}),
        );
        let low = json!({"model":json!({"providerID":"synthetic","modelID":"model-a","variant":"low","fast":false}).to_string(),"tokens":{"input":30,"output":6,"cache":{"read":3}}});
        message(&connection, "two", "s", TODAY + 2, low.clone());
        message(&connection, "two", "s", TODAY + 2, low);
        message(
            &connection,
            "three",
            "s",
            TODAY + 3,
            json!({"model":{"providerID":"synthetic","modelID":"model-b","variant":"max"},"reasoning_effort":"xhigh","tokens":{"input":10,"output":2,"cache":{"read":1}}}),
        );
        message(
            &connection,
            "four",
            "s",
            TODAY + 4,
            json!({"tokens":{"input":10,"output":2},"thinking":"ultra"}),
        );

        let parsed = parse_openagent_connection(&connection, &calendar()).unwrap();
        assert_eq!(parsed.summary.total_tokens(), 130);
        assert_eq!(parsed.summary.prompt_tokens(), 110);
        assert_eq!(parsed.summary.completion_tokens(), 20);
        assert_eq!(parsed.summary.explicit_cached_input_tokens, 10);
        assert_eq!(parsed.summary.session_count, 1);
        assert_eq!(parsed.summary.explicit_records, 5);
        assert_eq!(
            totals(&parsed),
            BTreeMap::from([
                (
                    (
                        "synthetic/model-a".to_owned(),
                        UsageVariant {
                            effort: Some("high".to_owned()),
                            fast: Some(true)
                        }
                    ),
                    26
                ),
                (
                    (
                        "synthetic/model-a".to_owned(),
                        UsageVariant {
                            effort: Some("low".to_owned()),
                            fast: Some(false)
                        }
                    ),
                    39
                ),
                (
                    (
                        "synthetic/model-b".to_owned(),
                        UsageVariant {
                            effort: Some("xhigh".to_owned()),
                            fast: None
                        }
                    ),
                    13
                ),
                (("Others".to_owned(), UsageVariant::default()), 52),
            ])
        );
        let snapshot = &parsed.cumulative_snapshots[0];
        assert!(!snapshot.projects_usage);
        assert_eq!(snapshot.variant, UsageVariant::default());
        assert_eq!(snapshot.model, None);
        assert_eq!(snapshot.totals.prompt, 110);
        connection
            .execute(
                "UPDATE session SET model=?1",
                [json!({"modelID":"model-b","variant":"low"}).to_string()],
            )
            .unwrap();
        let updated = parse_openagent_connection(&connection, &calendar()).unwrap();
        assert_eq!(
            snapshot.usage_key,
            updated.cumulative_snapshots[0].usage_key
        );
    }

    #[test]
    fn explicit_parent_outside_window_supplies_options_without_cross_session_or_cross_day_guessing()
    {
        let connection = database();
        connection
            .execute(
                "INSERT INTO session VALUES('s','model-a',?1,?2,999,999,0,0,0)",
                params![YESTERDAY, TODAY],
            )
            .unwrap();
        message(
            &connection,
            "request",
            "s",
            YESTERDAY,
            json!({"model":"model-a","variant":"high","fast":false}),
        );
        message(
            &connection,
            "request",
            "other",
            YESTERDAY,
            json!({"model":"wrong-model","variant":"ultra","fast":true}),
        );
        message(
            &connection,
            "answer",
            "s",
            TODAY,
            json!({"parentID":"request","tokens":{"input":6,"output":2}}),
        );
        let parsed = parse_openagent_connection(&connection, &calendar()).unwrap();
        assert_eq!(parsed.summary.total_tokens(), 8);
        assert_eq!(parsed.summary.explicit_records, 1);
        assert_eq!(
            totals(&parsed),
            BTreeMap::from([(
                (
                    "model-a".to_owned(),
                    UsageVariant {
                        effort: Some("high".to_owned()),
                        fast: Some(false)
                    }
                ),
                8
            )])
        );
        assert!(!parsed.cumulative_snapshots[0].projects_usage);
        assert_eq!(parsed.cumulative_snapshots[0].totals.prompt, 999);
    }

    #[test]
    fn inconsistent_session_components_do_not_create_a_larger_combined_total() {
        for (input, output, cached, message_input, message_output) in [
            (100_i64, 20_i64, 0_i64, 80_i64, 30_i64),
            // A positive cached residual cannot exceed its prompt residual.
            (40, 20, 60, 80, 20),
        ] {
            let connection = database();
            connection
                .execute(
                    "INSERT INTO session VALUES('s','model-a',?1,?1,?2,?3,0,?4,0)",
                    params![TODAY, input, output, cached],
                )
                .unwrap();
            message(
                &connection,
                "answer",
                "s",
                TODAY,
                json!({"model":"model-a","variant":"high","tokens":{"input":message_input,"output":message_output}}),
            );
            let parsed = parse_openagent_connection(&connection, &calendar()).unwrap();
            assert_eq!(parsed.summary.prompt_tokens(), message_input as u64);
            assert_eq!(parsed.summary.completion_tokens(), message_output as u64);
            assert_eq!(
                parsed.summary.total_tokens(),
                (message_input + message_output) as u64
            );
            assert_eq!(parsed.summary.explicit_records, 1);
            assert!(!parsed.cumulative_snapshots[0].projects_usage);
            assert_eq!(
                parsed.cumulative_snapshots[0].totals,
                CumulativeTotals {
                    prompt: (input + cached) as u64,
                    cached: cached as u64,
                    completion: output as u64
                }
            );
        }
    }

    #[test]
    fn session_only_counters_keep_one_watermark_without_inventing_model_or_effort() {
        let connection = database();
        connection
            .execute(
                "INSERT INTO session VALUES('s',?1,?2,?2,20,3,2,4,1)",
                params![
                    json!({"modelID":"model-a","variant":"high"}).to_string(),
                    TODAY
                ],
            )
            .unwrap();
        let parsed = parse_openagent_connection(&connection, &calendar()).unwrap();
        assert_eq!(parsed.summary.total_tokens(), 0);
        assert_eq!(parsed.cumulative_snapshots.len(), 1);
        let snapshot = &parsed.cumulative_snapshots[0];
        assert!(snapshot.projects_usage);
        assert_eq!(snapshot.model, None);
        assert_eq!(snapshot.variant, UsageVariant::default());
        assert_eq!(
            snapshot.totals,
            CumulativeTotals {
                prompt: 25,
                cached: 4,
                completion: 5
            }
        );
    }

    #[test]
    fn message_only_schema_keeps_usage_metadata_and_default_variant_unknown() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE message(session_id TEXT,time_created INTEGER,data TEXT);")
            .unwrap();
        for data in [
            json!({"model":{"providerID":"synthetic","modelID":"model-a","variant":"default"},"tokens":{"input":4,"output":1,"reasoning_effort":"medium"},"fast":false}),
            json!({"model":"requested-model","usage":{"model":"model-a","promptTokens":2,"completionTokens":1},"variant":"default","thinking":"high"}),
            json!({"model":"model-a","tokens":{"input":1,"output":1},"variant":"custom-balanced"}),
        ] {
            connection
                .execute(
                    "INSERT INTO message VALUES('s',?1,?2)",
                    params![TODAY, data.to_string()],
                )
                .unwrap();
        }
        let parsed = parse_openagent_connection(&connection, &calendar()).unwrap();
        assert_eq!(parsed.summary.total_tokens(), 10);
        assert!(parsed.cumulative_snapshots.is_empty());
        assert_eq!(
            totals(&parsed),
            BTreeMap::from([
                (
                    (
                        "synthetic/model-a".to_owned(),
                        UsageVariant {
                            effort: Some("medium".to_owned()),
                            fast: Some(false)
                        }
                    ),
                    5
                ),
                (("model-a".to_owned(), UsageVariant::default()), 5),
            ])
        );
    }

    #[test]
    fn message_provider_namespace_survives_model_ids_with_slashes() {
        let connection = database();
        for (id, provider, input) in [("one", "relay-a", 7), ("two", "relay-b", 11)] {
            message(
                &connection,
                id,
                "s",
                TODAY,
                json!({"modelID":"lab/flash-latest","providerID":provider,"effort":"high","usage":{"model":"lab/flash-latest","promptTokens":input,"completionTokens":2}}),
            );
        }
        let parsed = parse_openagent_connection(&connection, &calendar()).unwrap();
        assert_eq!(parsed.summary.total_tokens(), 22);
        let observed = totals(&parsed);
        let high = UsageVariant {
            effort: Some("high".to_owned()),
            fast: None,
        };
        assert_eq!(
            observed[&("relay-a/lab/flash-latest".to_owned(), high.clone())],
            9
        );
        assert_eq!(observed[&("relay-b/lab/flash-latest".to_owned(), high)], 13);
    }
}
