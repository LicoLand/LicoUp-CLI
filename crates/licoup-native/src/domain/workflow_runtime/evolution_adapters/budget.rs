//! Reserve and settle budget by effect id, against a durable reservation.
//!
//! The durable facts belong to
//! [`licoup_statistics::ledger`]: it owns the budget pool, the
//! reservation row and its settlement state. This adapter calls
//! `reserve_graph_command` / `settle_graph_command` / `release_graph_command`
//! with the effect id as the reservation identity and translates the answers
//! into typed admissions, settlements and releases.
//!
//! # Two connections, no pretend transaction
//!
//! The reservation is durable in the usage ledger; the workflow's started
//! marker is durable in the workflow store. Those are two SQLite connections
//! and this adapter never claims otherwise. The order is
//! *reserve → commit started → invoke → settle*, so:
//!
//! - a crash before the started marker leaves an **orphan**: nothing ran, and
//!   [`BudgetAdapter::release_orphan`] gives the amount back idempotently;
//! - a crash after it leaves an **in-doubt reservation**: the effect may have
//!   run, so the amount stays held until a settlement or reconciliation
//!   supplies usage instead of being re-allocated.
//!
//! # No configuration, no gate
//!
//! A request with no [`BudgetConfiguration`] answers
//! [`BudgetAdmission::NotConfigured`] and writes nothing at all — no pool row,
//! no reservation, no denial. An unconfigured budget must not become an
//! invented gate, and the effect's real cost is still observed by
//! [`super::observation`].

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use licoup_statistics::ledger;

use super::observation::EffectOutcome;
use super::source::SettleableUsage;
use super::{AdapterError, AdapterName, AdapterResult, CostUsage, FactState};

const ADAPTER: AdapterName = AdapterName::Budget;

/// The honest state when no budget is configured.
///
/// It carries no limit and therefore enforces nothing: work proceeds and the
/// cost is still metered.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BudgetNotConfigured;

impl BudgetNotConfigured {
    pub const fn state(self) -> FactState {
        FactState::NotConfigured
    }

    /// An unconfigured budget blocks no work.
    pub const fn work_proceeds(self) -> bool {
        true
    }
}

/// What the caller has configured for this decision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BudgetConfiguration {
    pub budget_id: String,
    pub limit_tokens: Option<u64>,
    pub used_tokens: Option<u64>,
    pub remaining_tokens: Option<u64>,
}

impl BudgetConfiguration {
    /// A pool described by the user's configured limit.
    pub fn limit(budget_id: impl Into<String>, limit_tokens: u64) -> Self {
        Self {
            budget_id: budget_id.into(),
            limit_tokens: Some(limit_tokens),
            used_tokens: None,
            remaining_tokens: Some(limit_tokens),
        }
    }

    /// A pool described by counters observed from the provider side.
    pub fn observed(budget_id: impl Into<String>, used_tokens: u64, remaining_tokens: u64) -> Self {
        Self {
            budget_id: budget_id.into(),
            limit_tokens: None,
            used_tokens: Some(used_tokens),
            remaining_tokens: Some(remaining_tokens),
        }
    }
}

/// The admission estimate for one effect.
///
/// An estimate protects concurrent work. It is not a measurement, and the
/// adapter never reports it as actual spend.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenEstimate {
    pub total_tokens: u64,
}

/// One effect asking to be admitted against the configured budget.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectBudgetRequest {
    /// The durable effect id. Reservation, settlement and release all use it,
    /// so one effect never accumulates two reservations.
    pub effect_id: String,
    pub run_id: Option<String>,
    pub command_id: Option<String>,
    pub estimate: Option<TokenEstimate>,
    /// Whether this effect may spend at all. A read that cannot spend is
    /// admitted as a free read by the ledger's own rule.
    pub chargeable: bool,
    /// `None` means no budget is configured. It is not a zero limit.
    pub budget: Option<BudgetConfiguration>,
}

/// The state of one durable reservation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReservationState {
    /// Held for an effect that has not reported usage yet.
    Reserved,
    /// Held for an effect whose outcome is unknown; it must settle or be
    /// reconciled, never be given back early.
    UnknownHeld,
    Settled,
    Released,
}

