//! Host-wide unfinished-local-work read for maintenance admission.
//!
//! One read answers, across every Conversation of this host, which canonical
//! records still hold unfinished work. It exists so a maintenance owner can
//! decide whether this host may replace installed state or activate a new
//! package without losing settlement records; the predicate is the same one
//! [`super::refuse_in_flight_group_work`] already applies to a single group,
//! shared here as SQL fragments so the two readers cannot drift.
//!
//! # Ownership
//!
//! Every row read here is a record this host admitted: it was created through
//! this store's own dispatch, direct-turn, event and Subagent-claim writes.
//! The store is the host's canonical Conversation authority under the local
//! data root, so presence of a row *is* the local-work fact. There is no
//! `local|remote|origin` column, and `conversation_dispatches.native_provenance`
//! is not one: it records the *provider-side* identity (`agentId`,
//! `nativeSessionId`, turn and message ids) of a dispatch this host executed
//! against a native agent runtime, and a locally running Codex or ACP dispatch
//! carries it too. Reading it as "remote" would exempt exactly the dispatches
//! that are executing here right now. Remote-only work executed by another peer
//! therefore never appears in this read — it has no row in this store — and an
//! unreachable peer cannot block this host by its absence.
//!
//! The limitation is the other direction: if a future path ever writes a
//! non-terminal `conversation_dispatches` row here purely to describe an
//! execution owned elsewhere, this read reports it as local work until a real
//! origin column exists. That is the fail-closed direction: a wrong "busy" is
//! recoverable, a wrong "idle" is not.

use super::*;

/// Kind of canonical record that still holds unfinished work.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LocalWorkKind {
    /// An Event this host has not finalized, so its settlement record is open.
    UnfinalizedEvent,
    /// A Direct Turn in a non-terminal state (queued, claimed, running, or
    /// waiting for a human decision).
    DirectTurn,
    /// A Conversation dispatch this host accepted and has not settled.
    ConversationDispatch,
    /// A Subagent dispatch claim this host granted and has not settled.
    SubagentClaim,
}

impl LocalWorkKind {
    /// The stable wire name, also used as the SQL discriminator.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnfinalizedEvent => "unfinalized-event",
            Self::DirectTurn => "direct-turn",
            Self::ConversationDispatch => "conversation-dispatch",
            Self::SubagentClaim => "subagent-claim",
        }
    }
}

/// One record that makes this host's work unfinished.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalWorkBlocker {
    pub kind: LocalWorkKind,
    /// The Conversation the record belongs to.
    pub conversation_id: String,
    /// Stable identity inside its kind: event id, turn id, dispatch id or
    /// claim id.
    pub identity: String,
    /// The stored state that makes the record a blocker. Events have no state
    /// column, so an unfinalized Event reports `unfinalized`.
    pub state: String,
}

/// The bounded answer of one host-wide unfinished-work read.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnfinishedLocalWork {
    pub blockers: Vec<LocalWorkBlocker>,
    /// True when more blockers exist than [`MAX_UNFINISHED_LOCAL_WORK`]
    /// reports. The decision is still "not idle"; the list is a bounded
    /// explanation, not a complete inventory.
    pub truncated: bool,
}

impl UnfinishedLocalWork {
    /// The empty answer for a host that has no Conversation store yet: no
    /// record exists, so no local work does either.
    pub fn empty() -> Self {
        Self {
            blockers: Vec::new(),
            truncated: false,
        }
    }

    /// No unfinished local work is recorded: the host is idle.
    pub fn is_idle(&self) -> bool {
        self.blockers.is_empty()
    }

    /// The reported blockers, in the read's deterministic order.
    pub fn blockers(&self) -> &[LocalWorkBlocker] {
        &self.blockers
    }
}

/// Upper bound on the blockers one read returns. The decision only needs
/// existence, but a truthful reason needs at least a representative sample.
pub const MAX_UNFINISHED_LOCAL_WORK: usize = 128;

/// Event predicate: an unfinalized Event has not settled.
pub(super) const UNFINISHED_EVENT_SQL: &str = "finalized=0";
/// Direct Turn states that still own work.
pub(super) const UNFINISHED_DIRECT_TURN_STATES: &str =
    "'pending','claimed','running','waiting-for-human'";
