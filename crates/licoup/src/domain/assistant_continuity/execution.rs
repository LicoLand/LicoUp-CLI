//! N4/N8 helpers: factual review briefs and settlement identity.
//!
//! Briefs carry Goal/progress facts, admitted granted source spans, evidence
//! refs, and settlement receipt pointers. They do not copy a Conversation
//! transcript and do not treat parent relation as permission.

use licoup_conversation::continuity::{
    ContinuityEvidenceRef, ContinuityGoalContract, ContinuityGoalProgress,
    ContinuitySourceOwnerKind, ContinuitySourceRef, ContinuitySourceValidity,
    ContinuityTaskConversationRelation, ContinuityVisibilityScope,
};
use serde_json::Value;

pub const CONTINUITY_KIND_CHILD_WORK: &str = "child-work";
pub const CONTINUITY_KIND_WAKE_REVIEW: &str = "wake-review";
pub const CONTINUITY_KIND_USER_POSTED: &str = "user-posted";

#[derive(Clone, Debug, Default)]
pub struct AdmittedRecipientFacts {
    pub granted_source_texts: Vec<AdmittedGrantedFact>,
}

#[derive(Clone, Debug)]
pub struct AdmittedGrantedFact {
    pub source_id: String,
    pub text: String,
}

pub fn settlement_identity(payload: &Value) -> String {
    payload
        .get("dispatchId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| {
            payload
                .get("causationId")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "settlement:unknown".to_owned())
}

