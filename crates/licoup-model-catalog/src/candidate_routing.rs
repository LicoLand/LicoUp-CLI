//! The dispatch entry's routing decision: which allowed alternative runs now,
//! asked of the catalogue's candidate policy instead of chosen ad hoc.
//!
//! A dispatch entry — a workflow callback, a delegation, a direct turn — holds
//! alternatives it could run (an Agent, a model selector and, when the request
//! names one, a serving provider). Picking the first entry of that list is not
//! a routing decision: it is insertion order wearing a decision's clothes. This
//! module is the one place such an entry asks the policy
//! ([`crate::candidate_policy::select_candidates`]) and gets
//! back a ranked recommendation with one named reason for every alternative
//! that may not run.
//!
//! Three rules belong to the policy, and this module re-derives none of them:
//!
//! - **Only allowed alternatives are considered.** The allowed set is narrowed
//!   here by the effective per-scope admission
//!   ([`crate::selection_matrix::SelectionMatrixPort`]). An Agent
//!   the admission owner did not allow for *this* scope is not in the allowed
//!   set, so it is never ranked and never reached by a fallback. A direct
//!   request and a workflow turn are separate decisions with separate outcomes.
//! - **Unknown is never consent.** An alternative the fact source establishes
//!   nothing about is reported as [`CandidateRoutingGap::NoRecordedFacts`] and
//!   is not offered: something nobody established anything about is not an
//!   eligible substitute.
//! - **Nothing here grants execution permission.** The selected alternative is
//!   a recommendation inside the allowed set. The entry still executes the work
//!   under whatever authority it already held, and the pre-effect recheck stays
//!   the gate it is.
//!
//! The facts the policy reads are owned by whoever composes the host and arrive
//! through [`install_candidate_facts`], exactly like
//! [`crate::port::install_model_catalog_port`] answers the
//! catalogue's own port. A process that installs nothing keeps every candidate
//! fact unknown, so the policy excludes every alternative and routing reports
//! an unavailable result rather than a guessed model.

use crate::availability::ObservedAvailability;
use crate::candidate_policy::{
    CandidateDecision, CandidatePolicyPort, CandidateRequest, CandidateUnavailable, QuotaState,
    RequirementState, select_candidates,
};
/// The catalogue's own vocabulary, re-exported so a dispatch entry names the
/// policy question in one import instead of reaching into three modules.
pub use crate::candidate_policy::{
    CandidateExclusion, CandidateId, CandidateRelation, CandidateRequirement,
};
use crate::port::CredentialState;
pub use crate::selection_matrix::{ScopeAdmission, SelectionScope};
use crate::selection_matrix::{ScopeAdmissionFacts, ScopeOutcomeState, SelectionMatrixPort};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock, RwLock};

/// The facts one candidate is answered with.
///
/// Every field is what its owner established, kept three-valued where the
/// catalogue keeps it three-valued: `observed_at_unix_ms: None` is "no live
/// source on this host reported it", `credential` and `quota` keep their own
/// unknown answer, and a requirement absent from `requirements` is one nothing
/// establishes — answered `Unknown`, never `Satisfied`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateFacts {
    /// When a live source on this host reported the candidate. `None` is no
    /// observation, never an observation at the epoch.
    pub observed_at_unix_ms: Option<u64>,
    pub credential: CredentialState,
    pub quota: QuotaState,
    /// The owner's answer for the requirements it actually checked.
    pub requirements: BTreeMap<CandidateRequirement, RequirementState>,
}

impl Default for CandidateFacts {
    fn default() -> Self {
        Self {
            observed_at_unix_ms: None,
            credential: CredentialState::Unknown,
            quota: QuotaState::Unknown,
            requirements: BTreeMap::new(),
        }
    }
}

