//! The model selection catalogue: what a model identity is, what this host has
//! actually observed, and what it costs — kept apart and never guessed.
//!
//! Three kinds of fact live here, and the crate exists so that they stay
//! distinguishable:
//!
//! - **declared identity** ([`identity`]): the canonical model a selector
//!   names, its lab, its family and the provider aliases that reach it,
//!   including the historical aliases recorded usage is projected through.
//!   Identity is a declaration about names, not a claim that this host can run
//!   the model.
//! - **observed availability** ([`availability`]): which models a live source
//!   on this machine reported, with the time it reported them. An observation
//!   is evidence with a timestamp; its absence is `Unknown`, and nothing is
//!   ever synthesized as available from a name, a price row or an intelligence
//!   score.
//! - **recorded price facts** ([`pricing`]) and the [`planning`] inputs built
//!   on them. A missing route has no price; it is never priced at zero.
//!
//! [`selection`] is the one query that joins them, and it reports the two
//! dimensions separately: a declared model this host never observed is not
//! available, and an observed model with no declared identity is still
//! observed.
//!
//! [`selection_matrix`] is the client-facing join of that query with the
//! effective policy. Support, availability, credentials and execution are four
//! separate dimensions, each with its own state and reason, and execution is
//! decided once per request scope — so the same Agent can show a different
//! direct and workflow outcome, and neither is ever read as readiness.
//! [`candidate_policy`] is the non-learning routing query over those facts:
//! given an admitted request and the alternatives its effective grants already
//! allow, it ranks the survivors deterministically and names one exclusion for
//! every candidate that may not run — a requirement (capability or model)
//! mismatch, an unusable credential, a spent quota window, or a model no live
//! source reported. It recommends within the allowed set and never widens it.
//!
//! The crate reads nothing owned above it directly. Probes, credential states
//! and source generations arrive through [`port::ModelCatalogPort`], and the
//! effective per-scope admission through
//! [`selection_matrix::SelectionMatrixPort`]; this crate declares both and the
//! `licoup-native` crate root composes them. The crate's own fail-closed answer
//! is [`port::ModelCatalogPort::unavailable`] together with
//! [`selection_matrix::SelectionMatrixPort::unavailable`], and a process that
//! composes nothing keeps them.
//!
//! Nothing here reaches upward: the only LicoUp dependency is
//! `licoup-foundation`, which owns the path, atomic-write and display-name
//! primitives every consumer of these facts already stands on.

pub mod availability;
pub mod candidate_policy;
pub mod candidate_routing;
pub mod identity;
pub mod planning;
pub mod port;
pub mod pricing;
pub mod selection;
pub mod selection_matrix;

pub use availability::{
    ObservedAvailability, ObservedCatalog, ObservedCatalogCache, ObservedModel, now_unix_ms,
};
pub use candidate_policy::{
    CandidateAvailability, CandidateCredential, CandidateDecision, CandidateExclusion, CandidateId,
    CandidatePolicyPort, CandidateQuota, CandidateRelation, CandidateRequest, CandidateRequirement,
    CandidateUnavailable, ExcludedCandidate, ExclusionCategory, ExclusionCode, QuotaState,
    RankedCandidate, RequirementAnswer, RequirementState, candidate_policy_port,
    install_candidate_policy_port, select_candidates,
};
pub use candidate_routing::{
    CandidateFactSource, CandidateFactTable, CandidateFacts, CandidateOffer, CandidateRoutingGap,
    CandidateRoutingOutcome, CandidateRoutingPort, CandidateRoutingRequest, PolicyCandidateRouting,
    RoutingWithFacts, admission_facts, install_candidate_facts, install_scope_admission,
    installed_candidate_facts, route_candidates, route_with, routing_rationale,
    scope_admission_port,
};
pub use identity::{
    CanonicalModel, RegistrySnapshot, SnapshotProvenance, model_display_name, snapshot_report,
};
pub use planning::{PlanningModelPrice, planning_agent_model_price, planning_model_price};
pub use port::{
    CatalogSourceGeneration, CredentialState, ModelCatalogPort, install_model_catalog_port,
    model_catalog_port,
};
pub use pricing::{ModelTokenPrice, agent_model_price, model_price};
pub use selection::{
    SelectionEvidence, SelectionFacts, SelectionReport, declared_facts, selection_facts,
    selection_report, unknown_facts,
};
pub use selection_matrix::{
    AvailabilityState, SELECTION_MATRIX_SCHEMA_VERSION, ScopeAdmission, ScopeAdmissionFacts,
    ScopeOutcome, ScopeOutcomeState, SelectionMatrix, SelectionMatrixEntry, SelectionMatrixPort,
    SelectionScope, SupportState, selection_matrix, selection_matrix_document,
};
