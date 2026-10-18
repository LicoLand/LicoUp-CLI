use super::admission::{
    ContinuityCommitmentAdmission, OWNER_HANDOFF_REVIEW_POLICY,
    admit_achieved_required_current_evidence, admit_commitment_admission,
    admit_completion_against_current, admit_completion_transition, admit_goal_progress,
    admit_idempotency, admit_source_ref, admit_task_child_admission, admit_utf8_span,
    admit_versions, admit_wake, current_required_evidence, retain_current_criterion_evidence,
};
use super::error::{continuity_failure, sql_failure, store_to_continuity};
use super::generated::{
    CONTINUITY_MAX_PAGE_SIZE, ContinuityAgreement, ContinuityAgreementProposal,
    ContinuityCandidateIdentity, ContinuityCommitBasis, ContinuityCommitmentProposal,
    ContinuityEffectClass, ContinuityEvidenceRef, ContinuityFailure, ContinuityFailureCode,
    ContinuityFailureStage, ContinuityFollowThroughKind, ContinuityGoalCompletionTransition,
    ContinuityGoalContract, ContinuityGoalControl, ContinuityGoalLifecycle, ContinuityGoalProgress,
    ContinuityInterpretationProposal, ContinuityMatter, ContinuityMatterStatus,
    ContinuityNextAttention, ContinuityParentContextGrant, ContinuityParentGrantBasis,
    ContinuityParentGrantStatus, ContinuitySourceOwnerKind, ContinuitySourceRef,
    ContinuitySourceValidity, ContinuitySpeechAct, ContinuityTaskChildAdmission,
    ContinuityTaskConversationRelation, ContinuityTaskListingKind, ContinuityVisibilityScope,
    ContinuityWake,
};
use super::hooks::{
    ContinuityEffectStatus, ContinuityInterrupt, continuity_now_ms, take_interrupt,
};
use super::lifecycle::{
    ContinuityGoalEvent, ContinuityGoalState, apply_goal_event, is_terminal, suppresses_new_work,
};
use super::migrate::migrate_or_fail;
pub use super::persist::{
    CHILD_WORK_ACCEPTED_DESIGNATION, CHILD_WORK_INTENT_DESIGNATION, CHILD_WORK_LIVE_DESIGNATION,
    CHILD_WORK_PENDING_DESIGNATION, CHILD_WORK_STARTED_DESIGNATION, ChildWorkIdentity,
    INGRESS_USER_POSTED_DESIGNATION, PENDING_OBLIGATION_PAGE_SIZE, SETTLEMENT_APPLIED_DESIGNATION,
    SETTLEMENT_PENDING_DESIGNATION, child_work_identity_from_payload, child_work_identity_payload,
    child_work_named_key, child_work_operation_id,
};
use super::persist::{
    StoredEvaluationCase, StoredEvaluationCorpus, StoredEvaluationSession, StoredOwnerAuthority,
    admit_local_owner_principal, append_goal_evidence, child_recipient_membership,
    child_work_accepted as child_work_accepted_unit, child_work_started as child_work_started_unit,
    consume_completion_ids, consume_wake, count_cancel_effects as count_cancel_effects_unit,
    current_agreements, default_receipt, delete_child_work_live as delete_child_work_live_unit,
    delete_effect_if_status, due_from_attention, enqueue_wake, ensure_scope,
    evaluation_corpus_version_digest,
    find_admitted_parent_grant as find_admitted_parent_grant_unit, goal_conversation_id,
    goals_for_matter, has_unknown_effects, increment_revocation, insert_agreement, insert_derived,
    list_all_parent_grants as list_all_parent_grants_unit,
    list_all_pending_wakes as list_all_pending_wakes_unit, list_child_links, list_child_relations,
    list_child_work_live as list_child_work_live_unit, list_completion_ids,
    list_due_goals as list_due_goals_unit, list_matters, list_parent_grants, list_pending_wakes,
    list_qualified_completion_rows_for_ids, list_qualified_pending_completion_rows,
    list_unacked_child_work as list_unacked_child_work_unit,
    list_unacked_child_work_page as list_unacked_child_work_page_unit,
    list_unapplied_settlements as list_unapplied_settlements_unit,
    list_unapplied_settlements_page as list_unapplied_settlements_page_unit, live_derived_count,
    load_child_work_intent, load_child_work_live as load_child_work_live_unit, load_completion,
    load_effect, load_evaluation_corpus_in_unit, load_evaluation_session_in_unit, load_goal,
    load_idempotency, load_ingress_execution, load_named_cursor_payload,
    load_oldest_pending_child_work as load_oldest_pending_child_work_unit,
    load_qualification_evidence, load_qualification_evidence_row, load_qualified_completion_row,
    load_relation, load_relation_for_child, load_schema_value, new_continuity_id,
    pending_obligation_scan_evidence as pending_obligation_scan_evidence_unit,
    pending_outbox_count, persist_evaluation_corpus_in_unit, persist_evaluation_session_in_unit,
    persist_qualification_evidence, persist_schema_value, record_effect,
    revoke_admitted_recipient_grants, set_goal_wait_due as set_goal_wait_due_unit,
    settlement_applied as settlement_applied_unit, source_is_revoked, store_completion,
    store_idempotency, update_wake_generation, upsert_association, upsert_goal, upsert_grant,
    upsert_matter, upsert_relation, write_child_work_accepted as write_child_work_accepted_unit,
    write_child_work_intent as write_child_work_intent_unit,
    write_child_work_live as write_child_work_live_unit,
    write_child_work_started as write_child_work_started_unit, write_ingress_execution,
    write_settlement_applied as write_settlement_applied_unit,
    write_settlement_pending as write_settlement_pending_unit, write_source_cursor,
};
use super::ports::{ContinuityCommitReceipt, no_effect_receipt};
use crate::store::{ContinuityUnitOfWork, ConversationStore};
use rusqlite::OptionalExtension;
use serde_json::{Value, json};

enum UnitOutcome<T> {
    Done(T),
    Failed(ContinuityFailure),
}

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

fn interrupt(point: ContinuityInterrupt) -> Result<(), ContinuityFailure> {
    if take_interrupt(point) {
        Err(continuity_failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityCommit,
        ))
    } else {
        Ok(())
    }
}

pub fn commit_proposal(
    store: &ConversationStore,
    proposal: &ContinuityInterpretationProposal,
) -> Result<ContinuityCommitReceipt, ContinuityFailure> {
    let payload = serde_json::to_value(proposal).map_err(|_| {
        continuity_failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityParse,
        )
    })?;
    let result = run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        interrupt(ContinuityInterrupt::BeforeFirstWrite)?;
        if !unit
            .conversation_exists(&proposal.envelope.conversation_id)
            .map_err(store_to_continuity)?
        {
            return Err(continuity_failure(
                ContinuityFailureCode::ScopeDenied,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
        let basis = unit
            .read_commit_basis(&proposal.envelope.conversation_id)
            .map_err(store_to_continuity)?;
        admit_versions(&basis, &proposal.envelope)?;
        if let Some((previous, receipt)) = load_idempotency(unit, &proposal.envelope.request_id)? {
            admit_idempotency(&previous, &payload)?;
            return Ok(receipt);
        }
        apply_proposal(unit, proposal, &basis, &payload, None)
    })?;
    if take_interrupt(ContinuityInterrupt::AfterCommit) {
        return Ok(result);
    }
    Ok(result)
}

/// Same business commit as [`commit_proposal`], plus the user-posted applied
/// receipt in the same unit. A `BeforeCommit` interrupt rolls both back.
pub fn commit_user_posted_proposal(
    store: &ConversationStore,
    proposal: &ContinuityInterpretationProposal,
    source_event_id: &str,
    recipient_membership_id: &str,
) -> Result<ContinuityCommitReceipt, ContinuityFailure> {
    let payload = serde_json::to_value(proposal).map_err(|_| {
        continuity_failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityParse,
        )
    })?;
    let result = run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        interrupt(ContinuityInterrupt::BeforeFirstWrite)?;
        if !unit
            .conversation_exists(&proposal.envelope.conversation_id)
            .map_err(store_to_continuity)?
        {
            return Err(continuity_failure(
                ContinuityFailureCode::ScopeDenied,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
        let basis = unit
            .read_commit_basis(&proposal.envelope.conversation_id)
            .map_err(store_to_continuity)?;
        admit_versions(&basis, &proposal.envelope)?;
        if let Some((previous, receipt)) = load_idempotency(unit, &proposal.envelope.request_id)? {
            admit_idempotency(&previous, &payload)?;
            write_ingress_execution(
                unit,
                &proposal.envelope.conversation_id,
                source_event_id,
                recipient_membership_id,
                INGRESS_USER_POSTED_DESIGNATION,
            )?;
            unit.request_commit();
            return Ok(receipt);
        }
        apply_proposal(
            unit,
            proposal,
            &basis,
            &payload,
            Some((source_event_id, recipient_membership_id)),
        )
    })?;
    if take_interrupt(ContinuityInterrupt::AfterCommit) {
        return Ok(result);
    }
    Ok(result)
}

