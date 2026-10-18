//! Canonical schema creation and upgrades from published stores.

use super::*;
use std::collections::BTreeSet;

#[cfg(test)]
mod tests;

/// Canonical Conversation table and index layout. Shared by the schema
/// initializer and the synthetic versioned fixtures used by migration tests.
pub(super) const CONVERSATION_SCHEMA_TABLES: &str = "
         CREATE TABLE IF NOT EXISTS principals (
           id TEXT PRIMARY KEY, kind TEXT NOT NULL CHECK(kind IN ('human','agent')),
           display_name TEXT NOT NULL, agent_id TEXT, created_at INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS conversations (
           id TEXT PRIMARY KEY, title TEXT NOT NULL, archived INTEGER NOT NULL DEFAULT 0 CHECK(archived IN (0,1)),
           pinned INTEGER NOT NULL DEFAULT 0 CHECK(pinned IN (0,1)),
           is_group INTEGER NOT NULL DEFAULT 0 CHECK(is_group IN (0,1)), strategy_revision TEXT,
           assistant_membership_id TEXT REFERENCES memberships(id),
           revision INTEGER NOT NULL DEFAULT 0,
           created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS conversations_updated_idx ON conversations(updated_at DESC, id DESC);
         CREATE TABLE IF NOT EXISTS memberships (
           id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
           principal_id TEXT NOT NULL REFERENCES principals(id), access TEXT NOT NULL CHECK(access IN ('owner','member')),
           status TEXT NOT NULL CHECK(status IN ('active','left')), joined_at INTEGER NOT NULL, left_at INTEGER
         );
         CREATE UNIQUE INDEX IF NOT EXISTS memberships_active_unique ON memberships(conversation_id, principal_id) WHERE status='active';
         CREATE INDEX IF NOT EXISTS memberships_conversation_idx ON memberships(conversation_id, status, joined_at);
         CREATE TABLE IF NOT EXISTS membership_profiles (
           membership_id TEXT PRIMARY KEY REFERENCES memberships(id) ON DELETE CASCADE,
           revision INTEGER NOT NULL,
           responsibility TEXT NOT NULL DEFAULT 'member' CHECK(responsibility IN ('assistant','member')),
           required_capabilities TEXT NOT NULL DEFAULT '[]',
           preferred_capabilities TEXT NOT NULL DEFAULT '[]',
           skill_references TEXT NOT NULL DEFAULT '[]',
           preferred_model TEXT, preferred_reasoning_effort TEXT,
           preferred_environment TEXT, preferred_permission_mode TEXT,
           preferred_instructions TEXT, preferred_context_window INTEGER,
           updated_at INTEGER NOT NULL
         );
         CREATE INDEX IF NOT EXISTS membership_profiles_membership_idx
           ON membership_profiles(membership_id, revision);
         CREATE TABLE IF NOT EXISTS events (
           id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
           sequence INTEGER NOT NULL, author_membership_id TEXT REFERENCES memberships(id), kind TEXT NOT NULL,
           causation_id TEXT, correlation_id TEXT,
           created_at INTEGER NOT NULL, finalized INTEGER NOT NULL DEFAULT 0 CHECK(finalized IN (0,1)),
           UNIQUE(conversation_id, sequence)
         );
         CREATE INDEX IF NOT EXISTS events_conversation_idx ON events(conversation_id, sequence);
         CREATE TABLE IF NOT EXISTS event_parts (
           id TEXT PRIMARY KEY, event_id TEXT NOT NULL REFERENCES events(id) ON DELETE CASCADE,
           ordinal INTEGER NOT NULL, kind TEXT NOT NULL, content TEXT NOT NULL,
           runtime_cursor INTEGER, execution_kind TEXT, created_at INTEGER NOT NULL,
           UNIQUE(event_id, ordinal)
         );
         CREATE INDEX IF NOT EXISTS event_parts_event_idx ON event_parts(event_id, ordinal);
         CREATE TABLE IF NOT EXISTS direct_turns (
           id TEXT PRIMARY KEY,
           conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
           source_event_id TEXT NOT NULL REFERENCES events(id) ON DELETE CASCADE,
           membership_id TEXT NOT NULL REFERENCES memberships(id),
           state TEXT NOT NULL, ordinal INTEGER NOT NULL,
           UNIQUE(source_event_id, membership_id)
         );
         CREATE INDEX IF NOT EXISTS direct_turns_pending_idx
           ON direct_turns(state, conversation_id, ordinal);
         CREATE VIRTUAL TABLE IF NOT EXISTS event_search USING fts5(event_id UNINDEXED, conversation_id UNINDEXED, content);
         CREATE TABLE IF NOT EXISTS source_links (
           id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
           source_kind TEXT NOT NULL, native_identity TEXT NOT NULL,
           UNIQUE(source_kind, native_identity)
         );
         CREATE TABLE IF NOT EXISTS runtime_bindings (
           id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
           membership_id TEXT NOT NULL REFERENCES memberships(id) ON DELETE CASCADE, lane TEXT NOT NULL,
           availability TEXT NOT NULL, safe_reason TEXT,
           runtime_session_id TEXT, runtime_conversation_path TEXT, working_directory TEXT,
           UNIQUE(conversation_id, membership_id, lane)
         );
         CREATE TABLE IF NOT EXISTS conversation_dispatches (
           id TEXT PRIMARY KEY,
           conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
           membership_id TEXT NOT NULL REFERENCES memberships(id),
           operation TEXT NOT NULL,
           state TEXT NOT NULL CHECK(state IN ('accepted','running','completed','failed','cancel-requested','cancelled')),
           session_mode TEXT NOT NULL CHECK(session_mode IN ('new','resume')),
           runtime_conversation_path TEXT, error_code TEXT,
           request_payload TEXT, terminal_payload TEXT, native_provenance TEXT,
           created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
         );
          CREATE INDEX IF NOT EXISTS conversation_dispatches_resume_idx
            ON conversation_dispatches(conversation_id, membership_id, state, updated_at DESC);
          CREATE TABLE IF NOT EXISTS subagent_dispatch_claims (
            id TEXT PRIMARY KEY,
            conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
            caller_membership_id TEXT NOT NULL REFERENCES memberships(id),
            target_membership_id TEXT NOT NULL REFERENCES memberships(id),
            parent_dispatch_id TEXT REFERENCES subagent_dispatch_claims(id),
            depth INTEGER NOT NULL CHECK(depth BETWEEN 1 AND 8),
            state TEXT NOT NULL CHECK(state IN (
              'claimed','running','cancel-requested','reconciliation-required',
              'completed','failed','cancelled'
            )),
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            watchdog_deadline_unix_ms INTEGER
          );
          CREATE UNIQUE INDEX IF NOT EXISTS subagent_dispatch_claims_active_edge
            ON subagent_dispatch_claims(conversation_id, caller_membership_id, target_membership_id)
            WHERE state IN ('claimed','running','cancel-requested','reconciliation-required');
          CREATE INDEX IF NOT EXISTS subagent_dispatch_claims_parent_idx
            ON subagent_dispatch_claims(parent_dispatch_id);
          CREATE INDEX IF NOT EXISTS subagent_dispatch_claims_target_idx
            ON subagent_dispatch_claims(conversation_id, target_membership_id, updated_at DESC);
          CREATE TABLE IF NOT EXISTS subagent_mcp_inbound (
            id TEXT PRIMARY KEY,
            conversation_id TEXT NOT NULL,
            caller_membership_id TEXT,
            target_membership_id TEXT,
            tool TEXT NOT NULL CHECK(tool IN (
              'lico_subagent_delegate','lico_subagent_continue','lico_subagent_cancel'
            )),
            outcome TEXT NOT NULL,
            created_at INTEGER NOT NULL
          );
          CREATE INDEX IF NOT EXISTS subagent_mcp_inbound_edge_idx
            ON subagent_mcp_inbound(
              conversation_id, caller_membership_id, target_membership_id, created_at, id
            );
          CREATE TABLE IF NOT EXISTS subagent_dispatch_deliveries (
            claim_id TEXT NOT NULL REFERENCES subagent_dispatch_claims(id) ON DELETE CASCADE,
            kind TEXT NOT NULL CHECK(kind IN ('observation','terminal')),
            conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
            recipient_membership_id TEXT NOT NULL REFERENCES memberships(id),
            state TEXT NOT NULL CHECK(state IN ('pending','delivering','delivered','failed')),
            terminal_state TEXT,
            payload TEXT,
            attempt_count INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            delivered_at INTEGER,
            admitted_turn_id TEXT,
            PRIMARY KEY (claim_id, kind)
          );
          CREATE INDEX IF NOT EXISTS subagent_dispatch_deliveries_pending_idx
            ON subagent_dispatch_deliveries(state, conversation_id, recipient_membership_id, updated_at ASC);
         CREATE TABLE IF NOT EXISTS migration_provenance (
           source_kind TEXT NOT NULL, source_identity TEXT NOT NULL,
           conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
           PRIMARY KEY(source_kind, source_identity)
         );
         CREATE TABLE IF NOT EXISTS archived_native_sessions (
           agent_id TEXT NOT NULL, native_session_id TEXT NOT NULL,
           conversation_id TEXT NOT NULL, archived_at INTEGER NOT NULL,
           PRIMARY KEY(agent_id, native_session_id)
         );";

pub(super) fn configure_connection(connection: &Connection) -> StoreResult<()> {
    // WAL + NORMAL skips fsync of the main file during checkpoint. A SIGKILL
    // in that window can leave a torn database and an empty WAL, which SQLite
    // reports as malformed. FULL fsyncs the main file before the WAL is reset.
    // On macOS, fullfsync is required because ordinary fsync does not flush
    // the drive cache.
    connection.execute_batch(
        "PRAGMA foreign_keys=ON;
         PRAGMA journal_mode=WAL;
         PRAGMA synchronous=FULL;
         PRAGMA fullfsync=ON;
         PRAGMA checkpoint_fullfsync=ON;
         PRAGMA busy_timeout=8000;
         PRAGMA trusted_schema=OFF;",
    )?;
    Ok(())
}

pub(super) fn validate_current_schema(connection: &mut Connection) -> StoreResult<()> {
    let schema_version = preflight_schema(connection)?;
    if schema_version
        .as_deref()
        .is_some_and(|version| version != CURRENT_SCHEMA_VERSION)
    {
        return Err(anyhow!("conversation_schema_migration_required"));
    }
    configure_connection(connection)?;
    if recorded_schema_version(connection)?.is_none() {
        // Empty stores initialize normally; complete current structures may
        // reconstruct only their missing marker without replacing any rows.
        create_current_schema(connection)?;
    }
    ensure_search_index(connection)?;
    // Row-free additive structures are ensured in place: a store already at the
    // current version gains them without a migration and without touching a row.
    connection.execute_batch(admission_keys::TABLE)?;
    Ok(())
}

fn recorded_schema_version(connection: &Connection) -> StoreResult<Option<String>> {
    let has_metadata: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type='table' AND name='schema_meta')",
        [],
        |row| row.get(0),
    )?;
    if !has_metadata {
        return Ok(None);
    }
    Ok(connection
        .query_row(
            "SELECT value FROM schema_meta WHERE key='version'",
            [],
            |row| row.get(0),
        )
        .optional()?)
}

