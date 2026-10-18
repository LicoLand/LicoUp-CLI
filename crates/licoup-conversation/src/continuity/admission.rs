//! Deterministic admission for continuity writes.
//!
//! UTF-8 span checks use `str::is_char_boundary`, the same well-formedness
//! test used by rustc and encoding_rs. Spans are UTF-8 byte ranges, not
//! UTF-16 indexes or Python code-point indexes.

use super::generated::{
    CONTINUITY_MAX_PAGE_SIZE, ContinuityClosureAuthorityKind, ContinuityCommitBasis,
    ContinuityCommitmentProposal, ContinuityContextCompositionRequest, ContinuityCriterion,
    ContinuityDecisionLayer, ContinuityEffectClass, ContinuityEvidenceRef,
    ContinuityEvidenceResult, ContinuityFailure, ContinuityFailureCode, ContinuityFailureStage,
    ContinuityFollowThroughKind, ContinuityGoalCompletionTransition, ContinuityGoalContract,
    ContinuityGoalControl, ContinuityGoalLifecycle, ContinuityGoalProgress,
    ContinuityParentCardAnchor, ContinuityParentContextGrant, ContinuityParentGrantBasis,
    ContinuityParentGrantStatus, ContinuityRecoveryClass, ContinuitySourceOwnerKind,
    ContinuitySourceRef, ContinuitySourceValidity, ContinuitySpeechAct,
    ContinuityTaskChildAdmission, ContinuityTaskConversationRelation, ContinuityTaskListingKind,
    ContinuityUtf8ByteSpan, ContinuityVisibilityScope, ContinuityWake, ContinuityWriteEnvelope,
};
use serde_json::Value;
use std::collections::BTreeSet;

use super::lifecycle::suppresses_new_work;

fn failure(code: ContinuityFailureCode, stage: ContinuityFailureStage) -> ContinuityFailure {
    let recovery = match code {
        ContinuityFailureCode::StaleRevision | ContinuityFailureCode::DesignationChanged => {
            ContinuityRecoveryClass::RecomputeProposal
        }
        ContinuityFailureCode::ReconciliationRequired | ContinuityFailureCode::PrematureClosure => {
            ContinuityRecoveryClass::ReconcileEffects
        }
        ContinuityFailureCode::ApprovalRequired => ContinuityRecoveryClass::ObtainApproval,
        ContinuityFailureCode::UnsupportedCapability
        | ContinuityFailureCode::SourceUnavailable
        | ContinuityFailureCode::WriterBusy => ContinuityRecoveryClass::ReviewOrWait,
        _ => ContinuityRecoveryClass::CorrectRequest,
    };
    ContinuityFailure {
        code,
        stage,
        recovery,
        effect_class: ContinuityEffectClass::None,
        decision_layer: ContinuityDecisionLayer::Admission,
        retryable: false,
    }
}

pub fn admit_utf8_span(
    text: &str,
    start_byte: u64,
    end_byte: u64,
) -> Result<(), ContinuityFailure> {
    let start = usize::try_from(start_byte).map_err(|_| {
        failure(
            ContinuityFailureCode::InvalidSpan,
            ContinuityFailureStage::ContinuityParse,
        )
    })?;
    let end = usize::try_from(end_byte).map_err(|_| {
        failure(
            ContinuityFailureCode::InvalidSpan,
            ContinuityFailureStage::ContinuityParse,
        )
    })?;
    if end < start
        || end > text.len()
        || !text.is_char_boundary(start)
        || !text.is_char_boundary(end)
    {
        return Err(failure(
            ContinuityFailureCode::InvalidSpan,
            ContinuityFailureStage::ContinuityParse,
        ));
    }
    Ok(())
}

