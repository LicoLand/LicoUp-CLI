//! Idempotent continuity schema. Repeated apply does not insert Goals,
//! rewrite authors, or revive deleted summaries.

use super::error::store_to_continuity;
use super::generated::ContinuityFailure;
use crate::store::{ContinuityUnitOfWork, StoreResult};
use anyhow::anyhow;

pub const CONTINUITY_SCHEMA_VERSION: &str = "7";
pub(crate) const CURRENT_CONTINUITY_SCHEMA_VERSION: u32 = 7;

const DESIGNATION_EPOCH_COLUMN: &str =
    "ALTER TABLE conversations ADD COLUMN designation_epoch INTEGER NOT NULL DEFAULT 0";
const DESIGNATION_EPOCH_TRIGGER: &str = "CREATE TRIGGER continuity_bump_designation_epoch
             AFTER UPDATE OF assistant_membership_id ON conversations
             WHEN (OLD.assistant_membership_id IS NOT NEW.assistant_membership_id)
             BEGIN
               UPDATE conversations
               SET designation_epoch = designation_epoch + 1
               WHERE id = NEW.id;
             END";

const STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS continuity_schema (
      key TEXT PRIMARY KEY,
      value TEXT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS continuity_scope (
      conversation_id TEXT PRIMARY KEY,
      revocation_generation INTEGER NOT NULL DEFAULT 0,
      acl_generation INTEGER NOT NULL DEFAULT 0
    )",
    "CREATE TABLE IF NOT EXISTS continuity_matters (
      id TEXT PRIMARY KEY,
      conversation_id TEXT NOT NULL,
      revision INTEGER NOT NULL,
      label TEXT NOT NULL,
      association_refs TEXT NOT NULL,
      created_event TEXT NOT NULL,
      status TEXT NOT NULL,
      deleted_at INTEGER,
      deletion_generation INTEGER NOT NULL DEFAULT 0
    )",
    "CREATE INDEX IF NOT EXISTS continuity_matters_scope_idx
      ON continuity_matters(conversation_id, revision, id)",
    "CREATE TABLE IF NOT EXISTS continuity_matter_associations (
      conversation_id TEXT NOT NULL,
      source_event_id TEXT NOT NULL,
      interpretation_key TEXT NOT NULL,
      matter_id TEXT NOT NULL,
      association_revision INTEGER NOT NULL,
      proposed_by TEXT NOT NULL,
      reason_code TEXT NOT NULL,
      source_ref TEXT NOT NULL,
      supersedes INTEGER,
      PRIMARY KEY (conversation_id, source_event_id, interpretation_key)
    )",
    "CREATE TABLE IF NOT EXISTS continuity_agreements (
      id TEXT PRIMARY KEY,
      conversation_id TEXT NOT NULL,
      scope TEXT NOT NULL,
      statement_ref TEXT NOT NULL,
      origin TEXT NOT NULL,
      effective_revision INTEGER NOT NULL,
      supersedes INTEGER,
      valid_from INTEGER NOT NULL,
      valid_until INTEGER,
      revocation_generation INTEGER NOT NULL,
      superseded_by TEXT,
      deleted_at INTEGER
    )",
    "CREATE INDEX IF NOT EXISTS continuity_agreements_scope_idx
      ON continuity_agreements(conversation_id, revocation_generation, id)",
    "CREATE TABLE IF NOT EXISTS continuity_goals (
      goal_id TEXT PRIMARY KEY,
      conversation_id TEXT NOT NULL,
      matter_id TEXT NOT NULL,
      contract TEXT NOT NULL,
      progress TEXT NOT NULL,
      lifecycle TEXT NOT NULL,
      control TEXT NOT NULL,
      revision INTEGER NOT NULL,
      next_due INTEGER,
      deleted_at INTEGER
    )",
    "CREATE INDEX IF NOT EXISTS continuity_goals_state_idx
      ON continuity_goals(conversation_id, lifecycle, next_due, goal_id)",
    "CREATE TABLE IF NOT EXISTS continuity_source_cursors (
      conversation_id TEXT NOT NULL,
      source_event_id TEXT NOT NULL,
      interpretation_key TEXT NOT NULL,
      payload TEXT NOT NULL,
      PRIMARY KEY (conversation_id, source_event_id, interpretation_key)
    )",
    "CREATE INDEX IF NOT EXISTS continuity_source_cursors_key_idx
      ON continuity_source_cursors(interpretation_key)",
    "CREATE INDEX IF NOT EXISTS continuity_source_cursors_outstanding_work_idx
      ON continuity_source_cursors(conversation_id, source_event_id, interpretation_key)
      WHERE interpretation_key LIKE 'work:%:child-work-pending'",
    "CREATE INDEX IF NOT EXISTS continuity_source_cursors_outstanding_settlement_idx
      ON continuity_source_cursors(conversation_id, source_event_id, interpretation_key)
      WHERE interpretation_key LIKE 'settlement:%:settlement-pending'",
    "CREATE TABLE IF NOT EXISTS continuity_outbox (
      logical_wake_id TEXT PRIMARY KEY,
      conversation_id TEXT NOT NULL,
      goal_id TEXT NOT NULL,
      payload TEXT NOT NULL,
      created_at INTEGER NOT NULL,
      consumed_at INTEGER,
      settlement TEXT NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS continuity_outbox_goal_idx
      ON continuity_outbox(goal_id, settlement, created_at)",
    "CREATE INDEX IF NOT EXISTS continuity_outbox_pending_idx
      ON continuity_outbox(conversation_id, consumed_at, logical_wake_id)",
    "CREATE TABLE IF NOT EXISTS continuity_idempotency (
      request_id TEXT PRIMARY KEY,
      conversation_id TEXT NOT NULL,
      payload TEXT NOT NULL,
      receipt TEXT NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS continuity_task_relations (
      goal_id TEXT PRIMARY KEY,
      parent_conversation_id TEXT NOT NULL,
      child_conversation_id TEXT NOT NULL,
      card_event_id TEXT NOT NULL,
      card_sequence INTEGER NOT NULL,
      card_part_id TEXT,
      listing_kind TEXT NOT NULL,
      follow_through_kind TEXT NOT NULL,
      created_event TEXT NOT NULL,
      completion_transition TEXT,
      revision INTEGER NOT NULL
    )",
    "CREATE UNIQUE INDEX IF NOT EXISTS continuity_task_relations_child_idx
      ON continuity_task_relations(child_conversation_id)",
    "CREATE INDEX IF NOT EXISTS continuity_task_relations_parent_idx
      ON continuity_task_relations(parent_conversation_id, card_sequence, goal_id)",
    "CREATE TABLE IF NOT EXISTS continuity_parent_grants (
      grant_id TEXT PRIMARY KEY,
      source_conversation_id TEXT NOT NULL,
      recipient_conversation_id TEXT NOT NULL,
      recipient_membership_id TEXT NOT NULL,
      source_refs TEXT NOT NULL,
      authorized_scopes TEXT NOT NULL,
      status TEXT NOT NULL,
      request_id TEXT NOT NULL,
      revocation_generation INTEGER NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS continuity_parent_grants_recipient_idx
      ON continuity_parent_grants(
        recipient_conversation_id, recipient_membership_id, revocation_generation, grant_id
      )",
    "CREATE TABLE IF NOT EXISTS continuity_completion_transitions (
      notification_id TEXT PRIMARY KEY,
      goal_id TEXT NOT NULL UNIQUE,
      transition TEXT NOT NULL,
      consumed INTEGER NOT NULL DEFAULT 1,
      created_at INTEGER NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS continuity_completion_pending_idx
      ON continuity_completion_transitions(consumed, created_at, notification_id)",
    "CREATE TABLE IF NOT EXISTS continuity_effects (
      logical_effect_id TEXT PRIMARY KEY,
      conversation_id TEXT NOT NULL,
      goal_id TEXT,
      status TEXT NOT NULL,
      updated_at INTEGER NOT NULL
    )",
    "CREATE TABLE IF NOT EXISTS continuity_derived (
      id TEXT PRIMARY KEY,
      conversation_id TEXT NOT NULL,
      kind TEXT NOT NULL,
      source_opaque_id TEXT NOT NULL,
      body TEXT NOT NULL,
      revocation_generation INTEGER NOT NULL,
      invalidated INTEGER NOT NULL DEFAULT 0,
      deleted_at INTEGER
    )",
    "CREATE INDEX IF NOT EXISTS continuity_derived_src_idx
      ON continuity_derived(conversation_id, revocation_generation, invalidated, id)",
    "CREATE TABLE IF NOT EXISTS continuity_source_revocations (
      conversation_id TEXT NOT NULL,
      opaque_id TEXT NOT NULL,
      revocation_generation INTEGER NOT NULL,
      deleted INTEGER NOT NULL DEFAULT 0,
      PRIMARY KEY (conversation_id, opaque_id)
    )",
    "CREATE TABLE IF NOT EXISTS continuity_work_contexts (
      conversation_id TEXT NOT NULL,
      membership_id TEXT NOT NULL,
      matter_id TEXT NOT NULL,
      generation INTEGER NOT NULL,
      status TEXT NOT NULL,
      last_reconciled INTEGER NOT NULL,
      PRIMARY KEY (conversation_id, membership_id, matter_id, generation)
    )",
    "CREATE TABLE IF NOT EXISTS continuity_qualification_invalidations (
      responsibility_id TEXT NOT NULL,
      identity_key TEXT NOT NULL,
      withdrawn_at INTEGER NOT NULL,
      PRIMARY KEY (responsibility_id, identity_key)
    )",
    "CREATE TABLE IF NOT EXISTS continuity_qualification_evidence (
      responsibility_id TEXT NOT NULL,
      identity_key TEXT NOT NULL,
      payload TEXT NOT NULL,
      evidence_class TEXT NOT NULL,
      ingested_at INTEGER NOT NULL,
      PRIMARY KEY (responsibility_id, identity_key)
    )",
];