fn apply_proposal(
    unit: &mut ContinuityUnitOfWork<'_>,
    proposal: &ContinuityInterpretationProposal,
    basis: &ContinuityCommitBasis,
    payload: &Value,
    user_posted: Option<(&str, &str)>,
) -> Result<ContinuityCommitReceipt, ContinuityFailure> {
    let conversation_id = &proposal.envelope.conversation_id;
    let (revocation_generation, _) = ensure_scope(unit, conversation_id)?;
    let scopes = [
        ContinuityVisibilityScope::Conversation,
        ContinuityVisibilityScope::Matter,
        ContinuityVisibilityScope::Goal,
    ];
    for source in &proposal.envelope.source_event_refs {
        admit_source_ref(&scopes, source)?;
        if source_is_revoked(unit, conversation_id, source)? {
            return Err(continuity_failure(
                ContinuityFailureCode::SourceRevoked,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
        if source.owner_kind == ContinuitySourceOwnerKind::Event
            && !unit
                .event_exists(conversation_id, &source.opaque_id)
                .map_err(store_to_continuity)?
        {
            return Err(continuity_failure(
                ContinuityFailureCode::SourceUnavailable,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
    }

    for association in &proposal.matter_associations {
        admit_source_ref(&scopes, &association.source_ref)?;
        if source_is_revoked(unit, conversation_id, &association.source_ref)? {
            return Err(continuity_failure(
                ContinuityFailureCode::SourceRevoked,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
        let created = association.source_ref.clone();
        let refs = vec![association.source_ref.clone()];
        upsert_matter(
            unit,
            &ContinuityMatter {
                id: association.matter_id.clone(),
                conversation_id: conversation_id.clone(),
                revision: basis.revision + 1,
                label: association.matter_id.clone(),
                association_refs: refs,
                created_event: created,
                status: ContinuityMatterStatus::Open,
            },
        )?;
        upsert_association(
            unit,
            conversation_id,
            &proposal.envelope.request_id,
            association,
        )?;
    }

    for (index, agreement) in proposal.agreement_proposals.iter().enumerate() {
        persist_agreement(
            unit,
            conversation_id,
            agreement,
            index,
            revocation_generation,
        )?;
    }

    match proposal.speech_act {
        ContinuitySpeechAct::Pause => {
            apply_speech_control(unit, conversation_id, proposal, ContinuityGoalEvent::Pause)?;
        }
        ContinuitySpeechAct::Cancellation => {
            apply_speech_control(
                unit,
                conversation_id,
                proposal,
                ContinuityGoalEvent::CancelRequest,
            )?;
        }
        _ => {}
    }

    for (index, commitment) in proposal.commitment_proposals.iter().enumerate() {
        if !commitment.create_goal {
            continue;
        }
        let goal_id = derived_goal_id(proposal, commitment);
        let stored = load_goal(unit, &goal_id)?;
        match admit_commitment_admission(
            proposal.speech_act,
            commitment,
            stored
                .as_ref()
                .map(|(contract, progress)| (contract, progress)),
        )? {
            // Ordinary conversation, questions and hypothetical material keep
            // their matter association and admit no durable work.
            ContinuityCommitmentAdmission::Chat => continue,
            // The same commitment restated: the stored Goal and its revision
            // stay untouched, so nothing is duplicated or rewritten.
            ContinuityCommitmentAdmission::Reuse => continue,
            ContinuityCommitmentAdmission::Create => {
                admit_new_goal(
                    unit,
                    conversation_id,
                    proposal,
                    commitment,
                    &goal_id,
                    index,
                    basis,
                )?;
            }
            ContinuityCommitmentAdmission::Amend => {
                let (contract, progress) = stored.ok_or_else(invalid_request)?;
                amend_stored_goal(
                    unit,
                    conversation_id,
                    proposal,
                    commitment,
                    goal_id,
                    index,
                    contract,
                    progress,
                )?;
            }
        }
    }

    let mut created_card = false;
    if let Some(admission) = &proposal.task_child_admission {
        admit_task_child_admission(admission)?;
        if admission.parent_conversation_id != *conversation_id {
            return Err(continuity_failure(
                ContinuityFailureCode::ScopeDenied,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
        created_card = admit_or_reuse_child(unit, proposal, admission, basis)?;
        let wrote_obligation = persist_child_work_obligation(unit, conversation_id, admission)?;
        if wrote_obligation {
            if let Some((event_id, ingress_recipient)) = user_posted {
                issue_current_input_grants(
                    unit,
                    conversation_id,
                    admission,
                    event_id,
                    ingress_recipient,
                    &proposal.envelope.request_id,
                )?;
            }
        }
    }

    write_source_cursor(unit, &proposal.envelope)?;
    interrupt(ContinuityInterrupt::AfterStateWrite)?;
    if !created_card {
        unit.bump_revision_cas(conversation_id, basis.revision)
            .map_err(store_to_continuity)?;
    }
    let revision = unit
        .read_commit_basis(conversation_id)
        .map_err(store_to_continuity)?
        .revision;
    let receipt = default_receipt(
        conversation_id.clone(),
        revision,
        ContinuityEffectClass::None,
    );
    store_idempotency(
        unit,
        &proposal.envelope.request_id,
        conversation_id,
        payload,
        &receipt,
    )?;
    if let Some((event_id, membership_id)) = user_posted {
        write_ingress_execution(
            unit,
            conversation_id,
            event_id,
            membership_id,
            INGRESS_USER_POSTED_DESIGNATION,
        )?;
    }
    interrupt(ContinuityInterrupt::BeforeCommit)?;
    unit.request_commit();
    Ok(receipt)
}

/// Admit one new Goal for an explicit ongoing commitment. The matter is created
/// on first use and the first wake re-engages the Assistant at revision 1.
fn admit_new_goal(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    proposal: &ContinuityInterpretationProposal,
    commitment: &ContinuityCommitmentProposal,
    goal_id: &str,
    index: usize,
    basis: &ContinuityCommitBasis,
) -> Result<(), ContinuityFailure> {
    let matter_id = commitment
        .matter_id
        .clone()
        .or_else(|| {
            proposal
                .matter_associations
                .first()
                .map(|association| association.matter_id.clone())
        })
        .unwrap_or_else(|| new_continuity_id("matter"));
    let source = admitted_intent_source(proposal)?;
    if load_matter_label(unit, &matter_id)?.is_none() {
        upsert_matter(
            unit,
            &ContinuityMatter {
                id: matter_id.clone(),
                conversation_id: conversation_id.to_owned(),
                revision: basis.revision + 1,
                label: matter_id.clone(),
                association_refs: vec![source.clone()],
                created_event: source.clone(),
                status: ContinuityMatterStatus::Open,
            },
        )?;
    }
    let contract = ContinuityGoalContract {
        id: goal_id.to_owned(),
        matter_id: matter_id.clone(),
        source_intent_refs: proposal.envelope.source_event_refs.clone(),
        contract_revision: 1,
        expected_result: commitment.expected_result.clone(),
        criteria: commitment.criteria.clone(),
        scope_refs: Vec::new(),
        responsible_role_ref: basis
            .assistant_membership_id
            .clone()
            .unwrap_or_else(|| "role:assistant".into()),
        resource_envelope_ref: format!("envelope:{goal_id}"),
        acceptance_method: "goal-evaluation".into(),
        created_event: source,
    };
    let progress = ContinuityGoalProgress {
        goal_id: goal_id.to_owned(),
        revision: 1,
        lifecycle: ContinuityGoalLifecycle::Active,
        control: ContinuityGoalControl::Enabled,
        criterion_evidence_refs: Vec::new(),
        active_execution_refs: Vec::new(),
        blockers: Vec::new(),
        next_attention: Some(ContinuityNextAttention::DispatchableStep {
            step_ref: format!("step:{goal_id}:{index}"),
        }),
        closure_ref: None,
    };
    admit_goal_progress(&progress)?;
    upsert_goal(unit, conversation_id, &contract, &progress)?;
    let wake = goal_wake(unit, proposal, basis, goal_id, &progress, "event-priority")?;
    enqueue_wake(unit, &wake, conversation_id)?;
    Ok(())
}

/// Apply a corrected commitment to the Goal that already owns the identity. The
/// revision advances monotonically instead of restarting, accumulated control
/// and in-flight executions survive, and only criteria whose definition is
/// unchanged keep their evidence. The corrected revision is announced once.
fn amend_stored_goal(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    proposal: &ContinuityInterpretationProposal,
    commitment: &ContinuityCommitmentProposal,
    goal_id: String,
    index: usize,
    contract: ContinuityGoalContract,
    mut progress: ContinuityGoalProgress,
) -> Result<(), ContinuityFailure> {
    let previous_criteria = contract.criteria.clone();
    let mut contract = contract;
    contract.contract_revision += 1;
    contract.source_intent_refs = proposal.envelope.source_event_refs.clone();
    contract.expected_result = commitment.expected_result.clone();
    contract.criteria = commitment.criteria.clone();
    progress.criterion_evidence_refs = retain_current_criterion_evidence(
        &previous_criteria,
        &commitment.criteria,
        &progress.criterion_evidence_refs,
    );
    progress.revision += 1;
    if progress.active_execution_refs.is_empty() {
        progress.next_attention = Some(ContinuityNextAttention::DispatchableStep {
            step_ref: format!("step:{goal_id}:{index}"),
        });
    }
    admit_goal_progress(&progress)?;
    upsert_goal(unit, conversation_id, &contract, &progress)?;
    let basis = unit
        .read_commit_basis(conversation_id)
        .map_err(store_to_continuity)?;
    let wake = goal_wake(
        unit,
        proposal,
        &basis,
        &goal_id,
        &progress,
        "correction-priority",
    )?;
    enqueue_wake(unit, &wake, conversation_id)?;
    Ok(())
}

fn goal_wake(
    unit: &ContinuityUnitOfWork<'_>,
    proposal: &ContinuityInterpretationProposal,
    basis: &ContinuityCommitBasis,
    goal_id: &str,
    progress: &ContinuityGoalProgress,
    review_policy: &str,
) -> Result<ContinuityWake, ContinuityFailure> {
    Ok(ContinuityWake {
        logical_wake_id: format!("wake:{goal_id}:{}", progress.revision),
        goal_id: goal_id.to_owned(),
        cause_refs: proposal.envelope.source_event_refs.clone(),
        due_at: due_from_attention(&progress.next_attention),
        review_policy: review_policy.into(),
        goal_revision: progress.revision,
        epoch: basis.designation_epoch,
        host_generation: read_host_generation(unit)?,
        claim: None,
        settlement: None,
    })
}

fn invalid_request() -> ContinuityFailure {
    continuity_failure(
        ContinuityFailureCode::InvalidRequest,
        ContinuityFailureStage::ContinuityAdmission,
    )
}

fn admitted_intent_source(
    proposal: &ContinuityInterpretationProposal,
) -> Result<ContinuitySourceRef, ContinuityFailure> {
    proposal
        .envelope
        .source_event_refs
        .first()
        .cloned()
        .ok_or_else(invalid_request)
}

fn derived_goal_id(
    proposal: &ContinuityInterpretationProposal,
    commitment: &ContinuityCommitmentProposal,
) -> String {
    if let Some(admission) = &proposal.task_child_admission {
        return admission.goal_id.clone();
    }
    match commitment.matter_id.as_deref() {
        Some(matter) => matter_goal_id(matter),
        None => new_continuity_id("goal"),
    }
}

fn matter_goal_id(matter_id: &str) -> String {
    if let Some(rest) = matter_id.strip_prefix("matter:") {
        format!("goal:{rest}")
    } else if matter_id.starts_with("goal:") {
        matter_id.to_owned()
    } else {
        format!("goal:{matter_id}")
    }
}

fn load_matter_label(
    unit: &ContinuityUnitOfWork<'_>,
    matter_id: &str,
) -> Result<Option<String>, ContinuityFailure> {
    unit.query_row(
        "SELECT label FROM continuity_matters WHERE id=?1 AND deleted_at IS NULL",
        [matter_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(super::error::sql_failure)
}

fn persist_agreement(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    proposal: &ContinuityAgreementProposal,
    index: usize,
    revocation_generation: i64,
) -> Result<(), ContinuityFailure> {
    let agreement = ContinuityAgreement {
        id: new_continuity_id("agreement"),
        scope: proposal.scope,
        statement_ref: proposal.statement_ref.clone(),
        origin: proposal.origin,
        effective_revision: (index as i64) + 1,
        supersedes: None,
        valid_from: continuity_now_ms(),
        valid_until: None,
        revocation_generation,
    };
    insert_agreement(unit, conversation_id, &agreement)
}

fn targeted_goal_ids(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    proposal: &ContinuityInterpretationProposal,
) -> Result<Vec<String>, ContinuityFailure> {
    let mut ids = std::collections::BTreeSet::new();
    if let Some(admission) = &proposal.task_child_admission {
        ids.insert(admission.goal_id.clone());
    }
    for commitment in &proposal.commitment_proposals {
        if let Some(matter_id) = &commitment.matter_id {
            ids.insert(matter_goal_id(matter_id));
            for goal_id in goals_for_matter(unit, conversation_id, matter_id)? {
                ids.insert(goal_id);
            }
        }
    }
    for association in &proposal.matter_associations {
        ids.insert(matter_goal_id(&association.matter_id));
        for goal_id in goals_for_matter(unit, conversation_id, &association.matter_id)? {
            ids.insert(goal_id);
        }
    }
    Ok(ids.into_iter().collect())
}

fn apply_speech_control(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    proposal: &ContinuityInterpretationProposal,
    event: ContinuityGoalEvent,
) -> Result<(), ContinuityFailure> {
    let ids = targeted_goal_ids(unit, conversation_id, proposal)?;
    if ids.is_empty() {
        return Ok(());
    }
    for goal_id in ids {
        let Some((_, progress)) = load_goal(unit, &goal_id)? else {
            continue;
        };
        if is_terminal(progress.lifecycle) {
            continue;
        }
        apply_control_to_goal(unit, conversation_id, &goal_id, event)?;
    }
    Ok(())
}

fn apply_control_to_goal(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    goal_id: &str,
    event: ContinuityGoalEvent,
) -> Result<ContinuityGoalProgress, ContinuityFailure> {
    let stored_conversation = goal_conversation_id(unit, goal_id)?;
    if stored_conversation.as_deref() != Some(conversation_id) {
        return Err(continuity_failure(
            ContinuityFailureCode::ScopeDenied,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    let (contract, mut progress) = load_goal(unit, goal_id)?.ok_or_else(|| {
        continuity_failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityAdmission,
        )
    })?;
    let next = apply_goal_event(
        ContinuityGoalState {
            lifecycle: progress.lifecycle,
            control: progress.control,
        },
        event,
    )?;
    if event == ContinuityGoalEvent::Resume {
        if let Some(relation) = load_relation(unit, goal_id)? {
            if source_is_revoked(unit, conversation_id, &relation.created_event)? {
                return Err(continuity_failure(
                    ContinuityFailureCode::ScopeDenied,
                    ContinuityFailureStage::ContinuityAdmission,
                ));
            }
        }
    }
    progress.lifecycle = next.lifecycle;
    progress.control = next.control;
    progress.revision += 1;
    if next.control == ContinuityGoalControl::Paused {
        let previous_ref = match &progress.next_attention {
            Some(ContinuityNextAttention::DispatchableStep { step_ref }) => step_ref.clone(),
            Some(ContinuityNextAttention::ActiveExecution { execution_ref }) => {
                execution_ref.clone()
            }
            Some(ContinuityNextAttention::Wait { trigger_ref, .. })
                if !trigger_ref.starts_with("resume:") =>
            {
                trigger_ref.clone()
            }
            Some(ContinuityNextAttention::Wait {
                resumption_ref: Some(previous),
                ..
            }) => previous.clone(),
            _ => format!("step:{goal_id}:0"),
        };
        progress.next_attention = Some(ContinuityNextAttention::Wait {
            trigger_ref: format!("resume:{goal_id}"),
            review_policy: "user-resume".into(),
            responsible_party: "user".into(),
            resumption_ref: Some(previous_ref),
        });
    }
    if event == ContinuityGoalEvent::Resume {
        let restored = match &progress.next_attention {
            Some(ContinuityNextAttention::Wait {
                trigger_ref,
                resumption_ref,
                ..
            }) if trigger_ref.starts_with("resume:") => resumption_ref
                .clone()
                .unwrap_or_else(|| format!("step:{goal_id}:resume")),
            _ => format!("step:{goal_id}:resume"),
        };
        progress.next_attention =
            Some(ContinuityNextAttention::DispatchableStep { step_ref: restored });
    }
    if is_terminal(next.lifecycle) {
        progress.next_attention = None;
        progress.active_execution_refs.clear();
        progress.control = ContinuityGoalControl::Enabled;
    }
    admit_goal_progress(&progress)?;
    upsert_goal(unit, conversation_id, &contract, &progress)?;
    Ok(progress)
}

fn admit_or_reuse_child(
    unit: &ContinuityUnitOfWork<'_>,
    proposal: &ContinuityInterpretationProposal,
    admission: &super::ContinuityTaskChildAdmission,
    basis: &ContinuityCommitBasis,
) -> Result<bool, ContinuityFailure> {
    if let Some(existing) = load_relation(unit, &admission.goal_id)? {
        if let Some(observed) = &admission.observed_child_conversation_id {
            if observed != &existing.child_conversation_id {
                return Err(continuity_failure(
                    ContinuityFailureCode::IdentityConflict,
                    ContinuityFailureStage::ContinuityAdmission,
                ));
            }
        }
        if let Some(observed) = &admission.observed_card_anchor {
            super::admission::admit_card_identity_stable(&existing.card_anchor, observed)?;
        }
        return Ok(false);
    }
    if load_goal(unit, &admission.goal_id)?.is_none() {
        return Err(continuity_failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    let child = unit
        .create_canonical_child(
            &admission.parent_conversation_id,
            &format!("task:{}", admission.goal_id),
            admission.observed_child_conversation_id.as_deref(),
        )
        .map_err(store_to_continuity)?;
    let (relation, created_card) = if let Some(anchor) = &admission.observed_card_anchor {
        super::admission::admit_card_anchor(anchor)?;
        (
            ContinuityTaskConversationRelation {
                goal_id: admission.goal_id.clone(),
                parent_conversation_id: admission.parent_conversation_id.clone(),
                child_conversation_id: child.conversation_id,
                card_anchor: anchor.clone(),
                listing_kind: ContinuityTaskListingKind::ChildTask,
                follow_through_kind: ContinuityFollowThroughKind::Durable,
                completion_transition: None,
                revision: 1,
                created_event: ContinuitySourceRef {
                    owner_kind: ContinuitySourceOwnerKind::Event,
                    opaque_id: anchor.event_id.clone(),
                    part_id: anchor.part_id.clone(),
                    span: None,
                    source_revision: anchor.sequence,
                    digest: format!("card:{}", anchor.event_id),
                    visibility_scope: ContinuityVisibilityScope::Conversation,
                    validity: super::ContinuitySourceValidity::Current,
                },
            },
            false,
        )
    } else {
        let author = basis.assistant_membership_id.as_deref();
        let card = unit
            .append_parent_task_card(
                &admission.parent_conversation_id,
                author,
                &json!({
                    "goalId": admission.goal_id,
                    "childConversationId": child.conversation_id,
                    "requestId": admission.request_id,
                }),
            )
            .map_err(store_to_continuity)?;
        (
            ContinuityTaskConversationRelation {
                goal_id: admission.goal_id.clone(),
                parent_conversation_id: admission.parent_conversation_id.clone(),
                child_conversation_id: child.conversation_id,
                card_anchor: card.anchor,
                listing_kind: ContinuityTaskListingKind::ChildTask,
                follow_through_kind: ContinuityFollowThroughKind::Durable,
                completion_transition: None,
                revision: 1,
                created_event: card.created_event,
            },
            true,
        )
    };
    let _ = proposal;
    upsert_relation(unit, &relation, None)?;
    Ok(created_card)
}

fn persist_child_work_obligation(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    admission: &super::ContinuityTaskChildAdmission,
) -> Result<bool, ContinuityFailure> {
    let Some(relation) = load_relation(unit, &admission.goal_id)? else {
        return Ok(false);
    };
    let Some((_, progress)) = load_goal(unit, &admission.goal_id)? else {
        return Ok(false);
    };
    if child_work_started_unit(unit, conversation_id, &admission.goal_id, progress.revision)? {
        return Ok(false);
    }
    if load_child_work_intent(unit, conversation_id, &admission.goal_id, progress.revision)?
        .is_some()
    {
        return Ok(false);
    }
    let Some(membership) = child_recipient_membership(unit, &relation.child_conversation_id)?
    else {
        return Ok(false);
    };
    write_child_work_intent_unit(
        unit,
        conversation_id,
        &admission.goal_id,
        progress.revision,
        &json!({
            "kind": CHILD_WORK_INTENT_DESIGNATION,
            "goalId": admission.goal_id,
            "revision": progress.revision,
            "admittedRevision": progress.revision,
            "workGeneration": progress.revision.max(1),
            "childConversationId": relation.child_conversation_id,
            "membershipId": membership,
            "parentConversationId": conversation_id,
            "operationId": child_work_operation_id(&admission.goal_id, progress.revision),
        }),
    )?;
    Ok(true)
}

fn issue_current_input_grants(
    unit: &ContinuityUnitOfWork<'_>,
    parent_conversation_id: &str,
    admission: &super::ContinuityTaskChildAdmission,
    source_event_id: &str,
    ingress_recipient: &str,
    request_id: &str,
) -> Result<(), ContinuityFailure> {
    let owner = current_authorizing_human_owner(unit, parent_conversation_id)?;
    let Some((author, sequence, parts)) =
        load_grantable_event(unit, parent_conversation_id, source_event_id)?
    else {
        return Err(continuity_failure(
            ContinuityFailureCode::SourceUnavailable,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    };
    let Some(author) = author.filter(|value| !value.trim().is_empty()) else {
        return Err(continuity_failure(
            ContinuityFailureCode::ScopeDenied,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    };
    if author == ingress_recipient {
        return Err(continuity_failure(
            ContinuityFailureCode::ScopeDenied,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    admit_local_owner_principal(unit, parent_conversation_id, &author)?;
    if author != owner {
        return Err(continuity_failure(
            ContinuityFailureCode::ScopeDenied,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    let sources = exact_grantable_sources(
        source_event_id,
        sequence,
        &parts,
        ContinuityVisibilityScope::Conversation,
    );
    upsert_validated_child_grants(
        unit,
        parent_conversation_id,
        &admission.goal_id,
        &sources,
        request_id,
    )
}

fn issue_evidence_source_grant(
    unit: &ContinuityUnitOfWork<'_>,
    parent_conversation_id: &str,
    goal_id: &str,
    evidence: &ContinuityEvidenceRef,
) -> Result<(), ContinuityFailure> {
    if !matches!(
        evidence.source.owner_kind,
        ContinuitySourceOwnerKind::Event
            | ContinuitySourceOwnerKind::Part
            | ContinuitySourceOwnerKind::Span
    ) {
        return Ok(());
    }
    if evidence.source.validity == ContinuitySourceValidity::Revoked
        || evidence.validity == ContinuitySourceValidity::Revoked
    {
        return Ok(());
    }
    let Some((author, sequence, parts)) =
        load_grantable_event(unit, parent_conversation_id, &evidence.source.opaque_id)?
    else {
        return Ok(());
    };
    let Some(author) = author.filter(|value| !value.trim().is_empty()) else {
        return Ok(());
    };
    if admit_local_owner_principal(unit, parent_conversation_id, &author).is_err() {
        return Ok(());
    }
    if sequence != evidence.source.source_revision {
        return Ok(());
    }
    if source_is_revoked(unit, parent_conversation_id, &evidence.source)? {
        return Ok(());
    }
    let Some(source) = validated_exact_source(&evidence.source, sequence, &parts) else {
        return Ok(());
    };
    upsert_validated_child_grants(
        unit,
        parent_conversation_id,
        goal_id,
        std::slice::from_ref(&source),
        &format!(
            "evidence:{goal_id}:{}:{}",
            evidence.criterion_id, evidence.subject_version
        ),
    )
}

fn revoke_superseded_evidence_grants(
    unit: &ContinuityUnitOfWork<'_>,
    parent_conversation_id: &str,
    goal_id: &str,
    evidence: &ContinuityEvidenceRef,
) -> Result<(), ContinuityFailure> {
    let Some((_, progress)) = load_goal(unit, goal_id)? else {
        return Ok(());
    };
    let Some(relation) = load_relation(unit, goal_id)? else {
        return Ok(());
    };
    let Some(recipient) = child_recipient_membership(unit, &relation.child_conversation_id)? else {
        return Ok(());
    };
    let superseded: Vec<ContinuitySourceRef> = progress
        .criterion_evidence_refs
        .iter()
        .filter(|item| {
            item.criterion_id == evidence.criterion_id
                && item.subject_version < evidence.subject_version
        })
        .map(|item| item.source.clone())
        .collect();
    if superseded.is_empty() {
        return Ok(());
    }
    revoke_admitted_recipient_grants(
        unit,
        parent_conversation_id,
        &relation.child_conversation_id,
        &recipient,
        |allowed| {
            superseded.iter().any(|old| {
                allowed.owner_kind == old.owner_kind
                    && allowed.opaque_id == old.opaque_id
                    && allowed.part_id == old.part_id
                    && allowed.source_revision == old.source_revision
                    && allowed.digest == old.digest
            })
        },
    )
}

fn upsert_validated_child_grants(
    unit: &ContinuityUnitOfWork<'_>,
    parent_conversation_id: &str,
    goal_id: &str,
    sources: &[ContinuitySourceRef],
    request_id: &str,
) -> Result<(), ContinuityFailure> {
    if sources.is_empty() {
        return Ok(());
    }
    let Some(relation) = load_relation(unit, goal_id)? else {
        return Ok(());
    };
    if relation.parent_conversation_id != parent_conversation_id {
        return Err(continuity_failure(
            ContinuityFailureCode::ScopeDenied,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    let Some((_, progress)) = load_goal(unit, goal_id)? else {
        return Ok(());
    };
    let Some(recipient) = child_recipient_membership(unit, &relation.child_conversation_id)? else {
        return Err(continuity_failure(
            ContinuityFailureCode::ScopeDenied,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    };
    if !unit
        .membership_is_active_agent(&relation.child_conversation_id, &recipient)
        .map_err(store_to_continuity)?
    {
        return Err(continuity_failure(
            ContinuityFailureCode::ScopeDenied,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    let operation_id = child_work_operation_id(goal_id, progress.revision);
    let (recipient_generation, _) = ensure_scope(unit, &relation.child_conversation_id)?;
    for source in sources {
        if source_is_revoked(unit, parent_conversation_id, source)? {
            return Err(continuity_failure(
                ContinuityFailureCode::SourceRevoked,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
        let part = source.part_id.as_deref().unwrap_or("event");
        let grant = ContinuityParentContextGrant {
            grant_id: format!(
                "grant:{}:{operation_id}:{}:{part}",
                relation.child_conversation_id, source.opaque_id
            ),
            source_conversation_id: parent_conversation_id.to_owned(),
            recipient_conversation_id: relation.child_conversation_id.clone(),
            recipient_membership_id: recipient.clone(),
            source_refs: vec![source.clone()],
            authorized_scopes: vec![source.visibility_scope],
            status: ContinuityParentGrantStatus::Admitted,
            request_id: request_id.to_owned(),
            revocation_generation: recipient_generation,
        };
        upsert_grant(unit, &grant)?;
    }
    Ok(())
}

fn current_authorizing_human_owner(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
) -> Result<String, ContinuityFailure> {
    let owner = unit
        .current_human_owner_membership(conversation_id)
        .map_err(store_to_continuity)?
        .ok_or_else(|| {
            continuity_failure(
                ContinuityFailureCode::ScopeDenied,
                ContinuityFailureStage::ContinuityAdmission,
            )
        })?;
    admit_local_owner_principal(unit, conversation_id, &owner)?;
    Ok(owner)
}

fn load_grantable_event(
    unit: &ContinuityUnitOfWork<'_>,
    conversation_id: &str,
    event_id: &str,
) -> Result<Option<(Option<String>, i64, Vec<(String, String, String)>)>, ContinuityFailure> {
    let Some((author, sequence)) = unit
        .event_author_and_sequence(conversation_id, event_id)
        .map_err(store_to_continuity)?
    else {
        return Ok(None);
    };
    let parts = unit
        .event_canonical_parts(conversation_id, event_id)
        .map_err(store_to_continuity)?;
    Ok(Some((author, sequence, parts)))
}

fn exact_grantable_sources(
    event_id: &str,
    sequence: i64,
    parts: &[(String, String, String)],
    scope: ContinuityVisibilityScope,
) -> Vec<ContinuitySourceRef> {
    parts
        .iter()
        .filter(|(_, kind, _)| matches!(kind.as_str(), "text" | "reasoning" | "image"))
        .map(|(part_id, kind, _)| exact_part_source(event_id, sequence, part_id, kind, scope))
        .collect()
}

fn exact_part_source(
    event_id: &str,
    sequence: i64,
    part_id: &str,
    kind: &str,
    visibility_scope: ContinuityVisibilityScope,
) -> ContinuitySourceRef {
    ContinuitySourceRef {
        owner_kind: canonical_part_owner_kind(kind),
        opaque_id: event_id.to_owned(),
        part_id: Some(part_id.to_owned()),
        span: None,
        source_revision: sequence,
        digest: canonical_part_digest(event_id, part_id, kind),
        visibility_scope,
        validity: ContinuitySourceValidity::Current,
    }
}

fn canonical_part_digest(event_id: &str, part_id: &str, kind: &str) -> String {
    if kind == "image" {
        format!("part:{part_id}")
    } else {
        format!("event:{event_id}:{part_id}")
    }
}

fn canonical_part_owner_kind(kind: &str) -> ContinuitySourceOwnerKind {
    if kind == "image" {
        ContinuitySourceOwnerKind::Part
    } else {
        ContinuitySourceOwnerKind::Event
    }
}

fn validated_exact_source(
    requested: &ContinuitySourceRef,
    sequence: i64,
    parts: &[(String, String, String)],
) -> Option<ContinuitySourceRef> {
    if requested.validity == ContinuitySourceValidity::Revoked {
        return None;
    }
    if sequence != requested.source_revision {
        return None;
    }
    let scopes = [
        ContinuityVisibilityScope::Conversation,
        ContinuityVisibilityScope::Matter,
        ContinuityVisibilityScope::Goal,
    ];
    if admit_source_ref(&scopes, requested).is_err() {
        return None;
    }
    match requested.part_id.as_deref().filter(|id| !id.is_empty()) {
        None => {
            if requested.owner_kind != ContinuitySourceOwnerKind::Event || requested.span.is_some()
            {
                return None;
            }
            Some(requested.clone())
        }
        Some(part_id) => {
            let (_, kind, content) = parts.iter().find(|(id, _, _)| id == part_id)?;
            if !matches!(kind.as_str(), "text" | "reasoning" | "image") {
                return None;
            }
            if requested.digest != canonical_part_digest(&requested.opaque_id, part_id, kind) {
                return None;
            }
            let canonical_kind = canonical_part_owner_kind(kind);
            match requested.owner_kind {
                ContinuitySourceOwnerKind::Span => {
                    if kind == "image" {
                        return None;
                    }
                    let span = requested.span.as_ref()?;
                    if admit_utf8_span(content, span.start_byte, span.end_byte).is_err() {
                        return None;
                    }
                }
                other if other == canonical_kind => {
                    if let Some(span) = requested.span.as_ref() {
                        if admit_utf8_span(content, span.start_byte, span.end_byte).is_err() {
                            return None;
                        }
                    }
                }
                _ => return None,
            }
            Some(requested.clone())
        }
    }
}

pub fn apply_goal_control(
    store: &ConversationStore,
    conversation_id: &str,
    goal_id: &str,
    event: ContinuityGoalEvent,
) -> Result<ContinuityGoalProgress, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        let progress = apply_control_to_goal(unit, conversation_id, goal_id, event)?;
        unit.request_commit();
        Ok(progress)
    })
}

/// Move an unfinished Goal to the owner that now holds the work.
///
/// A handoff changes who is responsible, not what was agreed: the expected
/// result, the criteria, the recorded evidence and the in-flight executions
/// survive unchanged, so an executor replacement never loses or rewrites
/// admitted work. The revision advances, which makes every proposal observed
/// before the handoff stale, so the previous owner cannot overwrite the current
/// work; the new owner is then announced the Goal exactly once.
///
/// A closed Goal and a Goal already owned by the named owner are left
/// untouched, which makes a repeated handoff idempotent.
pub fn hand_off_goal_owner(
    store: &ConversationStore,
    conversation_id: &str,
    goal_id: &str,
    responsible_role_ref: &str,
) -> Result<Option<ContinuityGoalProgress>, ContinuityFailure> {
    if responsible_role_ref.trim().is_empty() {
        return Err(invalid_request());
    }
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        let basis = unit
            .read_commit_basis(conversation_id)
            .map_err(store_to_continuity)?;
        let Some((contract, progress)) = load_goal(unit, goal_id)? else {
            return Err(continuity_failure(
                ContinuityFailureCode::SourceUnavailable,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        };
        if goal_conversation_id(unit, goal_id)?.as_deref() != Some(conversation_id) {
            return Err(invalid_request());
        }
        if is_terminal(progress.lifecycle) || contract.responsible_role_ref == responsible_role_ref
        {
            return Ok(None);
        }
        let mut contract = contract;
        contract.responsible_role_ref = responsible_role_ref.to_owned();
        contract.contract_revision += 1;
        let mut progress = progress;
        progress.revision += 1;
        admit_goal_progress(&progress)?;
        upsert_goal(unit, conversation_id, &contract, &progress)?;
        let wake = ContinuityWake {
            logical_wake_id: format!("wake:{goal_id}:{}:owner-handoff", progress.revision),
            goal_id: goal_id.to_owned(),
            cause_refs: vec![contract.created_event.clone()],
            due_at: due_from_attention(&progress.next_attention),
            review_policy: OWNER_HANDOFF_REVIEW_POLICY.to_owned(),
            goal_revision: progress.revision,
            epoch: 0,
            host_generation: read_host_generation(unit)?,
            claim: None,
            settlement: None,
        };
        enqueue_wake(unit, &wake, conversation_id)?;
        // The revision advances, so a proposal observed before the handoff is
        // stale and cannot overwrite the work the successor now holds.
        unit.bump_revision_cas(conversation_id, basis.revision)
            .map_err(store_to_continuity)?;
        unit.request_commit();
        Ok(Some(progress))
    })
}

/// Hand every unfinished Goal of a conversation to the owner that now holds it.
///
/// Returns how many Goals actually changed owner.
pub fn hand_off_conversation_goals(
    store: &ConversationStore,
    conversation_id: &str,
    responsible_role_ref: &str,
) -> Result<usize, ContinuityFailure> {
    if responsible_role_ref.trim().is_empty() {
        return Err(invalid_request());
    }
    let relations = read_child_relations(store, conversation_id, None, CONTINUITY_MAX_PAGE_SIZE)?;
    let mut handed = 0;
    for relation in relations {
        if hand_off_goal_owner(
            store,
            conversation_id,
            &relation.goal_id,
            responsible_role_ref,
        )?
        .is_some()
        {
            handed += 1;
        }
    }
    Ok(handed)
}

pub fn accept_completion(
    store: &ConversationStore,
    conversation_id: &str,
    transition: &ContinuityGoalCompletionTransition,
    progress: &ContinuityGoalProgress,
) -> Result<bool, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        admit_completion_transition(transition, progress)?;
        if goal_conversation_id(unit, &transition.goal_id)?.as_deref() != Some(conversation_id) {
            return Err(continuity_failure(
                ContinuityFailureCode::ScopeDenied,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
        let (mut contract, current) = load_goal(unit, &transition.goal_id)?.ok_or_else(|| {
            continuity_failure(
                ContinuityFailureCode::InvalidRequest,
                ContinuityFailureStage::ContinuityAdmission,
            )
        })?;
        if is_terminal(current.lifecycle) {
            return Ok(false);
        }
        admit_completion_against_current(transition, &current)?;
        if has_unknown_effects(unit, &transition.goal_id)? {
            return Err(continuity_failure(
                ContinuityFailureCode::ReconciliationRequired,
                ContinuityFailureStage::ContinuityEffects,
            ));
        }
        if transition.to_lifecycle == ContinuityGoalLifecycle::Achieved {
            admit_achieved_required_current_evidence(transition, &contract, &current)?;
            for item in current_required_evidence(&contract, &current) {
                if source_is_revoked(unit, conversation_id, &item.source)? {
                    return Err(continuity_failure(
                        ContinuityFailureCode::SourceRevoked,
                        ContinuityFailureStage::ContinuityAdmission,
                    ));
                }
            }
            if source_is_revoked(unit, conversation_id, &transition.evaluation_ref)? {
                return Err(continuity_failure(
                    ContinuityFailureCode::SourceRevoked,
                    ContinuityFailureStage::ContinuityAdmission,
                ));
            }
        }
        if load_completion(unit, &transition.notification_id)?.is_some() {
            return Ok(false);
        }
        contract.contract_revision = current.revision;
        let mut stored = current.clone();
        stored.lifecycle = transition.to_lifecycle;
        stored.next_attention = None;
        stored.active_execution_refs.clear();
        stored.control = ContinuityGoalControl::Enabled;
        admit_goal_progress(&stored)?;
        upsert_goal(unit, conversation_id, &contract, &stored)?;
        let first = store_completion(unit, transition)?;
        if let Some(mut relation) = load_relation(unit, &transition.goal_id)? {
            relation.completion_transition = Some(transition.clone());
            relation.revision += 1;
            let previous = load_relation(unit, &transition.goal_id)?;
            upsert_relation(unit, &relation, previous.as_ref())?;
            if let Some(part_id) = &relation.card_anchor.part_id {
                unit.update_event_part_content(
                    part_id,
                    &json!({
                        "goalId": transition.goal_id,
                        "childConversationId": relation.child_conversation_id,
                        "toLifecycle": match transition.to_lifecycle {
                            ContinuityGoalLifecycle::Achieved => "achieved",
                            ContinuityGoalLifecycle::Cancelled => "cancelled",
                            ContinuityGoalLifecycle::Superseded => "superseded",
                            _ => "active",
                        },
                        "notificationId": transition.notification_id,
                    }),
                )
                .map_err(store_to_continuity)?;
            }
        }
        unit.request_commit();
        Ok(first)
    })
}

pub fn put_effect(
    store: &ConversationStore,
    conversation_id: &str,
    goal_id: Option<&str>,
    logical_effect_id: &str,
    status: ContinuityEffectStatus,
) -> Result<ContinuityEffectStatus, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        if let Some(existing) = load_effect(unit, logical_effect_id)? {
            if existing == ContinuityEffectStatus::Unknown
                && status != ContinuityEffectStatus::Unknown
            {
                // resolve after query
            } else if existing == ContinuityEffectStatus::Unknown {
                return Err(continuity_failure(
                    ContinuityFailureCode::ReconciliationRequired,
                    ContinuityFailureStage::ContinuityEffects,
                ));
            } else if existing == ContinuityEffectStatus::Executed
                && status == ContinuityEffectStatus::NotExecuted
            {
                return Err(continuity_failure(
                    ContinuityFailureCode::InvalidRequest,
                    ContinuityFailureStage::ContinuityEffects,
                ));
            }
        }
        if status == ContinuityEffectStatus::NotExecuted {
            if let Some(ContinuityEffectStatus::Unknown) = load_effect(unit, logical_effect_id)? {
                return Err(continuity_failure(
                    ContinuityFailureCode::ReconciliationRequired,
                    ContinuityFailureStage::ContinuityEffects,
                ));
            }
        }
        let stored = record_effect(unit, conversation_id, goal_id, logical_effect_id, status)?;
        unit.request_commit();
        Ok(stored)
    })
}

pub fn replay_effect(
    store: &ConversationStore,
    logical_effect_id: &str,
) -> Result<(), ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        match load_effect(unit, logical_effect_id)? {
            Some(ContinuityEffectStatus::Unknown) => Err(continuity_failure(
                ContinuityFailureCode::ReconciliationRequired,
                ContinuityFailureStage::ContinuityEffects,
            )),
            Some(ContinuityEffectStatus::Executed) => Err(continuity_failure(
                ContinuityFailureCode::InvalidRequest,
                ContinuityFailureStage::ContinuityEffects,
            )),
            Some(ContinuityEffectStatus::NotExecuted) | None => Ok(()),
        }
    })
}

pub fn revoke_source(
    store: &ConversationStore,
    conversation_id: &str,
    opaque_id: &str,
    deleted: bool,
) -> Result<i64, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        let generation = increment_revocation(unit, conversation_id, Some(opaque_id), deleted)?;
        unit.request_commit();
        Ok(generation)
    })
}

pub fn put_grant(
    store: &ConversationStore,
    grant: &ContinuityParentContextGrant,
) -> Result<(), ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        upsert_grant(unit, grant)?;
        unit.request_commit();
        Ok(())
    })
}

pub fn put_derived(
    store: &ConversationStore,
    conversation_id: &str,
    kind: &str,
    source_opaque_id: &str,
    body: &str,
) -> Result<String, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        let (generation, _) = ensure_scope(unit, conversation_id)?;
        let id = insert_derived(
            unit,
            conversation_id,
            kind,
            source_opaque_id,
            body,
            generation,
        )?;
        unit.request_commit();
        Ok(id)
    })
}

pub fn derived_live_count(
    store: &ConversationStore,
    conversation_id: &str,
) -> Result<i64, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        live_derived_count(unit, conversation_id)
    })
}

pub fn read_goal(
    store: &ConversationStore,
    goal_id: &str,
) -> Result<Option<ContinuityGoalProgress>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        Ok(load_goal(unit, goal_id)?.map(|(_, progress)| progress))
    })
}

pub fn read_goal_bundle(
    store: &ConversationStore,
    goal_id: &str,
) -> Result<Option<(ContinuityGoalContract, ContinuityGoalProgress)>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        load_goal(unit, goal_id)
    })
}

pub fn read_agreements(
    store: &ConversationStore,
    conversation_id: &str,
) -> Result<Vec<ContinuityAgreement>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        current_agreements(unit, conversation_id)
    })
}

pub fn read_pending_wakes(
    store: &ConversationStore,
    conversation_id: &str,
) -> Result<Vec<ContinuityWake>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        list_pending_wakes(unit, conversation_id)
    })
}

fn read_host_generation(unit: &ContinuityUnitOfWork<'_>) -> Result<i64, ContinuityFailure> {
    Ok(unit
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM continuity_schema WHERE key='host_generation'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(super::error::sql_failure)?
        .unwrap_or(0))
}

pub fn read_pending_outbox(
    store: &ConversationStore,
    conversation_id: &str,
) -> Result<i64, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        pending_outbox_count(unit, conversation_id)
    })
}

pub fn consume_logical_wake(
    store: &ConversationStore,
    logical_wake_id: &str,
) -> Result<bool, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        let changed = consume_wake(unit, logical_wake_id)?;
        if changed {
            unit.request_commit();
        }
        Ok(changed)
    })
}