pub fn admit_source_ref(
    authorized_scopes: &[ContinuityVisibilityScope],
    source: &ContinuitySourceRef,
) -> Result<(), ContinuityFailure> {
    if source.validity == ContinuitySourceValidity::Revoked {
        return Err(failure(
            ContinuityFailureCode::SourceRevoked,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    if !authorized_scopes.contains(&source.visibility_scope) {
        return Err(failure(
            ContinuityFailureCode::ScopeDenied,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    Ok(())
}

pub fn admit_versions(
    basis: &ContinuityCommitBasis,
    envelope: &ContinuityWriteEnvelope,
) -> Result<(), ContinuityFailure> {
    if envelope.conversation_id != basis.conversation_id {
        return Err(failure(
            ContinuityFailureCode::ScopeDenied,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    if envelope.observed_revision != basis.revision {
        return Err(failure(
            ContinuityFailureCode::StaleRevision,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    if envelope.designation_epoch != basis.designation_epoch {
        return Err(failure(
            ContinuityFailureCode::DesignationChanged,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    Ok(())
}

fn request_id_value(value: &Value) -> Option<&Value> {
    value.get("requestId").or_else(|| {
        value
            .get("envelope")
            .and_then(|envelope| envelope.get("requestId"))
    })
}

fn comparable_idempotency_body(value: &Value) -> Value {
    let mut body = value.clone();
    if let Some(object) = body.as_object_mut() {
        object.remove("requestId");
        if let Some(envelope) = object
            .get_mut("envelope")
            .and_then(|envelope| envelope.as_object_mut())
        {
            envelope.remove("requestId");
            envelope.remove("observedRevision");
            envelope.remove("designationEpoch");
        }
    }
    body
}

/// Same `requestId` must carry the same business payload. Comparison uses JSON
/// value equality after removing the idempotency key and CAS cursor fields so
/// a retry after a committed revision bump is not a conflict.
pub fn admit_idempotency(previous: &Value, current: &Value) -> Result<(), ContinuityFailure> {
    match (request_id_value(previous), request_id_value(current)) {
        (None, None) => {}
        (Some(previous_key), Some(current_key)) if previous_key == current_key => {}
        (Some(_), Some(_)) => return Ok(()),
        _ => {
            return Err(failure(
                ContinuityFailureCode::InvalidRequest,
                ContinuityFailureStage::ContinuityParse,
            ));
        }
    }
    if comparable_idempotency_body(previous) != comparable_idempotency_body(current) {
        return Err(failure(
            ContinuityFailureCode::IdempotencyConflict,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    Ok(())
}

pub fn admit_goal_progress(progress: &ContinuityGoalProgress) -> Result<(), ContinuityFailure> {
    let terminal = matches!(
        progress.lifecycle,
        ContinuityGoalLifecycle::Achieved
            | ContinuityGoalLifecycle::Cancelled
            | ContinuityGoalLifecycle::Superseded
    );
    if terminal && progress.control != ContinuityGoalControl::Enabled {
        return Err(failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    if terminal && (progress.next_attention.is_some() || !progress.active_execution_refs.is_empty())
    {
        return Err(failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    let needs_attention = !terminal
        && progress.control == ContinuityGoalControl::Enabled
        && progress.next_attention.is_none();
    if needs_attention {
        return Err(failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    Ok(())
}

fn is_terminal(lifecycle: ContinuityGoalLifecycle) -> bool {
    matches!(
        lifecycle,
        ContinuityGoalLifecycle::Achieved
            | ContinuityGoalLifecycle::Cancelled
            | ContinuityGoalLifecycle::Superseded
    )
}

/// How an admitted commitment proposal relates to the Goal already stored for
/// the same matter-derived identity.
///
/// Ordinary conversation stays ordinary: a chat reply, a question, hypothetical
/// or quoted material never creates durable work, even when a proposal marks a
/// commitment as goal-creating. An explicit ongoing commitment admits exactly
/// one Goal; repeating it is idempotent and re-states no notification; changed
/// content amends the stored Goal in place instead of inventing a second one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContinuityCommitmentAdmission {
    /// Ordinary conversation: no Goal is created or amended.
    Chat,
    /// No Goal is stored for this identity: admit one.
    Create,
    /// The stored Goal already expresses this commitment: reuse it unchanged.
    Reuse,
    /// The stored Goal expresses this identity with different content: amend it
    /// under a monotonic revision.
    Amend,
}

fn criterion_definitions_match(
    left: &[ContinuityCriterion],
    right: &[ContinuityCriterion],
) -> bool {
    let mut left = left.to_vec();
    let mut right = right.to_vec();
    left.sort_by(|a, b| a.id.cmp(&b.id));
    right.sort_by(|a, b| a.id.cmp(&b.id));
    left == right
}

/// Decide what an admitted commitment does to durable work. Identity comes from
/// the matter-derived Goal, so a repeated commitment cannot duplicate a Goal and
/// a corrected commitment cannot reset or invent one. A stored terminal or
/// suppressed Goal keeps refusing new work.
pub fn admit_commitment_admission(
    speech_act: ContinuitySpeechAct,
    commitment: &ContinuityCommitmentProposal,
    stored: Option<(&ContinuityGoalContract, &ContinuityGoalProgress)>,
) -> Result<ContinuityCommitmentAdmission, ContinuityFailure> {
    if !commitment.create_goal {
        return Ok(ContinuityCommitmentAdmission::Chat);
    }
    if matches!(
        speech_act,
        ContinuitySpeechAct::Question
            | ContinuitySpeechAct::Hypothetical
            | ContinuitySpeechAct::Quotation
            | ContinuitySpeechAct::Reference
    ) {
        return Ok(ContinuityCommitmentAdmission::Chat);
    }
    let Some((contract, progress)) = stored else {
        return Ok(ContinuityCommitmentAdmission::Create);
    };
    if is_terminal(progress.lifecycle) || suppresses_new_work(progress.control) {
        return Err(failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    if contract.expected_result == commitment.expected_result
        && criterion_definitions_match(&contract.criteria, &commitment.criteria)
    {
        return Ok(ContinuityCommitmentAdmission::Reuse);
    }
    Ok(ContinuityCommitmentAdmission::Amend)
}

/// Evidence survives a contract change only for criteria whose definition is
/// unchanged. A corrected or replaced criterion cannot keep its previous
/// sign-off, and a removed criterion keeps nothing.
pub fn retain_current_criterion_evidence(
    previous: &[ContinuityCriterion],
    next: &[ContinuityCriterion],
    evidence: &[ContinuityEvidenceRef],
) -> Vec<ContinuityEvidenceRef> {
    evidence
        .iter()
        .filter(|item| {
            let Some(previous_criterion) = previous.iter().find(|c| c.id == item.criterion_id)
            else {
                return false;
            };
            next.iter()
                .find(|c| c.id == item.criterion_id)
                .is_some_and(|next_criterion| next_criterion == previous_criterion)
        })
        .cloned()
        .collect()
}

fn granted_span_covers(
    allowed: Option<&ContinuityUtf8ByteSpan>,
    requested: Option<&ContinuityUtf8ByteSpan>,
) -> bool {
    match (allowed, requested) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(allowed), Some(requested)) => {
            requested.end_byte >= requested.start_byte
                && requested.start_byte >= allowed.start_byte
                && requested.end_byte <= allowed.end_byte
        }
    }
}

/// Trusted grant refs decide identity and provenance. Requested validity
/// cannot upgrade or replace that basis.
fn granted_source_covers(allowed: &ContinuitySourceRef, requested: &ContinuitySourceRef) -> bool {
    allowed.validity != ContinuitySourceValidity::Revoked
        && allowed.owner_kind == requested.owner_kind
        && allowed.opaque_id == requested.opaque_id
        && allowed.part_id == requested.part_id
        && allowed.source_revision == requested.source_revision
        && allowed.digest == requested.digest
        && allowed.visibility_scope == requested.visibility_scope
        && granted_span_covers(allowed.span.as_ref(), requested.span.as_ref())
}

/// Simple chat and immediate work never force a Canonical child Conversation.
/// Only admitted durable follow-through with a delegation speech act may.
pub fn admit_task_child_admission(
    admission: &ContinuityTaskChildAdmission,
) -> Result<(), ContinuityFailure> {
    if admission.follow_through_kind != ContinuityFollowThroughKind::Durable
        || admission.speech_act != ContinuitySpeechAct::Delegation
    {
        return Err(failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    if let Some(anchor) = &admission.observed_card_anchor {
        admit_card_anchor(anchor)?;
        if anchor.parent_conversation_id != admission.parent_conversation_id {
            return Err(failure(
                ContinuityFailureCode::IdentityConflict,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
    }
    Ok(())
}

pub fn admit_card_anchor(anchor: &ContinuityParentCardAnchor) -> Result<(), ContinuityFailure> {
    if anchor.sequence < 0 || anchor.parent_conversation_id.is_empty() || anchor.event_id.is_empty()
    {
        return Err(failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    Ok(())
}

/// Two long-running children in one parent keep distinct Event identities
/// and sequences. Completion must not swap or collide those anchors.
pub fn admit_sibling_card_order(
    left: &ContinuityParentCardAnchor,
    right: &ContinuityParentCardAnchor,
) -> Result<(), ContinuityFailure> {
    admit_card_anchor(left)?;
    admit_card_anchor(right)?;
    if left.parent_conversation_id != right.parent_conversation_id
        || left.event_id == right.event_id
        || left.sequence == right.sequence
    {
        return Err(failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    Ok(())
}

pub fn admit_card_identity_stable(
    previous: &ContinuityParentCardAnchor,
    next: &ContinuityParentCardAnchor,
) -> Result<(), ContinuityFailure> {
    admit_card_anchor(previous)?;
    admit_card_anchor(next)?;
    if previous.parent_conversation_id != next.parent_conversation_id
        || previous.event_id != next.event_id
        || previous.sequence != next.sequence
        || previous.part_id != next.part_id
    {
        return Err(failure(
            ContinuityFailureCode::IdentityConflict,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    Ok(())
}

pub fn admit_task_relation(
    relation: &ContinuityTaskConversationRelation,
    previous: Option<&ContinuityTaskConversationRelation>,
) -> Result<(), ContinuityFailure> {
    if relation.follow_through_kind != ContinuityFollowThroughKind::Durable
        || relation.listing_kind != ContinuityTaskListingKind::ChildTask
        || relation.parent_conversation_id == relation.child_conversation_id
        || relation.card_anchor.parent_conversation_id != relation.parent_conversation_id
        || relation.created_event.owner_kind != ContinuitySourceOwnerKind::Event
        || relation.created_event.opaque_id != relation.card_anchor.event_id
        || relation.created_event.part_id != relation.card_anchor.part_id
    {
        return Err(failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    admit_card_anchor(&relation.card_anchor)?;
    if let Some(transition) = &relation.completion_transition {
        if transition.goal_id != relation.goal_id
            || !is_terminal(transition.to_lifecycle)
            || is_terminal(transition.from_lifecycle)
        {
            return Err(failure(
                ContinuityFailureCode::InvalidRequest,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
    }
    if let Some(previous) = previous {
        if previous.goal_id != relation.goal_id
            || previous.parent_conversation_id != relation.parent_conversation_id
            || previous.child_conversation_id != relation.child_conversation_id
        {
            return Err(failure(
                ContinuityFailureCode::IdentityConflict,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
        admit_card_identity_stable(&previous.card_anchor, &relation.card_anchor)?;
    }
    Ok(())
}

/// Structural check that the intended transition and caller snapshot are
/// well-formed. A worker or PersistentTurn exit is not a closure authority.
/// Stored Goal identity, revision, lifecycle, and required current evidence are
/// admitted separately against the persisted record.
pub fn admit_completion_transition(
    transition: &ContinuityGoalCompletionTransition,
    progress: &ContinuityGoalProgress,
) -> Result<(), ContinuityFailure> {
    if !matches!(
        transition.authority_kind,
        ContinuityClosureAuthorityKind::GoalEvaluation
            | ContinuityClosureAuthorityKind::UserAcceptance
    ) || !is_terminal(transition.to_lifecycle)
        || is_terminal(transition.from_lifecycle)
        || transition.goal_id != progress.goal_id
    {
        return Err(failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    if progress.lifecycle != transition.to_lifecycle
        || progress.revision != transition.goal_revision
        || transition.evaluation_ref.validity == ContinuitySourceValidity::Revoked
    {
        return Err(failure(
            ContinuityFailureCode::PrematureClosure,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    admit_goal_progress(progress)?;
    Ok(())
}

pub(super) fn admit_completion_against_current(
    transition: &ContinuityGoalCompletionTransition,
    current: &ContinuityGoalProgress,
) -> Result<(), ContinuityFailure> {
    if transition.goal_id != current.goal_id {
        return Err(failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    if transition.goal_revision != current.revision {
        return Err(failure(
            ContinuityFailureCode::StaleRevision,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    if transition.from_lifecycle != current.lifecycle {
        return Err(failure(
            ContinuityFailureCode::PrematureClosure,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    Ok(())
}

fn current_subject_version(progress: &ContinuityGoalProgress, criterion_id: &str) -> Option<i64> {
    progress
        .criterion_evidence_refs
        .iter()
        .filter(|item| item.criterion_id == criterion_id)
        .map(|item| item.subject_version)
        .max()
}

fn evidence_is_current_valid_pass(item: &ContinuityEvidenceRef) -> bool {
    item.validity == ContinuitySourceValidity::Current
        && item.source.validity == ContinuitySourceValidity::Current
        && item.result == ContinuityEvidenceResult::Pass
}

fn evaluation_refers_to_evidence(
    evaluation_ref: &ContinuitySourceRef,
    evidence: &ContinuityEvidenceRef,
) -> bool {
    evaluation_ref.opaque_id == evidence.source.opaque_id
        && evaluation_ref.source_revision == evidence.source.source_revision
        && match (&evaluation_ref.part_id, &evidence.source.part_id) {
            (Some(left), Some(right)) => left == right,
            _ => true,
        }
}

pub(super) fn current_required_evidence<'a>(
    contract: &'a ContinuityGoalContract,
    progress: &'a ContinuityGoalProgress,
) -> Vec<&'a ContinuityEvidenceRef> {
    contract
        .criteria
        .iter()
        .filter(|criterion| criterion.required)
        .flat_map(|criterion| {
            let version = current_subject_version(progress, &criterion.id);
            progress.criterion_evidence_refs.iter().filter(move |item| {
                item.criterion_id == criterion.id && Some(item.subject_version) == version
            })
        })
        .collect()
}

pub(super) fn admit_achieved_required_current_evidence(
    transition: &ContinuityGoalCompletionTransition,
    contract: &ContinuityGoalContract,
    current: &ContinuityGoalProgress,
) -> Result<(), ContinuityFailure> {
    if transition.to_lifecycle != ContinuityGoalLifecycle::Achieved {
        return Ok(());
    }
    if transition.evaluation_ref.validity != ContinuitySourceValidity::Current {
        return Err(failure(
            ContinuityFailureCode::PrematureClosure,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    for criterion in contract.criteria.iter().filter(|item| item.required) {
        let Some(version) = current_subject_version(current, &criterion.id) else {
            return Err(failure(
                ContinuityFailureCode::PrematureClosure,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        };
        let current_items: Vec<&ContinuityEvidenceRef> = current
            .criterion_evidence_refs
            .iter()
            .filter(|item| item.criterion_id == criterion.id && item.subject_version == version)
            .collect();
        if current_items.is_empty()
            || !current_items
                .iter()
                .all(|item| evidence_is_current_valid_pass(item))
        {
            return Err(failure(
                ContinuityFailureCode::PrematureClosure,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
    }
    admit_evaluation_ref_against_required_current(transition, contract, current)
}

fn admit_evaluation_ref_against_required_current(
    transition: &ContinuityGoalCompletionTransition,
    contract: &ContinuityGoalContract,
    current: &ContinuityGoalProgress,
) -> Result<(), ContinuityFailure> {
    let required: Vec<_> = contract
        .criteria
        .iter()
        .filter(|item| item.required)
        .collect();
    if required.is_empty() {
        return Ok(());
    }
    if transition.evaluation_ref.owner_kind == ContinuitySourceOwnerKind::Goal
        && transition.evaluation_ref.opaque_id == transition.goal_id
        && transition.evaluation_ref.source_revision == current.revision
    {
        return Ok(());
    }
    let matched: Vec<&ContinuityEvidenceRef> = current
        .criterion_evidence_refs
        .iter()
        .filter(|item| {
            required
                .iter()
                .any(|criterion| criterion.id == item.criterion_id)
                && evaluation_refers_to_evidence(&transition.evaluation_ref, item)
        })
        .collect();
    if matched.is_empty() {
        return Err(failure(
            ContinuityFailureCode::PrematureClosure,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    if matched.iter().any(|item| {
        current_subject_version(current, &item.criterion_id) != Some(item.subject_version)
            || !evidence_is_current_valid_pass(item)
    }) {
        return Err(failure(
            ContinuityFailureCode::PrematureClosure,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    Ok(())
}

/// Parent Event/Part/span disclosure into a child requires an admitted grant
/// whose trusted recipient, membership and revocation generation match the
/// current store basis. Requested validity cannot widen that basis.
pub fn admit_parent_context_grant(
    grant: &ContinuityParentContextGrant,
    requested: &ContinuitySourceRef,
    basis: &ContinuityParentGrantBasis,
) -> Result<(), ContinuityFailure> {
    if grant.status != ContinuityParentGrantStatus::Admitted
        || grant.source_conversation_id == grant.recipient_conversation_id
        || grant.recipient_conversation_id != basis.recipient_conversation_id
        || grant.recipient_membership_id != basis.recipient_membership_id
    {
        return Err(failure(
            ContinuityFailureCode::ScopeDenied,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    if grant.revocation_generation != basis.revocation_generation {
        return Err(failure(
            ContinuityFailureCode::StaleRevision,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    let Some(allowed) = grant
        .source_refs
        .iter()
        .find(|candidate| granted_source_covers(candidate, requested))
    else {
        return Err(failure(
            ContinuityFailureCode::ScopeDenied,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    };
    admit_source_ref(&grant.authorized_scopes, allowed)
}

pub fn admit_page_limit(limit: u64) -> Result<usize, ContinuityFailure> {
    if limit == 0 || limit > CONTINUITY_MAX_PAGE_SIZE as u64 {
        return Err(failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    usize::try_from(limit).map_err(|_| {
        failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityAdmission,
        )
    })
}

pub fn admit_composition_request(
    request: &ContinuityContextCompositionRequest,
) -> Result<(), ContinuityFailure> {
    if request.conversation_id.is_empty() || request.recipient_membership_id.is_empty() {
        return Err(failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    admit_page_limit(request.limit)?;
    Ok(())
}

pub fn admit_wake(wake: &ContinuityWake) -> Result<(), ContinuityFailure> {
    if wake.logical_wake_id.trim().is_empty() || wake.goal_id.trim().is_empty() {
        return Err(failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    if wake.epoch < 0 || wake.goal_revision < 0 || wake.host_generation < 0 {
        return Err(failure(
            ContinuityFailureCode::InvalidRequest,
            ContinuityFailureStage::ContinuityAdmission,
        ));
    }
    for cause in &wake.cause_refs {
        if cause.opaque_id.trim().is_empty() || cause.digest.trim().is_empty() {
            return Err(failure(
                ContinuityFailureCode::InvalidRequest,
                ContinuityFailureStage::ContinuityAdmission,
            ));
        }
    }
    Ok(())
}

/// Review policy carried by the wake that announces a new responsible owner.
///
/// An ownership handoff is an explicit event, not a reminder: the new owner
/// must be engaged even though the Goal's recorded sources did not change.
pub const OWNER_HANDOFF_REVIEW_POLICY: &str = "owner-handoff";

/// Whether a pending wake re-reads only sources the Goal already recorded.
///
/// A reminder exists so the Assistant looks again; it is not by itself new
/// information. When no settlement arrived and every source the wake names is
/// already recorded by the Goal — at the same revision, or the wake names none
/// — reviewing it can only restate the status the Goal already holds. Such a
/// wake is settled deterministically instead of paying for a model call, and a
/// Goal that is still waiting keeps its responsibility, its revision and its
/// reachable pause, resume and cancel controls.
pub fn wake_repeats_recorded_sources(
    wake: &ContinuityWake,
    contract: &ContinuityGoalContract,
    progress: &ContinuityGoalProgress,
) -> bool {
    if wake.settlement.is_some() || wake.review_policy == OWNER_HANDOFF_REVIEW_POLICY {
        return false;
    }
    let mut recorded: BTreeSet<(&str, i64)> = BTreeSet::new();
    for source in contract
        .source_intent_refs
        .iter()
        .chain(contract.criteria.iter().map(|item| &item.description_ref))
        .chain(
            progress
                .criterion_evidence_refs
                .iter()
                .map(|item| &item.source),
        )
    {
        recorded.insert((source.opaque_id.as_str(), source.source_revision));
    }
    wake.cause_refs.iter().all(|source| {
        // The Goal naming itself, at or before its current revision, restates
        // its own identity rather than reporting anything new.
        (source.owner_kind == ContinuitySourceOwnerKind::Goal
            && source.opaque_id == progress.goal_id
            && source.source_revision <= progress.revision)
            || recorded.contains(&(source.opaque_id.as_str(), source.source_revision))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn criterion(id: &str, required: bool, rule: &str) -> ContinuityCriterion {
        ContinuityCriterion {
            id: id.into(),
            description_ref: ContinuitySourceRef {
                owner_kind: ContinuitySourceOwnerKind::Event,
                opaque_id: format!("event:{id}"),
                part_id: None,
                span: None,
                source_revision: 1,
                digest: format!("sha256:{}", "a".repeat(64)),
                visibility_scope: ContinuityVisibilityScope::Conversation,
                validity: ContinuitySourceValidity::Current,
            },
            required,
            oracle_kind: super::super::generated::ContinuityOracleKind::Machine,
            artifact_version_rule: rule.into(),
            freshness_rule: "current".into(),
            evaluator_policy: "goal-evaluation".into(),
        }
    }

    fn commitment(
        expected: &str,
        criteria: Vec<ContinuityCriterion>,
    ) -> ContinuityCommitmentProposal {
        ContinuityCommitmentProposal {
            matter_id: Some("matter:notes".into()),
            subject: super::super::generated::ContinuityMatterSubject::New,
            expected_result: expected.into(),
            criteria,
            create_goal: true,
        }
    }

    fn stored(
        expected: &str,
        criteria: Vec<ContinuityCriterion>,
    ) -> (ContinuityGoalContract, ContinuityGoalProgress) {
        let contract = ContinuityGoalContract {
            id: "goal:notes".into(),
            matter_id: "matter:notes".into(),
            source_intent_refs: Vec::new(),
            contract_revision: 3,
            expected_result: expected.into(),
            criteria,
            scope_refs: Vec::new(),
            responsible_role_ref: "role:assistant".into(),
            resource_envelope_ref: "envelope:goal:notes".into(),
            acceptance_method: "goal-evaluation".into(),
            created_event: criterion("c1", true, "exact-source").description_ref,
        };
        let progress = ContinuityGoalProgress {
            goal_id: "goal:notes".into(),
            revision: 4,
            lifecycle: ContinuityGoalLifecycle::Active,
            control: ContinuityGoalControl::Enabled,
            criterion_evidence_refs: Vec::new(),
            active_execution_refs: Vec::new(),
            blockers: Vec::new(),
            next_attention: Some(
                super::super::generated::ContinuityNextAttention::DispatchableStep {
                    step_ref: "step:goal:notes:0".into(),
                },
            ),
            closure_ref: None,
        };
        (contract, progress)
    }

    #[test]
    fn ordinary_chat_and_nonassertive_material_never_create_work() {
        let mut chat = commitment("reply", Vec::new());
        chat.create_goal = false;
        assert_eq!(
            admit_commitment_admission(ContinuitySpeechAct::Exploration, &chat, None).unwrap(),
            ContinuityCommitmentAdmission::Chat
        );
        for act in [
            ContinuitySpeechAct::Question,
            ContinuitySpeechAct::Hypothetical,
            ContinuitySpeechAct::Quotation,
            ContinuitySpeechAct::Reference,
        ] {
            assert_eq!(
                admit_commitment_admission(act, &commitment("draft", Vec::new()), None).unwrap(),
                ContinuityCommitmentAdmission::Chat,
                "{act:?} must stay ordinary conversation"
            );
        }
    }

    #[test]
    fn explicit_commitment_creates_once_and_restating_it_reuses() {
        let proposal = commitment("Draft the notes", Vec::new());
        assert_eq!(
            admit_commitment_admission(ContinuitySpeechAct::Delegation, &proposal, None).unwrap(),
            ContinuityCommitmentAdmission::Create
        );
        let (contract, progress) = stored("Draft the notes", Vec::new());
        assert_eq!(
            admit_commitment_admission(
                ContinuitySpeechAct::Delegation,
                &proposal,
                Some((&contract, &progress))
            )
            .unwrap(),
            ContinuityCommitmentAdmission::Reuse
        );
        // Criteria order is not identity.
        let ordered = commitment(
            "Draft the notes",
            vec![criterion("b", true, "x"), criterion("a", false, "y")],
        );
        let (contract, progress) = stored(
            "Draft the notes",
            vec![criterion("a", false, "y"), criterion("b", true, "x")],
        );
        assert_eq!(
            admit_commitment_admission(
                ContinuitySpeechAct::Delegation,
                &ordered,
                Some((&contract, &progress))
            )
            .unwrap(),
            ContinuityCommitmentAdmission::Reuse
        );
    }

    #[test]
    fn corrected_commitment_amends_instead_of_duplicating() {
        let (contract, progress) = stored("Draft the notes", Vec::new());
        let corrected = commitment("Draft and send the notes", Vec::new());
        assert_eq!(
            admit_commitment_admission(
                ContinuitySpeechAct::Correction,
                &corrected,
                Some((&contract, &progress))
            )
            .unwrap(),
            ContinuityCommitmentAdmission::Amend
        );
        // Same identity, different criteria: an amendment, never a reset.
        let narrowed = commitment(
            "Draft the notes",
            vec![criterion("c1", true, "exact-source")],
        );
        assert_eq!(
            admit_commitment_admission(
                ContinuitySpeechAct::Delegation,
                &narrowed,
                Some((&contract, &progress))
            )
            .unwrap(),
            ContinuityCommitmentAdmission::Amend
        );
    }

    #[test]
    fn terminal_or_suppressed_goals_still_refuse_new_work() {
        let proposal = commitment("Draft the notes", Vec::new());
        let (contract, mut progress) = stored("Draft the notes", Vec::new());
        progress.lifecycle = ContinuityGoalLifecycle::Achieved;
        progress.next_attention = None;
        assert_eq!(
            admit_commitment_admission(
                ContinuitySpeechAct::Delegation,
                &proposal,
                Some((&contract, &progress))
            )
            .unwrap_err()
            .code,
            ContinuityFailureCode::InvalidRequest
        );
        let (contract, mut progress) = stored("Draft the notes", Vec::new());
        progress.control = ContinuityGoalControl::Paused;
        assert_eq!(
            admit_commitment_admission(
                ContinuitySpeechAct::Delegation,
                &proposal,
                Some((&contract, &progress))
            )
            .unwrap_err()
            .code,
            ContinuityFailureCode::InvalidRequest
        );
    }

    #[test]
    fn contract_changes_drop_only_evidence_for_changed_criteria() {
        let previous = vec![
            criterion("kept", true, "exact-source"),
            criterion("changed", true, "exact-source"),
            criterion("removed", false, "exact-source"),
        ];
        let next = vec![
            criterion("kept", true, "exact-source"),
            criterion("changed", true, "latest-source"),
            criterion("added", true, "exact-source"),
        ];
        let evidence = |id: &str| ContinuityEvidenceRef {
            source: criterion(id, true, "exact-source").description_ref,
            issuer: "member:worker".into(),
            subject_version: 1,
            criterion_id: id.into(),
            observed_at: 10,
            result: ContinuityEvidenceResult::Pass,
            verification_kind: super::super::generated::ContinuityVerificationKind::Deterministic,
            scope: ContinuityVisibilityScope::Goal,
            validity: ContinuitySourceValidity::Current,
        };
        let retained = retain_current_criterion_evidence(
            &previous,
            &next,
            &[
                evidence("kept"),
                evidence("changed"),
                evidence("removed"),
                evidence("added"),
            ],
        );
        let ids: Vec<&str> = retained
            .iter()
            .map(|item| item.criterion_id.as_str())
            .collect();
        assert_eq!(ids, vec!["kept"]);
    }

    fn reminder(settlement: Option<&str>, causes: Vec<ContinuitySourceRef>) -> ContinuityWake {
        ContinuityWake {
            logical_wake_id: "wake:goal:notes:4:review-due".into(),
            goal_id: "goal:notes".into(),
            cause_refs: causes,
            due_at: Some(10),
            review_policy: "review-due".into(),
            goal_revision: 4,
            epoch: 0,
            host_generation: 1,
            claim: None,
            settlement: settlement.map(str::to_owned),
        }
    }

    #[test]
    fn a_reminder_that_repeats_recorded_sources_decides_nothing() {
        let (contract, progress) = stored(
            "Draft the notes",
            vec![criterion("criterion:notes-done", true, "exact-source")],
        );
        let recorded = criterion("criterion:notes-done", true, "exact-source").description_ref;
        assert!(
            wake_repeats_recorded_sources(
                &reminder(None, vec![recorded.clone()]),
                &contract,
                &progress
            ),
            "a reminder re-reading a recorded source is pure status aggregation"
        );
        assert!(
            wake_repeats_recorded_sources(&reminder(None, Vec::new()), &contract, &progress),
            "a timer reminder names no new source"
        );
        let goal_self = ContinuitySourceRef {
            owner_kind: ContinuitySourceOwnerKind::Goal,
            opaque_id: progress.goal_id.clone(),
            part_id: None,
            span: None,
            source_revision: progress.revision,
            digest: format!("goal:{}:{}", progress.goal_id, progress.revision),
            visibility_scope: ContinuityVisibilityScope::Goal,
            validity: ContinuitySourceValidity::Current,
        };
        assert!(
            wake_repeats_recorded_sources(&reminder(None, vec![goal_self]), &contract, &progress),
            "the Goal naming itself is not new information"
        );
    }

    #[test]
    fn new_sources_and_settlements_always_earn_a_review() {
        let (contract, progress) = stored(
            "Draft the notes",
            vec![criterion("criterion:notes-done", true, "exact-source")],
        );
        let mut unseen = criterion("criterion:notes-done", true, "exact-source").description_ref;
        unseen.opaque_id = "event:user-correction".into();
        assert!(
            !wake_repeats_recorded_sources(&reminder(None, vec![unseen]), &contract, &progress),
            "a wake naming an unrecorded source changes what the Assistant knows"
        );
        let mut newer = criterion("criterion:notes-done", true, "exact-source").description_ref;
        newer.source_revision += 1;
        assert!(
            !wake_repeats_recorded_sources(&reminder(None, vec![newer]), &contract, &progress),
            "a newer revision of a recorded source is new information"
        );
        let recorded = criterion("criterion:notes-done", true, "exact-source").description_ref;
        assert!(
            !wake_repeats_recorded_sources(
                &reminder(Some("settlement:child"), vec![recorded]),
                &contract,
                &progress
            ),
            "a settled child result is a decision, not a reminder"
        );
    }
}
