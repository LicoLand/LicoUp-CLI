//! Persistent observation of what work actually cost.
//!
//! The durable facts belong to
//! [`licoup_statistics::ledger`]: it owns the Graph run
//! identity, the per-command usage row and its accuracy marker. This adapter
//! writes those rows through that API and reads them back as typed values.
//!
//! Three rules from the plan are enforced here rather than described:
//!
//! 1. **Every outcome is observed.** A failed, cancelled or in-doubt effect is
//!    recorded like a successful one: a call that failed after tokens were spent
//!    still cost those tokens.
//! 2. **Unknown is not zero.** A cost that was not reported, or that belongs to
//!    an effect whose external outcome is in doubt, is [`ObservedCost::Unknown`].
//!    The record sent to the ledger carries no usage sample at all, so no zero
//!    is written in place of a number nobody has.
//! 3. **Metering is not presentation.** [`ObservationAdapter::meter_run`]
//!    returns the base facts a budget or a policy may read.
//!    [`ObservationAdapter::statistics`] returns an optional projection for
//!    display or research; nothing that admits, reserves or settles work may
//!    read it, and it is allowed to be absent.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use licoup_statistics::ledger;

use super::{AdapterError, AdapterName, AdapterResult, CostUsage, FactState};

const ADAPTER: AdapterName = AdapterName::Observation;

/// Why a cost is unknown.
///
/// The reasons are kept apart because they need different follow-up: an
/// in-doubt effect must be reconciled, an unreported one may arrive late, and
/// an unreadable ledger is a local read failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CostUnknownReason {
    /// The provider or adapter reported no usage for this effect.
    NotReported,
    /// The external outcome is in doubt, so no number can be trusted yet.
    InDoubt,
    /// No effect of this run has been recorded, so no spend was observed.
    NotRecorded,
    /// The ledger could not be read.
    Unreadable,
    /// Some effects of this run have a known cost and at least one does not.
    /// The sum of the known ones is a lower bound, not the run's cost.
    SomeEffectsUnknown,
}

/// What one effect cost, as far as durable facts support.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "cost")]
pub enum ObservedCost {
    /// A recorded sample. A measured zero is `Known`, because the reporter
    /// asserted it; anything else is [`ObservedCost::Unknown`]. How the owner
    /// obtained the counters — exact or estimated — is a provenance question
    /// answered by [`super::source`], not by this state.
    Known { usage: CostUsage },
    /// No number exists. This is never rendered as zero.
    Unknown { reason: CostUnknownReason },
}

impl ObservedCost {
    pub const fn state(self) -> FactState {
        match self {
            Self::Known { .. } => FactState::Present,
            Self::Unknown { .. } => FactState::Unknown,
        }
    }

    /// The sample, when one exists. `None` is the unknown case — callers must
    /// decide what to do about it rather than substitute a zero.
    pub const fn usage(self) -> Option<CostUsage> {
        match self {
            Self::Known { usage } => Some(usage),
            Self::Unknown { .. } => None,
        }
    }
}

/// The command kind the ledger records for an effect.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EffectKind {
    Authorization,
    Actor,
    Script,
    WorksetItem,
}

impl EffectKind {
    const fn ledger_kind(self) -> &'static str {
        match self {
            Self::Authorization => "authorization",
            Self::Actor => "actor",
            Self::Script => "script",
            Self::WorksetItem => "workset-item",
        }
    }
}

/// A terminal outcome of an effect.
///
/// The names follow C03 rather than inventing a second effect vocabulary, and
/// they map onto the ledger's own command and settlement statuses.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EffectOutcome {
    Succeeded,
    Failed,
    Cancelled,
    /// The effect may have run and its result is not authenticated.
    Unknown,
}

impl EffectOutcome {
    pub const fn ledger_status(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Unknown => "in-doubt",
        }
    }
}

/// The lifecycle a run is recorded with.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunLifecycle {
    Pending,
    Running,
    Waiting,
    Blocked,
    Completed,
    Failed,
    Cancelled,
}