impl ReservationState {
    fn parse(value: &str) -> AdapterResult<Self> {
        match value {
            "reserved" => Ok(Self::Reserved),
            "unknown" => Ok(Self::UnknownHeld),
            "settled" => Ok(Self::Settled),
            "released" => Ok(Self::Released),
            _ => Err(AdapterError::refused(
                ADAPTER,
                "budget_reservation_state_unrecognized",
                "inspect_the_recorded_reservation",
            )),
        }
    }

    pub const fn fact_state(self) -> FactState {
        match self {
            Self::Reserved | Self::Settled | Self::Released => FactState::Present,
            Self::UnknownHeld => FactState::Unknown,
        }
    }
}

/// A reservation that was granted, identified by the effect it belongs to.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReservationIdentity {
    pub effect_id: String,
    pub budget_id: String,
    pub state: ReservationState,
    pub reserved_tokens: u64,
    pub estimate_tokens: Option<u64>,
    pub available_tokens: Option<u64>,
    /// True when the ledger answered with the reservation it already had for
    /// this effect id, so a retry cannot reserve the same effect twice.
    pub reused: bool,
}

/// Why new work was not admitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BudgetDenial {
    /// The pool has nothing available.
    Exhausted,
    /// The estimate alone exceeds what is available.
    ExceedsRemaining,
}

/// A denial from a configured budget.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BudgetWaiting {
    pub effect_id: String,
    pub budget_id: String,
    pub denial: BudgetDenial,
    pub available_tokens: Option<u64>,
    pub estimate_tokens: Option<u64>,
    /// New dispatch stops, while work already in flight keeps running.
    pub stops_new_dispatch: bool,
    pub keeps_in_flight: bool,
}

/// The answer to a reservation request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "admission")]
pub enum BudgetAdmission {
    /// No budget is configured: nothing is enforced and work proceeds.
    NotConfigured { unconfigured: BudgetNotConfigured },
    /// A durable reservation is held for this effect id.
    Reserved { reservation: ReservationIdentity },
    /// A configured budget refused this effect.
    Waiting { waiting: BudgetWaiting },
    /// The effect cannot spend, so the ledger admitted it as a free read.
    FreeRead {
        effect_id: String,
        budget_id: String,
    },
    /// The effect id already has a concluded reservation: the ledger settled it
    /// against real usage or released it as never-started. Nothing new is held,
    /// and this is not a budget gate — the effect must not start again under
    /// this identity.
    AlreadyConcluded {
        effect_id: String,
        budget_id: String,
        state: ReservationState,
    },
}

impl BudgetAdmission {
    pub const fn state(&self) -> FactState {
        match self {
            Self::NotConfigured { .. } => FactState::NotConfigured,
            Self::Reserved { reservation } => reservation.state.fact_state(),
            Self::Waiting { .. } => FactState::Present,
            Self::FreeRead { .. } => FactState::Present,
            Self::AlreadyConcluded { .. } => FactState::Present,
        }
    }

    /// Whether the caller may start the effect.
    ///
    /// Only a configured budget that refused says no; an unconfigured budget
    /// never becomes a gate. A concluded reservation also says no, but as a
    /// stale identity rather than as a budget denial.
    pub const fn work_proceeds(&self) -> bool {
        match self {
            Self::NotConfigured { .. } | Self::Reserved { .. } | Self::FreeRead { .. } => true,
            Self::Waiting { .. } | Self::AlreadyConcluded { .. } => false,
        }
    }

    /// The effect this admission is about. Empty only when no budget is
    /// configured, where no reservation identity exists.
    pub fn effect_id(&self) -> &str {
        match self {
            Self::NotConfigured { .. } => "",
            Self::Reserved { reservation } => reservation.effect_id.as_str(),
            Self::Waiting { waiting } => waiting.effect_id.as_str(),
            Self::FreeRead { effect_id, .. } => effect_id.as_str(),
            Self::AlreadyConcluded { effect_id, .. } => effect_id.as_str(),
        }
    }

    pub const fn reservation(&self) -> Option<&ReservationIdentity> {
        match self {
            Self::Reserved { reservation } => Some(reservation),
            _ => None,
        }
    }
}

