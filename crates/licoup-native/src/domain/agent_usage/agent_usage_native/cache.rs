//! Private aggregate-cache ownership for native usage sources.

use super::models::{CachedSource, SourceMetadata};
use anyhow::{Context, Result};
use licoup_statistics::contract::{
    DailyUsageSummary, HistoryUsageSummary, ModelTokenUsageSummary, UsageVariant,
};
use licoup_statistics::variant::UsageRequestContext;
use licoup_statistics::window::UsageWindow;
use rusqlite::{
    Connection, OptionalExtension, Statement, Transaction, TransactionBehavior, params,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(super) const CACHE_SCHEMA_VERSION: i64 = 9;
pub(super) const CACHE_FILE_NAME: &str = "agent-usage-rollups-v2.sqlite3";

#[derive(Debug)]
pub(super) struct UnsupportedSchemaVersion(pub(super) i64);

impl std::fmt::Display for UnsupportedSchemaVersion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "unsupported native usage cache schema: {}; data preserved",
            self.0
        )
    }
}

impl std::error::Error for UnsupportedSchemaVersion {}

pub(super) fn cache_path(state_root: &Path) -> PathBuf {
    state_root.join(CACHE_FILE_NAME)
}

pub(super) fn open_cache_database(path: &Path) -> Result<Connection> {
    let mut connection = Connection::open(path).context("native usage cache open failed")?;
    connection.busy_timeout(Duration::from_secs(30))?;
    let version =
        connection.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
    if version != CACHE_SCHEMA_VERSION {
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Another process may have migrated while this connection waited for
        // the write lock. Only the version observed under that lock owns DDL.
        let locked_version =
            transaction.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
        match locked_version {
            0 => {
                let occupied: bool = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%')",
                    [],
                    |row| row.get(0),
                )?;
                if occupied {
                    return Err(UnsupportedSchemaVersion(locked_version).into());
                }
                create_schema(&transaction)?;
                add_cursor_and_count_columns(&transaction)?;
            }
            7 | 8 => {
                if locked_version == 7 {
                    migrate_variant_columns(&transaction)?;
                }
                transaction.execute_batch(
                    "ALTER TABLE native_usage_sources ADD COLUMN migration_state INTEGER NOT NULL DEFAULT 0;
                     UPDATE native_usage_scans SET last_scan_ms=0;",
                )?;
                if locked_version == 7 {
                    transaction.execute("UPDATE native_usage_sources SET migration_state=1", [])?;
                }
                add_cursor_and_count_columns(&transaction)?;
            }
            CACHE_SCHEMA_VERSION => {}
            _ => return Err(UnsupportedSchemaVersion(locked_version).into()),
        }
        if locked_version != CACHE_SCHEMA_VERSION {
            transaction.pragma_update(None, "user_version", CACHE_SCHEMA_VERSION)?;
        }
        transaction.commit()?;
    }
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    #[cfg(unix)]
    if path.exists() {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(connection)
}

fn create_schema(transaction: &Transaction<'_>) -> Result<()> {
    transaction.execute_batch(
        "CREATE TABLE native_usage_sources (
               scope_key TEXT NOT NULL,
               source_key TEXT NOT NULL,
               modified_ns INTEGER NOT NULL,
               size INTEGER NOT NULL,
               file_id TEXT,
               parsed_bytes INTEGER NOT NULL,
               append_guard TEXT NOT NULL,
               session_count INTEGER NOT NULL,
               sealed INTEGER NOT NULL DEFAULT 0,
               request_context TEXT NOT NULL,
               migration_state INTEGER NOT NULL DEFAULT 0,
               PRIMARY KEY(scope_key, source_key)
             );
             CREATE TABLE native_usage_source_days (
               scope_key TEXT NOT NULL,
               source_key TEXT NOT NULL,
               day TEXT NOT NULL,
               prompt_tokens INTEGER NOT NULL,
               cached_input_tokens INTEGER NOT NULL,
               completion_tokens INTEGER NOT NULL,
               estimated_prompt_tokens INTEGER NOT NULL,
               estimated_completion_tokens INTEGER NOT NULL,
               explicit_records INTEGER NOT NULL,
               estimated_records INTEGER NOT NULL,
               message_count INTEGER NOT NULL,
               PRIMARY KEY(scope_key, source_key, day)
             );
             CREATE INDEX native_usage_source_days_window
               ON native_usage_source_days(scope_key, day);
             CREATE TABLE native_usage_source_models (
               scope_key TEXT NOT NULL,
               source_key TEXT NOT NULL,
               day TEXT NOT NULL,
               model TEXT NOT NULL,
               prompt_tokens INTEGER NOT NULL,
               cached_input_tokens INTEGER NOT NULL,
               completion_tokens INTEGER NOT NULL,
               total_tokens INTEGER NOT NULL,
               estimated_prompt_tokens INTEGER NOT NULL,
               estimated_completion_tokens INTEGER NOT NULL,
               effort TEXT NOT NULL DEFAULT '',
               fast INTEGER NOT NULL DEFAULT -1,
               PRIMARY KEY(scope_key, source_key, day, model, effort, fast)
             );
             CREATE INDEX native_usage_source_models_window
               ON native_usage_source_models(scope_key, day);
             CREATE TABLE native_usage_daily_totals (
               scope_key TEXT NOT NULL,
               day TEXT NOT NULL,
               prompt_tokens INTEGER NOT NULL,
               cached_input_tokens INTEGER NOT NULL,
               completion_tokens INTEGER NOT NULL,
               estimated_prompt_tokens INTEGER NOT NULL,
               estimated_completion_tokens INTEGER NOT NULL,
               explicit_records INTEGER NOT NULL,
               estimated_records INTEGER NOT NULL,
               message_count INTEGER NOT NULL,
               session_count INTEGER NOT NULL,
               PRIMARY KEY(scope_key, day)
             );
             CREATE TABLE native_usage_daily_models (
               scope_key TEXT NOT NULL,
               day TEXT NOT NULL,
               model TEXT NOT NULL,
               prompt_tokens INTEGER NOT NULL,
               cached_input_tokens INTEGER NOT NULL,
               completion_tokens INTEGER NOT NULL,
               total_tokens INTEGER NOT NULL,
               estimated_prompt_tokens INTEGER NOT NULL,
               estimated_completion_tokens INTEGER NOT NULL,
               effort TEXT NOT NULL DEFAULT '',
               fast INTEGER NOT NULL DEFAULT -1,
               PRIMARY KEY(scope_key, day, model, effort, fast)
             );
             CREATE TABLE native_usage_scans (
               scope_key TEXT PRIMARY KEY,
               last_scan_ms INTEGER NOT NULL
             );
             CREATE TABLE native_usage_watermarks (
               scope_key TEXT NOT NULL,
               source_key TEXT NOT NULL,
               usage_key TEXT NOT NULL,
               session_key TEXT NOT NULL,
               model TEXT,
               day TEXT NOT NULL,
               last_prompt INTEGER NOT NULL,
               last_cached INTEGER NOT NULL,
               last_completion INTEGER NOT NULL,
               day_prompt INTEGER NOT NULL,
               day_cached INTEGER NOT NULL,
               day_completion INTEGER NOT NULL,
               effort TEXT NOT NULL DEFAULT '',
               fast INTEGER NOT NULL DEFAULT -1,
               day_variants TEXT NOT NULL,
               PRIMARY KEY(scope_key, source_key, usage_key)
             );",
    )?;
    Ok(())
}