/// Classify the current owner's data without modifying retained unrelated data.
/// Missing labels are reconstructible only when the complete current layout
/// validates. Incomplete owned structures and unknown recorded semantics refuse.
pub(super) fn preflight_schema(connection: &Connection) -> StoreResult<Option<String>> {
    let prior_schema_version = recorded_schema_version(connection)?;
    if prior_schema_version.is_none() {
        let mut reference = Connection::open_in_memory()?;
        create_current_schema(&mut reference)?;
        crate::continuity::migrate::add_schema_contract(&reference, false)?;
        let owned = licoup_foundation::core::sqlite_contract::tables(&reference)?;
        let actual = licoup_foundation::core::sqlite_contract::tables(connection)?;
        if actual
            .iter()
            .any(|table| table != "schema_meta" && owned.contains(table))
        {
            validate_owner_contract(connection)?;
            return Ok(Some(CURRENT_SCHEMA_VERSION.to_owned()));
        }
        if actual.contains(&"schema_meta".to_owned()) {
            licoup_foundation::core::sqlite_contract::validate_table(
                connection,
                &reference,
                "schema_meta",
                None,
            )?;
        }
        return Ok(None);
    }
    if let Some(version) = prior_schema_version.as_deref()
        && !matches!(
            version,
            CURRENT_SCHEMA_VERSION
                | "12"
                | "18"
                | "1"
                | "2"
                | "3"
                | "4"
                | "5"
                | "6"
                | "7"
                | "8"
                | "9"
                | "10"
                | "11"
        )
    {
        return Err(anyhow!("conversation_schema_unsupported_version"));
    }
    if prior_schema_version.as_deref() == Some(CURRENT_SCHEMA_VERSION) {
        validate_owner_contract(connection)?;
    }
    Ok(prior_schema_version)
}