pub fn enqueue_follow_up(
    store: &ConversationStore,
    wake: &ContinuityWake,
) -> Result<ContinuityCommitReceipt, ContinuityFailure> {
    admit_wake(wake)?;
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        let conversation_id = goal_conversation_id(unit, &wake.goal_id)?.ok_or_else(|| {
            continuity_failure(
                ContinuityFailureCode::InvalidRequest,
                ContinuityFailureStage::ContinuityAdmission,
            )
        })?;
        if let Some((_, progress)) = load_goal(unit, &wake.goal_id)? {
            if is_terminal(progress.lifecycle) || suppresses_new_work(progress.control) {
                return Ok(no_effect_receipt(
                    conversation_id.clone(),
                    unit.read_commit_basis(&conversation_id)
                        .map_err(store_to_continuity)?
                        .revision,
                ));
            }
            if has_unknown_effects(unit, &wake.goal_id)? {
                return Err(continuity_failure(
                    ContinuityFailureCode::ReconciliationRequired,
                    ContinuityFailureStage::ContinuityEffects,
                ));
            }
        }
        enqueue_wake(unit, wake, &conversation_id)?;
        let basis = unit
            .read_commit_basis(&conversation_id)
            .map_err(store_to_continuity)?;
        unit.request_commit();
        Ok(default_receipt(
            conversation_id,
            basis.revision,
            ContinuityEffectClass::None,
        ))
    })
}