fn add_cursor_and_count_columns(transaction: &Transaction<'_>) -> Result<()> {
    transaction
        .execute_batch("ALTER TABLE native_usage_sources ADD COLUMN snapshot_cursor TEXT;")?;
    for table in [
        "native_usage_source_days",
        "native_usage_daily_totals",
        "native_usage_source_models",
        "native_usage_daily_models",
    ] {
        transaction.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN request_count INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE {table} ADD COLUMN token_unavailable_requests INTEGER NOT NULL DEFAULT 0;"
        ))?;
    }
    Ok(())
}

fn migrate_variant_columns(transaction: &Transaction<'_>) -> Result<()> {
    transaction.execute_batch(
        "ALTER TABLE native_usage_sources ADD COLUMN request_context TEXT NOT NULL DEFAULT '{\"model\":null,\"variant\":{}}';
         ALTER TABLE native_usage_watermarks ADD COLUMN effort TEXT NOT NULL DEFAULT '';
         ALTER TABLE native_usage_watermarks ADD COLUMN fast INTEGER NOT NULL DEFAULT -1;
         ALTER TABLE native_usage_watermarks ADD COLUMN day_variants TEXT NOT NULL DEFAULT '[]';
         UPDATE native_usage_watermarks SET day_variants=json_array(json_array(
           json_array(model,json_object('effort',NULL,'fast',NULL)),
           json_object('prompt',day_prompt,'cached',day_cached,'completion',day_completion)));
         ALTER TABLE native_usage_source_models RENAME TO native_usage_source_models_v7;
         DROP INDEX native_usage_source_models_window;
         CREATE TABLE native_usage_source_models (
           scope_key TEXT NOT NULL,source_key TEXT NOT NULL,day TEXT NOT NULL,model TEXT NOT NULL,
           prompt_tokens INTEGER NOT NULL,cached_input_tokens INTEGER NOT NULL,
           completion_tokens INTEGER NOT NULL,total_tokens INTEGER NOT NULL,
           estimated_prompt_tokens INTEGER NOT NULL,estimated_completion_tokens INTEGER NOT NULL,
           effort TEXT NOT NULL DEFAULT '',fast INTEGER NOT NULL DEFAULT -1,
           PRIMARY KEY(scope_key,source_key,day,model,effort,fast)
         );
         INSERT INTO native_usage_source_models SELECT *, '', -1 FROM native_usage_source_models_v7;
         DROP TABLE native_usage_source_models_v7;
         CREATE INDEX native_usage_source_models_window ON native_usage_source_models(scope_key,day);
         ALTER TABLE native_usage_daily_models RENAME TO native_usage_daily_models_v7;
         CREATE TABLE native_usage_daily_models (
           scope_key TEXT NOT NULL,day TEXT NOT NULL,model TEXT NOT NULL,
           prompt_tokens INTEGER NOT NULL,cached_input_tokens INTEGER NOT NULL,
           completion_tokens INTEGER NOT NULL,total_tokens INTEGER NOT NULL,
           estimated_prompt_tokens INTEGER NOT NULL,estimated_completion_tokens INTEGER NOT NULL,
           effort TEXT NOT NULL DEFAULT '',fast INTEGER NOT NULL DEFAULT -1,
           PRIMARY KEY(scope_key,day,model,effort,fast)
         );
         INSERT INTO native_usage_daily_models SELECT *, '', -1 FROM native_usage_daily_models_v7;
         DROP TABLE native_usage_daily_models_v7;",
    )?;
    Ok(())
}

pub(super) fn cache_is_fresh(
    connection: &Connection,
    scope_key: &str,
    now_ms: u64,
    interval_ms: u64,
) -> Result<bool> {
    let last = connection
        .query_row(
            "SELECT last_scan_ms FROM native_usage_scans WHERE scope_key=?1",
            [scope_key],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    Ok(last.is_some_and(|last| last > 0 && now_ms.saturating_sub(from_i64(last)) < interval_ms))
}

pub(super) fn cache_has_baseline(connection: &Connection, scope_key: &str) -> Result<bool> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM native_usage_scans WHERE scope_key=?1)",
            [scope_key],
            |row| row.get::<_, bool>(0),
        )
        .map_err(Into::into)
}

pub(super) fn migration_counts(connection: &Connection, scope_key: &str) -> Result<(u64, u64)> {
    connection
        .query_row(
            "SELECT COALESCE(SUM(migration_state=1),0), COALESCE(SUM(migration_state IN (3,5)),0)
         FROM native_usage_sources WHERE scope_key=?1",
            [scope_key],
            |row| Ok((from_i64(row.get(0)?), from_i64(row.get(1)?))),
        )
        .map_err(Into::into)
}