pub fn ensure_continuity_schema(unit: &ContinuityUnitOfWork<'_>) -> StoreResult<bool> {
    let current = match unit.query_row(
        "SELECT value FROM continuity_schema WHERE key='version'",
        [],
        |row| row.get::<_, String>(0),
    ) {
        Ok(value) => Some(value),
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        Err(error) if is_missing_table(&error) => None,
        Err(error) => return Err(error.into()),
    };
    let current_version = current
        .as_deref()
        .map(parse_continuity_schema_version)
        .transpose()?;
    if current_version == Some(CURRENT_CONTINUITY_SCHEMA_VERSION) {
        let mut dirty = ensure_designation_epoch(unit)?;
        dirty |= ensure_adoption_policy_keys(unit)?;
        return Ok(dirty);
    }
    for statement in STATEMENTS {
        unit.execute(statement, [])?;
    }
    if current.is_some() {
        migrate_outstanding_pending_state(unit)?;
    }
    unit.execute(
        "INSERT INTO continuity_schema(key, value) VALUES ('version', ?1)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        [CONTINUITY_SCHEMA_VERSION],
    )?;
    ensure_designation_epoch(unit)?;
    ensure_adoption_policy_keys(unit)?;
    Ok(true)
}

/// Build the persistence owner's contract on an in-memory Conversation schema.
/// This shares the actual owner DDL; callers never run it on the source database.
pub(crate) fn add_schema_contract(
    reference: &rusqlite::Connection,
    designation_epoch: bool,
) -> StoreResult<()> {
    for statement in STATEMENTS {
        reference.execute(statement, [])?;
    }
    if designation_epoch {
        reference.execute(DESIGNATION_EPOCH_COLUMN, [])?;
        reference.execute(DESIGNATION_EPOCH_TRIGGER, [])?;
    }
    Ok(())
}