pub fn read_matters_page(
    store: &ConversationStore,
    conversation_id: &str,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<ContinuityMatter>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        list_matters(unit, conversation_id, after, limit)
    })
}

pub fn read_relation(
    store: &ConversationStore,
    goal_id: &str,
) -> Result<ContinuityTaskConversationRelation, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        load_relation(unit, goal_id)?.ok_or_else(|| {
            continuity_failure(
                ContinuityFailureCode::SourceUnavailable,
                ContinuityFailureStage::ContinuityAdmission,
            )
        })
    })
}

pub fn read_child_relations(
    store: &ConversationStore,
    parent_conversation_id: &str,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<ContinuityTaskConversationRelation>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        list_child_relations(unit, parent_conversation_id, after, limit as u64)
    })
}

pub fn read_parent_grants(
    store: &ConversationStore,
    recipient_conversation_id: &str,
    recipient_membership_id: &str,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<ContinuityParentContextGrant>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        list_parent_grants(
            unit,
            recipient_conversation_id,
            recipient_membership_id,
            after,
            limit as u64,
        )
    })
}

pub fn read_basis(
    store: &ConversationStore,
    conversation_id: &str,
) -> Result<ContinuityCommitBasis, ContinuityFailure> {
    store
        .continuity_commit_basis(conversation_id)
        .map_err(store_to_continuity)
}

