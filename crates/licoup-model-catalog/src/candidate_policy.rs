//! Non-learning candidate routing: which allowed alternative may run an
//! admitted request, and one named reason for every alternative that may not.
//!
//! A request that reaches this module has already been admitted by its owner:
//! the effective grants, the account, the scope and the cost or disclosure
//! limits were decided above the catalogue, and this module is handed the
//! alternatives those grants allow. What remains here is a catalogue question
//! — among the alternatives that are already allowed, which one may run now —
//! and the answer is a recommendation, never a permission. The rules the
//! module exists to keep:
//!
//! - **Only allowed alternatives are considered.** [`CandidateRequest::allowed`]
//!   is the admission owner's answer. A candidate absent from it is never
//!   ranked, never substituted and never returned, whatever facts exist for
//!   it. A recommendation therefore cannot widen execution authority, and a
//!   disallowed candidate can never be reached by a silent fallback: the
//!   preferred candidates the grants did not allow are reported back in
//!   [`CandidateDecision::disallowed_preference`].
//! - **A preference orders, it does not authorise.** The configured choice and
//!   the configured substitute path are read as an order over the allowed set,
//!   not as entries into it. When the explicit choice cannot run and a
//!   substitute is selected, the decision says so in
//!   [`CandidateDecision::substitute_for`].
//! - **Unknown is never consent.** A requirement nothing establishes, a
//!   credential the host cannot confirm, or a model no live source reported is
//!   excluded under its own name rather than read as satisfied, present or
//!   available. Nothing here fabricates availability, a credential or a
//!   capability.
//! - **Nothing is guessed.** Capability, model, credential and quota facts
//!   arrive through [`CandidatePolicyPort`]. This module owns no readiness
//!   rule, no provider table and no budget: a host that composes no owner
//!   answers [`CandidatePolicyPort::unavailable`] and selects nothing.
//! - **The order is deterministic.** The surviving candidates are ranked by the
//!   declared order first and by the stable identity tuple last, so two runs
//!   over the same facts return the same answer, and equally preferred
//!   candidates never depend on insertion order.
//!
//! Rejections are reported as one exclusion per candidate, chosen by a fixed
//! precedence: the request's requirements in declared order, then the
//! catalogue's observation, then the credential, then the quota window. A
//! candidate is never left out silently, and an admitted request whose every
//! candidate was excluded returns
//! [`CandidateUnavailable::EveryCandidateExcluded`] with the distinct codes
//! that produced it.

use crate::availability::ObservedAvailability;
use crate::port::CredentialState;
use std::collections::BTreeSet;
use std::sync::OnceLock;

/// The stable identity of one candidate alternative.
///
/// Agent, model and provider stay separate dimensions: the Agent runtime id,
/// the model selector and the serving provider are not one boolean, and a
/// provider is never inferred from a model name.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CandidateId {
    /// The Agent that would run the work.
    pub agent_id: String,
    /// The model selector that Agent would run.
    pub model_id: String,
    /// The serving provider, when the request declares one.
    pub provider_id: Option<String>,
}

impl CandidateId {
    pub fn new(
        agent_id: impl Into<String>,
        model_id: impl Into<String>,
        provider_id: Option<&str>,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            model_id: model_id.into(),
            provider_id: provider_id.map(str::to_owned),
        }
    }
}

/// One fact the admitted request requires of every candidate it considers.
///
/// The vocabulary is the published model-fact vocabulary: a model identity, a
/// modality, tool support, reasoning support or a context window. A declared
/// requirement is answered by the port owner, so a host whose vendor publishes
/// nothing keeps the unknown answer instead of a substituted default.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum CandidateRequirement {
    /// The candidate must serve this declared model identity.
    Model(String),
    InputModality(String),
    OutputModality(String),
    /// The candidate must declare tool support.
    Tools,
    /// The candidate must declare reasoning support.
    Reasoning,
    /// The candidate must declare at least this many context tokens.
    ContextTokensAtLeast(u32),
}

/// The owner's three-valued answer for one requirement.
///
/// The third value is the point: a requirement the owner declares nothing
/// about is neither satisfied nor violated, and this module never promotes it
/// into either.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RequirementState {
    /// The declared facts satisfy the requirement.
    Satisfied,
    /// The declared facts contradict the requirement. `declared` carries the
    /// published value that contradicts it, when the owner has one, so the
    /// rejection is inspectable rather than a bare refusal.
    Violated { declared: Option<String> },
    /// The owner declares nothing about the requirement for this candidate.
    Unknown,
}

