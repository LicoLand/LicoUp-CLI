//! Workflow notices: the Conversation side's durable acceptance of a fact.
//!
//! ## What arrives here, and what it is worth
//!
//! A workflow notice is a committed fact of a run — a result, a cancellation, a
//! projection — addressed to this Conversation. It arrives through the
//! subscription and delivery machinery
//! (`licoup-workflow-runtime::routing`, `licoup-workflow-store::deliveries`),
//! which guarantees only *delivery*: at-least-once, with a stable identity that
//! survives a retry. This module is the other half. It is where the fact either
//! becomes something durable in this Conversation or is durably recorded as
//! still owed.
//!
//! Two things it does not do, because the contract says it must not:
//!
//! * **It grants nothing.** A subscription carries committed facts and
//!   notifications; it does not confer authority, and accepting a notice here
//!   produces no permission, no grant, and no membership. The only scope it
//!   checks is that the Conversation exists.
//! * **It owns no workflow state.** It does not decide whether a run succeeded,
//!   whether an effect happened, or what a goal's lifecycle is — those facts are
//!   committed where they are decided, and this module only records that this
//!   Conversation has accepted them.
//!
//! ## Identity, and why a repeat is not a second piece of work
//!
//! The notice id comes from the fact — the run, the sequence, the recipient, and
//! the kind — and never from the attempt that carried it. This module takes that
//! id as given and refuses to reinterpret it: if an id is presented a second time
//! with a different kind or for a different Conversation, that is an identity
//! moved onto another fact, and it is refused
//! ([`ContinuityFailureCode::IdentityConflict`]) rather than merged. When the id
//! is presented again with the *same* fact, the acceptance count rises and
//! [`WorkflowNoticeReceipt::first_acceptance`] is `false`: the physical
//! redelivery is recorded as a repeat, and nothing is created twice.
//!
//! ## Two obligations, accounted separately
//!
//! ```text
//!   obligation           what accepting it means
//!   timelineProjection   this Conversation has durably taken the projection
//!   reliableWake         this Conversation has durably taken the wake
//! ```
//!
//! They are separate rows with separate counts, so one can be accepted and
//! retried while the other is still owed, and the acknowledgement of the notice
//! is the intersection: [`WorkflowNoticeAck::acknowledged`] is true only when
//! every required obligation has been accepted. A delivery pass that reported
//! the notice settled on the strength of one of them would be claiming a wake it
//! did not schedule, or a projection it did not record.
//!
//! ## One notice, one logical wake
//!
//! [`workflow_notice_wake_id`] is the identity a wake must be enqueued under. The
//! wake outbox is keyed by it, so a notice redelivered five times yields one
//! logical wake — and the paid turn behind a wake therefore cannot be started
//! twice by a delivery retry. The physical delivery count and the logical wake
//! count are different numbers on purpose, and this is the function that keeps
//! them different.

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};

use super::error::{continuity_failure, sql_failure, store_to_continuity};
use super::generated::{ContinuityFailure, ContinuityFailureCode, ContinuityFailureStage};
use super::hooks::continuity_now_ms;
use super::migrate::migrate_or_fail;
use crate::store::{ContinuityUnitOfWork, ConversationStore};

/// The table this module owns, additive to the continuity schema.
///
/// Created by [`ensure_workflow_notice_schema`] on every entry point rather than
/// by the continuity migration, for the same reason the migration is idempotent:
/// a host that opens an older Conversation file gets the table on first use, and
/// a host that has it validates nothing and rewrites nothing.
const NOTICE_STATEMENTS: [&str; 2] = [
    "CREATE TABLE IF NOT EXISTS continuity_workflow_notices (
       notice_id TEXT NOT NULL,
       obligation TEXT NOT NULL CHECK(obligation IN ('timelineProjection', 'reliableWake')),
       conversation_id TEXT NOT NULL,
       kind TEXT NOT NULL,
       accept_count INTEGER NOT NULL,
       first_accepted_at INTEGER NOT NULL,
       last_accepted_at INTEGER NOT NULL,
       PRIMARY KEY(notice_id, obligation)
     )",
    "CREATE INDEX IF NOT EXISTS continuity_workflow_notices_scope_idx
       ON continuity_workflow_notices(conversation_id, obligation, notice_id)",
];

/// The longest identity this format stores for a notice or a kind.
const MAX_NOTICE_FIELD_LEN: usize = 160;

