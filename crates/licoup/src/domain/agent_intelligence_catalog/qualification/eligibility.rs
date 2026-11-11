use licoup_conversation::continuity::{
    ContinuityCandidateIdentity, ContinuityFailure, ContinuityQualificationRecord,
    ContinuityQualificationResult,
};

use super::evidence::{EvidenceClass, child_expands_parent_permission};
use super::failure::{qualification_stale, qualification_unknown, scope_denied};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestKind {
    AutomaticAdvancement,
    ExplicitlyRequested,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoutingCandidate {
    pub responsibility_id: String,
    pub identity: ContinuityCandidateIdentity,
    pub stable_key: String,
    pub hard_admitted: bool,
    pub explicit_model_choice: bool,
    pub request_kind: RequestKind,
}

pub fn admit_requested_execution() -> Result<(), ContinuityFailure> {
    Ok(())
}

pub fn admit_automatic_from_record(
    record: &ContinuityQualificationRecord,
) -> Result<(), ContinuityFailure> {
    if record.revoked || record.result == ContinuityQualificationResult::Stale {
        return Err(qualification_stale());
    }
    if record.result != ContinuityQualificationResult::Qualified {
        return Err(qualification_unknown());
    }
    Ok(())
}

pub fn filter_eligible_then_stable_order(
    mut candidates: Vec<RoutingCandidate>,
    lookup: impl Fn(&RoutingCandidate) -> ContinuityQualificationRecord,
) -> Vec<RoutingCandidate> {
    candidates.retain(|candidate| {
        if !candidate.hard_admitted {
            return false;
        }
        if candidate.request_kind == RequestKind::ExplicitlyRequested
            || candidate.explicit_model_choice
        {
            return true;
        }
        admit_automatic_from_record(&lookup(candidate)).is_ok()
    });
    candidates.sort_by(|left, right| {
        left.stable_key
            .cmp(&right.stable_key)
            .then_with(|| left.responsibility_id.cmp(&right.responsibility_id))
            .then_with(|| left.identity.model_digest.cmp(&right.identity.model_digest))
    });
    candidates
}

pub fn admit_child_scope(
    parent: &ContinuityCandidateIdentity,
    child: &ContinuityCandidateIdentity,
    parent_class: EvidenceClass,
    child_class: EvidenceClass,
) -> Result<(), ContinuityFailure> {
    if child_expands_parent_permission(parent, child) {
        return Err(scope_denied());
    }
    if parent_class == EvidenceClass::Synthetic && child_class == EvidenceClass::LiveAuthorized {
        return Err(scope_denied());
    }
    Ok(())
}