/// What a quota source answered for one candidate.
///
/// `Unknown` is a real answer: a provider with no quota source, or a source
/// that could not answer, states no capacity. It is not exhaustion — absence
/// of a quota report is not a spent window — and it is not a claim of capacity
/// either, so it is recorded, never resolved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QuotaState {
    /// A quota source answered: this window still has capacity.
    Available { window: String },
    /// A quota source answered: this window is exhausted.
    Exhausted { window: String },
    /// No quota source answered for this candidate.
    Unknown,
}

/// One admitted request, with its effective grants already decided above.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateRequest {
    /// The identity of the admitted request or attempt this decision answers.
    pub request_id: String,
    /// The scope the admission was decided for. It is opaque here: the
    /// admission owner names its scopes, and this module never interprets one.
    pub scope: String,
    /// The facts every considered candidate must establish.
    pub requirements: Vec<CandidateRequirement>,
    /// The alternatives the effective grants already allow. Order is not
    /// significant; a candidate absent from this set is never considered.
    pub allowed: BTreeSet<CandidateId>,
    /// The configured order: the explicit choice first, then the authorized
    /// substitute path in configured order. Entries the grants do not allow are
    /// reported, never used.
    pub preference: Vec<CandidateId>,
}

impl CandidateRequest {
    pub fn new(request_id: impl Into<String>, scope: impl Into<String>) -> Self {
        Self {
            request_id: request_id.into(),
            scope: scope.into(),
            requirements: Vec::new(),
            allowed: BTreeSet::new(),
            preference: Vec::new(),
        }
    }

    /// The configured path with its repeated entries removed, so one declared
    /// choice occupies one step.
    fn preference_path(&self) -> Vec<CandidateId> {
        let mut path: Vec<CandidateId> = Vec::with_capacity(self.preference.len());
        for candidate in &self.preference {
            if !path.contains(candidate) {
                path.push(candidate.clone());
            }
        }
        path
    }
}

/// The named family an exclusion belongs to.
///
/// Three families are the disallowances this policy reports — a requirement
/// (capability or model) mismatch, an unusable credential and a spent quota
/// window — and the fourth is the catalogue's own observation gate, which is a
/// fact about this host rather than a policy judgement.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ExclusionCategory {
    /// The candidate contradicts a request requirement, or nothing establishes it.
    Mismatch,
    /// No live source on this host reported the candidate.
    Availability,
    /// No usable credential can be attributed to the candidate.
    Credential,
    /// The candidate's quota window is exhausted.
    Quota,
}

/// The stable code of one exclusion, for grouping and for the summary of an
/// all-excluded request.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ExclusionCode {
    RequirementViolated,
    RequirementUnknown,
    AvailabilityUnobserved,
    CredentialAbsent,
    CredentialUnknown,
    QuotaExhausted,
}

impl ExclusionCode {
    /// The stable wire value. A caller keys a report on this, never on the
    /// variant's position.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RequirementViolated => "requirement_violated",
            Self::RequirementUnknown => "requirement_unknown",
            Self::AvailabilityUnobserved => "availability_unobserved",
            Self::CredentialAbsent => "credential_absent",
            Self::CredentialUnknown => "credential_unknown",
            Self::QuotaExhausted => "quota_exhausted",
        }
    }
}

/// Why one candidate cannot be selected. Each variant is separately named, so
/// a report never collapses a spent window into a missing credential or an
/// unestablished fact into a refusal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CandidateExclusion {
    /// The candidate contradicts a requirement of the admitted request.
    RequirementViolated {
        requirement: CandidateRequirement,
        declared: Option<String>,
    },
    /// Nothing establishes the requirement for this candidate.
    RequirementUnknown { requirement: CandidateRequirement },
    /// No live source on this host reported the candidate.
    AvailabilityUnobserved,
    /// The host holds no credential the candidate may use.
    CredentialAbsent { provider_id: Option<String> },
    /// The host cannot establish whether the candidate has a usable credential.
    CredentialUnknown { provider_id: Option<String> },
    /// A quota source reported this window exhausted.
    QuotaExhausted {
        window: String,
        provider_id: Option<String>,
    },
}