pub(super) fn load_sources(
    connection: &Connection,
    scope_key: &str,
) -> Result<BTreeMap<String, CachedSource>> {
    let mut statement = connection.prepare(
        "SELECT source_key, modified_ns, size, file_id, parsed_bytes, append_guard,
                session_count, sealed, request_context, migration_state, snapshot_cursor
         FROM native_usage_sources WHERE scope_key=?1",
    )?;
    let rows = statement.query_map([scope_key], |row| {
        Ok((
            row.get::<_, String>(0)?,
            CachedSource {
                modified_ns: from_i64(row.get(1)?),
                size: from_i64(row.get(2)?),
                file_id: row.get(3)?,
                parsed_bytes: from_i64(row.get(4)?),
                append_guard: row.get(5)?,
                session_count: from_i64(row.get(6)?),
                sealed: row.get::<_, i64>(7)? != 0,
                request_context: json_column(row, 8)?,
                migration_state: row.get(9)?,
                snapshot_cursor: row
                    .get::<_, Option<String>>(10)?
                    .map(|raw| {
                        serde_json::from_str(&raw).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                10,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })
                    })
                    .transpose()?,
            },
        ))
    })?;
    rows.collect::<rusqlite::Result<BTreeMap<_, _>>>()
        .map_err(Into::into)
}

pub(super) fn load_compaction_targets(
    connection: &Connection,
    scope_key: &str,
    before_day: &str,
) -> Result<BTreeSet<String>> {
    let mut statement = connection.prepare(
        "SELECT DISTINCT source_key FROM native_usage_source_days
         WHERE scope_key=?1 AND day<?2",
    )?;
    let rows = statement.query_map(params![scope_key, before_day], |row| {
        row.get::<_, String>(0)
    })?;
    rows.collect::<rusqlite::Result<BTreeSet<_>>>()
        .map_err(Into::into)
}

/// One set of prepared mutation statements shared by the whole refresh
/// transaction. Every SQL invocation is counted so the scan report can prove
/// that statement volume scales with batches instead of per-day or per-model
/// rows.
pub(super) struct RefreshStatements<'a> {
    transaction: &'a Transaction<'a>,
    compact_count: Statement<'a>,
    compact_first_day: Statement<'a>,
    compact_totals: Statement<'a>,
    compact_models: Statement<'a>,
    compact_session: Statement<'a>,
    compact_delete_days: Statement<'a>,
    compact_delete_models: Statement<'a>,
    compact_zero: Statement<'a>,
    seal_first_day: Statement<'a>,
    seal_totals: Statement<'a>,
    seal_models: Statement<'a>,
    seal_session: Statement<'a>,
    seal_delete_days: Statement<'a>,
    seal_delete_models: Statement<'a>,
    seal_mark: Statement<'a>,
    source_upsert: Statement<'a>,
    migration_update: Statement<'a>,
    snapshot_cursor_update: Statement<'a>,
    watermark_delete: Statement<'a>,
    day_upsert: Statement<'a>,
    model_upsert: Statement<'a>,
    replace_delete_days: Statement<'a>,
    replace_delete_models: Statement<'a>,
    watermark_upsert: Statement<'a>,
    mark_scan: Statement<'a>,
    executed: u64,
}