/// Read-only preflight for the migration admission.
///
/// Classifies the store exactly as the owner does and validates the complete
/// owner-required contract before the caller advances its high-water, frontier
/// or domain markers. Malformed metadata and development snapshots (13..17)
/// are refused outright. A current store must already carry the contract its
/// readers and writers use; an older published schema is copied schema-only
/// into memory, upgraded by the owner's real path, and the upgraded result
/// must satisfy that same complete contract. Nothing here writes to the store.
pub fn validate_migration_source(connection: &Connection) -> StoreResult<Option<String>> {
    let version = preflight_schema(connection)?;
    match version.as_deref() {
        Some(CURRENT_SCHEMA_VERSION) => {}
        Some(prior) => {
            // A stamp on a file that never held a store is refused before the
            // owner's upgrade path is even tried.
            for (table, required) in MIGRATION_BASELINE_TABLES {
                let columns = table_columns(connection, table)?;
                if required.iter().any(|column| !columns.contains(*column)) {
                    return Err(anyhow!("conversation_schema_incomplete"));
                }
            }
            let upgraded = schema_only_upgrade_copy(connection, prior)?;
            validate_owner_contract(&upgraded)?;
        }
        None => {}
    }
    Ok(version)
}

/// The identity columns every published Conversation generation carries.
const MIGRATION_BASELINE_TABLES: &[(&str, &[&str])] = &[
    ("schema_meta", &["key", "value"]),
    ("principals", &["id"]),
    ("conversations", &["id", "title"]),
    ("memberships", &["id"]),
    ("events", &["id"]),
];

/// Rebuild the source's physical schema (no rows) in memory, stamp its
/// recorded version, and run the owner's real upgrade path over the copy.
fn schema_only_upgrade_copy(source: &Connection, prior: &str) -> StoreResult<Connection> {
    let mut virtual_tables = source.prepare(
        "SELECT name FROM sqlite_schema WHERE type='table' AND sql LIKE 'CREATE VIRTUAL TABLE%'",
    )?;
    let shadow_prefixes = virtual_tables
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(|name| format!("{name}_"))
        .collect::<Vec<_>>();
    drop(virtual_tables);

    let mut statement = source.prepare(
        "SELECT name, sql FROM sqlite_schema
          WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%'
          ORDER BY CASE type
                     WHEN 'table' THEN 0
                     WHEN 'index' THEN 1
                     WHEN 'trigger' THEN 2
                     ELSE 3
                   END",
    )?;
    let objects = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);

    let mut memory = Connection::open_in_memory()?;
    for (name, sql) in objects {
        if shadow_prefixes
            .iter()
            .any(|prefix| name.starts_with(prefix.as_str()))
        {
            continue;
        }
        memory.execute_batch(&sql)?;
    }
    memory.execute(
        "INSERT INTO schema_meta(key, value) VALUES ('version', ?1)",
        [prior],
    )?;
    // The continuity generation marker is schema metadata, not user content:
    // the owner-contract check reads the version row to decide which extension
    // layout the replayed tables answer to. A schema-only copy still has to
    // carry that marker or the extension check cannot classify the store.
    if table_columns(&memory, "continuity_schema")?.contains("key") {
        let mut keys = source.prepare("SELECT key, value FROM continuity_schema")?;
        let entries = keys
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(keys);
        for (key, value) in entries {
            memory.execute(
                "INSERT INTO continuity_schema(key, value) VALUES (?1, ?2)",
                params![key, value],
            )?;
        }
    }
    initialize_schema_unchecked(&mut memory)?;
    Ok(memory)
}

