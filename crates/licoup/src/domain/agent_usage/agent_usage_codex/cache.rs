use super::constants::{CACHE_REFRESH_INTERVAL, CACHE_SCHEMA_VERSION};
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, ErrorCode, OptionalExtension, Transaction, TransactionBehavior};
use std::collections::BTreeSet;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

pub(super) fn open_cache_database(path: &Path) -> Result<Connection> {
    let mut connection = Connection::open(path).context("agent usage cache open failed")?;
    connection.busy_timeout(Duration::from_secs(30))?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    let observed_version =
        connection.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
    if observed_version != CACHE_SCHEMA_VERSION {
        let migration = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("agent usage cache schema transaction failed")?;
        let locked_version =
            migration.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
        match locked_version {
            0 => migration.execute_batch(
                "CREATE TABLE usage_files (
                   root_key TEXT NOT NULL,
                   source_key TEXT NOT NULL,
                   modified_ns INTEGER NOT NULL,
                   size INTEGER NOT NULL,
                   file_id TEXT,
                   parsed_bytes INTEGER NOT NULL,
                   append_guard TEXT NOT NULL,
                   session_id TEXT,
                   forked_from_id TEXT,
                   lineage_scope TEXT NOT NULL,
                   last_model TEXT,
                   current_turn_id TEXT,
                   raw_input INTEGER,
                   raw_cached INTEGER,
                   raw_output INTEGER,
                   counted_input INTEGER,
                   counted_cached INTEGER,
                   counted_output INTEGER,
                   divergent INTEGER NOT NULL DEFAULT 0,
                   next_event_index INTEGER NOT NULL DEFAULT 0,
                   token_chain_hash TEXT NOT NULL DEFAULT '',
                   current_effort TEXT,
                   current_fast INTEGER,
                   pending_context INTEGER NOT NULL DEFAULT 0,
                   PRIMARY KEY(root_key, source_key)
                 );
                 CREATE TABLE usage_rows (
                   root_key TEXT NOT NULL,
                   source_key TEXT NOT NULL,
                   event_index INTEGER NOT NULL,
                   session_id TEXT,
                   turn_id TEXT,
                   day TEXT NOT NULL,
                   model TEXT,
                   input_tokens INTEGER NOT NULL,
                   cached_input_tokens INTEGER NOT NULL,
                   output_tokens INTEGER NOT NULL,
                   event_identity TEXT NOT NULL,
                   effort TEXT,
                   fast INTEGER,
                   PRIMARY KEY(root_key, source_key, event_index)
                 );
                 CREATE INDEX usage_rows_window ON usage_rows(root_key, day);
                 CREATE INDEX usage_rows_identity
                   ON usage_rows(root_key, event_identity);
                 CREATE TABLE usage_daily_totals (
                   root_key TEXT NOT NULL,
                   day TEXT NOT NULL,
                   explicit_prompt INTEGER NOT NULL,
                   explicit_cached INTEGER NOT NULL,
                   explicit_completion INTEGER NOT NULL,
                   explicit_records INTEGER NOT NULL,
                   message_count INTEGER NOT NULL,
                   PRIMARY KEY(root_key, day)
                 );
                 CREATE TABLE usage_daily_models (
                   root_key TEXT NOT NULL,
                   day TEXT NOT NULL,
                   model TEXT NOT NULL,
                   prompt_tokens INTEGER NOT NULL,
                   cached_input_tokens INTEGER NOT NULL,
                   completion_tokens INTEGER NOT NULL,
                   total_tokens INTEGER NOT NULL,
                   effort TEXT NOT NULL,
                   fast INTEGER NOT NULL,
                   PRIMARY KEY(root_key, day, model, effort, fast)
                 );
                 CREATE TABLE usage_daily_sessions (
                   root_key TEXT NOT NULL,
                   day TEXT NOT NULL,
                   session_key TEXT NOT NULL,
                   PRIMARY KEY(root_key, day, session_key)
                 );
                 CREATE TABLE usage_scans (
                   root_key TEXT PRIMARY KEY,
                   last_scan_ms INTEGER NOT NULL
                 );",
            )?,
            12 => {
                migration.execute_batch(
                    "ALTER TABLE usage_files ADD COLUMN current_effort TEXT;
                     ALTER TABLE usage_files ADD COLUMN current_fast INTEGER;
                     ALTER TABLE usage_files ADD COLUMN pending_context INTEGER NOT NULL DEFAULT 0;
                     ALTER TABLE usage_rows ADD COLUMN effort TEXT;
                     ALTER TABLE usage_rows ADD COLUMN fast INTEGER;
                     ALTER TABLE usage_daily_models RENAME TO usage_daily_models_previous;
                     CREATE TABLE usage_daily_models (
                       root_key TEXT NOT NULL,
                       day TEXT NOT NULL,
                       model TEXT NOT NULL,
                       prompt_tokens INTEGER NOT NULL,
                       cached_input_tokens INTEGER NOT NULL,
                       completion_tokens INTEGER NOT NULL,
                       total_tokens INTEGER NOT NULL,
                       effort TEXT NOT NULL,
                       fast INTEGER NOT NULL,
                       PRIMARY KEY(root_key, day, model, effort, fast)
                     );
                     INSERT INTO usage_daily_models
                       SELECT root_key,day,model,prompt_tokens,cached_input_tokens,
                              completion_tokens,total_tokens,'',-1
                       FROM usage_daily_models_previous;
                     DROP TABLE usage_daily_models_previous;",
                )?;
                invalidate_mutable_sources(&migration)?;
            }
            13 => invalidate_mutable_sources(&migration)?,
            CACHE_SCHEMA_VERSION => {}
            _ => bail!("unsupported agent usage cache schema"),
        }
        migration.pragma_update(None, "user_version", CACHE_SCHEMA_VERSION)?;
        migration
            .commit()
            .context("agent usage cache schema commit failed")?;
        if locked_version == 0 {
            connection.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
            connection
                .execute_batch("VACUUM;")
                .context("agent usage cache schema compaction failed")?;
        }
    }
    #[cfg(unix)]
    if path.exists() {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(connection)
}

