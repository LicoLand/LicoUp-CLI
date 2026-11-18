//! The core fact port: the narrow slice of M33 this package consumes.
//!
//! The minimal metering ledger, its pending obligations and its settlement stay
//! with the core. This package borrows them through one trait, so three things
//! are true by construction:
//!
//! - **There is no second ledger.** The index keeps observations for
//!   deduplication, correction and display; the only durable facts are the ones
//!   recorded here, and they are the core's.
//! - **Uninstalling the package cannot delete a fact.** The port is borrowed,
//!   not owned: dropping every surface of this package leaves the facts exactly
//!   where they were.
//! - **A correction replaces.** [`CoreUsageFacts::record`] is keyed by
//!   `fact_id`, so a corrected reading updates its own fact and a replay is a
//!   no-op rather than an addition.
//!
//! A fact carries the exact decimal text and the producer's [`Quality`]. The
//! quality is the producer's claim about how it got the number; it is not
//! settlement authority, which is why [`SettlementEligibility`] is a separate
//! field the host's policy fills in.

use licoup_extension_contracts::usage::{ExactNumber, Quality};
use licoup_extension_contracts::{ApplicationFailure, is_namespaced};

use crate::refusal;

/// Whether a fact may take part in settlement, or is display material only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettlementEligibility {
    /// Shown with its provenance; never recorded as a settlement.
    DisplayOnly,
    /// The host's source policy allows this fact to settle.
    SettlementEligible,
}

impl SettlementEligibility {
    pub const fn id(self) -> &'static str {
        match self {
            Self::DisplayOnly => "display-only",
            Self::SettlementEligible => "settlement-eligible",
        }
    }

    pub const fn is_settlement(self) -> bool {
        matches!(self, Self::SettlementEligible)
    }
}

/// One metering fact the core records.
///
/// It is deliberately small: an identity, the scope it belongs to, one metric,
/// an exact decimal or an explicit unknown, and the two facts about its origin
/// (which source, and whether the host lets it settle).
#[derive(Clone, Debug, PartialEq)]
pub struct MeteringFact {
    /// Stable identity. Recording the same id again is a correction or a no-op,
    /// never a second charge.
    pub fact_id: String,
    /// The invocation or authorized aggregate range this fact belongs to.
    pub scope_ref: String,
    pub metric: String,
    /// Exact decimal text, or `None` when the value is unknown.
    pub value: Option<String>,
    pub unit: String,
    pub quality: Quality,
    pub eligibility: SettlementEligibility,
    pub source_ref: String,
    pub observed_at: String,
}

impl MeteringFact {
    /// Structural validation: a namespaced metric, a unit, and a value present
    /// exactly when the quality claims one.
    pub fn validate(&self) -> Result<(), ApplicationFailure> {
        if self.fact_id.is_empty() {
            return Err(refusal("analytics_fact_invalid").with_field("factId"));
        }
        if self.scope_ref.is_empty() {
            return Err(refusal("analytics_fact_invalid").with_field("scopeRef"));
        }
        if !is_namespaced(&self.metric) || self.unit.is_empty() {
            return Err(refusal("analytics_fact_invalid").with_field("metric"));
        }
        if self.quality.has_value() != self.value.is_some() {
            return Err(refusal("analytics_fact_invalid").with_field("value"));
        }
        if let Some(value) = &self.value
            && ExactNumber::parse(value).is_none()
        {
            return Err(refusal("analytics_fact_invalid").with_field("value"));
        }
        if self.source_ref.is_empty() || self.observed_at.is_empty() {
            return Err(refusal("analytics_fact_invalid").with_field("sourceRef"));
        }
        Ok(())
    }

    pub fn exact(&self) -> Option<ExactNumber> {
        self.value.as_deref().and_then(ExactNumber::parse)
    }
}

/// What recording a fact did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FactOutcome {
    /// A new fact.
    Inserted,
    /// The same identity with a corrected value.
    Updated,
    /// The same identity with the same value: a replay.
    Unchanged,
    /// The fact was withdrawn by a retraction.
    Withdrawn,
    /// A retraction named an identity the ledger does not hold.
    UnknownFact,
}

/// The result of recording or retracting one fact.
#[derive(Clone, Debug, PartialEq)]
pub struct FactReceipt {
    pub fact_id: String,
    pub outcome: FactOutcome,
}

impl FactReceipt {
    pub fn recorded(&self) -> bool {
        matches!(
            self.outcome,
            FactOutcome::Inserted | FactOutcome::Updated | FactOutcome::Unchanged
        )
    }
}

/// An obligation the core still holds unsettled for a scope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingObligation {
    pub scope_ref: String,
    pub metric: String,
    /// Why it is still pending, in a form the client can show.
    pub reason: String,
}

/// One page of recorded facts.
#[derive(Clone, Debug, PartialEq)]
pub struct FactPage {
    pub facts: Vec<MeteringFact>,
    pub next_cursor: Option<String>,
}

impl FactPage {
    pub fn empty() -> Self {
        Self {
            facts: Vec::new(),
            next_cursor: None,
        }
    }
}

/// The core slice this package consumes.
///
/// The trait is intentionally read-mostly: the package may record an admitted
/// fact and retract one it recorded, and it may read the facts and the pending
/// obligations. It cannot rewrite a fact it does not own, and it cannot compute
/// a settlement: that stays in the core, which is what "the trusted account and
/// the display declaration are separate" means in code.
pub trait CoreUsageFacts {
    /// Record one fact, keyed by `fact_id`.
    fn record(&mut self, fact: MeteringFact) -> FactReceipt;
    /// Withdraw one fact by identity. It cancels that fact and nothing else.
    fn retract(&mut self, fact_id: &str) -> FactReceipt;
    /// Read facts, by cursor, for display.
    fn read(&self, cursor: Option<&str>, limit: usize) -> FactPage;
    /// The obligations the core still holds.
    fn pending(&self) -> Vec<PendingObligation>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact() -> MeteringFact {
        MeteringFact {
            fact_id: "scope-1#m#licoup.tokens.input".to_owned(),
            scope_ref: "scope-1".to_owned(),
            metric: "licoup.tokens.input".to_owned(),
            value: Some("120".to_owned()),
            unit: "tokens".to_owned(),
            quality: Quality::Reported,
            eligibility: SettlementEligibility::SettlementEligible,
            source_ref: "source:example.agent#1".to_owned(),
            observed_at: "2026-09-21T00:00:00Z".to_owned(),
        }
    }

    #[test]
    fn a_fact_with_a_value_must_be_exact_and_a_claim_must_carry_one() {
        assert!(fact().validate().is_ok());
        assert_eq!(fact().exact().expect("exact").to_canonical_string(), "120");

        let mut float = fact();
        float.value = Some("1.5e3".to_owned());
        assert!(float.validate().is_err());

        let mut claimed_zero = fact();
        claimed_zero.value = None;
        claimed_zero.quality = Quality::Reported;
        assert_eq!(
            claimed_zero
                .validate()
                .expect_err("no value")
                .field
                .as_deref(),
            Some("value")
        );

        let mut unknown = fact();
        unknown.value = None;
        unknown.quality = Quality::Unknown;
        assert!(unknown.validate().is_ok());
    }

    #[test]
    fn eligibility_is_separate_from_quality() {
        let mut display = fact();
        display.quality = Quality::Reported;
        display.eligibility = SettlementEligibility::DisplayOnly;
        assert!(!display.eligibility.is_settlement());
        assert_eq!(
            display.eligibility.id(),
            "display-only",
            "saying reported grants nothing"
        );
    }
}