pub fn put_agreement(
    store: &ConversationStore,
    conversation_id: &str,
    agreement: &ContinuityAgreement,
) -> Result<(), ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        insert_agreement(unit, conversation_id, agreement)?;
        unit.request_commit();
        Ok(())
    })
}

pub fn read_child_links(
    store: &ConversationStore,
) -> Result<Vec<(String, String, String)>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        list_child_links(unit)
    })
}

pub fn bump_host_generation(store: &ConversationStore) -> Result<i64, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        let current: i64 = unit
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM continuity_schema WHERE key='host_generation'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(super::error::sql_failure)?
            .unwrap_or(0);
        let next = current + 1;
        unit.execute(
            "INSERT INTO continuity_schema(key, value) VALUES ('host_generation', ?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [next.to_string()],
        )
        .map_err(super::error::sql_failure)?;
        unit.request_commit();
        Ok(next)
    })
}

pub fn record_qualification_invalidation(
    store: &ConversationStore,
    responsibility_id: &str,
    identity_key: &str,
) -> Result<(), ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        unit.execute(
            "INSERT INTO continuity_qualification_invalidations(
               responsibility_id, identity_key, withdrawn_at
             ) VALUES (?1, ?2, ?3)
             ON CONFLICT(responsibility_id, identity_key) DO UPDATE SET withdrawn_at=excluded.withdrawn_at",
            rusqlite::params![responsibility_id, identity_key, continuity_now_ms()],
        )
        .map_err(super::error::sql_failure)?;
        unit.request_commit();
        Ok(())
    })
}

pub fn read_qualification_invalidations(
    store: &ConversationStore,
) -> Result<Vec<(String, String)>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        unit.query_vec(
            "SELECT responsibility_id, identity_key
             FROM continuity_qualification_invalidations
             ORDER BY responsibility_id ASC, identity_key ASC",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(super::error::sql_failure)
    })
}

pub fn current_host_generation(store: &ConversationStore) -> Result<i64, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        read_host_generation(unit)
    })
}

pub fn list_all_pending_wakes(
    store: &ConversationStore,
) -> Result<Vec<(String, ContinuityWake)>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        list_all_pending_wakes_unit(unit)
    })
}

pub fn update_wake_host_generation(
    store: &ConversationStore,
    logical_wake_id: &str,
    generation: i64,
) -> Result<bool, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        let changed = update_wake_generation(unit, logical_wake_id, generation)?;
        if changed {
            unit.request_commit();
        }
        Ok(changed)
    })
}

pub fn append_criterion_evidence(
    store: &ConversationStore,
    conversation_id: &str,
    goal_id: &str,
    evidence: ContinuityEvidenceRef,
) -> Result<ContinuityGoalProgress, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        ensure_scope(unit, conversation_id)?;
        revoke_superseded_evidence_grants(unit, conversation_id, goal_id, &evidence)?;
        let progress = append_goal_evidence(unit, conversation_id, goal_id, evidence.clone())?;
        issue_evidence_source_grant(unit, conversation_id, goal_id, &evidence)?;
        let _ = persist_child_work_obligation(
            unit,
            conversation_id,
            &ContinuityTaskChildAdmission {
                goal_id: goal_id.to_owned(),
                parent_conversation_id: conversation_id.to_owned(),
                speech_act: ContinuitySpeechAct::Delegation,
                follow_through_kind: ContinuityFollowThroughKind::Durable,
                observed_child_conversation_id: None,
                observed_card_anchor: None,
                request_id: format!("request:evidence-work:{goal_id}:{}", progress.revision),
            },
        )?;
        unit.request_commit();
        Ok(progress)
    })
}

pub fn bump_revocation(
    store: &ConversationStore,
    conversation_id: &str,
) -> Result<i64, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        let generation = increment_revocation(unit, conversation_id, None, false)?;
        unit.request_commit();
        Ok(generation)
    })
}

pub fn load_effect_status(
    store: &ConversationStore,
    logical_effect_id: &str,
) -> Result<Option<ContinuityEffectStatus>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        load_effect(unit, logical_effect_id)
    })
}

pub fn put_qualification_evidence(
    store: &ConversationStore,
    responsibility_id: &str,
    identity_key: &str,
    payload: &str,
    evidence_class: &str,
) -> Result<(), ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        persist_qualification_evidence(
            unit,
            responsibility_id,
            identity_key,
            payload,
            evidence_class,
        )?;
        unit.request_commit();
        Ok(())
    })
}

pub fn list_qualification_evidence(
    store: &ConversationStore,
) -> Result<Vec<(String, String, String, String)>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        load_qualification_evidence(unit)
    })
}

pub fn load_adoption_policy_values(
    store: &ConversationStore,
) -> Result<(bool, String), ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        let enabled = match load_schema_value(unit, super::persist::ADOPTION_ENABLED_KEY)? {
            Some(value) => value != "0",
            None => true,
        };
        let stage = load_schema_value(unit, super::persist::ADOPTION_STAGE_KEY)?
            .unwrap_or_else(|| super::persist::ADOPTION_STAGE_OFFLINE.to_owned());
        Ok((enabled, stage))
    })
}

pub fn persist_adoption_stage(
    store: &ConversationStore,
    stage: &str,
) -> Result<(), ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        persist_schema_value(unit, super::persist::ADOPTION_STAGE_KEY, stage)?;
        unit.request_commit();
        Ok(())
    })
}

pub fn resolve_stored_owner_authority(
    store: &ConversationStore,
    conversation_id: &str,
    owner_membership_id: &str,
) -> Result<StoredOwnerAuthority, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        let archived: Option<i64> = unit
            .query_row(
                "SELECT archived FROM conversations WHERE id=?1",
                [conversation_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_failure)?;
        if archived != Some(0) {
            return Err(continuity_failure(
                ContinuityFailureCode::ScopeDenied,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
        let owner_principal_id =
            admit_local_owner_principal(unit, conversation_id, owner_membership_id)?;
        Ok(StoredOwnerAuthority::from_admitted(
            conversation_id.to_owned(),
            owner_membership_id.to_owned(),
            owner_principal_id,
        ))
    })
}

pub fn persist_evaluation_session(
    store: &ConversationStore,
    session: &StoredEvaluationSession,
) -> Result<(), ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        persist_evaluation_session_in_unit(unit, session)?;
        unit.request_commit();
        Ok(())
    })
}

pub fn load_evaluation_session(
    store: &ConversationStore,
    session_id: &str,
) -> Result<Option<StoredEvaluationSession>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        load_evaluation_session_in_unit(unit, session_id)
    })
}

pub fn load_qualification_evidence_for(
    store: &ConversationStore,
    responsibility_id: &str,
    identity_key: &str,
) -> Result<Option<(String, String, String, String)>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        load_qualification_evidence_row(unit, responsibility_id, identity_key)
    })
}

