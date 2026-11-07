use super::super::{
    ScanStats, apply_refresh_plan, cache,
    files::source_metadata,
    models::{PlannedSource, PlannedSourceAction, RefreshPlan},
    runtime::CacheRuntime,
    snapshot_cursor::SnapshotCursor,
};
use super::*;
use rusqlite::Connection;
use serde_json::json;
use std::fs;
use std::path::PathBuf;

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("native-stored-usage-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    writer: Connection,
    path: PathBuf,
    root: Directory,
}

impl Fixture {
    fn new() -> Self {
        let root = Directory::new();
        let path = root.0.join("state.db");
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch(
            "CREATE TABLE sessions(id TEXT PRIMARY KEY, model TEXT, started_at REAL, ended_at REAL,
               input_tokens INTEGER, output_tokens INTEGER, cache_read_tokens INTEGER,
               cache_write_tokens INTEGER, api_call_count INTEGER);
             CREATE TABLE session_model_usage(
               session_id TEXT, model TEXT, billing_provider TEXT, billing_base_url TEXT,
               billing_mode TEXT, task TEXT, first_seen REAL, last_seen REAL,
               input_tokens INTEGER, output_tokens INTEGER, cache_read_tokens INTEGER,
               cache_write_tokens INTEGER, api_call_count INTEGER,
               PRIMARY KEY(session_id,model,billing_provider,billing_base_url,billing_mode,task));
             CREATE TABLE messages(session_id TEXT, timestamp REAL);"
        ).unwrap();
        let fixture = Self { writer, path, root };
        fixture.populate();
        fixture
    }

    fn populate(&self) {
        self.writer.execute(
            "INSERT INTO sessions VALUES('synthetic-session','selected-model',?1,NULL,100,10,0,0,4)",
            [epoch("2026-07-15T08:00:00Z")],
        ).unwrap();
        self.writer
            .execute(
                "INSERT INTO session_model_usage VALUES(
               'synthetic-session','aux-model','synthetic-provider','synthetic-endpoint',
               'synthetic-mode','compression',?1,?2,20,5,0,0,1)",
                [epoch("2026-07-15T09:00:00Z"), epoch("2026-07-15T10:00:00Z")],
            )
            .unwrap();
        self.writer
            .execute(
                "INSERT INTO messages VALUES('synthetic-session',?1)",
                [epoch("2026-07-15T10:00:00Z")],
            )
            .unwrap();
    }

    fn read(
        &self,
        reader: &mut StoredSource,
        old: Option<&CachedSource>,
        calendar: &UsageWindow,
    ) -> Projection {
        read(
            reader,
            &self.path,
            old,
            &source_metadata(&self.path).unwrap(),
            calendar,
        )
        .unwrap()
    }

    fn cache_path(&self) -> PathBuf {
        self.root.0.join("usage-cache.sqlite")
    }
}

fn epoch(value: &str) -> f64 {
    OffsetDateTime::parse(value, &Rfc3339)
        .unwrap()
        .unix_timestamp() as f64
}

fn calendar() -> UsageWindow {
    UsageWindow::from_params(&json!({"now":"2026-07-15T12:00:00Z","historyDays":2}))
}

fn reader() -> StoredSource {
    StoredSource::new(declaration(HistoryAdapter::Hermes).unwrap()).unwrap()
}

fn cached(fixture: &Fixture, projection: &Projection, calendar: &UsageWindow) -> CachedSource {
    let metadata = source_metadata(&fixture.path).unwrap();
    CachedSource {
        modified_ns: metadata.modified_ns,
        size: metadata.size,
        file_id: metadata.file_id,
        session_count: projection.session_count,
        migration_state: if projection.gap { 5 } else { 4 },
        snapshot_cursor: Some(SnapshotCursor::for_stored(
            projection.cursor.clone(),
            calendar,
            projection.session_count,
        )),
        ..Default::default()
    }
}

fn counts(prompt: u64, completion: u64, requests: u64) -> Counts {
    Counts {
        prompt: Some(prompt),
        cached: Some(0),
        completion: Some(completion),
        total: Some(prompt + completion),
        requests: Some(requests),
    }
}