fn ensure_adoption_policy_keys(unit: &ContinuityUnitOfWork<'_>) -> StoreResult<bool> {
    let mut dirty = false;
    for (key, value) in [("adoption_enabled", "1"), ("adoption_stage", "offline")] {
        let present: i64 = unit.query_row(
            "SELECT EXISTS(SELECT 1 FROM continuity_schema WHERE key=?1)",
            [key],
            |row| row.get(0),
        )?;
        if present == 0 {
            unit.execute(
                "INSERT INTO continuity_schema(key, value) VALUES (?1, ?2)",
                rusqlite::params![key, value],
            )?;
            dirty = true;
        }
    }
    Ok(dirty)
}

pub(crate) fn parse_continuity_schema_version(value: &str) -> StoreResult<u32> {
    let version = value
        .parse::<u32>()
        .map_err(|_| anyhow!("continuity_schema_unsupported_version"))?;
    if version > CURRENT_CONTINUITY_SCHEMA_VERSION {
        return Err(anyhow!("continuity_schema_unsupported_version"));
    }
    Ok(version)
}

fn migrate_outstanding_pending_state(unit: &ContinuityUnitOfWork<'_>) -> StoreResult<()> {
    unit.execute(
        "INSERT OR IGNORE INTO continuity_source_cursors(
           conversation_id, source_event_id, interpretation_key, payload
         )
         SELECT i.conversation_id, i.source_event_id,
                replace(i.interpretation_key, ':child-work-intent', ':child-work-pending'),
                i.payload
         FROM continuity_source_cursors i
         WHERE i.interpretation_key LIKE 'work:%:child-work-intent'
           AND NOT EXISTS (
             SELECT 1 FROM continuity_source_cursors s
             WHERE s.conversation_id = i.conversation_id
               AND s.source_event_id = i.source_event_id
               AND s.interpretation_key = 'work:' || i.source_event_id || ':' ||
                   COALESCE(
                     CAST(json_extract(i.payload, '$.admittedRevision') AS TEXT),
                     CAST(json_extract(i.payload, '$.revision') AS TEXT),
                     '0'
                   )
           )",
        [],
    )?;
    unit.execute(
        "DELETE FROM continuity_source_cursors
         WHERE interpretation_key LIKE 'settlement:%:settlement-pending'
           AND EXISTS (
             SELECT 1 FROM continuity_source_cursors a
             WHERE a.conversation_id = continuity_source_cursors.conversation_id
               AND a.source_event_id = continuity_source_cursors.source_event_id
               AND a.interpretation_key = 'settlement:' ||
                   continuity_source_cursors.source_event_id || ':settlement-applied'
           )",
        [],
    )?;
    unit.execute(
        "INSERT OR IGNORE INTO continuity_source_cursors(
           conversation_id, source_event_id, interpretation_key, payload
         )
         SELECT a.conversation_id, a.source_event_id,
                'work:' || a.source_event_id || ':child-work-live',
                a.payload
         FROM continuity_source_cursors a
         WHERE a.interpretation_key LIKE 'work:%:child-work-accepted'
           AND EXISTS (
             SELECT 1 FROM continuity_source_cursors s
             WHERE s.conversation_id = a.conversation_id
               AND s.source_event_id = a.source_event_id
               AND s.interpretation_key = 'work:' || a.source_event_id || ':' ||
                   COALESCE(
                     CAST(json_extract(a.payload, '$.admittedRevision') AS TEXT),
                     CAST(json_extract(a.payload, '$.revision') AS TEXT),
                     '0'
                   )
           )",
        [],
    )?;
    unit.execute(
        "DROP INDEX IF EXISTS continuity_source_cursors_pending_work_idx",
        [],
    )?;
    unit.execute(
        "DROP INDEX IF EXISTS continuity_source_cursors_pending_settlement_idx",
        [],
    )?;
    Ok(())
}

