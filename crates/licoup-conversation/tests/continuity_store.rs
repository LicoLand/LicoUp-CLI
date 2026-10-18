use licoup_conversation::continuity::{
    ASSISTANT_TURN_INVALID_ERROR, ContextCompositionPort, ContinuityAgreement,
    ContinuityAgreementOrigin, ContinuityAgreementScope, ContinuityAssistantTurnResponse,
    ContinuityClosureAuthorityKind, ContinuityCommitPort, ContinuityContextCompositionRequest,
    ContinuityCriterion, ContinuityEffectStatus, ContinuityEvidenceRef, ContinuityEvidenceResult,
    ContinuityFailureCode, ContinuityFollowThroughKind, ContinuityGoalCompletionTransition,
    ContinuityGoalControl, ContinuityGoalEvent, ContinuityGoalLifecycle, ContinuityGoalProgress,
    ContinuityInterpretationProposal, ContinuityInterrupt, ContinuityOracleKind,
    ContinuityParentContextGrant, ContinuityParentGrantStatus, ContinuityReadPort,
    ContinuitySourceOwnerKind, ContinuitySourceRef, ContinuitySourceValidity, ContinuitySpeechAct,
    ContinuityTaskChildAdmission, ContinuityUtf8ByteSpan, ContinuityVerificationKind,
    ContinuityVisibilityScope, ContinuityWake, FollowUpPort, INGRESS_USER_POSTED_DESIGNATION,
    PENDING_OBLIGATION_PAGE_SIZE, TRUSTED_RESPONSE_MODE_ASSISTANT_TURN,
    UnavailableContextComposition, accept_completion, ack_completion_notices,
    append_criterion_evidence, apply_goal_control, commit_user_posted_proposal,
    consume_logical_wake, derived_live_count, ingress_execution_recorded, list_all_parent_grants,
    list_completion_notification_ids, list_due_goals, list_pending_completion_notices,
    parse_continuity_value, put_agreement, put_derived, put_effect, put_grant, read_agreements,
    read_goal, read_pending_outbox, read_pending_wakes, read_relation_for_child,
    record_settlement_applied, record_settlement_pending, replay_effect, resolve_completion_notice,
    revoke_source, schedule_goal_due, set_continuity_interrupt, settlement_applied,
};
use licoup_conversation::{
    ConversationStore, DispatchState, EventKind, EventPartKind, MembershipAccess, NewEventPart,
    Principal, PrincipalKind,
};
use rusqlite;

fn human() -> Principal {
    Principal {
        id: "principal:human".into(),
        kind: PrincipalKind::Human,
        display_name: "Human".into(),
        agent_id: None,
        created_at_unix_ms: 1,
    }
}

fn agent() -> Principal {
    Principal {
        id: "principal:agent".into(),
        kind: PrincipalKind::Agent,
        display_name: "Agent".into(),
        agent_id: Some("agent:local".into()),
        created_at_unix_ms: 1,
    }
}

fn source_ref(event_id: &str, revision: i64) -> ContinuitySourceRef {
    ContinuitySourceRef {
        owner_kind: ContinuitySourceOwnerKind::Event,
        opaque_id: event_id.to_owned(),
        part_id: None,
        span: None,
        source_revision: revision,
        digest: format!("card:{event_id}"),
        visibility_scope: ContinuityVisibilityScope::Conversation,
        validity: ContinuitySourceValidity::Current,
    }
}

struct Harness {
    store: ConversationStore,
    conversation_id: String,
    owner_membership_id: String,
    event_id: String,
    revision: i64,
    epoch: i64,
}

impl Harness {
    fn new(title: &str) -> Self {
        let store = ConversationStore::open_in_memory().unwrap();
        let conversation = store.create_conversation(title, human()).unwrap();
        let owner = conversation
            .memberships
            .iter()
            .find(|membership| membership.access == MembershipAccess::Owner)
            .unwrap()
            .id
            .clone();
        let event = store
            .append_event(
                &conversation.id,
                Some(&owner),
                EventKind::Message,
                &[NewEventPart {
                    id: String::new(),
                    kind: EventPartKind::Text,
                    content: "delegate notes".into(),
                }],
                None,
                None,
                true,
            )
            .unwrap();
        let basis = store.continuity_commit_basis(&conversation.id).unwrap();
        Self {
            store,
            conversation_id: conversation.id,
            owner_membership_id: owner,
            event_id: event.id,
            revision: basis.revision,
            epoch: basis.designation_epoch,
        }
    }

    fn refresh(&mut self) {
        let basis = self
            .store
            .continuity_commit_basis(&self.conversation_id)
            .unwrap();
        self.revision = basis.revision;
        self.epoch = basis.designation_epoch;
    }

    fn proposal(
        &self,
        request_id: &str,
        goal_id: &str,
        matter_id: &str,
        with_child: bool,
    ) -> ContinuityInterpretationProposal {
        ContinuityInterpretationProposal {
            envelope: licoup_conversation::continuity::ContinuityWriteEnvelope {
                conversation_id: self.conversation_id.clone(),
                source_event_refs: vec![source_ref(&self.event_id, 1)],
                observed_revision: self.revision,
                designation_epoch: self.epoch,
                request_id: request_id.to_owned(),
            },
            matter_associations: vec![
                licoup_conversation::continuity::ContinuityMatterAssociation {
                    matter_id: matter_id.to_owned(),
                    source_ref: source_ref(&self.event_id, 1),
                    association_revision: 1,
                    proposed_by: self.owner_membership_id.clone(),
                    reason_code: "user-delegation".into(),
                    supersedes: None,
                },
            ],
            speech_act: ContinuitySpeechAct::Delegation,
            commitment_proposals: vec![
                licoup_conversation::continuity::ContinuityCommitmentProposal {
                    matter_id: Some(matter_id.to_owned()),
                    subject: licoup_conversation::continuity::ContinuityMatterSubject::New,
                    expected_result: "Draft the notes".into(),
                    criteria: Vec::new(),
                    create_goal: true,
                },
            ],
            agreement_proposals: Vec::new(),
            capability_needs: vec!["writing".into()],
            uncertainty_reasons: Vec::new(),
            requested_reads: Vec::new(),
            task_child_admission: with_child.then(|| ContinuityTaskChildAdmission {
                goal_id: goal_id.to_owned(),
                parent_conversation_id: self.conversation_id.clone(),
                speech_act: ContinuitySpeechAct::Delegation,
                follow_through_kind: ContinuityFollowThroughKind::Durable,
                observed_child_conversation_id: None,
                observed_card_anchor: None,
                request_id: format!("admit:{request_id}"),
            }),
        }
    }
}

#[test]
fn migrate_does_not_synthesize_goals_or_revive_summaries() {
    let harness = Harness::new("legacy chat");
    let before = read_goal(&harness.store, "goal:missing").unwrap();
    assert!(before.is_none());
    let second = harness
        .store
        .continuity_commit_basis(&harness.conversation_id);
    assert!(second.is_ok());
    assert!(read_goal(&harness.store, "goal:missing").unwrap().is_none());
    assert_eq!(
        derived_live_count(&harness.store, &harness.conversation_id).unwrap(),
        0
    );
}

#[test]
fn current_schema_repairs_missing_defaults_and_designation_trigger() {
    let store = ConversationStore::open_in_memory().unwrap();
    assert!(store.ensure_continuity_migrated().unwrap());
    store
        .with_continuity_unit_of_work(|unit| {
            unit.execute(
                "DELETE FROM continuity_schema
                 WHERE key IN ('adoption_enabled', 'adoption_stage')",
                [],
            )?;
            unit.execute("DROP TRIGGER continuity_bump_designation_epoch", [])?;
            unit.request_commit();
            Ok(())
        })
        .unwrap();

    assert!(store.ensure_continuity_migrated().unwrap());
    let values = store
        .with_continuity_unit_of_work(|unit| {
            Ok((
                unit.query_row(
                    "SELECT value FROM continuity_schema WHERE key='adoption_enabled'",
                    [],
                    |row| row.get::<_, String>(0),
                )?,
                unit.query_row(
                    "SELECT value FROM continuity_schema WHERE key='adoption_stage'",
                    [],
                    |row| row.get::<_, String>(0),
                )?,
                unit.query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type='trigger' AND name='continuity_bump_designation_epoch'",
                    [],
                    |row| row.get::<_, i64>(0),
                )?,
            ))
        })
        .unwrap();
    assert_eq!(values, ("1".to_owned(), "offline".to_owned(), 1));
    assert!(!store.ensure_continuity_migrated().unwrap());
}

#[test]
fn newer_continuity_schema_is_rejected_without_rewriting_it() {
    let store = ConversationStore::open_in_memory().unwrap();
    store.ensure_continuity_migrated().unwrap();
    store
        .with_continuity_unit_of_work(|unit| {
            unit.execute(
                "UPDATE continuity_schema SET value='8' WHERE key='version'",
                [],
            )?;
            unit.request_commit();
            Ok(())
        })
        .unwrap();

    let error = store.ensure_continuity_migrated().unwrap_err();
    assert_eq!(error.to_string(), "continuity_schema_unsupported_version");
    let version = store
        .with_continuity_unit_of_work(|unit| {
            Ok(unit.query_row(
                "SELECT value FROM continuity_schema WHERE key='version'",
                [],
                |row| row.get::<_, String>(0),
            )?)
        })
        .unwrap();
    assert_eq!(version, "8");
}

#[test]
fn commit_is_atomic_for_state_cursor_and_outbox() {
    let mut harness = Harness::new("atomic");
    let proposal = harness.proposal("request:atomic", "goal:notes", "matter:notes", false);
    harness.store.commit(&proposal).unwrap();
    harness.refresh();
    let progress = read_goal(&harness.store, "goal:notes").unwrap().unwrap();
    assert_eq!(progress.lifecycle, ContinuityGoalLifecycle::Active);
    assert!(progress.next_attention.is_some());
    assert_eq!(
        read_pending_outbox(&harness.store, &harness.conversation_id).unwrap(),
        1
    );
    let matters = harness
        .store
        .list_matters(&harness.conversation_id, None, 20)
        .unwrap();
    assert_eq!(matters.len(), 1);
}

#[test]
fn interrupt_boundaries_leave_no_orphan_goal() {
    for point in [
        ContinuityInterrupt::BeforeFirstWrite,
        ContinuityInterrupt::AfterStateWrite,
        ContinuityInterrupt::BeforeCommit,
    ] {
        let harness = Harness::new("interrupt");
        set_continuity_interrupt(Some(point));
        let proposal = harness.proposal("request:int", "goal:notes", "matter:notes", false);
        assert!(harness.store.commit(&proposal).is_err());
        set_continuity_interrupt(None);
        assert!(read_goal(&harness.store, "goal:notes").unwrap().is_none());
        assert_eq!(
            read_pending_outbox(&harness.store, &harness.conversation_id).unwrap(),
            0
        );
    }

    let mut harness = Harness::new("after-commit");
    set_continuity_interrupt(Some(ContinuityInterrupt::AfterCommit));
    let proposal = harness.proposal("request:after", "goal:notes", "matter:notes", false);
    harness.store.commit(&proposal).unwrap();
    set_continuity_interrupt(None);
    assert!(read_goal(&harness.store, "goal:notes").unwrap().is_some());
    assert_eq!(
        read_pending_outbox(&harness.store, &harness.conversation_id).unwrap(),
        1
    );
    assert!(consume_logical_wake(&harness.store, "wake:goal:notes:1").unwrap());
    assert!(!consume_logical_wake(&harness.store, "wake:goal:notes:1").unwrap());
    harness.refresh();
}

#[test]
fn cas_and_idempotency_reject_stale_and_conflicts() {
    let mut harness = Harness::new("cas");
    let first = harness.proposal("request:one", "goal:notes", "matter:notes", false);
    harness.store.commit(&first).unwrap();
    let stale = harness.proposal("request:two", "goal:notes", "matter:notes", false);
    assert_eq!(
        harness.store.commit(&stale).unwrap_err().code,
        ContinuityFailureCode::StaleRevision
    );
    harness.refresh();
    let again = harness.proposal("request:one", "goal:notes", "matter:notes", false);
    let first_revision = harness.store.commit(&again).unwrap().revision;
    let after = harness
        .store
        .continuity_commit_basis(&harness.conversation_id)
        .unwrap();
    assert_eq!(first_revision, after.revision);

    let mut conflict = harness.proposal("request:one", "goal:other", "matter:other", false);
    conflict.envelope.observed_revision = after.revision;
    assert_eq!(
        harness.store.commit(&conflict).unwrap_err().code,
        ContinuityFailureCode::IdempotencyConflict
    );
}