/// What one effect reports when it finishes.
///
/// The usage is [`SettleableUsage`], so an imported usage marker cannot be
/// passed here at all: see [`super::source`]. `usage: None` records the outcome
/// with no number, which keeps the reservation held.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectSettlement {
    pub outcome: EffectOutcome,
    pub usage: Option<SettleableUsage>,
}

impl EffectSettlement {
    /// A settlement carrying the effect's own reported outcome.
    pub const fn reported(outcome: EffectOutcome, usage: SettleableUsage) -> Self {
        Self {
            outcome,
            usage: Some(usage),
        }
    }

    /// A settlement with no number: failed, cancelled or in-doubt, exact spend
    /// unknown.
    pub const fn unknown(outcome: EffectOutcome) -> Self {
        Self {
            outcome,
            usage: None,
        }
    }

    /// What the effect cost, when a number exists.
    pub const fn cost(&self) -> Option<CostUsage> {
        match &self.usage {
            Some(usage) => Some(usage.usage()),
            None => None,
        }
    }

    const fn ledger_status(&self) -> &'static str {
        match self.outcome {
            EffectOutcome::Succeeded => "completed",
            EffectOutcome::Failed => "failed",
            EffectOutcome::Cancelled => "cancelled",
            EffectOutcome::Unknown => "unknown",
        }
    }
}

/// The durable result of settling or releasing one reservation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettlementReceipt {
    pub effect_id: String,
    pub budget_id: String,
    pub state: FactState,
    pub reservation_state: ReservationState,
    pub reserved_tokens: u64,
    /// Charged tokens, when the settlement had a number for them.
    pub actual_tokens: Option<u64>,
    pub overage_tokens: u64,
    /// True when usage arrived after the effect had already been settled as
    /// unknown.
    pub late: bool,
    pub available_tokens: Option<u64>,
    /// The outcome the ledger recorded for this settlement.
    pub settlement_status: Option<String>,
}

/// The answer to a settlement.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "settlement")]
pub enum SettlementOutcome {
    /// Actual usage was charged; the estimate is no longer held.
    Settled { receipt: SettlementReceipt },
    /// No usage was available for an effect that may have run. The reservation
    /// keeps holding its estimate until reconciliation supplies usage.
    UnknownHeld { receipt: SettlementReceipt },
    /// Nothing was reserved, because no budget was configured. The effect's
    /// cost is still recorded by the observation adapter.
    NotConfigured { unconfigured: BudgetNotConfigured },
    /// The ledger has no reservation for this effect id.
    NotReserved {
        effect_id: String,
        absence: ReservationAbsent,
    },
}

impl SettlementOutcome {
    pub const fn state(&self) -> FactState {
        match self {
            Self::Settled { receipt } => receipt.state,
            Self::UnknownHeld { .. } => FactState::Unknown,
            Self::NotConfigured { .. } => FactState::NotConfigured,
            Self::NotReserved { .. } => FactState::NotRecorded,
        }
    }

    pub const fn receipt(&self) -> Option<&SettlementReceipt> {
        match self {
            Self::Settled { receipt } | Self::UnknownHeld { receipt } => Some(receipt),
            _ => None,
        }
    }

    /// Whether a late report may still fill in a number for this effect.
    pub const fn reconciliation_required(&self) -> bool {
        matches!(self, Self::UnknownHeld { .. })
    }
}

/// Why no reservation was found for an effect id.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReservationAbsent {
    /// This effect was never reserved.
    NeverReserved,
    /// The reservation was already given back.
    AlreadyReleased,
}

/// What the caller knows about whether the effect was started.
///
/// The started marker lives in the workflow store, not in the usage ledger, so
/// the caller supplies this evidence and the adapter refuses to release budget
/// for anything that may have run.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EffectStartEvidence {
    /// The effect provably never started: its reservation is an orphan.
    NeverStarted,
    /// The effect was started, or may have been.
    PossiblyStarted,
}

/// A request to give an orphaned reservation back.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrphanReleaseRequest {
    pub effect_id: String,
    pub budget_id: Option<String>,
    pub evidence: EffectStartEvidence,
}