/// The complete contract the current owner requires: every table, column,
/// key, foreign key and uniqueness constraint the owner itself creates. It is
/// derived from the owner's canonical schema rather than a hand-kept list, so
/// every business field a reader or writer uses is covered by construction.
fn validate_owner_contract(connection: &Connection) -> StoreResult<()> {
    let mut reference = Connection::open_in_memory()?;
    create_current_schema(&mut reference)?;
    let base_tables = licoup_foundation::core::sqlite_contract::tables(&reference)?;
    let actual_tables = licoup_foundation::core::sqlite_contract::tables(connection)?;
    let designation_epoch =
        table_columns(connection, "conversations")?.contains("designation_epoch");
    crate::continuity::migrate::add_schema_contract(&reference, designation_epoch)?;
    let owned_tables = licoup_foundation::core::sqlite_contract::tables(&reference)?;
    let extension_tables: Vec<_> = owned_tables
        .iter()
        .filter(|name| !base_tables.contains(name))
        .collect();
    let extension_present = designation_epoch
        || extension_tables
            .iter()
            .any(|name| actual_tables.contains(name));
    let continuity_version = if extension_present {
        let version: String = connection
            .query_row(
                "SELECT value FROM continuity_schema WHERE key='version'",
                [],
                |row| row.get(0),
            )
            .map_err(|_| anyhow!("conversation_schema_incomplete"))?;
        let version = crate::continuity::migrate::parse_continuity_schema_version(&version)?;
        if version == crate::continuity::migrate::CURRENT_CONTINUITY_SCHEMA_VERSION
            && !designation_epoch
        {
            return Err(anyhow!("conversation_schema_incomplete"));
        }
        Some(version)
    } else {
        None
    };
    for table in &base_tables {
        if table == "schema_meta" && !actual_tables.contains(table) {
            continue;
        }
        licoup_foundation::core::sqlite_contract::validate_table(
            connection, &reference, table, None,
        )
        .map_err(|_| anyhow!("conversation_schema_incomplete"))?;
    }
    if let Some(version) = continuity_version {
        for table in extension_tables {
            // The existing owner creates new tables while upgrading its older
            // internal states. Current states must already have the complete
            // owner contract; a marker cannot manufacture missing structures.
            if version != crate::continuity::migrate::CURRENT_CONTINUITY_SCHEMA_VERSION
                && !actual_tables.contains(table)
            {
                continue;
            }
            licoup_foundation::core::sqlite_contract::validate_table(
                connection, &reference, table, None,
            )
            .map_err(|_| anyhow!("conversation_schema_incomplete"))?;
        }
    }
    Ok(())
}

fn table_columns(connection: &Connection, table: &str) -> StoreResult<BTreeSet<String>> {
    let pragma = format!("PRAGMA table_info({table})");
    Ok(connection
        .prepare(&pragma)?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?)
}

pub(super) fn initialize_schema(connection: &mut Connection) -> StoreResult<()> {
    // Validate the same full structural contract on direct owner opens as on
    // native admission, before journal setup or any durable schema writes.
    validate_migration_source(connection)?;
    initialize_schema_unchecked(connection)
}