impl CandidateExclusion {
    pub fn code(&self) -> ExclusionCode {
        match self {
            Self::RequirementViolated { .. } => ExclusionCode::RequirementViolated,
            Self::RequirementUnknown { .. } => ExclusionCode::RequirementUnknown,
            Self::AvailabilityUnobserved => ExclusionCode::AvailabilityUnobserved,
            Self::CredentialAbsent { .. } => ExclusionCode::CredentialAbsent,
            Self::CredentialUnknown { .. } => ExclusionCode::CredentialUnknown,
            Self::QuotaExhausted { .. } => ExclusionCode::QuotaExhausted,
        }
    }

    pub fn category(&self) -> ExclusionCategory {
        match self {
            Self::RequirementViolated { .. } | Self::RequirementUnknown { .. } => {
                ExclusionCategory::Mismatch
            }
            Self::AvailabilityUnobserved => ExclusionCategory::Availability,
            Self::CredentialAbsent { .. } | Self::CredentialUnknown { .. } => {
                ExclusionCategory::Credential
            }
            Self::QuotaExhausted { .. } => ExclusionCategory::Quota,
        }
    }
}

/// How a surviving candidate relates to the configured order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateRelation {
    /// The explicit configured choice: the first step of the configured path.
    Requested,
    /// A declared step of the configured substitute path, at its 1-based
    /// position. The order declares which substitute is preferred, so an
    /// upgrade or a downgrade is a position, never a guessed relation.
    ConfiguredSubstitute { position: usize },
    /// An allowed alternative the request did not prefer.
    Alternative,
}

/// One allowed candidate that survived, with its deterministic rank.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RankedCandidate {
    /// 1-based rank in the surviving order.
    pub rank: usize,
    pub candidate: CandidateId,
    pub relation: CandidateRelation,
    /// When a live source reported the candidate, for the observation this
    /// decision was taken against.
    pub observed_at_unix_ms: Option<u64>,
    /// The quota answer recorded for the candidate, kept three-valued.
    pub quota: QuotaState,
}

/// One considered candidate that was not selectable, with its own reason.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExcludedCandidate {
    pub candidate: CandidateId,
    pub exclusion: CandidateExclusion,
}

/// Why an admitted request has no selectable candidate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CandidateUnavailable {
    /// The effective grants allowed no candidate to consider.
    NoAllowedCandidate,
    /// Every considered candidate was excluded. `codes` names the distinct
    /// exclusions that occurred, sorted and deduplicated, so the reason is
    /// readable without walking each entry.
    EveryCandidateExcluded { codes: Vec<ExclusionCode> },
}

/// The ordering of one admitted request: who may run, who may not and why.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CandidateDecision {
    pub request_id: String,
    pub scope: String,
    /// The surviving allowed candidates, deterministically ranked.
    pub ranked: Vec<RankedCandidate>,
    /// Every considered candidate that was not selectable.
    pub excluded: Vec<ExcludedCandidate>,
    /// The recommended candidate: the first ranked entry. It is a
    /// recommendation within the allowed set, never an execution permission.
    pub selected: Option<CandidateId>,
    /// The explicit configured choice this selection substitutes for, when the
    /// request named one and it was not selected. A substitution is always
    /// reported, never silent.
    pub substitute_for: Option<CandidateId>,
    /// The preferred candidates the effective grants did not allow, in
    /// configured order. They are reported and never used.
    pub disallowed_preference: Vec<CandidateId>,
    /// Why nothing was selected. Present exactly when `selected` is `None`.
    pub unavailable: Option<CandidateUnavailable>,
}

impl CandidateDecision {
    /// The ranked entry that was recommended.
    pub fn selected_candidate(&self) -> Option<&RankedCandidate> {
        self.ranked.first()
    }

    /// The exclusion recorded for one candidate, when it was considered and
    /// rejected.
    pub fn exclusion_for(&self, candidate: &CandidateId) -> Option<&CandidateExclusion> {
        self.excluded
            .iter()
            .find(|excluded| &excluded.candidate == candidate)
            .map(|excluded| &excluded.exclusion)
    }

    pub fn is_available(&self) -> bool {
        self.selected.is_some()
    }
}

/// The owner's three-valued answer for one requirement of one candidate.
pub type RequirementAnswer =
    fn(candidate: &CandidateId, requirement: &CandidateRequirement) -> RequirementState;

/// Whether, and when, a live source on this host reported the candidate.
pub type CandidateAvailability = fn(candidate: &CandidateId) -> ObservedAvailability;