impl<'a> RefreshStatements<'a> {
    pub(super) fn prepare(transaction: &'a Transaction<'a>) -> Result<Self> {
        Ok(Self {
            transaction,
            compact_count: transaction.prepare(
                "SELECT COUNT(*) FROM native_usage_source_days
                 WHERE scope_key=?1 AND source_key=?2 AND day<?3",
            )?,
            compact_first_day: transaction.prepare(
                "SELECT MIN(day) FROM native_usage_source_days
                 WHERE scope_key=?1 AND source_key=?2 AND day<?3",
            )?,
            compact_totals: transaction.prepare(
                "INSERT INTO native_usage_daily_totals
                   SELECT scope_key, day, prompt_tokens, cached_input_tokens,
                          completion_tokens, estimated_prompt_tokens,
                          estimated_completion_tokens, explicit_records,
                          estimated_records, message_count, 0, request_count, token_unavailable_requests
                   FROM native_usage_source_days
                   WHERE scope_key=?1 AND source_key=?2 AND day<?3
                 ON CONFLICT(scope_key,day) DO UPDATE SET
                   prompt_tokens=prompt_tokens+excluded.prompt_tokens,
                   cached_input_tokens=cached_input_tokens+excluded.cached_input_tokens,
                   completion_tokens=completion_tokens+excluded.completion_tokens,
                   estimated_prompt_tokens=estimated_prompt_tokens+excluded.estimated_prompt_tokens,
                   estimated_completion_tokens=estimated_completion_tokens+excluded.estimated_completion_tokens,
                   explicit_records=explicit_records+excluded.explicit_records,
                   estimated_records=estimated_records+excluded.estimated_records,
                   message_count=message_count+excluded.message_count,
                   request_count=request_count+excluded.request_count,
                   token_unavailable_requests=token_unavailable_requests+excluded.token_unavailable_requests",
            )?,
            compact_models: transaction.prepare(
                "INSERT INTO native_usage_daily_models
                   SELECT scope_key, day, model, prompt_tokens, cached_input_tokens,
                          completion_tokens, total_tokens, estimated_prompt_tokens,
                          estimated_completion_tokens, effort, fast, request_count, token_unavailable_requests
                   FROM native_usage_source_models
                   WHERE scope_key=?1 AND source_key=?2 AND day<?3
                 ON CONFLICT(scope_key,day,model,effort,fast) DO UPDATE SET
                   prompt_tokens=prompt_tokens+excluded.prompt_tokens,
                   cached_input_tokens=cached_input_tokens+excluded.cached_input_tokens,
                   completion_tokens=completion_tokens+excluded.completion_tokens,
                   total_tokens=total_tokens+excluded.total_tokens,
                   estimated_prompt_tokens=estimated_prompt_tokens+excluded.estimated_prompt_tokens,
                   estimated_completion_tokens=estimated_completion_tokens+excluded.estimated_completion_tokens,
                   request_count=request_count+excluded.request_count,
                   token_unavailable_requests=token_unavailable_requests+excluded.token_unavailable_requests",
            )?,
            compact_session: transaction.prepare(
                "UPDATE native_usage_daily_totals
                 SET session_count=session_count+?3
                 WHERE scope_key=?1 AND day=?2",
            )?,
            compact_delete_days: transaction.prepare(
                "DELETE FROM native_usage_source_days
                 WHERE scope_key=?1 AND source_key=?2 AND day<?3",
            )?,
            compact_delete_models: transaction.prepare(
                "DELETE FROM native_usage_source_models
                 WHERE scope_key=?1 AND source_key=?2 AND day<?3",
            )?,
            compact_zero: transaction.prepare(
                "UPDATE native_usage_sources SET session_count=0
                 WHERE scope_key=?1 AND source_key=?2",
            )?,
            seal_first_day: transaction.prepare(
                "SELECT MIN(day) FROM native_usage_source_days
                 WHERE scope_key=?1 AND source_key=?2",
            )?,
            seal_totals: transaction.prepare(
                "INSERT INTO native_usage_daily_totals
                   SELECT scope_key, day, prompt_tokens, cached_input_tokens,
                          completion_tokens, estimated_prompt_tokens,
                          estimated_completion_tokens, explicit_records,
                          estimated_records, message_count, 0, request_count, token_unavailable_requests
                   FROM native_usage_source_days WHERE scope_key=?1 AND source_key=?2
                 ON CONFLICT(scope_key,day) DO UPDATE SET
                   prompt_tokens=prompt_tokens+excluded.prompt_tokens,
                   cached_input_tokens=cached_input_tokens+excluded.cached_input_tokens,
                   completion_tokens=completion_tokens+excluded.completion_tokens,
                   estimated_prompt_tokens=estimated_prompt_tokens+excluded.estimated_prompt_tokens,
                   estimated_completion_tokens=estimated_completion_tokens+excluded.estimated_completion_tokens,
                   explicit_records=explicit_records+excluded.explicit_records,
                   estimated_records=estimated_records+excluded.estimated_records,
                   message_count=message_count+excluded.message_count,
                   request_count=request_count+excluded.request_count,
                   token_unavailable_requests=token_unavailable_requests+excluded.token_unavailable_requests",
            )?,
            seal_models: transaction.prepare(
                "INSERT INTO native_usage_daily_models
                   SELECT scope_key, day, model, prompt_tokens, cached_input_tokens,
                          completion_tokens, total_tokens, estimated_prompt_tokens,
                          estimated_completion_tokens, effort, fast, request_count, token_unavailable_requests
                   FROM native_usage_source_models WHERE scope_key=?1 AND source_key=?2
                 ON CONFLICT(scope_key,day,model,effort,fast) DO UPDATE SET
                   prompt_tokens=prompt_tokens+excluded.prompt_tokens,
                   cached_input_tokens=cached_input_tokens+excluded.cached_input_tokens,
                   completion_tokens=completion_tokens+excluded.completion_tokens,
                   total_tokens=total_tokens+excluded.total_tokens,
                   estimated_prompt_tokens=estimated_prompt_tokens+excluded.estimated_prompt_tokens,
                   estimated_completion_tokens=estimated_completion_tokens+excluded.estimated_completion_tokens,
                   request_count=request_count+excluded.request_count,
                   token_unavailable_requests=token_unavailable_requests+excluded.token_unavailable_requests",
            )?,
            seal_session: transaction.prepare(
                "UPDATE native_usage_daily_totals
                 SET session_count=session_count+?3
                 WHERE scope_key=?1 AND day=?2",
            )?,
            seal_delete_days: transaction.prepare(
                "DELETE FROM native_usage_source_days WHERE scope_key=?1 AND source_key=?2",
            )?,
            seal_delete_models: transaction.prepare(
                "DELETE FROM native_usage_source_models WHERE scope_key=?1 AND source_key=?2",
            )?,
            seal_mark: transaction.prepare(
                "UPDATE native_usage_sources SET sealed=1
                 WHERE scope_key=?1 AND source_key=?2",
            )?,
            source_upsert: transaction.prepare(
                "INSERT INTO native_usage_sources (scope_key,source_key,modified_ns,size,file_id,parsed_bytes,append_guard,session_count,sealed,request_context) VALUES(
                   ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10
                 ) ON CONFLICT(scope_key,source_key) DO UPDATE SET
                   modified_ns=excluded.modified_ns,
                   size=excluded.size,
                   file_id=excluded.file_id,
                   parsed_bytes=excluded.parsed_bytes,
                   append_guard=excluded.append_guard,
                   session_count=excluded.session_count,
                   sealed=excluded.sealed,
                   request_context=excluded.request_context",
            )?,
            migration_update: transaction.prepare(
                "UPDATE native_usage_sources SET migration_state=?3 WHERE scope_key=?1 AND source_key=?2",
            )?,
            snapshot_cursor_update: transaction.prepare(
                "UPDATE native_usage_sources SET snapshot_cursor=?3 WHERE scope_key=?1 AND source_key=?2",
            )?,
            watermark_delete: transaction.prepare(
                "DELETE FROM native_usage_watermarks WHERE scope_key=?1 AND source_key=?2",
            )?,
            day_upsert: transaction.prepare(
                "INSERT INTO native_usage_source_days VALUES(
                   ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13
                 )
                 ON CONFLICT(scope_key,source_key,day) DO UPDATE SET
                   prompt_tokens=prompt_tokens+excluded.prompt_tokens,
                   cached_input_tokens=cached_input_tokens+excluded.cached_input_tokens,
                   completion_tokens=completion_tokens+excluded.completion_tokens,
                   estimated_prompt_tokens=estimated_prompt_tokens+excluded.estimated_prompt_tokens,
                   estimated_completion_tokens=estimated_completion_tokens+excluded.estimated_completion_tokens,
                   explicit_records=explicit_records+excluded.explicit_records,
                   estimated_records=estimated_records+excluded.estimated_records,
                   message_count=message_count+excluded.message_count,
                   request_count=request_count+excluded.request_count,
                   token_unavailable_requests=token_unavailable_requests+excluded.token_unavailable_requests",
            )?,
            model_upsert: transaction.prepare(
                "INSERT INTO native_usage_source_models VALUES(
                   ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14
                 )
                 ON CONFLICT(scope_key,source_key,day,model,effort,fast) DO UPDATE SET
                   prompt_tokens=prompt_tokens+excluded.prompt_tokens,
                   cached_input_tokens=cached_input_tokens+excluded.cached_input_tokens,
                   completion_tokens=completion_tokens+excluded.completion_tokens,
                   total_tokens=total_tokens+excluded.total_tokens,
                   estimated_prompt_tokens=estimated_prompt_tokens+excluded.estimated_prompt_tokens,
                   estimated_completion_tokens=estimated_completion_tokens+excluded.estimated_completion_tokens,
                   request_count=request_count+excluded.request_count,
                   token_unavailable_requests=token_unavailable_requests+excluded.token_unavailable_requests",
            )?,
            replace_delete_days: transaction.prepare(
                "DELETE FROM native_usage_source_days WHERE scope_key=?1 AND source_key=?2",
            )?,
            replace_delete_models: transaction.prepare(
                "DELETE FROM native_usage_source_models WHERE scope_key=?1 AND source_key=?2",
            )?,
            watermark_upsert: transaction.prepare(
                "INSERT INTO native_usage_watermarks VALUES(
                   ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15
                 ) ON CONFLICT(scope_key,source_key,usage_key) DO UPDATE SET
                   session_key=excluded.session_key,model=excluded.model,day=excluded.day,
                   last_prompt=excluded.last_prompt,last_cached=excluded.last_cached,
                   last_completion=excluded.last_completion,day_prompt=excluded.day_prompt,
                   day_cached=excluded.day_cached,day_completion=excluded.day_completion,
                   effort=excluded.effort,fast=excluded.fast,day_variants=excluded.day_variants",
            )?,
            mark_scan: transaction.prepare(
                "INSERT INTO native_usage_scans(scope_key,last_scan_ms) VALUES(?1,?2)
                 ON CONFLICT(scope_key) DO UPDATE SET last_scan_ms=excluded.last_scan_ms",
            )?,
            executed: 0,
        })
    }

    pub(super) fn transaction(&self) -> &Transaction<'a> {
        self.transaction
    }

    pub(super) fn executed(&self) -> u64 {
        self.executed
    }

    fn count(&mut self) {
        self.executed = self.executed.saturating_add(1);
    }

    /// Reduces every completed local day for a still-live source into the
    /// shared agent/day/model tables. Only the current day's source rows
    /// remain mutable.
    pub(super) fn compact(
        &mut self,
        scope_key: &str,
        source_key: &str,
        before_day: &str,
        session_count: u64,
    ) -> Result<u64> {
        let counted = self
            .compact_count
            .query_row(params![scope_key, source_key, before_day], |row| {
                row.get::<_, i64>(0)
            });
        self.count();
        let compacted_days = counted?;
        if compacted_days <= 0 {
            return Ok(0);
        }
        let first = self
            .compact_first_day
            .query_row(params![scope_key, source_key, before_day], |row| {
                row.get::<_, Option<String>>(0)
            });
        self.count();
        let first_day = first?;
        let totals = self
            .compact_totals
            .execute(params![scope_key, source_key, before_day]);
        self.count();
        totals?;
        let models = self
            .compact_models
            .execute(params![scope_key, source_key, before_day]);
        self.count();
        models?;
        if let Some(first_day) = first_day {
            let session =
                self.compact_session
                    .execute(params![scope_key, first_day, to_i64(session_count),]);
            self.count();
            session?;
        }
        let days = self
            .compact_delete_days
            .execute(params![scope_key, source_key, before_day]);
        self.count();
        days?;
        let models = self
            .compact_delete_models
            .execute(params![scope_key, source_key, before_day]);
        self.count();
        models?;
        let zero = self.compact_zero.execute(params![scope_key, source_key]);
        self.count();
        zero?;
        Ok(from_i64(compacted_days))
    }

    pub(super) fn seal(
        &mut self,
        scope_key: &str,
        source_key: &str,
        session_count: u64,
    ) -> Result<()> {
        let first = self
            .seal_first_day
            .query_row(params![scope_key, source_key], |row| {
                row.get::<_, Option<String>>(0)
            });
        self.count();
        let first_day = first?;
        let totals = self.seal_totals.execute(params![scope_key, source_key]);
        self.count();
        totals?;
        let models = self.seal_models.execute(params![scope_key, source_key]);
        self.count();
        models?;
        if let Some(first_day) = first_day {
            let session =
                self.seal_session
                    .execute(params![scope_key, first_day, to_i64(session_count),]);
            self.count();
            session?;
        }
        let days = self
            .seal_delete_days
            .execute(params![scope_key, source_key]);
        self.count();
        days?;
        let models = self
            .seal_delete_models
            .execute(params![scope_key, source_key]);
        self.count();
        models?;
        let mark = self.seal_mark.execute(params![scope_key, source_key]);
        self.count();
        mark?;
        Ok(())
    }

    pub(super) fn save_source(
        &mut self,
        scope_key: &str,
        source_key: &str,
        metadata: &SourceMetadata,
        parsed_bytes: u64,
        append_guard: &str,
        session_count: u64,
        request_context: &UsageRequestContext,
    ) -> Result<()> {
        let saved = self.source_upsert.execute(params![
            scope_key,
            source_key,
            to_i64(metadata.modified_ns),
            to_i64(metadata.size),
            metadata.file_id,
            to_i64(parsed_bytes),
            append_guard,
            to_i64(session_count),
            i64::from(false),
            serde_json::to_string(request_context)?,
        ]);
        self.count();
        saved?;
        Ok(())
    }

    pub(super) fn save_migration_state(
        &mut self,
        scope_key: &str,
        source_key: &str,
        state: i64,
    ) -> Result<()> {
        self.migration_update
            .execute(params![scope_key, source_key, state])?;
        self.count();
        Ok(())
    }

    pub(super) fn save_snapshot_cursor(
        &mut self,
        scope_key: &str,
        source_key: &str,
        cursor: &super::snapshot_cursor::SnapshotCursor,
    ) -> Result<()> {
        self.snapshot_cursor_update.execute(params![
            scope_key,
            source_key,
            serde_json::to_string(cursor)?
        ])?;
        self.count();
        Ok(())
    }

    pub(super) fn clear_watermarks(&mut self, scope_key: &str, source_key: &str) -> Result<()> {
        self.watermark_delete
            .execute(params![scope_key, source_key])?;
        self.count();
        Ok(())
    }

    pub(super) fn replace_rollup(
        &mut self,
        scope_key: &str,
        source_key: &str,
        summary: &HistoryUsageSummary,
    ) -> Result<()> {
        let days = self
            .replace_delete_days
            .execute(params![scope_key, source_key]);
        self.count();
        days?;
        let models = self
            .replace_delete_models
            .execute(params![scope_key, source_key]);
        self.count();
        models?;
        self.add_rollup(scope_key, source_key, summary)
    }

    pub(super) fn add_rollup(
        &mut self,
        scope_key: &str,
        source_key: &str,
        summary: &HistoryUsageSummary,
    ) -> Result<()> {
        for (day, usage) in &summary.daily_usage {
            if usage.total_tokens == 0
                && usage.request_count == 0
                && usage.token_unavailable_requests == 0
            {
                continue;
            }
            let day_saved = self.day_upsert.execute(params![
                scope_key,
                source_key,
                day,
                to_i64(usage.prompt_tokens),
                to_i64(usage.cached_input_tokens.min(usage.prompt_tokens)),
                to_i64(usage.completion_tokens),
                to_i64(usage.estimated_prompt_tokens),
                to_i64(usage.estimated_completion_tokens),
                to_i64(usage.explicit_records),
                to_i64(usage.estimated_records),
                to_i64(usage.message_count),
                to_i64(usage.request_count),
                to_i64(usage.token_unavailable_requests),
            ]);
            self.count();
            day_saved?;
            self.add_model_rows(scope_key, source_key, day, usage)?;
        }
        Ok(())
    }

    pub(super) fn replace_model_rows(
        &mut self,
        scope_key: &str,
        source_key: &str,
        day: &str,
        usage: &DailyUsageSummary,
    ) -> Result<()> {
        self.transaction.execute(
            "DELETE FROM native_usage_source_models WHERE scope_key=?1 AND source_key=?2 AND day=?3",
            params![scope_key, source_key, day],
        )?;
        self.count();
        self.add_model_rows(scope_key, source_key, day, usage)
    }

    fn add_model_rows(
        &mut self,
        scope_key: &str,
        source_key: &str,
        day: &str,
        usage: &DailyUsageSummary,
    ) -> Result<()> {
        for ((model, variant), model_usage) in &usage.model_variants {
            let model_saved = self.model_upsert.execute(params![
                scope_key,
                source_key,
                day,
                model,
                to_i64(model_usage.prompt_tokens),
                to_i64(
                    model_usage
                        .cached_input_tokens
                        .min(model_usage.prompt_tokens)
                ),
                to_i64(model_usage.completion_tokens),
                to_i64(model_usage.total_tokens),
                to_i64(model_usage.estimated_prompt_tokens),
                to_i64(model_usage.estimated_completion_tokens),
                variant.effort.as_deref().unwrap_or_default(),
                variant.fast.map(i64::from).unwrap_or(-1),
                to_i64(model_usage.request_count),
                to_i64(model_usage.token_unavailable_requests),
            ]);
            self.count();
            model_saved?;
        }
        Ok(())
    }

    pub(super) fn save_watermark(
        &mut self,
        scope_key: &str,
        source_key: &str,
        usage_key: &str,
        state: &super::watermark::Watermark,
    ) -> Result<()> {
        let saved = self.watermark_upsert.execute(params![
            scope_key,
            source_key,
            usage_key,
            state.session_key,
            state.model,
            state.day,
            to_i64(state.last.prompt),
            to_i64(state.last.cached),
            to_i64(state.last.completion),
            to_i64(state.day_total.prompt),
            to_i64(state.day_total.cached),
            to_i64(state.day_total.completion),
            state.variant.effort.as_deref().unwrap_or_default(),
            state.variant.fast.map(i64::from).unwrap_or(-1),
            serde_json::to_string(&state.day_variants.iter().collect::<Vec<_>>())?,
        ]);
        self.count();
        saved?;
        Ok(())
    }

    pub(super) fn mark_scan(&mut self, scope_key: &str, now_ms: u64) -> Result<()> {
        let marked = self.mark_scan.execute(params![scope_key, to_i64(now_ms)]);
        self.count();
        marked?;
        Ok(())
    }
}