#[test]
fn projects_actual_requests_and_counts_each_increment_once() {
    let fixture = Fixture::new();
    let calendar = calendar();
    let mut reader = reader();
    let first = fixture.read(&mut reader, None, &calendar);
    assert_eq!(first.summary.explicit_total_tokens, 135);
    assert_eq!(first.summary.daily_usage[&calendar.end].request_count, 5);
    assert_eq!(first.summary.message_count, 5);
    assert_eq!(first.session_count, 1);
    assert!(!first.gap);
    let old = cached(&fixture, &first, &calendar);
    let encoded = serde_json::to_string(&old.snapshot_cursor).unwrap();
    assert!(!encoded.contains("synthetic-endpoint"));
    assert!(!encoded.contains("firstSeen"));
    assert_eq!(
        serde_json::from_str::<Option<SnapshotCursor>>(&encoded).unwrap(),
        old.snapshot_cursor
    );
    let repeated = fixture.read(&mut reader, Some(&old), &calendar);
    assert_eq!(repeated.summary.explicit_total_tokens, 0);
    assert!(repeated.summary.daily_usage.is_empty());
    assert_eq!(repeated.session_count, 1);
    fixture
        .writer
        .execute("UPDATE sessions SET input_tokens=104, api_call_count=5", [])
        .unwrap();
    let delta = fixture.read(&mut reader, Some(&old), &calendar);
    assert_eq!(delta.summary.explicit_total_tokens, 4);
    assert_eq!(delta.summary.daily_usage[&calendar.end].request_count, 1);
    let old = cached(&fixture, &delta, &calendar);
    fixture
        .writer
        .execute("UPDATE sessions SET api_call_count=6", [])
        .unwrap();
    let call = fixture.read(&mut reader, Some(&old), &calendar);
    assert_eq!(call.summary.explicit_total_tokens, 0);
    assert_eq!(call.summary.daily_usage[&calendar.end].request_count, 1);
    assert_eq!(call.session_count, 1);
}

#[test]
fn model_redistribution_does_not_create_consumption() {
    let fixture = Fixture::new();
    let calendar = calendar();
    let mut reader = reader();
    let first = fixture.read(&mut reader, None, &calendar);
    let old = cached(&fixture, &first, &calendar);
    fixture
        .writer
        .execute(
            "INSERT INTO session_model_usage VALUES(
           'synthetic-session','recorded-model','synthetic-provider','synthetic-endpoint',
           'synthetic-mode','',?1,?1,100,10,0,0,4)",
            [epoch("2026-07-15T10:00:00Z")],
        )
        .unwrap();
    let classified = fixture.read(&mut reader, Some(&old), &calendar);
    assert_eq!(classified.summary.explicit_total_tokens, 0);
    assert!(classified.summary.daily_usage.is_empty());
    assert!(classified.gap);
}

#[test]
fn resets_and_disappearing_sessions_never_replay_their_cumulative_amounts() {
    let fixture = Fixture::new();
    let calendar = calendar();
    let mut reader = reader();
    let first = fixture.read(&mut reader, None, &calendar);
    let old = cached(&fixture, &first, &calendar);
    fixture
        .writer
        .execute_batch(
            "UPDATE sessions SET input_tokens=50, output_tokens=0;
         UPDATE session_model_usage SET input_tokens=160, output_tokens=0;",
        )
        .unwrap();
    let reset = fixture.read(&mut reader, Some(&old), &calendar);
    assert_eq!(reset.summary.explicit_total_tokens, 0);
    assert!(reset.gap);
    let old = cached(&fixture, &reset, &calendar);
    fixture
        .writer
        .execute_batch(
            "DELETE FROM session_model_usage; DELETE FROM messages; DELETE FROM sessions;",
        )
        .unwrap();
    let missing = fixture.read(&mut reader, Some(&old), &calendar);
    assert!(!missing.cursor.sessions["synthetic-session"].present);
    assert_eq!(
        missing.cursor.sessions["synthetic-session"]
            .value
            .counts
            .total,
        Some(210)
    );
    let old = cached(&fixture, &missing, &calendar);
    fixture.populate();
    let restored = fixture.read(&mut reader, Some(&old), &calendar);
    assert_eq!(restored.summary.explicit_total_tokens, 0);
    assert!(restored.gap);
    let old = cached(&fixture, &restored, &calendar);
    fixture
        .writer
        .execute("UPDATE sessions SET input_tokens=104, api_call_count=5", [])
        .unwrap();
    assert_eq!(
        fixture
            .read(&mut reader, Some(&old), &calendar)
            .summary
            .explicit_total_tokens,
        4
    );
}