fn initialize_schema_unchecked(connection: &mut Connection) -> StoreResult<()> {
    let prior_schema_version = preflight_schema(connection)?;
    configure_connection(connection)?;
    match prior_schema_version.as_deref() {
        None => return create_current_schema(connection),
        Some(CURRENT_SCHEMA_VERSION) => {
            normalize_existing_groups(connection)?;
            return validate_current_schema(connection);
        }
        Some("12" | "18") => return upgrade_published_schema(connection),
        Some("1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "10" | "11") => {}
        Some(_) => return Err(anyhow!("conversation_schema_unsupported_version")),
    }
    connection.execute_batch(CONVERSATION_SCHEMA_TABLES)?;
    match prior_schema_version.as_deref() {
        Some("1") => {
            connection.execute_batch(
                "DELETE FROM event_search WHERE event_id IN (
               SELECT id FROM events WHERE kind IN (
                 'role-changed','flywheel-changed','run-started','run-progress',
                 'run-completed','run-failed','run-cancelled'
               )
             );
             DELETE FROM events WHERE kind IN (
               'role-changed','flywheel-changed','run-started','run-progress',
               'run-completed','run-failed','run-cancelled'
             );
             DROP TABLE IF EXISTS idempotency;
             DROP TABLE IF EXISTS run_candidate_snapshots;
             DROP TABLE IF EXISTS run_stage_snapshots;
             DROP TABLE IF EXISTS turns;
             DROP TABLE IF EXISTS runs;
             DROP TABLE IF EXISTS round_robin_cursors;
             DROP TABLE IF EXISTS flywheel_stages;
             DROP TABLE IF EXISTS flywheels;
             DROP TABLE IF EXISTS role_candidates;
             DROP TABLE IF EXISTS conversation_roles;
             INSERT INTO schema_meta(key, value) VALUES ('version', '2')
               ON CONFLICT(key) DO UPDATE SET value='2';",
            )?;
            migrate_reserved_group_v3(connection)?;
            migrate_reserved_group_v4(connection)?;
        }
        Some("2") => {
            migrate_reserved_group_v3(connection)?;
            migrate_reserved_group_v4(connection)?;
        }
        Some("3") => {
            migrate_reserved_group_v4(connection)?;
        }
        _ => {}
    }
    let current_schema_version: String = connection.query_row(
        "SELECT value FROM schema_meta WHERE key='version'",
        [],
        |row| row.get(0),
    )?;
    if current_schema_version == "4" {
        migrate_runtime_replay_v5(connection)?;
    }
    let current_schema_version: String = connection.query_row(
        "SELECT value FROM schema_meta WHERE key='version'",
        [],
        |row| row.get(0),
    )?;
    if current_schema_version == "5" {
        migrate_strategy_selection_v6(connection)?;
    }
    let current_schema_version: String = connection.query_row(
        "SELECT value FROM schema_meta WHERE key='version'",
        [],
        |row| row.get(0),
    )?;
    if current_schema_version == "6" {
        migrate_assistant_profile_v7(connection)?;
    }
    let current_schema_version: String = connection.query_row(
        "SELECT value FROM schema_meta WHERE key='version'",
        [],
        |row| row.get(0),
    )?;
    if current_schema_version == "7" {
        migrate_profile_intent_v8(connection)?;
    }
    let current_schema_version: String = connection.query_row(
        "SELECT value FROM schema_meta WHERE key='version'",
        [],
        |row| row.get(0),
    )?;
    if current_schema_version == "8" {
        migrate_profile_reasoning_effort_v9(connection)?;
    }
    let current_schema_version: String = connection.query_row(
        "SELECT value FROM schema_meta WHERE key='version'",
        [],
        |row| row.get(0),
    )?;
    if current_schema_version == "9" {
        migrate_subagent_dispatch_claims_v10(connection)?;
    }
    let current_schema_version: String = connection.query_row(
        "SELECT value FROM schema_meta WHERE key='version'",
        [],
        |row| row.get(0),
    )?;
    if current_schema_version == "10" {
        migrate_subagent_mcp_inbound_v11(connection)?;
    }
    let current_schema_version: String = connection.query_row(
        "SELECT value FROM schema_meta WHERE key='version'",
        [],
        |row| row.get(0),
    )?;
    if current_schema_version == "11" {
        migrate_membership_convergence_v12(connection)?;
    }
    ensure_column(
        connection,
        "conversations",
        "assistant_membership_id",
        "TEXT REFERENCES memberships(id)",
    )?;
    ensure_column(connection, "runtime_bindings", "runtime_session_id", "TEXT")?;
    ensure_column(
        connection,
        "conversations",
        "pinned",
        "INTEGER NOT NULL DEFAULT 0 CHECK(pinned IN (0,1))",
    )?;
    ensure_column(
        connection,
        "conversations",
        "is_group",
        "INTEGER NOT NULL DEFAULT 0 CHECK(is_group IN (0,1))",
    )?;
    ensure_column(
        connection,
        "runtime_bindings",
        "runtime_conversation_path",
        "TEXT",
    )?;
    ensure_column(connection, "runtime_bindings", "working_directory", "TEXT")?;
    ensure_column(
        connection,
        "membership_profiles",
        "preferred_instructions",
        "TEXT",
    )?;
    ensure_column(
        connection,
        "membership_profiles",
        "preferred_context_window",
        "INTEGER",
    )?;
    ensure_column(
        connection,
        "subagent_dispatch_claims",
        "watchdog_deadline_unix_ms",
        "INTEGER",
    )?;
    upgrade_published_schema(connection)
}

/// New stores start at the complete current layout without replaying migrations.
fn create_current_schema(connection: &mut Connection) -> StoreResult<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )?;
    ensure_current_layout(&transaction)?;
    write_current_version(&transaction)?;
    transaction.commit()?;
    Ok(())
}

/// All changes since the published schema are one atomic transition. Development
/// snapshots are not additional supported formats or migration checkpoints.
fn upgrade_published_schema(connection: &mut Connection) -> StoreResult<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure_current_layout(&transaction)?;
    normalize_existing_groups(&transaction)?;
    update_assistant_guide_references(&transaction)?;
    native_sessions::backfill_native_sessions(&transaction)?;
    ensure_search_index(&transaction)?;
    write_current_version(&transaction)?;
    transaction.commit()?;
    Ok(())
}