/// The facts a routing decision reads, answered per candidate.
///
/// It is a trait so the owner can answer from the read that actually holds the
/// fact — a declared model list, a live observation, a credential claim — for
/// the candidate the decision asks about, rather than from a snapshot the owner
/// had to guess the members of. `None` is the honest answer for a candidate
/// whose facts were never established, and it is what keeps such a candidate
/// out of the allowed set.
pub trait CandidateFactSource: Send + Sync {
    fn facts(&self, candidate: &CandidateId) -> Option<CandidateFacts>;
}

/// A stated fact table, keyed by candidate.
///
/// It is what a caller with a fixed set of facts installs or states directly,
/// and what an owner with a read of its own builds before installing.
#[derive(Clone, Debug, Default)]
pub struct CandidateFactTable {
    rows: BTreeMap<CandidateId, CandidateFacts>,
}

impl CandidateFactTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the facts for one candidate, replacing any previous row.
    pub fn record(&mut self, candidate: CandidateId, facts: CandidateFacts) -> &mut Self {
        self.rows.insert(candidate, facts);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }
}

impl CandidateFactSource for CandidateFactTable {
    fn facts(&self, candidate: &CandidateId) -> Option<CandidateFacts> {
        self.rows.get(candidate).cloned()
    }
}

static INSTALLED_FACTS: RwLock<Option<Arc<dyn CandidateFactSource>>> = RwLock::new(None);
static INSTALLED_SCOPE_ADMISSION: RwLock<Option<ScopeAdmission>> = RwLock::new(None);
static FAIL_CLOSED_MATRIX_PORT: OnceLock<SelectionMatrixPort> = OnceLock::new();

/// Install the candidate facts this host established, once per process.
///
/// The composition that calls it lives at the `licoup-native` crate root,
/// above both layers: the facts come from the owners that know them, and this
/// module reads no catalogue, credential store or target of its own.
pub fn install_candidate_facts(facts: Arc<dyn CandidateFactSource>) {
    let mut guard = INSTALLED_FACTS
        .write()
        .unwrap_or_else(|poison| poison.into_inner());
    *guard = Some(facts);
}

/// The installed fact source, or nothing when no owner was composed.
pub fn installed_candidate_facts() -> Option<Arc<dyn CandidateFactSource>> {
    INSTALLED_FACTS
        .read()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone()
}

/// Install the effective per-scope admission this host decides with.
///
/// It is the same answer
/// [`crate::selection_matrix::SelectionMatrixPort`] carries to
/// the client projection: one owner answers both, so the scope outcome a client
/// renders and the scope outcome routing obeys cannot disagree.
pub fn install_scope_admission(admission: ScopeAdmission) {
    let mut guard = INSTALLED_SCOPE_ADMISSION
        .write()
        .unwrap_or_else(|poison| poison.into_inner());
    *guard = Some(admission);
}

/// The admission port this host composed, or the fail-closed one.
///
/// A host that installed no admission owner keeps every scope `Undetermined`,
/// and then no alternative is allowed: routing reports an unavailable result
/// instead of admitting an Agent on readiness evidence alone.
pub fn scope_admission_port() -> SelectionMatrixPort {
    let installed = *INSTALLED_SCOPE_ADMISSION
        .read()
        .unwrap_or_else(|poison| poison.into_inner());
    match installed {
        Some(agent_scope_admission) => SelectionMatrixPort {
            agent_scope_admission,
        },
        None => *FAIL_CLOSED_MATRIX_PORT.get_or_init(SelectionMatrixPort::unavailable),
    }
}

/// One alternative the entry offers to the routing decision, in the entry's own
/// configured order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateOffer {
    pub candidate: CandidateId,
    /// The configured position of this alternative. A configured order is a
    /// preference, never a permission.
    pub configured_position: usize,
}

/// One admitted request, as the entry asks it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateRoutingRequest {
    /// The scope this decision is asked for. The allowed set is narrowed by the
    /// admission answer *for this scope*, so a direct request and a workflow
    /// turn never borrow each other's outcome.
    pub scope: SelectionScope,
    /// The alternatives the entry offers, in configured order.
    pub offers: Vec<CandidateOffer>,
    /// The facts every considered alternative must establish.
    pub requirements: Vec<CandidateRequirement>,
}

