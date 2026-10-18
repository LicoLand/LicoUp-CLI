//! Deterministic tests over synthetic data roots.
//!
//! Every fixture builds its state through the owning store's own public writes,
//! so the decision owner is exercised against the shapes production produces
//! rather than against rows invented here. The two exceptions are stated where
//! they appear: the `waiting-for-human` turn (no public store entry parks a turn
//! on a human decision) and the protocol-only records the remote-only test
//! needs.

use super::*;
use crate::domain::workflow_runtime::routing::{ChannelKind, QueueBounds};
use crate::domain::workflow_store::{DurableControlledStore, StrategyStore};
use licoup_conversation::store::local_work_database_path;
use licoup_conversation::{
    ConversationStore, DispatchSessionMode, DispatchState, EventKind, EventPartKind,
    MembershipAccess, NewEventPart, Principal, PrincipalKind, SubagentDispatchClaimState,
};
use std::fs;

fn root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("lico-work-admission-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).expect("synthetic root");
    licoup_foundation::platform::file_security::ensure_private_dir(&root).expect("private root");
    licoup_foundation::platform::file_security::ensure_private_dir(&root.join("client-state"))
        .expect("private client state");
    root
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

/// One conversation with one local agent and no unfinished work: the fixture's
/// own admitted dispatch and Event are settled first.
fn settled_conversation(root: &Path) -> (ConversationStore, String, String) {
    let store = ConversationStore::open(root).expect("conversation store");
    let scope = store
        .prepare_runtime_dispatch(
            "synthetic",
            "synthetic-session",
            "synthetic request",
            None,
            None,
            None,
            None,
        )
        .expect("dispatch");
    store.finalize_event(&scope.event_id).expect("finalize");
    store
        .update_dispatch(&scope.dispatch_id, DispatchState::Running, None, None)
        .expect("running");
    store
        .update_dispatch(&scope.dispatch_id, DispatchState::Completed, None, None)
        .expect("settled");
    (store, scope.conversation_id, scope.membership_id)
}

fn states(admission: &Admission) -> Vec<(String, String)> {
    admission
        .blockers
        .iter()
        .map(|blocker| (blocker.kind.clone(), blocker.state.clone()))
        .collect()
}

fn decision(root: &Path) -> Admission {
    WorkAdmission::open(root).admission().expect("decision")
}

#[test]
fn an_empty_data_root_is_idle() {
    let root = root();
    let admission = decision(&root);
    assert_eq!(admission.decision, AdmissionDecision::Idle);
    assert!(admission.decision.allows_maintenance());
    assert!(admission.blockers.is_empty());
    assert!(admission.barrier.is_none());
    assert!(!admission.truncated);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_settled_conversation_is_idle() {
    let root = root();
    let (store, _, _) = settled_conversation(&root);
    drop(store);
    assert_eq!(decision(&root).decision, AdmissionDecision::Idle);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn queued_approval_waiting_stopping_and_unsettled_conversation_work_each_block() {
    let root = root();

    // Queued: a dispatch this host accepted and has not started.
    {
        let (store, conversation_id, membership_id) = settled_conversation(&root);
        let queued = store
            .create_dispatch(
                &conversation_id,
                &membership_id,
                "send",
                DispatchSessionMode::New,
            )
            .expect("queued dispatch");
        let admission = decision(&root);
        assert_eq!(admission.decision, AdmissionDecision::Blocked);
        assert_eq!(
            states(&admission),
            vec![("conversation-dispatch".to_owned(), "accepted".to_owned())]
        );
        assert_eq!(
            admission.blockers[0].owner,
            LocalWorkOwner::CanonicalConversation
        );
        assert_eq!(admission.blockers[0].scope, conversation_id);
        // Settling the queued dispatch reopens the decision.
        store
            .update_dispatch(&queued.id, DispatchState::Cancelled, None, None)
            .expect("cancel");
    }
    assert_eq!(decision(&root).decision, AdmissionDecision::Idle);

    // Approval-waiting: a turn the runtime parked on a human decision. No
    // public store entry parks a turn, so the fixture writes the same stored
    // state the turn machine defines.
    {
        let (store, conversation_id, membership_id) = settled_conversation(&root);
        let event = store
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
                true,
            )
            .expect("event");
        let turns = store
            .enqueue_mention_turns(
                &conversation_id,
                &event.id,
                std::slice::from_ref(&membership_id),
            )
            .expect("mention turn");
        let connection = rusqlite::Connection::open(local_work_database_path(&root))
            .expect("fixture connection");
        connection
            .execute(
                "UPDATE direct_turns SET state='waiting-for-human' WHERE id=?1",
                rusqlite::params![turns[0].id],
            )
            .expect("parked turn");
        let admission = decision(&root);
        assert_eq!(admission.decision, AdmissionDecision::Blocked);
        assert_eq!(
            states(&admission),
            vec![("direct-turn".to_owned(), "waiting-for-human".to_owned())]
        );
        assert_eq!(admission.blockers[0].identity, turns[0].id);
    }

    let _ = fs::remove_dir_all(root);
}

#[test]
fn stopping_and_disconnected_but_unsettled_claim_work_blocks_until_it_settles() {
    let root = root();
    let store = ConversationStore::open(&root).expect("conversation store");
    let conversation = store
        .create_conversation_with_members(
            "Synthetic",
            owner(),
            &[
                (agent("caller"), MembershipAccess::Member),
                (agent("target"), MembershipAccess::Member),
            ],
        )
        .expect("group conversation");
    let memberships = conversation
        .memberships
        .iter()
        .filter(|membership| membership.principal.kind == PrincipalKind::Agent)
        .map(|membership| membership.id.clone())
        .collect::<Vec<_>>();
    let claim = store
        .claim_subagent_dispatch(&conversation.id, &memberships[0], &memberships[1], None)
        .expect("claim");
    store
        .update_subagent_claim_state(&claim.id, SubagentDispatchClaimState::Running)
        .expect("running");
    store
        .update_subagent_claim_state(&claim.id, SubagentDispatchClaimState::CancelRequested)
        .expect("stopping");
    let admission = decision(&root);
    assert_eq!(admission.decision, AdmissionDecision::Blocked);
    assert_eq!(
        states(&admission),
        vec![("subagent-claim".to_owned(), "cancel-requested".to_owned())]
    );
    assert_eq!(admission.blockers[0].identity, claim.id);
    store
        .update_subagent_claim_state(
            &claim.id,
            SubagentDispatchClaimState::ReconciliationRequired,
        )
        .expect("disconnected");
    assert_eq!(
        states(&decision(&root)),
        vec![(
            "subagent-claim".to_owned(),
            "reconciliation-required".to_owned()
        )],
        "an unsettled claim whose execution disconnected still blocks"
    );
    store
        .update_subagent_claim_state(&claim.id, SubagentDispatchClaimState::Completed)
        .expect("settle");
    assert_eq!(decision(&root).decision, AdmissionDecision::Idle);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn paused_and_queued_workflow_work_blocks_until_it_is_cleared() {
    let root = root();
    let store = StrategyStore::open(&root).expect("strategy store");
    let control = DurableControlledStore::from_store(store.clone());
    control
        .mark_pause_negotiating("graph:synthetic", "node:synthetic")
        .expect("pause");
    let admission = decision(&root);
    assert_eq!(admission.decision, AdmissionDecision::Blocked);
    assert_eq!(
        states(&admission),
        vec![(
            "workflow-pause-request".to_owned(),
            "pause-requested".to_owned()
        )]
    );
    assert_eq!(
        admission.blockers[0].owner,
        LocalWorkOwner::AdaptiveFlywheel
    );
    assert_eq!(admission.blockers[0].scope, "graph:synthetic");
    assert!(
        control
            .clear_pause_negotiating("graph:synthetic", "node:synthetic")
            .expect("clear")
    );
    assert_eq!(decision(&root).decision, AdmissionDecision::Idle);

    store
        .durable_queue()
        .enqueue(
            ChannelKind::Control,
            "item:synthetic",
            serde_json::json!({}),
            1,
            QueueBounds::default(),
        )
        .expect("queued item");
    assert_eq!(
        states(&decision(&root)),
        vec![("workflow-queue-item".to_owned(), "pending".to_owned())]
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn an_unsettled_invocation_and_a_held_graph_block_until_they_are_released() {
    let root = root();
    let store = StrategyStore::open(&root).expect("strategy store");
    let control = DurableControlledStore::from_store(store.clone());
    control
        .set_graph_barrier("graph:synthetic", true)
        .expect("barrier");
    store
        .with_connection(|connection| -> anyhow::Result<()> {
            connection.execute(
                "INSERT INTO workflow_invocations(graph_id, node_id, invocation_id, settled)
                 VALUES ('graph:synthetic', 'node:synthetic', 'invocation:synthetic', 0)",
                [],
            )?;
            Ok(())
        })
        .expect("unsettled invocation");
    let admission = decision(&root);
    assert_eq!(admission.decision, AdmissionDecision::Blocked);
    assert_eq!(
        states(&admission),
        vec![
            (
                "workflow-graph-barrier".to_owned(),
                "barrier-active".to_owned()
            ),
            ("workflow-invocation".to_owned(), "unsettled".to_owned()),
        ]
    );
    control
        .mark_invocation_settled("graph:synthetic", "node:synthetic", "invocation:synthetic")
        .expect("settle");
    control
        .set_graph_barrier("graph:synthetic", false)
        .expect("release");
    assert_eq!(decision(&root).decision, AdmissionDecision::Idle);

    // A durable stop request is a control fact, not unfinished work: the run or
    // queue item it targets is what blocks.
    control
        .mark_stop_requested("graph:synthetic", "node:synthetic")
        .expect("stop request");
    assert_eq!(decision(&root).decision, AdmissionDecision::Idle);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_remote_only_record_and_a_pending_protocol_reply_do_not_block() {
    let root = root();
    let store = ConversationStore::open(&root).expect("conversation store");
    let conversation = store
        .create_conversation_with_members(
            "Synthetic",
            owner(),
            &[
                (agent("caller"), MembershipAccess::Member),
                (agent("target"), MembershipAccess::Member),
            ],
        )
        .expect("group conversation");
    let memberships = conversation
        .memberships
        .iter()
        .filter(|membership| membership.principal.kind == PrincipalKind::Agent)
        .map(|membership| membership.id.clone())
        .collect::<Vec<_>>();
    let claim = store
        .claim_subagent_dispatch(&conversation.id, &memberships[0], &memberships[1], None)
        .expect("claim");
    store
        .update_subagent_claim_state(&claim.id, SubagentDispatchClaimState::Completed)
        .expect("settle");
    // A delivery to a recipient and an inbound protocol reply are records about
    // a peer's exchange, not locally owned tasks.
    let connection =
        rusqlite::Connection::open(local_work_database_path(&root)).expect("fixture connection");
    connection
        .execute(
            "INSERT INTO subagent_dispatch_deliveries(
               claim_id, kind, conversation_id, recipient_membership_id, state,
               payload, attempt_count, created_at, updated_at
             ) VALUES (?1, 'observation', ?2, ?3, 'pending', '{}', 0, 1, 1)",
            rusqlite::params![claim.id, conversation.id, memberships[1]],
        )
        .expect("pending delivery");
    connection
        .execute(
            "INSERT INTO subagent_mcp_inbound(
               id, conversation_id, caller_membership_id, target_membership_id,
               tool, outcome, created_at
             ) VALUES ('inbound:synthetic', ?1, ?2, ?3, 'lico_subagent_continue', 'pending', 1)",
            rusqlite::params![conversation.id, memberships[0], memberships[1]],
        )
        .expect("pending reply");
    let admission = decision(&root);
    assert_eq!(
        admission.decision,
        AdmissionDecision::Idle,
        "{:?}",
        admission.blockers
    );
    assert!(admission.blockers.is_empty());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn cold_recovery_settles_interrupted_local_work_and_the_decision_returns_to_idle() {
    let root = root();
    {
        let store = ConversationStore::open(&root).expect("conversation store");
        store
            .prepare_runtime_dispatch(
                "synthetic",
                "synthetic-session",
                "synthetic request",
                None,
                None,
                None,
                None,
            )
            .expect("dispatch")
    };
    let admission = decision(&root);
    assert_eq!(admission.decision, AdmissionDecision::Blocked);
    assert!(admission.blockers.len() >= 2, "{:?}", admission.blockers);

    // Opening the canonical authority cold-recovers the interrupted dispatch
    // and finalizes its Event; recovery is idempotent from then on.
    let store = ConversationStore::open(&root).expect("conversation store");
    let report = store.cold_recover().expect("cold recovery");
    assert_eq!(
        (
            report.recovered_dispatches,
            report.finalized_events,
            report.interrupted_turns
        ),
        (0, 0, 0),
        "the opening already settled the interrupted work: {report:?}"
    );
    drop(store);
    assert_eq!(
        decision(&root).decision,
        AdmissionDecision::Idle,
        "confirmed recovery of the interrupted dispatch reopens admission"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn the_barrier_is_taken_with_the_idle_decision_and_survives_a_reopen() {
    let root = root();
    let barrier = match WorkAdmission::open(&root)
        .begin_maintenance(MaintenanceOperation::ClientReplacement)
        .expect("begin")
    {
        MaintenanceAdmission::Held(barrier) => barrier,
        other => panic!("expected the barrier, got {other:?}"),
    };
    assert_eq!(barrier.operation, MaintenanceOperation::ClientReplacement);
    assert_eq!(barrier.state, "claimed");

    // A restart reads the durable record instead of reopening admission.
    let reopened = WorkAdmission::open(&root);
    let admission = reopened.admission().expect("decision");
    assert_eq!(admission.decision, AdmissionDecision::Closed);
    assert!(!admission.decision.allows_maintenance());
    assert_eq!(admission.barrier.as_ref(), Some(&barrier));
    match reopened
        .begin_maintenance(MaintenanceOperation::PackageActivation)
        .expect("second begin")
    {
        MaintenanceAdmission::AlreadyClosed(held) => assert_eq!(held, barrier),
        other => panic!("a held barrier refuses a second switch, got {other:?}"),
    }

    // An explicit release restores admission, idempotently.
    reopened.release_admission().expect("release");
    assert_eq!(decision(&root).decision, AdmissionDecision::Idle);
    reopened.release_admission().expect("release is idempotent");
    match reopened
        .begin_maintenance(MaintenanceOperation::PackageActivation)
        .expect("begin again")
    {
        MaintenanceAdmission::Held(next) => {
            assert_eq!(next.operation, MaintenanceOperation::PackageActivation)
        }
        other => panic!("a released barrier admits the next switch, got {other:?}"),
    }
    reopened.release_admission().expect("release");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn unfinished_work_refuses_the_barrier_and_holds_no_barrier_record() {
    let root = root();
    let store = ConversationStore::open(&root).expect("conversation store");
    store
        .prepare_runtime_dispatch(
            "synthetic",
            "synthetic-session",
            "synthetic request",
            None,
            None,
            None,
            None,
        )
        .expect("dispatch");
    drop(store);
    match WorkAdmission::open(&root)
        .begin_maintenance(MaintenanceOperation::ClientReplacement)
        .expect("begin")
    {
        MaintenanceAdmission::Blocked { blockers, .. } => {
            assert!(
                blockers
                    .iter()
                    .any(|blocker| blocker.kind == "conversation-dispatch")
            );
        }
        other => panic!("unfinished work must refuse the barrier, got {other:?}"),
    }
    assert!(
        !barrier::record_path(&root).exists(),
        "a refused switch writes no barrier"
    );
    assert_eq!(decision(&root).decision, AdmissionDecision::Blocked);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn an_unreadable_barrier_record_is_refused_instead_of_reopening_admission() {
    let root = root();
    let path = barrier::record_path(&root);
    fs::write(&path, b"{ not a barrier }\n").expect("synthetic record");
    let error = WorkAdmission::open(&root)
        .admission()
        .expect_err("an unreadable barrier is an error");
    assert!(
        format!("{error:#}").contains("maintenance_admission_record_invalid"),
        "{error:#}"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn the_layer_neutral_package_entries_wrap_the_same_decision() {
    let root = root();
    assert_eq!(hold_package_activation_admission(&root), Ok(()));
    assert_eq!(
        hold_package_activation_admission(&root),
        Err("maintenance_admission_closed")
    );
    assert_eq!(release_maintenance_admission(&root), Ok(()));
    assert_eq!(hold_package_activation_admission(&root), Ok(()));
    assert_eq!(release_maintenance_admission(&root), Ok(()));

    let store = ConversationStore::open(&root).expect("conversation store");
    store
        .prepare_runtime_dispatch(
            "synthetic",
            "synthetic-session",
            "synthetic request",
            None,
            None,
            None,
            None,
        )
        .expect("dispatch");
    drop(store);
    assert_eq!(
        hold_package_activation_admission(&root),
        Err("maintenance_admission_blocked")
    );
    let _ = fs::remove_dir_all(root);
}