/// Claim the exact session collection effect before a native evaluation call.
/// Executed or already-claimed operations fail closed without a second invoke.
pub fn claim_collection_operation(
    store: &ConversationStore,
    session: &StoredEvaluationSession,
    identity_key: &str,
) -> Result<(), ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        let current =
            load_evaluation_session_in_unit(unit, &session.session_id)?.ok_or_else(|| {
                continuity_failure(
                    ContinuityFailureCode::InvalidRequest,
                    ContinuityFailureStage::ContinuityAdmission,
                )
            })?;
        if current.consumed {
            return Err(continuity_failure(
                ContinuityFailureCode::IdempotencyConflict,
                ContinuityFailureStage::ContinuityCommit,
            ));
        }
        if !current.same_admitted_binding(session) {
            return Err(continuity_failure(
                ContinuityFailureCode::InvalidRequest,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
        if load_qualification_evidence_row(unit, &current.responsibility_id, identity_key)?
            .is_some()
        {
            return Err(continuity_failure(
                ContinuityFailureCode::IdempotencyConflict,
                ContinuityFailureStage::ContinuityCommit,
            ));
        }
        let effect_id = super::persist::collection_effect_id_for_session(&current.session_id);
        match load_effect(unit, &effect_id)? {
            Some(ContinuityEffectStatus::Executed) => {
                return Err(continuity_failure(
                    ContinuityFailureCode::IdempotencyConflict,
                    ContinuityFailureStage::ContinuityCommit,
                ));
            }
            Some(ContinuityEffectStatus::Unknown) => {
                return Err(continuity_failure(
                    ContinuityFailureCode::ReconciliationRequired,
                    ContinuityFailureStage::ContinuityEffects,
                ));
            }
            Some(ContinuityEffectStatus::NotExecuted) => {
                return Err(continuity_failure(
                    ContinuityFailureCode::IdempotencyConflict,
                    ContinuityFailureStage::ContinuityCommit,
                ));
            }
            None => {}
        }
        record_effect(
            unit,
            &current.conversation_id,
            None,
            &effect_id,
            ContinuityEffectStatus::NotExecuted,
        )?;
        unit.request_commit();
        Ok(())
    })
}

/// Mark the exact collection claim Unknown immediately before the first native
/// invocation. Pre-invoke callers must not use this; post-invoke failures keep
/// Unknown so retry reconciles instead of rerunning.
pub fn begin_collection_invocation(
    store: &ConversationStore,
    session: &StoredEvaluationSession,
) -> Result<(), ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        let current =
            load_evaluation_session_in_unit(unit, &session.session_id)?.ok_or_else(|| {
                continuity_failure(
                    ContinuityFailureCode::InvalidRequest,
                    ContinuityFailureStage::ContinuityAdmission,
                )
            })?;
        if current.consumed || !current.same_admitted_binding(session) {
            return Err(continuity_failure(
                ContinuityFailureCode::InvalidRequest,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
        let effect_id = super::persist::collection_effect_id_for_session(&current.session_id);
        match load_effect(unit, &effect_id)? {
            Some(ContinuityEffectStatus::NotExecuted) => {}
            Some(ContinuityEffectStatus::Unknown) => {
                return Err(continuity_failure(
                    ContinuityFailureCode::ReconciliationRequired,
                    ContinuityFailureStage::ContinuityEffects,
                ));
            }
            Some(ContinuityEffectStatus::Executed) => {
                return Err(continuity_failure(
                    ContinuityFailureCode::IdempotencyConflict,
                    ContinuityFailureStage::ContinuityCommit,
                ));
            }
            None => {
                return Err(continuity_failure(
                    ContinuityFailureCode::InvalidRequest,
                    ContinuityFailureStage::ContinuityEffects,
                ));
            }
        }
        record_effect(
            unit,
            &current.conversation_id,
            None,
            &effect_id,
            ContinuityEffectStatus::Unknown,
        )?;
        unit.request_commit();
        Ok(())
    })
}

/// Drop an uncommitted collection claim after a failed native evaluation.
/// Executed receipts are left untouched. Unknown claims are retained.
pub fn release_collection_operation(
    store: &ConversationStore,
    session_id: &str,
) -> Result<(), ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        delete_effect_if_status(
            unit,
            &super::persist::collection_effect_id_for_session(session_id),
            ContinuityEffectStatus::NotExecuted,
        )?;
        unit.request_commit();
        Ok(())
    })
}

/// Atomically persist collected live evidence and consume the admitted session.
/// Replay, an existing evidence key, or a consumed session fail before write.
pub fn commit_collected_qualification(
    store: &ConversationStore,
    session: &StoredEvaluationSession,
    responsibility_id: &str,
    identity_key: &str,
    payload: &str,
    evidence_class: &str,
) -> Result<StoredEvaluationSession, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        let current =
            load_evaluation_session_in_unit(unit, &session.session_id)?.ok_or_else(|| {
                continuity_failure(
                    ContinuityFailureCode::InvalidRequest,
                    ContinuityFailureStage::ContinuityAdmission,
                )
            })?;
        if current.consumed {
            return Err(continuity_failure(
                ContinuityFailureCode::IdempotencyConflict,
                ContinuityFailureStage::ContinuityCommit,
            ));
        }
        if !current.same_admitted_binding(session) || current.responsibility_id != responsibility_id
        {
            return Err(continuity_failure(
                ContinuityFailureCode::InvalidRequest,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
        if load_qualification_evidence_row(unit, responsibility_id, identity_key)?.is_some() {
            return Err(continuity_failure(
                ContinuityFailureCode::IdempotencyConflict,
                ContinuityFailureStage::ContinuityCommit,
            ));
        }
        persist_qualification_evidence(
            unit,
            responsibility_id,
            identity_key,
            payload,
            evidence_class,
        )?;
        record_effect(
            unit,
            &current.conversation_id,
            None,
            &super::persist::collection_effect_id_for_session(&current.session_id),
            ContinuityEffectStatus::Executed,
        )?;
        let mut consumed = current;
        consumed.consumed = true;
        persist_evaluation_session_in_unit(unit, &consumed)?;
        unit.request_commit();
        Ok(consumed)
    })
}

pub fn admit_evaluation_session(
    store: &ConversationStore,
    conversation_id: &str,
    owner_membership_id: &str,
    recipient_membership_id: &str,
    responsibility_id: &str,
    identity: ContinuityCandidateIdentity,
    policy_revision: &str,
) -> Result<StoredEvaluationSession, ContinuityFailure> {
    let authority = resolve_stored_owner_authority(store, conversation_id, owner_membership_id)?;
    if policy_revision.trim().is_empty()
        || recipient_membership_id.trim().is_empty()
        || responsibility_id != format!("{conversation_id}:{recipient_membership_id}")
    {
        return Err(continuity_failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    let corpus = load_evaluation_corpus(store, authority.conversation_id())?;
    let (dataset_id, corpus_version, identity) = match corpus {
        Some(corpus) => {
            let mut identity = identity;
            identity.dataset_version = corpus.version_digest.clone();
            (corpus.dataset_id, corpus.version_digest, identity)
        }
        None => (String::new(), String::new(), identity),
    };
    let session = StoredEvaluationSession {
        session_id: new_continuity_id("evaluation-session"),
        conversation_id: authority.conversation_id().to_owned(),
        owner_membership_id: authority.owner_membership_id().to_owned(),
        owner_principal_id: authority.owner_principal_id().to_owned(),
        recipient_membership_id: recipient_membership_id.to_owned(),
        responsibility_id: responsibility_id.to_owned(),
        identity,
        policy_revision: policy_revision.to_owned(),
        dataset_id,
        corpus_version,
        consumed: false,
    };
    persist_evaluation_session(store, &session)?;
    Ok(session)
}

pub fn admit_evaluation_corpus(
    store: &ConversationStore,
    conversation_id: &str,
    owner_membership_id: &str,
    dataset_id: &str,
    cases: Vec<StoredEvaluationCase>,
) -> Result<StoredEvaluationCorpus, ContinuityFailure> {
    let authority = resolve_stored_owner_authority(store, conversation_id, owner_membership_id)?;
    if dataset_id.trim().is_empty() || cases.is_empty() {
        return Err(continuity_failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for case in &cases {
        if case.case_id.trim().is_empty()
            || case.family.trim().is_empty()
            || case.subgroup.trim().is_empty()
            || case.input.trim().is_empty()
            || !seen.insert(case.case_id.as_str())
        {
            return Err(continuity_failure(
                ContinuityFailureCode::InvalidRequest,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
    }
    let version_digest = evaluation_corpus_version_digest(dataset_id, &cases)?;
    let corpus = StoredEvaluationCorpus {
        conversation_id: authority.conversation_id().to_owned(),
        owner_membership_id: authority.owner_membership_id().to_owned(),
        owner_principal_id: authority.owner_principal_id().to_owned(),
        dataset_id: dataset_id.to_owned(),
        version_digest,
        cases,
    };
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        persist_evaluation_corpus_in_unit(unit, &corpus)?;
        unit.request_commit();
        Ok(())
    })?;
    Ok(corpus)
}

pub fn load_evaluation_corpus(
    store: &ConversationStore,
    conversation_id: &str,
) -> Result<Option<StoredEvaluationCorpus>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        load_evaluation_corpus_in_unit(unit, conversation_id)
    })
}

pub fn persist_adoption_enabled(
    store: &ConversationStore,
    conversation_id: &str,
    owner_membership_id: &str,
    enabled: bool,
) -> Result<(bool, String), ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        admit_local_owner_principal(unit, conversation_id, owner_membership_id)?;
        persist_schema_value(
            unit,
            super::persist::ADOPTION_ENABLED_KEY,
            if enabled { "1" } else { "0" },
        )?;
        let stage = load_schema_value(unit, super::persist::ADOPTION_STAGE_KEY)?
            .unwrap_or_else(|| super::persist::ADOPTION_STAGE_OFFLINE.to_owned());
        unit.request_commit();
        Ok((enabled, stage))
    })
}

pub fn list_completion_notification_ids(
    store: &ConversationStore,
) -> Result<Vec<String>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        list_completion_ids(unit)
    })
}

#[derive(Clone, Debug)]
pub struct CompletionNoticeView {
    pub notification_id: String,
    pub goal_id: String,
    pub parent_conversation_id: String,
    pub child_conversation_id: String,
    pub card_event_id: String,
    pub card_sequence: i64,
}

fn notice_view(
    notification_id: String,
    goal_id: String,
    relation: &ContinuityTaskConversationRelation,
) -> CompletionNoticeView {
    CompletionNoticeView {
        notification_id,
        goal_id,
        parent_conversation_id: relation.parent_conversation_id.clone(),
        child_conversation_id: relation.child_conversation_id.clone(),
        card_event_id: relation.card_anchor.event_id.clone(),
        card_sequence: relation.card_anchor.sequence,
    }
}

pub fn list_pending_completion_notices(
    store: &ConversationStore,
    conversation_id: &str,
    owner_membership_id: &str,
) -> Result<Vec<CompletionNoticeView>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        let principal = admit_local_owner_principal(unit, conversation_id, owner_membership_id)?;
        let rows =
            list_qualified_pending_completion_rows(unit, &principal, CONTINUITY_MAX_PAGE_SIZE)?;
        let mut notices = Vec::new();
        for row in rows {
            let Some(relation) = load_relation(unit, &row.goal_id)? else {
                continue;
            };
            notices.push(notice_view(row.notification_id, row.goal_id, &relation));
        }
        Ok(notices)
    })
}

pub fn ack_completion_notices(
    store: &ConversationStore,
    conversation_id: &str,
    owner_membership_id: &str,
    notification_ids: &[String],
) -> Result<Vec<String>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        let principal = admit_local_owner_principal(unit, conversation_id, owner_membership_id)?;
        let bounded = notification_ids
            .iter()
            .filter(|id| !id.trim().is_empty())
            .take(CONTINUITY_MAX_PAGE_SIZE)
            .cloned()
            .collect::<Vec<_>>();
        let rows = list_qualified_completion_rows_for_ids(unit, &principal, &bounded)?;
        let eligible = rows
            .into_iter()
            .map(|row| row.notification_id)
            .collect::<Vec<_>>();
        consume_completion_ids(unit, &eligible)?;
        if !eligible.is_empty() {
            unit.request_commit();
        }
        Ok(eligible)
    })
}

pub fn resolve_completion_notice(
    store: &ConversationStore,
    conversation_id: &str,
    owner_membership_id: &str,
    notification_id: &str,
) -> Result<CompletionNoticeView, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        let principal = admit_local_owner_principal(unit, conversation_id, owner_membership_id)?;
        let row =
            load_qualified_completion_row(unit, &principal, notification_id)?.ok_or_else(|| {
                continuity_failure(
                    ContinuityFailureCode::ScopeDenied,
                    ContinuityFailureStage::ContinuityAdmission,
                )
            })?;
        let relation = load_relation(unit, &row.goal_id)?.ok_or_else(|| {
            continuity_failure(
                ContinuityFailureCode::SourceUnavailable,
                ContinuityFailureStage::ContinuityAdmission,
            )
        })?;
        Ok(notice_view(row.notification_id, row.goal_id, &relation))
    })
}

pub fn list_all_parent_grants(
    store: &ConversationStore,
) -> Result<Vec<ContinuityParentContextGrant>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        list_all_parent_grants_unit(unit)
    })
}

pub fn count_cancel_effects(
    store: &ConversationStore,
    conversation_id: &str,
) -> Result<i64, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        count_cancel_effects_unit(unit, conversation_id)
    })
}

