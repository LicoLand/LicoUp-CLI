use super::{opaque_scope, positive, read_only_connection, table_columns};
use crate::domain::agent_usage::agent_usage_native::models::{
    CumulativeSnapshot, CumulativeTotals, ParseResult,
};
use licoup_statistics::contract::{HistoryUsageSummary, MessageUsage, UsageAccuracy};
use licoup_statistics::window::UsageWindow;
use rusqlite::Connection;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct TokenCounters {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
}

impl TokenCounters {
    fn from_sql(input: i64, output: i64, cache_read: i64, cache_write: i64) -> Self {
        Self {
            input: positive(input),
            output: positive(output),
            cache_read: positive(cache_read),
            cache_write: positive(cache_write),
        }
    }

    fn add(self, other: Self) -> Self {
        Self {
            input: self.input.saturating_add(other.input),
            output: self.output.saturating_add(other.output),
            cache_read: self.cache_read.saturating_add(other.cache_read),
            cache_write: self.cache_write.saturating_add(other.cache_write),
        }
    }

    fn residual(self, attributed: Self) -> Self {
        Self {
            input: self.input.saturating_sub(attributed.input),
            output: self.output.saturating_sub(attributed.output),
            cache_read: self.cache_read.saturating_sub(attributed.cache_read),
            cache_write: self.cache_write.saturating_sub(attributed.cache_write),
        }
    }

    fn cumulative(self) -> CumulativeTotals {
        CumulativeTotals {
            prompt: self
                .input
                .saturating_add(self.cache_read)
                .saturating_add(self.cache_write),
            cached: self.cache_read,
            completion: self.output,
        }
    }
}

#[derive(Clone, Debug)]
struct GatewayUsageRow {
    session: String,
    model: Option<String>,
    first_day: String,
    observed_day: String,
    counters: TokenCounters,
    residual: bool,
}

pub(super) fn parse_hermes_usage_database(
    path: &Path,
    calendar: &UsageWindow,
) -> Option<ParseResult> {
    let connection = read_only_connection(path)?;
    let session_columns = table_columns(&connection, "sessions");
    let usage_columns = table_columns(&connection, "session_model_usage");
    let message_columns = table_columns(&connection, "messages");
    let required_session_columns = [
        "id",
        "model",
        "started_at",
        "ended_at",
        "input_tokens",
        "output_tokens",
        "cache_read_tokens",
        "cache_write_tokens",
    ];
    let required_usage_columns = [
        "session_id",
        "model",
        "input_tokens",
        "output_tokens",
        "cache_read_tokens",
        "cache_write_tokens",
        "first_seen",
        "last_seen",
    ];
    if required_session_columns
        .iter()
        .any(|column| !session_columns.contains(*column))
        || required_usage_columns
            .iter()
            .any(|column| !usage_columns.contains(*column))
        || !message_columns.contains("session_id")
        || !message_columns.contains("timestamp")
    {
        return None;
    }

    let (mut rows, attributed) = read_model_usage(&connection, calendar)?;
    read_session_residuals(&connection, calendar, &attributed, &mut rows)?;
    Some(project_rows(rows, calendar))
}

fn read_model_usage(
    connection: &Connection,
    calendar: &UsageWindow,
) -> Option<(Vec<GatewayUsageRow>, BTreeMap<String, TokenCounters>)> {
    let mut statement = connection
        .prepare(
            "SELECT u.session_id, u.model,
                    SUM(COALESCE(u.input_tokens, 0)),
                    SUM(COALESCE(u.output_tokens, 0)),
                    SUM(COALESCE(u.cache_read_tokens, 0)),
                    SUM(COALESCE(u.cache_write_tokens, 0)),
                    COALESCE(MIN(u.first_seen), s.started_at),
                    COALESCE(
                        MAX(u.last_seen),
                        (SELECT MAX(m.timestamp) FROM messages m WHERE m.session_id = u.session_id),
                        s.ended_at,
                        s.started_at
                    )
             FROM session_model_usage u
             JOIN sessions s ON s.id = u.session_id
             GROUP BY u.session_id, u.model",
        )
        .ok()?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                row.get::<_, Option<i64>>(5)?.unwrap_or(0),
                row.get::<_, Option<f64>>(6)?,
                row.get::<_, Option<f64>>(7)?,
            ))
        })
        .ok()?
        .collect::<rusqlite::Result<Vec<_>>>()
        .ok()?;
    let mut projected = Vec::with_capacity(rows.len());
    let mut attributed = BTreeMap::<String, TokenCounters>::new();
    for (session, model, input, output, cache_read, cache_write, first, last) in rows {
        let counters = TokenCounters::from_sql(input, output, cache_read, cache_write);
        if counters == TokenCounters::default() {
            continue;
        }
        let first_day = epoch_day(calendar, first?)?;
        let observed_day = epoch_day(calendar, last?)?;
        attributed
            .entry(session.clone())
            .and_modify(|totals| *totals = totals.add(counters))
            .or_insert(counters);
        projected.push(GatewayUsageRow {
            session,
            model: normalize_model(model),
            first_day,
            observed_day,
            counters,
            residual: false,
        });
    }
    Some((projected, attributed))
}