#[test]
fn designation_epoch_advances_on_real_assignment_change() {
    let mut harness = Harness::new("epoch");
    let member = harness
        .store
        .add_member(&harness.conversation_id, agent(), MembershipAccess::Member)
        .unwrap();
    harness.refresh();
    harness
        .store
        .set_conversation_assistant(
            &harness.conversation_id,
            &harness.owner_membership_id,
            harness.revision,
            Some(&member.id),
        )
        .unwrap();
    harness.refresh();
    let first_epoch = harness.epoch;
    assert!(first_epoch >= 1);
    let second = harness
        .store
        .add_member(
            &harness.conversation_id,
            Principal {
                id: "principal:agent-b".into(),
                kind: PrincipalKind::Agent,
                display_name: "Agent B".into(),
                agent_id: Some("agent:b".into()),
                created_at_unix_ms: 1,
            },
            MembershipAccess::Member,
        )
        .unwrap();
    harness.refresh();
    harness
        .store
        .set_conversation_assistant(
            &harness.conversation_id,
            &harness.owner_membership_id,
            harness.revision,
            Some(&second.id),
        )
        .unwrap();
    harness.refresh();
    let second_epoch = harness.epoch;
    assert!(second_epoch > first_epoch);
    harness
        .store
        .set_conversation_assistant(
            &harness.conversation_id,
            &harness.owner_membership_id,
            harness.revision,
            Some(&member.id),
        )
        .unwrap();
    harness.refresh();
    assert!(harness.epoch > second_epoch);
    let after_return = harness.epoch;
    harness
        .store
        .set_conversation_assistant(
            &harness.conversation_id,
            &harness.owner_membership_id,
            harness.revision,
            Some(&member.id),
        )
        .unwrap();
    harness.refresh();
    assert_eq!(harness.epoch, after_return);
    let mut stale_epoch = harness.proposal("request:epoch", "goal:notes", "matter:notes", false);
    stale_epoch.envelope.designation_epoch = first_epoch;
    stale_epoch.envelope.observed_revision = harness.revision;
    assert_eq!(
        harness.store.commit(&stale_epoch).unwrap_err().code,
        ContinuityFailureCode::DesignationChanged
    );
}

#[test]
fn leave_member_clearing_assistant_advances_epoch() {
    let mut harness = Harness::new("leave-epoch");
    let member = harness
        .store
        .add_member(&harness.conversation_id, agent(), MembershipAccess::Member)
        .unwrap();
    harness.refresh();
    harness
        .store
        .set_conversation_assistant(
            &harness.conversation_id,
            &harness.owner_membership_id,
            harness.revision,
            Some(&member.id),
        )
        .unwrap();
    harness.refresh();
    let assigned = harness.epoch;
    assert!(assigned >= 1);
    harness
        .store
        .leave_member(&harness.conversation_id, &member.id)
        .unwrap();
    harness.refresh();
    assert!(harness.epoch > assigned);
    let conversation = harness.store.get(&harness.conversation_id).unwrap();
    assert!(conversation.assistant_membership_id.is_none());
}

#[test]
fn pause_and_cancel_are_orthogonal_and_cancel_blocks_new_work() {
    let mut harness = Harness::new("control");
    harness
        .store
        .commit(&harness.proposal("request:c", "goal:notes", "matter:notes", false))
        .unwrap();
    let paused = apply_goal_control(
        &harness.store,
        &harness.conversation_id,
        "goal:notes",
        ContinuityGoalEvent::Pause,
    )
    .unwrap();
    assert_eq!(paused.lifecycle, ContinuityGoalLifecycle::Active);
    assert_eq!(paused.control, ContinuityGoalControl::Paused);
    apply_goal_control(
        &harness.store,
        &harness.conversation_id,
        "goal:notes",
        ContinuityGoalEvent::Resume,
    )
    .unwrap();
    let canceling = apply_goal_control(
        &harness.store,
        &harness.conversation_id,
        "goal:notes",
        ContinuityGoalEvent::CancelRequest,
    )
    .unwrap();
    assert_eq!(canceling.control, ContinuityGoalControl::CancelRequested);
    harness.refresh();
    let blocked = harness.proposal("request:more", "goal:notes", "matter:notes", false);
    assert_eq!(
        harness.store.commit(&blocked).unwrap_err().code,
        ContinuityFailureCode::InvalidRequest
    );
    let cancelled = apply_goal_control(
        &harness.store,
        &harness.conversation_id,
        "goal:notes",
        ContinuityGoalEvent::CancelSettled,
    )
    .unwrap();
    assert_eq!(cancelled.lifecycle, ContinuityGoalLifecycle::Cancelled);
    assert!(
        apply_goal_control(
            &harness.store,
            &harness.conversation_id,
            "goal:notes",
            ContinuityGoalEvent::Resume,
        )
        .is_err()
    );
}

#[test]
fn unknown_effect_requires_reconciliation_not_replay() {
    let harness = Harness::new("effects");
    harness
        .store
        .commit(&harness.proposal("request:e", "goal:notes", "matter:notes", false))
        .unwrap();
    put_effect(
        &harness.store,
        &harness.conversation_id,
        Some("goal:notes"),
        "effect:one",
        ContinuityEffectStatus::Unknown,
    )
    .unwrap();
    assert_eq!(
        replay_effect(&harness.store, "effect:one")
            .unwrap_err()
            .code,
        ContinuityFailureCode::ReconciliationRequired
    );
    put_effect(
        &harness.store,
        &harness.conversation_id,
        Some("goal:notes"),
        "effect:one",
        ContinuityEffectStatus::Executed,
    )
    .unwrap();
    assert_eq!(
        replay_effect(&harness.store, "effect:one")
            .unwrap_err()
            .code,
        ContinuityFailureCode::InvalidRequest
    );
    let progress = read_goal(&harness.store, "goal:notes").unwrap().unwrap();
    assert_eq!(progress.lifecycle, ContinuityGoalLifecycle::Active);
}

#[test]
fn revocation_and_agreement_replacement_invalidate_derivations() {
    let mut harness = Harness::new("revoke");
    harness
        .store
        .commit(&harness.proposal("request:r", "goal:notes", "matter:notes", false))
        .unwrap();
    put_derived(
        &harness.store,
        &harness.conversation_id,
        "summary",
        &harness.event_id,
        "old-summary",
    )
    .unwrap();
    assert_eq!(
        derived_live_count(&harness.store, &harness.conversation_id).unwrap(),
        1
    );
    revoke_source(
        &harness.store,
        &harness.conversation_id,
        &harness.event_id,
        true,
    )
    .unwrap();
    assert_eq!(
        derived_live_count(&harness.store, &harness.conversation_id).unwrap(),
        0
    );
    harness.refresh();
    let replay = harness.proposal("request:replay", "goal:notes", "matter:notes", false);
    assert_eq!(
        harness.store.commit(&replay).unwrap_err().code,
        ContinuityFailureCode::SourceRevoked
    );
    put_agreement(
        &harness.store,
        &harness.conversation_id,
        &ContinuityAgreement {
            id: "agreement:one".into(),
            scope: ContinuityAgreementScope::Conversation,
            statement_ref: source_ref(&harness.event_id, 1),
            origin: ContinuityAgreementOrigin::UserExplicit,
            effective_revision: 1,
            supersedes: None,
            valid_from: 1,
            valid_until: None,
            revocation_generation: 0,
        },
    )
    .unwrap();
    put_agreement(
        &harness.store,
        &harness.conversation_id,
        &ContinuityAgreement {
            id: "agreement:two".into(),
            scope: ContinuityAgreementScope::Conversation,
            statement_ref: source_ref(&harness.event_id, 1),
            origin: ContinuityAgreementOrigin::UserExplicit,
            effective_revision: 2,
            supersedes: Some(1),
            valid_from: 2,
            valid_until: None,
            revocation_generation: 1,
        },
    )
    .unwrap();
    let current = read_agreements(&harness.store, &harness.conversation_id).unwrap();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].id, "agreement:two");
}

#[test]
fn child_relation_is_atomic_idempotent_and_ordered() {
    let mut harness = Harness::new("children");
    let first = harness.proposal("request:a", "goal:a", "matter:a", true);
    harness.store.commit(&first).unwrap();
    let relation_a = harness.store.relation_for_goal("goal:a").unwrap();
    let child = harness
        .store
        .get(&relation_a.child_conversation_id)
        .unwrap();
    assert!(!child.memberships.is_empty());
    assert_ne!(child.id, harness.conversation_id);
    harness.refresh();
    let retry = harness.proposal("request:a", "goal:a", "matter:a", true);
    harness.store.commit(&retry).unwrap();
    let again = harness.store.relation_for_goal("goal:a").unwrap();
    assert_eq!(
        again.child_conversation_id,
        relation_a.child_conversation_id
    );
    assert_eq!(again.card_anchor.event_id, relation_a.card_anchor.event_id);
    assert_eq!(again.card_anchor.sequence, relation_a.card_anchor.sequence);

    harness
        .store
        .append_event(
            &harness.conversation_id,
            Some(&harness.owner_membership_id),
            EventKind::Message,
            &[NewEventPart {
                id: String::new(),
                kind: EventPartKind::Text,
                content: "ordinary chat".into(),
            }],
            None,
            None,
            true,
        )
        .unwrap();
    harness.refresh();
    let second = harness.proposal("request:b", "goal:b", "matter:b", true);
    harness.store.commit(&second).unwrap();
    let relation_b = harness.store.relation_for_goal("goal:b").unwrap();
    assert!(relation_a.card_anchor.sequence < relation_b.card_anchor.sequence);
    assert_ne!(
        relation_a.child_conversation_id,
        relation_b.child_conversation_id
    );

    complete_goal(&harness, "goal:b");
    complete_goal(&harness, "goal:a");
    let listed = harness
        .store
        .list_child_relations(&harness.conversation_id, None, 20)
        .unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].goal_id, "goal:a");
    assert_eq!(listed[1].goal_id, "goal:b");
    assert_eq!(
        listed[0].card_anchor.sequence,
        relation_a.card_anchor.sequence
    );
    assert_eq!(
        listed[1].card_anchor.sequence,
        relation_b.card_anchor.sequence
    );
}

fn complete_goal(harness: &Harness, goal_id: &str) {
    let current = read_goal(&harness.store, goal_id).unwrap().unwrap();
    let (progress, transition) = achieved_close(
        goal_id,
        &current,
        goal_evaluation_ref(goal_id, current.revision),
        &format!("notice:{goal_id}"),
        ContinuityClosureAuthorityKind::GoalEvaluation,
    );
    assert!(
        accept_completion(
            &harness.store,
            &harness.conversation_id,
            &transition,
            &progress,
        )
        .unwrap()
    );
    assert!(
        !accept_completion(
            &harness.store,
            &harness.conversation_id,
            &transition,
            &progress,
        )
        .unwrap()
    );
    assert_eq!(
        read_goal(&harness.store, goal_id)
            .unwrap()
            .unwrap()
            .lifecycle,
        ContinuityGoalLifecycle::Achieved
    );
}

fn goal_evaluation_ref(goal_id: &str, revision: i64) -> ContinuitySourceRef {
    ContinuitySourceRef {
        owner_kind: ContinuitySourceOwnerKind::Goal,
        opaque_id: goal_id.to_owned(),
        part_id: None,
        span: None,
        source_revision: revision,
        digest: format!("goal:{goal_id}"),
        visibility_scope: ContinuityVisibilityScope::Goal,
        validity: ContinuitySourceValidity::Current,
    }
}

fn achieved_close(
    goal_id: &str,
    current: &ContinuityGoalProgress,
    evaluation_ref: ContinuitySourceRef,
    notification_id: &str,
    authority_kind: ContinuityClosureAuthorityKind,
) -> (ContinuityGoalProgress, ContinuityGoalCompletionTransition) {
    let mut progress = current.clone();
    progress.lifecycle = ContinuityGoalLifecycle::Achieved;
    progress.next_attention = None;
    progress.active_execution_refs.clear();
    let transition = ContinuityGoalCompletionTransition {
        transition_id: format!("transition:{notification_id}"),
        goal_id: goal_id.to_owned(),
        from_lifecycle: current.lifecycle,
        to_lifecycle: ContinuityGoalLifecycle::Achieved,
        goal_revision: current.revision,
        authority_kind,
        evaluation_ref,
        notification_id: notification_id.to_owned(),
    };
    (progress, transition)
}

fn commit_required_criterion_goal(
    harness: &mut Harness,
    request_id: &str,
    goal_id: &str,
    matter_id: &str,
    criterion_id: &str,
    with_child: bool,
) {
    let parent_assistant = with_child.then(|| designate_agent(harness));
    let mut proposal = harness.proposal(request_id, goal_id, matter_id, with_child);
    proposal.commitment_proposals[0]
        .criteria
        .push(ContinuityCriterion {
            id: criterion_id.to_owned(),
            description_ref: source_ref(&harness.event_id, 1),
            required: true,
            oracle_kind: ContinuityOracleKind::User,
            artifact_version_rule: "current-subject-version".into(),
            freshness_rule: "current".into(),
            evaluator_policy: "user-acceptance".into(),
        });
    if let Some(parent_assistant) = parent_assistant {
        commit_user_posted_proposal(
            &harness.store,
            &proposal,
            &harness.event_id,
            &parent_assistant,
        )
        .unwrap();
    } else {
        harness.store.commit(&proposal).unwrap();
    }
    harness.refresh();
}