fn ensure_current_layout(connection: &Connection) -> StoreResult<()> {
    connection.execute_batch(CONVERSATION_SCHEMA_TABLES)?;
    connection.execute_batch(native_sessions::TABLE)?;
    connection.execute_batch(admission_keys::TABLE)?;
    ensure_column(connection, "event_parts", "execution_kind", "TEXT")?;
    ensure_column(
        connection,
        "conversation_dispatches",
        "request_payload",
        "TEXT",
    )?;
    ensure_column(
        connection,
        "conversation_dispatches",
        "terminal_payload",
        "TEXT",
    )?;
    ensure_column(
        connection,
        "conversation_dispatches",
        "native_provenance",
        "TEXT",
    )?;
    ensure_column(
        connection,
        "membership_profiles",
        "preferred_permission_mode",
        "TEXT",
    )?;
    ensure_column(
        connection,
        "membership_profiles",
        "preferred_instructions",
        "TEXT",
    )?;
    ensure_column(
        connection,
        "membership_profiles",
        "preferred_context_window",
        "INTEGER",
    )?;
    connection.execute_batch("CREATE INDEX IF NOT EXISTS conversation_dispatches_native_provenance_idx
        ON conversation_dispatches(json_extract(native_provenance,'$.agentId'),json_extract(native_provenance,'$.nativeSessionId'))
        WHERE native_provenance IS NOT NULL;
        CREATE INDEX IF NOT EXISTS conversations_pinned_updated_idx
          ON conversations(pinned DESC, updated_at DESC, id DESC);
        CREATE INDEX IF NOT EXISTS event_parts_runtime_replay_idx
          ON event_parts(event_id, runtime_cursor, ordinal)
          WHERE runtime_cursor IS NOT NULL;")?;
    ensure_converged_membership_index(connection)?;
    Ok(())
}

fn normalize_existing_groups(connection: &Connection) -> StoreResult<()> {
    connection.execute(
        "UPDATE conversations SET is_group=1
         WHERE is_group=0 AND (
           pinned=1 OR id=?1 OR
           EXISTS (
             SELECT 1 FROM migration_provenance p
             WHERE p.conversation_id=conversations.id AND p.source_kind='group'
           ) OR
           (SELECT COUNT(*) FROM memberships m
            WHERE m.conversation_id=conversations.id AND m.status='active') > 2
         )",
        params![DEFAULT_LOCAL_AGENT_GROUP_ID],
    )?;
    Ok(())
}

fn write_current_version(connection: &Connection) -> StoreResult<()> {
    connection.execute(
        "INSERT INTO schema_meta(key, value) VALUES ('version', ?1)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![CURRENT_SCHEMA_VERSION],
    )?;
    Ok(())
}

/// `event_search` is a derived FTS index. Rebuild it from `event_parts` only
/// when the index is corrupt or empty while searchable parts still exist.
fn ensure_search_index(connection: &Connection) -> StoreResult<()> {
    if search_index_is_corrupt(connection) || search_index_is_empty_with_source(connection)? {
        rebuild_search_index(connection)?;
    }
    Ok(())
}

fn search_index_is_corrupt(connection: &Connection) -> bool {
    connection
        .execute(
            "INSERT INTO event_search(event_search) VALUES('integrity-check')",
            [],
        )
        .is_err()
}

fn search_index_is_empty_with_source(connection: &Connection) -> StoreResult<bool> {
    let indexed: i64 =
        connection.query_row("SELECT COUNT(*) FROM event_search", [], |row| row.get(0))?;
    if indexed > 0 {
        return Ok(false);
    }
    let source: i64 = connection.query_row(
        "SELECT COUNT(*) FROM event_parts WHERE kind IN ('text', 'reasoning')",
        [],
        |row| row.get(0),
    )?;
    Ok(source > 0)
}

fn rebuild_search_index(connection: &Connection) -> StoreResult<()> {
    connection.execute_batch(
        "DROP TABLE IF EXISTS event_search;
         CREATE VIRTUAL TABLE event_search USING fts5(
           event_id UNINDEXED, conversation_id UNINDEXED, content
         );
         INSERT INTO event_search(event_id, conversation_id, content)
         SELECT p.event_id, e.conversation_id, p.content
           FROM event_parts p
           JOIN events e ON e.id = p.event_id
          WHERE p.kind IN ('text', 'reasoning');",
    )?;
    Ok(())
}

/// One-time schema transition to version 3: normalize the pre-cutover
/// reserved default local group inside one immediate transaction that also
/// records the new schema version. A failure rolls back both the cleanup and
/// the version write, leaving the store at version 2.
fn migrate_reserved_group_v3(connection: &mut Connection) -> StoreResult<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    normalize_reserved_group(&transaction)?;
    transaction.execute(
        "INSERT INTO schema_meta(key, value) VALUES ('version', '3')
         ON CONFLICT(key) DO UPDATE SET value='3'",
        [],
    )?;
    transaction.commit()?;
    Ok(())
}

/// One-time schema transition to version 4: rename only the reserved local
/// group's retired built-in title. Custom group names and recency timestamps
/// are preserved.
fn migrate_reserved_group_v4(connection: &mut Connection) -> StoreResult<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    rename_reserved_group(&transaction)?;
    transaction.execute(
        "INSERT INTO schema_meta(key, value) VALUES ('version', '4')
         ON CONFLICT(key) DO UPDATE SET value='4'",
        [],
    )?;
    transaction.commit()?;
    Ok(())
}