/// The answer to an orphan release.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "release")]
pub enum ReleaseOutcome {
    /// The held amount is available again.
    ///
    /// The ledger's answer does not distinguish a release made now from one
    /// made earlier, so repeating the call reports the same state — which is
    /// exactly what makes the release safe to retry. What a caller then checks
    /// is the pool: the amount is available and the effect holds no second
    /// reservation.
    Released { receipt: SettlementReceipt },
    /// The reservation was already settled against real usage; nothing is given
    /// back.
    AlreadySettled {
        effect_id: String,
        budget_id: String,
    },
    /// The adapter refused: the effect may have run, so its amount stays held
    /// and is not re-allocated.
    RefusedEarly {
        effect_id: String,
        evidence: EffectStartEvidence,
    },
    NotReserved {
        effect_id: String,
        absence: ReservationAbsent,
    },
}

impl ReleaseOutcome {
    pub const fn state(&self) -> FactState {
        match self {
            Self::Released { .. } | Self::AlreadySettled { .. } => FactState::Present,
            Self::RefusedEarly { .. } => FactState::RefusedEarly,
            Self::NotReserved { .. } => FactState::NotRecorded,
        }
    }
}

/// One pool and what it currently holds.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BudgetPoolFact {
    pub budget_id: String,
    pub limit_tokens: Option<u64>,
    pub actual_tokens: u64,
    pub reserved_tokens: u64,
    pub available_tokens: Option<u64>,
    /// The ledger's own words for the numeric state: available, exhausted, or
    /// unknown when no limit was configured.
    pub status: BudgetPoolStatus,
}

/// The numeric state of a pool.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BudgetPoolStatus {
    Available,
    Exhausted,
    Unknown,
}

impl BudgetPoolStatus {
    fn parse(value: &str) -> Self {
        match value {
            "available" => Self::Available,
            "exhausted" => Self::Exhausted,
            _ => Self::Unknown,
        }
    }
}

/// A reservation still holding budget whose started state the ledger cannot
/// decide.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrphanCandidate {
    pub effect_id: String,
    pub budget_id: String,
    pub run_id: Option<String>,
    pub command_id: Option<String>,
    pub reserved_tokens: u64,
}

/// What a boot-time reconciliation sees.
///
/// The candidates are just that: the usage ledger knows a reservation is held
/// but not whether the effect started, so the caller must recheck the started
/// marker in the workflow store before releasing anything.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconciliationReport {
    pub state: FactState,
    pub pools: Vec<BudgetPoolFact>,
    pub in_flight_count: u64,
    pub unknown_in_flight_count: u64,
    /// A configured pool has nothing available, so new dispatch stops while
    /// work in flight keeps running.
    pub stops_new_dispatch: bool,
    pub orphan_candidates: Vec<OrphanCandidate>,
}

impl ReconciliationReport {
    /// Whether any budget is configured at all. When false nothing is enforced
    /// and there is nothing to reconcile.
    pub fn budgets_configured(&self) -> bool {
        !self.pools.is_empty()
    }
}

/// The budget adapter over the durable reservations in the usage ledger.
pub struct BudgetAdapter {
    portable_root: PathBuf,
}

impl BudgetAdapter {
    pub fn new(portable_root: &Path) -> Self {
        Self {
            portable_root: portable_root.to_path_buf(),
        }
    }

    pub fn portable_root(&self) -> &Path {
        &self.portable_root
    }

    /// Try to hold budget for one effect.
    ///
    /// The reservation is durable before the caller commits the started marker,
    /// so a crash in between leaves a releasable orphan rather than an
    /// unaccounted spend. Repeating the call for the same effect id returns the
    /// reservation already held instead of reserving twice.
    pub fn reserve(&self, request: &EffectBudgetRequest) -> AdapterResult<BudgetAdmission> {
        let Some(budget) = &request.budget else {
            return Ok(BudgetAdmission::NotConfigured {
                unconfigured: BudgetNotConfigured,
            });
        };
        let mut params = json!({
            "stateRoot": self.state_root(),
            "budgetId": budget.budget_id,
            "invocationId": request.effect_id,
            "chargeable": request.chargeable,
            "budget": budget_value(budget),
        });
        if let Some(run_id) = &request.run_id {
            params["runId"] = json!(run_id);
        }
        if let Some(command_id) = &request.command_id {
            params["commandId"] = json!(command_id);
        }
        if let Some(estimate) = &request.estimate {
            params["estimate"] = json!({
                "totalTokens": estimate.total_tokens,
                "accuracy": "estimated",
            });
        }
        let answer = workflow_ledger::reserve_graph_command(&params).map_err(ledger_error)?;
        decode_admission(&answer, request)
    }