/// Whether the host holds a usable credential for the candidate's route.
///
/// The answer is per candidate because attribution is the owner's job: a route
/// that needs no credential may answer `Present`, and a route whose credential
/// the owner cannot attribute answers `Unknown`.
pub type CandidateCredential = fn(candidate: &CandidateId) -> CredentialState;

/// What a quota source answered for one candidate.
pub type CandidateQuota = fn(candidate: &CandidateId) -> QuotaState;

/// Every fact this module reads from a module composed above it.
///
/// It is a value of `fn` pointers rather than a trait, exactly like
/// [`crate::port::ModelCatalogPort`], so the policy keeps no state a caller did
/// not hand it and no member runs on a branch that does not need it.
/// [`Self::unavailable`] declares every fact unknown: a process that composes
/// no owner excludes every candidate instead of selecting one on a guess.
#[derive(Clone, Copy)]
pub struct CandidatePolicyPort {
    pub requirement: RequirementAnswer,
    pub availability: CandidateAvailability,
    pub credential: CandidateCredential,
    pub quota: CandidateQuota,
}

impl CandidatePolicyPort {
    /// No owner composed: every fact stays unknown.
    pub const fn unavailable() -> Self {
        Self {
            requirement: no_requirement_answer,
            availability: no_availability,
            credential: no_credential,
            quota: no_quota,
        }
    }
}

impl Default for CandidatePolicyPort {
    fn default() -> Self {
        Self::unavailable()
    }
}

fn no_requirement_answer(
    _candidate: &CandidateId,
    _requirement: &CandidateRequirement,
) -> RequirementState {
    RequirementState::Unknown
}

fn no_availability(_candidate: &CandidateId) -> ObservedAvailability {
    ObservedAvailability::Unknown
}

fn no_credential(_candidate: &CandidateId) -> CredentialState {
    CredentialState::Unknown
}

fn no_quota(_candidate: &CandidateId) -> QuotaState {
    QuotaState::Unknown
}

/// The fail-closed declaration, kept as a value so an unanswered process
/// borrows it instead of installing a second answer.
static FAIL_CLOSED: CandidatePolicyPort = CandidatePolicyPort::unavailable();
static INSTALLED: OnceLock<CandidatePolicyPort> = OnceLock::new();

/// Install this host's answers for the candidate policy port, once per process.
///
/// The composition that calls it lives at the `licoup-native` crate root, above
/// both layers: the declared registry holds the identity fact, the Gateway read
/// holds the route it observed, and the credential and quota owners answer for
/// their own facts. One answer per port: reinstalling the same answers is
/// accepted so a repeated startup is harmless, and a different second answer is
/// refused rather than silently replacing the first. A process that never calls
/// it keeps every member fail-closed, so nothing is ever selected on a guess.
pub fn install_candidate_policy_port(port: CandidatePolicyPort) -> Result<(), &'static str> {
    if let Some(installed) = INSTALLED.get() {
        return if same_answers(installed, &port) {
            Ok(())
        } else {
            Err("candidate_policy_port_already_installed")
        };
    }
    INSTALLED
        .set(port)
        .map_err(|_| "candidate_policy_port_already_installed")
}

/// The installed answers, or the fail-closed declaration when none was
/// installed.
pub fn candidate_policy_port() -> &'static CandidatePolicyPort {
    INSTALLED.get().unwrap_or(&FAIL_CLOSED)
}

fn same_answers(left: &CandidatePolicyPort, right: &CandidatePolicyPort) -> bool {
    std::ptr::fn_addr_eq(left.requirement, right.requirement)
        && std::ptr::fn_addr_eq(left.availability, right.availability)
        && std::ptr::fn_addr_eq(left.credential, right.credential)
        && std::ptr::fn_addr_eq(left.quota, right.quota)
}