impl RunLifecycle {
    pub const fn ledger_status(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Blocked => "blocked",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// The run identity an observation belongs to.
///
/// The ledger owns this identity; the adapter only carries it. Registering the
/// same `run_id` again with a different revision or conversation is the
/// ledger's conflict to report, and it reports one.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunObservationIdentity {
    pub run_id: String,
    pub revision_digest: String,
    pub conversation_id: Option<String>,
    pub assistant_membership_id: Option<String>,
    pub lifecycle: RunLifecycle,
}

/// One effect's outcome and cost, ready to be made durable.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectObservation {
    pub run_id: String,
    /// The durable effect identity. Reservation and settlement use this same
    /// id, so one effect never has two cost identities.
    pub effect_id: String,
    pub state_id: String,
    pub kind: EffectKind,
    pub membership_id: Option<String>,
    /// Labels the ledger records for attribution. They are provenance, not an
    /// authorization context: see [`super::source`] and [`super::policy`].
    pub agent_id: Option<String>,
    pub model: Option<String>,
    /// Attempt number of this effect. Later attempts are how rework becomes a
    /// durable fact instead of a guess.
    pub attempt: u64,
    pub outcome: EffectOutcome,
    pub cost: ObservedCost,
}

/// Receipt for one recorded observation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordReceipt {
    pub effect_id: String,
    pub outcome: EffectOutcome,
    pub attempt: u64,
    /// What was made durable. For an unknown cost this says unknown, and no
    /// usage sample was written in its place.
    pub recorded: ObservedCost,
    pub state: FactState,
}

/// One effect's metering fact, read back from the ledger.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectMeteringFact {
    pub effect_id: String,
    pub state_id: String,
    /// `None` while the effect has no terminal outcome recorded.
    pub outcome: Option<EffectOutcome>,
    pub attempt: u64,
    pub agent_id: Option<String>,
    pub model: Option<String>,
    pub cost: ObservedCost,
}

/// Sums over the effects whose cost was actually read.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeteringTotals {
    pub known_usage: CostUsage,
    pub known_effects: u64,
    pub unknown_effects: u64,
}

impl MeteringTotals {
    /// The cost of this set, or an honest unknown when any effect was not read.
    pub const fn cost(self) -> ObservedCost {
        if self.unknown_effects > 0 {
            ObservedCost::Unknown {
                reason: CostUnknownReason::SomeEffectsUnknown,
            }
        } else {
            ObservedCost::Known {
                usage: self.known_usage,
            }
        }
    }

    /// The sum of the effects that were read. This is a lower bound whenever
    /// `unknown_effects` is non-zero; it is never presented as the total.
    pub const fn read_tokens(self) -> u64 {
        self.known_usage.total_tokens
    }
}

/// The base metering facts of one run.
///
/// This is what admission, reservation and settlement may read.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeteringFacts {
    pub run_id: String,
    pub state: FactState,
    pub effects: Vec<EffectMeteringFact>,
    pub totals: MeteringTotals,
    /// Effects recorded with an attempt above the first.
    pub rework_effects: u64,
}

impl MeteringFacts {
    /// The run's cost as a durable fact, honouring the state before the sums.
    pub const fn total_cost(&self) -> ObservedCost {
        match self.state {
            FactState::Unreadable => ObservedCost::Unknown {
                reason: CostUnknownReason::Unreadable,
            },
            FactState::NotRecorded => ObservedCost::Unknown {
                reason: CostUnknownReason::NotRecorded,
            },
            _ if self.effects.is_empty() => ObservedCost::Unknown {
                reason: CostUnknownReason::NotRecorded,
            },
            _ => self.totals.cost(),
        }
    }

    pub fn effect(&self, effect_id: &str) -> Option<&EffectMeteringFact> {
        self.effects
            .iter()
            .find(|effect| effect.effect_id == effect_id)
    }
}

/// Optional projection for display or research.
///
/// Derived from the same rows as [`MeteringFacts`], but it is presentation: it
/// is allowed to be absent, and no admission, reservation or settlement may
/// read it. It exists so a statistics surface does not grow out of the base
/// facts and then start being treated as authoritative.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservationStatistics {
    pub run_id: String,
    pub by_model: BTreeMap<String, MeteringTotals>,
    pub total_effects: u64,
    pub rework_effects: u64,
    pub succeeded_effects: u64,
    pub failed_effects: u64,
    pub cancelled_effects: u64,
    pub unknown_outcome_effects: u64,
}

/// The observation adapter over the durable usage ledger.
pub struct ObservationAdapter {
    portable_root: PathBuf,
}

impl ObservationAdapter {
    pub fn new(portable_root: &Path) -> Self {
        Self {
            portable_root: portable_root.to_path_buf(),
        }
    }

    pub fn portable_root(&self) -> &Path {
        &self.portable_root
    }