    /// Charge or hold the reservation for one effect.
    ///
    /// Passing no usage records the outcome without a number: the reservation
    /// stays held, which is what keeps an unknown effect from having its budget
    /// re-allocated before anyone knows what it spent.
    pub fn settle(
        &self,
        admission: &BudgetAdmission,
        settlement: &EffectSettlement,
    ) -> AdapterResult<SettlementOutcome> {
        let Some((effect_id, budget_id)) = settlement_identity(admission) else {
            return Ok(match admission {
                BudgetAdmission::NotConfigured { unconfigured } => {
                    SettlementOutcome::NotConfigured {
                        unconfigured: *unconfigured,
                    }
                }
                _ => SettlementOutcome::NotReserved {
                    effect_id: admission.effect_id().to_owned(),
                    absence: ReservationAbsent::NeverReserved,
                },
            });
        };
        let mut params = json!({
            "stateRoot": self.state_root(),
            "invocationId": effect_id,
            "budgetId": budget_id,
            "settlementStatus": settlement.ledger_status(),
        });
        if let Some(usage) = &settlement.usage {
            params["usage"] = json!({
                "promptTokens": usage.usage().prompt_tokens,
                "cachedInputTokens": usage.usage().cached_input_tokens,
                "completionTokens": usage.usage().completion_tokens,
                "totalTokens": usage.usage().total_tokens,
                "accuracy": usage.ledger_accuracy(),
            });
        }
        match workflow_ledger::settle_graph_command(&params) {
            Ok(answer) => decode_settlement(&answer),
            Err(error) => match error.code.as_str() {
                "usage_ledger_reservation_not_found" => Ok(SettlementOutcome::NotReserved {
                    effect_id,
                    absence: ReservationAbsent::NeverReserved,
                }),
                "usage_ledger_reservation_released" => Ok(SettlementOutcome::NotReserved {
                    effect_id,
                    absence: ReservationAbsent::AlreadyReleased,
                }),
                _ => Err(ledger_error(error)),
            },
        }
    }

    /// Give back a reservation for an effect that provably never started.
    ///
    /// Two things must both hold: the caller states [`EffectStartEvidence`] and
    /// the durable reservation must not already be in doubt. Either refusal
    /// leaves the amount held, so it is never re-allocated for an effect that
    /// might still report usage. Releasing twice is a no-op.
    pub fn release_orphan(&self, request: &OrphanReleaseRequest) -> AdapterResult<ReleaseOutcome> {
        if request.evidence == EffectStartEvidence::PossiblyStarted {
            return Ok(ReleaseOutcome::RefusedEarly {
                effect_id: request.effect_id.clone(),
                evidence: request.evidence,
            });
        }
        let mut params = json!({
            "stateRoot": self.state_root(),
            "invocationId": request.effect_id,
        });
        if let Some(budget_id) = &request.budget_id {
            params["budgetId"] = json!(budget_id);
        }
        match workflow_ledger::release_graph_command(&params) {
            Ok(answer) => decode_release(&answer, request.effect_id.clone()),
            Err(error) => match error.code.as_str() {
                // The durable state beats the caller's claim: an effect whose
                // outcome is unknown keeps its amount held.
                "usage_ledger_unknown_not_releasable" => Ok(ReleaseOutcome::RefusedEarly {
                    effect_id: request.effect_id.clone(),
                    evidence: EffectStartEvidence::PossiblyStarted,
                }),
                "usage_ledger_reservation_not_found" => Ok(ReleaseOutcome::NotReserved {
                    effect_id: request.effect_id.clone(),
                    absence: ReservationAbsent::NeverReserved,
                }),
                _ => Err(ledger_error(error)),
            },
        }
    }

