//! Ports from the runtime's economic consumers to the existing fact owners (V7-EC1).
//!
//! Four independent adapters, one per concern, each with its own honest missing
//! states and its own typed errors:
//!
//! - [`observation`] persists what an effect actually spent — including FAILED,
//!   CANCELLED and UNKNOWN outcomes — and keeps the base metering facts apart
//!   from the optional statistics/research projection.
//! - [`budget`] reserves and settles keyed by effect id against a durable
//!   reservation, releases an orphaned reservation idempotently, and refuses to
//!   give budget back for an effect that may already have run.
//! - [`source`] says where a number came from and what that number is allowed to
//!   authorize. An imported usage marker authorizes no budget settlement, and
//!   that is stated in the type surface rather than in a comment.
//! - [`policy`] turns real history into a suggestion for *future* work only;
//!   adopting changes the future default, revoking restores it, and neither
//!   rewrites a binding that is already in flight.
//!
//! # Ownership
//!
//! Nothing here owns a fact. The durable ledgers stay with
//! [`licoup_statistics::ledger`] (Graph usage rows, budget
//! pools and reservations), the billing facts stay with
//! [`crate::domain::provider_model_pricing`], and the observation/strategy
//! seams stay with [`crate::domain::workflow_runtime::evolution`]. These
//! adapters call those owners through their own published API and translate the
//! JSON they return into typed values; they never write a second copy of a fact
//! an owner already keeps.
//!
//! # What is deliberately not claimed
//!
//! - A reservation and the workflow's started marker live in two different
//!   SQLite connections. No adapter claims those two writes are one
//!   transaction; the durable reservation exists so that the intermediate
//!   failure leaves a releasable orphan instead.
//! - An unreadable or unreported cost is [`FactState::Unknown`], never zero.
//! - A mechanism that runs correctly is not evidence of a statistically better
//!   outcome. [`policy::EvidenceClaim`] has no "measured improvement" variant,
//!   so no suggestion can carry one.

pub mod budget;
pub mod observation;
pub mod policy;
pub mod source;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

use licoup_statistics::ledger::LedgerError;

pub use budget::{
    BudgetAdapter, BudgetAdmission, BudgetConfiguration, BudgetDenial, BudgetNotConfigured,
    BudgetPoolFact, BudgetPoolStatus, BudgetWaiting, EffectBudgetRequest, EffectSettlement,
    EffectStartEvidence, OrphanCandidate, OrphanReleaseRequest, ReconciliationReport,
    ReleaseOutcome, ReservationAbsent, ReservationIdentity, ReservationState, SettlementOutcome,
    SettlementReceipt, TokenEstimate,
};
pub use observation::{
    CostUnknownReason, EffectKind, EffectMeteringFact, EffectObservation, EffectOutcome,
    MeteringFacts, MeteringTotals, ObservationAdapter, ObservationStatistics, ObservedCost,
    RecordReceipt, RunLifecycle, RunObservationIdentity,
};
pub use policy::{
    AdoptionReceipt, AuthorityContextState, DefaultApplication, EvidenceClaim,
    MINIMUM_HISTORY_SAMPLES, OptionHistory, OptionKey, PolicyAdapter, PolicyHistory, PolicyRequest,
    RevocationReceipt, SuggestionBasis, SuggestionOutcome,
};
pub use source::{
    CostProvenance, ImportedUsageMarker, NumberOrigin, ProvenanceUnknownReason, RecordedAccuracy,
    SettleableUsage, SourceAdapter, SourceErrorReason,
};

/// Which adapter produced a value or an error.
///
/// The four adapters report with one error shape so a caller can tell which
/// concern failed without parsing a message.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AdapterName {
    Observation,
    Budget,
    Source,
    Policy,
}

impl AdapterName {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Observation => "observation",
            Self::Budget => "budget",
            Self::Source => "source",
            Self::Policy => "policy",
        }
    }
}

/// A typed adapter error.
///
/// The owner's own code is preserved in `owner_code` so nothing is lost in
/// translation, while `code` stays a stable adapter-level vocabulary. A
/// `retryable` error may succeed on a later attempt; a non-retryable one needs
/// the request or the local store corrected first.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterError {
    pub adapter: AdapterName,
    pub code: &'static str,
    /// The code the owning ledger/source reported, when the failure came from
    /// one. `None` means the adapter itself refused the request.
    pub owner_code: Option<String>,
    pub retryable: bool,
    pub recovery: &'static str,
}

impl AdapterError {
    /// An adapter-level refusal that never reached an owner.
    pub const fn refused(adapter: AdapterName, code: &'static str, recovery: &'static str) -> Self {
        Self {
            adapter,
            code,
            owner_code: None,
            retryable: false,
            recovery,
        }
    }