impl CandidateRoutingRequest {
    pub fn new(scope: SelectionScope, offers: Vec<CandidateOffer>) -> Self {
        Self {
            scope,
            offers,
            requirements: Vec::new(),
        }
    }
}

/// Why one offered alternative was not routed to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CandidateRoutingGap {
    /// The effective scope admission did not allow the Agent this alternative
    /// runs on. `state` and `reason` are the admission owner's own answer.
    ScopeNotAllowed {
        candidate: CandidateId,
        state: ScopeOutcomeState,
        reason: String,
    },
    /// Nothing established facts for the alternative, so it is not usable. It
    /// is never read as eligible.
    NoRecordedFacts { candidate: CandidateId },
}

/// The routing decision, with the allowed set it was taken over.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateRoutingOutcome {
    pub scope: SelectionScope,
    /// The alternatives the effective grants allowed, ordered by identity, so a
    /// caller can assert the decision could not have left the allowed set.
    pub allowed: BTreeSet<CandidateId>,
    /// The catalogue's own decision: ranked survivors, one exclusion per
    /// rejected alternative, the recommendation, and any substitution made.
    pub decision: CandidateDecision,
    /// The offered alternatives no admission allowed, and the alternatives
    /// nothing established facts for.
    pub gaps: Vec<CandidateRoutingGap>,
}

impl CandidateRoutingOutcome {
    /// The recommended alternative, or `None` when nothing was selectable.
    pub fn selected(&self) -> Option<&CandidateId> {
        self.decision.selected.as_ref()
    }

    /// The relations the surviving alternatives carry, in ranked order.
    pub fn relations(&self) -> Vec<CandidateRelation> {
        self.decision
            .ranked
            .iter()
            .map(|entry| entry.relation)
            .collect()
    }

    /// Why nothing was selected, when nothing was.
    pub fn unavailable(&self) -> Option<&CandidateUnavailable> {
        self.decision.unavailable.as_ref()
    }
}

/// The routing owner a dispatch entry asks.
///
/// It is a trait rather than a value of `fn` pointers because the entry holds
/// its owner for the life of the entry, and because the entry must be provable
/// against stated facts without installing a process-wide snapshot.
pub trait CandidateRoutingPort: Send + Sync {
    fn route(&self, request: &CandidateRoutingRequest) -> CandidateRoutingOutcome;
}

impl CandidateRoutingPort for RoutingWithFacts {
    fn route(&self, request: &CandidateRoutingRequest) -> CandidateRoutingOutcome {
        route_with(request, Some(Arc::clone(&self.facts)), &self.admission)
    }
}

/// One fact source and one admission port, held together as an entry's owner.
pub struct RoutingWithFacts {
    facts: Arc<dyn CandidateFactSource>,
    admission: SelectionMatrixPort,
}

impl RoutingWithFacts {
    pub fn new(facts: Arc<dyn CandidateFactSource>, admission: SelectionMatrixPort) -> Self {
        Self { facts, admission }
    }
}

/// The owner a composed process routes with: the facts and admission this host
/// installed through [`install_candidate_facts`] and [`install_scope_admission`].
#[derive(Clone, Copy, Debug, Default)]
pub struct PolicyCandidateRouting;

impl CandidateRoutingPort for PolicyCandidateRouting {
    fn route(&self, request: &CandidateRoutingRequest) -> CandidateRoutingOutcome {
        route_candidates(request)
    }
}

/// Ask the policy which allowed alternative may run, for one scope, through the
/// facts and admission this process composed.
pub fn route_candidates(request: &CandidateRoutingRequest) -> CandidateRoutingOutcome {
    route_with(
        request,
        installed_candidate_facts(),
        &scope_admission_port(),
    )
}

