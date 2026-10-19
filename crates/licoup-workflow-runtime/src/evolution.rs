//! Evolution wiring connecting workflow runtime to observation, cost, context,
//! and strategy ports.
//!
//! # Architecture & Governance
//!
//! In accordance with `docs/plans/Plan.md` T07.2, D09, and D21:
//! - Connects observation, cost, context, and strategy into existing workflow callbacks.
//! - The Assistant receives usable facts and actionable suggestions via callbacks.
//! - Strategy versions (from T06.1) feed selection without granting execution permission.
//!   Every [`StrategySuggestion`] explicitly marks `has_execution_permission: false`.
//!   The configured choice and the adopted default decide first; with neither, the
//!   selection is routed by the candidate policy
//!   ([`licoup_model_catalog::candidate_routing`]) over the same candidate catalog and the
//!   effective scope admission, so an alternative the policy does not allow is never
//!   suggested — including the first entry of the list, which is not a decision.
//! - Permission, version, and resource recheck is strictly evaluated before any effect execution
//!   through [`EffectRecheckContext::for_run_command`], the single construction path used by
//!   the executor.
//! - Group B (economics loop, T03–T06) is a separate Draft: integration seams are
//!   strictly typed and wired against existing host-side seams, with gaps explicitly recorded
//!   in [`GroupBIntegrationGaps`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use licoup_workflow::{PendingCallback, RunCommand, RunSnapshot, StrategyRunStatus};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::adapter::SingleWriterSessionRegistry;
use crate::ports::host_ports;
use licoup_model_catalog::candidate_routing::{
    CandidateId, CandidateOffer, CandidateRoutingPort, CandidateRoutingRequest,
    PolicyCandidateRouting, SelectionScope, routing_rationale,
};
use licoup_workflow_store::{StrategyAuthorization, StrategyDefinition};

// The vocabulary that carries no host naming is the engine's, re-exported
// under this module so the established `evolution::*` surface keeps
// resolving. What stays declared below is what the LicoUp composition owns:
// the callback facts that name conversations and memberships, the context
// and strategy ports that read them, the ledger cost port bound to this
// crate's own host-ports registry, the recheck context typed on this
// crate's snapshot and command, and the Group B governance record.
pub use acativa::runtime::evolution::{
    AdoptedPlanningDefaultSeam, AgentModelOptionSeam, BudgetPoolReservationSeam,
    ContextManifestSummarySeam, DefaultEvolutionObservationPort, EffectRecheckFailure,
    EffectRecheckReceipt, EvolutionCostPort, EvolutionObservationPort, EvolutionStrategyPort,
    NodeObservationSummary, ObservationFactKindSeam, ObservationFactRecordSeam,
    ObservationScopeSeam, PlanningScopeSeam, SourceRefSeam, StrategySourceSeam, StrategySuggestion,
    StrategyVersionSeam,
};

/// The callback's cost reading. The fields are the engine's; the name this
/// host's callback envelope knows them by is its own.
pub type CallbackCostFacts = acativa::runtime::evolution::EvolutionCostFacts;

// ============================================================================
// Group B Typed Integration Seams & Gap Tracking
// ============================================================================

/// Recorded status and gaps for the separate Group B (economics loop, T03–T06) Draft.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupBGapReport {
    pub t03_observation_gap: &'static str,
    pub t04_cost_budget_pool_gap: &'static str,
    pub t05_context_composition_gap: &'static str,
    pub t06_replaceable_strategy_gap: &'static str,
}

pub struct GroupBIntegrationGaps;

impl GroupBIntegrationGaps {
    pub fn report() -> GroupBGapReport {
        GroupBGapReport {
            t03_observation_gap: "assistant_continuity::observation with continuity_source_cursors is in group B branch (fix/t03-1-observation); wired here against run snapshots and NodeObservationSummary, whose producer is the T07.4 Node Facade observe() seam rather than native NodeObservation",
            t04_cost_budget_pool_gap: "workflow_ledger graph_usage_budget_pools and graph_usage_reservations are in group B branch (fix/t04-1-usage-admission); wired here against workflow_ledger v2 numeric token accounting, so the configured budget pool and its reservations stay unavailable and the pre-effect budget recheck has no remaining-token fact to compare",
            t05_context_composition_gap: "assistant_continuity::context multi-source refinement with parent grants is in group B branch (fix/t05-1-context); wired here against conversation run/causation metadata",
            t06_replaceable_strategy_gap: "the replaceable strategy seam is wired to the non-learning candidate policy: an explicit user choice and an adopted default decide first, and every other selection is routed by domain::candidate_routing over the configured candidate catalog and the effective scope admission. Durable learned defaults and the qualification policy of domain::model_planning are not composed into this seam yet, so no learned default reaches a callback; the catalogue's ranking is not used to choose a model either, because the seam carries no price or intelligence facts",
        }
    }
}

#[derive(Default)]
pub struct LedgerEvolutionCostPort;

impl LedgerEvolutionCostPort {
    pub fn new() -> Self {
        Self
    }
}