fn post_owner_text(harness: &Harness, text: &str) -> licoup_conversation::ConversationEvent {
    harness
        .store
        .append_event(
            &harness.conversation_id,
            Some(&harness.owner_membership_id),
            EventKind::Message,
            &[NewEventPart {
                id: String::new(),
                kind: EventPartKind::Text,
                content: text.into(),
            }],
            None,
            None,
            true,
        )
        .unwrap()
}

fn stored_subject_version(store: &ConversationStore, goal_id: &str, criterion_id: &str) -> i64 {
    read_goal(store, goal_id)
        .unwrap()
        .unwrap()
        .criterion_evidence_refs
        .iter()
        .filter(|item| item.criterion_id == criterion_id)
        .map(|item| item.subject_version)
        .max()
        .unwrap_or(0)
}

fn notice_recorded(store: &ConversationStore, notification_id: &str) -> bool {
    list_completion_notification_ids(store)
        .unwrap()
        .iter()
        .any(|item| item == notification_id)
}

#[test]
fn worker_exit_does_not_complete_goal_and_notice_is_consumed_once() {
    let harness = Harness::new("notice");
    harness
        .store
        .commit(&harness.proposal("request:n", "goal:notes", "matter:notes", true))
        .unwrap();
    put_effect(
        &harness.store,
        &harness.conversation_id,
        Some("goal:notes"),
        "effect:worker",
        ContinuityEffectStatus::Executed,
    )
    .unwrap();
    let still = read_goal(&harness.store, "goal:notes").unwrap().unwrap();
    assert_eq!(still.lifecycle, ContinuityGoalLifecycle::Active);
    complete_goal(&harness, "goal:notes");
}

#[test]
fn grants_are_recipient_scoped_and_compose_authorized_is_the_only_entry() {
    let harness = Harness::new("grants");
    harness
        .store
        .commit(&harness.proposal("request:g", "goal:notes", "matter:notes", true))
        .unwrap();
    let relation = harness.store.relation_for_goal("goal:notes").unwrap();
    let grant = ContinuityParentContextGrant {
        grant_id: "grant:one".into(),
        source_conversation_id: harness.conversation_id.clone(),
        recipient_conversation_id: relation.child_conversation_id.clone(),
        recipient_membership_id: "membership:child".into(),
        source_refs: vec![source_ref(&harness.event_id, 1)],
        authorized_scopes: vec![ContinuityVisibilityScope::Conversation],
        status: ContinuityParentGrantStatus::Admitted,
        request_id: "request:grant".into(),
        revocation_generation: 0,
    };
    put_grant(&harness.store, &grant).unwrap();
    let listed = harness
        .store
        .list_parent_grants(
            &relation.child_conversation_id,
            "membership:child",
            None,
            20,
        )
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert!(
        harness
            .store
            .list_parent_grants(&harness.conversation_id, "membership:child", None, 20)
            .unwrap()
            .is_empty()
    );
    let request: ContinuityContextCompositionRequest = parse_continuity_value(&serde_json::json!({
        "conversationId": relation.child_conversation_id,
        "recipientMembershipId": "membership:child",
        "authorizedScopes": ["conversation"],
        "revocationGeneration": 0,
        "limit": 20
    }))
    .unwrap();
    assert_eq!(
        UnavailableContextComposition
            .compose_authorized(&request)
            .unwrap_err()
            .code,
        ContinuityFailureCode::UnsupportedCapability
    );
}

#[test]
fn missing_fixture_conversation_is_no_effect_scope_denial() {
    let store = ConversationStore::open_in_memory().unwrap();
    let conversation = store.create_conversation("other", human()).unwrap();
    let before = store.get(&conversation.id).unwrap();
    let fixture: ContinuityInterpretationProposal = parse_continuity_value(&serde_json::json!({
        "envelope": {
            "conversationId": "conversation:group",
            "sourceEventRefs": [{
                "ownerKind": "event",
                "opaqueId": "event:fixture-one",
                "sourceRevision": 1,
                "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "visibilityScope": "conversation",
                "validity": "current"
            }],
            "observedRevision": 4,
            "designationEpoch": 0,
            "requestId": "request:interpret-1"
        },
        "matterAssociations": [],
        "speechAct": "delegation",
        "commitmentProposals": [],
        "agreementProposals": [],
        "capabilityNeeds": [],
        "uncertaintyReasons": [],
        "requestedReads": []
    }))
    .unwrap();
    let error = store.commit(&fixture).unwrap_err();
    assert_eq!(error.code, ContinuityFailureCode::ScopeDenied);
    assert_eq!(
        error.effect_class,
        licoup_conversation::continuity::ContinuityEffectClass::None
    );
    let after = store.get(&conversation.id).unwrap();
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.event_count, before.event_count);
}

#[test]
fn ten_thousand_production_store_sequences_preserve_invariants() {
    let store = ConversationStore::open_in_memory().unwrap();
    let mut passed = 0u32;
    for seed in 0u64..10_000 {
        run_seeded_sequence(&store, seed);
        passed += 1;
    }
    assert_eq!(passed, 10_000);
}

fn run_seeded_sequence(store: &ConversationStore, seed: u64) {
    let conversation = store
        .create_conversation(&format!("seq-{seed}"), human())
        .unwrap();
    let owner = conversation.memberships[0].id.clone();
    let event = store
        .append_event(
            &conversation.id,
            Some(&owner),
            EventKind::Message,
            &[NewEventPart {
                id: String::new(),
                kind: EventPartKind::Text,
                content: format!("seq {seed}"),
            }],
            None,
            None,
            true,
        )
        .unwrap();
    let basis = store.continuity_commit_basis(&conversation.id).unwrap();
    let matter_id = format!("m{seed}");
    let goal_id = format!("goal:m{seed}");
    let request_id = format!("request:{seed}");
    let proposal = ContinuityInterpretationProposal {
        envelope: licoup_conversation::continuity::ContinuityWriteEnvelope {
            conversation_id: conversation.id.clone(),
            source_event_refs: vec![source_ref(&event.id, 1)],
            observed_revision: basis.revision,
            designation_epoch: basis.designation_epoch,
            request_id,
        },
        matter_associations: Vec::new(),
        speech_act: ContinuitySpeechAct::Delegation,
        commitment_proposals: vec![
            licoup_conversation::continuity::ContinuityCommitmentProposal {
                matter_id: Some(matter_id.clone()),
                subject: licoup_conversation::continuity::ContinuityMatterSubject::New,
                expected_result: "seq".into(),
                criteria: Vec::new(),
                create_goal: true,
            },
        ],
        agreement_proposals: Vec::new(),
        capability_needs: Vec::new(),
        uncertainty_reasons: Vec::new(),
        requested_reads: Vec::new(),
        task_child_admission: (seed % 7 == 0).then(|| ContinuityTaskChildAdmission {
            goal_id: goal_id.clone(),
            parent_conversation_id: conversation.id.clone(),
            speech_act: ContinuitySpeechAct::Delegation,
            follow_through_kind: ContinuityFollowThroughKind::Durable,
            observed_child_conversation_id: None,
            observed_card_anchor: None,
            request_id: format!("admit:{seed}"),
        }),
    };
    if seed % 11 == 0 {
        set_continuity_interrupt(Some(ContinuityInterrupt::BeforeCommit));
        let _ = store.commit(&proposal);
        set_continuity_interrupt(None);
        assert!(read_goal(store, &goal_id).unwrap().is_none());
        return;
    }
    store.commit(&proposal).unwrap();
    let progress = read_goal(store, &goal_id).unwrap().unwrap();
    assert!(
        progress.next_attention.is_some() || progress.control != ContinuityGoalControl::Enabled
    );
    match seed % 5 {
        1 => {
            let paused = apply_goal_control(
                store,
                &conversation.id,
                &goal_id,
                ContinuityGoalEvent::Pause,
            )
            .unwrap();
            assert_eq!(paused.lifecycle, ContinuityGoalLifecycle::Active);
        }
        2 => {
            apply_goal_control(
                store,
                &conversation.id,
                &goal_id,
                ContinuityGoalEvent::CancelRequest,
            )
            .unwrap();
            let cancelled = apply_goal_control(
                store,
                &conversation.id,
                &goal_id,
                ContinuityGoalEvent::CancelSettled,
            )
            .unwrap();
            assert_eq!(cancelled.lifecycle, ContinuityGoalLifecycle::Cancelled);
        }
        3 => {
            put_effect(
                store,
                &conversation.id,
                Some(&goal_id),
                &format!("effect:{seed}"),
                ContinuityEffectStatus::Unknown,
            )
            .unwrap();
            assert_eq!(
                replay_effect(store, &format!("effect:{seed}"))
                    .unwrap_err()
                    .code,
                ContinuityFailureCode::ReconciliationRequired
            );
        }
        4 => {
            revoke_source(store, &conversation.id, &event.id, true).unwrap();
            assert_eq!(derived_live_count(store, &conversation.id).unwrap(), 0);
        }
        _ => {
            assert_eq!(progress.lifecycle, ContinuityGoalLifecycle::Active);
        }
    }
    if seed % 7 == 0 {
        let relation = store.relation_for_goal(&goal_id).unwrap();
        assert_ne!(relation.child_conversation_id, conversation.id);
        let _ = store.get(&relation.child_conversation_id).unwrap();
    }
}

#[test]
fn natural_language_pause_targets_only_associated_goal() {
    let mut harness = Harness::new("f1-scope");
    harness
        .store
        .commit(&harness.proposal("request:a", "goal:a", "matter:a", false))
        .unwrap();
    harness.refresh();
    harness
        .store
        .commit(&harness.proposal("request:b", "goal:b", "matter:b", false))
        .unwrap();
    harness.refresh();
    harness
        .store
        .commit(&harness.proposal("request:c", "goal:c", "matter:c", false))
        .unwrap();
    complete_goal(&harness, "goal:c");
    harness.refresh();

    let mut pause_a = harness.proposal("request:pause-a", "goal:a", "matter:a", false);
    pause_a.speech_act = ContinuitySpeechAct::Pause;
    pause_a.commitment_proposals[0].create_goal = false;
    harness.store.commit(&pause_a).unwrap();

    assert_eq!(
        read_goal(&harness.store, "goal:a")
            .unwrap()
            .unwrap()
            .control,
        ContinuityGoalControl::Paused
    );
    assert_eq!(
        read_goal(&harness.store, "goal:b")
            .unwrap()
            .unwrap()
            .control,
        ContinuityGoalControl::Enabled
    );
    assert_eq!(
        read_goal(&harness.store, "goal:c")
            .unwrap()
            .unwrap()
            .lifecycle,
        ContinuityGoalLifecycle::Achieved
    );

    harness.refresh();
    let mut ambiguous = harness.proposal("request:pause-all", "goal:a", "matter:a", false);
    ambiguous.speech_act = ContinuitySpeechAct::Pause;
    ambiguous.matter_associations.clear();
    ambiguous.commitment_proposals.clear();
    ambiguous.task_child_admission = None;
    harness.store.commit(&ambiguous).unwrap();
    assert_eq!(
        read_goal(&harness.store, "goal:b")
            .unwrap()
            .unwrap()
            .control,
        ContinuityGoalControl::Enabled
    );
}