/// One downstream acceptance a workflow notice owes this Conversation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AcceptanceObligation {
    /// The timeline projection of the fact has been durably taken.
    TimelineProjection,
    /// The wake that the fact asks for has been durably taken.
    ReliableWake,
}

impl AcceptanceObligation {
    /// Every obligation a notice owes, in a stable order.
    ///
    /// Stated as a list rather than derived from the enum so that adding an
    /// obligation is a decision someone makes here, where the acknowledgement
    /// rule is, instead of a change that quietly widens what "settled" means.
    pub const REQUIRED: [Self; 2] = [Self::TimelineProjection, Self::ReliableWake];

    /// The obligation as it is stored.
    pub const fn wire(self) -> &'static str {
        match self {
            Self::TimelineProjection => "timelineProjection",
            Self::ReliableWake => "reliableWake",
        }
    }

    /// The obligation a stored value names.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "timelineProjection" => Some(Self::TimelineProjection),
            "reliableWake" => Some(Self::ReliableWake),
            _ => None,
        }
    }
}

/// What one acceptance did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkflowNoticeReceipt {
    pub notice_id: String,
    pub obligation: AcceptanceObligation,
    pub conversation_id: String,
    /// False when this Conversation had already accepted this obligation: the
    /// same logical notice arriving again, which must not become new work.
    pub first_acceptance: bool,
    /// How many times this obligation has now been accepted, including this one.
    pub accept_count: u64,
}

/// How far the acknowledgement of one notice has got.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkflowNoticeAck {
    pub notice_id: String,
    /// The Conversation the notice was accepted under, once any obligation has
    /// been accepted.
    pub conversation_id: Option<String>,
    /// The obligations durably accepted, in a stable order.
    pub accepted: Vec<AcceptanceObligation>,
    /// The required obligations still owed. Each of these is retryable on its
    /// own: accepting one never accepts another.
    pub missing: Vec<AcceptanceObligation>,
}

impl WorkflowNoticeAck {
    /// Whether every required obligation has been durably accepted.
    pub fn acknowledged(&self) -> bool {
        self.missing.is_empty()
    }

    /// Whether at least one obligation has been accepted.
    pub fn partially_accepted(&self) -> bool {
        !self.accepted.is_empty() && !self.acknowledged()
    }
}

/// The logical wake identity one notice's wake obligation carries.
///
/// The wake outbox is keyed by this id, so a notice delivered twice yields one
/// logical wake and therefore at most one wake turn. A caller that minted a new
/// id per attempt would turn a delivery retry into a second paid turn, which is
/// exactly what the stable notice identity exists to prevent.
pub fn workflow_notice_wake_id(notice_id: &str) -> String {
    format!("workflow-notice:{notice_id}")
}