pub fn enqueue_review_wake(
    store: &ConversationStore,
    conversation_id: &str,
    wake: &ContinuityWake,
) -> Result<bool, ContinuityFailure> {
    admit_wake(wake)?;
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        let inserted = enqueue_wake(unit, wake, conversation_id)?;
        unit.request_commit();
        Ok(inserted)
    })
}

pub fn record_ingress_execution(
    store: &ConversationStore,
    conversation_id: &str,
    source_event_id: &str,
    recipient_membership_id: &str,
    designation: &str,
) -> Result<(), ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        write_ingress_execution(
            unit,
            conversation_id,
            source_event_id,
            recipient_membership_id,
            designation,
        )?;
        unit.request_commit();
        Ok(())
    })
}

pub fn ingress_execution_recorded(
    store: &ConversationStore,
    conversation_id: &str,
    source_event_id: &str,
    recipient_membership_id: &str,
    designation: &str,
) -> Result<bool, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        load_ingress_execution(
            unit,
            conversation_id,
            source_event_id,
            recipient_membership_id,
            designation,
        )
    })
}

pub fn source_is_revoked_now(
    store: &ConversationStore,
    conversation_id: &str,
    source: &ContinuitySourceRef,
) -> Result<bool, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        source_is_revoked(unit, conversation_id, source)
    })
}

pub fn list_unknown_effect_ids(
    store: &ConversationStore,
) -> Result<Vec<(String, String, Option<String>)>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        unit.query_vec(
            "SELECT logical_effect_id, conversation_id, goal_id
             FROM continuity_effects WHERE status='unknown'
             ORDER BY logical_effect_id ASC",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(super::error::sql_failure)
    })
}

pub fn read_relation_for_child(
    store: &ConversationStore,
    child_conversation_id: &str,
) -> Result<Option<ContinuityTaskConversationRelation>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        load_relation_for_child(unit, child_conversation_id)
    })
}

pub fn list_due_goals(
    store: &ConversationStore,
    now: i64,
) -> Result<Vec<(String, String, i64)>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        list_due_goals_unit(unit, now)
    })
}

pub fn record_settlement_pending(
    store: &ConversationStore,
    conversation_id: &str,
    settlement_id: &str,
    payload: &Value,
) -> Result<(), ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        write_settlement_pending_unit(unit, conversation_id, settlement_id, payload)?;
        unit.request_commit();
        Ok(())
    })
}

pub fn record_settlement_applied(
    store: &ConversationStore,
    conversation_id: &str,
    settlement_id: &str,
    payload: &Value,
) -> Result<(), ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        write_settlement_applied_unit(unit, conversation_id, settlement_id, payload)?;
        unit.request_commit();
        Ok(())
    })
}

pub fn settlement_applied(
    store: &ConversationStore,
    conversation_id: &str,
    settlement_id: &str,
) -> Result<bool, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        settlement_applied_unit(unit, conversation_id, settlement_id)
    })
}

pub fn read_settlement_pending(
    store: &ConversationStore,
    conversation_id: &str,
    settlement_id: &str,
) -> Result<Option<Value>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        load_named_cursor_payload(
            unit,
            conversation_id,
            settlement_id,
            &super::persist::settlement_interpretation_key(
                settlement_id,
                SETTLEMENT_PENDING_DESIGNATION,
            ),
        )
    })
}

pub fn read_settlement_applied(
    store: &ConversationStore,
    conversation_id: &str,
    settlement_id: &str,
) -> Result<Option<Value>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        load_named_cursor_payload(
            unit,
            conversation_id,
            settlement_id,
            &super::persist::settlement_interpretation_key(
                settlement_id,
                SETTLEMENT_APPLIED_DESIGNATION,
            ),
        )
    })
}

pub fn list_unapplied_settlements(
    store: &ConversationStore,
) -> Result<Vec<(String, String, Value)>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        list_unapplied_settlements_unit(unit)
    })
}

pub fn record_child_work_started(
    store: &ConversationStore,
    conversation_id: &str,
    goal_id: &str,
    revision: i64,
) -> Result<(), ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        write_child_work_started_unit(unit, conversation_id, goal_id, revision)?;
        unit.request_commit();
        Ok(())
    })
}

pub fn child_work_started(
    store: &ConversationStore,
    conversation_id: &str,
    goal_id: &str,
    revision: i64,
) -> Result<bool, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        child_work_started_unit(unit, conversation_id, goal_id, revision)
    })
}

pub fn record_child_work_intent(
    store: &ConversationStore,
    conversation_id: &str,
    goal_id: &str,
    revision: i64,
    payload: &Value,
) -> Result<(), ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        write_child_work_intent_unit(unit, conversation_id, goal_id, revision, payload)?;
        unit.request_commit();
        Ok(())
    })
}

pub fn record_child_work_accepted(
    store: &ConversationStore,
    conversation_id: &str,
    goal_id: &str,
    revision: i64,
    payload: &Value,
) -> Result<(), ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        write_child_work_accepted_unit(unit, conversation_id, goal_id, revision, payload)?;
        unit.request_commit();
        Ok(())
    })
}

pub fn child_work_accepted(
    store: &ConversationStore,
    conversation_id: &str,
    goal_id: &str,
    revision: i64,
) -> Result<bool, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        child_work_accepted_unit(unit, conversation_id, goal_id, revision)
    })
}

pub fn read_child_work_accepted(
    store: &ConversationStore,
    conversation_id: &str,
    goal_id: &str,
    revision: i64,
) -> Result<Option<Value>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        load_named_cursor_payload(
            unit,
            conversation_id,
            goal_id,
            &super::persist::child_work_named_key(
                goal_id,
                revision,
                CHILD_WORK_ACCEPTED_DESIGNATION,
            ),
        )
    })
}

pub fn read_child_work_intent(
    store: &ConversationStore,
    conversation_id: &str,
    goal_id: &str,
    revision: i64,
) -> Result<Option<Value>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        load_child_work_intent(unit, conversation_id, goal_id, revision)
    })
}

pub fn list_unacked_child_work(
    store: &ConversationStore,
) -> Result<Vec<(String, String, i64, Value)>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        list_unacked_child_work_unit(unit)
    })
}

pub fn list_unacked_child_work_page(
    store: &ConversationStore,
    after_conversation_id: Option<&str>,
    after_goal_id: Option<&str>,
    after_interpretation_key: Option<&str>,
    limit: u64,
) -> Result<Vec<(String, String, i64, Value)>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        list_unacked_child_work_page_unit(
            unit,
            after_conversation_id,
            after_goal_id,
            after_interpretation_key,
            limit,
        )
    })
}

pub fn list_unapplied_settlements_page(
    store: &ConversationStore,
    after_conversation_id: Option<&str>,
    after_settlement_id: Option<&str>,
    limit: u64,
) -> Result<Vec<(String, String, Value)>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        list_unapplied_settlements_page_unit(
            unit,
            after_conversation_id,
            after_settlement_id,
            limit,
        )
    })
}

pub fn record_child_work_live(
    store: &ConversationStore,
    conversation_id: &str,
    goal_id: &str,
    payload: &Value,
) -> Result<(), ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        write_child_work_live_unit(unit, conversation_id, goal_id, payload)?;
        unit.request_commit();
        Ok(())
    })
}

pub fn read_child_work_live(
    store: &ConversationStore,
    conversation_id: &str,
    goal_id: &str,
) -> Result<Option<Value>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        load_child_work_live_unit(unit, conversation_id, goal_id)
    })
}

pub fn clear_child_work_live(
    store: &ConversationStore,
    conversation_id: &str,
    goal_id: &str,
) -> Result<(), ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        delete_child_work_live_unit(unit, conversation_id, goal_id)?;
        unit.request_commit();
        Ok(())
    })
}

pub fn list_child_work_live(
    store: &ConversationStore,
) -> Result<Vec<(String, String, Value)>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        list_child_work_live_unit(unit)
    })
}

pub fn read_oldest_pending_child_work(
    store: &ConversationStore,
    conversation_id: &str,
    goal_id: &str,
) -> Result<Option<(i64, Value)>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        load_oldest_pending_child_work_unit(unit, conversation_id, goal_id)
    })
}

pub fn find_admitted_parent_grant(
    store: &ConversationStore,
    recipient_conversation_id: &str,
    recipient_membership_id: &str,
    source_conversation_id: &str,
    requested: &ContinuitySourceRef,
    basis: &ContinuityParentGrantBasis,
) -> Result<Option<ContinuityParentContextGrant>, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        find_admitted_parent_grant_unit(
            unit,
            recipient_conversation_id,
            recipient_membership_id,
            source_conversation_id,
            requested,
            basis,
        )
    })
}

pub fn pending_obligation_scan_evidence(
    store: &ConversationStore,
) -> Result<(Vec<String>, Vec<String>, i64, i64, usize), ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        pending_obligation_scan_evidence_unit(unit)
    })
}

