//! Durable cold recovery for a host terminated during a turn.

use super::{ConversationStore, StoreResult, enum_wire, new_id, now_ms};
use crate::{DispatchState, EventPartKind, TurnState};
use rusqlite::{OptionalExtension, params};

const HOST_INTERRUPTED: &str = "host_lifecycle_interrupted";

/// Privacy-safe result of rebuilding process state from SQLite.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ColdRecoveryReport {
    pub recovered_dispatches: usize,
    pub finalized_events: usize,
    pub interrupted_turns: usize,
}

/// Public host-neutral recovery boundary. Desktop restart and mobile resume
/// invoke this same operation; no process-memory snapshot participates.
pub trait ColdRecoverableConversationStore {
    fn cold_recover(&self) -> StoreResult<ColdRecoveryReport>;
}

impl ColdRecoverableConversationStore for ConversationStore {
    fn cold_recover(&self) -> StoreResult<ColdRecoveryReport> {
        self.with_connection(|connection| {
            let transaction = connection.unchecked_transaction()?;
            let recoverable = {
                let mut statement = transaction.prepare(
                    "SELECT d.id, d.conversation_id, e.id, d.state
                     FROM conversation_dispatches d
                     LEFT JOIN events e ON e.conversation_id=d.conversation_id
                       AND e.correlation_id=d.id AND e.author_membership_id=d.membership_id
                       AND e.kind='message' AND e.finalized=0
                     WHERE d.state IN ('accepted','running','cancel-requested')
                     ORDER BY d.created_at, d.id",
                )?;
                statement
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            let mut report = ColdRecoveryReport::default();
            for (dispatch_id, conversation_id, event_id, dispatch_state) in recoverable {
                let dispatch_state = DispatchState::from_name(&dispatch_state)
                    .ok_or_else(|| anyhow::anyhow!("runtime_dispatch_state_invalid"))?;
                let failed = super::lifecycle::fail_dispatch(dispatch_state)
                    .ok_or_else(|| anyhow::anyhow!("runtime_dispatch_transition_invalid"))?;
                let changed = transaction.execute(
                    "UPDATE conversation_dispatches
                     SET state=?2, error_code=?3, updated_at=?4
                     WHERE id=?1 AND state IN ('accepted','running','cancel-requested')",
                    params![dispatch_id, enum_wire(failed)?, HOST_INTERRUPTED, now_ms(),],
                )?;
                if changed > 0 {
                    report.recovered_dispatches += 1;
                    report.interrupted_turns += transaction.execute(
                        "UPDATE direct_turns SET state=?2
                         WHERE id=?1 AND state IN ('claimed','running')",
                        params![dispatch_id, enum_wire(TurnState::Interrupted)?],
                    )?;
                }
                // A dispatch row repeated by the join (more than one unfinalized
                // message event correlated to it) reports changed == 0 on its
                // later rows; every joined event is still finalized exactly
                // once through its own finalized=0 guard.
                if let Some(event_id) = event_id {
                    report.finalized_events +=
                        finalize_interrupted_message(&transaction, &event_id, &conversation_id)?;
                }
            }
            // A completed/failed dispatch, a missing correlation, or a
            // membership-join miss can leave a Message Event at finalized=0.
            // Those orphans stay "streaming" across relaunch unless this
            // second pass settles them.
            let orphans = {
                let mut statement = transaction.prepare(
                    "SELECT id, conversation_id FROM events
                     WHERE kind='message' AND finalized=0
                     ORDER BY conversation_id, sequence, id",
                )?;
                statement
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            for (event_id, conversation_id) in orphans {
                report.finalized_events +=
                    finalize_interrupted_message(&transaction, &event_id, &conversation_id)?;
            }
            super::dispatches::reconcile_terminal_subagent_claims(&transaction)?;
            transaction.commit()?;
            Ok(report)
        })
    }
}

impl ConversationStore {
    /// Rebuild durable state after an unclean host stop. Idempotent: once all
    /// interrupted rows are terminal, later opens report an empty recovery.
    pub fn cold_recover(&self) -> StoreResult<ColdRecoveryReport> {
        ColdRecoverableConversationStore::cold_recover(self)
    }
}

fn finalize_interrupted_message(
    transaction: &impl super::CountedSqlite,
    event_id: &str,
    conversation_id: &str,
) -> StoreResult<usize> {
    let pending: Option<i64> = transaction
        .query_row(
            "SELECT finalized FROM events WHERE id=?1 AND conversation_id=?2 AND kind='message'",
            params![event_id, conversation_id],
            |row| row.get(0),
        )
        .optional()?;
    if pending != Some(0) {
        return Ok(0);
    }
    let diagnostic = serde_json::json!({"code": HOST_INTERRUPTED}).to_string();
    let already_marked: bool = transaction.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM event_parts
           WHERE event_id=?1 AND kind='diagnostic' AND content=?2
         )",
        params![event_id, diagnostic],
        |row| row.get(0),
    )?;
    if !already_marked {
        let ordinal: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(ordinal), -1)+1 FROM event_parts WHERE event_id=?1",
            params![event_id],
            |row| row.get(0),
        )?;
        transaction.execute(
            "INSERT INTO event_parts(id, event_id, ordinal, kind, content, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                new_id("part"),
                event_id,
                ordinal,
                enum_wire(EventPartKind::Diagnostic)?,
                diagnostic,
                now_ms(),
            ],
        )?;
    }
    let changed = transaction.execute(
        "UPDATE events SET finalized=1 WHERE id=?1 AND finalized=0",
        params![event_id],
    )?;
    if changed > 0 {
        super::bump_revision(transaction, conversation_id, now_ms())?;
    }
    Ok(changed)
}