#[test]
fn ambiguous_dates_establish_watermarks_without_inventing_a_consumption_day() {
    let fixture = Fixture::new();
    fixture
        .writer
        .execute(
            "UPDATE sessions SET started_at=?1",
            [epoch("2026-07-14T08:00:00Z")],
        )
        .unwrap();
    let calendar = calendar();
    let mut reader = reader();
    let first = fixture.read(&mut reader, None, &calendar);
    assert_eq!(first.summary.explicit_total_tokens, 25);
    assert!(first.gap);
    assert!(first.cursor.time_inferred);
    assert_eq!(
        first.cursor.sessions["synthetic-session"]
            .value
            .counts
            .total,
        Some(135)
    );
    let old = cached(&fixture, &first, &calendar);
    fixture
        .writer
        .execute("UPDATE sessions SET input_tokens=104, api_call_count=5", [])
        .unwrap();
    let delta = fixture.read(&mut reader, Some(&old), &calendar);
    assert_eq!(delta.summary.explicit_total_tokens, 4);
    let old = cached(&fixture, &delta, &calendar);
    let tomorrow = UsageWindow::from_params(&json!({"now":"2026-07-16T12:00:00Z"}));
    fixture
        .writer
        .execute("UPDATE sessions SET input_tokens=114, api_call_count=6", [])
        .unwrap();
    let uncertain = fixture.read(&mut reader, Some(&old), &tomorrow);
    assert_eq!(uncertain.summary.explicit_total_tokens, 0);
    assert!(uncertain.gap);
    assert_eq!(
        uncertain.cursor.sessions["synthetic-session"]
            .value
            .counts
            .total,
        Some(149)
    );
}

#[test]
fn explicit_allocation_dates_use_the_host_timezone_without_rereading_history() {
    let fixture = Fixture::new();
    let instant = epoch("2026-07-15T23:45:00Z");
    fixture
        .writer
        .execute("UPDATE sessions SET started_at=?1", [instant])
        .unwrap();
    fixture
        .writer
        .execute("UPDATE messages SET timestamp=?1", [instant])
        .unwrap();
    fixture
        .writer
        .execute(
            "UPDATE session_model_usage SET first_seen=?1,last_seen=?1",
            [instant],
        )
        .unwrap();
    fixture
        .writer
        .execute(
            "INSERT INTO session_model_usage VALUES(
           'synthetic-session','recorded-model','synthetic-provider','synthetic-endpoint',
           'synthetic-mode','',?1,?1,100,10,0,0,4)",
            [instant],
        )
        .unwrap();
    let mut reader = reader();
    let readings = reader.collect(&fixture.path).unwrap();
    let metadata = source_metadata(&fixture.path).unwrap();
    let utc = UsageWindow::from_params(&json!({"now":"2026-07-16T00:30:00Z"}));
    let local = UsageWindow::from_params(&json!({
        "now":"2026-07-16T00:30:00Z","timezoneOffsetMinutes":-60,
        "timezoneTransitions":[{"atEpochSeconds":0,"offsetMinutes":120}]
    }));
    let utc = project(&mut reader, &readings, None, &metadata, &utc).unwrap();
    let local = project(&mut reader, &readings, None, &metadata, &local).unwrap();
    assert_eq!(utc.summary.daily_usage["2026-07-15"].total_tokens, 135);
    assert_eq!(local.summary.daily_usage["2026-07-16"].total_tokens, 135);
    assert!(!utc.gap && !local.gap);
}