fn read_session_residuals(
    connection: &Connection,
    calendar: &UsageWindow,
    attributed: &BTreeMap<String, TokenCounters>,
    rows: &mut Vec<GatewayUsageRow>,
) -> Option<()> {
    let mut statement = connection
        .prepare(
            "SELECT s.id, s.model, s.started_at,
                    COALESCE(
                        (SELECT MAX(m.timestamp) FROM messages m WHERE m.session_id = s.id),
                        s.ended_at,
                        s.started_at
                    ),
                    COALESCE(s.input_tokens, 0),
                    COALESCE(s.output_tokens, 0),
                    COALESCE(s.cache_read_tokens, 0),
                    COALESCE(s.cache_write_tokens, 0)
             FROM sessions s",
        )
        .ok()?;
    let sessions = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, f64>(2)?,
                row.get::<_, f64>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
            ))
        })
        .ok()?
        .collect::<rusqlite::Result<Vec<_>>>()
        .ok()?;
    for (session, model, first, last, input, output, cache_read, cache_write) in sessions {
        let aggregate = TokenCounters::from_sql(input, output, cache_read, cache_write);
        let residual = aggregate.residual(attributed.get(&session).copied().unwrap_or_default());
        if residual == TokenCounters::default() {
            continue;
        }
        rows.push(GatewayUsageRow {
            session,
            model: normalize_model(model),
            first_day: epoch_day(calendar, first)?,
            observed_day: epoch_day(calendar, last)?,
            counters: residual,
            residual: true,
        });
    }
    Some(())
}

fn project_rows(rows: Vec<GatewayUsageRow>, calendar: &UsageWindow) -> ParseResult {
    let rebuild_all_history = calendar.days == u64::MAX;
    let mut summary = HistoryUsageSummary::default();
    let mut cumulative_snapshots = Vec::with_capacity(rows.len());
    let mut sessions = BTreeSet::new();
    for row in rows {
        if !calendar.contains(&row.observed_day) {
            continue;
        }
        let session_key = opaque_scope(&row.session);
        let totals = row.counters.cumulative();
        if rebuild_all_history && calendar.contains(&row.first_day) {
            summary.add(
                MessageUsage {
                    prompt_tokens: totals.prompt,
                    cached_input_tokens: totals.cached,
                    completion_tokens: totals.completion,
                    total_tokens: totals.prompt.saturating_add(totals.completion),
                    model: row.model.clone(),
                    variant: Default::default(),
                    accuracy: UsageAccuracy::Exact,
                },
                Some(row.first_day.clone()),
            );
            sessions.insert(session_key.clone());
        }
        let scope = if row.residual { "residual" } else { "model" };
        cumulative_snapshots.push(CumulativeSnapshot {
            usage_key: opaque_scope(&format!(
                "hermes\0{scope}\0{}\0{}",
                row.session,
                row.model.as_deref().unwrap_or_default()
            )),
            session_key,
            model: row.model,
            variant: Default::default(),
            first_day: row.first_day,
            observed_day: row.observed_day,
            totals,
            projects_usage: !rebuild_all_history,
        });
    }
    summary.session_count = sessions.len() as u64;
    summary.message_count = summary.explicit_records;
    ParseResult {
        summary,
        cumulative_snapshots,
        ..ParseResult::default()
    }
}

fn epoch_day(calendar: &UsageWindow, value: f64) -> Option<String> {
    if !value.is_finite() || value <= 0.0 || value > i64::MAX as f64 {
        return None;
    }
    calendar.date_key(&(value.floor() as i64).to_string())
}