/// The same decision over an explicit fact source and admission port.
///
/// It is separated from [`route_candidates`] so a decision can be exercised
/// against exactly the facts a caller states, and so the process-wide
/// installation stays the one composition path.
pub fn route_with(
    request: &CandidateRoutingRequest,
    facts: Option<Arc<dyn CandidateFactSource>>,
    admission: &SelectionMatrixPort,
) -> CandidateRoutingOutcome {
    let params = Value::Object(serde_json::Map::new());
    let mut allowed = BTreeSet::new();
    let mut gaps = Vec::new();
    let mut preference = Vec::with_capacity(request.offers.len());
    for offer in &request.offers {
        if !preference.contains(&offer.candidate) {
            preference.push(offer.candidate.clone());
        }
        let decision =
            (admission.agent_scope_admission)(&offer.candidate.agent_id, request.scope, &params);
        if decision.state != ScopeOutcomeState::Allowed {
            gaps.push(CandidateRoutingGap::ScopeNotAllowed {
                candidate: offer.candidate.clone(),
                state: decision.state,
                reason: decision.reason,
            });
            continue;
        }
        if facts
            .as_ref()
            .is_none_or(|source| source.facts(&offer.candidate).is_none())
        {
            gaps.push(CandidateRoutingGap::NoRecordedFacts {
                candidate: offer.candidate.clone(),
            });
            continue;
        }
        allowed.insert(offer.candidate.clone());
    }

    let mut policy_request = CandidateRequest::new(
        format!("routing:{}", request.scope.as_str()),
        request.scope.as_str(),
    );
    policy_request.requirements = request.requirements.clone();
    policy_request.allowed = allowed.clone();
    policy_request.preference = preference;

    let decision = decide(&policy_request, facts);

    CandidateRoutingOutcome {
        scope: request.scope,
        allowed,
        decision,
        gaps,
    }
}

/// Run the catalogue's policy with one fact source consulted for the call.
///
/// The policy port is a value of plain `fn` pointers and carries no context, so
/// the source is lent to those pointers for exactly this call and cleared
/// immediately afterwards: nothing outlives the decision and nothing leaks
/// between threads.
fn decide(
    request: &CandidateRequest,
    facts: Option<Arc<dyn CandidateFactSource>>,
) -> CandidateDecision {
    DECIDING_FACTS.with(|slot| {
        let previous = slot.replace(facts.clone());
        let decision = select_candidates(&policy_port(facts.as_deref()), request);
        slot.replace(previous);
        decision
    })
}

thread_local! {
    /// The fact source one routing decision is taken over.
    static DECIDING_FACTS: std::cell::RefCell<Option<Arc<dyn CandidateFactSource>>> =
        const { std::cell::RefCell::new(None) };
}

/// The facts one candidate is answered with, or `None` when nothing established
/// them.
fn recorded(candidate: &CandidateId) -> Option<CandidateFacts> {
    DECIDING_FACTS.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|source| source.facts(candidate))
    })
}

/// The policy port over one fact source, or the fail-closed declarations when
/// there is none: every fact unknown, so nothing is selected.
fn policy_port(facts: Option<&dyn CandidateFactSource>) -> CandidatePolicyPort {
    match facts {
        Some(_) => CandidatePolicyPort {
            requirement: requirement_answer,
            availability: availability_answer,
            credential: credential_answer,
            quota: quota_answer,
        },
        None => CandidatePolicyPort::unavailable(),
    }
}

fn requirement_answer(
    candidate: &CandidateId,
    requirement: &CandidateRequirement,
) -> RequirementState {
    match recorded(candidate) {
        Some(facts) => facts
            .requirements
            .get(requirement)
            .cloned()
            .unwrap_or(RequirementState::Unknown),
        None => RequirementState::Unknown,
    }
}

