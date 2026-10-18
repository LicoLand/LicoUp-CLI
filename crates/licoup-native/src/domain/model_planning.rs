//! Comparable rankings for Agent + Model + Thinking planning options, a
//! replaceable planning/qualification strategy boundary, and revocable learned
//! defaults that can only select from the caller's candidate set.
//!
//! Planning behaviour stays here: the ranking basis, the comparison contract,
//! the qualification policy and the learned defaults are this module's own.
//! The facts it reads come from their owners — the curated intelligence catalog
//! and the catalogue's planning inputs — and never from a reader of its own. A
//! missing price or intelligence score fails the ranking; it is never replaced
//! by a zero, an average or a similar name.

use super::agent_intelligence_catalog::{
    agent_model_benchmark, model_intelligence,
    qualification::{EvidenceClass, QualificationPolicy},
};
use licoup_model_catalog::planning::{
    PlanningModelPrice, agent_model_planning_price, model_planning_price,
};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashSet},
};

const INPUT_SHARE: f64 = 0.75;
const OUTPUT_SHARE: f64 = 0.25;
const CACHE_HIT_RATE: f64 = 0.90;
const CACHE_CREATE_RATE: f64 = 1.0 - CACHE_HIT_RATE;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AgentModelOption {
    pub agent_id: String,
    pub model_id: String,
    pub thinking: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RankingBasis {
    ArtificialAnalysisCodingAgents,
    ModelIntelligenceAndRoutePrice,
    UserConfigured,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RankingPriceSource {
    ArtificialAnalysisCodingAgents,
    Agent,
    ModelApi,
    UserConfigured,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RankedAgentModel {
    pub rank: usize,
    pub option: AgentModelOption,
    pub price_source: RankingPriceSource,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentModelRanking {
    pub basis: RankingBasis,
    pub entries: Vec<RankedAgentModel>,
}

/// The immutable, user-visible scope in which a learned default may apply.
///
/// A scope is deliberately just a task/configuration key. It carries no
/// permission, budget, conversation, or runtime state, so adopting a default
/// cannot widen the work that a caller is already allowed to perform.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PlanningScope {
    pub task_kind: String,
    pub configuration: String,
}

impl PlanningScope {
    pub fn new(
        task_kind: impl Into<String>,
        configuration: impl Into<String>,
    ) -> Result<Self, StrategyMetadataError> {
        let scope = Self {
            task_kind: task_kind.into(),
            configuration: configuration.into(),
        };
        if scope.task_kind.trim().is_empty() || scope.configuration.trim().is_empty() {
            return Err(StrategyMetadataError::EmptyScope);
        }
        Ok(scope)
    }
}

/// A source reference retained with an adopted default. The source is an
/// opaque evidence identifier and revision, not a copy of any model catalog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StrategySource {
    pub source_id: String,
    pub revision: String,
}

impl StrategySource {
    pub fn new(
        source_id: impl Into<String>,
        revision: impl Into<String>,
    ) -> Result<Self, StrategyMetadataError> {
        let source = Self {
            source_id: source_id.into(),
            revision: revision.into(),
        };
        if source.source_id.trim().is_empty() || source.revision.trim().is_empty() {
            return Err(StrategyMetadataError::EmptySource);
        }
        Ok(source)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StrategyMetadataError {
    EmptyScope,
    EmptySource,
}

/// Coverage required before a comparison may become a product default.
///
/// These flags make the comparison contract explicit: a cheaper or successful
/// happy-path sample is not enough when failures, cancellation, in-flight work,
/// rework, or late charges were not observed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OutcomeCoverage {
    pub failures: bool,
    pub cancellations: bool,
    pub in_flight: bool,
    pub rework: bool,
    pub late_charges: bool,
}

impl OutcomeCoverage {
    pub const fn complete() -> Self {
        Self {
            failures: true,
            cancellations: true,
            in_flight: true,
            rework: true,
            late_charges: true,
        }
    }

    fn is_complete(self) -> bool {
        self == Self::complete()
    }
}

/// Aggregated, paired evidence for replacing one planning option with another.
///
/// The caller supplies facts already observed by the existing qualification and
/// usage owners. This type does not create an evaluation session, read model
/// data, or reserve execution resources.
#[derive(Clone, Debug, PartialEq)]
pub struct ComparableOutcome {
    /// Stable identity for the candidate strategy version.
    pub strategy_id: String,
    pub candidate: AgentModelOption,
    pub baseline: AgentModelOption,
    pub scope: PlanningScope,
    pub source: StrategySource,
    pub paired_cases: u64,
    pub candidate_accepted: u64,
    pub baseline_accepted: u64,
    pub candidate_cost_per_accepted_outcome: Option<f64>,
    pub baseline_cost_per_accepted_outcome: Option<f64>,
    pub candidate_corrections: u64,
    pub baseline_corrections: u64,
    pub candidate_latency_p95_ms: Option<u64>,
    pub baseline_latency_p95_ms: Option<u64>,
    pub coverage: OutcomeCoverage,
    pub evidence_class: EvidenceClass,
    /// The existing qualification owner sets this after applying its policy.
    pub policy_pass: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StrategyQualificationFailure {
    SyntheticEvidence,
    QualificationPolicyFailed,
    InsufficientCases,
    IncompleteCoverage,
    SuccessRegression,
    UnknownEconomics,
    CostNotImproved,
    ReworkRegression,
    LatencyRegression,
}

/// Qualification is a replaceable policy boundary. The default implementation
/// below uses the existing qualification policy; an owner may provide another
/// strategy without changing model identity, permissions, or budget handling.
pub trait StrategyQualification {
    fn qualify(&self, outcome: &ComparableOutcome) -> Result<(), StrategyQualificationFailure>;
}

#[derive(Clone, Debug)]
pub struct RegistryQualificationStrategy {
    policy: QualificationPolicy,
}

impl Default for RegistryQualificationStrategy {
    fn default() -> Self {
        Self {
            policy: QualificationPolicy::draft_1(),
        }
    }
}

impl RegistryQualificationStrategy {
    pub fn new(policy: QualificationPolicy) -> Self {
        Self { policy }
    }

    pub fn policy(&self) -> &QualificationPolicy {
        &self.policy
    }
}

impl StrategyQualification for RegistryQualificationStrategy {
    fn qualify(&self, outcome: &ComparableOutcome) -> Result<(), StrategyQualificationFailure> {
        if outcome.evidence_class != EvidenceClass::LiveAuthorized {
            return Err(StrategyQualificationFailure::SyntheticEvidence);
        }
        if !outcome.policy_pass {
            return Err(StrategyQualificationFailure::QualificationPolicyFailed);
        }
        if outcome.paired_cases < 2
            || outcome.candidate_accepted > outcome.paired_cases
            || outcome.baseline_accepted > outcome.paired_cases
            || outcome.candidate_accepted == 0
            || outcome.baseline_accepted == 0
        {
            return Err(StrategyQualificationFailure::InsufficientCases);
        }
        if !outcome.coverage.is_complete() {
            return Err(StrategyQualificationFailure::IncompleteCoverage);
        }

        let cases = outcome.paired_cases as f64;
        let candidate_rate = outcome.candidate_accepted as f64 / cases;
        let baseline_rate = outcome.baseline_accepted as f64 / cases;
        if candidate_rate + self.policy.success_noninferiority_margin < baseline_rate {
            return Err(StrategyQualificationFailure::SuccessRegression);
        }

        let (Some(candidate_cost), Some(baseline_cost)) = (
            outcome.candidate_cost_per_accepted_outcome,
            outcome.baseline_cost_per_accepted_outcome,
        ) else {
            return Err(StrategyQualificationFailure::UnknownEconomics);
        };
        if !candidate_cost.is_finite()
            || !baseline_cost.is_finite()
            || candidate_cost < 0.0
            || baseline_cost <= 0.0
        {
            return Err(StrategyQualificationFailure::UnknownEconomics);
        }
        if candidate_cost / baseline_cost > self.policy.cost_per_accepted_outcome_ratio_max {
            return Err(StrategyQualificationFailure::CostNotImproved);
        }
        if outcome.candidate_corrections > outcome.baseline_corrections {
            return Err(StrategyQualificationFailure::ReworkRegression);
        }

        let (Some(candidate_latency), Some(baseline_latency)) = (
            outcome.candidate_latency_p95_ms,
            outcome.baseline_latency_p95_ms,
        ) else {
            return Err(StrategyQualificationFailure::UnknownEconomics);
        };
        if candidate_latency
            > baseline_latency
                .saturating_add(self.policy.foreground_added_latency_p95_ms_max.max(0.0) as u64)
        {
            return Err(StrategyQualificationFailure::LatencyRegression);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StrategyRevokeEntry {
    pub entry_id: String,
    pub strategy_id: String,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AdoptedPlanningDefault {
    pub strategy_id: String,
    pub option: AgentModelOption,
    pub source: StrategySource,
    pub scope: PlanningScope,
    pub revoke_entry: StrategyRevokeEntry,
    pub supersedes: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StrategyAdoptionError {
    InvalidStrategyId,
    InvalidMetadata,
    CandidateEqualsBaseline,
    StrategyAlreadyActive,
    StrategyAlreadyRevoked,
    StrategyNotActive,
    EmptyRevokeReason,
    Qualification(StrategyQualificationFailure),
}

/// In-memory future-default registry. Durable strategy state remains owned by
/// Adaptive Flywheel's store; this leaf only records a suggestion/default for
/// the current planning boundary and keeps a revocation entry.
#[derive(Clone, Debug, Default)]
pub struct PlanningDefaults {
    active: BTreeMap<PlanningScope, AdoptedPlanningDefault>,
    revoked: BTreeMap<String, StrategyRevokeEntry>,
}

impl PlanningDefaults {
    pub fn adopt(
        &mut self,
        outcome: ComparableOutcome,
    ) -> Result<AdoptedPlanningDefault, StrategyAdoptionError> {
        self.adopt_with(outcome, &RegistryQualificationStrategy::default())
    }

    pub fn adopt_with<Q: StrategyQualification>(
        &mut self,
        outcome: ComparableOutcome,
        qualification: &Q,
    ) -> Result<AdoptedPlanningDefault, StrategyAdoptionError> {
        if outcome.strategy_id.trim().is_empty() {
            return Err(StrategyAdoptionError::InvalidStrategyId);
        }
        if outcome.scope.task_kind.trim().is_empty()
            || outcome.scope.configuration.trim().is_empty()
            || outcome.source.source_id.trim().is_empty()
            || outcome.source.revision.trim().is_empty()
            || outcome.candidate.agent_id.trim().is_empty()
            || outcome.candidate.model_id.trim().is_empty()
            || outcome.candidate.thinking.trim().is_empty()
            || outcome.baseline.agent_id.trim().is_empty()
            || outcome.baseline.model_id.trim().is_empty()
            || outcome.baseline.thinking.trim().is_empty()
        {
            return Err(StrategyAdoptionError::InvalidMetadata);
        }
        if outcome.candidate == outcome.baseline {
            return Err(StrategyAdoptionError::CandidateEqualsBaseline);
        }
        if self.revoked.contains_key(&outcome.strategy_id) {
            return Err(StrategyAdoptionError::StrategyAlreadyRevoked);
        }
        if self
            .active
            .values()
            .any(|default| default.strategy_id == outcome.strategy_id)
        {
            return Err(StrategyAdoptionError::StrategyAlreadyActive);
        }
        qualification
            .qualify(&outcome)
            .map_err(StrategyAdoptionError::Qualification)?;

        let previous = self
            .active
            .get(&outcome.scope)
            .map(|default| default.strategy_id.clone());
        let revoke_entry = StrategyRevokeEntry {
            entry_id: format!("revoke:{}", outcome.strategy_id),
            strategy_id: outcome.strategy_id.clone(),
            reason: None,
        };
        let adopted = AdoptedPlanningDefault {
            strategy_id: outcome.strategy_id,
            option: outcome.candidate,
            source: outcome.source,
            scope: outcome.scope.clone(),
            revoke_entry,
            supersedes: previous,
        };
        self.active.insert(outcome.scope, adopted.clone());
        Ok(adopted)
    }

    pub fn active(&self, scope: &PlanningScope) -> Option<&AdoptedPlanningDefault> {
        self.active.get(scope)
    }

    pub fn revoke(
        &mut self,
        strategy_id: &str,
        reason: impl Into<String>,
    ) -> Result<StrategyRevokeEntry, StrategyAdoptionError> {
        let reason = reason.into();
        if reason.trim().is_empty() {
            return Err(StrategyAdoptionError::EmptyRevokeReason);
        }
        let scope = self
            .active
            .iter()
            .find(|(_, default)| default.strategy_id == strategy_id)
            .map(|(scope, _)| scope.clone())
            .ok_or(if self.revoked.contains_key(strategy_id) {
                StrategyAdoptionError::StrategyAlreadyRevoked
            } else {
                StrategyAdoptionError::StrategyNotActive
            })?;
        let default = self
            .active
            .remove(&scope)
            .expect("active default was found immediately before removal");
        let entry = StrategyRevokeEntry {
            entry_id: default.revoke_entry.entry_id,
            strategy_id: default.strategy_id,
            reason: Some(reason),
        };
        self.revoked
            .insert(entry.strategy_id.clone(), entry.clone());
        Ok(entry)
    }

    pub fn revocation(&self, strategy_id: &str) -> Option<&StrategyRevokeEntry> {
        self.revoked.get(strategy_id)
    }
}

#[derive(Clone, Debug)]
pub struct PlanningRequest {
    pub scope: PlanningScope,
    pub options: Vec<AgentModelOption>,
    /// An exact user choice is never replaced by an adopted default or a
    /// registry ranking. The option must already be in `options`.
    pub user_config: Option<AgentModelOption>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanningSelectionSource {
    UserConfig,
    AdoptedDefault {
        strategy_id: String,
        source: StrategySource,
        revoke_entry: StrategyRevokeEntry,
    },
    Registry,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlanningDecision {
    pub selected: AgentModelOption,
    pub ranking: AgentModelRanking,
    pub source: PlanningSelectionSource,
}

/// A replaceable planner that keeps the default qualification and adoption
/// policy outside the ranking implementation.
pub struct ModelPlanner<S = RegistryModelPlanningStrategy> {
    strategy: S,
    defaults: PlanningDefaults,
}

impl Default for ModelPlanner<RegistryModelPlanningStrategy> {
    fn default() -> Self {
        Self::new(RegistryModelPlanningStrategy)
    }
}

impl<S: ModelPlanningStrategy> ModelPlanner<S> {
    pub fn new(strategy: S) -> Self {
        Self {
            strategy,
            defaults: PlanningDefaults::default(),
        }
    }

    pub fn with_defaults(strategy: S, defaults: PlanningDefaults) -> Self {
        Self { strategy, defaults }
    }

    pub fn defaults(&self) -> &PlanningDefaults {
        &self.defaults
    }

    pub fn defaults_mut(&mut self) -> &mut PlanningDefaults {
        &mut self.defaults
    }

    pub fn adopt_default(
        &mut self,
        outcome: ComparableOutcome,
    ) -> Result<AdoptedPlanningDefault, StrategyAdoptionError> {
        self.defaults.adopt(outcome)
    }

    pub fn adopt_default_with<Q: StrategyQualification>(
        &mut self,
        outcome: ComparableOutcome,
        qualification: &Q,
    ) -> Result<AdoptedPlanningDefault, StrategyAdoptionError> {
        self.defaults.adopt_with(outcome, qualification)
    }

    pub fn revoke_default(
        &mut self,
        strategy_id: &str,
        reason: impl Into<String>,
    ) -> Result<StrategyRevokeEntry, StrategyAdoptionError> {
        self.defaults.revoke(strategy_id, reason)
    }

    pub fn plan(&self, request: &PlanningRequest) -> Result<PlanningDecision, RankingError> {
        validate_options(&request.options)?;
        if let Some(user_config) = request.user_config.as_ref() {
            if !request.options.iter().any(|option| option == user_config) {
                return Err(RankingError::UserSelectionUnavailable);
            }
            return Ok(PlanningDecision {
                selected: user_config.clone(),
                ranking: user_configured_ranking(user_config),
                source: PlanningSelectionSource::UserConfig,
            });
        }

        let ranking = self.strategy.rank(&request.options)?;
        validate_ranking_options(&request.options, &ranking)?;
        if let Some(default) = self.defaults.active(&request.scope)
            && ranking
                .entries
                .iter()
                .any(|entry| entry.option == default.option)
        {
            return Ok(PlanningDecision {
                selected: default.option.clone(),
                ranking,
                source: PlanningSelectionSource::AdoptedDefault {
                    strategy_id: default.strategy_id.clone(),
                    source: default.source.clone(),
                    revoke_entry: default.revoke_entry.clone(),
                },
            });
        }
        let selected = ranking
            .entries
            .first()
            .map(|entry| entry.option.clone())
            .ok_or(RankingError::EmptyInput)?;
        Ok(PlanningDecision {
            selected,
            ranking,
            source: PlanningSelectionSource::Registry,
        })
    }
}

pub trait ModelPlanningStrategy {
    fn rank(&self, options: &[AgentModelOption]) -> Result<AgentModelRanking, RankingError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RegistryModelPlanningStrategy;

impl ModelPlanningStrategy for RegistryModelPlanningStrategy {
    fn rank(&self, options: &[AgentModelOption]) -> Result<AgentModelRanking, RankingError> {
        rank_agent_models(options)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RankingError {
    EmptyInput,
    DuplicateOption { index: usize },
    UserSelectionUnavailable,
    StrategyReturnedUnavailableOption,
    MissingModelIntelligence { index: usize },
    MissingPrice { index: usize },
    InvalidCost { index: usize },
    IncomparablePriceUnits,
}

struct RankCandidate {
    option: AgentModelOption,
    intelligence: i64,
    cost: f64,
    ratio: f64,
    price_source: RankingPriceSource,
}

/// Rank one candidate set without returning the internal value ratio.
///
/// The Artificial Analysis Coding Agents path is used only when it covers the
/// complete input set. Otherwise every option uses Model Intelligence plus its
/// Agent price, falling back to the model's raw API price. The fallback cost
/// models one million planning tokens: 75% input, 25% output, and a 90% cache
/// hit rate within the input share.
pub fn rank_agent_models(options: &[AgentModelOption]) -> Result<AgentModelRanking, RankingError> {
    validate_options(options)?;

    let benchmarks = options
        .iter()
        .map(|option| agent_model_benchmark(&option.agent_id, &option.model_id, &option.thinking))
        .collect::<Option<Vec<_>>>();

    if let Some(benchmarks) = benchmarks {
        let candidates = options
            .iter()
            .cloned()
            .zip(benchmarks)
            .enumerate()
            .map(|(index, (option, benchmark))| {
                candidate(
                    index,
                    option,
                    benchmark.intelligence,
                    benchmark.cost_per_task_usd,
                    RankingPriceSource::ArtificialAnalysisCodingAgents,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(build_ranking(
            RankingBasis::ArtificialAnalysisCodingAgents,
            candidates,
        ));
    }

    rank_with_model_intelligence(options)
}

fn user_configured_ranking(option: &AgentModelOption) -> AgentModelRanking {
    AgentModelRanking {
        basis: RankingBasis::UserConfigured,
        entries: vec![RankedAgentModel {
            rank: 1,
            option: option.clone(),
            price_source: RankingPriceSource::UserConfigured,
        }],
    }
}

fn validate_options(options: &[AgentModelOption]) -> Result<(), RankingError> {
    if options.is_empty() {
        return Err(RankingError::EmptyInput);
    }
    let mut seen = HashSet::with_capacity(options.len());
    for (index, option) in options.iter().enumerate() {
        if !seen.insert(option) {
            return Err(RankingError::DuplicateOption { index });
        }
    }
    Ok(())
}

fn validate_ranking_options(
    options: &[AgentModelOption],
    ranking: &AgentModelRanking,
) -> Result<(), RankingError> {
    let available = options.iter().collect::<HashSet<_>>();
    if ranking
        .entries
        .iter()
        .any(|entry| !available.contains(&entry.option))
    {
        return Err(RankingError::StrategyReturnedUnavailableOption);
    }
    Ok(())
}

fn rank_with_model_intelligence(
    options: &[AgentModelOption],
) -> Result<AgentModelRanking, RankingError> {
    let mut candidates = Vec::with_capacity(options.len());
    let mut comparable_unit: Option<String> = None;

    for (index, option) in options.iter().cloned().enumerate() {
        let intelligence = model_intelligence(&option.model_id, &option.thinking)
            .ok_or(RankingError::MissingModelIntelligence { index })?;
        let (price, price_source) =
            agent_model_planning_price(&option.agent_id, &option.model_id, &option.thinking)
                .map(|price| (price, RankingPriceSource::Agent))
                .or_else(|| {
                    model_planning_price(&option.model_id)
                        .map(|price| (price, RankingPriceSource::ModelApi))
                })
                .ok_or(RankingError::MissingPrice { index })?;
        let cost = fallback_cost(&price);
        if cost > 0.0 {
            if let Some(unit) = comparable_unit.as_deref() {
                if unit != price.unit {
                    return Err(RankingError::IncomparablePriceUnits);
                }
            } else {
                comparable_unit = Some(price.unit.clone());
            }
        }
        candidates.push(candidate(index, option, intelligence, cost, price_source)?);
    }

    Ok(build_ranking(
        RankingBasis::ModelIntelligenceAndRoutePrice,
        candidates,
    ))
}

fn fallback_cost(price: &PlanningModelPrice) -> f64 {
    INPUT_SHARE * (CACHE_HIT_RATE * price.cached_input + CACHE_CREATE_RATE * price.input)
        + OUTPUT_SHARE * price.output
}

fn candidate(
    index: usize,
    option: AgentModelOption,
    intelligence: i64,
    cost: f64,
    price_source: RankingPriceSource,
) -> Result<RankCandidate, RankingError> {
    if intelligence < 0 || !cost.is_finite() || cost < 0.0 {
        return Err(RankingError::InvalidCost { index });
    }
    Ok(RankCandidate {
        option,
        intelligence,
        cost,
        ratio: intelligence as f64 / cost,
        price_source,
    })
}

fn build_ranking(basis: RankingBasis, mut candidates: Vec<RankCandidate>) -> AgentModelRanking {
    candidates.sort_by(compare_candidates);
    let mut previous_ratio: Option<f64> = None;
    let mut previous_rank = 0;
    let entries = candidates
        .into_iter()
        .enumerate()
        .map(|(index, candidate)| {
            let rank = if previous_ratio
                .is_some_and(|ratio| ratio.total_cmp(&candidate.ratio) == Ordering::Equal)
            {
                previous_rank
            } else {
                index + 1
            };
            previous_ratio = Some(candidate.ratio);
            previous_rank = rank;
            RankedAgentModel {
                rank,
                option: candidate.option,
                price_source: candidate.price_source,
            }
        })
        .collect();
    AgentModelRanking { basis, entries }
}

fn compare_candidates(left: &RankCandidate, right: &RankCandidate) -> Ordering {
    right
        .ratio
        .total_cmp(&left.ratio)
        .then_with(|| right.intelligence.cmp(&left.intelligence))
        .then_with(|| left.cost.total_cmp(&right.cost))
        .then_with(|| left.option.agent_id.cmp(&right.option.agent_id))
        .then_with(|| left.option.model_id.cmp(&right.option.model_id))
        .then_with(|| left.option.thinking.cmp(&right.option.thinking))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(agent_id: &str, model_id: &str, thinking: &str) -> AgentModelOption {
        AgentModelOption {
            agent_id: agent_id.into(),
            model_id: model_id.into(),
            thinking: thinking.into(),
        }
    }

    #[test]
    fn complete_coding_agent_coverage_uses_one_benchmark_path() {
        let ranking = rank_agent_models(&[
            option("codex", "gpt-5-6-sol", "high"),
            option("codex", "gpt-5-6-luna", "medium"),
        ])
        .unwrap();

        assert_eq!(ranking.basis, RankingBasis::ArtificialAnalysisCodingAgents);
        assert_eq!(ranking.entries[0].option.model_id, "gpt-5-6-luna");
        assert!(ranking.entries.iter().all(|entry| {
            entry.price_source == RankingPriceSource::ArtificialAnalysisCodingAgents
        }));
    }

    #[test]
    fn one_missing_coding_variant_downgrades_the_complete_set() {
        let ranking = rank_agent_models(&[
            option("codex", "gpt-5.6-terra", "max-with-fallback"),
            option("codex", "gpt-5.6-luna", "medium"),
        ])
        .unwrap();

        assert_eq!(ranking.basis, RankingBasis::ModelIntelligenceAndRoutePrice);
        assert_eq!(ranking.entries[0].option.model_id, "gpt-5.6-luna");
        assert!(
            ranking
                .entries
                .iter()
                .all(|entry| entry.price_source == RankingPriceSource::Agent)
        );
    }

    #[test]
    fn missing_agent_price_falls_back_to_the_model_api_price() {
        let ranking = rank_agent_models(&[
            option("hermes", "deepseek-v4-pro", "max"),
            option("openclaw", "deepseek-v4-flash", "max"),
        ])
        .unwrap();

        assert_eq!(ranking.entries[0].option.model_id, "deepseek-v4-flash");
        assert!(
            ranking
                .entries
                .iter()
                .all(|entry| entry.price_source == RankingPriceSource::ModelApi)
        );
    }

    #[test]
    fn fallback_cost_uses_input_output_and_cache_planning_shares() {
        let cost = fallback_cost(&PlanningModelPrice {
            input: 10.0,
            cached_input: 1.0,
            output: 20.0,
            unit: "usd_per_million_tokens".into(),
        });
        assert!((cost - 6.425).abs() < f64::EPSILON);
    }

    #[test]
    fn fallback_rejects_incomparable_non_zero_price_units() {
        assert_eq!(
            rank_agent_models(&[
                option("codex", "gpt-5-6-luna", "max-with-fallback"),
                option("hermes", "deepseek-v4-flash", "max"),
            ]),
            Err(RankingError::IncomparablePriceUnits)
        );
    }

    #[test]
    fn duplicate_options_are_not_ranked_twice() {
        let duplicate = option("codex", "gpt-5-6-luna", "medium");
        assert_eq!(
            rank_agent_models(&[duplicate.clone(), duplicate]),
            Err(RankingError::DuplicateOption { index: 1 })
        );
    }

    #[derive(Clone, Copy, Debug)]
    struct FirstOptionStrategy;

    impl ModelPlanningStrategy for FirstOptionStrategy {
        fn rank(&self, options: &[AgentModelOption]) -> Result<AgentModelRanking, RankingError> {
            validate_options(options)?;
            Ok(AgentModelRanking {
                basis: RankingBasis::UserConfigured,
                entries: options
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(index, option)| RankedAgentModel {
                        rank: index + 1,
                        option,
                        price_source: RankingPriceSource::UserConfigured,
                    })
                    .collect(),
            })
        }
    }

    #[derive(Clone, Copy, Debug)]
    struct OutOfScopeStrategy;

    impl ModelPlanningStrategy for OutOfScopeStrategy {
        fn rank(&self, _options: &[AgentModelOption]) -> Result<AgentModelRanking, RankingError> {
            Ok(AgentModelRanking {
                basis: RankingBasis::UserConfigured,
                entries: vec![RankedAgentModel {
                    rank: 1,
                    option: option("other-agent", "other-model", "high"),
                    price_source: RankingPriceSource::UserConfigured,
                }],
            })
        }
    }

    fn planning_scope() -> PlanningScope {
        PlanningScope::new("coding", "default").unwrap()
    }

    fn strategy_source() -> StrategySource {
        StrategySource::new("qualification:comparison-1", "policy-v1").unwrap()
    }

    fn comparable_outcome(
        candidate: AgentModelOption,
        baseline: AgentModelOption,
        evidence_class: EvidenceClass,
    ) -> ComparableOutcome {
        ComparableOutcome {
            strategy_id: "strategy:learned-1".into(),
            candidate,
            baseline,
            scope: planning_scope(),
            source: strategy_source(),
            paired_cases: 2,
            candidate_accepted: 2,
            baseline_accepted: 2,
            candidate_cost_per_accepted_outcome: Some(0.5),
            baseline_cost_per_accepted_outcome: Some(1.0),
            candidate_corrections: 0,
            baseline_corrections: 1,
            candidate_latency_p95_ms: Some(10),
            baseline_latency_p95_ms: Some(20),
            coverage: OutcomeCoverage::complete(),
            evidence_class,
            policy_pass: true,
        }
    }

    #[test]
    fn replaceable_planner_adopts_a_qualified_default_and_keeps_metadata() {
        let candidate = option("codex", "candidate", "high");
        let baseline = option("codex", "baseline", "medium");
        let mut planner = ModelPlanner::new(FirstOptionStrategy);
        let adopted = planner
            .adopt_default(comparable_outcome(
                candidate.clone(),
                baseline.clone(),
                EvidenceClass::LiveAuthorized,
            ))
            .unwrap();

        assert_eq!(adopted.option, candidate);
        assert_eq!(adopted.scope, planning_scope());
        assert_eq!(adopted.source, strategy_source());
        assert_eq!(adopted.revoke_entry.entry_id, "revoke:strategy:learned-1");
        assert_eq!(adopted.revoke_entry.reason, None);

        let decision = planner
            .plan(&PlanningRequest {
                scope: planning_scope(),
                options: vec![baseline.clone(), candidate.clone()],
                user_config: None,
            })
            .unwrap();
        assert_eq!(decision.selected, candidate);
        assert!(matches!(
            decision.source,
            PlanningSelectionSource::AdoptedDefault { .. }
        ));

        let user_decision = planner
            .plan(&PlanningRequest {
                scope: planning_scope(),
                options: vec![baseline.clone(), candidate],
                user_config: Some(baseline.clone()),
            })
            .unwrap();
        assert_eq!(user_decision.selected, baseline);
        assert_eq!(user_decision.source, PlanningSelectionSource::UserConfig);
        assert_eq!(user_decision.ranking.basis, RankingBasis::UserConfigured);
    }

    #[test]
    fn default_qualification_rejects_synthetic_and_incomplete_comparisons() {
        let candidate = option("codex", "candidate", "high");
        let baseline = option("codex", "baseline", "medium");
        let mut defaults = PlanningDefaults::default();
        assert_eq!(
            defaults
                .adopt(comparable_outcome(
                    candidate.clone(),
                    baseline.clone(),
                    EvidenceClass::Synthetic,
                ))
                .unwrap_err(),
            StrategyAdoptionError::Qualification(StrategyQualificationFailure::SyntheticEvidence)
        );

        let mut insufficient =
            comparable_outcome(candidate, baseline, EvidenceClass::LiveAuthorized);
        insufficient.paired_cases = 1;
        assert_eq!(
            defaults.adopt(insufficient).unwrap_err(),
            StrategyAdoptionError::Qualification(StrategyQualificationFailure::InsufficientCases)
        );
    }

    #[derive(Clone, Copy, Debug)]
    struct RejectingQualification;

    impl StrategyQualification for RejectingQualification {
        fn qualify(
            &self,
            _outcome: &ComparableOutcome,
        ) -> Result<(), StrategyQualificationFailure> {
            Err(StrategyQualificationFailure::QualificationPolicyFailed)
        }
    }

    #[test]
    fn adopt_with_honors_a_replacement_qualification_strategy() {
        let mut defaults = PlanningDefaults::default();
        let outcome = comparable_outcome(
            option("codex", "candidate", "high"),
            option("codex", "baseline", "medium"),
            EvidenceClass::LiveAuthorized,
        );

        assert_eq!(
            defaults
                .adopt_with(outcome, &RejectingQualification)
                .unwrap_err(),
            StrategyAdoptionError::Qualification(
                StrategyQualificationFailure::QualificationPolicyFailed
            )
        );
        assert!(defaults.active(&planning_scope()).is_none());
    }

    #[test]
    fn qualification_rejects_each_comparison_regression() {
        let cases: Vec<(fn(&mut ComparableOutcome), StrategyQualificationFailure)> = vec![
            (
                |outcome| outcome.coverage = OutcomeCoverage::default(),
                StrategyQualificationFailure::IncompleteCoverage,
            ),
            (
                |outcome| outcome.candidate_accepted = 1,
                StrategyQualificationFailure::SuccessRegression,
            ),
            (
                |outcome| outcome.candidate_cost_per_accepted_outcome = None,
                StrategyQualificationFailure::UnknownEconomics,
            ),
            (
                |outcome| outcome.candidate_cost_per_accepted_outcome = Some(1.0),
                StrategyQualificationFailure::CostNotImproved,
            ),
            (
                |outcome| outcome.candidate_corrections = 2,
                StrategyQualificationFailure::ReworkRegression,
            ),
            (
                |outcome| outcome.candidate_latency_p95_ms = Some(821),
                StrategyQualificationFailure::LatencyRegression,
            ),
        ];

        for (mutate, expected) in cases {
            let mut outcome = comparable_outcome(
                option("codex", "candidate", "high"),
                option("codex", "baseline", "medium"),
                EvidenceClass::LiveAuthorized,
            );
            mutate(&mut outcome);
            assert_eq!(
                PlanningDefaults::default().adopt(outcome).unwrap_err(),
                StrategyAdoptionError::Qualification(expected)
            );
        }
    }

    #[test]
    fn a_second_default_supersedes_and_keeps_the_relation() {
        let mut defaults = PlanningDefaults::default();
        let baseline = option("codex", "baseline", "medium");
        let mut first = comparable_outcome(
            option("codex", "first", "high"),
            baseline.clone(),
            EvidenceClass::LiveAuthorized,
        );
        first.strategy_id = "strategy:first".into();
        defaults.adopt(first).unwrap();

        let mut second = comparable_outcome(
            option("codex", "second", "high"),
            baseline,
            EvidenceClass::LiveAuthorized,
        );
        second.strategy_id = "strategy:second".into();
        let adopted = defaults.adopt(second).unwrap();

        assert_eq!(adopted.supersedes.as_deref(), Some("strategy:first"));
        assert_eq!(
            defaults
                .active(&planning_scope())
                .map(|default| default.strategy_id.as_str()),
            Some("strategy:second")
        );
        assert!(defaults.revocation("strategy:first").is_none());
        assert_eq!(
            defaults.revoke("strategy:first", "superseded").unwrap_err(),
            StrategyAdoptionError::StrategyNotActive
        );
    }

    #[test]
    fn revocation_requires_a_reason_and_blocks_readoption() {
        let mut defaults = PlanningDefaults::default();
        assert_eq!(
            defaults.revoke("strategy:missing", "reason").unwrap_err(),
            StrategyAdoptionError::StrategyNotActive
        );

        let outcome = comparable_outcome(
            option("codex", "candidate", "high"),
            option("codex", "baseline", "medium"),
            EvidenceClass::LiveAuthorized,
        );
        defaults.adopt(outcome.clone()).unwrap();
        assert_eq!(
            defaults.revoke("strategy:learned-1", "  ").unwrap_err(),
            StrategyAdoptionError::EmptyRevokeReason
        );
        defaults.revoke("strategy:learned-1", "regression").unwrap();
        assert_eq!(
            defaults.revoke("strategy:learned-1", "again").unwrap_err(),
            StrategyAdoptionError::StrategyAlreadyRevoked
        );
        assert_eq!(
            defaults.adopt(outcome).unwrap_err(),
            StrategyAdoptionError::StrategyAlreadyRevoked
        );
    }

    #[test]
    fn adoption_rejects_invalid_metadata_and_self_replacement() {
        let mut defaults = PlanningDefaults::default();
        let mut blank_id = comparable_outcome(
            option("codex", "candidate", "high"),
            option("codex", "baseline", "medium"),
            EvidenceClass::LiveAuthorized,
        );
        blank_id.strategy_id = " ".into();
        assert_eq!(
            defaults.adopt(blank_id).unwrap_err(),
            StrategyAdoptionError::InvalidStrategyId
        );

        let mut blank_thinking = comparable_outcome(
            option("codex", "candidate", "high"),
            option("codex", "baseline", "medium"),
            EvidenceClass::LiveAuthorized,
        );
        blank_thinking.candidate.thinking = " ".into();
        assert_eq!(
            defaults.adopt(blank_thinking).unwrap_err(),
            StrategyAdoptionError::InvalidMetadata
        );

        let mut self_replacement = comparable_outcome(
            option("codex", "candidate", "high"),
            option("codex", "baseline", "medium"),
            EvidenceClass::LiveAuthorized,
        );
        self_replacement.baseline = self_replacement.candidate.clone();
        assert_eq!(
            defaults.adopt(self_replacement).unwrap_err(),
            StrategyAdoptionError::CandidateEqualsBaseline
        );
    }

    #[test]
    fn revocation_only_changes_future_plans_and_does_not_expand_candidates() {
        let candidate = option("codex", "candidate", "high");
        let baseline = option("codex", "baseline", "medium");
        let mut planner = ModelPlanner::new(FirstOptionStrategy);
        let before = planner
            .adopt_default(comparable_outcome(
                candidate.clone(),
                baseline.clone(),
                EvidenceClass::LiveAuthorized,
            ))
            .and_then(|_| {
                planner
                    .plan(&PlanningRequest {
                        scope: planning_scope(),
                        options: vec![baseline.clone(), candidate.clone()],
                        user_config: None,
                    })
                    .map_err(|_| StrategyAdoptionError::StrategyNotActive)
            })
            .unwrap();
        assert_eq!(before.selected, candidate);

        let revoked = planner
            .revoke_default("strategy:learned-1", "new evidence")
            .unwrap();
        assert_eq!(revoked.reason.as_deref(), Some("new evidence"));
        assert_eq!(
            planner.defaults().revocation("strategy:learned-1"),
            Some(&revoked)
        );

        let after = planner
            .plan(&PlanningRequest {
                scope: planning_scope(),
                options: vec![baseline.clone(), candidate],
                user_config: None,
            })
            .unwrap();
        assert_eq!(after.selected, baseline);
        assert_eq!(after.source, PlanningSelectionSource::Registry);
        assert_eq!(before.selected.agent_id, "codex");
        assert!(
            planner
                .plan(&PlanningRequest {
                    scope: planning_scope(),
                    options: vec![option("codex", "baseline", "medium")],
                    user_config: None,
                })
                .is_ok()
        );
    }

    #[test]
    fn user_config_must_be_an_existing_candidate() {
        let configured = option("codex", "configured", "high");
        let error = ModelPlanner::new(FirstOptionStrategy)
            .plan(&PlanningRequest {
                scope: planning_scope(),
                options: vec![option("codex", "available", "medium")],
                user_config: Some(configured),
            })
            .unwrap_err();
        assert_eq!(error, RankingError::UserSelectionUnavailable);
    }

    #[test]
    fn replacement_strategy_cannot_expand_the_candidate_set() {
        let error = ModelPlanner::new(OutOfScopeStrategy)
            .plan(&PlanningRequest {
                scope: planning_scope(),
                options: vec![option("codex", "available", "medium")],
                user_config: None,
            })
            .unwrap_err();
        assert_eq!(error, RankingError::StrategyReturnedUnavailableOption);
    }
}