impl EvolutionCostPort for LedgerEvolutionCostPort {
    fn query_cost_facts(&self, portable_root: &Path, run_id: &str) -> Option<CallbackCostFacts> {
        let report_result = host_ports().usage.workflow_report(&json!({
            "stateRoot": portable_root,
            "runId": run_id,
        }));
        match report_result {
            Ok(report) => {
                let summary = report.get("summary");
                Some(CallbackCostFacts {
                    prompt_tokens: summary
                        .and_then(|s| s.get("promptTokens"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    cached_input_tokens: summary
                        .and_then(|s| s.get("cachedInputTokens"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    completion_tokens: summary
                        .and_then(|s| s.get("completionTokens"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    total_tokens: summary
                        .and_then(|s| s.get("totalTokens"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    exact_count: summary
                        .and_then(|s| s.get("exactCount"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    estimated_count: summary
                        .and_then(|s| s.get("estimatedCount"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    budget_pool_seam_active: false,
                })
            }
            Err(_) => None,
        }
    }
}

pub trait EvolutionContextPort: Send + Sync {
    fn assemble_context_facts(
        &self,
        snapshot: &RunSnapshot,
        state_id: &str,
        state_visit: u64,
        transition_id: &str,
        answer_channel: &str,
    ) -> CallbackContextFacts;
}

#[derive(Default)]
pub struct DefaultEvolutionContextPort;

impl DefaultEvolutionContextPort {
    pub fn new() -> Self {
        Self
    }
}

impl EvolutionContextPort for DefaultEvolutionContextPort {
    fn assemble_context_facts(
        &self,
        snapshot: &RunSnapshot,
        state_id: &str,
        state_visit: u64,
        transition_id: &str,
        answer_channel: &str,
    ) -> CallbackContextFacts {
        CallbackContextFacts {
            conversation_id: snapshot.conversation_id.clone(),
            assistant_membership_id: snapshot.assistant_membership_id.clone(),
            state_id: state_id.to_owned(),
            state_visit,
            transition_id: transition_id.to_owned(),
            causation_id: Some(snapshot.run_id.clone()),
            answer_channel: answer_channel.to_owned(),
            context_composition_seam_active: true,
        }
    }
}

pub struct DefaultEvolutionStrategyPort {
    defaults: RwLock<BTreeMap<PlanningScopeSeam, AdoptedPlanningDefaultSeam>>,
    revocations: RwLock<BTreeSet<String>>,
    routing: Arc<dyn CandidateRoutingPort>,
}

impl DefaultEvolutionStrategyPort {
    pub fn new() -> Self {
        Self {
            defaults: RwLock::new(BTreeMap::new()),
            revocations: RwLock::new(BTreeSet::new()),
            routing: Arc::new(PolicyCandidateRouting),
        }
    }

    /// Replace the routing owner this port asks for a non-default selection.
    ///
    /// A caller that composes its own policy facts supplies its owner here; a
    /// caller that supplies none keeps [`PolicyCandidateRouting`], which reads
    /// the facts the host installed and selects nothing when none were.
    pub fn with_routing(mut self, routing: Arc<dyn CandidateRoutingPort>) -> Self {
        self.routing = routing;
        self
    }

    /// Adopt a revocable default strategy per D21.
    pub fn adopt_default(&self, default: AdoptedPlanningDefaultSeam) {
        let mut guard = self.defaults.write().unwrap_or_else(|p| p.into_inner());
        guard.insert(default.scope.clone(), default);
    }

    /// Revoke an adopted strategy default per D21.
    pub fn revoke_strategy(&self, source_id: &str) {
        let mut guard = self.revocations.write().unwrap_or_else(|p| p.into_inner());
        guard.insert(source_id.to_owned());
    }
}

impl Default for DefaultEvolutionStrategyPort {
    fn default() -> Self {
        Self::new()
    }
}

impl EvolutionStrategyPort for DefaultEvolutionStrategyPort {
    fn suggest_strategy(
        &self,
        scope: &PlanningScopeSeam,
        candidates: &[AgentModelOptionSeam],
        explicit_user_choice: Option<&AgentModelOptionSeam>,
    ) -> Option<StrategySuggestion> {
        // D09 / D21: Explicit user configuration strictly takes precedence.
        if let Some(user_choice) = explicit_user_choice {
            if candidates.contains(user_choice) {
                return Some(StrategySuggestion {
                    candidate: user_choice.clone(),
                    ranking_basis: "user-configured".to_owned(),
                    source: None,
                    scope: Some(scope.clone()),
                    is_default: false,
                    has_execution_permission: false, // Invariant: no execution permission granted
                    rationale: "Explicit user selection preserved without implicit upgrade"
                        .to_owned(),
                });
            }
        }

        let defaults_guard = self.defaults.read().unwrap_or_else(|p| p.into_inner());
        let revocations_guard = self.revocations.read().unwrap_or_else(|p| p.into_inner());

        if let Some(adopted) = defaults_guard.get(scope) {
            // Check revocation
            if !revocations_guard.contains(&adopted.source.source_id) {
                // Must select only from caller's candidate set
                if candidates.contains(&adopted.selected_option) {
                    return Some(StrategySuggestion {
                        candidate: adopted.selected_option.clone(),
                        ranking_basis: "adopted-default-comparable-outcome".to_owned(),
                        source: Some(adopted.source.clone()),
                        scope: Some(scope.clone()),
                        is_default: true,
                        has_execution_permission: false, // Invariant: no execution permission granted
                        rationale: format!(
                            "Adopted default from source '{}' rev '{}'",
                            adopted.source.source_id, adopted.source.revision
                        ),
                    });
                }
            }
        }

        // Fallback: ask the routing policy which of the caller's alternatives
        // may run. The configured order is the preference, not the answer, and
        // an alternative the policy does not allow is never suggested — a
        // process that composes no admission owner therefore gets no
        // suggestion at all instead of the first entry of the list.
        let outcome = self.routing.route(&candidate_routing_request(candidates));
        let suggestion = outcome
            .selected()
            .and_then(|selected| configured_option(candidates, selected))?;
        Some(StrategySuggestion {
            candidate: suggestion.clone(),
            ranking_basis: "candidate-policy".to_owned(),
            source: None,
            scope: Some(scope.clone()),
            is_default: false,
            has_execution_permission: false, // Invariant: no execution permission granted
            rationale: routing_rationale(&outcome),
        })
    }
}

// ============================================================================
// Usable Callback Facts & Suggestions
// ============================================================================

/// The policy question one strategy suggestion is answered by.
///
/// It is a workflow turn: the alternatives are the configured candidate list
/// the enricher was given, in its configured order, and the scope is the
/// catalogue's own [`SelectionScope::Workflow`]. A direct request asks the same
/// question under `SelectionScope::Direct`; the policy answers each scope on its
/// own, so one Agent can be suggested for one and not the other.
fn candidate_routing_request(candidates: &[AgentModelOptionSeam]) -> CandidateRoutingRequest {
    CandidateRoutingRequest::new(
        SelectionScope::Workflow,
        candidates
            .iter()
            .enumerate()
            .map(|(index, candidate)| CandidateOffer {
                candidate: CandidateId::new(
                    candidate.agent_id.clone(),
                    candidate.model_id.clone(),
                    None,
                ),
                configured_position: index + 1,
            })
            .collect(),
    )
}

/// The configured option one routed candidate names.
///
/// The policy ranks Agent/provider/model identities; the suggestion carries the
/// configured option with its thinking setting. A recommended identity the
/// configured list does not carry is reported as no suggestion rather than as an
/// invented option, so the suggestion can never leave the caller's candidate set.
fn configured_option<'a>(
    candidates: &'a [AgentModelOptionSeam],
    candidate: &CandidateId,
) -> Option<&'a AgentModelOptionSeam> {
    candidates.iter().find(|option| {
        option.agent_id == candidate.agent_id && option.model_id == candidate.model_id
    })
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallbackObservationFacts {
    pub completed_states: Vec<String>,
    pub state_visits: BTreeMap<String, u64>,
    pub node_observations: Vec<NodeObservationSummary>,
    pub failure_diagnostic: Option<String>,
    pub observation_seam_active: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallbackContextFacts {
    pub conversation_id: Option<String>,
    pub assistant_membership_id: Option<String>,
    pub state_id: String,
    pub state_visit: u64,
    pub transition_id: String,
    pub causation_id: Option<String>,
    pub answer_channel: String,
    pub context_composition_seam_active: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallbackFacts {
    pub observation: CallbackObservationFacts,
    /// `None` when the run's spend could not be read: unknown cost is reported
    /// as unknown, never as zero.
    pub cost: Option<CallbackCostFacts>,
    pub context: CallbackContextFacts,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallbackSuggestions {
    pub recommended_decision: String,
    pub decision_reasons: Vec<String>,
    pub strategy_suggestion: Option<StrategySuggestion>,
    pub alternative_decisions: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallbackEvolutionPayload {
    pub facts: CallbackFacts,
    pub suggestions: CallbackSuggestions,
}

pub struct CallbackEvolutionEnricher {
    portable_root: PathBuf,
    observation_port: Arc<dyn EvolutionObservationPort>,
    cost_port: Arc<dyn EvolutionCostPort>,
    context_port: Arc<dyn EvolutionContextPort>,
    strategy_port: Arc<dyn EvolutionStrategyPort>,
    strategy_candidates: Vec<AgentModelOptionSeam>,
}

impl CallbackEvolutionEnricher {
    pub fn new(portable_root: &Path) -> Self {
        Self {
            portable_root: portable_root.to_path_buf(),
            observation_port: Arc::new(DefaultEvolutionObservationPort::new()),
            cost_port: Arc::new(LedgerEvolutionCostPort::new()),
            context_port: Arc::new(DefaultEvolutionContextPort::new()),
            strategy_port: Arc::new(DefaultEvolutionStrategyPort::new()),
            strategy_candidates: Vec::new(),
        }
    }

    /// Supply the model options this host can actually run for a scope.
    ///
    /// Only options present here can be suggested; an empty catalog (the state
    /// until the T06 durable-defaults seam is wired) yields no strategy
    /// suggestion at all, never a model the host has not been told about.
    pub fn with_strategy_candidates(mut self, candidates: Vec<AgentModelOptionSeam>) -> Self {
        self.strategy_candidates = candidates;
        self
    }

    pub fn with_observation_port(mut self, port: Arc<dyn EvolutionObservationPort>) -> Self {
        self.observation_port = port;
        self
    }

    pub fn with_cost_port(mut self, port: Arc<dyn EvolutionCostPort>) -> Self {
        self.cost_port = port;
        self
    }

    pub fn with_context_port(mut self, port: Arc<dyn EvolutionContextPort>) -> Self {
        self.context_port = port;
        self
    }

    pub fn with_strategy_port(mut self, port: Arc<dyn EvolutionStrategyPort>) -> Self {
        self.strategy_port = port;
        self
    }

    pub fn enrich_callback_request(
        &self,
        snapshot: &RunSnapshot,
        pending: &PendingCallback,
        answer_channel: &str,
    ) -> CallbackEvolutionPayload {
        let node_obs = self
            .observation_port
            .collect_node_observations(&snapshot.run_id);
        let obs_facts = CallbackObservationFacts {
            completed_states: snapshot.completed_states.iter().cloned().collect(),
            state_visits: snapshot.state_visits.clone(),
            node_observations: node_obs,
            failure_diagnostic: snapshot.diagnostic_code.clone(),
            observation_seam_active: true,
        };

        let cost_facts = self
            .cost_port
            .query_cost_facts(&self.portable_root, &snapshot.run_id);
        let ctx_facts = self.context_port.assemble_context_facts(
            snapshot,
            &pending.state_id,
            pending.state_visit,
            &pending.transition_id,
            answer_channel,
        );

        let mut decision_reasons = Vec::new();
        let recommended_decision = if snapshot.status == StrategyRunStatus::Failed {
            decision_reasons
                .push("Run has encountered a terminal failure; termination recommended".into());
            "terminate".to_owned()
        } else {
            decision_reasons.push(format!(
                "Callback parked at state '{}' (visit {}); previous states completed successfully",
                pending.state_id, pending.state_visit
            ));
            match &cost_facts {
                Some(cost) if cost.total_tokens > 0 => decision_reasons.push(format!(
                    "Current run usage is {} tokens (exact: {}, estimated: {})",
                    cost.total_tokens, cost.exact_count, cost.estimated_count
                )),
                Some(_) => {}
                None => decision_reasons
                    .push("Run usage could not be read from the ledger; spend is unknown".into()),
            }
            "advance".to_owned()
        };

        // Ask the strategy port to rank the options this host actually offers.
        // The port only advises: the suggestion it returns never carries
        // execution permission and never leaves the candidate catalog.
        let scope = PlanningScopeSeam {
            task_kind: "workflow-turn".to_owned(),
            configuration: pending.state_id.clone(),
        };
        let strategy_suggestion =
            self.strategy_port
                .suggest_strategy(&scope, &self.strategy_candidates, None);

        let alternative_decisions = match recommended_decision.as_str() {
            "advance" => vec!["return".to_owned(), "terminate".to_owned()],
            "return" => vec!["advance".to_owned(), "terminate".to_owned()],
            _ => vec!["advance".to_owned(), "return".to_owned()],
        };

        CallbackEvolutionPayload {
            facts: CallbackFacts {
                observation: obs_facts,
                cost: cost_facts,
                context: ctx_facts,
            },
            suggestions: CallbackSuggestions {
                recommended_decision,
                decision_reasons,
                strategy_suggestion,
                alternative_decisions,
            },
        }
    }

    pub fn enrich_terminal_outcome(&self, snapshot: &RunSnapshot) -> CallbackEvolutionPayload {
        let node_obs = self
            .observation_port
            .collect_node_observations(&snapshot.run_id);
        let obs_facts = CallbackObservationFacts {
            completed_states: snapshot.completed_states.iter().cloned().collect(),
            state_visits: snapshot.state_visits.clone(),
            node_observations: node_obs,
            failure_diagnostic: snapshot.diagnostic_code.clone(),
            observation_seam_active: true,
        };

        let cost_facts = self
            .cost_port
            .query_cost_facts(&self.portable_root, &snapshot.run_id);
        let ctx_facts = self
            .context_port
            .assemble_context_facts(snapshot, "", 0, "", "terminal");

        CallbackEvolutionPayload {
            facts: CallbackFacts {
                observation: obs_facts,
                cost: cost_facts,
                context: ctx_facts,
            },
            suggestions: CallbackSuggestions {
                recommended_decision: "terminate".to_owned(),
                decision_reasons: vec![format!(
                    "Terminal run failure with diagnostic: {}",
                    snapshot.diagnostic_code.as_deref().unwrap_or("unknown")
                )],
                strategy_suggestion: None,
                alternative_decisions: Vec::new(),
            },
        }
    }

    pub fn enrich_flow_settled(
        &self,
        snapshot: &RunSnapshot,
        state_id: &str,
        state_visit: u64,
    ) -> CallbackEvolutionPayload {
        let node_obs = self
            .observation_port
            .collect_node_observations(&snapshot.run_id);
        let obs_facts = CallbackObservationFacts {
            completed_states: snapshot.completed_states.iter().cloned().collect(),
            state_visits: snapshot.state_visits.clone(),
            node_observations: node_obs,
            failure_diagnostic: None,
            observation_seam_active: true,
        };

        let cost_facts = self
            .cost_port
            .query_cost_facts(&self.portable_root, &snapshot.run_id);
        let ctx_facts = self.context_port.assemble_context_facts(
            snapshot,
            state_id,
            state_visit,
            "flow",
            "flow",
        );

        CallbackEvolutionPayload {
            facts: CallbackFacts {
                observation: obs_facts,
                cost: cost_facts,
                context: ctx_facts,
            },
            suggestions: CallbackSuggestions {
                recommended_decision: "advance".to_owned(),
                decision_reasons: vec![format!(
                    "State '{}' (visit {}) completed successfully in flow mode",
                    state_id, state_visit
                )],
                strategy_suggestion: None,
                alternative_decisions: Vec::new(),
            },
        }
    }
}

// ============================================================================
// Pre-Effect Recheck: Permission, Version, Resource
// ============================================================================

/// Context evaluated immediately prior to executing any effect.
pub struct EffectRecheckContext<'a> {
    pub run_id: &'a str,
    pub command_id: &'a str,
    pub definition_revision: &'a str,
    pub expected_revision: &'a str,
    pub expected_generation: u64,
    pub current_generation: u64,
    pub authorization: Option<&'a StrategyAuthorization>,
    pub claimant: &'a str,
    /// Native session this effect will write through, when the executor holds a
    /// single-writer registration. `None` means the executor has no session lock
    /// to recheck, not that the lock check passed.
    pub session_id: Option<&'a str>,
    pub single_writer_registry: Option<&'a SingleWriterSessionRegistry>,
    /// Tokens this effect is about to spend against a configured pool, and what
    /// is left of it. `None` means no pool fact is available to compare.
    pub required_tokens: Option<u64>,
    pub token_budget_remaining: Option<u64>,
}

impl<'a> EffectRecheckContext<'a> {
    /// Build the pre-effect recheck context for one command the executor claimed.
    ///
    /// The generation check compares two independently sourced facts: the visit
    /// the command was minted for (`RunCommand::state_visit`) against the visit
    /// the run is at now (`RunSnapshot::state_visits`). A command whose state was
    /// re-entered, or that was claimed for a state the run has since advanced
    /// past, must not reach the executor with stale semantics.
    ///
    /// The revision check asserts the run is still bound to the definition it
    /// was loaded from. That binding is structural here (the definition is read
    /// by `snapshot.definition_digest`), so the check is an invariant assertion
    /// rather than a second read of mutable state.
    ///
    /// Session-lock and token-budget facts stay `None` until the T07.4 node
    /// facade and the T04 pool ports are reachable from the executor; see
    /// [`GroupBIntegrationGaps`]. Callers holding those facts set the fields
    /// directly.
    pub fn for_run_command(
        run_id: &'a str,
        snapshot: &'a RunSnapshot,
        definition: &'a StrategyDefinition,
        command: &'a RunCommand,
        claimant: &'a str,
    ) -> Self {
        Self {
            run_id,
            command_id: &command.id,
            definition_revision: &snapshot.definition_digest,
            expected_revision: &definition.summary.revision_digest,
            expected_generation: command.state_visit,
            current_generation: snapshot
                .state_visits
                .get(&command.state_id)
                .copied()
                .unwrap_or(0),
            authorization: definition.authorization.as_ref(),
            claimant,
            session_id: None,
            single_writer_registry: None,
            required_tokens: None,
            token_budget_remaining: None,
        }
    }
}

/// Recheck permission, version, and resources before executing any effect.
///
/// Returns [`Ok(EffectRecheckReceipt)`] if all checks pass, or [`Err(EffectRecheckFailure)`]
/// stopping execution before any real effect occurs.
pub fn recheck_before_effect(
    ctx: &EffectRecheckContext<'_>,
) -> std::result::Result<EffectRecheckReceipt, EffectRecheckFailure> {
    // 1. Permission Recheck: authorization must be present and active
    let Some(auth) = ctx.authorization else {
        return Err(EffectRecheckFailure::PermissionDenied {
            code: "authorization_missing".into(),
            reason: "No authorization configured on definition".into(),
        });
    };
    if !auth.active {
        return Err(EffectRecheckFailure::PermissionDenied {
            code: "authorization_inactive".into(),
            reason: "Strategy authorization is marked inactive or has been revoked".into(),
        });
    }

    // 2. Version Recheck: expected definition revision and node generation must match
    if ctx.definition_revision != ctx.expected_revision {
        return Err(EffectRecheckFailure::VersionMismatch {
            expected: ctx.expected_revision.to_owned(),
            actual: ctx.definition_revision.to_owned(),
        });
    }
    if ctx.expected_generation != ctx.current_generation {
        return Err(EffectRecheckFailure::StaleGeneration {
            expected: ctx.expected_generation,
            current: ctx.current_generation,
        });
    }

    // 3. Resource Recheck: check single-writer lock and token budget limits
    if let (Some(registry), Some(session)) = (ctx.single_writer_registry, ctx.session_id) {
        if let Some(active_writer) = registry.current_writer(session) {
            if active_writer != ctx.claimant {
                return Err(EffectRecheckFailure::ResourceUnavailable {
                    code: "session_writer_conflict".into(),
                    reason: format!(
                        "Native session '{}' is currently locked by claimant '{}'",
                        session, active_writer
                    ),
                });
            }
        }
    }

    if let (Some(req), Some(remaining)) = (ctx.required_tokens, ctx.token_budget_remaining) {
        if req > remaining {
            return Err(EffectRecheckFailure::ResourceUnavailable {
                code: "token_budget_exhausted".into(),
                reason: format!("Required tokens ({req}) exceed remaining budget ({remaining})"),
            });
        }
    }

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    Ok(EffectRecheckReceipt {
        run_id: ctx.run_id.to_owned(),
        command_id: ctx.command_id.to_owned(),
        rechecked_at_unix_ms: now_ms,
        revision_digest: ctx.definition_revision.to_owned(),
        generation: ctx.current_generation,
        permission_verified: true,
        version_verified: true,
        resource_verified: true,
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use licoup_model_catalog::candidate_routing::{
        CandidateFactTable, CandidateFacts, CandidateRoutingOutcome, CandidateRoutingRequest,
        RoutingWithFacts,
    };
    use licoup_model_catalog::port::CredentialState;
    use licoup_model_catalog::selection_matrix::{
        ScopeAdmissionFacts, ScopeOutcomeState, SelectionMatrixPort, SelectionScope,
    };

    /// A routing owner that admits exactly the alternatives the test states
    /// facts for, under the workflow scope.
    fn routing_owner(rows: &[&AgentModelOptionSeam]) -> Arc<dyn CandidateRoutingPort> {
        fn workflow_only(
            _agent: &str,
            scope: SelectionScope,
            _params: &Value,
        ) -> ScopeAdmissionFacts {
            match scope {
                SelectionScope::Workflow => {
                    ScopeAdmissionFacts::new(ScopeOutcomeState::Allowed, "workflow_scope_admitted")
                }
                SelectionScope::Direct => {
                    ScopeAdmissionFacts::new(ScopeOutcomeState::Blocked, "direct_scope_blocked")
                }
            }
        }
        let mut facts = CandidateFactTable::new();
        for row in rows {
            facts.record(
                CandidateId::new(row.agent_id.clone(), row.model_id.clone(), None),
                CandidateFacts {
                    observed_at_unix_ms: Some(1_726_000_000_000),
                    credential: CredentialState::Present,
                    quota: licoup_model_catalog::candidate_policy::QuotaState::Available {
                        window: "primary".to_owned(),
                    },
                    requirements: std::collections::BTreeMap::new(),
                },
            );
        }
        Arc::new(RoutingWithFacts::new(
            Arc::new(facts),
            SelectionMatrixPort {
                agent_scope_admission: workflow_only,
            },
        ))
    }

    #[test]
    fn test_group_b_gap_report_identifies_all_four_subsystems() {
        let report = GroupBIntegrationGaps::report();
        assert!(report.t03_observation_gap.contains("fix/t03-1-observation"));
        assert!(
            report
                .t04_cost_budget_pool_gap
                .contains("fix/t04-1-usage-admission")
        );
        assert!(
            report
                .t05_context_composition_gap
                .contains("fix/t05-1-context")
        );
        assert!(
            report
                .t06_replaceable_strategy_gap
                .contains("domain::candidate_routing")
        );
    }

    #[test]
    fn test_strategy_suggestion_has_no_execution_permission() {
        let scope = PlanningScopeSeam {
            task_kind: "coding".into(),
            configuration: "default".into(),
        };
        let candidates = vec![
            AgentModelOptionSeam {
                agent_id: "agent-1".into(),
                model_id: "model-a".into(),
                thinking: "low".into(),
            },
            AgentModelOptionSeam {
                agent_id: "agent-2".into(),
                model_id: "model-b".into(),
                thinking: "high".into(),
            },
        ];
        // The fallback is the routing policy's answer, so this port is given
        // the owner the composed host installs instead of one with no facts.
        let port = DefaultEvolutionStrategyPort::new()
            .with_routing(routing_owner(&candidates.iter().collect::<Vec<_>>()));

        let suggestion = port.suggest_strategy(&scope, &candidates, None).unwrap();
        assert_eq!(suggestion.candidate.agent_id, "agent-1");
        // CRITICAL INVARIANT: Strategy selection NEVER grants execution permission!
        assert_eq!(suggestion.has_execution_permission, false);
    }

    #[test]
    fn test_strategy_user_explicit_choice_precedence_d09() {
        let port = DefaultEvolutionStrategyPort::new();
        let scope = PlanningScopeSeam {
            task_kind: "coding".into(),
            configuration: "default".into(),
        };
        let candidates = vec![
            AgentModelOptionSeam {
                agent_id: "agent-1".into(),
                model_id: "model-a".into(),
                thinking: "low".into(),
            },
            AgentModelOptionSeam {
                agent_id: "agent-2".into(),
                model_id: "model-b".into(),
                thinking: "high".into(),
            },
        ];

        // Adopt a default recommending agent-1
        port.adopt_default(AdoptedPlanningDefaultSeam {
            scope: scope.clone(),
            source: StrategySourceSeam {
                source_id: "test-src".into(),
                revision: "r1".into(),
            },
            selected_option: candidates[0].clone(),
            revocable: true,
            supersedes_revision: None,
        });

        // User explicitly requests agent-2
        let user_choice = candidates[1].clone();
        let suggestion = port
            .suggest_strategy(&scope, &candidates, Some(&user_choice))
            .unwrap();
        assert_eq!(suggestion.candidate.agent_id, "agent-2");
        assert_eq!(suggestion.ranking_basis, "user-configured");
        assert_eq!(suggestion.is_default, false);
        assert_eq!(suggestion.has_execution_permission, false);
    }

    #[test]
    fn test_strategy_adopted_default_selection_and_revocation_d21() {
        let scope = PlanningScopeSeam {
            task_kind: "review".into(),
            configuration: "default".into(),
        };
        let candidates = vec![
            AgentModelOptionSeam {
                agent_id: "reviewer-1".into(),
                model_id: "fast".into(),
                thinking: "low".into(),
            },
            AgentModelOptionSeam {
                agent_id: "reviewer-2".into(),
                model_id: "deep".into(),
                thinking: "high".into(),
            },
        ];
        // After revocation the routing policy decides, so this port is given
        // the owner the composed host installs instead of one with no facts.
        let port = DefaultEvolutionStrategyPort::new()
            .with_routing(routing_owner(&candidates.iter().collect::<Vec<_>>()));

        // Adopt reviewer-2
        port.adopt_default(AdoptedPlanningDefaultSeam {
            scope: scope.clone(),
            source: StrategySourceSeam {
                source_id: "eval-42".into(),
                revision: "rev-2".into(),
            },
            selected_option: candidates[1].clone(),
            revocable: true,
            supersedes_revision: None,
        });

        // Default takes effect when no explicit user choice
        let suggestion = port.suggest_strategy(&scope, &candidates, None).unwrap();
        assert_eq!(suggestion.candidate.agent_id, "reviewer-2");
        assert_eq!(suggestion.is_default, true);
        assert_eq!(
            suggestion.ranking_basis,
            "adopted-default-comparable-outcome"
        );
        assert_eq!(suggestion.has_execution_permission, false);

        // Revoking the strategy drops back to the routing policy, which selects
        // the configured alternative it admits.
        port.revoke_strategy("eval-42");
        let after_revoke = port.suggest_strategy(&scope, &candidates, None).unwrap();
        assert_eq!(after_revoke.candidate.agent_id, "reviewer-1");
        assert_eq!(after_revoke.is_default, false);
        assert_eq!(
            after_revoke.ranking_basis, "candidate-policy",
            "the basis the routing port reports for a policy-selected alternative"
        );
    }

    #[test]
    fn test_pre_effect_recheck_success() {
        let auth = StrategyAuthorization {
            definition_digest: "def-1".into(),
            semantics_digest: "sem-1".into(),
            binding_digest: "bind-1".into(),
            authorization_digest: "auth-1".into(),
            revision: 1,
            active: true,
        };
        let ctx = EffectRecheckContext {
            run_id: "run-1",
            command_id: "cmd-1",
            definition_revision: "rev-1",
            expected_revision: "rev-1",
            expected_generation: 2,
            current_generation: 2,
            authorization: Some(&auth),
            claimant: "worker-1",
            session_id: None,
            single_writer_registry: None,
            required_tokens: Some(500),
            token_budget_remaining: Some(1000),
        };

        let receipt = recheck_before_effect(&ctx).unwrap();
        assert_eq!(receipt.run_id, "run-1");
        assert_eq!(receipt.command_id, "cmd-1");
        assert_eq!(receipt.permission_verified, true);
        assert_eq!(receipt.version_verified, true);
        assert_eq!(receipt.resource_verified, true);
    }

    #[test]
    fn test_pre_effect_recheck_fails_on_inactive_authorization() {
        let auth = StrategyAuthorization {
            definition_digest: "def-1".into(),
            semantics_digest: "sem-1".into(),
            binding_digest: "bind-1".into(),
            authorization_digest: "auth-1".into(),
            revision: 1,
            active: false, // Inactive / revoked!
        };
        let ctx = EffectRecheckContext {
            run_id: "run-1",
            command_id: "cmd-1",
            definition_revision: "rev-1",
            expected_revision: "rev-1",
            expected_generation: 1,
            current_generation: 1,
            authorization: Some(&auth),
            claimant: "worker-1",
            session_id: None,
            single_writer_registry: None,
            required_tokens: None,
            token_budget_remaining: None,
        };

        let err = recheck_before_effect(&ctx).unwrap_err();
        assert!(matches!(err, EffectRecheckFailure::PermissionDenied { .. }));
        assert_eq!(err.code(), "authorization_required");
    }

    #[test]
    fn test_pre_effect_recheck_fails_on_version_mismatch() {
        let auth = StrategyAuthorization {
            definition_digest: "def-1".into(),
            semantics_digest: "sem-1".into(),
            binding_digest: "bind-1".into(),
            authorization_digest: "auth-1".into(),
            revision: 1,
            active: true,
        };
        let ctx = EffectRecheckContext {
            run_id: "run-1",
            command_id: "cmd-1",
            definition_revision: "rev-1",
            expected_revision: "rev-2", // Mismatch!
            expected_generation: 1,
            current_generation: 1,
            authorization: Some(&auth),
            claimant: "worker-1",
            session_id: None,
            single_writer_registry: None,
            required_tokens: None,
            token_budget_remaining: None,
        };

        let err = recheck_before_effect(&ctx).unwrap_err();
        assert!(matches!(err, EffectRecheckFailure::VersionMismatch { .. }));
        assert_eq!(err.code(), "strategy_recheck_version_mismatch");
    }

    #[test]
    fn test_pre_effect_recheck_fails_on_stale_generation() {
        let auth = StrategyAuthorization {
            definition_digest: "def-1".into(),
            semantics_digest: "sem-1".into(),
            binding_digest: "bind-1".into(),
            authorization_digest: "auth-1".into(),
            revision: 1,
            active: true,
        };
        let ctx = EffectRecheckContext {
            run_id: "run-1",
            command_id: "cmd-1",
            definition_revision: "rev-1",
            expected_revision: "rev-1",
            expected_generation: 1,
            current_generation: 2, // Node generation changed!
            authorization: Some(&auth),
            claimant: "worker-1",
            session_id: None,
            single_writer_registry: None,
            required_tokens: None,
            token_budget_remaining: None,
        };

        let err = recheck_before_effect(&ctx).unwrap_err();
        assert!(matches!(err, EffectRecheckFailure::StaleGeneration { .. }));
        assert_eq!(err.code(), "strategy_recheck_stale_generation");
    }

    #[test]
    fn test_pre_effect_recheck_fails_on_budget_exhaustion() {
        let auth = StrategyAuthorization {
            definition_digest: "def-1".into(),
            semantics_digest: "sem-1".into(),
            binding_digest: "bind-1".into(),
            authorization_digest: "auth-1".into(),
            revision: 1,
            active: true,
        };
        let ctx = EffectRecheckContext {
            run_id: "run-1",
            command_id: "cmd-1",
            definition_revision: "rev-1",
            expected_revision: "rev-1",
            expected_generation: 1,
            current_generation: 1,
            authorization: Some(&auth),
            claimant: "worker-1",
            session_id: None,
            single_writer_registry: None,
            required_tokens: Some(5000),
            token_budget_remaining: Some(2000), // Exceeded!
        };

        let err = recheck_before_effect(&ctx).unwrap_err();
        assert!(matches!(
            err,
            EffectRecheckFailure::ResourceUnavailable { .. }
        ));
        assert_eq!(err.code(), "strategy_recheck_token_budget_exhausted");
    }

    #[test]
    fn test_callback_evolution_enricher_populates_facts_and_suggestions() {
        let temp_dir =
            std::env::temp_dir().join(format!("licoup-evolution-test-{}", uuid::Uuid::new_v4()));
        let enricher = CallbackEvolutionEnricher::new(&temp_dir);

        let mut snapshot = RunSnapshot::empty("test-run-1", "def-1", "sem-1");
        snapshot.status = StrategyRunStatus::Waiting;
        snapshot.conversation_id = Some("conv-1".into());
        snapshot.assistant_membership_id = Some("asst-1".into());
        snapshot.completed_states.insert("init".into());
        snapshot.state_visits.insert("init".into(), 1);
        snapshot.state_visits.insert("step1".into(), 1);

        let pending = PendingCallback {
            state_id: "step1".into(),
            state_visit: 1,
            transition_id: "trans-1".into(),
            event: licoup_workflow::TransitionEvent::Complete,
            target: "step2".into(),
        };

        let payload = enricher.enrich_callback_request(
            &snapshot,
            &pending,
            "lico_assistant_workflow_execute",
        );

        // Facts assertions
        assert_eq!(payload.facts.observation.completed_states, vec!["init"]);
        assert_eq!(payload.facts.context.conversation_id, Some("conv-1".into()));
        assert_eq!(payload.facts.context.state_id, "step1");
        assert_eq!(
            payload.facts.context.answer_channel,
            "lico_assistant_workflow_execute"
        );

        // Suggestions assertions
        assert_eq!(payload.suggestions.recommended_decision, "advance");
        assert!(payload.suggestions.decision_reasons.len() >= 1);
        assert!(
            payload
                .suggestions
                .alternative_decisions
                .contains(&"terminate".to_owned())
        );
        // No candidate catalog is wired in this slice, so the payload carries no
        // strategy advice rather than a model option the host never offered.
        assert!(payload.suggestions.strategy_suggestion.is_none());
    }

    #[test]
    fn callback_payload_omits_strategy_advice_when_the_candidate_catalog_is_empty() {
        let enricher = CallbackEvolutionEnricher::new(&temp_root("empty-catalog"));
        let snapshot = waiting_snapshot();
        let pending = callback_pending();

        let payload = enricher.enrich_callback_request(&snapshot, &pending, "strategy.run.resume");

        assert!(payload.suggestions.strategy_suggestion.is_none());
        assert!(payload.facts.observation.observation_seam_active);
        // The usage ledger is a host port. An uninstalled host is an unreadable
        // ledger, so spend stays unknown rather than a fabricated zero.
        assert!(payload.facts.cost.is_none());
    }

    #[test]
    fn callback_payload_reports_unknown_spend_as_unknown_not_zero() {
        struct UnreadableCostPort;
        impl EvolutionCostPort for UnreadableCostPort {
            fn query_cost_facts(&self, _root: &Path, _run_id: &str) -> Option<CallbackCostFacts> {
                None
            }
        }

        let enricher = CallbackEvolutionEnricher::new(&temp_root("unreadable-ledger"))
            .with_cost_port(Arc::new(UnreadableCostPort));

        let payload = enricher.enrich_callback_request(
            &waiting_snapshot(),
            &callback_pending(),
            "strategy.run.resume",
        );

        assert!(payload.facts.cost.is_none());
        assert!(
            payload
                .suggestions
                .decision_reasons
                .iter()
                .any(|reason| reason.contains("spend is unknown")),
            "{:?}",
            payload.suggestions.decision_reasons
        );
    }

    #[test]
    fn callback_payload_carries_the_adopted_default_until_it_is_revoked() {
        let candidates = vec![
            option("reviewer-1", "fast", "low"),
            option("reviewer-2", "deep", "high"),
        ];
        let strategy = Arc::new(
            DefaultEvolutionStrategyPort::new()
                .with_routing(routing_owner(&candidates.iter().collect::<Vec<_>>())),
        );
        strategy.adopt_default(AdoptedPlanningDefaultSeam {
            // The enricher scopes suggestions by the parked state.
            scope: PlanningScopeSeam {
                task_kind: "workflow-turn".into(),
                configuration: "step1".into(),
            },
            source: StrategySourceSeam {
                source_id: "eval-7".into(),
                revision: "rev-3".into(),
            },
            selected_option: candidates[1].clone(),
            revocable: true,
            supersedes_revision: None,
        });
        let enricher = CallbackEvolutionEnricher::new(&temp_root("adopted-default"))
            .with_strategy_port(strategy.clone())
            .with_strategy_candidates(candidates);

        let suggested = enricher
            .enrich_callback_request(
                &waiting_snapshot(),
                &callback_pending(),
                "strategy.run.resume",
            )
            .suggestions
            .strategy_suggestion
            .expect("the adopted default reaches the callback payload");
        assert_eq!(suggested.candidate.model_id, "deep");
        assert!(suggested.is_default);
        assert!(!suggested.has_execution_permission);

        // Revocation restores the previous policy: the configured order decides
        // again, and the routing policy — not insertion order — names the
        // alternative that runs.
        strategy.revoke_strategy("eval-7");
        let revoked = enricher
            .enrich_callback_request(
                &waiting_snapshot(),
                &callback_pending(),
                "strategy.run.resume",
            )
            .suggestions
            .strategy_suggestion
            .expect("the routing policy selects after revocation");
        assert_eq!(revoked.candidate.model_id, "fast");
        assert_eq!(revoked.ranking_basis, "candidate-policy");
        assert!(!revoked.is_default);
        assert!(!revoked.has_execution_permission);
    }

    /// With no routing owner composed there is no selection to make: the
    /// payload carries no strategy advice instead of the first entry of the
    /// caller's list.
    #[test]
    fn a_callback_without_a_routing_owner_carries_no_strategy_advice() {
        let enricher = CallbackEvolutionEnricher::new(&temp_root("no-routing-owner"))
            .with_strategy_port(Arc::new(DefaultEvolutionStrategyPort::new()))
            .with_strategy_candidates(vec![
                option("reviewer-1", "fast", "low"),
                option("reviewer-2", "deep", "high"),
            ]);

        let payload = enricher.enrich_callback_request(
            &waiting_snapshot(),
            &callback_pending(),
            "strategy.run.resume",
        );
        assert!(
            payload.suggestions.strategy_suggestion.is_none(),
            "an unanswered policy must not fall back to the first candidate"
        );
    }

    /// The decision keeps the scope it was asked for, so the same alternatives
    /// answer differently for a direct request than for a workflow turn.
    #[test]
    fn routing_keeps_direct_and_workflow_outcomes_apart() {
        let candidates = vec![option("reviewer-1", "fast", "low")];
        let owner = routing_owner(&candidates.iter().collect::<Vec<_>>());
        let workflow = owner.route(&candidate_routing_request(&candidates));
        assert_eq!(
            workflow
                .selected()
                .map(|selected| selected.model_id.as_str()),
            Some("fast")
        );
        let direct = owner.route(&CandidateRoutingRequest::new(
            SelectionScope::Direct,
            vec![CandidateOffer {
                candidate: CandidateId::new("reviewer-1", "fast", None),
                configured_position: 1,
            }],
        ));
        assert_eq!(direct.selected(), None);
        assert!(matches!(
            direct.unavailable(),
            Some(licoup_model_catalog::candidate_policy::CandidateUnavailable::NoAllowedCandidate)
        ));
        let _: CandidateRoutingOutcome = direct;
    }

    #[test]
    fn effect_recheck_compares_the_claimed_visit_against_the_run_visit() {
        let definition = test_definition(Some(active_authorization()));
        let mut snapshot = RunSnapshot::empty("run-1", "rev-1", "sem-1");
        snapshot.state_visits.insert("step".into(), 2);

        let receipt = recheck_before_effect(&EffectRecheckContext::for_run_command(
            "run-1",
            &snapshot,
            &definition,
            &test_command("step", 2),
            "worker-1",
        ))
        .expect("the command minted for the current visit executes");
        assert_eq!(receipt.generation, 2);
        assert_eq!(receipt.revision_digest, "rev-1");

        let failure = recheck_before_effect(&EffectRecheckContext::for_run_command(
            "run-1",
            &snapshot,
            &definition,
            &test_command("step", 1),
            "worker-1",
        ))
        .expect_err("a command left over from a superseded visit must not execute");
        assert!(matches!(
            failure,
            EffectRecheckFailure::StaleGeneration { .. }
        ));
        assert_eq!(failure.code(), "strategy_recheck_stale_generation");

        let never_entered = recheck_before_effect(&EffectRecheckContext::for_run_command(
            "run-1",
            &snapshot,
            &definition,
            &test_command("elsewhere", 1),
            "worker-1",
        ))
        .expect_err("a command for a state the run never entered must not execute");
        assert_eq!(never_entered.code(), "strategy_recheck_stale_generation");
    }

    #[test]
    fn effect_recheck_is_the_admission_gate_for_missing_and_revoked_authorization() {
        let mut revoked = active_authorization();
        revoked.active = false;
        let mut snapshot = RunSnapshot::empty("run-1", "rev-1", "sem-1");
        snapshot.state_visits.insert("step".into(), 1);
        let command = test_command("step", 1);

        for definition in [test_definition(Some(revoked)), test_definition(None)] {
            let failure = recheck_before_effect(&EffectRecheckContext::for_run_command(
                "run-1",
                &snapshot,
                &definition,
                &command,
                "worker-1",
            ))
            .expect_err("an unusable authorization stops the effect at the gate");
            assert!(matches!(
                failure,
                EffectRecheckFailure::PermissionDenied { .. }
            ));
            assert_eq!(failure.code(), "authorization_required");
        }
    }

    fn temp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("licoup-evolution-{tag}-{}", uuid::Uuid::new_v4()))
    }

    fn option(agent_id: &str, model_id: &str, thinking: &str) -> AgentModelOptionSeam {
        AgentModelOptionSeam {
            agent_id: agent_id.into(),
            model_id: model_id.into(),
            thinking: thinking.into(),
        }
    }

    fn waiting_snapshot() -> RunSnapshot {
        let mut snapshot = RunSnapshot::empty("test-run-1", "rev-1", "sem-1");
        snapshot.status = StrategyRunStatus::Waiting;
        snapshot.conversation_id = Some("conv-1".into());
        snapshot.assistant_membership_id = Some("asst-1".into());
        snapshot.completed_states.insert("init".into());
        snapshot.state_visits.insert("init".into(), 1);
        snapshot.state_visits.insert("step1".into(), 1);
        snapshot
    }

    fn callback_pending() -> PendingCallback {
        PendingCallback {
            state_id: "step1".into(),
            state_visit: 1,
            transition_id: "trans-1".into(),
            event: licoup_workflow::TransitionEvent::Complete,
            target: "step2".into(),
        }
    }

    fn active_authorization() -> StrategyAuthorization {
        StrategyAuthorization {
            definition_digest: "rev-1".into(),
            semantics_digest: "sem-1".into(),
            binding_digest: "bind-1".into(),
            authorization_digest: "auth-1".into(),
            revision: 1,
            active: true,
        }
    }

    fn test_command(state_id: &str, state_visit: u64) -> RunCommand {
        RunCommand {
            id: format!("command:{state_id}:{state_visit}"),
            state_id: state_id.to_owned(),
            state_visit,
            kind: licoup_workflow::CommandKind::Actor,
            status: licoup_workflow::CommandStatus::Claimed,
            attempt: 1,
            attempt_token: "attempt:1".into(),
            binding_id: Some("entry".into()),
            runtime_id: None,
            entry: None,
            item_id: None,
            session_policy: licoup_workflow::SessionPolicy::default(),
            binding_ordinal: 0,
            resume_session_id: None,
            input_digest: "input-1".into(),
            input: json!({"message": "hi"}),
            output_digest: None,
            failure_class: None,
            failure_code: None,
        }
    }

    fn test_definition(authorization: Option<StrategyAuthorization>) -> StrategyDefinition {
        use licoup_workflow::{
            GraphState, GraphStateKind, RetryPolicy, WorkflowDefinition, WorkflowLimits,
            WorkflowMetadata,
        };
        StrategyDefinition {
            summary: licoup_workflow_store::StrategyDefinitionSummary {
                definition_id: "evolution-test".into(),
                revision_digest: "rev-1".into(),
                semantics_digest: "sem-1".into(),
                name: "Evolution test".into(),
                version: "1".into(),
                imported_at_unix_ms: 0,
                authorized: authorization.is_some(),
            },
            workflow: WorkflowDefinition {
                schema: licoup_workflow::WORKFLOW_SCHEMA_VERSION.into(),
                metadata: WorkflowMetadata {
                    id: "evolution-test".into(),
                    name: "Evolution test".into(),
                    version: "1".into(),
                    description: String::new(),
                },
                limits: WorkflowLimits::default(),
                actor_slots: Vec::new(),
                runtimes: Vec::new(),
                worksets: Vec::new(),
                initial: "step".into(),
                states: vec![GraphState {
                    id: "step".into(),
                    kind: GraphStateKind::Succeed,
                    label: "Step".into(),
                    instruction: String::new(),
                    binding: None,
                    runtime: None,
                    entry: None,
                    workset: None,
                    retry: RetryPolicy::default(),
                }],
                transitions: Vec::new(),
            },
            asset_count: 0,
            bindings: Vec::new(),
            authorization,
        }
    }
}
