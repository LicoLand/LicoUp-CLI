use super::*;
use licoup_statistics::contract::{MessageUsage, UsageVariant};
use std::io::Write;

fn calendar() -> UsageWindow {
    UsageWindow::from_params(&json!({"now":"2026-07-15T12:00:00Z", "historyDays":1}))
}

fn usage(prompt: u64, effort: Option<&str>) -> HistoryUsageSummary {
    let mut summary = HistoryUsageSummary::default();
    summary.add(
        MessageUsage {
            prompt_tokens: prompt,
            total_tokens: prompt,
            model: Some("raw-model".to_owned()),
            variant: UsageVariant {
                effort: effort.map(str::to_owned),
                fast: None,
            },
            ..Default::default()
        },
        Some("2026-07-15".to_owned()),
    );
    summary.session_count = 1;
    summary
}

fn event(prompt: u64) -> String {
    format!(
        "{}\n",
        json!({"type":"session_usage", "sessionId":"synthetic-session",
        "timestamp":"2026-07-15T10:00:00Z", "model":"raw-model", "effort":"high",
        "usage":{"input_tokens":prompt,"output_tokens":0}})
    )
}

fn migration_append_case(sealed: bool, replaced: bool) {
    let root = std::env::temp_dir().join(format!("lico-native-migration-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    let path = root.join("synthetic.jsonl");
    fs::write(&path, event(100)).unwrap();
    let calendar = calendar();
    let old = parse_append_source(
        HistoryAdapter::ClaudeCode,
        &path,
        0,
        &calendar,
        false,
        Default::default(),
    )
    .unwrap();
    let old_snapshot = old.cumulative_snapshots[0].clone();
    let mut metadata = source_metadata(&path).unwrap();
    if replaced {
        metadata.file_id = Some("different-synthetic-file".to_owned());
    }
    let mut connection = cache::open_cache_database(Path::new(":memory:")).unwrap();
    let transaction = connection.transaction().unwrap();
    let mut statements = RefreshStatements::prepare(&transaction).unwrap();
    statements
        .add_rollup("scope", "source", &usage(100, None))
        .unwrap();
    statements
        .save_source(
            "scope",
            "source",
            &metadata,
            old.parsed_bytes,
            &append_guard(&path, metadata.size).unwrap(),
            1,
            &Default::default(),
        )
        .unwrap();
    statements
        .save_migration_state("scope", "source", 1)
        .unwrap();
    statements
        .save_watermark(
            "scope",
            "source",
            "old-model-key",
            &watermark::Watermark {
                session_key: old_snapshot.session_key,
                model: old_snapshot.model,
                variant: Default::default(),
                day: calendar.end.clone(),
                last: old_snapshot.totals,
                day_total: old_snapshot.totals,
                day_variants: BTreeMap::new(),
            },
        )
        .unwrap();
    if sealed {
        statements.seal("scope", "source", 1).unwrap();
    }
    drop(statements);
    transaction.commit().unwrap();
    let previous = load_sources(&connection, "scope")
        .unwrap()
        .remove("source")
        .unwrap();
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    write!(file, "{}", event(130)).unwrap();
    let (action, metadata) = plan_source_migration(
        HistoryAdapter::ClaudeCode,
        &path,
        "jsonl",
        &previous,
        &calendar,
        &mut deepseek::Reader::default(),
    )
    .unwrap();
    let PlannedSourceAction::Migrate {
        baseline,
        tail,
        complete,
        append_format,
        append_guard,
    } = action
    else {
        panic!("expected migration")
    };
    assert_eq!(complete, !replaced);
    let transaction = connection.transaction().unwrap();
    let mut statements = RefreshStatements::prepare(&transaction).unwrap();
    apply_source_migration(
        &mut statements,
        "scope",
        "source",
        &metadata,
        &previous,
        *baseline,
        tail.map(|tail| *tail),
        complete,
        append_format,
        &append_guard,
        &calendar,
    )
    .unwrap();
    drop(statements);
    transaction.commit().unwrap();
    let first = cache::aggregate_usage(&mut connection, "scope", &calendar).unwrap();
    assert_eq!(first.total_tokens(), if replaced { 100 } else { 130 });
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM native_usage_watermarks WHERE usage_key='old-model-key'",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        0
    );
    let previous = load_sources(&connection, "scope")
        .unwrap()
        .remove("source")
        .unwrap();
    assert!(source_unchanged(&previous, &metadata));
    assert_eq!(previous.parsed_bytes, metadata.size);
    assert_eq!(previous.migration_state, if replaced { 3 } else { 2 });
    write!(file, "{}", event(150)).unwrap();
    let mut parsed = parse_stable_source(
        HistoryAdapter::ClaudeCode,
        &path,
        "jsonl",
        Some(&previous),
        0,
        &calendar,
        &mut deepseek::Reader::default(),
    )
    .unwrap();
    assert!(parsed.append);
    let transaction = connection.transaction().unwrap();
    let mut statements = RefreshStatements::prepare(&transaction).unwrap();
    apply_cumulative_watermarks(
        &mut statements,
        "scope",
        "source",
        &calendar,
        &parsed.parsed.cumulative_snapshots,
        WatermarkProjection::AppendDelta,
        &mut parsed.parsed.summary,
    )
    .unwrap();
    assert_eq!(parsed.parsed.summary.total_tokens(), 20);
    statements
        .add_rollup("scope", "source", &parsed.parsed.summary)
        .unwrap();
    drop(statements);
    transaction.commit().unwrap();
    assert_eq!(
        cache::aggregate_usage(&mut connection, "scope", &calendar)
            .unwrap()
            .total_tokens(),
        if replaced { 120 } else { 150 }
    );
    drop(file);
    drop(connection);
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[cfg(unix)]
fn old_prefix_restores_session_cursor_and_already_sealed_ledger_is_not_counted_twice() {
    migration_append_case(false, false);
    migration_append_case(true, false);
}

#[test]
#[cfg(unix)]
fn changed_source_keeps_ledger_reports_gap_and_resumes_from_current_cursor() {
    migration_append_case(false, true);
}

#[test]
fn first_snapshot_keeps_old_ledger_and_only_future_requests_are_added() {
    let calendar = calendar();
    let mut connection = cache::open_cache_database(Path::new(":memory:")).unwrap();
    let metadata = SourceMetadata {
        modified_ns: 1,
        size: 100,
        file_id: Some("synthetic-db".to_owned()),
    };
    let transaction = connection.transaction().unwrap();
    let mut statements = RefreshStatements::prepare(&transaction).unwrap();
    statements
        .add_rollup("scope", "source", &usage(100, None))
        .unwrap();
    statements
        .save_source(
            "scope",
            "source",
            &metadata,
            100,
            "",
            1,
            &Default::default(),
        )
        .unwrap();
    statements
        .save_migration_state("scope", "source", 1)
        .unwrap();
    drop(statements);
    transaction.commit().unwrap();
    let previous = load_sources(&connection, "scope")
        .unwrap()
        .remove("source")
        .unwrap();
    let current = usage(140, Some("high"));
    let baseline = ParseResult {
        summary: current.clone(),
        parsed_bytes: 100,
        ..Default::default()
    };
    let transaction = connection.transaction().unwrap();
    let mut statements = RefreshStatements::prepare(&transaction).unwrap();
    apply_source_migration(
        &mut statements,
        "scope",
        "source",
        &metadata,
        &previous,
        baseline,
        None,
        false,
        false,
        "",
        &calendar,
    )
    .unwrap();
    drop(statements);
    transaction.commit().unwrap();
    assert_eq!(
        cache::aggregate_usage(&mut connection, "scope", &calendar)
            .unwrap()
            .total_tokens(),
        100
    );
    assert_eq!(
        cache::migration_counts(&connection, "scope").unwrap(),
        (0, 1)
    );
    let previous = load_sources(&connection, "scope")
        .unwrap()
        .remove("source")
        .unwrap();
    let cursor = previous.snapshot_cursor.unwrap();
    let unchanged = SnapshotCursor::capture(&current, &calendar);
    assert_eq!(unchanged.delta(&cursor, &calendar).0.total_tokens(), 0);
    let mut next = current;
    next.merge(&usage(20, Some("low")));
    let next_cursor = SnapshotCursor::capture(&next, &calendar);
    let (delta, gap) = next_cursor.delta(&cursor, &calendar);
    assert!(!gap);
    assert_eq!(delta.total_tokens(), 20);
    let transaction = connection.transaction().unwrap();
    let mut statements = RefreshStatements::prepare(&transaction).unwrap();
    statements.add_rollup("scope", "source", &delta).unwrap();
    statements
        .save_snapshot_cursor("scope", "source", &next_cursor)
        .unwrap();
    statements.seal("scope", "source", 0).unwrap();
    drop(statements);
    transaction.commit().unwrap();
    let summary = cache::aggregate_usage(&mut connection, "scope", &calendar).unwrap();
    assert_eq!(summary.total_tokens(), 120);
    let day = &summary.daily_usage[&calendar.end];
    assert_eq!(day.request_count, 2);
    assert_eq!(day.model_usage["raw-model"].request_count, 2);
    assert_eq!(
        day.model_variants
            .values()
            .map(|value| value.total_tokens)
            .sum::<u64>(),
        120
    );
    assert_eq!(
        day.model_variants[&(
            "raw-model".to_owned(),
            UsageVariant {
                effort: Some("low".to_owned()),
                fast: None
            }
        )]
            .total_tokens,
        20
    );
    assert_eq!(
        day.model_variants[&("raw-model".to_owned(), UsageVariant::default())].total_tokens,
        100
    );
}
