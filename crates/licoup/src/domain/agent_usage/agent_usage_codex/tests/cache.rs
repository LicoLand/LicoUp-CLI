use super::super::cache::{cache_is_fresh, open_cache_database};
use super::super::cache_cleanup::reclaim_cache_space;
use super::super::constants::CACHE_SCHEMA_VERSION;
use super::support::{install_v12_fixture_schema, temp_dir};
use rusqlite::Connection;
use std::fs;

#[test]
fn schema_upgrade_preserves_archived_facts_and_installs_variant_keys() {
    let root = temp_dir("cache-schema");
    let path = root.join("usage.sqlite3");
    {
        let stale = open_cache_database(&path).unwrap();
        stale
            .execute_batch(
                "INSERT INTO usage_daily_totals VALUES('scope','2026-07-08',10,2,3,1,1);
                 INSERT INTO usage_daily_models VALUES('scope','2026-07-08','model',10,2,3,13,'high',1);
                 INSERT INTO usage_daily_sessions VALUES('scope','2026-07-08','session');",
            )
            .unwrap();
        install_v12_fixture_schema(&stale);
    }
    let connection = open_cache_database(&path).unwrap();
    let version = connection
        .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
        .unwrap();
    let index_sql = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='index' AND name='usage_rows_window'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap();
    assert_eq!(version, CACHE_SCHEMA_VERSION);
    assert_eq!(
        connection
            .query_row(
                "SELECT explicit_prompt+explicit_completion FROM usage_daily_totals",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        13
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT total_tokens,effort,fast FROM usage_daily_models",
                [],
                |row| Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?
                ))
            )
            .unwrap(),
        (13, String::new(), -1)
    );
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM usage_daily_sessions", [], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT last_scan_ms FROM usage_scans WHERE root_key='scope'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert!(index_sql.contains("root_key, day"));
    assert!(!index_sql.contains("source_key"));
    let auto_vacuum = connection
        .pragma_query_value(None, "auto_vacuum", |row| row.get::<_, i64>(0))
        .unwrap();
    assert_eq!(auto_vacuum, 2);
}

#[test]
fn freshness_uses_one_root_scoped_scan_timestamp() {
    let root = temp_dir("cache-freshness");
    let path = root.join("usage.sqlite3");
    let connection = open_cache_database(&path).unwrap();
    connection
        .execute(
            "INSERT INTO usage_scans(root_key, last_scan_ms) VALUES(?1, ?2)",
            rusqlite::params!["root", 1_000_i64],
        )
        .unwrap();
    assert!(cache_is_fresh(&connection, "root", 1_001).unwrap());
    assert!(!cache_is_fresh(&connection, "root", 61_001).unwrap());
}

#[test]
fn opening_one_scope_preserves_other_scopes_until_their_own_migration() {
    let root = temp_dir("cache-obsolete");
    let stale_path = root.join("agent-usage-cache-v2-stale.sqlite3");
    let current_path = root.join("agent-usage-cache-v2-current.sqlite3");
    let peer_path = root.join("agent-usage-cache-v2-peer.sqlite3");
    let stale = open_cache_database(&stale_path).unwrap();
    stale
        .execute(
            "INSERT INTO usage_daily_totals VALUES('other','2026-07-08',10,2,3,1,1)",
            [],
        )
        .unwrap();
    install_v12_fixture_schema(&stale);
    drop(stale);
    open_cache_database(&peer_path).unwrap();
    open_cache_database(&current_path).unwrap();

    assert!(stale_path.exists());
    assert!(peer_path.exists());
    assert!(current_path.exists());
    let unselected = Connection::open(&stale_path).unwrap();
    assert_eq!(
        unselected
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        12
    );
    drop(unselected);
    let selected = open_cache_database(&stale_path).unwrap();
    assert_eq!(
        selected
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        CACHE_SCHEMA_VERSION
    );
    assert_eq!(
        selected
            .query_row(
                "SELECT explicit_prompt+explicit_completion FROM usage_daily_totals",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        13
    );
}

#[test]
fn unsupported_schema_returns_error_without_discarding_its_records() {
    for version in [6, CACHE_SCHEMA_VERSION + 1] {
        let root = temp_dir("cache-unsupported");
        let path = root.join("usage.sqlite3");
        let original = Connection::open(&path).unwrap();
        original.execute_batch("CREATE TABLE usage_rows (model TEXT NOT NULL); INSERT INTO usage_rows VALUES('retained');").unwrap();
        original
            .pragma_update(None, "user_version", version)
            .unwrap();
        drop(original);
        assert!(open_cache_database(&path).is_err());
        let retained = Connection::open(&path).unwrap();
        assert_eq!(
            retained
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            version
        );
        assert_eq!(
            retained
                .query_row("SELECT model FROM usage_rows", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "retained"
        );
    }
}

#[test]
fn physical_compaction_truncates_pages_after_bulk_detail_deletion() {
    let root = temp_dir("cache-physical-compaction");
    let path = root.join("usage.sqlite3");
    let mut connection = open_cache_database(&path).unwrap();
    let transaction = connection.transaction().unwrap();
    {
        let mut insert = transaction
            .prepare(
                "INSERT INTO usage_rows(root_key,source_key,event_index,session_id,turn_id,day,model,input_tokens,cached_input_tokens,output_tokens,event_identity) VALUES(
                   'root',?1,?2,NULL,NULL,'2026-01-01',NULL,1,0,1,?3
                 )",
            )
            .unwrap();
        let padding = "x".repeat(512);
        for index in 0..10_000_i64 {
            insert
                .execute(rusqlite::params![format!("source-{index}"), index, padding])
                .unwrap();
        }
    }
    transaction.commit().unwrap();
    connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    let expanded = fs::metadata(&path).unwrap().len();
    connection.execute("DELETE FROM usage_rows", []).unwrap();
    reclaim_cache_space(&connection).unwrap();
    let compacted = fs::metadata(&path).unwrap().len();

    assert!(
        compacted < expanded / 2,
        "cache file was not truncated: {expanded} -> {compacted}"
    );
}
