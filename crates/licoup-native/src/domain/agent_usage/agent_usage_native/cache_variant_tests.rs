use super::*;
use licoup_statistics::contract::MessageUsage;
use serde_json::json;

fn variant(effort: Option<&str>, fast: Option<bool>) -> UsageVariant {
    UsageVariant {
        effort: effort.map(str::to_owned),
        fast,
    }
}

fn window() -> UsageWindow {
    UsageWindow::from_params(&json!({"now":"2026-07-15T12:00:00Z", "historyDays":2}))
}

#[test]
fn variants_and_request_context_survive_compaction_sealing_and_reopen() {
    let root = std::env::temp_dir().join(format!("lico-usage-variants-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    let path = cache_path(&root);
    let variants = [
        variant(Some("high"), None),
        variant(Some("high"), Some(false)),
        variant(Some("low"), Some(true)),
        variant(None, Some(true)),
        variant(None, None),
    ];
    let context = UsageRequestContext {
        model: Some("raw-model:large".to_owned()),
        variant: variants[1].clone(),
    };
    let mut connection = open_cache_database(&path).unwrap();
    let transaction = connection.transaction().unwrap();
    let mut statements = RefreshStatements::prepare(&transaction).unwrap();
    let mut history = HistoryUsageSummary::default();
    for (index, variant) in variants.iter().enumerate() {
        let prompt = 10 + index as u64;
        history.add(
            MessageUsage {
                prompt_tokens: prompt,
                cached_input_tokens: 1,
                completion_tokens: 2,
                total_tokens: prompt + 2,
                model: context.model.clone(),
                variant: variant.clone(),
                accuracy: Default::default(),
            },
            Some("2026-07-14".to_owned()),
        );
    }
    statements.add_rollup("scope", "source", &history).unwrap();
    statements
        .save_source(
            "scope",
            "source",
            &SourceMetadata {
                modified_ns: 1,
                size: 10,
                file_id: Some("synthetic-file".to_owned()),
            },
            10,
            "synthetic-guard",
            1,
            &context,
        )
        .unwrap();
    assert_eq!(
        statements
            .compact("scope", "source", "2026-07-15", 1)
            .unwrap(),
        1
    );
    let mut appended = HistoryUsageSummary::default();
    appended.add(
        MessageUsage {
            prompt_tokens: 5,
            cached_input_tokens: 0,
            completion_tokens: 1,
            total_tokens: 6,
            model: context.model.clone(),
            variant: variants[1].clone(),
            accuracy: Default::default(),
        },
        Some("2026-07-15".to_owned()),
    );
    appended.add_token_unavailable_request_with_variant(
        Some("2026-07-15".to_owned()),
        context.model.clone(),
        variants[1].clone(),
    );
    statements.add_rollup("scope", "source", &appended).unwrap();
    statements.seal("scope", "source", 0).unwrap();
    drop(statements);
    transaction.commit().unwrap();
    drop(connection);

    let mut connection = open_cache_database(&path).unwrap();
    assert_eq!(
        load_sources(&connection, "scope").unwrap()["source"].request_context,
        context
    );
    let aggregate = aggregate_usage(&mut connection, "scope", &window()).unwrap();
    assert_eq!(aggregate.total_tokens(), 76);
    assert_eq!(aggregate.token_unavailable_records, 1);
    assert_eq!(aggregate.daily_usage["2026-07-15"].request_count, 2);
    assert_eq!(
        aggregate.daily_usage["2026-07-15"].model_usage["raw-model:large"]
            .token_unavailable_requests,
        1
    );
    let day = &aggregate.daily_usage["2026-07-14"];
    assert_eq!(day.model_usage["raw-model:large"].total_tokens, 70);
    assert_eq!(day.request_count, 5);
    assert_eq!(day.model_usage["raw-model:large"].request_count, 5);
    assert_eq!(day.model_variants.len(), variants.len());
    for (index, variant) in variants.iter().enumerate() {
        assert_eq!(
            day.model_variants[&("raw-model:large".to_owned(), variant.clone())].total_tokens,
            12 + index as u64
        );
    }
    assert_eq!(
        aggregate.daily_usage["2026-07-15"].model_variants
            [&("raw-model:large".to_owned(), variants[1].clone())]
            .total_tokens,
        6
    );
    drop(connection);
    fs::remove_dir_all(root).unwrap();
}

fn legacy_database(path: &Path, version: i64) -> Connection {
    let mut connection = Connection::open(path).unwrap();
    let transaction = connection.transaction().unwrap();
    create_schema(&transaction).unwrap();
    transaction
        .execute_batch("ALTER TABLE native_usage_sources DROP COLUMN migration_state;")
        .unwrap();
    if version == 7 {
        transaction.execute_batch(
            "ALTER TABLE native_usage_sources DROP COLUMN request_context;
             ALTER TABLE native_usage_watermarks DROP COLUMN effort;
             ALTER TABLE native_usage_watermarks DROP COLUMN fast;
             ALTER TABLE native_usage_watermarks DROP COLUMN day_variants;
             DROP TABLE native_usage_source_models;
             CREATE TABLE native_usage_source_models (
               scope_key TEXT NOT NULL,source_key TEXT NOT NULL,day TEXT NOT NULL,model TEXT NOT NULL,
               prompt_tokens INTEGER NOT NULL,cached_input_tokens INTEGER NOT NULL,
               completion_tokens INTEGER NOT NULL,total_tokens INTEGER NOT NULL,
               estimated_prompt_tokens INTEGER NOT NULL,estimated_completion_tokens INTEGER NOT NULL,
               PRIMARY KEY(scope_key,source_key,day,model));
             CREATE INDEX native_usage_source_models_window ON native_usage_source_models(scope_key,day);
             DROP TABLE native_usage_daily_models;
             CREATE TABLE native_usage_daily_models (
               scope_key TEXT NOT NULL,day TEXT NOT NULL,model TEXT NOT NULL,
               prompt_tokens INTEGER NOT NULL,cached_input_tokens INTEGER NOT NULL,
               completion_tokens INTEGER NOT NULL,total_tokens INTEGER NOT NULL,
               estimated_prompt_tokens INTEGER NOT NULL,estimated_completion_tokens INTEGER NOT NULL,
               PRIMARY KEY(scope_key,day,model));"
        ).unwrap();
    }
    transaction
        .pragma_update(None, "user_version", version)
        .unwrap();
    transaction.commit().unwrap();
    connection
}

#[test]
fn schema_seven_migrates_existing_ledgers_sources_and_cursors_without_recounting() {
    let root = std::env::temp_dir().join(format!("lico-usage-schema-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    let path = cache_path(&root);
    let connection = legacy_database(&path, 7);
    connection.execute_batch(
        "INSERT INTO native_usage_scans VALUES('scope',42);
         INSERT INTO native_usage_sources VALUES('scope','active',7,80,'file-a',72,'guard',1,0);
         INSERT INTO native_usage_sources VALUES('scope','sealed',9,90,'file-b',88,'guard-b',0,1);
         INSERT INTO native_usage_source_days VALUES('scope','active','2026-07-15',10,2,2,0,0,1,0,1);
         INSERT INTO native_usage_source_models VALUES('scope','active','2026-07-15','raw-model',10,2,2,12,0,0);
         INSERT INTO native_usage_daily_totals VALUES('scope','2026-07-14',9,1,2,0,0,1,0,1,1);
         INSERT INTO native_usage_daily_models VALUES('scope','2026-07-14','old-model',9,1,2,11,0,0);
         INSERT INTO native_usage_watermarks VALUES('scope','active','old-model-key','session','raw-model','2026-07-15',100,2,2,10,2,2);"
    ).unwrap();
    drop(connection);
    let mut connection = open_cache_database(&path).unwrap();
    assert!(cache_has_baseline(&connection, "scope").unwrap());
    assert!(!cache_is_fresh(&connection, "scope", 100_000, 60_000).unwrap());
    let sources = load_sources(&connection, "scope").unwrap();
    assert_eq!(sources["active"].parsed_bytes, 72);
    assert_eq!(sources["active"].file_id.as_deref(), Some("file-a"));
    assert_eq!(sources["active"].append_guard, "guard");
    assert_eq!(sources["active"].session_count, 1);
    assert_eq!(
        sources["active"].request_context,
        UsageRequestContext::default()
    );
    assert_eq!(sources["active"].migration_state, 1);
    assert!(sources["sealed"].sealed);
    assert_eq!(sources["sealed"].parsed_bytes, 88);
    let summary = aggregate_usage(&mut connection, "scope", &window()).unwrap();
    assert_eq!(summary.total_tokens(), 23);
    assert_eq!(summary.session_count, 2);
    assert_eq!(
        summary.daily_usage["2026-07-15"].model_variants
            [&("raw-model".to_owned(), UsageVariant::default())]
            .total_tokens,
        12
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT last_prompt FROM native_usage_watermarks WHERE usage_key='old-model-key'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        100
    );
    assert_eq!(migration_counts(&connection, "scope").unwrap(), (2, 0));
    drop(connection);
    let mut reopened = open_cache_database(&path).unwrap();
    assert_eq!(
        aggregate_usage(&mut reopened, "scope", &window())
            .unwrap()
            .total_tokens(),
        23
    );
    drop(reopened);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn schema_eight_preserves_variants_and_unknown_schema_preserves_all_tables() {
    let root = std::env::temp_dir().join(format!("lico-usage-schema-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    let path = cache_path(&root);
    let connection = legacy_database(&path, 8);
    connection.execute_batch(
        "INSERT INTO native_usage_scans VALUES('scope',42);
         INSERT INTO native_usage_daily_totals VALUES('scope','2026-07-14',9,1,2,0,0,1,0,1,1);
         INSERT INTO native_usage_daily_models VALUES('scope','2026-07-14','raw-model',9,1,2,11,0,0,'high',0);"
    ).unwrap();
    drop(connection);
    let mut connection = open_cache_database(&path).unwrap();
    let summary = aggregate_usage(&mut connection, "scope", &window()).unwrap();
    assert_eq!(summary.total_tokens(), 11);
    assert_eq!(
        summary.daily_usage["2026-07-14"].model_variants
            [&("raw-model".to_owned(), variant(Some("high"), Some(false)))]
            .total_tokens,
        11
    );
    connection.pragma_update(None, "user_version", 77).unwrap();
    drop(connection);
    assert!(open_cache_database(&path).is_err());
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        77
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT total_tokens FROM native_usage_daily_models",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        11
    );
    drop(connection);
    fs::remove_dir_all(root).unwrap();
}