    /// Translate one ledger error into the adapter vocabulary.
    ///
    /// The owner keeps publishing its own codes; this only names what the
    /// adapter's caller can act on.
    pub fn from_ledger(adapter: AdapterName, error: &LedgerError) -> Self {
        let (code, retryable, recovery) = match error.code.as_str() {
            "usage_ledger_store_unavailable" => (
                "usage_ledger_unavailable",
                true,
                "retry_after_store_recovers",
            ),
            "usage_ledger_schema_unsupported" => (
                "usage_ledger_schema_unsupported",
                false,
                "rebuild_local_usage_ledger",
            ),
            "usage_ledger_run_not_found" => (
                "observation_run_not_registered",
                false,
                "register_run_before_observing",
            ),
            "usage_ledger_reservation_not_found" => (
                "budget_reservation_absent",
                false,
                "reserve_before_settling",
            ),
            "usage_ledger_reservation_released" => (
                "budget_reservation_already_released",
                false,
                "inspect_reconciliation_before_reallocating",
            ),
            "usage_ledger_unknown_not_releasable" => (
                "budget_release_refused_unknown_effect",
                false,
                "settle_or_reconcile_the_unknown_effect",
            ),
            "usage_ledger_reservation_identity_conflict" => (
                "budget_reservation_identity_conflict",
                false,
                "reuse_the_original_effect_identity",
            ),
            "usage_ledger_settlement_conflict" => (
                "budget_settlement_conflict",
                false,
                "inspect_the_recorded_settlement",
            ),
            "usage_ledger_counter_overflow" => (
                "usage_counter_overflow",
                false,
                "correct_the_reported_counters",
            ),
            _ => (
                "usage_ledger_rejected",
                error.retryable,
                "correct_request_and_retry",
            ),
        };
        Self {
            adapter,
            code,
            owner_code: Some(error.code.clone()),
            retryable,
            recovery,
        }
    }
}

impl std::fmt::Display for AdapterError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.owner_code {
            Some(owner_code) => {
                write!(formatter, "{}/{owner_code}", self.code)
            }
            None => formatter.write_str(self.code),
        }
    }
}

impl std::error::Error for AdapterError {}

pub type AdapterResult<T> = std::result::Result<T, AdapterError>;

/// How much of a fact an adapter actually has.
///
/// This vocabulary exists so a caller cannot read "we have not looked" as "the
/// answer is zero". Every adapter answers an unavailable concern with one of
/// these states instead of a default-looking number.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FactState {
    /// A durable fact exists and is authoritative.
    Present,
    /// Nothing is configured for this concern. Nothing is enforced in its
    /// place: an unconfigured budget must not become an invented gate.
    NotConfigured,
    /// Nothing has been recorded for this subject yet. That is not a zero: no
    /// effect has been observed, so no spend of zero was observed either.
    NotRecorded,
    /// The owner could not be read. A later read may succeed.
    Unreadable,
    /// The fact is genuinely unknown — an in-doubt effect, a provider that
    /// reported no usage, a catalog with no route for this model.
    Unknown,
    /// The caller asked to give budget back for an effect that may already have
    /// run; the adapter refused rather than free the amount for reuse.
    RefusedEarly,
}

impl FactState {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::NotConfigured => "not-configured",
            Self::NotRecorded => "not-recorded",
            Self::Unreadable => "unreadable",
            Self::Unknown => "unknown",
            Self::RefusedEarly => "refused-early",
        }
    }
}

/// Checked token counters — the unit the usage ledger owns.
///
/// Every constructor keeps `total == prompt + completion` because that is the
/// ledger's own invariant for a recorded sample; a value that cannot satisfy it
/// is refused instead of being quietly adjusted.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostUsage {
    pub prompt_tokens: u64,
    pub cached_input_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

impl CostUsage {
    /// Build a checked usage sample.
    ///
    /// Returns `None` when the numbers cannot describe one sample: a cached
    /// count above the prompt count, or a total that is not the prompt plus the
    /// completion.
    pub fn checked(
        prompt_tokens: u64,
        cached_input_tokens: u64,
        completion_tokens: u64,
    ) -> Option<Self> {
        if cached_input_tokens > prompt_tokens {
            return None;
        }
        Some(Self {
            prompt_tokens,
            cached_input_tokens,
            completion_tokens,
            total_tokens: prompt_tokens.checked_add(completion_tokens)?,
        })
    }

    pub const fn is_empty(self) -> bool {
        self.total_tokens == 0
    }
}
