//! Durable admission keys for an at-most-once Event append.
//!
//! A caller that receives the same inbound unit more than once needs an answer
//! that outlives the process: whether that unit was already committed, and as
//! which Event. A window held in memory answers only until the next start, so a
//! resend after a restart would be admitted a second time. This table is that
//! answer.
//!
//! The key is written before the Event it authorizes and inside the same
//! transaction, so a committed row can never exist without its admission record
//! and an admission record can never outlive a rolled-back row. The key is the
//! caller's own identity of the unit; this owner stores and returns it
//! unchanged and gives it no meaning beyond equality.

use super::*;

pub(super) const TABLE: &str = "CREATE TABLE IF NOT EXISTS event_admission_keys (
    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    admission_key TEXT NOT NULL,
    event_id TEXT,
    sequence INTEGER,
    created_at INTEGER NOT NULL,
    PRIMARY KEY(conversation_id, admission_key)
);";

/// The result of one at-most-once Event append.
#[derive(Clone, Debug)]
pub enum EventAppendOutcome {
    /// The admission key was unused: the Event is committed under it.
    Appended(ConversationEvent),
    /// The admission key was already consumed: nothing was written. The Event
    /// that consumed the key is named when this owner still knows it, so a
    /// caller can merge into the effect it already produced.
    AlreadyAdmitted {
        event_id: Option<String>,
        sequence: Option<i64>,
    },
}

impl EventAppendOutcome {
    /// The committed Event, when this append was the one that committed it.
    #[must_use]
    pub fn appended(&self) -> Option<&ConversationEvent> {
        match self {
            Self::Appended(event) => Some(event),
            Self::AlreadyAdmitted { .. } => None,
        }
    }

    /// Whether this append wrote the Event.
    #[must_use]
    pub const fn is_appended(&self) -> bool {
        matches!(self, Self::Appended(_))
    }

    /// The Event that already consumed the key, when this owner knows it.
    #[must_use]
    pub fn admitted_event(&self) -> Option<(Option<&str>, Option<i64>)> {
        match self {
            Self::Appended(_) => None,
            Self::AlreadyAdmitted { event_id, sequence } => Some((event_id.as_deref(), *sequence)),
        }
    }
}

impl ConversationStore {
    /// Appends one Event under a caller-owned admission key, at most once.
    ///
    /// `admission_key` is the caller's durable identity of one inbound unit.
    /// Reusing a key answers [`EventAppendOutcome::AlreadyAdmitted`] with
    /// nothing written, which is what makes a replayed unit a merge rather than
    /// a second effect. A different key writes a different Event, so only the
    /// caller's own identity decides what counts as the same unit.
    ///
    /// Nothing here interprets the key: the owner compares it and returns it.
    #[allow(clippy::too_many_arguments)]
    pub fn append_event_once(
        &self,
        conversation_id: &str,
        author_membership_id: Option<&str>,
        kind: EventKind,
        parts: &[NewEventPart],
        causation_id: Option<&str>,
        correlation_id: Option<&str>,
        finalized: bool,
        admission_key: &str,
    ) -> StoreResult<EventAppendOutcome> {
        validate_identifier(conversation_id, "conversation_id")?;
        if let Some(author) = author_membership_id {
            validate_identifier(author, "membership_id")?;
        }
        validate_required_text(admission_key, "admission_key")?;
        if parts.len() > 4096 {
            return Err(anyhow!("event_parts_limit_exceeded"));
        }
        let event_id = new_id("event");
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            // The admission record lands before the row it authorizes, inside
            // this one transaction. A used key therefore refuses the append
            // before any Event exists, and a failure after this point rolls the
            // record back with the row.
            let claimed = transaction.execute(
                "INSERT INTO event_admission_keys(
                   conversation_id, admission_key, created_at
                 ) VALUES (?1, ?2, ?3)
                 ON CONFLICT(conversation_id, admission_key) DO NOTHING",
                params![conversation_id, admission_key, now],
            )?;
            if claimed == 0 {
                let existing = transaction
                    .query_row(
                        "SELECT event_id, sequence FROM event_admission_keys
                         WHERE conversation_id=?1 AND admission_key=?2",
                        params![conversation_id, admission_key],
                        |row| {
                            Ok((
                                row.get::<_, Option<String>>(0)?,
                                row.get::<_, Option<i64>>(1)?,
                            ))
                        },
                    )
                    .optional()?;
                transaction.commit()?;
                let (event_id, sequence) = existing.unwrap_or((None, None));
                return Ok(EventAppendOutcome::AlreadyAdmitted { event_id, sequence });
            }
            let event = insert_event(
                &transaction,
                &event_id,
                conversation_id,
                author_membership_id,
                kind,
                parts,
                causation_id,
                correlation_id,
                finalized,
                true,
                now,
            )?;
            transaction.execute(
                "UPDATE event_admission_keys SET event_id=?3, sequence=?4
                 WHERE conversation_id=?1 AND admission_key=?2",
                params![conversation_id, admission_key, event.id, event.sequence],
            )?;
            transaction.commit()?;
            Ok(EventAppendOutcome::Appended(event))
        })
    }
}