#[test]
fn durable_cursors_preserve_large_integers_and_rebase_on_source_replacement() {
    let fixture = Fixture::new();
    fixture
        .writer
        .execute(
            "UPDATE sessions SET input_tokens=9007199254740993,output_tokens=0",
            [],
        )
        .unwrap();
    let calendar = calendar();
    let mut reader = reader();
    let first = fixture.read(&mut reader, None, &calendar);
    assert_eq!(first.summary.explicit_total_tokens, 9_007_199_254_741_018);
    let encoded = serde_json::to_string(&first.cursor).unwrap();
    let decoded: Cursor = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, first.cursor);
    let old = cached(&fixture, &first, &calendar);
    let mut metadata = source_metadata(&fixture.path).unwrap();
    metadata.file_id = Some("synthetic-replacement".to_owned());
    let replacement = read(&mut reader, &fixture.path, Some(&old), &metadata, &calendar).unwrap();
    assert!(replacement.rebase && replacement.gap);
    assert_eq!(replacement.summary.explicit_total_tokens, 0);
}

fn plan(
    fixture: &Fixture,
    reader: &mut StoredSource,
    runtime: &CacheRuntime,
    calendar: &UsageWindow,
) -> RefreshPlan {
    let (previous, targets) = {
        let mut lease = runtime.lease("scope", &fixture.cache_path(), 100).unwrap();
        (
            cache::load_sources(lease.connection(0), "scope").unwrap(),
            cache::load_compaction_targets(lease.connection(0), "scope", &calendar.end).unwrap(),
        )
    };
    let mut old = previous.get("source").cloned();
    if targets.contains("source")
        && let Some(old) = &mut old
    {
        old.session_count = 0;
    }
    let metadata = source_metadata(&fixture.path).unwrap();
    let projection = read(reader, &fixture.path, old.as_ref(), &metadata, calendar).unwrap();
    RefreshPlan {
        compactions: previous
            .iter()
            .map(|(key, source)| (key.clone(), source.session_count))
            .collect(),
        previous,
        compaction_targets: targets,
        sources: vec![PlannedSource {
            key: "source".to_owned(),
            path: fixture.path.clone(),
            metadata,
            action: PlannedSourceAction::Stored {
                projection: Box::new(projection),
            },
        }],
    }
}

fn apply(
    fixture: &Fixture,
    runtime: &CacheRuntime,
    calendar: &UsageWindow,
    plan: RefreshPlan,
) -> Result<()> {
    let mut lease = runtime.lease("scope", &fixture.cache_path(), 100).unwrap();
    apply_refresh_plan(
        &mut lease,
        "scope",
        &calendar.today_only(),
        calendar,
        100,
        plan,
        &mut ScanStats::default(),
    )
}

fn aggregate(
    fixture: &Fixture,
    runtime: &CacheRuntime,
    calendar: &UsageWindow,
) -> HistoryUsageSummary {
    let mut lease = runtime.lease("scope", &fixture.cache_path(), 100).unwrap();
    cache::aggregate_usage(lease.connection(0), "scope", calendar).unwrap()
}

#[test]
fn cache_switch_preserves_history_and_does_not_charge_the_correction() {
    let fixture = Fixture::new();
    let runtime = CacheRuntime::new();
    let calendar = calendar();
    let mut legacy = HistoryUsageSummary::default();
    add(&mut legacy, "2026-07-14", None, counts(90, 0, 3)).unwrap();
    add(&mut legacy, "2026-07-15", None, counts(10, 10, 1)).unwrap();
    {
        let mut lease = runtime.lease("scope", &fixture.cache_path(), 100).unwrap();
        let transaction = lease.connection(0).transaction().unwrap();
        let mut statements = cache::RefreshStatements::prepare(&transaction).unwrap();
        statements.add_rollup("scope", "source", &legacy).unwrap();
        statements
            .save_source(
                "scope",
                "source",
                &source_metadata(&fixture.path).unwrap(),
                0,
                "",
                1,
                &Default::default(),
            )
            .unwrap();
        drop(statements);
        transaction.commit().unwrap();
    }
    let mut reader = reader();
    apply(
        &fixture,
        &runtime,
        &calendar,
        plan(&fixture, &mut reader, &runtime, &calendar),
    )
    .unwrap();
    let preserved = aggregate(&fixture, &runtime, &calendar);
    assert_eq!(preserved.explicit_total_tokens, 110);
    assert_eq!(preserved.daily_usage["2026-07-14"].total_tokens, 90);
    assert_eq!(preserved.daily_usage["2026-07-15"].total_tokens, 20);
    assert_eq!(preserved.session_count, 1);
    fixture
        .writer
        .execute("UPDATE sessions SET input_tokens=104,api_call_count=5", [])
        .unwrap();
    apply(
        &fixture,
        &runtime,
        &calendar,
        plan(&fixture, &mut reader, &runtime, &calendar),
    )
    .unwrap();
    let grown = aggregate(&fixture, &runtime, &calendar);
    assert_eq!(grown.explicit_total_tokens, 114);
    assert_eq!(grown.daily_usage["2026-07-15"].total_tokens, 24);
    assert_eq!(grown.session_count, 1);
}