fn availability_answer(candidate: &CandidateId) -> ObservedAvailability {
    match recorded(candidate).and_then(|facts| facts.observed_at_unix_ms) {
        Some(at_unix_ms) => ObservedAvailability::Observed {
            at_unix_ms,
            sources: vec!["candidate_policy_owner".to_owned()],
        },
        None => ObservedAvailability::Unknown,
    }
}

fn credential_answer(candidate: &CandidateId) -> CredentialState {
    recorded(candidate).map_or(CredentialState::Unknown, |facts| facts.credential)
}

fn quota_answer(candidate: &CandidateId) -> QuotaState {
    recorded(candidate).map_or(QuotaState::Unknown, |facts| facts.quota)
}

/// The advisory sentence a routing outcome is reported with: it names the
/// recommendation, the substitution it made, and why nothing was selected.
pub fn routing_rationale(outcome: &CandidateRoutingOutcome) -> String {
    let Some(selected) = outcome.selected() else {
        return match outcome.unavailable() {
            Some(CandidateUnavailable::NoAllowedCandidate) => {
                "No alternative is admitted for this scope; nothing was selected".to_owned()
            }
            Some(CandidateUnavailable::EveryCandidateExcluded { codes }) => format!(
                "Every admitted alternative was excluded ({}); nothing was selected",
                codes
                    .iter()
                    .map(|code| code.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            None => "No alternative was selectable; nothing was selected".to_owned(),
        };
    };
    match &outcome.decision.substitute_for {
        Some(preferred) => format!(
            "Selected {} as the configured substitute for {}",
            describe(selected),
            describe(preferred)
        ),
        None => format!(
            "Selected {} from {} admitted alternative(s)",
            describe(selected),
            outcome.allowed.len()
        ),
    }
}

fn describe(candidate: &CandidateId) -> String {
    match &candidate.provider_id {
        Some(provider) => format!(
            "{} / {} via {}",
            candidate.agent_id, candidate.model_id, provider
        ),
        None => format!("{} / {}", candidate.agent_id, candidate.model_id),
    }
}

/// The admission facts one Agent and scope are decided with, so an entry can
/// report the answer it routed under.
pub fn admission_facts(agent: &str, scope: SelectionScope) -> ScopeAdmissionFacts {
    let admission = scope_admission_port();
    (admission.agent_scope_admission)(agent, scope, &Value::Object(serde_json::Map::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::candidate_policy::ExclusionCode;

    fn candidate(agent: &str, model: &str) -> CandidateId {
        CandidateId::new(agent, model, None)
    }

    fn usable(observed_at_unix_ms: u64) -> CandidateFacts {
        CandidateFacts {
            observed_at_unix_ms: Some(observed_at_unix_ms),
            credential: CredentialState::Present,
            quota: QuotaState::Available {
                window: "primary".to_owned(),
            },
            requirements: BTreeMap::new(),
        }
    }

    fn arc(table: &CandidateFactTable) -> Arc<dyn CandidateFactSource> {
        Arc::new(table.clone())
    }

    fn facts_with(rows: &[(&CandidateId, CandidateFacts)]) -> CandidateFactTable {
        let mut table = CandidateFactTable::new();
        for (candidate, facts) in rows {
            table.record((*candidate).clone(), facts.clone());
        }
        table
    }

    fn every_agent_allowed(
        _agent: &str,
        _scope: SelectionScope,
        _params: &Value,
    ) -> ScopeAdmissionFacts {
        ScopeAdmissionFacts::new(ScopeOutcomeState::Allowed, "admitted")
    }

    fn workflow_only(_agent: &str, scope: SelectionScope, _params: &Value) -> ScopeAdmissionFacts {
        match scope {
            SelectionScope::Workflow => {
                ScopeAdmissionFacts::new(ScopeOutcomeState::Allowed, "workflow_scope_admitted")
            }
            SelectionScope::Direct => {
                ScopeAdmissionFacts::new(ScopeOutcomeState::Blocked, "direct_scope_blocked")
            }
        }
    }

    fn admission(f: ScopeAdmission) -> SelectionMatrixPort {
        SelectionMatrixPort {
            agent_scope_admission: f,
        }
    }

    fn request(offers: &[CandidateId], scope: SelectionScope) -> CandidateRoutingRequest {
        CandidateRoutingRequest::new(
            scope,
            offers
                .iter()
                .cloned()
                .enumerate()
                .map(|(position, candidate)| CandidateOffer {
                    candidate,
                    configured_position: position + 1,
                })
                .collect(),
        )
    }

    #[test]
    fn the_policy_ranks_the_allowed_alternatives_instead_of_taking_the_first_offer() {
        let first = candidate("agent-a", "model-a");
        let second = candidate("agent-b", "model-b");
        let facts = facts_with(&[
            (&first, usable(1_726_000_000_000)),
            // The second alternative carries the fresher observation; the
            // configured order still decides, because it is a preference.
            (&second, usable(1_726_000_000_999)),
        ]);
        let outcome = route_with(
            &request(&[first.clone(), second.clone()], SelectionScope::Workflow),
            Some(arc(&facts)),
            &admission(every_agent_allowed),
        );
        assert_eq!(outcome.allowed, BTreeSet::from([first.clone(), second]));
        assert_eq!(
            outcome.selected(),
            Some(&first),
            "the configured order decides before identity order"
        );
        assert!(outcome.gaps.is_empty());
        assert_eq!(
            outcome.relations(),
            vec![
                CandidateRelation::Requested,
                CandidateRelation::ConfiguredSubstitute { position: 2 },
            ]
        );
    }

    #[test]
    fn an_alternative_the_scope_did_not_admit_is_never_ranked() {
        let admitted = candidate("agent-a", "model-a");
        let blocked = candidate("agent-b", "model-b");
        let facts = facts_with(&[
            (&admitted, usable(1_726_000_000_000)),
            // The blocked alternative carries the fresher observation, and it
            // still cannot reach the decision.
            (&blocked, usable(1_726_000_000_999)),
        ]);
        let outcome = route_with(
            &request(
                &[admitted.clone(), blocked.clone()],
                SelectionScope::Workflow,
            ),
            Some(arc(&facts)),
            // The refusal belongs to the scope this decision is asked for, so
            // the fresher observation on the second alternative cannot rank it.
            &admission(|agent, _scope, _params| {
                if agent == "agent-b" {
                    ScopeAdmissionFacts::new(
                        ScopeOutcomeState::Blocked,
                        "workflow_scope_disallows_agent_b",
                    )
                } else {
                    ScopeAdmissionFacts::new(ScopeOutcomeState::Allowed, "admitted")
                }
            }),
        );
        assert_eq!(outcome.allowed, BTreeSet::from([admitted.clone()]));
        assert_eq!(outcome.selected(), Some(&admitted));
        assert!(matches!(
            outcome.gaps.as_slice(),
            [CandidateRoutingGap::ScopeNotAllowed { candidate, reason, .. }]
                if *candidate == blocked && reason == "workflow_scope_disallows_agent_b"
        ));
    }

    #[test]
    fn the_same_offers_route_differently_per_scope() {
        let only = candidate("agent-a", "model-a");
        let facts = facts_with(&[(&only, usable(1_726_000_000_000))]);
        let port = admission(workflow_only);

        let workflow = route_with(
            &request(std::slice::from_ref(&only), SelectionScope::Workflow),
            Some(arc(&facts)),
            &port,
        );
        assert_eq!(workflow.selected(), Some(&only));
        assert_eq!(workflow.scope, SelectionScope::Workflow);

        let direct = route_with(
            &request(std::slice::from_ref(&only), SelectionScope::Direct),
            Some(arc(&facts)),
            &port,
        );
        assert!(direct.allowed.is_empty());
        assert_eq!(direct.scope, SelectionScope::Direct);
        assert_eq!(
            direct.unavailable(),
            Some(&CandidateUnavailable::NoAllowedCandidate)
        );
    }

    #[test]
    fn an_alternative_nothing_established_is_never_offered() {
        let recorded = candidate("agent-a", "model-a");
        let unrecorded = candidate("agent-b", "model-b");
        let facts = facts_with(&[(&recorded, usable(1_726_000_000_000))]);
        let outcome = route_with(
            &request(
                &[recorded.clone(), unrecorded.clone()],
                SelectionScope::Workflow,
            ),
            Some(arc(&facts)),
            &admission(every_agent_allowed),
        );
        assert_eq!(outcome.allowed, BTreeSet::from([recorded]));
        assert!(matches!(
            outcome.gaps.as_slice(),
            [CandidateRoutingGap::NoRecordedFacts { candidate }] if *candidate == unrecorded
        ));
    }

    #[test]
    fn with_no_fact_source_at_all_nothing_is_selected() {
        let only = candidate("agent-a", "model-a");
        let outcome = route_with(
            &request(std::slice::from_ref(&only), SelectionScope::Workflow),
            None,
            &admission(every_agent_allowed),
        );
        assert!(outcome.allowed.is_empty());
        assert_eq!(outcome.selected(), None);
        assert!(matches!(
            outcome.gaps.as_slice(),
            [CandidateRoutingGap::NoRecordedFacts { .. }]
        ));
    }

    #[test]
    fn an_empty_fact_table_selects_nothing() {
        let only = candidate("agent-a", "model-a");
        let facts = CandidateFactTable::new();
        assert!(facts.is_empty());
        assert_eq!(facts.len(), 0);
        let outcome = route_with(
            &request(std::slice::from_ref(&only), SelectionScope::Workflow),
            Some(arc(&facts)),
            &admission(every_agent_allowed),
        );
        assert_eq!(outcome.selected(), None);
        assert!(
            matches!(
                outcome.gaps.as_slice(),
                [CandidateRoutingGap::NoRecordedFacts { candidate }] if *candidate == only
            ),
            "{:?}",
            outcome.gaps
        );
    }

    #[test]
    fn a_recorded_candidate_the_policy_excludes_is_reported_under_its_own_code() {
        let exhausted = candidate("agent-a", "model-a");
        let healthy = candidate("agent-b", "model-b");
        let facts = facts_with(&[
            (
                &exhausted,
                CandidateFacts {
                    quota: QuotaState::Exhausted {
                        window: "primary".to_owned(),
                    },
                    ..usable(1_726_000_000_000)
                },
            ),
            (&healthy, usable(1_726_000_000_000)),
        ]);
        let outcome = route_with(
            &request(
                &[exhausted.clone(), healthy.clone()],
                SelectionScope::Workflow,
            ),
            Some(arc(&facts)),
            &admission(every_agent_allowed),
        );
        assert_eq!(outcome.selected(), Some(&healthy));
        assert_eq!(outcome.decision.substitute_for, Some(exhausted.clone()));
        assert_eq!(
            outcome
                .decision
                .exclusion_for(&exhausted)
                .map(CandidateExclusion::code),
            Some(ExclusionCode::QuotaExhausted)
        );
        assert!(routing_rationale(&outcome).contains("substitute"));
    }

    #[test]
    fn every_exclusion_is_named_and_never_collapsed_into_one_refusal() {
        let unobserved = candidate("agent-a", "model-a");
        let without_credential = candidate("agent-b", "model-b");
        let facts = facts_with(&[
            (
                &unobserved,
                CandidateFacts {
                    observed_at_unix_ms: None,
                    ..usable(0)
                },
            ),
            (
                &without_credential,
                CandidateFacts {
                    credential: CredentialState::Absent,
                    ..usable(1_726_000_000_000)
                },
            ),
        ]);
        let outcome = route_with(
            &request(
                &[unobserved.clone(), without_credential.clone()],
                SelectionScope::Workflow,
            ),
            Some(arc(&facts)),
            &admission(every_agent_allowed),
        );
        assert_eq!(outcome.selected(), None);
        assert_eq!(
            outcome.unavailable(),
            Some(&CandidateUnavailable::EveryCandidateExcluded {
                codes: vec![
                    ExclusionCode::AvailabilityUnobserved,
                    ExclusionCode::CredentialAbsent,
                ],
            })
        );
        let rationale = routing_rationale(&outcome);
        assert!(rationale.contains("availability_unobserved"), "{rationale}");
        assert!(rationale.contains("credential_absent"), "{rationale}");
    }

    #[test]
    fn an_unestablished_requirement_is_excluded_rather_than_read_as_satisfied() {
        let unknown_requirement = candidate("agent-a", "model-a");
        let satisfying = candidate("agent-b", "model-b");
        let facts = facts_with(&[
            (&unknown_requirement, usable(1_726_000_000_000)),
            (
                &satisfying,
                CandidateFacts {
                    requirements: BTreeMap::from([(
                        CandidateRequirement::Tools,
                        RequirementState::Satisfied,
                    )]),
                    ..usable(1_726_000_000_000)
                },
            ),
        ]);
        let mut routed = request(
            &[unknown_requirement.clone(), satisfying.clone()],
            SelectionScope::Workflow,
        );
        routed.requirements = vec![CandidateRequirement::Tools];
        let outcome = route_with(&routed, Some(arc(&facts)), &admission(every_agent_allowed));
        assert_eq!(outcome.selected(), Some(&satisfying));
        assert_eq!(
            outcome
                .decision
                .exclusion_for(&unknown_requirement)
                .map(CandidateExclusion::code),
            Some(ExclusionCode::RequirementUnknown)
        );
    }

    #[test]
    fn a_routed_decision_never_names_a_candidate_outside_the_allowed_set() {
        let allowed = candidate("agent-a", "model-a");
        let blocked = candidate("agent-b", "model-b");
        let facts = facts_with(&[
            (&allowed, usable(1_726_000_000_000)),
            (&blocked, usable(1_726_000_000_999)),
        ]);
        let outcome = route_with(
            &request(
                &[allowed.clone(), blocked.clone()],
                SelectionScope::Workflow,
            ),
            Some(arc(&facts)),
            &admission(|agent, _scope, _params| {
                if agent == "agent-a" {
                    ScopeAdmissionFacts::new(ScopeOutcomeState::Allowed, "admitted")
                } else {
                    ScopeAdmissionFacts::new(ScopeOutcomeState::Blocked, "not_admitted")
                }
            }),
        );
        assert!(
            outcome
                .decision
                .ranked
                .iter()
                .all(|entry| outcome.allowed.contains(&entry.candidate))
        );
        assert_eq!(outcome.selected(), Some(&allowed));
    }

    #[test]
    fn the_entry_owner_routes_through_the_facts_and_admission_it_was_built_with() {
        let only = candidate("agent-a", "model-a");
        let facts = facts_with(&[(&only, usable(1_726_000_000_000))]);
        let owner = RoutingWithFacts::new(arc(&facts), admission(workflow_only));
        let outcome = owner.route(&request(
            std::slice::from_ref(&only),
            SelectionScope::Workflow,
        ));
        assert_eq!(outcome.selected(), Some(&only));
        let blocked = owner.route(&request(
            std::slice::from_ref(&only),
            SelectionScope::Direct,
        ));
        assert_eq!(blocked.selected(), None);
    }

    #[test]
    fn the_composed_admission_port_states_no_outcome_when_no_owner_was_installed() {
        // A process that composed no admission owner still answers one port,
        // and it never answers `Allowed`.
        let port = scope_admission_port();
        for scope in SelectionScope::ALL {
            let facts =
                (port.agent_scope_admission)("codex", scope, &Value::Object(Default::default()));
            assert_ne!(facts.state, ScopeOutcomeState::Allowed);
            assert!(!facts.reason.trim().is_empty());
        }
    }
}