    /// Read the numeric admission state for a boot-time reconciliation.
    ///
    /// A run with no pool answers [`FactState::NotConfigured`]: nothing is
    /// configured, so nothing is enforced and there is nothing to reconcile.
    pub fn reconcile(&self, run_id: Option<&str>) -> AdapterResult<ReconciliationReport> {
        let mut params = json!({ "stateRoot": self.state_root() });
        if let Some(run_id) = run_id {
            params["runId"] = json!(run_id);
        }
        let report = workflow_ledger::graph_admission_report(&params).map_err(ledger_error)?;
        let pools = report
            .get("pools")
            .and_then(Value::as_array)
            .map(|pools| pools.iter().map(decode_pool).collect::<Vec<_>>())
            .unwrap_or_default();
        let orphan_candidates = report
            .get("reservations")
            .and_then(Value::as_array)
            .map(|reservations| {
                reservations
                    .iter()
                    .filter(|reservation| {
                        reservation.get("state").and_then(Value::as_str) == Some("reserved")
                    })
                    .map(decode_candidate)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(ReconciliationReport {
            state: if pools.is_empty() {
                FactState::NotConfigured
            } else {
                FactState::Present
            },
            pools,
            in_flight_count: number(&report, "inFlightCount"),
            unknown_in_flight_count: number(&report, "unknownInFlightCount"),
            stops_new_dispatch: report
                .get("stopsNewDispatch")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            orphan_candidates,
        })
    }

    fn state_root(&self) -> String {
        self.portable_root.to_string_lossy().into_owned()
    }
}

/// The identity a settlement must be keyed by.
///
/// A reservation that was just admitted carries one; so does a concluded
/// reservation, so a caller that tries to settle it again reaches the ledger's
/// own idempotent or conflict answer instead of a fabricated absence.
fn settlement_identity(admission: &BudgetAdmission) -> Option<(String, String)> {
    match admission {
        BudgetAdmission::Reserved { reservation } => {
            Some((reservation.effect_id.clone(), reservation.budget_id.clone()))
        }
        BudgetAdmission::AlreadyConcluded {
            effect_id,
            budget_id,
            ..
        } => Some((effect_id.clone(), budget_id.clone())),
        _ => None,
    }
}

fn budget_value(budget: &BudgetConfiguration) -> Value {
    let mut value = json!({ "budgetId": budget.budget_id });
    if let Some(limit_tokens) = budget.limit_tokens {
        value["limitTokens"] = json!(limit_tokens);
    }
    if let Some(used_tokens) = budget.used_tokens {
        value["usedTokens"] = json!(used_tokens);
    }
    if let Some(remaining_tokens) = budget.remaining_tokens {
        value["remainingTokens"] = json!(remaining_tokens);
    }
    value
}

fn decode_admission(
    answer: &Value,
    request: &EffectBudgetRequest,
) -> AdapterResult<BudgetAdmission> {
    let budget_id = answer
        .get("budgetId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let available_tokens = answer
        .get("budget")
        .and_then(|budget| budget.get("availableTokens"))
        .and_then(Value::as_u64);
    let state_text = answer
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or_default();
    // A concluded reservation is reused by the ledger with `admitted: false`,
    // but it is not a budget denial: the effect id already ran (or was given
    // back), so nothing is held and no gate is invented.
    if matches!(state_text, "settled" | "released") {
        return Ok(BudgetAdmission::AlreadyConcluded {
            effect_id: request.effect_id.clone(),
            budget_id,
            state: ReservationState::parse(state_text)?,
        });
    }
    if answer.get("admitted").and_then(Value::as_bool) == Some(false) {
        let denial = match answer.get("code").and_then(Value::as_str) {
            Some("budget_exhausted") => BudgetDenial::Exhausted,
            _ => BudgetDenial::ExceedsRemaining,
        };
        let prompt = answer.get("prompt");
        return Ok(BudgetAdmission::Waiting {
            waiting: BudgetWaiting {
                effect_id: request.effect_id.clone(),
                budget_id,
                denial,
                available_tokens,
                estimate_tokens: request.estimate.map(|estimate| estimate.total_tokens),
                stops_new_dispatch: prompt
                    .and_then(|prompt| prompt.get("stopsNewDispatch"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                keeps_in_flight: prompt
                    .and_then(|prompt| prompt.get("keepsInFlight"))
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
            },
        });
    }
    if state_text == "free" {
        return Ok(BudgetAdmission::FreeRead {
            effect_id: request.effect_id.clone(),
            budget_id,
        });
    }
    let state = ReservationState::parse(state_text)?;
    Ok(BudgetAdmission::Reserved {
        reservation: ReservationIdentity {
            effect_id: answer
                .get("invocationId")
                .and_then(Value::as_str)
                .unwrap_or(&request.effect_id)
                .to_owned(),
            budget_id,
            state,
            reserved_tokens: answer
                .get("reservedTokens")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            estimate_tokens: answer
                .get("estimate")
                .and_then(|estimate| estimate.get("totalTokens"))
                .and_then(Value::as_u64),
            available_tokens,
            reused: answer
                .get("reused")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
    })
}

fn decode_settlement(answer: &Value) -> AdapterResult<SettlementOutcome> {
    let receipt = decode_receipt(answer)?;
    if receipt.reservation_state == ReservationState::UnknownHeld {
        return Ok(SettlementOutcome::UnknownHeld { receipt });
    }
    Ok(SettlementOutcome::Settled { receipt })
}

fn decode_receipt(answer: &Value) -> AdapterResult<SettlementReceipt> {
    Ok(SettlementReceipt {
        effect_id: answer
            .get("invocationId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        budget_id: answer
            .get("budgetId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        state: state_of(answer),
        reservation_state: ReservationState::parse(
            answer.get("state").and_then(Value::as_str).unwrap_or(""),
        )?,
        reserved_tokens: number(answer, "reservedTokens"),
        actual_tokens: answer
            .get("usage")
            .and_then(|usage| usage.get("totalTokens"))
            .and_then(Value::as_u64),
        overage_tokens: number(answer, "overageTokens"),
        late: answer.get("late").and_then(Value::as_bool).unwrap_or(false),
        available_tokens: answer
            .get("budget")
            .and_then(|budget| budget.get("availableTokens"))
            .and_then(Value::as_u64),
        settlement_status: answer
            .get("settlementStatus")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

fn decode_release(answer: &Value, effect_id: String) -> AdapterResult<ReleaseOutcome> {
    let receipt = decode_receipt(answer)?;
    let budget_id = receipt.budget_id.clone();
    match receipt.reservation_state {
        ReservationState::Released => Ok(ReleaseOutcome::Released { receipt }),
        ReservationState::Settled => Ok(ReleaseOutcome::AlreadySettled {
            effect_id,
            budget_id,
        }),
        // A release answered by a reservation that is neither released nor
        // settled contradicts the call, so it is reported instead of guessed at.
        _ => Err(AdapterError::refused(
            ADAPTER,
            "budget_release_state_unexpected",
            "inspect_the_recorded_reservation",
        )),
    }
}

fn decode_pool(pool: &Value) -> BudgetPoolFact {
    BudgetPoolFact {
        budget_id: pool
            .get("budgetId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        limit_tokens: pool.get("limitTokens").and_then(Value::as_u64),
        actual_tokens: number(pool, "actualTokens"),
        reserved_tokens: number(pool, "reservedTokens"),
        available_tokens: pool.get("availableTokens").and_then(Value::as_u64),
        status: BudgetPoolStatus::parse(
            pool.get("status")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        ),
    }
}

fn decode_candidate(reservation: &Value) -> OrphanCandidate {
    OrphanCandidate {
        effect_id: reservation
            .get("invocationId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        budget_id: reservation
            .get("budgetId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        run_id: reservation
            .get("runId")
            .and_then(Value::as_str)
            .map(str::to_owned),
        command_id: reservation
            .get("commandId")
            .and_then(Value::as_str)
            .map(str::to_owned),
        reserved_tokens: number(reservation, "reservedTokens"),
    }
}

/// The fact state a settlement or release answer describes.
fn state_of(answer: &Value) -> FactState {
    match answer.get("state").and_then(Value::as_str) {
        Some("unknown") => FactState::Unknown,
        _ => FactState::Present,
    }
}

fn number(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or_default()
}

fn ledger_error(error: workflow_ledger::LedgerError) -> AdapterError {
    AdapterError::from_ledger(ADAPTER, &error)
}