#[test]
fn a_late_cache_write_failure_rolls_back_both_usage_and_watermarks() {
    let fixture = Fixture::new();
    let runtime = CacheRuntime::new();
    let calendar = calendar();
    let mut reader = reader();
    apply(
        &fixture,
        &runtime,
        &calendar,
        plan(&fixture, &mut reader, &runtime, &calendar),
    )
    .unwrap();
    fixture
        .writer
        .execute("UPDATE sessions SET input_tokens=104,api_call_count=5", [])
        .unwrap();
    let next = plan(&fixture, &mut reader, &runtime, &calendar);
    {
        let mut lease = runtime.lease("scope", &fixture.cache_path(), 100).unwrap();
        lease
            .connection(0)
            .execute_batch(
                "CREATE TEMP TRIGGER reject_stored_cursor BEFORE UPDATE OF snapshot_cursor
             ON native_usage_sources BEGIN SELECT RAISE(ABORT, 'synthetic-write-failure'); END;",
            )
            .unwrap();
        assert!(
            apply_refresh_plan(
                &mut lease,
                "scope",
                &calendar.today_only(),
                &calendar,
                101,
                next,
                &mut ScanStats::default(),
            )
            .is_err()
        );
        lease
            .connection(0)
            .execute_batch("DROP TRIGGER reject_stored_cursor;")
            .unwrap();
    }
    assert_eq!(
        aggregate(&fixture, &runtime, &calendar).explicit_total_tokens,
        135
    );
    apply(
        &fixture,
        &runtime,
        &calendar,
        plan(&fixture, &mut reader, &runtime, &calendar),
    )
    .unwrap();
    assert_eq!(
        aggregate(&fixture, &runtime, &calendar).explicit_total_tokens,
        139
    );
}

#[test]
fn a_completed_sqlite_snapshot_survives_later_wal_writes() {
    let fixture = Fixture::new();
    fixture
        .writer
        .pragma_update(None, "journal_mode", "WAL")
        .unwrap();
    let runtime = CacheRuntime::new();
    let calendar = calendar();
    let mut reader = reader();
    let snapshot = plan(&fixture, &mut reader, &runtime, &calendar);
    fixture
        .writer
        .execute("UPDATE sessions SET input_tokens=104,api_call_count=5", [])
        .unwrap();
    apply(&fixture, &runtime, &calendar, snapshot).unwrap();
    assert_eq!(
        aggregate(&fixture, &runtime, &calendar).explicit_total_tokens,
        135
    );
    apply(
        &fixture,
        &runtime,
        &calendar,
        plan(&fixture, &mut reader, &runtime, &calendar),
    )
    .unwrap();
    assert_eq!(
        aggregate(&fixture, &runtime, &calendar).explicit_total_tokens,
        139
    );
}

#[test]
fn invalid_source_reads_leave_the_previous_cache_available_and_unchanged() {
    let fixture = Fixture::new();
    let runtime = CacheRuntime::new();
    let calendar = calendar();
    let mut reader = reader();
    apply(
        &fixture,
        &runtime,
        &calendar,
        plan(&fixture, &mut reader, &runtime, &calendar),
    )
    .unwrap();
    fixture
        .writer
        .execute("UPDATE sessions SET input_tokens=-1", [])
        .unwrap();
    assert!(
        read(
            &mut reader,
            &fixture.path,
            None,
            &source_metadata(&fixture.path).unwrap(),
            &calendar,
        )
        .is_err()
    );
    assert_eq!(
        aggregate(&fixture, &runtime, &calendar).explicit_total_tokens,
        135
    );
}

