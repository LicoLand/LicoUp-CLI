//! Suggestions from real history, applied to future work only.
//!
//! The seam types and the revocable default facts belong to the evolution owner
//! ([`crate::domain::workflow_runtime::evolution`]): `PlanningScopeSeam`,
//! `AgentModelOptionSeam`, `StrategySourceSeam`, `StrategySuggestion`,
//! `AdoptedPlanningDefaultSeam` and `DefaultEvolutionStrategyPort`, which holds
//! the per-scope adoption history and its revocation. This adapter owns no
//! default of its own: adopting and revoking call the owner's mutation API and
//! every read goes through the owner's `current_default`, so an adoption made
//! here is the same fact the owner's own `suggest_strategy` sees, and an
//! adoption made directly on the owner is the same fact this adapter suggests.
//! What this adapter adds is the part the seam does not carry:
//!
//! - **History-shaped suggestions.** Real outcome counts, rework and late
//!   settlement facts from the usage ledger become a proposal for work that has
//!   not been admitted yet. Adoption and revocation reach future selection only:
//!   this path holds no in-flight state, so it cannot rewrite a binding that was
//!   already admitted.
//! - **Honest missing states.** An empty candidate catalog, a user choice the
//!   installed catalog cannot run, or too little history each answer with their
//!   own state instead of a made-up model.
//!
//! # What is never claimed
//!
//! [`EvidenceClaim::NotMeasured`] is the only claim this adapter can make. It
//! has outcome counts for a scope, not a controlled comparison, so a suggestion
//! never says an option performs better — the mechanism can be correct while the
//! statistical question stays open. A suggestion also never grants permission:
//! every [`StrategySuggestion`] this adapter builds keeps
//! `has_execution_permission: false`, and the authorization context is reported
//! alongside the suggestion instead of being implied by it.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::domain::workflow_runtime::evolution::{
    AdoptedPlanningDefaultSeam, AgentModelOptionSeam, DefaultEvolutionStrategyPort,
    EvolutionStrategyPort, PlanningScopeSeam, StrategySourceSeam, StrategySuggestion,
};
use crate::domain::workflow_store::StrategyAuthorization;

use super::budget::SettlementReceipt;
use super::observation::{EffectOutcome, MeteringFacts};
use super::{AdapterError, AdapterName, AdapterResult, FactState};

const ADAPTER: AdapterName = AdapterName::Policy;

/// How many observed outcomes an option needs before history may propose it.
///
/// This is a floor so that a single lucky run cannot become a default. It is not
/// a significance test and the adapter does not present it as one.
pub const MINIMUM_HISTORY_SAMPLES: u64 = 4;

/// What this adapter is allowed to claim about a suggestion.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceClaim {
    /// Outcome counts only. No measured improvement is claimed, and none may be
    /// added to this type without evidence this adapter does not have.
    #[default]
    NotMeasured,
}

impl EvidenceClaim {
    pub const fn code(self) -> &'static str {
        match self {
            Self::NotMeasured => "not-measured",
        }
    }

    /// Always false: a correct mechanism is not a proven benefit.
    pub const fn claims_measured_improvement(self) -> bool {
        false
    }
}

/// Which work an adopted default reaches.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DefaultApplication {
    /// Work not yet admitted. Never the request currently in flight.
    FutureWork,
}

impl DefaultApplication {
    pub const fn code(self) -> &'static str {
        match self {
            Self::FutureWork => "future-work",
        }
    }

    /// Always false. Adoption is not a rewrite of a running effect.
    pub const fn applies_to_in_flight(self) -> bool {
        false
    }
}

/// The authorization context a suggestion was produced under.
///
/// It can only be built from a real [`StrategyAuthorization`] supplied by the
/// trusted session domain. Provenance metadata — an agent label, a model label,
/// an imported usage marker — cannot produce one, because knowing where a
/// number came from says nothing about who authorized the work.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "authority")]
pub enum AuthorityContextState {
    /// No authorization fact was supplied, so no automatic suggestion is made
    /// from history or ranking.
    NotEvaluated,
    /// A real authorization fact for the work's definition revision.
    Resolved {
        authorization_digest: String,
        semantics_digest: String,
        revision: u64,
        active: bool,
    },
}