/// One-time schema transition to version 5: add the private per-part cursor
/// used to reconstruct active-turn transport frames from their owning
/// canonical Message Event. Existing Event content and ordering are untouched.
fn migrate_runtime_replay_v5(connection: &mut Connection) -> StoreResult<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let has_runtime_cursor = {
        let mut statement = transaction.prepare("PRAGMA table_info(event_parts)")?;
        statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .iter()
            .any(|column| column == "runtime_cursor")
    };
    if !has_runtime_cursor {
        transaction.execute_batch("ALTER TABLE event_parts ADD COLUMN runtime_cursor INTEGER;")?;
    }
    transaction.execute_batch(
        "CREATE INDEX IF NOT EXISTS event_parts_runtime_replay_idx
         ON event_parts(event_id, runtime_cursor, ordinal)
         WHERE runtime_cursor IS NOT NULL;",
    )?;
    transaction.execute(
        "INSERT INTO schema_meta(key, value) VALUES ('version', '5')
         ON CONFLICT(key) DO UPDATE SET value='5'",
        [],
    )?;
    transaction.commit()?;
    Ok(())
}

/// One-time schema transition to version 6: persist the strategy explicitly
/// selected for each group Conversation. A nullable column preserves the
/// existing no-strategy state for all prior conversations.
fn migrate_strategy_selection_v6(connection: &mut Connection) -> StoreResult<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let has_strategy_revision = {
        let mut statement = transaction.prepare("PRAGMA table_info(conversations)")?;
        statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .iter()
            .any(|column| column == "strategy_revision")
    };
    if !has_strategy_revision {
        transaction
            .execute_batch("ALTER TABLE conversations ADD COLUMN strategy_revision TEXT;")?;
    }
    transaction.execute(
        "INSERT INTO schema_meta(key, value) VALUES ('version', '6')
         ON CONFLICT(key) DO UPDATE SET value='6'",
        [],
    )?;
    transaction.commit()?;
    Ok(())
}

/// One-time schema transition to version 7: persist one explicit long-lived
/// Assistant per Conversation and bounded endpoint-local Profile intent for
/// every active Agent Membership. The migration never assigns an Assistant
/// to an existing ambiguous group; it only backfills default Profile intent
/// rows so existing active Agent Memberships remain revisioned and visible.
fn migrate_assistant_profile_v7(connection: &mut Connection) -> StoreResult<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let has_assistant_column = {
        let mut statement = transaction.prepare("PRAGMA table_info(conversations)")?;
        statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .iter()
            .any(|column| column == "assistant_membership_id")
    };
    if !has_assistant_column {
        transaction.execute_batch(
            "ALTER TABLE conversations ADD COLUMN assistant_membership_id TEXT REFERENCES memberships(id);",
        )?;
    }
    transaction.execute_batch(
        "INSERT OR IGNORE INTO membership_profiles(
           membership_id, revision, responsibility, required_capabilities,
           preferred_capabilities, skill_references, preferred_model,
           preferred_environment, updated_at
         )
         SELECT m.id, 0, 'member', '[]', '[]', '[]', NULL, NULL, m.joined_at
         FROM memberships m
         WHERE m.status='active'
           AND EXISTS (
             SELECT 1 FROM principals p
             WHERE p.id=m.principal_id AND p.kind='agent'
           );",
    )?;
    transaction.execute(
        "INSERT INTO schema_meta(key, value) VALUES ('version', '7')
         ON CONFLICT(key) DO UPDATE SET value='7'",
        [],
    )?;
    transaction.commit()?;
    Ok(())
}

/// Version 8 replaces the draft Profile row with intent-only fields. Any
/// draft caller-asserted capability, Skill, or Authority values are discarded
/// instead of being translated into trusted facts. The current Assistant is
/// reconstructed solely from the Conversation-owned designation.
fn migrate_profile_intent_v8(connection: &mut Connection) -> StoreResult<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let columns = {
        let mut statement = transaction.prepare("PRAGMA table_info(membership_profiles)")?;
        statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<BTreeSet<_>>>()?
    };
    if !columns.contains("required_capabilities") || columns.contains("authority") {
        transaction.execute_batch(
            "ALTER TABLE membership_profiles RENAME TO membership_profiles_v7_draft;
             CREATE TABLE membership_profiles (
               membership_id TEXT PRIMARY KEY REFERENCES memberships(id) ON DELETE CASCADE,
               revision INTEGER NOT NULL,
               responsibility TEXT NOT NULL DEFAULT 'member' CHECK(responsibility IN ('assistant','member')),
               required_capabilities TEXT NOT NULL DEFAULT '[]',
               preferred_capabilities TEXT NOT NULL DEFAULT '[]',
               skill_references TEXT NOT NULL DEFAULT '[]',
               preferred_model TEXT, preferred_environment TEXT,
               updated_at INTEGER NOT NULL
             );",
        )?;
        transaction.execute(
            "INSERT INTO membership_profiles(
               membership_id, revision, responsibility, required_capabilities,
               preferred_capabilities, skill_references, preferred_model,
               preferred_environment, updated_at
             )
             SELECT m.id,
                    COALESCE(d.revision, 0) + 1,
                    CASE WHEN c.assistant_membership_id=m.id THEN 'assistant' ELSE 'member' END,
                    '[]', '[]',
                    CASE WHEN c.assistant_membership_id=m.id THEN ?1 ELSE '[]' END,
                    NULL, NULL, COALESCE(d.updated_at, m.joined_at)
             FROM memberships m
             JOIN principals p ON p.id=m.principal_id
             JOIN conversations c ON c.id=m.conversation_id
             LEFT JOIN membership_profiles_v7_draft d ON d.membership_id=m.id
             WHERE m.status='active' AND p.kind='agent'",
            params![serde_json::to_string(&vec![LICOUP_GUIDE_SKILL_ID])?],
        )?;
        transaction.execute_batch(
            "DROP TABLE membership_profiles_v7_draft;
             CREATE INDEX membership_profiles_membership_idx
               ON membership_profiles(membership_id, revision);",
        )?;
    }
    transaction.execute(
        "INSERT INTO schema_meta(key, value) VALUES ('version', '8')
         ON CONFLICT(key) DO UPDATE SET value='8'",
        [],
    )?;
    transaction.commit()?;
    Ok(())
}

