//! Responsibility-specific qualification evidence.
//!
//! Production entries:
//! - [`qualification_port`] / [`QualificationService::lookup`] — frozen
//!   [`QualificationPort`]
//! - [`QualificationService::ingest_immutable`] — append-only evidence
//! - [`QualificationService::admit_execution`] — automatic gate only;
//!   ordinary requested execution keeps its prior contract
//!
//! Model, price, Skill, and target catalogs stay with their existing owners.

mod economy;
mod eligibility;
mod evaluate;
mod evidence;
mod failure;
mod owners;
mod policy;
mod service;
mod wilson;

pub use economy::{EconomicReport, evaluate_economy};
pub use eligibility::{
    RequestKind, RoutingCandidate, admit_child_scope, admit_requested_execution,
    filter_eligible_then_stable_order,
};
pub use evaluate::{
    QualificationAssessment, RateReport, UnqualifiedReason, evaluate_bundle, result_from_assessment,
};
pub use evidence::{
    AUTHORIZED_LIVE_SESSION_LABEL, ClosureClaim, EconomyRole, EvidenceBundle, EvidenceClass,
    HardInvariantCounts, LiveAdmission, LiveAuthorityKind, LiveProvenance, LiveSourceKind,
    ObservationEconomy, ObservationJudgment, ObservationPolarity, QualificationObservation,
    SYNTHETIC_TEST_EVIDENCE_LABEL, admit_identity, child_expands_parent_permission,
    identities_match, identity_changed,
};
#[cfg(any(test, feature = "test-support"))]
pub use evidence::{SyntheticRecipe, generate_synthetic};
pub use owners::{
    agent_model_token_price, catalog_model_projection, model_token_price, skill_hub_list,
    target_read_only, token_cost_from_owner,
};
pub use policy::{DRAFT_POLICY_REVISION, QualificationPolicy};
pub use service::{QualificationService, query_record};
pub use wilson::{WILSON_Z_ONE_SIDED_95, nearest_rank_p95, one_sided_wilson_upper};

use licoup_conversation::continuity::QualificationPort;

/// Production qualification port. Empty evidence is unknown, never qualified.
pub fn qualification_port() -> QualificationService {
    QualificationService::draft_port()
}

pub fn lookup_via_port(
    port: &impl QualificationPort,
    record: &licoup_conversation::continuity::ContinuityQualificationRecord,
) -> Result<
    licoup_conversation::continuity::ContinuityQualificationRecord,
    licoup_conversation::continuity::ContinuityFailure,
> {
    port.lookup(record)
}