fn invalidate_mutable_sources(transaction: &Transaction<'_>) -> Result<()> {
    // Daily rollups are the permanent ledger. Keep their baseline marker so
    // the next scan seals existing past details before reparsing only today.
    // Invalid fingerprints rebuild context without discarding unreadable
    // sources' already completed days or adding a second historical total.
    transaction.execute_batch(
        "INSERT OR IGNORE INTO usage_scans(root_key,last_scan_ms)
           SELECT root_key,0 FROM usage_files
           UNION SELECT root_key,0 FROM usage_daily_totals;
         UPDATE usage_scans SET last_scan_ms=0;
         UPDATE usage_files SET modified_ns=-1,append_guard='';",
    )?;
    Ok(())
}

pub(super) fn sqlite_is_busy(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(code, _)
            if matches!(code.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

pub(super) fn cache_state(connection: &Connection, root_key: &str) -> Result<(bool, bool)> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM usage_files WHERE root_key=?1 LIMIT 1),
                    EXISTS(SELECT 1 FROM usage_scans WHERE root_key=?1)",
            [root_key],
            |row| Ok((row.get::<_, bool>(0)?, row.get::<_, bool>(1)?)),
        )
        .map_err(Into::into)
}

pub(super) fn cache_is_fresh(connection: &Connection, root_key: &str, now_ms: u64) -> Result<bool> {
    let last_scan = connection
        .query_row(
            "SELECT last_scan_ms FROM usage_scans WHERE root_key=?1",
            [root_key],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .unwrap_or(0)
        .max(0) as u64;
    Ok(last_scan > 0
        && now_ms.saturating_sub(last_scan) < CACHE_REFRESH_INTERVAL.as_millis() as u64)
}

pub(super) fn cached_source_keys(
    transaction: &Transaction<'_>,
    root_key: &str,
) -> Result<BTreeSet<String>> {
    let mut statement = transaction
        .prepare("SELECT source_key FROM usage_files WHERE root_key=?1 ORDER BY source_key")?;
    let rows = statement.query_map([root_key], |row| row.get::<_, String>(0))?;
    rows.collect::<rusqlite::Result<BTreeSet<_>>>()
        .map_err(Into::into)
}