    /// Make the ledger's own run identity durable before observing its effects.
    ///
    /// The run identity belongs to the ledger, not to this adapter. Without it
    /// the ledger refuses a command row, and that refusal is reported as an
    /// [`AdapterError`] with code `observation_run_not_registered` rather than
    /// being papered over.
    pub fn register_run(&self, identity: &RunObservationIdentity) -> AdapterResult<FactState> {
        let mut params = json!({
            "stateRoot": self.state_root(),
            "runId": identity.run_id,
            "revisionDigest": identity.revision_digest,
            "status": identity.lifecycle.ledger_status(),
        });
        if let Some(conversation_id) = &identity.conversation_id {
            params["conversationId"] = json!(conversation_id);
        }
        if let Some(membership_id) = &identity.assistant_membership_id {
            params["assistantMembershipId"] = json!(membership_id);
        }
        workflow_ledger::begin_graph_run(&params).map_err(ledger_error)?;
        Ok(FactState::Present)
    }

    /// Persist one effect's outcome and cost.
    ///
    /// A successful, failed, cancelled and in-doubt effect all take this path.
    /// The caller states the cost as [`ObservedCost`]: passing
    /// [`ObservedCost::Unknown`] writes an unknown marker with no sample, which
    /// is the only honest record for a number nobody could read.
    pub fn record(&self, observation: &EffectObservation) -> AdapterResult<RecordReceipt> {
        let mut params = json!({
            "stateRoot": self.state_root(),
            "runId": observation.run_id,
            "commandId": observation.effect_id,
            "stateId": observation.state_id,
            "kind": observation.kind.ledger_kind(),
            "status": observation.outcome.ledger_status(),
            "attempt": observation.attempt,
        });
        if let Some(membership_id) = &observation.membership_id {
            params["membershipId"] = json!(membership_id);
        }
        if let Some(agent_id) = &observation.agent_id {
            params["agentId"] = json!(agent_id);
        }
        if let Some(model) = &observation.model {
            params["model"] = json!(model);
        }
        if let ObservedCost::Known { usage } = observation.cost {
            params["usage"] = usage_value(usage);
        }
        workflow_ledger::record_graph_command(&params).map_err(ledger_error)?;
        Ok(RecordReceipt {
            effect_id: observation.effect_id.clone(),
            outcome: observation.outcome,
            attempt: observation.attempt,
            recorded: observation.cost,
            state: observation.cost.state(),
        })
    }

    /// Read the base metering facts of one run.
    ///
    /// A run with no recorded effects answers [`FactState::NotRecorded`], not a
    /// zero total: no spend was observed.
    pub fn meter_run(&self, run_id: &str) -> AdapterResult<MeteringFacts> {
        match self.read_run(run_id)? {
            Some(rows) => rows.into_metering(run_id),
            None => Ok(MeteringFacts {
                run_id: run_id.to_owned(),
                state: FactState::NotRecorded,
                effects: Vec::new(),
                totals: MeteringTotals::default(),
                rework_effects: 0,
            }),
        }
    }

    /// Build the optional presentation projection.
    ///
    /// `None` means the ledger could not be read, or a recorded row cannot be
    /// aggregated. The projection is optional by construction, so a caller may
    /// simply show nothing instead.
    pub fn statistics(&self, run_id: &str) -> Option<ObservationStatistics> {
        let rows = self.read_run(run_id).ok().flatten()?;
        let mut by_model = BTreeMap::<String, MeteringTotals>::new();
        for effect in &rows.effects {
            let Some(model) = effect.model.as_deref() else {
                continue;
            };
            accumulate(by_model.entry(model.to_owned()).or_default(), effect.cost).ok()?;
        }
        Some(ObservationStatistics {
            run_id: run_id.to_owned(),
            by_model,
            total_effects: rows.effects.len() as u64,
            rework_effects: rows.rework_effects(),
            succeeded_effects: rows.count(EffectOutcome::Succeeded),
            failed_effects: rows.count(EffectOutcome::Failed),
            cancelled_effects: rows.count(EffectOutcome::Cancelled),
            unknown_outcome_effects: rows.count(EffectOutcome::Unknown),
        })
    }

    fn read_run(&self, run_id: &str) -> AdapterResult<Option<RunRows>> {
        let report = workflow_ledger::workflow_report(&json!({
            "stateRoot": self.state_root(),
            "runId": run_id,
        }))
        .map_err(ledger_error)?;
        let Some(run) = report
            .get("runs")
            .and_then(Value::as_array)
            .and_then(|runs| {
                runs.iter()
                    .find(|run| run.get("runId").and_then(Value::as_str) == Some(run_id))
            })
        else {
            return Ok(None);
        };
        let effects = run
            .get("commands")
            .and_then(Value::as_array)
            .map(|commands| commands.iter().map(decode_effect).collect())
            .unwrap_or_default();
        Ok(Some(RunRows { effects }))
    }