fn normalize_model(model: Option<String>) -> Option<String> {
    model.filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use serde_json::json;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    fn temp_database() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "lico-hermes-gateway-usage-{}-{}.db",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn epoch(value: &str) -> f64 {
        OffsetDateTime::parse(value, &Rfc3339)
            .unwrap()
            .unix_timestamp() as f64
    }

    #[test]
    fn gateway_usage_merges_tasks_reconciles_session_residuals_and_ignores_reasoning_detail() {
        let path = temp_database();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    model TEXT,
                    started_at REAL NOT NULL,
                    ended_at REAL,
                    input_tokens INTEGER NOT NULL DEFAULT 0,
                    output_tokens INTEGER NOT NULL DEFAULT 0,
                    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                    cache_write_tokens INTEGER NOT NULL DEFAULT 0
                );
                CREATE TABLE messages (
                    session_id TEXT NOT NULL,
                    timestamp REAL NOT NULL
                );
                CREATE INDEX messages_session_timestamp
                    ON messages(session_id, timestamp);
                CREATE TABLE session_model_usage (
                    session_id TEXT NOT NULL,
                    model TEXT NOT NULL,
                    billing_provider TEXT NOT NULL DEFAULT '',
                    billing_base_url TEXT NOT NULL DEFAULT '',
                    billing_mode TEXT NOT NULL DEFAULT '',
                    task TEXT NOT NULL DEFAULT '',
                    input_tokens INTEGER NOT NULL DEFAULT 0,
                    output_tokens INTEGER NOT NULL DEFAULT 0,
                    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
                    reasoning_tokens INTEGER NOT NULL DEFAULT 0,
                    first_seen REAL,
                    last_seen REAL,
                    PRIMARY KEY (
                        session_id, model, billing_provider,
                        billing_base_url, billing_mode, task
                    )
                );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO sessions VALUES (?1, ?2, ?3, NULL, 25, 12, 4, 1)",
                rusqlite::params![
                    "synthetic-session",
                    "hermes-test",
                    epoch("2026-07-15T09:59:00Z")
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO messages VALUES (?1, ?2)",
                rusqlite::params!["synthetic-session", epoch("2026-07-15T10:00:04Z")],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO session_model_usage VALUES
                    (?1, ?2, '', '', '', '', 10, 5, 2, 0, 1, ?3, ?4),
                    (?1, ?2, '', '', '', 'compression', 7, 3, 1, 0, 2, ?5, ?6)",
                rusqlite::params![
                    "synthetic-session",
                    "hermes-test",
                    epoch("2026-07-15T10:00:00Z"),
                    epoch("2026-07-15T10:00:01Z"),
                    epoch("2026-07-15T10:00:02Z"),
                    epoch("2026-07-15T10:00:03Z")
                ],
            )
            .unwrap();
        drop(connection);

        let calendar = UsageWindow::from_params(&json!({"now": "2026-07-15T12:00:00Z"}));
        let parsed = parse_hermes_usage_database(&path, &calendar).unwrap();
        assert_eq!(parsed.cumulative_snapshots.len(), 2);
        assert!(
            parsed
                .cumulative_snapshots
                .iter()
                .all(|row| row.projects_usage)
        );
        assert!(parsed.cumulative_snapshots.iter().any(|row| {
            row.totals
                == CumulativeTotals {
                    prompt: 20,
                    cached: 3,
                    completion: 8,
                }
        }));
        assert!(parsed.cumulative_snapshots.iter().any(|row| {
            row.totals
                == CumulativeTotals {
                    prompt: 10,
                    cached: 1,
                    completion: 4,
                }
        }));

        let baseline = parse_hermes_usage_database(&path, &calendar.all_history()).unwrap();
        assert_eq!(baseline.summary.prompt_tokens(), 30);
        assert_eq!(baseline.summary.explicit_cached_input_tokens, 4);
        assert_eq!(baseline.summary.completion_tokens(), 12);
        assert_eq!(baseline.summary.total_tokens(), 42);
        assert_eq!(baseline.summary.explicit_records, 2);
        assert_eq!(baseline.summary.estimated_records, 0);
        assert_eq!(baseline.summary.session_count, 1);
        assert!(
            baseline
                .cumulative_snapshots
                .iter()
                .all(|row| !row.projects_usage)
        );

        fs::remove_file(path).unwrap();
    }
}