/// Accept one obligation of one workflow notice, durably.
///
/// Idempotent in the notice's identity: the same obligation of the same notice
/// accepted twice records a repeat and creates nothing new. A notice id already
/// stored for a different kind or a different Conversation is refused, because
/// that is an identity moved onto another fact.
pub fn accept_workflow_notice(
    store: &ConversationStore,
    conversation_id: &str,
    notice_id: &str,
    kind: &str,
    obligation: AcceptanceObligation,
) -> Result<WorkflowNoticeReceipt, ContinuityFailure> {
    admit_notice(conversation_id, notice_id, kind)?;
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        ensure_workflow_notice_schema(unit)?;
        if !unit
            .conversation_exists(conversation_id)
            .map_err(store_to_continuity)?
        {
            return Err(continuity_failure(
                ContinuityFailureCode::ScopeDenied,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
        let now = continuity_now_ms();
        // One notice id names one fact, and a fact belongs to one Conversation
        // with one kind. Checked across every obligation, not only against the
        // row this obligation would update: an id accepted for the projection
        // must not be silently completed by a wake row that names a different
        // Conversation or kind, because the ledger would then hold two
        // contradictory halves of one identity.
        let existing: Option<(String, String)> = unit
            .query_row(
                "SELECT conversation_id, kind FROM continuity_workflow_notices
                 WHERE notice_id=?1 ORDER BY obligation ASC LIMIT 1",
                rusqlite::params![notice_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(sql_failure)?;
        if let Some((known_conversation, known_kind)) = existing {
            if known_conversation != conversation_id || known_kind != kind {
                return Err(continuity_failure(
                    ContinuityFailureCode::IdentityConflict,
                    ContinuityFailureStage::ContinuityAdmission,
                ));
            }
        }
        let accept_count: Option<i64> = unit
            .query_row(
                "INSERT INTO continuity_workflow_notices(
                   notice_id, obligation, conversation_id, kind, accept_count,
                   first_accepted_at, last_accepted_at
                 ) VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5)
                 ON CONFLICT(notice_id, obligation) DO UPDATE SET
                   accept_count = continuity_workflow_notices.accept_count + 1,
                   last_accepted_at = excluded.last_accepted_at
                 WHERE continuity_workflow_notices.conversation_id = excluded.conversation_id
                   AND continuity_workflow_notices.kind = excluded.kind
                 RETURNING accept_count",
                rusqlite::params![notice_id, obligation.wire(), conversation_id, kind, now],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(sql_failure)?;
        let Some(accept_count) = accept_count else {
            // The conflict clause updated nothing. The check above already
            // refused a stored identity that names another fact, so this is the
            // same refusal seen from the write side — kept as a backstop rather
            // than trusted to be unreachable.
            return Err(continuity_failure(
                ContinuityFailureCode::IdentityConflict,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        };
        let accept_count = u64::try_from(accept_count.max(0)).unwrap_or(u64::MAX);
        unit.request_commit();
        Ok(WorkflowNoticeReceipt {
            notice_id: notice_id.to_owned(),
            obligation,
            conversation_id: conversation_id.to_owned(),
            first_acceptance: accept_count == 1,
            accept_count,
        })
    })
}

/// Whether one notice is settled: the intersection of its required obligations.
///
/// Read from this Conversation's own ledger, so the answer survives a restart
/// and does not depend on which process asks.
pub fn workflow_notice_ack(
    store: &ConversationStore,
    notice_id: &str,
) -> Result<WorkflowNoticeAck, ContinuityFailure> {
    admit_field(notice_id)?;
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        ensure_workflow_notice_schema(unit)?;
        let rows: Vec<(String, String)> = unit
            .query_vec(
                "SELECT obligation, conversation_id FROM continuity_workflow_notices
                 WHERE notice_id=?1 ORDER BY obligation ASC",
                rusqlite::params![notice_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(sql_failure)?;
        let mut conversation_id: Option<String> = None;
        let mut accepted = Vec::new();
        for (stored, conversation) in rows {
            match &conversation_id {
                None => conversation_id = Some(conversation),
                Some(known) if *known == conversation => {}
                Some(_) => {
                    // One notice belongs to one fact, and a fact belongs to one
                    // Conversation; rows that disagree are not a settled notice.
                    return Err(continuity_failure(
                        ContinuityFailureCode::IdentityConflict,
                        ContinuityFailureStage::ContinuityAdmission,
                    ));
                }
            }
            let obligation = AcceptanceObligation::parse(&stored).ok_or_else(|| {
                continuity_failure(
                    ContinuityFailureCode::InvalidRequest,
                    ContinuityFailureStage::ContinuityCommit,
                )
            })?;
            accepted.push(obligation);
        }
        accepted.sort();
        let missing = AcceptanceObligation::REQUIRED
            .iter()
            .filter(|obligation| !accepted.contains(obligation))
            .copied()
            .collect();
        Ok(WorkflowNoticeAck {
            notice_id: notice_id.to_owned(),
            conversation_id,
            accepted,
            missing,
        })
    })
}

/// Create this module's table if the Conversation file does not have it.
///
/// Additive, idempotent, and safe inside the continuity unit of work — the same
/// shape as the continuity schema's own ensure steps.
pub fn ensure_workflow_notice_schema(
    unit: &ContinuityUnitOfWork<'_>,
) -> Result<(), ContinuityFailure> {
    for statement in NOTICE_STATEMENTS {
        unit.execute(statement, []).map_err(sql_failure)?;
    }
    Ok(())
}

fn admit_notice(
    conversation_id: &str,
    notice_id: &str,
    kind: &str,
) -> Result<(), ContinuityFailure> {
    for field in [conversation_id, notice_id, kind] {
        admit_field(field)?;
    }
    Ok(())
}

/// Refuse an identity these columns cannot hold.
///
/// Checked before the write rather than left to the column: the same rule as the
/// delivery side's identity validation, so a host cannot store an identity here
/// that the producer of the notice could not have produced.
fn admit_field(value: &str) -> Result<(), ContinuityFailure> {
    if value.trim().is_empty()
        || value != value.trim()
        || value.len() > MAX_NOTICE_FIELD_LEN
        || value.chars().any(char::is_control)
    {
        return Err(continuity_failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    Ok(())
}

enum UnitOutcome<T> {
    Done(T),
    Failed(ContinuityFailure),
}

/// Run one continuity unit of work, the way the other continuity modules do.
///
/// The failure is carried out of the closure so the transaction rolls back
/// rather than committing a partial write.
fn run_unit<T>(
    store: &ConversationStore,
    work: impl FnOnce(&mut ContinuityUnitOfWork<'_>) -> Result<T, ContinuityFailure>,
) -> Result<T, ContinuityFailure> {
    let outcome = store
        .with_continuity_unit_of_work(|unit| match work(unit) {
            Ok(value) => Ok(UnitOutcome::Done(value)),
            Err(failure) => {
                unit.abandon();
                Ok(UnitOutcome::Failed(failure))
            }
        })
        .map_err(store_to_continuity)?;
    match outcome {
        UnitOutcome::Done(value) => Ok(value),
        UnitOutcome::Failed(failure) => Err(failure),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_conversation::{Principal, PrincipalKind};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn scratch(label: &str) -> PathBuf {
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "licoup-workflow-notices-{label}-{}-{unique}.sqlite3",
            std::process::id()
        ))
    }

    fn remove(path: &Path) {
        for suffix in ["", "-wal", "-shm"] {
            let mut name = path.as_os_str().to_owned();
            name.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(name));
        }
    }

    fn owner() -> Principal {
        Principal {
            id: "human:local".to_owned(),
            kind: PrincipalKind::Human,
            display_name: "Local User".to_owned(),
            agent_id: None,
            created_at_unix_ms: 1,
        }
    }

    fn new_conversation(store: &ConversationStore, title: &str) -> String {
        store
            .create_conversation(title, owner())
            .expect("the conversation exists")
            .id
    }

    #[test]
    fn a_repeat_records_a_repeat_and_creates_nothing_twice() {
        let store = ConversationStore::open_in_memory().expect("the store opens");
        let conversation = new_conversation(&store, "Notices");
        let first = accept_workflow_notice(
            &store,
            &conversation,
            "run-1:3:timeline:wake",
            "completion",
            AcceptanceObligation::ReliableWake,
        )
        .expect("the wake is accepted");
        assert!(first.first_acceptance);
        assert_eq!(first.accept_count, 1);

        let repeat = accept_workflow_notice(
            &store,
            &conversation,
            "run-1:3:timeline:wake",
            "completion",
            AcceptanceObligation::ReliableWake,
        )
        .expect("the redelivery is accepted as a repeat");
        assert!(!repeat.first_acceptance);
        assert_eq!(repeat.accept_count, 2);
        assert_eq!(
            rows(&store, "run-1:3:timeline:wake"),
            1,
            "a repeat adds no second record of the obligation"
        );
    }

    #[test]
    fn one_acceptance_is_not_the_acknowledgement_of_two_obligations() {
        let store = ConversationStore::open_in_memory().expect("the store opens");
        let conversation = new_conversation(&store, "Notices");
        let notice = "run-1:9:timeline:projection";
        accept_workflow_notice(
            &store,
            &conversation,
            notice,
            "timeline-projection",
            AcceptanceObligation::TimelineProjection,
        )
        .expect("the projection is accepted");

        let partial = workflow_notice_ack(&store, notice).expect("the ack reads");
        assert!(!partial.acknowledged());
        assert!(partial.partially_accepted());
        assert_eq!(
            partial.accepted,
            vec![AcceptanceObligation::TimelineProjection]
        );
        assert_eq!(
            partial.missing,
            vec![AcceptanceObligation::ReliableWake],
            "the wake is still owed and retryable on its own"
        );
        assert_eq!(
            partial.conversation_id.as_deref(),
            Some(conversation.as_str())
        );

        accept_workflow_notice(
            &store,
            &conversation,
            notice,
            "timeline-projection",
            AcceptanceObligation::ReliableWake,
        )
        .expect("the wake is accepted");
        let settled = workflow_notice_ack(&store, notice).expect("the ack reads");
        assert!(settled.acknowledged());
        assert!(settled.missing.is_empty());
    }

    #[test]
    fn the_two_obligations_keep_separate_counts() {
        let store = ConversationStore::open_in_memory().expect("the store opens");
        let conversation = new_conversation(&store, "Notices");
        let notice = "run-2:1:timeline:projection";
        for _ in 0..3 {
            accept_workflow_notice(
                &store,
                &conversation,
                notice,
                "completion",
                AcceptanceObligation::TimelineProjection,
            )
            .expect("the projection is accepted");
        }
        let wake = accept_workflow_notice(
            &store,
            &conversation,
            notice,
            "completion",
            AcceptanceObligation::ReliableWake,
        )
        .expect("the wake is accepted");
        assert!(
            wake.first_acceptance,
            "the wake's first acceptance is its own, not a repeat of the projection"
        );
        assert_eq!(wake.accept_count, 1);
        let ack = workflow_notice_ack(&store, notice).expect("the ack reads");
        assert!(ack.acknowledged());
    }

    #[test]
    fn an_identity_moved_onto_another_fact_is_refused() {
        let store = ConversationStore::open_in_memory().expect("the store opens");
        let first = new_conversation(&store, "First");
        let second = new_conversation(&store, "Second");
        let notice = "run-3:1:timeline:projection";
        accept_workflow_notice(
            &store,
            &first,
            notice,
            "completion",
            AcceptanceObligation::TimelineProjection,
        )
        .expect("the projection is accepted");

        let failure = accept_workflow_notice(
            &store,
            &second,
            notice,
            "completion",
            AcceptanceObligation::TimelineProjection,
        )
        .expect_err("the same identity in another Conversation is a different fact");
        assert_eq!(failure.code, ContinuityFailureCode::IdentityConflict);
        let other_kind = accept_workflow_notice(
            &store,
            &first,
            notice,
            "cancellation",
            AcceptanceObligation::TimelineProjection,
        )
        .expect_err("the same identity with another kind is a different fact");
        assert_eq!(other_kind.code, ContinuityFailureCode::IdentityConflict);
        assert_eq!(
            workflow_notice_ack(&store, notice)
                .expect("the ack reads")
                .conversation_id
                .as_deref(),
            Some(first.as_str()),
            "the refused attempts changed nothing"
        );
    }

    #[test]
    fn an_identity_cannot_be_completed_under_another_conversation_or_kind() {
        let store = ConversationStore::open_in_memory().expect("the store opens");
        let first = new_conversation(&store, "First");
        let second = new_conversation(&store, "Second");
        let notice = "run-8:1:timeline:projection";
        accept_workflow_notice(
            &store,
            &first,
            notice,
            "completion",
            AcceptanceObligation::TimelineProjection,
        )
        .expect("the projection is accepted");

        // The other half of the same identity arrives for another Conversation.
        // Accepting it would leave one notice with two contradictory owners, so
        // it is refused even though no row for this obligation exists yet.
        let other_conversation = accept_workflow_notice(
            &store,
            &second,
            notice,
            "completion",
            AcceptanceObligation::ReliableWake,
        )
        .expect_err("the same identity in another Conversation is a different fact");
        assert_eq!(
            other_conversation.code,
            ContinuityFailureCode::IdentityConflict
        );
        let other_kind = accept_workflow_notice(
            &store,
            &first,
            notice,
            "cancellation",
            AcceptanceObligation::ReliableWake,
        )
        .expect_err("the same identity with another kind is a different fact");
        assert_eq!(other_kind.code, ContinuityFailureCode::IdentityConflict);

        let ack = workflow_notice_ack(&store, notice).expect("the ack reads");
        assert_eq!(ack.accepted, vec![AcceptanceObligation::TimelineProjection]);
        assert_eq!(ack.conversation_id.as_deref(), Some(first.as_str()));
        assert!(
            !ack.acknowledged(),
            "the refused halves added nothing, so the wake is still owed"
        );
    }

    #[test]
    fn a_notice_for_a_conversation_that_does_not_exist_is_out_of_scope() {
        let store = ConversationStore::open_in_memory().expect("the store opens");
        let failure = accept_workflow_notice(
            &store,
            "conversation-absent",
            "run-4:1:timeline:projection",
            "completion",
            AcceptanceObligation::TimelineProjection,
        )
        .expect_err("a notice for no Conversation is refused");
        assert_eq!(failure.code, ContinuityFailureCode::ScopeDenied);
        assert_eq!(
            workflow_notice_ack(&store, "run-4:1:timeline:projection")
                .expect("the ack reads")
                .accepted,
            Vec::new()
        );
    }

    #[test]
    fn a_notice_with_an_unusable_identity_is_refused_before_it_is_stored() {
        let store = ConversationStore::open_in_memory().expect("the store opens");
        let conversation = new_conversation(&store, "Notices");
        for notice_id in ["", "  ", "run-5:1\n:timeline"] {
            let failure = accept_workflow_notice(
                &store,
                &conversation,
                notice_id,
                "completion",
                AcceptanceObligation::TimelineProjection,
            )
            .expect_err("an unusable identity is refused");
            assert_eq!(failure.code, ContinuityFailureCode::InvalidRequest);
        }
        let long = "n".repeat(MAX_NOTICE_FIELD_LEN + 1);
        assert!(
            accept_workflow_notice(
                &store,
                &conversation,
                &long,
                "completion",
                AcceptanceObligation::TimelineProjection,
            )
            .is_err()
        );
    }

    #[test]
    fn the_wake_identity_is_stable_and_one_per_notice() {
        assert_eq!(
            workflow_notice_wake_id("run-1:3:timeline:wake"),
            workflow_notice_wake_id("run-1:3:timeline:wake"),
            "the same notice asks for the same logical wake every time"
        );
        assert_ne!(
            workflow_notice_wake_id("run-1:3:timeline:wake"),
            workflow_notice_wake_id("run-1:4:timeline:wake"),
            "two facts are not one wake"
        );
    }

    #[test]
    fn the_ledger_survives_a_reopen() {
        let path = scratch("reopen");
        remove(&path);
        let notice = "run-6:1:timeline:projection";
        {
            let store = ConversationStore::open(&path).expect("the store opens");
            let conversation = new_conversation(&store, "Notices");
            accept_workflow_notice(
                &store,
                &conversation,
                notice,
                "completion",
                AcceptanceObligation::ReliableWake,
            )
            .expect("the wake is accepted");
        }
        {
            let store = ConversationStore::open(&path).expect("the store reopens");
            let ack = workflow_notice_ack(&store, notice).expect("the ack reads");
            assert_eq!(ack.accepted, vec![AcceptanceObligation::ReliableWake]);
            assert!(
                !ack.acknowledged(),
                "the projection is still owed after the restart"
            );
        }
        remove(&path);
    }

    /// How many obligation rows one notice has.
    fn rows(store: &ConversationStore, notice_id: &str) -> i64 {
        store
            .with_continuity_unit_of_work(|unit| {
                Ok(unit.query_row(
                    "SELECT COUNT(*) FROM continuity_workflow_notices WHERE notice_id=?1",
                    rusqlite::params![notice_id],
                    |row| row.get::<_, i64>(0),
                )?)
            })
            .expect("the ledger reads")
    }

    #[test]
    fn a_stored_obligation_round_trips_through_its_wire_name() {
        for obligation in AcceptanceObligation::REQUIRED {
            assert_eq!(
                AcceptanceObligation::parse(obligation.wire()),
                Some(obligation)
            );
        }
        assert_eq!(AcceptanceObligation::parse("immediate"), None);

        // The table's CHECK constraint is the same vocabulary as the enum: a
        // value the enum cannot name is also a value the table refuses, so a row
        // this module could not read back cannot be written either.
        let store = ConversationStore::open_in_memory().expect("the store opens");
        let conversation = new_conversation(&store, "Notices");
        accept_workflow_notice(
            &store,
            &conversation,
            "run-7:1:timeline:projection",
            "completion",
            AcceptanceObligation::TimelineProjection,
        )
        .expect("the schema exists once a notice is accepted");
        let refused = store.with_continuity_unit_of_work(|unit| {
            Ok(unit.execute(
                "INSERT INTO continuity_workflow_notices(
                   notice_id, obligation, conversation_id, kind, accept_count,
                   first_accepted_at, last_accepted_at
                 ) VALUES ('run-7:1:timeline:projection', 'immediate', ?1, 'completion', 1, 1, 1)",
                rusqlite::params![conversation],
            )?)
        });
        assert!(refused.is_err(), "an unknown obligation is not storable");
    }
}