    fn state_root(&self) -> String {
        self.portable_root.to_string_lossy().into_owned()
    }
}

struct RunRows {
    effects: Vec<EffectMeteringFact>,
}

impl RunRows {
    fn count(&self, outcome: EffectOutcome) -> u64 {
        self.effects
            .iter()
            .filter(|effect| effect.outcome == Some(outcome))
            .count() as u64
    }

    fn rework_effects(&self) -> u64 {
        self.effects
            .iter()
            .filter(|effect| effect.attempt > 1)
            .count() as u64
    }

    fn into_metering(self, run_id: &str) -> AdapterResult<MeteringFacts> {
        let rework_effects = self.rework_effects();
        let mut totals = MeteringTotals::default();
        for effect in &self.effects {
            accumulate(&mut totals, effect.cost)?;
        }
        Ok(MeteringFacts {
            run_id: run_id.to_owned(),
            state: FactState::Present,
            effects: self.effects,
            totals,
            rework_effects,
        })
    }
}

fn usage_value(usage: CostUsage) -> Value {
    json!({
        "promptTokens": usage.prompt_tokens,
        "cachedInputTokens": usage.cached_input_tokens,
        "completionTokens": usage.completion_tokens,
        "totalTokens": usage.total_tokens,
        "accuracy": "exact",
    })
}

fn decode_effect(command: &Value) -> EffectMeteringFact {
    let outcome = command
        .get("status")
        .and_then(Value::as_str)
        .and_then(decode_outcome);
    let accuracy = command
        .get("accuracy")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    EffectMeteringFact {
        effect_id: text(command, "commandId"),
        state_id: text(command, "stateId"),
        outcome,
        attempt: command.get("attempt").and_then(Value::as_u64).unwrap_or(0),
        agent_id: command
            .get("agentId")
            .and_then(Value::as_str)
            .map(str::to_owned),
        model: command
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_owned),
        cost: decode_cost(accuracy, command.get("usage"), outcome),
    }
}

fn decode_outcome(status: &str) -> Option<EffectOutcome> {
    match status {
        "succeeded" => Some(EffectOutcome::Succeeded),
        "failed" => Some(EffectOutcome::Failed),
        "cancelled" => Some(EffectOutcome::Cancelled),
        "in-doubt" => Some(EffectOutcome::Unknown),
        _ => None,
    }
}

fn decode_cost(
    accuracy: &str,
    usage: Option<&Value>,
    outcome: Option<EffectOutcome>,
) -> ObservedCost {
    if accuracy == "unknown" {
        return ObservedCost::Unknown {
            reason: if outcome == Some(EffectOutcome::Unknown) {
                CostUnknownReason::InDoubt
            } else {
                CostUnknownReason::NotReported
            },
        };
    }
    let read = |key: &str| {
        usage
            .and_then(|usage| usage.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    ObservedCost::Known {
        usage: CostUsage {
            prompt_tokens: read("promptTokens"),
            cached_input_tokens: read("cachedInputTokens"),
            completion_tokens: read("completionTokens"),
            total_tokens: read("totalTokens"),
        },
    }
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn accumulate(totals: &mut MeteringTotals, cost: ObservedCost) -> AdapterResult<()> {
    let ObservedCost::Known { usage } = cost else {
        totals.unknown_effects += 1;
        return Ok(());
    };
    let overflow = || {
        AdapterError::refused(
            ADAPTER,
            "observation_aggregate_overflow",
            "correct_the_recorded_counters",
        )
    };
    let known_usage = CostUsage {
        prompt_tokens: totals
            .known_usage
            .prompt_tokens
            .checked_add(usage.prompt_tokens)
            .ok_or_else(overflow)?,
        cached_input_tokens: totals
            .known_usage
            .cached_input_tokens
            .checked_add(usage.cached_input_tokens)
            .ok_or_else(overflow)?,
        completion_tokens: totals
            .known_usage
            .completion_tokens
            .checked_add(usage.completion_tokens)
            .ok_or_else(overflow)?,
        total_tokens: totals
            .known_usage
            .total_tokens
            .checked_add(usage.total_tokens)
            .ok_or_else(overflow)?,
    };
    totals.known_usage = known_usage;
    totals.known_effects += 1;
    Ok(())
}

fn ledger_error(error: workflow_ledger::LedgerError) -> AdapterError {
    AdapterError::from_ledger(ADAPTER, &error)
}