pub fn schedule_goal_due(
    store: &ConversationStore,
    conversation_id: &str,
    goal_id: &str,
    due_at: i64,
    review_policy: &str,
    responsible_party: &str,
) -> Result<ContinuityGoalProgress, ContinuityFailure> {
    run_unit(store, |unit| {
        migrate_or_fail(unit)?;
        let progress = set_goal_wait_due_unit(
            unit,
            conversation_id,
            goal_id,
            due_at,
            review_policy,
            responsible_party,
        )?;
        unit.request_commit();
        Ok(progress)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_conversation::{EventKind, EventPartKind, Principal, PrincipalKind};
    use crate::continuity::generated::{
        ContinuityCriterion, ContinuityEvidenceResult, ContinuityMatterAssociation,
        ContinuityMatterStatus, ContinuityMatterSubject, ContinuityOracleKind,
        ContinuityVerificationKind, ContinuityWriteEnvelope,
    };
    use crate::continuity::ports::{ContinuityCommitPort, ContinuityReadPort};
    use crate::store::NewEventPart;

    const MATTER: &str = "matter:notes";
    const GOAL: &str = "goal:notes";

    fn human() -> Principal {
        Principal {
            id: "principal:human".into(),
            kind: PrincipalKind::Human,
            display_name: "Fixture human".into(),
            agent_id: None,
            created_at_unix_ms: 1,
        }
    }

    fn digest(seed: char) -> String {
        format!("sha256:{}", seed.to_string().repeat(64))
    }

    fn source_ref(event_id: &str, sequence: i64, seed: char) -> ContinuitySourceRef {
        ContinuitySourceRef {
            owner_kind: ContinuitySourceOwnerKind::Event,
            opaque_id: event_id.to_owned(),
            part_id: None,
            span: None,
            source_revision: sequence,
            digest: digest(seed),
            visibility_scope: ContinuityVisibilityScope::Conversation,
            validity: ContinuitySourceValidity::Current,
        }
    }

    fn criterion(id: &str, required: bool, rule: &str) -> ContinuityCriterion {
        ContinuityCriterion {
            id: id.into(),
            description_ref: source_ref("event:criterion", 1, 'b'),
            required,
            oracle_kind: ContinuityOracleKind::Machine,
            artifact_version_rule: rule.into(),
            freshness_rule: "current".into(),
            evaluator_policy: "goal-evaluation".into(),
        }
    }

    fn evidence(
        event: &crate::client_conversation::ConversationEvent,
        criterion_id: &str,
    ) -> ContinuityEvidenceRef {
        ContinuityEvidenceRef {
            source: source_ref(&event.id, event.sequence, 'c'),
            issuer: "member:worker".into(),
            subject_version: 1,
            criterion_id: criterion_id.into(),
            observed_at: 10,
            result: ContinuityEvidenceResult::Pass,
            verification_kind: ContinuityVerificationKind::Deterministic,
            scope: ContinuityVisibilityScope::Goal,
            validity: ContinuitySourceValidity::Current,
        }
    }

    /// One deterministic dialogue: a canonical conversation whose events and
    /// proposals stand in for what a user and the Assistant actually say.
    struct Dialogue {
        store: ConversationStore,
        conversation_id: String,
        event_id: String,
        event_sequence: i64,
    }

    impl Dialogue {
        fn new(title: &str) -> Self {
            let store = ConversationStore::open_in_memory().unwrap();
            let conversation = store.create_conversation(title, human()).unwrap();
            let owner = conversation.memberships[0].id.clone();
            let event = store
                .append_event(
                    &conversation.id,
                    Some(&owner),
                    EventKind::Message,
                    &[NewEventPart {
                        id: String::new(),
                        kind: EventPartKind::Text,
                        content: "fixture dialogue".into(),
                    }],
                    None,
                    None,
                    true,
                )
                .unwrap();
            store.ensure_continuity_migrated().unwrap();
            Self {
                store,
                conversation_id: conversation.id,
                event_id: event.id,
                event_sequence: event.sequence,
            }
        }

        /// A dialogue turn: one speech act, one matter association and one
        /// commitment whose expected result the proposer chose.
        fn turn(
            &self,
            request_id: &str,
            speech_act: ContinuitySpeechAct,
            expected_result: &str,
            create_goal: bool,
            criteria: Vec<ContinuityCriterion>,
        ) -> ContinuityInterpretationProposal {
            let basis = self
                .store
                .continuity_commit_basis(&self.conversation_id)
                .unwrap();
            ContinuityInterpretationProposal {
                envelope: ContinuityWriteEnvelope {
                    conversation_id: self.conversation_id.clone(),
                    source_event_refs: vec![source_ref(&self.event_id, self.event_sequence, 'a')],
                    observed_revision: basis.revision,
                    designation_epoch: basis.designation_epoch,
                    request_id: request_id.to_owned(),
                },
                matter_associations: vec![ContinuityMatterAssociation {
                    matter_id: MATTER.into(),
                    source_ref: source_ref(&self.event_id, self.event_sequence, 'a'),
                    association_revision: 1,
                    proposed_by: "membership:fixture".into(),
                    reason_code: "user-intent".into(),
                    supersedes: None,
                }],
                speech_act,
                commitment_proposals: vec![ContinuityCommitmentProposal {
                    matter_id: Some(MATTER.into()),
                    subject: ContinuityMatterSubject::New,
                    expected_result: expected_result.into(),
                    criteria,
                    create_goal,
                }],
                agreement_proposals: Vec::new(),
                capability_needs: Vec::new(),
                uncertainty_reasons: Vec::new(),
                requested_reads: Vec::new(),
                task_child_admission: None,
            }
        }

        fn says(&self, proposal: &ContinuityInterpretationProposal) {
            self.store.commit(proposal).unwrap();
        }

        fn goal(&self) -> Option<(ContinuityGoalContract, ContinuityGoalProgress)> {
            read_goal_bundle(&self.store, GOAL).unwrap()
        }

        fn pending_notifications(&self) -> i64 {
            read_pending_outbox(&self.store, &self.conversation_id).unwrap()
        }

        fn matters(&self) -> Vec<ContinuityMatter> {
            self.store
                .list_matters(&self.conversation_id, None, 8)
                .unwrap()
        }
    }

    #[test]
    fn ordinary_chat_keeps_its_matter_and_admits_no_goal() {
        let dialogue = Dialogue::new("intent-chat");
        dialogue.says(&dialogue.turn(
            "request:chat",
            ContinuitySpeechAct::Exploration,
            "reply",
            false,
            Vec::new(),
        ));
        assert!(dialogue.goal().is_none());
        assert_eq!(dialogue.matters().len(), 1);
        assert_eq!(dialogue.matters()[0].id, MATTER);
        assert_eq!(dialogue.matters()[0].status, ContinuityMatterStatus::Open);
        assert_eq!(dialogue.pending_notifications(), 0);
    }

    #[test]
    fn questions_and_hypotheticals_stay_ordinary_even_when_marked_as_goals() {
        for (index, speech_act) in [
            ContinuitySpeechAct::Question,
            ContinuitySpeechAct::Hypothetical,
            ContinuitySpeechAct::Quotation,
        ]
        .into_iter()
        .enumerate()
        {
            let dialogue = Dialogue::new(&format!("intent-nonassertive-{index}"));
            dialogue.says(&dialogue.turn(
                "request:nonassertive",
                speech_act,
                "Draft the notes",
                true,
                Vec::new(),
            ));
            assert!(dialogue.goal().is_none(), "{speech_act:?} admitted a goal");
            assert_eq!(dialogue.pending_notifications(), 0);
        }
    }

    #[test]
    fn explicit_commitment_admits_one_goal_and_restating_it_changes_nothing() {
        let dialogue = Dialogue::new("intent-commit");
        dialogue.says(&dialogue.turn(
            "request:commit-1",
            ContinuitySpeechAct::Delegation,
            "Draft the notes",
            true,
            Vec::new(),
        ));
        let (contract, progress) = dialogue.goal().expect("explicit commitment admits a goal");
        assert_eq!(contract.contract_revision, 1);
        assert_eq!(progress.revision, 1);
        assert_eq!(progress.lifecycle, ContinuityGoalLifecycle::Active);
        assert_eq!(dialogue.pending_notifications(), 1);

        dialogue.says(&dialogue.turn(
            "request:commit-2",
            ContinuitySpeechAct::Delegation,
            "Draft the notes",
            true,
            Vec::new(),
        ));
        let (restated_contract, restated_progress) = dialogue.goal().unwrap();
        assert_eq!(restated_contract.contract_revision, 1);
        assert_eq!(restated_contract, contract);
        assert_eq!(restated_progress, progress);
        assert_eq!(
            dialogue.pending_notifications(),
            1,
            "a repeated commitment must not notify twice"
        );
    }

    #[test]
    fn user_correction_amends_the_goal_instead_of_duplicating_it() {
        let dialogue = Dialogue::new("intent-correction");
        dialogue.says(&dialogue.turn(
            "request:correction-1",
            ContinuitySpeechAct::Delegation,
            "Draft the notes",
            true,
            vec![criterion("criterion:notes-done", true, "exact-source")],
        ));
        dialogue.says(&dialogue.turn(
            "request:correction-2",
            ContinuitySpeechAct::Correction,
            "Draft the notes and send them",
            true,
            vec![criterion("criterion:notes-done", true, "exact-source")],
        ));
        let (contract, progress) = dialogue.goal().unwrap();
        assert_eq!(contract.expected_result, "Draft the notes and send them");
        assert_eq!(contract.contract_revision, 2);
        assert_eq!(progress.revision, 2);
        assert_eq!(dialogue.pending_notifications(), 2);
        assert_eq!(dialogue.matters().len(), 1);
    }

    #[test]
    fn correction_keeps_evidence_only_for_unchanged_criteria() {
        let dialogue = Dialogue::new("intent-evidence");
        dialogue.says(&dialogue.turn(
            "request:evidence-1",
            ContinuitySpeechAct::Delegation,
            "Draft the notes",
            true,
            vec![criterion("criterion:notes-done", true, "exact-source")],
        ));
        let observed = dialogue
            .store
            .append_event(
                &dialogue.conversation_id,
                None,
                EventKind::Message,
                &[NewEventPart {
                    id: String::new(),
                    kind: EventPartKind::Text,
                    content: "notes reviewed".into(),
                }],
                None,
                None,
                true,
            )
            .unwrap();
        append_criterion_evidence(
            &dialogue.store,
            &dialogue.conversation_id,
            GOAL,
            evidence(&observed, "criterion:notes-done"),
        )
        .unwrap();
        assert_eq!(dialogue.goal().unwrap().1.criterion_evidence_refs.len(), 1);

        // Same criteria, changed expectation: the sign-off still applies.
        dialogue.says(&dialogue.turn(
            "request:evidence-2",
            ContinuitySpeechAct::Correction,
            "Draft the notes and send them",
            true,
            vec![criterion("criterion:notes-done", true, "exact-source")],
        ));
        let (contract, progress) = dialogue.goal().unwrap();
        assert_eq!(contract.contract_revision, 2);
        assert_eq!(progress.criterion_evidence_refs.len(), 1);

        // Redefined criterion: the previous sign-off cannot carry over.
        dialogue.says(&dialogue.turn(
            "request:evidence-3",
            ContinuitySpeechAct::Correction,
            "Draft the notes and send them",
            true,
            vec![criterion("criterion:notes-done", true, "latest-source")],
        ));
        let (contract, progress) = dialogue.goal().unwrap();
        assert_eq!(contract.contract_revision, 3);
        assert_eq!(progress.revision, 4);
        assert!(progress.criterion_evidence_refs.is_empty());
    }

    #[test]
    fn a_paused_goal_still_refuses_new_work_and_keeps_its_revision() {
        let dialogue = Dialogue::new("intent-paused");
        dialogue.says(&dialogue.turn(
            "request:paused-1",
            ContinuitySpeechAct::Delegation,
            "Draft the notes",
            true,
            Vec::new(),
        ));
        apply_goal_control(
            &dialogue.store,
            &dialogue.conversation_id,
            GOAL,
            ContinuityGoalEvent::Pause,
        )
        .unwrap();
        let paused = dialogue.goal().unwrap().1;
        assert_eq!(paused.control, ContinuityGoalControl::Paused);
        let revision = paused.revision;
        let error = dialogue
            .store
            .commit(&dialogue.turn(
                "request:paused-2",
                ContinuitySpeechAct::Delegation,
                "Draft the notes again",
                true,
                Vec::new(),
            ))
            .unwrap_err();
        assert_eq!(error.code, ContinuityFailureCode::InvalidRequest);
        assert_eq!(dialogue.goal().unwrap().1.revision, revision);
    }

    #[test]
    fn a_handoff_moves_the_work_without_losing_it_or_letting_the_old_owner_rewrite_it() {
        let dialogue = Dialogue::new("intent-handoff");
        dialogue.says(&dialogue.turn(
            "request:handoff-1",
            ContinuitySpeechAct::Delegation,
            "Draft the notes",
            true,
            vec![criterion("criterion:notes-done", true, "exact-source")],
        ));
        let (before_contract, before_progress) = dialogue.goal().expect("admitted goal");
        let notifications = dialogue.pending_notifications();
        let observed_before_handoff = dialogue
            .store
            .continuity_commit_basis(&dialogue.conversation_id)
            .unwrap()
            .revision;

        let handed = hand_off_goal_owner(
            &dialogue.store,
            &dialogue.conversation_id,
            GOAL,
            "membership:successor",
        )
        .unwrap()
        .expect("an unfinished goal changes owner");
        let (contract, progress) = dialogue.goal().unwrap();
        assert_eq!(contract.responsible_role_ref, "membership:successor");
        assert_eq!(
            contract.contract_revision,
            before_contract.contract_revision + 1
        );
        assert_eq!(progress.revision, before_progress.revision + 1);
        // The agreement itself is not rewritten: result, criteria and evidence.
        assert_eq!(contract.expected_result, before_contract.expected_result);
        assert_eq!(contract.criteria, before_contract.criteria);
        assert_eq!(
            progress.criterion_evidence_refs,
            before_progress.criterion_evidence_refs
        );
        assert_eq!(progress.lifecycle, before_progress.lifecycle);
        assert_eq!(handed.goal_id, GOAL);
        assert_eq!(
            dialogue.pending_notifications(),
            notifications + 1,
            "the new owner is announced the goal exactly once"
        );

        // Restating the same handoff is idempotent.
        assert!(
            hand_off_goal_owner(
                &dialogue.store,
                &dialogue.conversation_id,
                GOAL,
                "membership:successor"
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(dialogue.pending_notifications(), notifications + 1);

        // A proposal observed before the handoff is stale, so the previous
        // owner cannot overwrite the work the successor now holds.
        let mut stale = dialogue.turn(
            "request:handoff-stale",
            ContinuitySpeechAct::Delegation,
            "Draft something else",
            true,
            Vec::new(),
        );
        stale.envelope.observed_revision = observed_before_handoff;
        let error = dialogue.store.commit(&stale).unwrap_err();
        assert_eq!(error.code, ContinuityFailureCode::StaleRevision);
        assert_eq!(
            dialogue.goal().unwrap().0.expected_result,
            "Draft the notes"
        );
    }

    #[test]
    fn a_closed_goal_is_not_handed_over() {
        let dialogue = Dialogue::new("intent-handoff-closed");
        dialogue.says(&dialogue.turn(
            "request:handoff-closed",
            ContinuitySpeechAct::Delegation,
            "Draft the notes",
            true,
            Vec::new(),
        ));
        let (_, progress) = dialogue.goal().unwrap();
        apply_goal_control(
            &dialogue.store,
            &dialogue.conversation_id,
            GOAL,
            ContinuityGoalEvent::CancelRequest,
        )
        .unwrap();
        apply_goal_control(
            &dialogue.store,
            &dialogue.conversation_id,
            GOAL,
            ContinuityGoalEvent::CancelSettled,
        )
        .unwrap();
        let closed = dialogue.goal().unwrap().1;
        assert_eq!(closed.lifecycle, ContinuityGoalLifecycle::Cancelled);
        assert!(closed.revision > progress.revision);
        assert!(
            hand_off_goal_owner(
                &dialogue.store,
                &dialogue.conversation_id,
                GOAL,
                "membership:successor"
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(dialogue.goal().unwrap().1, closed);
    }
}