impl AuthorityContextState {
    pub fn from_authorization(authorization: &StrategyAuthorization) -> Self {
        Self::Resolved {
            authorization_digest: authorization.authorization_digest.clone(),
            semantics_digest: authorization.semantics_digest.clone(),
            revision: authorization.revision,
            active: authorization.active,
        }
    }

    pub const fn state(&self) -> FactState {
        match self {
            Self::NotEvaluated => FactState::NotConfigured,
            Self::Resolved { .. } => FactState::Present,
        }
    }

    /// Whether automatic (non-user) suggestions may be made under this context.
    pub const fn permits_automatic_suggestion(&self) -> bool {
        matches!(self, Self::Resolved { active: true, .. })
    }
}

/// The grain history is recorded at.
///
/// The usage ledger records the agent and model an effect ran with; the thinking
/// setting is not part of that durable fact, so history is compared at this
/// grain and a candidate matches when its agent and model match.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionKey {
    pub agent_id: String,
    pub model_id: String,
}

impl OptionKey {
    pub fn of(option: &AgentModelOptionSeam) -> Self {
        Self {
            agent_id: option.agent_id.clone(),
            model_id: option.model_id.clone(),
        }
    }

    pub fn matches(&self, option: &AgentModelOptionSeam) -> bool {
        self.agent_id == option.agent_id && self.model_id == option.model_id
    }

    fn map_key(&self) -> String {
        format!("{}\u{1f}{}", self.agent_id, self.model_id)
    }
}

/// Observed outcomes for one option.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OptionHistory {
    pub agent_id: String,
    pub model_id: String,
    pub succeeded: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub unknown: u64,
    pub rework: u64,
}

impl OptionHistory {
    /// Effects observed for this option, whatever their outcome.
    pub const fn samples(&self) -> u64 {
        self.succeeded + self.failed + self.cancelled + self.unknown
    }
}

/// Real history for a planning scope.
///
/// Built from the base metering facts of the observation adapter — not from its
/// optional statistics projection — plus late settlement facts from the budget
/// adapter.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyHistory {
    pub succeeded: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub unknown: u64,
    pub rework: u64,
    /// Settlements that arrived after an unknown outcome had been recorded.
    pub late_settlements: u64,
    /// Tokens charged above the reserved estimate.
    pub overage_tokens: u64,
    pub options: BTreeMap<String, OptionHistory>,
}

impl PolicyHistory {
    /// Count the durable metering facts of one run into history.
    pub fn from_metering(facts: &MeteringFacts) -> Self {
        let mut history = Self::default();
        for effect in &facts.effects {
            let Some(outcome) = effect.outcome else {
                continue;
            };
            match outcome {
                EffectOutcome::Succeeded => history.succeeded += 1,
                EffectOutcome::Failed => history.failed += 1,
                EffectOutcome::Cancelled => history.cancelled += 1,
                EffectOutcome::Unknown => history.unknown += 1,
            }
            if effect.attempt > 1 {
                history.rework += 1;
            }
            let (Some(agent_id), Some(model_id)) = (&effect.agent_id, &effect.model) else {
                continue;
            };
            let key = OptionKey {
                agent_id: agent_id.clone(),
                model_id: model_id.clone(),
            }
            .map_key();
            let option = history.options.entry(key).or_insert_with(|| OptionHistory {
                agent_id: agent_id.clone(),
                model_id: model_id.clone(),
                ..OptionHistory::default()
            });
            match outcome {
                EffectOutcome::Succeeded => option.succeeded += 1,
                EffectOutcome::Failed => option.failed += 1,
                EffectOutcome::Cancelled => option.cancelled += 1,
                EffectOutcome::Unknown => option.unknown += 1,
            }
            if effect.attempt > 1 {
                option.rework += 1;
            }
        }
        history
    }

    /// Fold one settlement into history. A late or over-estimate settlement is
    /// a cost fact about the option that ran, not a verdict on it.
    pub fn observe_settlement(&mut self, receipt: &SettlementReceipt) {
        if receipt.late {
            self.late_settlements += 1;
        }
        self.overage_tokens += receipt.overage_tokens;
    }

    /// Total observed effects.
    pub const fn samples(&self) -> u64 {
        self.succeeded + self.failed + self.cancelled + self.unknown
    }

    pub fn option(&self, key: &OptionKey) -> Option<&OptionHistory> {
        self.options.get(&key.map_key())
    }
}