#[test]
fn cache_rejects_integer_overflow_and_out_of_range_bindings_without_rounding() {
    let fixture = Fixture::new();
    let mut connection = cache::open_cache_database(&fixture.cache_path()).unwrap();
    let calendar = calendar();
    let mut maximum = HistoryUsageSummary::default();
    add(
        &mut maximum,
        &calendar.end,
        None,
        counts(i64::MAX as u64, 0, 1),
    )
    .unwrap();
    {
        let transaction = connection.transaction().unwrap();
        cache::RefreshStatements::prepare(&transaction)
            .unwrap()
            .add_rollup("scope", "source", &maximum)
            .unwrap();
        transaction.commit().unwrap();
    }
    let mut increment = HistoryUsageSummary::default();
    add(&mut increment, &calendar.end, None, counts(1, 0, 1)).unwrap();
    {
        let transaction = connection.transaction().unwrap();
        assert!(
            cache::RefreshStatements::prepare(&transaction)
                .unwrap()
                .add_rollup("scope", "source", &increment)
                .is_err()
        );
    }
    let value: i64 = connection
        .query_row(
            "SELECT prompt_tokens FROM native_usage_source_days WHERE scope_key='scope'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(value, i64::MAX);
    maximum
        .daily_usage
        .get_mut(&calendar.end)
        .unwrap()
        .prompt_tokens = u64::MAX;
    {
        let transaction = connection.transaction().unwrap();
        assert!(
            cache::RefreshStatements::prepare(&transaction)
                .unwrap()
                .add_rollup("scope", "oversized", &maximum)
                .is_err()
        );
    }
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM native_usage_source_days WHERE source_key='oversized'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
}

#[test]
fn included_voice_usage_is_not_added_twice_and_unknown_tasks_refuse_the_snapshot() {
    let fixture = Fixture::new();
    let calendar = calendar();
    let mut reader = reader();
    fixture
        .writer
        .execute("UPDATE session_model_usage SET task='voice_chat'", [])
        .unwrap();
    let voice = fixture.read(&mut reader, None, &calendar);
    assert_eq!(voice.summary.explicit_total_tokens, 110);
    assert_eq!(voice.summary.daily_usage[&calendar.end].request_count, 4);
    fixture
        .writer
        .execute("UPDATE session_model_usage SET task='undeclared-task'", [])
        .unwrap();
    assert!(
        read(
            &mut reader,
            &fixture.path,
            None,
            &source_metadata(&fixture.path).unwrap(),
            &calendar,
        )
        .is_err()
    );
}

#[test]
fn the_native_consumer_refuses_read_failures_instead_of_returning_zero() {
    let fixture = Fixture::new();
    let state = Directory::new();
    let runtime = CacheRuntime::new();
    let calendar = calendar();
    let params = json!({
        "root": fixture.path, "stateRoot": state.0,
        "forceRefresh": true, "now": "2026-07-15T12:00:00Z"
    });
    let agent = agent_statistics::contract::AgentDef {
        id: "hermes",
        label: "Synthetic Hermes",
    };
    let mut warnings = Vec::new();
    let first =
        super::super::summarize(&agent, &params, &calendar, &mut warnings, &runtime).unwrap();
    assert_eq!(first.explicit_total_tokens, 135);
    assert!(
        warnings
            .iter()
            .any(|warning| warning["code"] == "native_usage_time_inferred")
    );
    fixture
        .writer
        .execute("UPDATE sessions SET input_tokens=-1", [])
        .unwrap();
    assert!(super::super::summarize(&agent, &params, &calendar, &mut warnings, &runtime).is_err());
    fixture
        .writer
        .execute("UPDATE sessions SET input_tokens=104,api_call_count=5", [])
        .unwrap();
    let repaired =
        super::super::summarize(&agent, &params, &calendar, &mut warnings, &runtime).unwrap();
    assert_eq!(repaired.explicit_total_tokens, 139);
}