#[test]
fn first_ever_failed_commit_does_not_persist_partial_business() {
    let store = ConversationStore::open_in_memory().unwrap();
    let conversation = store.create_conversation("f2-fresh", human()).unwrap();
    let owner = conversation.memberships[0].id.clone();
    let event = store
        .append_event(
            &conversation.id,
            Some(&owner),
            EventKind::Message,
            &[NewEventPart {
                id: String::new(),
                kind: EventPartKind::Text,
                content: "delegate notes".into(),
            }],
            None,
            None,
            true,
        )
        .unwrap();
    let loaded = store.get(&conversation.id).unwrap();
    let proposal = ContinuityInterpretationProposal {
        envelope: licoup_conversation::continuity::ContinuityWriteEnvelope {
            conversation_id: conversation.id.clone(),
            source_event_refs: vec![source_ref(&event.id, 1)],
            observed_revision: loaded.revision,
            designation_epoch: 0,
            request_id: "request:f2-fail".into(),
        },
        matter_associations: vec![
            licoup_conversation::continuity::ContinuityMatterAssociation {
                matter_id: "matter:f2".into(),
                source_ref: source_ref(&event.id, 1),
                association_revision: 1,
                proposed_by: owner,
                reason_code: "user-delegation".into(),
                supersedes: None,
            },
        ],
        speech_act: ContinuitySpeechAct::Delegation,
        commitment_proposals: vec![
            licoup_conversation::continuity::ContinuityCommitmentProposal {
                matter_id: Some("matter:f2".into()),
                subject: licoup_conversation::continuity::ContinuityMatterSubject::New,
                expected_result: "Draft".into(),
                criteria: Vec::new(),
                create_goal: true,
            },
        ],
        agreement_proposals: Vec::new(),
        capability_needs: Vec::new(),
        uncertainty_reasons: Vec::new(),
        requested_reads: Vec::new(),
        task_child_admission: Some(ContinuityTaskChildAdmission {
            goal_id: "goal:f2".into(),
            parent_conversation_id: "conversation:other-parent".into(),
            speech_act: ContinuitySpeechAct::Delegation,
            follow_through_kind: ContinuityFollowThroughKind::Durable,
            observed_child_conversation_id: None,
            observed_card_anchor: None,
            request_id: "request:admit:f2".into(),
        }),
    };
    let error = store.commit(&proposal).unwrap_err();
    assert_eq!(error.code, ContinuityFailureCode::ScopeDenied);
    store.ensure_continuity_migrated().unwrap();
    assert!(read_goal(&store, "goal:f2").unwrap().is_none());
    assert_eq!(read_pending_outbox(&store, &conversation.id).unwrap(), 0);
    assert!(
        store
            .list_matters(&conversation.id, None, 20)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn user_posted_applied_marker_rolls_back_with_before_commit_interrupt() {
    let mut harness = Harness::new("atomic-ingress");
    let agent = harness
        .store
        .add_member(&harness.conversation_id, agent(), MembershipAccess::Member)
        .unwrap();
    harness.refresh();
    let proposal = harness.proposal(
        "request:atomic-ingress",
        "goal:atomic",
        "matter:atomic",
        true,
    );
    set_continuity_interrupt(Some(ContinuityInterrupt::BeforeCommit));
    let error =
        commit_user_posted_proposal(&harness.store, &proposal, &harness.event_id, &agent.id)
            .unwrap_err();
    set_continuity_interrupt(None);
    assert_eq!(error.code, ContinuityFailureCode::InvalidRequest);
    assert!(read_goal(&harness.store, "goal:atomic").unwrap().is_none());
    assert!(
        !ingress_execution_recorded(
            &harness.store,
            &harness.conversation_id,
            &harness.event_id,
            &agent.id,
            INGRESS_USER_POSTED_DESIGNATION,
        )
        .unwrap()
    );
    commit_user_posted_proposal(&harness.store, &proposal, &harness.event_id, &agent.id).unwrap();
    assert!(read_goal(&harness.store, "goal:atomic").unwrap().is_some());
    assert!(
        ingress_execution_recorded(
            &harness.store,
            &harness.conversation_id,
            &harness.event_id,
            &agent.id,
            INGRESS_USER_POSTED_DESIGNATION,
        )
        .unwrap()
    );
    assert_eq!(
        harness
            .store
            .list_child_relations(&harness.conversation_id, None, 8)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn canonical_child_designates_parent_assistant() {
    let mut harness = Harness::new("child-assistant");
    let agent = harness
        .store
        .add_member(&harness.conversation_id, agent(), MembershipAccess::Member)
        .unwrap();
    harness.refresh();
    harness
        .store
        .set_conversation_assistant(
            &harness.conversation_id,
            &harness.owner_membership_id,
            harness.revision,
            Some(&agent.id),
        )
        .unwrap();
    harness.refresh();
    let proposal = harness.proposal(
        "request:child-asst",
        "goal:child-asst",
        "matter:child-asst",
        true,
    );
    harness.store.commit(&proposal).unwrap();
    let relation = harness.store.relation_for_goal("goal:child-asst").unwrap();
    let child = harness.store.get(&relation.child_conversation_id).unwrap();
    assert!(
        child.assistant_membership_id.is_some(),
        "canonical child must designate the admitted assistant"
    );
    let assistant = child
        .memberships
        .iter()
        .find(|membership| child.assistant_membership_id.as_deref() == Some(membership.id.as_str()))
        .expect("designated child assistant membership");
    assert_eq!(assistant.principal.kind, PrincipalKind::Agent);
    assert_eq!(
        read_relation_for_child(&harness.store, &relation.child_conversation_id)
            .unwrap()
            .map(|item| item.goal_id),
        Some("goal:child-asst".into())
    );
    let intent = licoup_conversation::continuity::read_child_work_intent(
        &harness.store,
        &harness.conversation_id,
        "goal:child-asst",
        1,
    )
    .unwrap()
    .expect("child work obligation is written in the parent commit unit");
    assert_eq!(
        intent["operationId"],
        licoup_conversation::continuity::child_work_operation_id("goal:child-asst", 1)
    );
    assert_eq!(
        intent["childConversationId"],
        relation.child_conversation_id
    );
}

#[test]
fn due_index_and_settlement_cursors_are_bounded() {
    let harness = Harness::new("due-index");
    let proposal = harness.proposal("request:due", "goal:due", "matter:due", true);
    harness.store.commit(&proposal).unwrap();
    schedule_goal_due(
        &harness.store,
        &harness.conversation_id,
        "goal:due",
        1_000,
        "review-due",
        "host",
    )
    .unwrap();
    let due = list_due_goals(&harness.store, 2_000).unwrap();
    assert!(
        due.iter().any(|(goal_id, conversation_id, next_due)| {
            goal_id == "goal:due"
                && conversation_id == &harness.conversation_id
                && *next_due == 1_000
        }),
        "due goals must come from next_due index: {due:?}"
    );
    assert!(list_due_goals(&harness.store, 500).unwrap().is_empty());
    record_settlement_pending(
        &harness.store,
        &harness.conversation_id,
        "dispatch:one",
        &serde_json::json!({"output": "pending-body"}),
    )
    .unwrap();
    assert!(!settlement_applied(&harness.store, &harness.conversation_id, "dispatch:one").unwrap());
    record_settlement_applied(
        &harness.store,
        &harness.conversation_id,
        "dispatch:one",
        &serde_json::json!({"output": "applied-body"}),
    )
    .unwrap();
    assert!(settlement_applied(&harness.store, &harness.conversation_id, "dispatch:one").unwrap());
}

#[test]
fn child_work_intent_accepted_and_started_are_separate_units() {
    use licoup_conversation::continuity::{
        child_work_accepted, child_work_started, list_unacked_child_work,
        record_child_work_accepted, record_child_work_intent, record_child_work_started,
    };
    let harness = Harness::new("child-work-cursors");
    record_child_work_intent(
        &harness.store,
        &harness.conversation_id,
        "goal:due",
        1,
        &serde_json::json!({
            "kind": "child-work-intent",
            "goalId": "goal:due",
            "revision": 1,
        }),
    )
    .unwrap();
    let unacked = list_unacked_child_work(&harness.store).unwrap();
    assert_eq!(unacked.len(), 1);
    assert_eq!(unacked[0].1, "goal:due");
    assert!(!child_work_accepted(&harness.store, &harness.conversation_id, "goal:due", 1).unwrap());
    record_child_work_accepted(
        &harness.store,
        &harness.conversation_id,
        "goal:due",
        1,
        &serde_json::json!({
            "kind": "child-work-accepted",
            "dispatchId": "dispatch:accepted",
            "revision": 1,
        }),
    )
    .unwrap();
    assert!(child_work_accepted(&harness.store, &harness.conversation_id, "goal:due", 1).unwrap());
    assert_eq!(list_unacked_child_work(&harness.store).unwrap().len(), 1);
    record_child_work_started(&harness.store, &harness.conversation_id, "goal:due", 1).unwrap();
    assert!(child_work_started(&harness.store, &harness.conversation_id, "goal:due", 1).unwrap());
    assert!(list_unacked_child_work(&harness.store).unwrap().is_empty());
}

#[test]
fn child_work_and_settlement_lists_are_pending_only_anti_joins() {
    use licoup_conversation::continuity::{
        child_work_operation_id, list_unacked_child_work, list_unapplied_settlements,
        read_child_work_intent, record_child_work_intent, record_child_work_started,
        record_settlement_applied, record_settlement_pending,
    };
    let harness = Harness::new("bounded-pending-lists");
    for revision in 1..=3 {
        record_child_work_intent(
            &harness.store,
            &harness.conversation_id,
            "goal:due",
            revision,
            &serde_json::json!({
                "kind": "child-work-intent",
                "goalId": "goal:due",
                "revision": revision,
                "operationId": child_work_operation_id("goal:due", revision),
            }),
        )
        .unwrap();
    }
    record_child_work_started(&harness.store, &harness.conversation_id, "goal:due", 1).unwrap();
    record_child_work_started(&harness.store, &harness.conversation_id, "goal:due", 2).unwrap();
    let unacked = list_unacked_child_work(&harness.store).unwrap();
    assert_eq!(unacked.len(), 1);
    assert_eq!(unacked[0].2, 3);
    assert_eq!(
        unacked[0].3["operationId"],
        child_work_operation_id("goal:due", 3)
    );
    let intent = read_child_work_intent(&harness.store, &harness.conversation_id, "goal:due", 3)
        .unwrap()
        .expect("durable intent");
    assert_eq!(
        intent["operationId"],
        child_work_operation_id("goal:due", 3)
    );

    record_settlement_pending(
        &harness.store,
        &harness.conversation_id,
        "dispatch:one",
        &serde_json::json!({"output": "pending-one"}),
    )
    .unwrap();
    record_settlement_pending(
        &harness.store,
        &harness.conversation_id,
        "dispatch:two",
        &serde_json::json!({"output": "pending-two"}),
    )
    .unwrap();
    record_settlement_applied(
        &harness.store,
        &harness.conversation_id,
        "dispatch:one",
        &serde_json::json!({"output": "applied-one"}),
    )
    .unwrap();
    let pending = list_unapplied_settlements(&harness.store).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].1, "dispatch:two");
}

#[test]
fn pending_obligation_pages_and_scan_ignore_growing_acked_history() {
    use licoup_conversation::continuity::{
        PENDING_OBLIGATION_PAGE_SIZE, child_work_operation_id, list_unacked_child_work,
        list_unacked_child_work_page, pending_obligation_scan_evidence, read_child_work_intent,
        read_child_work_live, record_child_work_intent, record_child_work_live,
        record_child_work_started,
    };
    let harness = Harness::new("pending-scan-evidence");
    for revision in 1..=12 {
        record_child_work_intent(
            &harness.store,
            &harness.conversation_id,
            "goal:history",
            revision,
            &serde_json::json!({
                "kind": "child-work-intent",
                "goalId": "goal:history",
                "revision": revision,
                "admittedRevision": revision,
                "workGeneration": revision,
                "operationId": child_work_operation_id("goal:history", revision),
            }),
        )
        .unwrap();
        record_child_work_started(
            &harness.store,
            &harness.conversation_id,
            "goal:history",
            revision,
        )
        .unwrap();
    }
    record_child_work_intent(
        &harness.store,
        &harness.conversation_id,
        "goal:live",
        1,
        &serde_json::json!({
            "kind": "child-work-intent",
            "goalId": "goal:live",
            "revision": 1,
            "admittedRevision": 1,
            "workGeneration": 1,
            "operationId": child_work_operation_id("goal:live", 1),
        }),
    )
    .unwrap();
    let (work_plan, settlement_plan, pending, historical, decoded) =
        pending_obligation_scan_evidence(&harness.store).unwrap();
    assert_eq!(pending, 1);
    assert_eq!(historical, 13);
    assert_eq!(decoded, 1);
    let work_plan_text = work_plan.join("\n");
    let settlement_plan_text = settlement_plan.join("\n");
    assert!(
        work_plan_text.contains("outstanding_work"),
        "pending work scan must use the outstanding-work index: {work_plan_text}"
    );
    assert!(
        settlement_plan_text.contains("outstanding_settlement"),
        "pending settlement scan must use the outstanding-settlement index: {settlement_plan_text}"
    );
    for revision in 13..=24 {
        record_child_work_intent(
            &harness.store,
            &harness.conversation_id,
            "goal:history",
            revision,
            &serde_json::json!({
                "kind": "child-work-intent",
                "goalId": "goal:history",
                "revision": revision,
                "admittedRevision": revision,
                "operationId": child_work_operation_id("goal:history", revision),
            }),
        )
        .unwrap();
        record_child_work_started(
            &harness.store,
            &harness.conversation_id,
            "goal:history",
            revision,
        )
        .unwrap();
    }
    let (_, _, pending_after, historical_after, decoded_after) =
        pending_obligation_scan_evidence(&harness.store).unwrap();
    assert_eq!(pending_after, 1, "acked history must not grow pending rows");
    assert_eq!(historical_after, 25);
    assert_eq!(decoded_after, 1, "poll decode must stay on pending rows");
    assert!(
        read_child_work_intent(&harness.store, &harness.conversation_id, "goal:history", 1)
            .unwrap()
            .is_some()
    );

    for index in 0..3 {
        record_child_work_intent(
            &harness.store,
            &harness.conversation_id,
            &format!("goal:page-{index}"),
            1,
            &serde_json::json!({
                "kind": "child-work-intent",
                "goalId": format!("goal:page-{index}"),
                "revision": 1,
                "admittedRevision": 1,
                "operationId": child_work_operation_id(&format!("goal:page-{index}"), 1),
            }),
        )
        .unwrap();
    }
    let first = list_unacked_child_work_page(&harness.store, None, None, None, 2).unwrap();
    assert_eq!(first.len(), 2);
    let last = first.last().unwrap();
    let rest = list_unacked_child_work_page(
        &harness.store,
        Some(&last.0),
        Some(&last.1),
        Some(&format!("work:{}:{}:child-work-pending", last.1, last.2)),
        2,
    )
    .unwrap();
    assert!(!rest.is_empty());
    assert!(
        rest.iter()
            .all(|row| !first.iter().any(|seen| seen.1 == row.1))
    );
    assert!(list_unacked_child_work(&harness.store).unwrap().len() >= 4);
    assert!(PENDING_OBLIGATION_PAGE_SIZE >= 2);

    record_child_work_live(
        &harness.store,
        &harness.conversation_id,
        "goal:live",
        &serde_json::json!({
            "kind": "child-work-live",
            "goalId": "goal:live",
            "operationId": child_work_operation_id("goal:live", 1),
            "admittedRevision": 1,
            "workGeneration": 1,
            "childConversationId": "conversation:child",
            "membershipId": "membership:child",
            "parentConversationId": harness.conversation_id,
            "dispatchId": "turn:live",
        }),
    )
    .unwrap();
    assert_eq!(
        read_child_work_live(&harness.store, &harness.conversation_id, "goal:live")
            .unwrap()
            .unwrap()["dispatchId"],
        "turn:live"
    );
}

#[test]
fn file_backed_pending_and_accepted_facts_survive_reopen() {
    use licoup_conversation::continuity::{
        child_work_operation_id, list_unacked_child_work, read_child_work_accepted,
        read_child_work_intent, record_child_work_accepted, record_child_work_intent,
        record_child_work_started,
    };
    let root = std::env::temp_dir().join(format!(
        "lico-ca-pending-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let conversation_id;
    {
        let store = ConversationStore::open(&root).unwrap();
        let harness_conversation = store
            .create_conversation("reopen-pending", human())
            .unwrap();
        conversation_id = harness_conversation.id.clone();
        record_child_work_intent(
            &store,
            &conversation_id,
            "goal:reopen",
            1,
            &serde_json::json!({
                "kind": "child-work-intent",
                "goalId": "goal:reopen",
                "revision": 1,
                "admittedRevision": 1,
                "workGeneration": 1,
                "operationId": child_work_operation_id("goal:reopen", 1),
            }),
        )
        .unwrap();
        record_child_work_accepted(
            &store,
            &conversation_id,
            "goal:reopen",
            1,
            &serde_json::json!({
                "kind": "child-work-accepted",
                "goalId": "goal:reopen",
                "revision": 1,
                "admittedRevision": 1,
                "workGeneration": 1,
                "operationId": child_work_operation_id("goal:reopen", 1),
                "dispatchId": "turn:reopen",
                "childConversationId": "conversation:child",
                "membershipId": "membership:child",
                "parentConversationId": conversation_id,
            }),
        )
        .unwrap();
        assert_eq!(list_unacked_child_work(&store).unwrap().len(), 1);
    }
    let reopened = ConversationStore::open(&root).unwrap();
    assert_eq!(list_unacked_child_work(&reopened).unwrap().len(), 1);
    assert!(
        read_child_work_intent(&reopened, &conversation_id, "goal:reopen", 1)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        read_child_work_accepted(&reopened, &conversation_id, "goal:reopen", 1)
            .unwrap()
            .unwrap()["dispatchId"],
        "turn:reopen"
    );
    record_child_work_started(&reopened, &conversation_id, "goal:reopen", 1).unwrap();
    assert!(list_unacked_child_work(&reopened).unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&root);
}

fn visible_text(store: &ConversationStore, conversation_id: &str, event_id: &str) -> String {
    store
        .event(conversation_id, event_id)
        .unwrap()
        .unwrap()
        .parts
        .iter()
        .filter(|part| part.kind == EventPartKind::Text)
        .map(|part| part.content.as_str())
        .collect()
}

fn metadata_joined(store: &ConversationStore, conversation_id: &str, event_id: &str) -> String {
    store
        .event(conversation_id, event_id)
        .unwrap()
        .unwrap()
        .parts
        .iter()
        .filter(|part| part.kind == EventPartKind::Metadata)
        .map(|part| part.content.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn assistant_envelope_json(harness: &Harness, reply: &str) -> String {
    serde_json::to_string(&ContinuityAssistantTurnResponse {
        reply_text: reply.to_owned(),
        interpretation_proposal: harness.proposal(
            "request:store-envelope",
            "goal:store",
            "matter:store",
            false,
        ),
    })
    .unwrap()
}

#[test]
fn admitted_envelope_buffers_chunks_and_publishes_reply_once() {
    let harness = Harness::new("visible reply");
    let member = harness
        .store
        .add_member(&harness.conversation_id, agent(), MembershipAccess::Member)
        .unwrap();
    let scope = harness
        .store
        .prepare_runtime_dispatch(
            "agent:local",
            "",
            "ordinary question",
            Some(&harness.conversation_id),
            Some(&member.id),
            None,
            None,
        )
        .unwrap();
    harness
        .store
        .admit_runtime_response_mode(&scope, TRUSTED_RESPONSE_MODE_ASSISTANT_TURN)
        .unwrap();
    let reply = "均值为 0，方差为 1。 He said \"use {\\\"ok\\\":true}\".";
    let envelope = assistant_envelope_json(&harness, reply);
    let mid = envelope.len() / 2;
    harness
        .store
        .append_runtime_frame(
            &scope,
            1,
            &serde_json::json!({
                "event": "agent.message.chunk",
                "payload": {"text": &envelope[..mid]},
            }),
        )
        .unwrap();
    assert!(
        visible_text(&harness.store, &harness.conversation_id, &scope.event_id).is_empty(),
        "admitted chunks must not publish envelope bytes as Text"
    );
    harness
        .store
        .append_runtime_frame(
            &scope,
            2,
            &serde_json::json!({
                "event": "agent.message.completed",
                "payload": {"text": envelope},
            }),
        )
        .unwrap();
    assert_eq!(
        visible_text(&harness.store, &harness.conversation_id, &scope.event_id),
        reply
    );
    let state = harness
        .store
        .finish_runtime_dispatch(
            &scope,
            &serde_json::json!({"output": envelope}),
            DispatchState::Completed,
            None,
        )
        .unwrap();
    assert_eq!(state, DispatchState::Completed);
    assert_eq!(
        visible_text(&harness.store, &harness.conversation_id, &scope.event_id),
        reply
    );
    let metadata = metadata_joined(&harness.store, &harness.conversation_id, &scope.event_id);
    assert!(metadata.contains("trustedResponseMode"));
    assert!(metadata.contains("speechAct"));
    assert!(
        !visible_text(&harness.store, &harness.conversation_id, &scope.event_id)
            .contains("speechAct")
    );
}

#[test]
fn unadmitted_chat_preserves_proposal_looking_json_and_trailing_prose() {
    let harness = Harness::new("unadmitted chat");
    let member = harness
        .store
        .add_member(&harness.conversation_id, agent(), MembershipAccess::Member)
        .unwrap();
    let scope = harness
        .store
        .prepare_runtime_dispatch(
            "agent:local",
            "",
            "ordinary question",
            Some(&harness.conversation_id),
            Some(&member.id),
            None,
            None,
        )
        .unwrap();
    let exact = r#"{"speechAct":"question","envelope":{"conversationId":"conversation:one"},"commitmentProposals":[]} trailing prose stays."#;
    harness
        .store
        .append_runtime_frame(
            &scope,
            1,
            &serde_json::json!({
                "event": "agent.message.chunk",
                "payload": {"text": exact},
            }),
        )
        .unwrap();
    let state = harness
        .store
        .finish_runtime_dispatch(
            &scope,
            &serde_json::json!({"output": exact}),
            DispatchState::Completed,
            None,
        )
        .unwrap();
    assert_eq!(state, DispatchState::Completed);
    assert_eq!(
        visible_text(&harness.store, &harness.conversation_id, &scope.event_id),
        exact
    );
}

#[test]
fn admitted_untyped_json_is_raw_completed_reply() {
    let harness = Harness::new("untyped json");
    let member = harness
        .store
        .add_member(&harness.conversation_id, agent(), MembershipAccess::Member)
        .unwrap();
    let scope = harness
        .store
        .prepare_runtime_dispatch(
            "agent:local",
            "",
            "ordinary question",
            Some(&harness.conversation_id),
            Some(&member.id),
            None,
            None,
        )
        .unwrap();
    harness
        .store
        .admit_runtime_response_mode(&scope, TRUSTED_RESPONSE_MODE_ASSISTANT_TURN)
        .unwrap();
    let private_only = serde_json::to_string(&harness.proposal(
        "request:private-only",
        "goal:private",
        "matter:private",
        false,
    ))
    .unwrap();
    let state = harness
        .store
        .finish_runtime_dispatch(
            &scope,
            &serde_json::json!({"output": private_only}),
            DispatchState::Completed,
            None,
        )
        .unwrap();
    assert_eq!(state, DispatchState::Completed);
    assert_eq!(
        visible_text(&harness.store, &harness.conversation_id, &scope.event_id),
        private_only
    );
    let metadata = metadata_joined(&harness.store, &harness.conversation_id, &scope.event_id);
    assert!(metadata.contains("untyped-assistant-reply"));
    let event = harness
        .store
        .event(&harness.conversation_id, &scope.event_id)
        .unwrap()
        .unwrap();
    assert!(
        event
            .parts
            .iter()
            .all(|part| part.kind != EventPartKind::Diagnostic
                || !part.content.contains(ASSISTANT_TURN_INVALID_ERROR)),
        "raw conversation must not be rewritten as an envelope failure"
    );
}

#[test]
fn admitted_empty_output_is_still_failed() {
    let harness = Harness::new("empty envelope");
    let member = harness
        .store
        .add_member(&harness.conversation_id, agent(), MembershipAccess::Member)
        .unwrap();
    let scope = harness
        .store
        .prepare_runtime_dispatch(
            "agent:local",
            "",
            "ordinary question",
            Some(&harness.conversation_id),
            Some(&member.id),
            None,
            None,
        )
        .unwrap();
    harness
        .store
        .admit_runtime_response_mode(&scope, TRUSTED_RESPONSE_MODE_ASSISTANT_TURN)
        .unwrap();
    let state = harness
        .store
        .finish_runtime_dispatch(
            &scope,
            &serde_json::json!({"output": "   "}),
            DispatchState::Completed,
            None,
        )
        .unwrap();
    assert_eq!(state, DispatchState::Failed);
    assert!(visible_text(&harness.store, &harness.conversation_id, &scope.event_id).is_empty());
    let event = harness
        .store
        .event(&harness.conversation_id, &scope.event_id)
        .unwrap()
        .unwrap();
    assert!(event.parts.iter().any(|part| {
        part.kind == EventPartKind::Diagnostic
            && part.content.contains(ASSISTANT_TURN_INVALID_ERROR)
    }));
}

#[test]
fn admitted_plain_prose_is_completed_reply_with_host_abstain() {
    let harness = Harness::new("plain prose");
    let member = harness
        .store
        .add_member(&harness.conversation_id, agent(), MembershipAccess::Member)
        .unwrap();
    let scope = harness
        .store
        .prepare_runtime_dispatch(
            "agent:local",
            "",
            "ordinary question",
            Some(&harness.conversation_id),
            Some(&member.id),
            None,
            None,
        )
        .unwrap();
    harness
        .store
        .admit_runtime_response_mode(&scope, TRUSTED_RESPONSE_MODE_ASSISTANT_TURN)
        .unwrap();
    let reply = "这是普通中文回复，不是信封 JSON。";
    harness
        .store
        .append_runtime_frame(
            &scope,
            1,
            &serde_json::json!({
                "event": "agent.message.chunk",
                "payload": {"text": reply},
            }),
        )
        .unwrap();
    assert!(
        visible_text(&harness.store, &harness.conversation_id, &scope.event_id).is_empty(),
        "admitted prose chunks must stay unpublished until terminal assembly"
    );
    let state = harness
        .store
        .finish_runtime_dispatch(
            &scope,
            &serde_json::json!({"output": reply}),
            DispatchState::Completed,
            None,
        )
        .unwrap();
    assert_eq!(state, DispatchState::Completed);
    assert_eq!(
        visible_text(&harness.store, &harness.conversation_id, &scope.event_id),
        reply
    );
    let metadata = metadata_joined(&harness.store, &harness.conversation_id, &scope.event_id);
    assert!(metadata.contains("trustedResponseMode"));
    assert!(metadata.contains("untyped-assistant-reply"));
    assert!(
        !visible_text(&harness.store, &harness.conversation_id, &scope.event_id)
            .contains("untyped-assistant-reply")
    );
}

#[test]
fn pending_completion_notices_filter_before_limit_and_ack_is_idempotent() {
    let harness = Harness::new("pending-index");
    harness
        .store
        .commit(&harness.proposal(
            "request:authorized",
            "goal:authorized",
            "matter:authorized",
            true,
        ))
        .unwrap();
    harness
        .store
        .with_continuity_unit_of_work(|unit| {
            for index in 0..50 {
                unit.execute(
                    "INSERT INTO continuity_completion_transitions(
                       notification_id, goal_id, transition, consumed, created_at
                     ) VALUES (?1, ?2, '{}', 0, ?3)",
                    rusqlite::params![
                        format!("notice:foreign:{index}"),
                        format!("goal:foreign:{index}"),
                        index as i64
                    ],
                )?;
            }
            unit.request_commit();
            Ok(())
        })
        .unwrap();
    complete_goal(&harness, "goal:authorized");

    let pending = list_pending_completion_notices(
        &harness.store,
        &harness.conversation_id,
        &harness.owner_membership_id,
    )
    .unwrap();
    assert_eq!(
        pending.len(),
        1,
        "qualified pending must be selected before LIMIT so 50 foreign rows cannot starve it"
    );
    assert_eq!(pending[0].notification_id, "notice:goal:authorized");
    assert_eq!(pending[0].goal_id, "goal:authorized");

    let agent = harness
        .store
        .add_member(&harness.conversation_id, agent(), MembershipAccess::Member)
        .unwrap();
    let denied =
        list_pending_completion_notices(&harness.store, &harness.conversation_id, &agent.id);
    assert_eq!(denied.unwrap_err().code, ContinuityFailureCode::ScopeDenied);

    let first = ack_completion_notices(
        &harness.store,
        &harness.conversation_id,
        &harness.owner_membership_id,
        &["notice:goal:authorized".into(), "notice:foreign:0".into()],
    )
    .unwrap();
    assert_eq!(first, vec!["notice:goal:authorized".to_string()]);
    let repeat = ack_completion_notices(
        &harness.store,
        &harness.conversation_id,
        &harness.owner_membership_id,
        &["notice:goal:authorized".into()],
    )
    .unwrap();
    assert_eq!(repeat, vec!["notice:goal:authorized".to_string()]);
    assert!(
        list_pending_completion_notices(
            &harness.store,
            &harness.conversation_id,
            &harness.owner_membership_id,
        )
        .unwrap()
        .is_empty()
    );
    let resolved = resolve_completion_notice(
        &harness.store,
        &harness.conversation_id,
        &harness.owner_membership_id,
        "notice:goal:authorized",
    )
    .unwrap();
    assert_eq!(resolved.parent_conversation_id, harness.conversation_id);
    assert_eq!(resolved.card_event_id.is_empty(), false);
}

fn designate_agent(harness: &mut Harness) -> String {
    let agent = harness
        .store
        .add_member(&harness.conversation_id, agent(), MembershipAccess::Member)
        .unwrap();
    harness.refresh();
    harness
        .store
        .set_conversation_assistant(
            &harness.conversation_id,
            &harness.owner_membership_id,
            harness.revision,
            Some(&agent.id),
        )
        .unwrap();
    harness.refresh();
    agent.id
}

#[test]
fn user_posted_child_admission_issues_exact_current_input_grants() {
    let mut harness = Harness::new("grant-current-input");
    let parent_assistant = designate_agent(&mut harness);
    let proposal = harness.proposal(
        "request:grant-current",
        "goal:grant-current",
        "matter:grant-current",
        true,
    );
    commit_user_posted_proposal(
        &harness.store,
        &proposal,
        &harness.event_id,
        &parent_assistant,
    )
    .unwrap();
    let relation = harness
        .store
        .relation_for_goal("goal:grant-current")
        .unwrap();
    let child = harness.store.get(&relation.child_conversation_id).unwrap();
    let child_member = child.assistant_membership_id.clone().unwrap();
    assert_ne!(child_member, parent_assistant);
    let grants = list_all_parent_grants(&harness.store).unwrap();
    assert_eq!(grants.len(), 1);
    let grant = &grants[0];
    assert_eq!(
        grant.recipient_conversation_id,
        relation.child_conversation_id
    );
    assert_eq!(grant.recipient_membership_id, child_member);
    assert_eq!(grant.status, ContinuityParentGrantStatus::Admitted);
    assert_eq!(grant.source_refs.len(), 1);
    let source = &grant.source_refs[0];
    let event = harness
        .store
        .event(&harness.conversation_id, &harness.event_id)
        .unwrap()
        .unwrap();
    assert_eq!(source.opaque_id, harness.event_id);
    assert_eq!(source.part_id.as_deref(), Some(event.parts[0].id.as_str()));
    assert_eq!(
        source.digest,
        format!("event:{}:{}", event.id, event.parts[0].id)
    );
    assert_eq!(
        harness
            .store
            .posted_event_part_text(
                &harness.conversation_id,
                &harness.event_id,
                source.part_id.as_deref().unwrap()
            )
            .unwrap(),
        "delegate notes"
    );
    assert_eq!(list_all_parent_grants(&harness.store).unwrap().len(), 1);
    harness.refresh();
    let later = harness
        .store
        .append_event(
            &harness.conversation_id,
            Some(&harness.owner_membership_id),
            EventKind::Message,
            &[NewEventPart {
                id: String::new(),
                kind: EventPartKind::Text,
                content: "later chitchat must not grant".into(),
            }],
            None,
            None,
            true,
        )
        .unwrap();
    harness.refresh();
    let later_proposal = harness.proposal(
        "request:grant-later",
        "goal:grant-current",
        "matter:grant-current",
        true,
    );
    commit_user_posted_proposal(
        &harness.store,
        &later_proposal,
        &later.id,
        &parent_assistant,
    )
    .unwrap();
    let grants = list_all_parent_grants(&harness.store).unwrap();
    assert_eq!(grants.len(), 1);
    assert!(!grants.iter().any(|grant| {
        grant
            .source_refs
            .iter()
            .any(|source| source.opaque_id == later.id)
    }));
}

#[test]
fn accepted_canonical_evidence_issues_exact_part_grant() {
    let mut harness = Harness::new("grant-evidence");
    let parent_assistant = designate_agent(&mut harness);
    let proposal = harness.proposal(
        "request:grant-evidence",
        "goal:grant-evidence",
        "matter:grant-evidence",
        true,
    );
    commit_user_posted_proposal(
        &harness.store,
        &proposal,
        &harness.event_id,
        &parent_assistant,
    )
    .unwrap();
    let event = harness
        .store
        .append_event(
            &harness.conversation_id,
            Some(&harness.owner_membership_id),
            EventKind::Message,
            &[
                NewEventPart {
                    id: String::new(),
                    kind: EventPartKind::Text,
                    content: "EVIDENCE-PART-A".into(),
                },
                NewEventPart {
                    id: String::new(),
                    kind: EventPartKind::Text,
                    content: "EVIDENCE-PART-B".into(),
                },
            ],
            None,
            None,
            true,
        )
        .unwrap();
    let granted_part = event.parts[0].id.clone();
    let sibling_part = event.parts[1].id.clone();
    append_criterion_evidence(
        &harness.store,
        &harness.conversation_id,
        "goal:grant-evidence",
        ContinuityEvidenceRef {
            source: ContinuitySourceRef {
                owner_kind: ContinuitySourceOwnerKind::Event,
                opaque_id: event.id.clone(),
                part_id: Some(granted_part.clone()),
                span: None,
                source_revision: event.sequence,
                digest: format!("event:{}:{granted_part}", event.id),
                visibility_scope: ContinuityVisibilityScope::Goal,
                validity: ContinuitySourceValidity::Current,
            },
            issuer: harness.owner_membership_id.clone(),
            subject_version: 1,
            criterion_id: "criterion:notes".into(),
            observed_at: 1,
            result: ContinuityEvidenceResult::Pass,
            verification_kind: ContinuityVerificationKind::UserAcceptance,
            scope: ContinuityVisibilityScope::Goal,
            validity: ContinuitySourceValidity::Current,
        },
    )
    .unwrap();
    let grants = list_all_parent_grants(&harness.store).unwrap();
    let evidence_grants: Vec<_> = grants
        .iter()
        .filter(|grant| {
            grant
                .source_refs
                .iter()
                .any(|source| source.opaque_id == event.id)
        })
        .collect();
    assert_eq!(evidence_grants.len(), 1);
    assert_eq!(
        evidence_grants[0].source_refs[0].part_id.as_deref(),
        Some(granted_part.as_str())
    );
    assert!(!evidence_grants.iter().any(|grant| {
        grant
            .source_refs
            .iter()
            .any(|source| source.part_id.as_deref() == Some(sibling_part.as_str()))
    }));
    assert_eq!(
        harness
            .store
            .posted_event_part_text(&harness.conversation_id, &event.id, &granted_part)
            .unwrap(),
        "EVIDENCE-PART-A"
    );
}

fn evidence_source(
    event: &licoup_conversation::ConversationEvent,
    part_id: &str,
    span: Option<ContinuityUtf8ByteSpan>,
    digest: String,
    revision: i64,
) -> ContinuitySourceRef {
    ContinuitySourceRef {
        owner_kind: ContinuitySourceOwnerKind::Event,
        opaque_id: event.id.clone(),
        part_id: Some(part_id.to_owned()),
        span,
        source_revision: revision,
        digest,
        visibility_scope: ContinuityVisibilityScope::Goal,
        validity: ContinuitySourceValidity::Current,
    }
}

fn accept_evidence(
    harness: &Harness,
    goal_id: &str,
    source: ContinuitySourceRef,
    criterion_id: &str,
    version: i64,
) {
    append_criterion_evidence(
        &harness.store,
        &harness.conversation_id,
        goal_id,
        ContinuityEvidenceRef {
            source,
            issuer: harness.owner_membership_id.clone(),
            subject_version: version,
            criterion_id: criterion_id.into(),
            observed_at: version,
            result: ContinuityEvidenceResult::Pass,
            verification_kind: ContinuityVerificationKind::UserAcceptance,
            scope: ContinuityVisibilityScope::Goal,
            validity: ContinuitySourceValidity::Current,
        },
    )
    .unwrap();
}

#[test]
fn evidence_grant_preserves_exact_span_digest_and_owner_kind() {
    let mut harness = Harness::new("grant-span");
    let parent_assistant = designate_agent(&mut harness);
    let proposal = harness.proposal(
        "request:grant-span",
        "goal:grant-span",
        "matter:grant-span",
        true,
    );
    commit_user_posted_proposal(
        &harness.store,
        &proposal,
        &harness.event_id,
        &parent_assistant,
    )
    .unwrap();
    let event = harness
        .store
        .append_event(
            &harness.conversation_id,
            Some(&harness.owner_membership_id),
            EventKind::Message,
            &[NewEventPart {
                id: String::new(),
                kind: EventPartKind::Text,
                content: "KEEP-SPAN|OUT-OF-SPAN-SECRET".into(),
            }],
            None,
            None,
            true,
        )
        .unwrap();
    let part_id = event.parts[0].id.clone();
    let admitted = evidence_source(
        &event,
        &part_id,
        Some(ContinuityUtf8ByteSpan {
            start_byte: 0,
            end_byte: 9,
        }),
        format!("event:{}:{part_id}", event.id),
        event.sequence,
    );
    accept_evidence(
        &harness,
        "goal:grant-span",
        admitted.clone(),
        "criterion:notes",
        1,
    );
    let grants = list_all_parent_grants(&harness.store).unwrap();
    let evidence = grants
        .iter()
        .find(|grant| {
            grant
                .source_refs
                .iter()
                .any(|source| source.opaque_id == event.id)
        })
        .expect("exact evidence grant");
    assert_eq!(evidence.source_refs[0], admitted);
}

#[test]
fn invalid_evidence_refs_do_not_issue_grants() {
    let mut harness = Harness::new("grant-invalid");
    let parent_assistant = designate_agent(&mut harness);
    let proposal = harness.proposal(
        "request:grant-invalid",
        "goal:grant-invalid",
        "matter:grant-invalid",
        true,
    );
    commit_user_posted_proposal(
        &harness.store,
        &proposal,
        &harness.event_id,
        &parent_assistant,
    )
    .unwrap();
    let before = list_all_parent_grants(&harness.store).unwrap().len();
    let event = harness
        .store
        .append_event(
            &harness.conversation_id,
            Some(&harness.owner_membership_id),
            EventKind::Message,
            &[NewEventPart {
                id: String::new(),
                kind: EventPartKind::Text,
                content: "VALID-TEXT".into(),
            }],
            None,
            None,
            true,
        )
        .unwrap();
    let part_id = event.parts[0].id.clone();
    let cases = [
        evidence_source(
            &event,
            &part_id,
            None,
            "digest:wrong".into(),
            event.sequence,
        ),
        evidence_source(
            &event,
            &part_id,
            None,
            format!("event:{}:{part_id}", event.id),
            event.sequence + 7,
        ),
        evidence_source(
            &event,
            "part:missing",
            None,
            format!("event:{}:part:missing", event.id),
            event.sequence,
        ),
        evidence_source(
            &event,
            &part_id,
            Some(ContinuityUtf8ByteSpan {
                start_byte: 0,
                end_byte: 64,
            }),
            format!("event:{}:{part_id}", event.id),
            event.sequence,
        ),
    ];
    for (index, source) in cases.into_iter().enumerate() {
        accept_evidence(
            &harness,
            "goal:grant-invalid",
            source,
            &format!("criterion:invalid-{index}"),
            1,
        );
    }
    let grants = list_all_parent_grants(&harness.store).unwrap();
    assert_eq!(grants.len(), before);
    assert!(!grants.iter().any(|grant| {
        grant
            .source_refs
            .iter()
            .any(|source| source.opaque_id == event.id)
    }));
}

#[test]
fn superseded_evidence_revokes_every_matching_old_grant_and_keeps_independent_b() {
    let mut harness = Harness::new("grant-supersede");
    let parent_assistant = designate_agent(&mut harness);
    let proposal = harness.proposal(
        "request:grant-supersede",
        "goal:grant-supersede",
        "matter:grant-supersede",
        true,
    );
    commit_user_posted_proposal(
        &harness.store,
        &proposal,
        &harness.event_id,
        &parent_assistant,
    )
    .unwrap();
    let relation = harness
        .store
        .relation_for_goal("goal:grant-supersede")
        .unwrap();
    let child = harness.store.get(&relation.child_conversation_id).unwrap();
    let child_member = child.assistant_membership_id.clone().unwrap();
    let event_a = harness
        .store
        .append_event(
            &harness.conversation_id,
            Some(&harness.owner_membership_id),
            EventKind::Message,
            &[NewEventPart {
                id: String::new(),
                kind: EventPartKind::Text,
                content: "A-V1".into(),
            }],
            None,
            None,
            true,
        )
        .unwrap();
    let event_b = harness
        .store
        .append_event(
            &harness.conversation_id,
            Some(&harness.owner_membership_id),
            EventKind::Message,
            &[NewEventPart {
                id: String::new(),
                kind: EventPartKind::Text,
                content: "B-V1".into(),
            }],
            None,
            None,
            true,
        )
        .unwrap();
    let part_a = event_a.parts[0].id.clone();
    let part_b = event_b.parts[0].id.clone();
    let source_a = evidence_source(
        &event_a,
        &part_a,
        None,
        format!("event:{}:{part_a}", event_a.id),
        event_a.sequence,
    );
    let source_b = evidence_source(
        &event_b,
        &part_b,
        None,
        format!("event:{}:{part_b}", event_b.id),
        event_b.sequence,
    );
    accept_evidence(
        &harness,
        "goal:grant-supersede",
        source_a.clone(),
        "criterion:material-a",
        1,
    );
    accept_evidence(
        &harness,
        "goal:grant-supersede",
        source_b.clone(),
        "criterion:material-b",
        1,
    );
    for index in 0..PENDING_OBLIGATION_PAGE_SIZE {
        put_grant(
            &harness.store,
            &ContinuityParentContextGrant {
                grant_id: format!("grant:{index:02}-pad-old-a"),
                source_conversation_id: harness.conversation_id.clone(),
                recipient_conversation_id: relation.child_conversation_id.clone(),
                recipient_membership_id: child_member.clone(),
                source_refs: vec![source_a.clone()],
                authorized_scopes: vec![ContinuityVisibilityScope::Goal],
                status: ContinuityParentGrantStatus::Admitted,
                request_id: format!("request:pad-old-a-{index}"),
                revocation_generation: 0,
            },
        )
        .unwrap();
    }
    put_grant(
        &harness.store,
        &ContinuityParentContextGrant {
            grant_id: "grant:zz-old-a".into(),
            source_conversation_id: harness.conversation_id.clone(),
            recipient_conversation_id: relation.child_conversation_id.clone(),
            recipient_membership_id: child_member.clone(),
            source_refs: vec![source_a.clone()],
            authorized_scopes: vec![ContinuityVisibilityScope::Goal],
            status: ContinuityParentGrantStatus::Admitted,
            request_id: "request:zz-old-a".into(),
            revocation_generation: 0,
        },
    )
    .unwrap();
    let event_a2 = harness
        .store
        .append_event(
            &harness.conversation_id,
            Some(&harness.owner_membership_id),
            EventKind::Message,
            &[NewEventPart {
                id: String::new(),
                kind: EventPartKind::Text,
                content: "A-V2".into(),
            }],
            None,
            None,
            true,
        )
        .unwrap();
    let part_a2 = event_a2.parts[0].id.clone();
    accept_evidence(
        &harness,
        "goal:grant-supersede",
        evidence_source(
            &event_a2,
            &part_a2,
            None,
            format!("event:{}:{part_a2}", event_a2.id),
            event_a2.sequence,
        ),
        "criterion:material-a",
        2,
    );
    let grants = list_all_parent_grants(&harness.store).unwrap();
    let old_a_admitted = grants.iter().filter(|grant| {
        grant.status == ContinuityParentGrantStatus::Admitted
            && grant
                .source_refs
                .iter()
                .any(|source| source.opaque_id == event_a.id)
    });
    assert_eq!(old_a_admitted.count(), 0);
    assert!(grants.iter().any(|grant| {
        grant.grant_id == "grant:zz-old-a" && grant.status == ContinuityParentGrantStatus::Revoked
    }));
    assert!(grants.iter().any(|grant| {
        grant.status == ContinuityParentGrantStatus::Admitted
            && grant
                .source_refs
                .iter()
                .any(|source| source.opaque_id == event_b.id)
    }));
    assert!(grants.iter().any(|grant| {
        grant.status == ContinuityParentGrantStatus::Admitted
            && grant
                .source_refs
                .iter()
                .any(|source| source.opaque_id == event_a2.id)
    }));
}

#[test]
fn accepted_v2_then_v1_close_rejects_without_rollback_or_notice() {
    let mut harness = Harness::new("stale-v1-close");
    commit_required_criterion_goal(
        &mut harness,
        "request:versioned",
        "goal:versioned",
        "matter:versioned",
        "criterion:delivery",
        true,
    );
    let v1 = post_owner_text(&harness, "artifact v1");
    let v1_source = evidence_source(
        &v1,
        &v1.parts[0].id,
        None,
        format!("event:{}:{}", v1.id, v1.parts[0].id),
        v1.sequence,
    );
    accept_evidence(
        &harness,
        "goal:versioned",
        v1_source.clone(),
        "criterion:delivery",
        1,
    );
    let v2 = post_owner_text(&harness, "artifact v2 current");
    accept_evidence(
        &harness,
        "goal:versioned",
        evidence_source(
            &v2,
            &v2.parts[0].id,
            None,
            format!("event:{}:{}", v2.id, v2.parts[0].id),
            v2.sequence,
        ),
        "criterion:delivery",
        2,
    );
    let relation_before = harness.store.relation_for_goal("goal:versioned").unwrap();
    let current = read_goal(&harness.store, "goal:versioned")
        .unwrap()
        .unwrap();
    let mut progress = current.clone();
    progress.lifecycle = ContinuityGoalLifecycle::Achieved;
    progress.next_attention = None;
    progress
        .criterion_evidence_refs
        .retain(|item| item.criterion_id == "criterion:delivery" && item.subject_version == 1);
    let transition = ContinuityGoalCompletionTransition {
        transition_id: "transition:stale-v1".into(),
        goal_id: "goal:versioned".into(),
        from_lifecycle: current.lifecycle,
        to_lifecycle: ContinuityGoalLifecycle::Achieved,
        goal_revision: progress.revision,
        authority_kind: ContinuityClosureAuthorityKind::UserAcceptance,
        evaluation_ref: v1_source,
        notification_id: "notice:stale-v1".into(),
    };
    assert_eq!(
        accept_completion(
            &harness.store,
            &harness.conversation_id,
            &transition,
            &progress,
        )
        .unwrap_err()
        .code,
        ContinuityFailureCode::PrematureClosure
    );
    let stored = read_goal(&harness.store, "goal:versioned")
        .unwrap()
        .unwrap();
    assert_eq!(stored.lifecycle, ContinuityGoalLifecycle::Active);
    assert_eq!(
        stored_subject_version(&harness.store, "goal:versioned", "criterion:delivery"),
        2
    );
    assert_eq!(stored.revision, current.revision);
    assert_eq!(stored.criterion_evidence_refs.len(), 2);
    assert!(!notice_recorded(&harness.store, "notice:stale-v1"));
    let relation_after = harness.store.relation_for_goal("goal:versioned").unwrap();
    assert_eq!(
        relation_after.card_anchor.event_id,
        relation_before.card_anchor.event_id
    );
    assert_eq!(
        relation_after.card_anchor.sequence,
        relation_before.card_anchor.sequence
    );
    assert!(relation_after.completion_transition.is_none());

    let current = read_goal(&harness.store, "goal:versioned")
        .unwrap()
        .unwrap();
    let (foreign_progress, foreign_transition) = achieved_close(
        "goal:versioned",
        &current,
        ContinuitySourceRef {
            owner_kind: ContinuitySourceOwnerKind::Event,
            opaque_id: "event:foreign".into(),
            part_id: None,
            span: None,
            source_revision: 1,
            digest: "event:foreign".into(),
            visibility_scope: ContinuityVisibilityScope::Goal,
            validity: ContinuitySourceValidity::Current,
        },
        "notice:foreign",
        ContinuityClosureAuthorityKind::UserAcceptance,
    );
    assert_eq!(
        accept_completion(
            &harness.store,
            &harness.conversation_id,
            &foreign_transition,
            &foreign_progress,
        )
        .unwrap_err()
        .code,
        ContinuityFailureCode::PrematureClosure
    );
    assert_eq!(
        stored_subject_version(&harness.store, "goal:versioned", "criterion:delivery"),
        2
    );
    assert!(!notice_recorded(&harness.store, "notice:foreign"));
}

#[test]
fn missing_failed_revoked_current_evidence_rejects_achieved() {
    let mut harness = Harness::new("required-evidence");
    commit_required_criterion_goal(
        &mut harness,
        "request:missing",
        "goal:missing",
        "matter:missing",
        "criterion:delivery",
        false,
    );
    let missing = read_goal(&harness.store, "goal:missing").unwrap().unwrap();
    let (progress, transition) = achieved_close(
        "goal:missing",
        &missing,
        goal_evaluation_ref("goal:missing", missing.revision),
        "notice:missing",
        ContinuityClosureAuthorityKind::UserAcceptance,
    );
    assert_eq!(
        accept_completion(
            &harness.store,
            &harness.conversation_id,
            &transition,
            &progress,
        )
        .unwrap_err()
        .code,
        ContinuityFailureCode::PrematureClosure
    );
    assert_eq!(
        read_goal(&harness.store, "goal:missing")
            .unwrap()
            .unwrap()
            .lifecycle,
        ContinuityGoalLifecycle::Active
    );
    assert!(!notice_recorded(&harness.store, "notice:missing"));

    let failed_event = post_owner_text(&harness, "failed artifact");
    append_criterion_evidence(
        &harness.store,
        &harness.conversation_id,
        "goal:missing",
        ContinuityEvidenceRef {
            source: evidence_source(
                &failed_event,
                &failed_event.parts[0].id,
                None,
                format!("event:{}:{}", failed_event.id, failed_event.parts[0].id),
                failed_event.sequence,
            ),
            issuer: harness.owner_membership_id.clone(),
            subject_version: 1,
            criterion_id: "criterion:delivery".into(),
            observed_at: 1,
            result: ContinuityEvidenceResult::Fail,
            verification_kind: ContinuityVerificationKind::UserAcceptance,
            scope: ContinuityVisibilityScope::Goal,
            validity: ContinuitySourceValidity::Current,
        },
    )
    .unwrap();
    let failed = read_goal(&harness.store, "goal:missing").unwrap().unwrap();
    let (progress, transition) = achieved_close(
        "goal:missing",
        &failed,
        goal_evaluation_ref("goal:missing", failed.revision),
        "notice:failed",
        ContinuityClosureAuthorityKind::UserAcceptance,
    );
    assert_eq!(
        accept_completion(
            &harness.store,
            &harness.conversation_id,
            &transition,
            &progress,
        )
        .unwrap_err()
        .code,
        ContinuityFailureCode::PrematureClosure
    );
    assert_eq!(
        read_goal(&harness.store, "goal:missing")
            .unwrap()
            .unwrap()
            .lifecycle,
        ContinuityGoalLifecycle::Active
    );
    assert!(!notice_recorded(&harness.store, "notice:failed"));

    harness.refresh();
    commit_required_criterion_goal(
        &mut harness,
        "request:revoked",
        "goal:revoked",
        "matter:revoked",
        "criterion:delivery",
        false,
    );
    let revoked_event = post_owner_text(&harness, "current then revoked");
    accept_evidence(
        &harness,
        "goal:revoked",
        evidence_source(
            &revoked_event,
            &revoked_event.parts[0].id,
            None,
            format!("event:{}:{}", revoked_event.id, revoked_event.parts[0].id),
            revoked_event.sequence,
        ),
        "criterion:delivery",
        1,
    );
    revoke_source(
        &harness.store,
        &harness.conversation_id,
        &revoked_event.id,
        true,
    )
    .unwrap();
    let revoked = read_goal(&harness.store, "goal:revoked").unwrap().unwrap();
    let (progress, transition) = achieved_close(
        "goal:revoked",
        &revoked,
        goal_evaluation_ref("goal:revoked", revoked.revision),
        "notice:revoked",
        ContinuityClosureAuthorityKind::UserAcceptance,
    );
    assert_eq!(
        accept_completion(
            &harness.store,
            &harness.conversation_id,
            &transition,
            &progress,
        )
        .unwrap_err()
        .code,
        ContinuityFailureCode::SourceRevoked
    );
    assert_eq!(
        read_goal(&harness.store, "goal:revoked")
            .unwrap()
            .unwrap()
            .lifecycle,
        ContinuityGoalLifecycle::Active
    );
    assert!(!notice_recorded(&harness.store, "notice:revoked"));

    let cancel = read_goal(&harness.store, "goal:missing").unwrap().unwrap();
    let mut cancel_progress = cancel.clone();
    cancel_progress.lifecycle = ContinuityGoalLifecycle::Cancelled;
    cancel_progress.next_attention = None;
    cancel_progress.active_execution_refs.clear();
    let cancel_transition = ContinuityGoalCompletionTransition {
        transition_id: "transition:cancel-missing".into(),
        goal_id: "goal:missing".into(),
        from_lifecycle: cancel.lifecycle,
        to_lifecycle: ContinuityGoalLifecycle::Cancelled,
        goal_revision: cancel.revision,
        authority_kind: ContinuityClosureAuthorityKind::UserAcceptance,
        evaluation_ref: goal_evaluation_ref("goal:missing", cancel.revision),
        notification_id: "notice:cancel-missing".into(),
    };
    assert!(
        accept_completion(
            &harness.store,
            &harness.conversation_id,
            &cancel_transition,
            &cancel_progress,
        )
        .unwrap()
    );
    let cancelled = read_goal(&harness.store, "goal:missing").unwrap().unwrap();
    assert_eq!(cancelled.lifecycle, ContinuityGoalLifecycle::Cancelled);
    assert_eq!(
        cancelled.criterion_evidence_refs.len(),
        cancel.criterion_evidence_refs.len()
    );
}

#[test]
fn valid_current_closure_once_retains_evidence_and_card_anchor() {
    let mut harness = Harness::new("valid-current-close");
    commit_required_criterion_goal(
        &mut harness,
        "request:current",
        "goal:current",
        "matter:current",
        "criterion:delivery",
        true,
    );
    let v1 = post_owner_text(&harness, "artifact v1");
    accept_evidence(
        &harness,
        "goal:current",
        evidence_source(
            &v1,
            &v1.parts[0].id,
            None,
            format!("event:{}:{}", v1.id, v1.parts[0].id),
            v1.sequence,
        ),
        "criterion:delivery",
        1,
    );
    let v2 = post_owner_text(&harness, "artifact v2 current");
    let v2_source = evidence_source(
        &v2,
        &v2.parts[0].id,
        None,
        format!("event:{}:{}", v2.id, v2.parts[0].id),
        v2.sequence,
    );
    accept_evidence(
        &harness,
        "goal:current",
        v2_source.clone(),
        "criterion:delivery",
        2,
    );
    let relation_before = harness.store.relation_for_goal("goal:current").unwrap();
    let current = read_goal(&harness.store, "goal:current").unwrap().unwrap();
    let evidence_before = current.criterion_evidence_refs.clone();
    let (progress, transition) = achieved_close(
        "goal:current",
        &current,
        v2_source,
        "notice:current",
        ContinuityClosureAuthorityKind::UserAcceptance,
    );
    assert!(
        accept_completion(
            &harness.store,
            &harness.conversation_id,
            &transition,
            &progress,
        )
        .unwrap()
    );
    assert!(
        !accept_completion(
            &harness.store,
            &harness.conversation_id,
            &transition,
            &progress,
        )
        .unwrap()
    );
    let stored = read_goal(&harness.store, "goal:current").unwrap().unwrap();
    assert_eq!(stored.lifecycle, ContinuityGoalLifecycle::Achieved);
    assert_eq!(stored.criterion_evidence_refs, evidence_before);
    assert_eq!(
        stored_subject_version(&harness.store, "goal:current", "criterion:delivery"),
        2
    );
    assert!(notice_recorded(&harness.store, "notice:current"));
    assert_eq!(
        list_completion_notification_ids(&harness.store)
            .unwrap()
            .iter()
            .filter(|item| *item == "notice:current")
            .count(),
        1
    );
    let relation_after = harness.store.relation_for_goal("goal:current").unwrap();
    assert_eq!(
        relation_after.card_anchor.event_id,
        relation_before.card_anchor.event_id
    );
    assert_eq!(
        relation_after.card_anchor.sequence,
        relation_before.card_anchor.sequence
    );
    assert_eq!(
        relation_after.card_anchor.part_id,
        relation_before.card_anchor.part_id
    );
}

#[test]
fn stale_revision_or_from_state_rejects_without_mutation() {
    let harness = Harness::new("stale-transition");
    harness
        .store
        .commit(&harness.proposal("request:stale", "goal:stale", "matter:stale", true))
        .unwrap();
    let current = read_goal(&harness.store, "goal:stale").unwrap().unwrap();
    let relation_before = harness.store.relation_for_goal("goal:stale").unwrap();
    let mut stale_progress = current.clone();
    stale_progress.lifecycle = ContinuityGoalLifecycle::Achieved;
    stale_progress.next_attention = None;
    stale_progress.revision = current.revision.saturating_add(4);
    let stale_transition = ContinuityGoalCompletionTransition {
        transition_id: "transition:stale-revision".into(),
        goal_id: "goal:stale".into(),
        from_lifecycle: current.lifecycle,
        to_lifecycle: ContinuityGoalLifecycle::Achieved,
        goal_revision: stale_progress.revision,
        authority_kind: ContinuityClosureAuthorityKind::GoalEvaluation,
        evaluation_ref: goal_evaluation_ref("goal:stale", stale_progress.revision),
        notification_id: "notice:stale-revision".into(),
    };
    assert_eq!(
        accept_completion(
            &harness.store,
            &harness.conversation_id,
            &stale_transition,
            &stale_progress,
        )
        .unwrap_err()
        .code,
        ContinuityFailureCode::StaleRevision
    );

    let mut from_state = current.clone();
    from_state.lifecycle = ContinuityGoalLifecycle::Achieved;
    from_state.next_attention = None;
    let from_transition = ContinuityGoalCompletionTransition {
        transition_id: "transition:stale-from".into(),
        goal_id: "goal:stale".into(),
        from_lifecycle: ContinuityGoalLifecycle::Waiting,
        to_lifecycle: ContinuityGoalLifecycle::Achieved,
        goal_revision: current.revision,
        authority_kind: ContinuityClosureAuthorityKind::GoalEvaluation,
        evaluation_ref: goal_evaluation_ref("goal:stale", current.revision),
        notification_id: "notice:stale-from".into(),
    };
    assert_eq!(
        accept_completion(
            &harness.store,
            &harness.conversation_id,
            &from_transition,
            &from_state,
        )
        .unwrap_err()
        .code,
        ContinuityFailureCode::PrematureClosure
    );

    let stored = read_goal(&harness.store, "goal:stale").unwrap().unwrap();
    assert_eq!(stored.lifecycle, ContinuityGoalLifecycle::Active);
    assert_eq!(stored.revision, current.revision);
    assert!(!notice_recorded(&harness.store, "notice:stale-revision"));
    assert!(!notice_recorded(&harness.store, "notice:stale-from"));
    let relation_after = harness.store.relation_for_goal("goal:stale").unwrap();
    assert_eq!(
        relation_after.card_anchor.event_id,
        relation_before.card_anchor.event_id
    );
    assert_eq!(
        relation_after.card_anchor.sequence,
        relation_before.card_anchor.sequence
    );
    assert!(relation_after.completion_transition.is_none());
}

#[test]
fn follow_up_wake_dedup_preserves_distinct_events() {
    let mut harness = Harness::new("wake-coalesce");
    let proposal = harness.proposal("request:wake", "goal:wake-test", "matter:wake-test", false);
    harness.store.commit(&proposal).unwrap();
    harness.refresh();

    // Consume any initial wake from the goal creation
    let _ = consume_logical_wake(&harness.store, "wake:goal:wake-test:1");

    // 1. Invalid wake is rejected by admission
    let invalid_wake = ContinuityWake {
        logical_wake_id: "".into(),
        goal_id: "goal:wake-test".into(),
        cause_refs: vec![],
        due_at: None,
        review_policy: "standard".into(),
        goal_revision: 1,
        epoch: 1,
        host_generation: 1,
        claim: None,
        settlement: None,
    };
    assert_eq!(
        harness.store.enqueue_wake(&invalid_wake).unwrap_err().code,
        ContinuityFailureCode::InvalidRequest
    );

    // 2. Enqueue first valid wake
    let ref1 = source_ref(&harness.event_id, 1);
    let wake1 = ContinuityWake {
        logical_wake_id: "wake:custom:1".into(),
        goal_id: "goal:wake-test".into(),
        cause_refs: vec![ref1.clone()],
        due_at: Some(20_000),
        review_policy: "standard".into(),
        goal_revision: 1,
        epoch: 1,
        host_generation: 1,
        claim: None,
        settlement: None,
    };
    let receipt1 = harness.store.enqueue_wake(&wake1).unwrap();
    assert_eq!(receipt1.conversation_id, harness.conversation_id);
    assert_eq!(
        read_pending_outbox(&harness.store, &harness.conversation_id).unwrap(),
        1
    );

    // 3. Duplicate notification with identical logical_wake_id does not create a second delivery
    let receipt_dup = harness.store.enqueue_wake(&wake1).unwrap();
    assert_eq!(receipt_dup.conversation_id, harness.conversation_id);
    assert_eq!(
        read_pending_outbox(&harness.store, &harness.conversation_id).unwrap(),
        1
    );

    // 4. A second wake for the same goal with a different cause_ref is an
    // original event: it is preserved as its own pending wake, never merged
    // away at enqueue. Coalescing into one logical advancement happens at
    // consumption (host drain), not by dropping events from the outbox.
    let ref2 = source_ref("event:another:2", 2);
    let wake2 = ContinuityWake {
        logical_wake_id: "wake:custom:2".into(),
        goal_id: "goal:wake-test".into(),
        cause_refs: vec![ref2.clone()],
        due_at: Some(10_000), // earlier due_at
        review_policy: "standard".into(),
        goal_revision: 1,
        epoch: 1,
        host_generation: 1,
        claim: None,
        settlement: None,
    };
    harness.store.enqueue_wake(&wake2).unwrap();
    assert_eq!(
        read_pending_outbox(&harness.store, &harness.conversation_id).unwrap(),
        2
    );

    // Both original wakes survive with their own identities and cause_refs.
    let pending = read_pending_wakes(&harness.store, &harness.conversation_id).unwrap();
    let pending1 = pending
        .iter()
        .find(|wake| wake.logical_wake_id == "wake:custom:1")
        .expect("first wake preserved");
    assert_eq!(pending1.cause_refs, vec![ref1]);
    assert_eq!(pending1.due_at, Some(20_000));
    let pending2 = pending
        .iter()
        .find(|wake| wake.logical_wake_id == "wake:custom:2")
        .expect("second wake preserved");
    assert_eq!(pending2.cause_refs, vec![ref2]);
    assert_eq!(pending2.due_at, Some(10_000));

    // 5. Consuming each wake succeeds and marks it handed off
    assert!(consume_logical_wake(&harness.store, "wake:custom:1").unwrap());
    assert!(consume_logical_wake(&harness.store, "wake:custom:2").unwrap());
    assert_eq!(
        read_pending_outbox(&harness.store, &harness.conversation_id).unwrap(),
        0
    );
}