/// One request for a suggestion about future work.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyRequest {
    pub scope: PlanningScopeSeam,
    /// The options the host can actually run. A suggestion never leaves this
    /// set.
    pub candidates: Vec<AgentModelOptionSeam>,
    /// The user's own configuration for this scope, when they made one.
    pub user_configuration: Option<AgentModelOptionSeam>,
    pub authority: AuthorityContextState,
    pub history: PolicyHistory,
}

/// Why a suggestion was made.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "basis")]
pub enum SuggestionBasis {
    /// The user's own configuration, which outranks everything below.
    UserConfiguration,
    /// A default adopted for future work, with the owner's future-default
    /// revision in force when this suggestion was made.
    AdoptedFutureDefault {
        source: StrategySourceSeam,
        revision: u64,
    },
    /// Outcome counts from real history.
    History {
        option_samples: u64,
        required_samples: u64,
    },
    /// The evolution owner's ranking seam answered, with its own basis string.
    OwnerRanking { ranking_basis: String },
}

/// The answer to a request for a suggestion.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "outcome")]
pub enum SuggestionOutcome {
    Suggested {
        /// Boxed because a suggestion carries the whole seam payload while the
        /// other states are a few words.
        suggestion: Box<StrategySuggestion>,
        basis: SuggestionBasis,
        claim: EvidenceClaim,
        /// The authorization context the suggestion was produced under, echoed
        /// so a caller can see it was not implied by metadata.
        authority: AuthorityContextState,
    },
    /// The installed catalog cannot run the user's configured option, so
    /// nothing is suggested in its place.
    UserConfigurationNotAvailable { option: AgentModelOptionSeam },
    /// The host offers no options at all. No model is invented.
    NoCandidateCatalog,
    /// Too little history to propose an option, and nothing was adopted for
    /// this scope.
    InsufficientHistory { samples: u64, required: u64 },
    /// An automatic suggestion needs a resolved authorization context; without
    /// one the adapter suggests nothing rather than treating labels as context.
    AuthorityNotResolved { authority: AuthorityContextState },
}

impl SuggestionOutcome {
    pub const fn state(&self) -> FactState {
        match self {
            Self::Suggested { .. } => FactState::Present,
            Self::UserConfigurationNotAvailable { .. } => FactState::Unknown,
            Self::NoCandidateCatalog => FactState::NotConfigured,
            Self::InsufficientHistory { .. } => FactState::NotRecorded,
            Self::AuthorityNotResolved { .. } => FactState::NotConfigured,
        }
    }

    pub const fn claim(&self) -> EvidenceClaim {
        EvidenceClaim::NotMeasured
    }
}

/// Receipt for an adoption.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptionReceipt {
    pub scope: PlanningScopeSeam,
    /// The owner's future-default revision after this adoption.
    pub revision: u64,
    pub source: StrategySourceSeam,
    pub active_default: AgentModelOptionSeam,
    pub applies_to: DefaultApplication,
}

/// Receipt for a revocation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevocationReceipt {
    pub scope: PlanningScopeSeam,
    pub source_id: String,
    /// The default in force afterwards. `None` means the scope is back to the
    /// catalog fallback.
    pub restored: Option<AgentModelOptionSeam>,
    /// The owner's future-default revision in force afterwards. Unchanged when
    /// nothing was revoked.
    pub revision: u64,
    /// False when the source had not been adopted for this scope; revoking an
    /// unknown source is a no-op rather than an error.
    pub changed: bool,
    pub applies_to: DefaultApplication,
}

/// The policy adapter: ranking seam in, owner-held future defaults out.
///
/// It holds no default fact of its own. `defaults` is the real
/// [`DefaultEvolutionStrategyPort`], and `strategy` is the ranking seam — the
/// same owner unless a caller replaces the ranking seam with
/// [`PolicyAdapter::with_strategy_port`].
pub struct PolicyAdapter {
    strategy: Arc<dyn EvolutionStrategyPort>,
    defaults: Arc<DefaultEvolutionStrategyPort>,
}

impl Default for PolicyAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl PolicyAdapter {
    pub fn new() -> Self {
        let owner = Arc::new(DefaultEvolutionStrategyPort::new());
        Self {
            strategy: owner.clone(),
            defaults: owner,
        }
    }