/// Conversation dispatch states that still own work.
pub(super) const UNFINISHED_DISPATCH_STATES: &str = "'accepted','running','cancel-requested'";
/// Subagent claim states that still own work.
pub(super) const UNFINISHED_CLAIM_STATES: &str =
    "'claimed','running','cancel-requested','reconciliation-required'";

/// The canonical Conversation database path for a data root, without opening
/// or creating the store.
pub fn local_work_database_path(portable_root: &Path) -> PathBuf {
    portable_root
        .join("client-state")
        .join("conversations")
        .join(DATABASE_FILE)
}

/// Every unfinished locally owned task this host records, read straight from
/// the canonical database without opening, initializing, recovering or
/// mutating it.
///
/// A maintenance decision must never change the work it decides about, so it
/// uses this entry rather than [`ConversationStore::open`], which initializes
/// the schema and cold-recovers interrupted work. A data root without a
/// database has no record, so it has no local work either; a database whose
/// schema is not current is an error, because this reader cannot migrate it.
pub fn read_unfinished_local_work(portable_root: &Path) -> StoreResult<UnfinishedLocalWork> {
    let path = local_work_database_path(portable_root);
    if !path.is_file() {
        return Ok(UnfinishedLocalWork::empty());
    }
    let connection = Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| anyhow!("conversation_database_open_failed"))?;
    let version: String = connection
        .query_row(
            "SELECT value FROM schema_meta WHERE key='version'",
            [],
            |row| row.get(0),
        )
        .map_err(|_| anyhow!("conversation_database_preflight_failed"))?;
    if version != CURRENT_SCHEMA_VERSION {
        return Err(anyhow!("conversation_schema_migration_required"));
    }
    unfinished_local_work(&connection)
}

impl ConversationStore {
    /// Every unfinished locally owned task this host records, across all
    /// Conversations, in a deterministic order (kind, conversation id,
    /// identity).
    ///
    /// This is one bounded read. It never creates work and never mutates the
    /// store.
    pub fn unfinished_local_work(&self) -> StoreResult<UnfinishedLocalWork> {
        self.with_connection(|connection| unfinished_local_work(connection))
    }
}