pub(super) fn aggregate_usage(
    connection: &mut Connection,
    scope_key: &str,
    window: &UsageWindow,
) -> Result<HistoryUsageSummary> {
    let snapshot = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
    let mut summary = HistoryUsageSummary {
        source: Some("native-metadata-first-usage-cache"),
        ..HistoryUsageSummary::default()
    };
    {
        let mut statement = snapshot.prepare(
            "SELECT day,
                    SUM(prompt_tokens), SUM(cached_input_tokens),
                    SUM(completion_tokens), SUM(estimated_prompt_tokens),
                    SUM(estimated_completion_tokens), SUM(explicit_records),
                    SUM(estimated_records), SUM(message_count), SUM(session_count),
                    SUM(request_count), SUM(token_unavailable_requests)
             FROM (
               SELECT day,prompt_tokens,cached_input_tokens,completion_tokens,
                      estimated_prompt_tokens,estimated_completion_tokens,
                      explicit_records,estimated_records,message_count,session_count,request_count,token_unavailable_requests
               FROM native_usage_daily_totals WHERE scope_key=?1
               UNION ALL
               SELECT day,prompt_tokens,cached_input_tokens,completion_tokens,
                      estimated_prompt_tokens,estimated_completion_tokens,
                      explicit_records,estimated_records,message_count,0,request_count,token_unavailable_requests
               FROM native_usage_source_days WHERE scope_key=?1
             )
             WHERE day>=?2 AND day<=?3 GROUP BY day ORDER BY day",
        )?;
        let rows = statement.query_map(params![scope_key, &window.start, &window.end], |row| {
            Ok((
                row.get::<_, String>(0)?,
                from_i64(row.get(1)?),
                from_i64(row.get(2)?),
                from_i64(row.get(3)?),
                from_i64(row.get(4)?),
                from_i64(row.get(5)?),
                from_i64(row.get(6)?),
                from_i64(row.get(7)?),
                from_i64(row.get(8)?),
                from_i64(row.get(9)?),
                from_i64(row.get(10)?),
                from_i64(row.get(11)?),
            ))
        })?;
        for row in rows {
            let (
                day,
                mut prompt,
                cached,
                mut completion,
                mut estimated_prompt,
                mut estimated_completion,
                explicit_records,
                mut estimated_records,
                mut messages,
                sessions,
                requests,
                unavailable,
            ) = row?;
            if explicit_records > 0 {
                prompt = prompt.saturating_sub(estimated_prompt);
                completion = completion.saturating_sub(estimated_completion);
                messages = messages.saturating_sub(estimated_records);
                estimated_prompt = 0;
                estimated_completion = 0;
                estimated_records = 0;
            }
            let explicit_prompt = prompt.saturating_sub(estimated_prompt);
            let explicit_completion = completion.saturating_sub(estimated_completion);
            summary.explicit_prompt_tokens = summary
                .explicit_prompt_tokens
                .saturating_add(explicit_prompt);
            summary.explicit_cached_input_tokens = summary
                .explicit_cached_input_tokens
                .saturating_add(cached.min(explicit_prompt));
            summary.explicit_completion_tokens = summary
                .explicit_completion_tokens
                .saturating_add(explicit_completion);
            summary.explicit_total_tokens = summary
                .explicit_total_tokens
                .saturating_add(explicit_prompt.saturating_add(explicit_completion));
            summary.estimated_prompt_tokens = summary
                .estimated_prompt_tokens
                .saturating_add(estimated_prompt);
            summary.estimated_completion_tokens = summary
                .estimated_completion_tokens
                .saturating_add(estimated_completion);
            summary.estimated_total_tokens = summary
                .estimated_total_tokens
                .saturating_add(estimated_prompt.saturating_add(estimated_completion));
            summary.explicit_records = summary.explicit_records.saturating_add(explicit_records);
            summary.estimated_records = summary.estimated_records.saturating_add(estimated_records);
            summary.message_count = summary.message_count.saturating_add(messages);
            summary.session_count = summary.session_count.saturating_add(sessions);
            let daily = summary.daily_usage.entry(day).or_default();
            daily.prompt_tokens = prompt;
            daily.cached_input_tokens = cached.min(prompt);
            daily.completion_tokens = completion;
            daily.total_tokens = prompt.saturating_add(completion);
            daily.explicit_records = explicit_records;
            daily.estimated_records = estimated_records;
            daily.estimated_prompt_tokens = estimated_prompt;
            daily.estimated_completion_tokens = estimated_completion;
            daily.message_count = messages;
            daily.request_count = requests;
            daily.token_unavailable_requests = unavailable;
            summary.token_unavailable_records = summary
                .token_unavailable_records
                .saturating_add(unavailable);
        }
    }
    {
        let mut statement = snapshot.prepare(
            "SELECT day,model,
                    SUM(prompt_tokens),SUM(cached_input_tokens),
                    SUM(completion_tokens),SUM(total_tokens),
                    SUM(estimated_prompt_tokens),SUM(estimated_completion_tokens),effort,fast,
                    SUM(request_count),SUM(token_unavailable_requests)
             FROM (
               SELECT day,model,prompt_tokens,cached_input_tokens,
                      completion_tokens,total_tokens,estimated_prompt_tokens,
                      estimated_completion_tokens,effort,fast,request_count,token_unavailable_requests
               FROM native_usage_daily_models WHERE scope_key=?1
               UNION ALL
               SELECT day,model,prompt_tokens,cached_input_tokens,
                      completion_tokens,total_tokens,estimated_prompt_tokens,
                      estimated_completion_tokens,effort,fast,request_count,token_unavailable_requests
               FROM native_usage_source_models WHERE scope_key=?1
             )
             WHERE day>=?2 AND day<=?3 GROUP BY day,model,effort,fast ORDER BY day,model,effort,fast",
        )?;
        let rows = statement.query_map(params![scope_key, &window.start, &window.end], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                from_i64(row.get(2)?),
                from_i64(row.get(3)?),
                from_i64(row.get(4)?),
                from_i64(row.get(5)?),
                from_i64(row.get(6)?),
                from_i64(row.get(7)?),
                variant_from_columns(row.get(8)?, row.get(9)?),
                from_i64(row.get(10)?),
                from_i64(row.get(11)?),
            ))
        })?;
        for row in rows {
            let (
                day,
                model,
                mut prompt,
                mut cached,
                mut completion,
                mut total,
                mut estimated_prompt,
                mut estimated_completion,
                variant,
                requests,
                unavailable,
            ) = row?;
            if summary
                .daily_usage
                .get(&day)
                .is_some_and(|usage| usage.explicit_records > 0)
            {
                prompt = prompt.saturating_sub(estimated_prompt);
                completion = completion.saturating_sub(estimated_completion);
                total = prompt.saturating_add(completion);
                cached = cached.min(prompt);
                estimated_prompt = 0;
                estimated_completion = 0;
            }
            summary
                .daily_usage
                .entry(day)
                .or_default()
                .add_model_variant_totals(
                    model,
                    variant,
                    ModelTokenUsageSummary {
                        prompt_tokens: prompt,
                        cached_input_tokens: cached,
                        completion_tokens: completion,
                        total_tokens: total,
                        estimated_prompt_tokens: estimated_prompt,
                        estimated_completion_tokens: estimated_completion,
                        request_count: requests,
                        token_unavailable_requests: unavailable,
                    },
                );
        }
    }
    summary.session_count = summary.session_count.saturating_add(
        snapshot
            .query_row(
                "SELECT COALESCE(SUM(session_count),0) FROM native_usage_sources s
             WHERE scope_key=?1 AND sealed=0 AND EXISTS(
               SELECT 1 FROM native_usage_source_days d
               WHERE d.scope_key=s.scope_key AND d.source_key=s.source_key
                 AND d.day>=?2 AND d.day<=?3
             )",
                params![scope_key, &window.start, &window.end],
                |row| row.get::<_, i64>(0),
            )
            .map(from_i64)?,
    );
    let source_count = snapshot.query_row(
        "SELECT COUNT(*) FROM native_usage_sources WHERE scope_key=?1",
        [scope_key],
        |row| row.get::<_, i64>(0),
    )?;
    snapshot.commit()?;
    if source_count > 0 {
        summary
            .source_paths
            .insert("native-metadata-first-usage-store".to_owned());
    }
    Ok(summary)
}