/// Version 9 makes reasoning effort part of the same revisioned Profile intent
/// as the preferred model. It is nullable so existing Profiles preserve their
/// native runtime default without inventing a value.
fn migrate_profile_reasoning_effort_v9(connection: &mut Connection) -> StoreResult<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let columns = {
        let mut statement = transaction.prepare("PRAGMA table_info(membership_profiles)")?;
        statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<BTreeSet<_>>>()?
    };
    if !columns.contains("preferred_reasoning_effort") {
        transaction.execute_batch(
            "ALTER TABLE membership_profiles ADD COLUMN preferred_reasoning_effort TEXT;",
        )?;
    }
    transaction.execute(
        "INSERT INTO schema_meta(key, value) VALUES ('version', '9')
         ON CONFLICT(key) DO UPDATE SET value='9'",
        [],
    )?;
    transaction.commit()?;
    Ok(())
}

/// Version 10 adds private, durable Subagent lineage and active-edge claims.
/// `CONVERSATION_SCHEMA_TABLES` creates the table before migrations run, so
/// this transaction advances the version only after that DDL succeeded.
fn migrate_subagent_dispatch_claims_v10(connection: &mut Connection) -> StoreResult<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute(
        "INSERT INTO schema_meta(key, value) VALUES ('version', '10')
         ON CONFLICT(key) DO UPDATE SET value='10'",
        [],
    )?;
    transaction.commit()?;
    Ok(())
}

/// Version 11 records inbound Subagent MCP `tools/call` rows. The table is
/// created by `CONVERSATION_SCHEMA_TABLES`; this step only advances the version.
fn migrate_subagent_mcp_inbound_v11(connection: &mut Connection) -> StoreResult<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute(
        "INSERT INTO schema_meta(key, value) VALUES ('version', '11')
         ON CONFLICT(key) DO UPDATE SET value='11'",
        [],
    )?;
    transaction.commit()?;
    Ok(())
}

/// Version 12 keeps one Membership row per (conversation, principal). Join and
/// leave flip that row's status; leftover left rows from earlier churn are
/// retargeted onto the surviving id and deleted.
fn migrate_membership_convergence_v12(connection: &mut Connection) -> StoreResult<()> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    converge_duplicate_memberships(&transaction, None)?;
    ensure_converged_membership_index(&transaction)?;
    transaction.execute(
        "INSERT INTO schema_meta(key, value) VALUES ('version', '12')
         ON CONFLICT(key) DO UPDATE SET value='12'",
        [],
    )?;
    transaction.commit()?;
    Ok(())
}

/// Move the retired bundled Assistant Skill reference to the
/// single product-owned LicoUp guide. Only Assistant Profiles are rewritten;
/// every unrelated user-selected reference keeps its original order.
fn update_assistant_guide_references(connection: &Connection) -> StoreResult<()> {
    const RETIRED_SKILL_ID: &str = "assistant-workflow-authoring";

    let profiles = {
        let mut statement = connection.prepare(
            "SELECT membership_id, skill_references
             FROM membership_profiles
             WHERE responsibility='assistant'",
        )?;
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (membership_id, encoded) in profiles {
        let current = serde_json::from_str::<Vec<String>>(&encoded)?;
        let mut migrated = current
            .iter()
            .filter(|reference| {
                reference.as_str() != RETIRED_SKILL_ID
                    && reference.as_str() != LICOUP_GUIDE_SKILL_ID
            })
            .cloned()
            .collect::<Vec<_>>();
        migrated.push(LICOUP_GUIDE_SKILL_ID.to_owned());
        if migrated != current {
            connection.execute(
                "UPDATE membership_profiles
                 SET revision=revision+1, skill_references=?2
                 WHERE membership_id=?1",
                params![membership_id, serde_json::to_string(&migrated)?],
            )?;
        }
    }
    Ok(())
}

fn ensure_converged_membership_index(connection: &Connection) -> StoreResult<()> {
    connection.execute_batch(
        "DROP INDEX IF EXISTS memberships_active_unique;
         CREATE UNIQUE INDEX IF NOT EXISTS memberships_principal_unique
           ON memberships(conversation_id, principal_id);",
    )?;
    Ok(())
}