    /// Share one real strategy owner between ranking and the revocable default
    /// facts. Both entries then read and write the same adoption history, so
    /// there is one future-default fact and not one per entry. This is the
    /// production sharing path.
    pub fn with_owner(mut self, owner: Arc<DefaultEvolutionStrategyPort>) -> Self {
        self.strategy = owner.clone();
        self.defaults = owner;
        self
    }

    /// Replace only the ranking seam. The revocable default facts stay with the
    /// adapter's own [`DefaultEvolutionStrategyPort`]; a ranking-only port owns
    /// no defaults, so it cannot become a second default source — but replacing
    /// the ranking seam alone does not share the default authority. Use
    /// [`PolicyAdapter::with_owner`] for that.
    pub fn with_strategy_port(mut self, strategy: Arc<dyn EvolutionStrategyPort>) -> Self {
        self.strategy = strategy;
        self
    }

    /// Suggest an option for future work.
    ///
    /// Order: the user's own configuration, then the default adopted for this
    /// scope, then real outcome counts, then the owner's ranking seam. Every
    /// step only selects from the candidates the host can actually run, and
    /// every returned suggestion keeps `has_execution_permission: false`.
    pub fn suggest(&self, request: &PolicyRequest) -> AdapterResult<SuggestionOutcome> {
        if request.candidates.is_empty() {
            return Ok(SuggestionOutcome::NoCandidateCatalog);
        }
        if let Some(user) = &request.user_configuration {
            if !request.candidates.contains(user) {
                return Ok(SuggestionOutcome::UserConfigurationNotAvailable {
                    option: user.clone(),
                });
            }
            return Ok(SuggestionOutcome::Suggested {
                suggestion: Box::new(StrategySuggestion {
                    candidate: user.clone(),
                    ranking_basis: "user-configured".to_owned(),
                    source: None,
                    scope: Some(request.scope.clone()),
                    is_default: false,
                    has_execution_permission: false,
                    rationale: "Explicit user configuration preserved without an implicit upgrade"
                        .to_owned(),
                }),
                basis: SuggestionBasis::UserConfiguration,
                claim: EvidenceClaim::NotMeasured,
                authority: request.authority.clone(),
            });
        }
        if !request.authority.permits_automatic_suggestion() {
            return Ok(SuggestionOutcome::AuthorityNotResolved {
                authority: request.authority.clone(),
            });
        }
        if let Some(record) = self.defaults.current_default(&request.scope) {
            if request.candidates.contains(&record.selected_option) {
                let revision = self.defaults.revision();
                return Ok(SuggestionOutcome::Suggested {
                    suggestion: Box::new(StrategySuggestion {
                        candidate: record.selected_option.clone(),
                        ranking_basis: "adopted-future-default".to_owned(),
                        source: Some(record.source.clone()),
                        scope: Some(request.scope.clone()),
                        is_default: true,
                        has_execution_permission: false,
                        rationale: format!(
                            "Default adopted for future work at revision {} from source '{}'; no measured improvement is claimed",
                            revision, record.source.source_id
                        ),
                    }),
                    basis: SuggestionBasis::AdoptedFutureDefault {
                        source: record.source.clone(),
                        revision,
                    },
                    claim: EvidenceClaim::NotMeasured,
                    authority: request.authority.clone(),
                });
            }
        }
        if let Some((option, samples)) = history_choice(request) {
            return Ok(SuggestionOutcome::Suggested {
                suggestion: Box::new(StrategySuggestion {
                    candidate: option.clone(),
                    ranking_basis: "history-outcome-counts".to_owned(),
                    source: None,
                    scope: Some(request.scope.clone()),
                    is_default: false,
                    has_execution_permission: false,
                    rationale: format!(
                        "{samples} observed outcomes for this option; counts only, no measured improvement is claimed"
                    ),
                }),
                basis: SuggestionBasis::History {
                    option_samples: samples,
                    required_samples: MINIMUM_HISTORY_SAMPLES,
                },
                claim: EvidenceClaim::NotMeasured,
                authority: request.authority.clone(),
            });
        }
        let best = best_history_samples(request);
        match self.strategy.suggest_strategy(
            &request.scope,
            &request.candidates,
            request.user_configuration.as_ref(),
        ) {
            Some(suggestion) => {
                let ranking_basis = suggestion.ranking_basis.clone();
                Ok(SuggestionOutcome::Suggested {
                    suggestion: Box::new(StrategySuggestion {
                        // The owner's seam stays the ranking authority; the
                        // permission invariant is re-stated here because this
                        // adapter must never hand back a suggestion that claims
                        // one.
                        has_execution_permission: false,
                        ..suggestion
                    }),
                    basis: SuggestionBasis::OwnerRanking { ranking_basis },
                    claim: EvidenceClaim::NotMeasured,
                    authority: request.authority.clone(),
                })
            }
            // The ranking seam offered nothing and no option cleared the
            // history floor. The honest answer is that there is not enough to
            // propose from, not a model picked to fill the gap.
            None => Ok(SuggestionOutcome::InsufficientHistory {
                samples: best,
                required: MINIMUM_HISTORY_SAMPLES,
            }),
        }
    }