fn is_missing_table(error: &rusqlite::Error) -> bool {
    match error {
        rusqlite::Error::SqliteFailure(_, Some(message)) => message.contains("no such table"),
        rusqlite::Error::SqliteFailure(code, _) => {
            code.extended_code == rusqlite::ffi::SQLITE_ERROR
                && format!("{error}").contains("no such table")
        }
        other => format!("{other}").contains("no such table"),
    }
}

fn ensure_designation_epoch(unit: &ContinuityUnitOfWork<'_>) -> StoreResult<bool> {
    let present: i64 = unit.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('conversations') WHERE name='designation_epoch'",
        [],
        |row| row.get(0),
    )?;
    let mut dirty = false;
    if present == 0 {
        unit.execute(DESIGNATION_EPOCH_COLUMN, [])?;
        dirty = true;
    }
    let trigger_present: i64 = unit.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM sqlite_master
           WHERE type='trigger' AND name='continuity_bump_designation_epoch'
         )",
        [],
        |row| row.get(0),
    )?;
    if trigger_present == 0 {
        unit.execute(DESIGNATION_EPOCH_TRIGGER, [])?;
        dirty = true;
    }
    Ok(dirty)
}

pub fn migrate_or_fail(unit: &ContinuityUnitOfWork<'_>) -> Result<bool, ContinuityFailure> {
    ensure_continuity_schema(unit).map_err(store_to_continuity)
}