/// Order one admitted request's allowed alternatives and name every rejection.
///
/// The decision is total: a request with no eligible candidate is a value
/// ([`CandidateUnavailable`]), not an error, so a caller always receives the
/// reasons rather than a failure it might be tempted to retry elsewhere.
pub fn select_candidates(
    port: &CandidatePolicyPort,
    request: &CandidateRequest,
) -> CandidateDecision {
    let path = request.preference_path();
    let considered = considered_order(request, &path);
    let disallowed_preference = path
        .iter()
        .filter(|candidate| !request.allowed.contains(*candidate))
        .cloned()
        .collect::<Vec<_>>();

    let mut ranked = Vec::with_capacity(considered.len());
    let mut excluded = Vec::new();
    for candidate in considered {
        match exclusion_for(port, request, &candidate) {
            Some(exclusion) => excluded.push(ExcludedCandidate {
                candidate,
                exclusion,
            }),
            None => {
                let availability = (port.availability)(&candidate);
                ranked.push(RankedCandidate {
                    rank: ranked.len() + 1,
                    relation: relation_for(&path, &candidate),
                    observed_at_unix_ms: availability.observed_at_unix_ms(),
                    quota: (port.quota)(&candidate),
                    candidate,
                });
            }
        }
    }

    let selected = ranked.first().map(|entry| entry.candidate.clone());
    // A substitution is reported only when something was actually selected in
    // the explicit choice's place: an unavailable request made no substitution.
    let substitute_for = selected
        .as_ref()
        .and_then(|selected| path.first().filter(|head| *head != selected).cloned());
    let unavailable = if request.allowed.is_empty() {
        Some(CandidateUnavailable::NoAllowedCandidate)
    } else if ranked.is_empty() {
        Some(CandidateUnavailable::EveryCandidateExcluded {
            codes: distinct_codes(&excluded),
        })
    } else {
        None
    };

    CandidateDecision {
        request_id: request.request_id.clone(),
        scope: request.scope.clone(),
        ranked,
        excluded,
        selected,
        substitute_for,
        disallowed_preference,
        unavailable,
    }
}

/// The allowed candidates in the configured order, then every remaining
/// allowed candidate in stable identity order.
///
/// The configured path is deduplicated, so a repeated declared choice occupies
/// one position. The remaining candidates come from an ordered set keyed by
/// `(agent_id, model_id, provider_id)`, so equally preferred candidates break
/// their tie by identity rather than by insertion.
fn considered_order(request: &CandidateRequest, path: &[CandidateId]) -> Vec<CandidateId> {
    let mut considered = Vec::with_capacity(request.allowed.len());
    for candidate in path {
        if request.allowed.contains(candidate) && !considered.contains(candidate) {
            considered.push(candidate.clone());
        }
    }
    for candidate in &request.allowed {
        if !considered.contains(candidate) {
            considered.push(candidate.clone());
        }
    }
    considered
}

/// The first reason this candidate cannot run, in fixed precedence order.
fn exclusion_for(
    port: &CandidatePolicyPort,
    request: &CandidateRequest,
    candidate: &CandidateId,
) -> Option<CandidateExclusion> {
    for requirement in &request.requirements {
        match (port.requirement)(candidate, requirement) {
            RequirementState::Satisfied => {}
            RequirementState::Violated { declared } => {
                return Some(CandidateExclusion::RequirementViolated {
                    requirement: requirement.clone(),
                    declared,
                });
            }
            RequirementState::Unknown => {
                return Some(CandidateExclusion::RequirementUnknown {
                    requirement: requirement.clone(),
                });
            }
        }
    }
    if !(port.availability)(candidate).is_observed() {
        return Some(CandidateExclusion::AvailabilityUnobserved);
    }
    match (port.credential)(candidate) {
        CredentialState::Present => {}
        CredentialState::Absent => {
            return Some(CandidateExclusion::CredentialAbsent {
                provider_id: candidate.provider_id.clone(),
            });
        }
        CredentialState::Unknown => {
            return Some(CandidateExclusion::CredentialUnknown {
                provider_id: candidate.provider_id.clone(),
            });
        }
    }
    match (port.quota)(candidate) {
        QuotaState::Exhausted { window } => {
            return Some(CandidateExclusion::QuotaExhausted {
                window,
                provider_id: candidate.provider_id.clone(),
            });
        }
        QuotaState::Available { .. } | QuotaState::Unknown => {}
    }
    None
}

fn relation_for(path: &[CandidateId], candidate: &CandidateId) -> CandidateRelation {
    match path.iter().position(|preferred| preferred == candidate) {
        Some(0) => CandidateRelation::Requested,
        Some(position) => CandidateRelation::ConfiguredSubstitute {
            position: position + 1,
        },
        None => CandidateRelation::Alternative,
    }
}

fn distinct_codes(excluded: &[ExcludedCandidate]) -> Vec<ExclusionCode> {
    let mut codes = excluded
        .iter()
        .map(|excluded| excluded.exclusion.code())
        .collect::<Vec<_>>();
    codes.sort_unstable();
    codes.dedup();
    codes
}