    /// Adopt a revocable default for future work.
    ///
    /// The option must be one the host can actually run and the adoption must be
    /// revocable. The adoption is written through the shared
    /// [`DefaultEvolutionStrategyPort`], so the owner's own `suggest_strategy`
    /// observes it immediately; this adapter keeps no copy.
    pub fn adopt(
        &self,
        adoption: &AdoptedPlanningDefaultSeam,
        candidates: &[AgentModelOptionSeam],
    ) -> AdapterResult<AdoptionReceipt> {
        if !adoption.revocable {
            return Err(AdapterError::refused(
                ADAPTER,
                "policy_adoption_not_revocable",
                "adopt_a_revocable_default",
            ));
        }
        if !candidates.contains(&adoption.selected_option) {
            return Err(AdapterError::refused(
                ADAPTER,
                "policy_adoption_outside_candidate_catalog",
                "choose_an_option_the_host_can_run",
            ));
        }
        self.defaults.adopt_default(adoption.clone());
        Ok(AdoptionReceipt {
            scope: adoption.scope.clone(),
            revision: self.defaults.revision(),
            source: adoption.source.clone(),
            active_default: adoption.selected_option.clone(),
            applies_to: DefaultApplication::FutureWork,
        })
    }

    /// Revoke an adopted source, restoring the default that preceded it.
    ///
    /// Revoking twice, or revoking a source that was never adopted, changes
    /// nothing and reports `changed: false`. The change happens in the shared
    /// owner, so the owner's own `suggest_strategy` sees the restored default.
    pub fn revoke(
        &self,
        scope: &PlanningScopeSeam,
        source_id: &str,
    ) -> AdapterResult<RevocationReceipt> {
        let outcome = self.defaults.revoke_default(scope, source_id);
        Ok(RevocationReceipt {
            scope: scope.clone(),
            source_id: source_id.to_owned(),
            restored: outcome.restored.map(|record| record.selected_option),
            revision: self.defaults.revision(),
            changed: outcome.changed,
            applies_to: DefaultApplication::FutureWork,
        })
    }

    /// The default in force for future work in this scope, read from the owner.
    pub fn future_default(&self, scope: &PlanningScopeSeam) -> Option<AgentModelOptionSeam> {
        self.defaults
            .current_default(scope)
            .map(|record| record.selected_option)
    }

    /// The owner's future-default revision in force.
    pub fn revision(&self) -> u64 {
        self.defaults.revision()
    }
}

/// The option with the most observed outcomes that clears the sample floor.
///
/// Counts only: the adapter has no controlled comparison, so this is not a
/// statistical claim about the options.
fn history_choice(request: &PolicyRequest) -> Option<(AgentModelOptionSeam, u64)> {
    let mut best: Option<(AgentModelOptionSeam, u64)> = None;
    for candidate in &request.candidates {
        let Some(option) = request.history.option(&OptionKey::of(candidate)) else {
            continue;
        };
        let samples = option.samples();
        if samples < MINIMUM_HISTORY_SAMPLES {
            continue;
        }
        if best
            .as_ref()
            .is_none_or(|(_, best_samples)| samples > *best_samples)
        {
            best = Some((candidate.clone(), samples));
        }
    }
    best
}

fn best_history_samples(request: &PolicyRequest) -> u64 {
    request
        .history
        .options
        .values()
        .map(OptionHistory::samples)
        .max()
        .unwrap_or_default()
}