pub(super) fn unfinished_local_work(
    connection: &impl CountedSqlite,
) -> StoreResult<UnfinishedLocalWork> {
    let claims = if table_exists(connection, "subagent_dispatch_claims")? {
        format!(
            " UNION ALL SELECT '{claim_kind}', conversation_id, id, state
                FROM subagent_dispatch_claims
               WHERE state IN ({states})",
            claim_kind = LocalWorkKind::SubagentClaim.as_str(),
            states = UNFINISHED_CLAIM_STATES,
        )
    } else {
        String::new()
    };
    let sql = format!(
        "SELECT kind, conversation_id, identity, state FROM (
           SELECT '{event_kind}' AS kind, conversation_id AS conversation_id,
                  id AS identity, 'unfinalized' AS state
             FROM events
            WHERE {event_predicate}
           UNION ALL
           SELECT '{turn_kind}', conversation_id, id, state
             FROM direct_turns
            WHERE state IN ({turn_states})
           UNION ALL
           SELECT '{dispatch_kind}', conversation_id, id, state
             FROM conversation_dispatches
            WHERE state IN ({dispatch_states})
           {claims}
         )
         ORDER BY kind, conversation_id, identity
         LIMIT {limit}",
        event_kind = LocalWorkKind::UnfinalizedEvent.as_str(),
        event_predicate = UNFINISHED_EVENT_SQL,
        turn_kind = LocalWorkKind::DirectTurn.as_str(),
        turn_states = UNFINISHED_DIRECT_TURN_STATES,
        dispatch_kind = LocalWorkKind::ConversationDispatch.as_str(),
        dispatch_states = UNFINISHED_DISPATCH_STATES,
        claims = claims,
        limit = MAX_UNFINISHED_LOCAL_WORK + 1,
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let truncated = rows.len() > MAX_UNFINISHED_LOCAL_WORK;
    let blockers = rows
        .into_iter()
        .take(MAX_UNFINISHED_LOCAL_WORK)
        .map(|(kind, conversation_id, identity, state)| {
            let kind = match kind.as_str() {
                "unfinalized-event" => LocalWorkKind::UnfinalizedEvent,
                "direct-turn" => LocalWorkKind::DirectTurn,
                "conversation-dispatch" => LocalWorkKind::ConversationDispatch,
                "subagent-claim" => LocalWorkKind::SubagentClaim,
                // The reader wrote the discriminator, so an unknown one is a
                // programming error rather than data.
                other => return Err(anyhow!("local_work_kind_unknown: {other}")),
            };
            Ok(LocalWorkBlocker {
                kind,
                conversation_id,
                identity,
                state,
            })
        })
        .collect::<StoreResult<Vec<_>>>()?;
    Ok(UnfinishedLocalWork {
        blockers,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DispatchSessionMode, DispatchState, SubagentDispatchClaimState, TurnState};

    fn store() -> ConversationStore {
        ConversationStore::open_in_memory().expect("synthetic conversation store")
    }

    fn agent(id: &str) -> Principal {
        Principal {
            id: format!("agent:{id}"),
            kind: PrincipalKind::Agent,
            display_name: id.to_owned(),
            agent_id: Some(id.to_owned()),
            created_at_unix_ms: 1,
        }
    }

    fn owner() -> Principal {
        Principal {
            id: "human:owner".to_owned(),
            kind: PrincipalKind::Human,
            display_name: "Owner".to_owned(),
            agent_id: None,
            created_at_unix_ms: 1,
        }
    }

    /// One conversation with one local agent, created through the store's own
    /// dispatch admission so the fixture matches production shapes. The
    /// fixture's own dispatch and Event are settled, so a test starts idle.
    fn single_agent(store: &ConversationStore, session: &str) -> (String, String) {
        let (conversation_id, membership_id, dispatch_id, event_id) =
            admitted_single_agent(store, session);
        store.finalize_event(&event_id).expect("finalize");
        settle(store, &dispatch_id);
        (conversation_id, membership_id)
    }

    /// The same fixture left exactly as the store admitted it: one unfinalized
    /// Event and one accepted dispatch are still unfinished work.
    fn admitted_single_agent(
        store: &ConversationStore,
        session: &str,
    ) -> (String, String, String, String) {
        let scope = store
            .prepare_runtime_dispatch(
                "synthetic",
                session,
                "synthetic request",
                None,
                None,
                None,
                None,
            )
            .expect("dispatch");
        (
            scope.conversation_id,
            scope.membership_id,
            scope.dispatch_id,
            scope.event_id,
        )
    }

    fn settle(store: &ConversationStore, dispatch_id: &str) {
        store
            .update_dispatch(dispatch_id, DispatchState::Running, None, None)
            .expect("running");
        store
            .update_dispatch(dispatch_id, DispatchState::Completed, None, None)
            .expect("settled");
    }

    /// One group conversation with local agent memberships.
    fn group(store: &ConversationStore, agents: &[&str]) -> (String, Vec<String>) {
        let members = agents
            .iter()
            .map(|id| (agent(id), MembershipAccess::Member))
            .collect::<Vec<_>>();
        let conversation = store
            .create_conversation_with_members("Synthetic", owner(), &members)
            .expect("group conversation");
        let memberships = conversation
            .memberships
            .iter()
            .filter(|membership| membership.principal.kind == PrincipalKind::Agent)
            .map(|membership| membership.id.clone())
            .collect();
        (conversation.id, memberships)
    }

    fn dispatch_blockers(work: &UnfinishedLocalWork) -> Vec<(String, String, String)> {
        let mut blockers = work
            .blockers()
            .iter()
            .filter(|blocker| blocker.kind == LocalWorkKind::ConversationDispatch)
            .map(|blocker| {
                (
                    blocker.identity.clone(),
                    blocker.conversation_id.clone(),
                    blocker.state.clone(),
                )
            })
            .collect::<Vec<_>>();
        blockers.sort();
        blockers
    }

    fn find<'a>(
        work: &'a UnfinishedLocalWork,
        kind: LocalWorkKind,
        identity: &str,
    ) -> Option<&'a LocalWorkBlocker> {
        work.blockers()
            .iter()
            .find(|blocker| blocker.kind == kind && blocker.identity == identity)
    }

    #[test]
    fn a_store_with_no_local_work_is_idle() {
        let work = store().unfinished_local_work().expect("read");
        assert!(work.is_idle());
        assert!(!work.truncated);
        assert!(work.blockers().is_empty());
    }

    #[test]
    fn every_unfinished_dispatch_state_blocks_with_its_own_identity() {
        for state in [
            DispatchState::Accepted,
            DispatchState::Running,
            DispatchState::CancelRequested,
        ] {
            let store = store();
            let (conversation_id, membership_id) = single_agent(&store, "synthetic-session");
            let dispatch = store
                .create_dispatch(
                    &conversation_id,
                    &membership_id,
                    "send",
                    DispatchSessionMode::New,
                )
                .expect("dispatch");
            if state != DispatchState::Accepted {
                store
                    .update_dispatch(&dispatch.id, state, None, None)
                    .expect("dispatch state");
            }
            let work = store.unfinished_local_work().expect("read");
            assert_eq!(
                find(&work, LocalWorkKind::ConversationDispatch, &dispatch.id),
                Some(&LocalWorkBlocker {
                    kind: LocalWorkKind::ConversationDispatch,
                    conversation_id: conversation_id.clone(),
                    identity: dispatch.id.clone(),
                    state: state.as_str().to_owned(),
                }),
                "{state:?} must block with its own identity"
            );
        }
    }

    #[test]
    fn a_stopped_or_settled_dispatch_stops_blocking() {
        for terminal in [
            DispatchState::Completed,
            DispatchState::Failed,
            DispatchState::Cancelled,
        ] {
            let store = store();
            let (conversation_id, membership_id) = single_agent(&store, "synthetic-session");
            let dispatch = store
                .create_dispatch(
                    &conversation_id,
                    &membership_id,
                    "send",
                    DispatchSessionMode::New,
                )
                .expect("dispatch");
            store
                .update_dispatch(&dispatch.id, DispatchState::Running, None, None)
                .expect("running");
            store
                .update_dispatch(&dispatch.id, terminal, None, None)
                .expect("terminal");
            let work = store.unfinished_local_work().expect("read");
            assert!(
                work.is_idle(),
                "{terminal:?} must stop blocking: {:?}",
                work.blockers()
            );
        }
    }

    #[test]
    fn queued_claimed_and_running_direct_turns_block_until_they_settle() {
        let store = store();
        let (conversation_id, memberships) = group(&store, &["one"]);
        let target = memberships[0].clone();
        let event = store
            .append_event(
                &conversation_id,
                None,
                EventKind::Message,
                &[NewEventPart {
                    id: String::new(),
                    kind: EventPartKind::Text,
                    content: "@One synthetic".to_owned(),
                }],
                None,
                None,
                true,
            )
            .expect("event");
        let turns = store
            .enqueue_mention_turns(&conversation_id, &event.id, &[target])
            .expect("mention turn");
        let turn_id = turns[0].id.clone();
        let blocked = |store: &ConversationStore, state: &str| {
            let work = store.unfinished_local_work().expect("read");
            let blocker = find(&work, LocalWorkKind::DirectTurn, &turn_id)
                .unwrap_or_else(|| panic!("{state} turn must block"));
            assert_eq!(blocker.state, state);
            assert_eq!(blocker.conversation_id, conversation_id);
        };
        blocked(&store, "pending");
        store.claim_direct_turn(&turn_id).expect("claim").unwrap();
        blocked(&store, "claimed");
        assert!(store.mark_direct_turn_running(&turn_id).expect("running"));
        blocked(&store, "running");
        // A turn the runtime parked on a human decision keeps blocking; the
        // state machine owns the state name and the store owns the row.
        store
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE direct_turns SET state=?2 WHERE id=?1",
                    params![turn_id, TurnState::WaitingForHuman.as_str()],
                )?;
                Ok(())
            })
            .expect("waiting turn");
        blocked(&store, "waiting-for-human");
        store
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE direct_turns SET state=?2 WHERE id=?1",
                    params![turn_id, TurnState::Succeeded.as_str()],
                )?;
                Ok(())
            })
            .expect("settled turn");
        assert!(
            store.unfinished_local_work().expect("read").is_idle(),
            "a settled turn and a finalized Event stop blocking"
        );
    }

    #[test]
    fn unfinalized_events_block_until_they_are_finalized() {
        let store = store();
        let (conversation_id, _, dispatch_id, event_id) =
            admitted_single_agent(&store, "synthetic-session");
        let work = store.unfinished_local_work().expect("read");
        assert_eq!(
            find(&work, LocalWorkKind::UnfinalizedEvent, &event_id),
            Some(&LocalWorkBlocker {
                kind: LocalWorkKind::UnfinalizedEvent,
                conversation_id: conversation_id.clone(),
                identity: event_id.clone(),
                state: "unfinalized".to_owned(),
            })
        );
        store.finalize_event(&event_id).expect("finalize");
        assert!(
            !store.unfinished_local_work().expect("read").is_idle(),
            "the fixture's accepted dispatch is still unfinished work"
        );
        settle(&store, &dispatch_id);
        assert!(store.unfinished_local_work().expect("read").is_idle());
    }

    #[test]
    fn every_unsettled_subagent_claim_state_blocks_and_settlement_releases() {
        for (state, expected) in [
            (SubagentDispatchClaimState::Claimed, "claimed"),
            (SubagentDispatchClaimState::Running, "running"),
            (
                SubagentDispatchClaimState::CancelRequested,
                "cancel-requested",
            ),
            (
                SubagentDispatchClaimState::ReconciliationRequired,
                "reconciliation-required",
            ),
        ] {
            let store = store();
            let (conversation_id, memberships) = group(&store, &["caller", "target"]);
            let claim = store
                .claim_subagent_dispatch(&conversation_id, &memberships[0], &memberships[1], None)
                .expect("claim");
            let steps: &[SubagentDispatchClaimState] = match state {
                SubagentDispatchClaimState::Claimed => &[],
                SubagentDispatchClaimState::Running => &[SubagentDispatchClaimState::Running],
                SubagentDispatchClaimState::CancelRequested => &[
                    SubagentDispatchClaimState::Running,
                    SubagentDispatchClaimState::CancelRequested,
                ],
                _ => &[SubagentDispatchClaimState::ReconciliationRequired],
            };
            for step in steps {
                store
                    .update_subagent_claim_state(&claim.id, *step)
                    .expect("claim state");
            }
            let work = store.unfinished_local_work().expect("read");
            assert_eq!(
                find(&work, LocalWorkKind::SubagentClaim, &claim.id),
                Some(&LocalWorkBlocker {
                    kind: LocalWorkKind::SubagentClaim,
                    conversation_id: conversation_id.clone(),
                    identity: claim.id.clone(),
                    state: expected.to_owned(),
                }),
                "{expected} must block"
            );
            store
                .update_subagent_claim_state(&claim.id, SubagentDispatchClaimState::Completed)
                .expect("settle");
            assert!(store.unfinished_local_work().expect("read").is_idle());
        }
    }

    #[test]
    fn a_remote_only_delivery_or_pending_protocol_reply_is_not_local_work() {
        let store = store();
        let (conversation_id, memberships) = group(&store, &["caller", "target"]);
        let claim = store
            .claim_subagent_dispatch(&conversation_id, &memberships[0], &memberships[1], None)
            .expect("claim");
        // A pending delivery to a recipient and an inbound protocol reply are
        // records about a remote peer's exchange, not locally owned tasks.
        store
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO subagent_dispatch_deliveries(
                       claim_id, kind, conversation_id, recipient_membership_id, state,
                       payload, attempt_count, created_at, updated_at
                     ) VALUES (?1, 'observation', ?2, ?3, 'pending', '{}', 0, 1, 1)",
                    params![claim.id, conversation_id, memberships[1]],
                )?;
                connection.execute(
                    "INSERT INTO subagent_mcp_inbound(
                       id, conversation_id, caller_membership_id, target_membership_id,
                       tool, outcome, created_at
                     ) VALUES ('inbound:synthetic', ?1, ?2, ?3, 'lico_subagent_continue',
                               'pending', 1)",
                    params![conversation_id, memberships[0], memberships[1]],
                )?;
                Ok(())
            })
            .expect("protocol records");
        let blockers = store
            .unfinished_local_work()
            .expect("read")
            .blockers()
            .iter()
            .filter(|blocker| {
                blocker.identity == "inbound:synthetic"
                    || blocker.kind == LocalWorkKind::ConversationDispatch
            })
            .count();
        assert_eq!(
            blockers, 0,
            "a pending delivery or protocol reply is never a local-work row"
        );
        // Settling the claim the delivery belongs to is what removes it.
        store
            .update_subagent_claim_state(&claim.id, SubagentDispatchClaimState::Completed)
            .expect("settle");
        assert!(store.unfinished_local_work().expect("read").is_idle());
    }

    #[test]
    fn every_conversation_is_read_at_once_in_a_deterministic_order() {
        let store = store();
        let mut conversations = Vec::new();
        for session in ["synthetic-one", "synthetic-two"] {
            let (conversation_id, membership_id) = single_agent(&store, session);
            let dispatch = store
                .create_dispatch(
                    &conversation_id,
                    &membership_id,
                    "send",
                    DispatchSessionMode::New,
                )
                .expect("dispatch");
            conversations.push((conversation_id, dispatch.id));
        }
        let first = conversations[0].0.clone();
        let second = conversations[1].0.clone();
        let read = || {
            store
                .unfinished_local_work()
                .expect("read")
                .blockers()
                .iter()
                .map(|blocker| {
                    (
                        blocker.kind.as_str(),
                        blocker.conversation_id.clone(),
                        blocker.identity.clone(),
                        blocker.state.clone(),
                    )
                })
                .collect::<Vec<_>>()
        };
        let first_read = read();
        assert_eq!(first_read, read(), "the read is deterministic");
        let read_conversations = first_read
            .iter()
            .map(|(_, conversation_id, _, _)| conversation_id.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            read_conversations,
            BTreeSet::from([first, second]),
            "one read covers every Conversation"
        );
        let mut expected = conversations
            .iter()
            .map(|(conversation_id, dispatch_id)| {
                (
                    dispatch_id.clone(),
                    conversation_id.clone(),
                    "accepted".to_owned(),
                )
            })
            .collect::<Vec<_>>();
        expected.sort();
        assert_eq!(
            dispatch_blockers(&store.unfinished_local_work().expect("read")),
            expected
        );
    }

    #[test]
    fn the_read_is_bounded_and_reports_truncation() {
        let store = store();
        let (conversation_id, membership_id) = single_agent(&store, "synthetic-session");
        for _ in 0..(MAX_UNFINISHED_LOCAL_WORK + 4) {
            store
                .append_event(
                    &conversation_id,
                    Some(&membership_id),
                    EventKind::Message,
                    &[NewEventPart {
                        id: String::new(),
                        kind: EventPartKind::Text,
                        content: "synthetic".to_owned(),
                    }],
                    None,
                    None,
                    false,
                )
                .expect("event");
        }
        let work = store.unfinished_local_work().expect("read");
        assert!(work.truncated, "a longer backlog reports truncation");
        assert_eq!(work.blockers().len(), MAX_UNFINISHED_LOCAL_WORK);
        assert!(!work.is_idle());
        assert_eq!(
            work.blockers()
                .iter()
                .filter(|blocker| blocker.kind == LocalWorkKind::UnfinalizedEvent)
                .count(),
            MAX_UNFINISHED_LOCAL_WORK
        );
    }

    #[test]
    fn the_database_path_is_the_store_path_and_reading_it_creates_nothing() {
        let root = std::env::temp_dir().join(format!("lico-local-work-{}", uuid::Uuid::new_v4()));
        let path = local_work_database_path(&root);
        assert_eq!(
            path,
            root.join("client-state")
                .join("conversations")
                .join("conversations.sqlite3")
        );
        assert!(!path.exists(), "reading the path creates nothing");
        let store = ConversationStore::open(&root).expect("open");
        assert_eq!(store.db_path(), path);
    }
}