pub fn child_settlement_identity(payload: &Value) -> Option<String> {
    payload
        .get("dispatchId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
}

pub fn child_settlement_is_unknown(payload: &Value) -> bool {
    if payload.get("ok").and_then(Value::as_bool) == Some(false) {
        return true;
    }
    matches!(
        payload
            .get("turnStatus")
            .or_else(|| payload
                .get("error")
                .and_then(|error| error.get("turnStatus")))
            .and_then(Value::as_str)
            .map(str::trim),
        Some("failed" | "cancelled" | "unknown" | "pending" | "running")
    )
}

pub fn payload_continuity_kind(payload: &Value) -> Option<&str> {
    payload.get("continuityKind").and_then(Value::as_str)
}

pub fn is_wake_review_payload(payload: &Value) -> bool {
    payload_continuity_kind(payload) == Some(CONTINUITY_KIND_WAKE_REVIEW)
}

pub fn compose_wake_review_brief(
    contract: &ContinuityGoalContract,
    progress: &ContinuityGoalProgress,
    facts: &AdmittedRecipientFacts,
    relation: &ContinuityTaskConversationRelation,
    settlement: Option<&Value>,
    review_policy: &str,
    review_reason: &str,
) -> String {
    let mut lines = Vec::new();
    lines.push("Goal review (factual refs only; no conversation transcript).".to_owned());
    lines.push(format!("goalId: {}", progress.goal_id));
    lines.push(format!("revision: {}", progress.revision));
    lines.push(format!("lifecycle: {:?}", progress.lifecycle));
    lines.push(format!("control: {:?}", progress.control));
    if !contract.expected_result.trim().is_empty() {
        lines.push(format!(
            "expectedResult: {}",
            contract.expected_result.trim()
        ));
    }
    lines.push(format!("acceptanceMethod: {}", contract.acceptance_method));
    lines.push(format!("reviewPolicy: {review_policy}"));
    lines.push(format!("reviewReason: {review_reason}"));
    lines.push(format!(
        "childConversationId: {}",
        relation.child_conversation_id
    ));
    lines.push(format!(
        "parentConversationId: {}",
        relation.parent_conversation_id
    ));
    lines.push(format!(
        "pendingEvidenceCount: {}",
        progress.criterion_evidence_refs.len()
    ));
    if !progress.criterion_evidence_refs.is_empty() {
        lines.push(format!(
            "evidenceRefs: {}",
            evidence_ref_list(&progress.criterion_evidence_refs)
        ));
    }
    append_granted_facts(&mut lines, facts);
    if let Some(payload) = settlement {
        if let Some(id) = child_settlement_identity(payload) {
            lines.push(format!("settlementReceiptId: {id}"));
        } else {
            lines.push(format!(
                "settlementReceiptId: {}",
                settlement_identity(payload)
            ));
        }
        if let Some(output) = settlement_output_ref(payload) {
            lines.push(format!("settlementOutputRef: {output}"));
        }
    }
    if let Some(attention) = &progress.next_attention {
        lines.push(format!("nextAttention: {attention:?}"));
    }
    lines.join("\n")
}

pub fn compose_child_work_brief(
    contract: &ContinuityGoalContract,
    progress: &ContinuityGoalProgress,
    facts: &AdmittedRecipientFacts,
    relation: &ContinuityTaskConversationRelation,
) -> String {
    let mut lines = Vec::new();
    lines.push(
        "Admitted Goal execution (factual refs only; no conversation transcript).".to_owned(),
    );
    lines.push(format!("goalId: {}", progress.goal_id));
    lines.push(format!("revision: {}", progress.revision));
    lines.push(format!("lifecycle: {:?}", progress.lifecycle));
    if !contract.expected_result.trim().is_empty() {
        lines.push(format!(
            "expectedResult: {}",
            contract.expected_result.trim()
        ));
    }
    lines.push(format!(
        "childConversationId: {}",
        relation.child_conversation_id
    ));
    lines.push(format!(
        "parentConversationId: {}",
        relation.parent_conversation_id
    ));
    lines.push(format!(
        "pendingEvidenceCount: {}",
        progress.criterion_evidence_refs.len()
    ));
    if !progress.criterion_evidence_refs.is_empty() {
        lines.push(format!(
            "evidenceRefs: {}",
            evidence_ref_list(&progress.criterion_evidence_refs)
        ));
    }
    append_granted_facts(&mut lines, facts);
    lines.join("\n")
}

pub fn review_cause_refs(
    contract: &ContinuityGoalContract,
    progress: &ContinuityGoalProgress,
    settlement_event: Option<&ContinuitySourceRef>,
) -> Vec<ContinuitySourceRef> {
    let mut refs = Vec::new();
    refs.push(goal_source_ref(&progress.goal_id, progress.revision));
    if !contract.created_event.opaque_id.trim().is_empty() {
        refs.push(contract.created_event.clone());
    }
    if let Some(source) = settlement_event {
        refs.push(source.clone());
    }
    for evidence in progress.criterion_evidence_refs.iter().take(8) {
        refs.push(evidence.source.clone());
    }
    refs
}

pub fn goal_source_ref(goal_id: &str, revision: i64) -> ContinuitySourceRef {
    ContinuitySourceRef {
        owner_kind: ContinuitySourceOwnerKind::Goal,
        opaque_id: goal_id.to_owned(),
        part_id: None,
        span: None,
        source_revision: revision,
        digest: format!("goal:{goal_id}:{revision}"),
        visibility_scope: ContinuityVisibilityScope::Goal,
        validity: ContinuitySourceValidity::Current,
    }
}

pub fn event_source_ref(
    event_id: &str,
    part_id: Option<String>,
    sequence: i64,
) -> ContinuitySourceRef {
    ContinuitySourceRef {
        owner_kind: ContinuitySourceOwnerKind::Event,
        opaque_id: event_id.to_owned(),
        part_id,
        span: None,
        source_revision: sequence,
        digest: format!("event:{event_id}"),
        visibility_scope: ContinuityVisibilityScope::Conversation,
        validity: ContinuitySourceValidity::Current,
    }
}

pub fn strip_review_unsafe_effects(
    mut proposal: licoup_conversation::continuity::ContinuityInterpretationProposal,
) -> licoup_conversation::continuity::ContinuityInterpretationProposal {
    proposal.task_child_admission = None;
    for commitment in &mut proposal.commitment_proposals {
        commitment.create_goal = false;
    }
    proposal
}

fn append_granted_facts(lines: &mut Vec<String>, facts: &AdmittedRecipientFacts) {
    for fact in &facts.granted_source_texts {
        if fact.text.trim().is_empty() {
            continue;
        }
        lines.push(format!("grantedSource: {}", fact.source_id));
        lines.push(fact.text.clone());
    }
}

fn evidence_ref_list(evidence: &[ContinuityEvidenceRef]) -> String {
    evidence
        .iter()
        .map(|item| item.source.opaque_id.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

fn settlement_output_ref(payload: &Value) -> Option<String> {
    payload
        .get("output")
        .and_then(Value::as_str)
        .or_else(|| payload.get("text").and_then(Value::as_str))
        .map(truncate_ref)
}

fn truncate_ref(value: &str) -> String {
    const LIMIT: usize = 240;
    let trimmed = value.trim();
    if trimmed.len() <= LIMIT {
        return trimmed.to_owned();
    }
    let mut end = LIMIT;
    while end > 0 && !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &trimmed[..end])
}