pub(super) fn reclaim_space(connection: &Connection) -> Result<()> {
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    let page_count =
        connection.pragma_query_value(None, "page_count", |row| row.get::<_, u64>(0))?;
    let free_pages =
        connection.pragma_query_value(None, "freelist_count", |row| row.get::<_, u64>(0))?;
    if page_count > 0 && free_pages.saturating_mul(4) >= page_count {
        connection.execute_batch("VACUUM;")?;
    } else if free_pages > 0 {
        connection.execute_batch("PRAGMA incremental_vacuum(256);")?;
    }
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    Ok(())
}

fn to_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

pub(super) fn variant_from_columns(effort: String, fast: i64) -> UsageVariant {
    UsageVariant {
        effort: (!effort.is_empty()).then_some(effort),
        fast: match fast {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        },
    }
}

pub(super) fn json_column<T: serde::de::DeserializeOwned>(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<T> {
    let text: String = row.get(index)?;
    serde_json::from_str(&text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn from_i64(value: i64) -> u64 {
    value.max(0) as u64
}

#[cfg(test)]
#[path = "cache_variant_tests.rs"]
mod variant_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use licoup_statistics::contract::MessageUsage;
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_DATABASE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn temp_database() -> PathBuf {
        std::env::temp_dir().join(format!(
            "lico-native-usage-cache-{}-{}-{}.sqlite3",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            TEMP_DATABASE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn opening_rollup_cache_preserves_other_ledger_files() {
        let root = temp_database();
        fs::create_dir_all(&root).unwrap();
        const LEGACY_CACHE_FILE_NAME: &str = "agent-usage-exact-v1.sqlite3";
        for suffix in ["", "-wal", "-shm"] {
            fs::write(
                root.join(format!("{LEGACY_CACHE_FILE_NAME}{suffix}")),
                b"legacy",
            )
            .unwrap();
        }
        let path = cache_path(&root);
        let connection = open_cache_database(&path).unwrap();
        for suffix in ["", "-wal", "-shm"] {
            assert!(
                root.join(format!("{LEGACY_CACHE_FILE_NAME}{suffix}"))
                    .exists()
            );
        }
        drop(connection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sealed_source_keeps_only_day_and_model_rollups() {
        let path = temp_database();
        let mut connection = open_cache_database(&path).unwrap();
        let transaction = connection.transaction().unwrap();
        let mut statements = RefreshStatements::prepare(&transaction).unwrap();
        let mut summary = HistoryUsageSummary::default();
        summary.add(
            MessageUsage {
                prompt_tokens: 10,
                cached_input_tokens: 4,
                completion_tokens: 2,
                total_tokens: 12,
                model: Some("model-a".to_owned()),
                variant: Default::default(),
                accuracy: Default::default(),
            },
            Some("2026-07-14".to_owned()),
        );
        statements.add_rollup("scope", "source", &summary).unwrap();
        statements
            .save_source(
                "scope",
                "source",
                &SourceMetadata {
                    modified_ns: 1,
                    size: 1,
                    file_id: None,
                },
                1,
                "guard",
                1,
                &UsageRequestContext::default(),
            )
            .unwrap();
        statements.seal("scope", "source", 1).unwrap();
        assert_eq!(
            transaction
                .query_row("SELECT COUNT(*) FROM native_usage_source_days", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
        drop(statements);
        transaction.commit().unwrap();
        let window = UsageWindow::from_params(&json!({
            "now": "2026-07-15T12:00:00Z",
            "historyDays": 2
        }));
        let aggregate = aggregate_usage(&mut connection, "scope", &window).unwrap();
        assert_eq!(aggregate.total_tokens(), 12);
        assert_eq!(aggregate.session_count, 1);
        drop(connection);
        for candidate in [
            path.clone(),
            PathBuf::from(format!("{}-wal", path.to_string_lossy())),
            PathBuf::from(format!("{}-shm", path.to_string_lossy())),
        ] {
            let _ = fs::remove_file(candidate);
        }
    }

    #[test]
    fn day_compaction_keeps_only_today_mutable() {
        let path = temp_database();
        let mut connection = open_cache_database(&path).unwrap();
        let transaction = connection.transaction().unwrap();
        let mut statements = RefreshStatements::prepare(&transaction).unwrap();
        let mut summary = HistoryUsageSummary::default();
        for (day, prompt, completion) in [("2026-07-14", 10, 2), ("2026-07-15", 20, 3)] {
            summary.add(
                MessageUsage {
                    prompt_tokens: prompt,
                    cached_input_tokens: 0,
                    completion_tokens: completion,
                    total_tokens: prompt + completion,
                    model: Some("model-a".to_owned()),
                    variant: Default::default(),
                    accuracy: Default::default(),
                },
                Some(day.to_owned()),
            );
        }
        statements.add_rollup("scope", "source", &summary).unwrap();
        statements
            .save_source(
                "scope",
                "source",
                &SourceMetadata {
                    modified_ns: 1,
                    size: 1,
                    file_id: None,
                },
                1,
                "guard",
                1,
                &UsageRequestContext::default(),
            )
            .unwrap();

        assert_eq!(
            statements
                .compact("scope", "source", "2026-07-15", 1)
                .unwrap(),
            1
        );
        assert_eq!(
            transaction
                .query_row("SELECT day FROM native_usage_source_days", [], |row| row
                    .get::<_, String>(
                    0
                ),)
                .unwrap(),
            "2026-07-15"
        );
        assert_eq!(
            transaction
                .query_row("SELECT day FROM native_usage_daily_totals", [], |row| {
                    row.get::<_, String>(0)
                },)
                .unwrap(),
            "2026-07-14"
        );
        drop(statements);
        transaction.commit().unwrap();
        let window = UsageWindow::from_params(&json!({
            "now": "2026-07-15T12:00:00Z",
            "historyDays": 2
        }));
        assert_eq!(
            aggregate_usage(&mut connection, "scope", &window)
                .unwrap()
                .total_tokens(),
            35
        );
        drop(connection);
        for candidate in [
            path.clone(),
            PathBuf::from(format!("{}-wal", path.to_string_lossy())),
            PathBuf::from(format!("{}-shm", path.to_string_lossy())),
        ] {
            let _ = fs::remove_file(candidate);
        }
    }
}
