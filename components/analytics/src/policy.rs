//! Who may settle, and who may only be shown.
//!
//! C11 separates the *quality* a producer reports from the *authority* the host
//! grants: "a producer that says exact does not thereby gain budget authority".
//! This module is that separation. [`AuthorityPolicy`] is a host-configured
//! grant table — which sources are admitted at all, and which of those may
//! record a settlement — and nothing in it reads a payload's quality. A source
//! that is admitted without a settlement grant is displayed with its provenance
//! and can never change a budget, however confidently it describes itself.
//!
//! Two facts are deliberately independent:
//!
//! - **Admission** is about the transport: is this the source the host bound,
//!   and did the host configure it at all? An unknown source is refused with an
//!   actionable recovery, because "this source is not installed or not enabled"
//!   is something the user can act on.
//! - **Eligibility** is about policy: may this source's facts settle? An
//!   admitted source without the grant is a legal, display-only configuration,
//!   not an error.

use licoup_extension_contracts::{ApplicationFailure, RecoveryAction};
use std::collections::BTreeMap;

use crate::facts::SettlementEligibility;
use crate::refusal_with;

/// One source the host has configured, and whether its facts may settle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceGrant {
    pub source_ref: String,
    /// `true` only when the host has decided this source may settle. It is never
    /// derived from what the source reports about itself.
    pub settlement: bool,
}

impl SourceGrant {
    pub fn new(source_ref: impl Into<String>, settlement: bool) -> Self {
        Self {
            source_ref: source_ref.into(),
            settlement,
        }
    }
}

/// The host's source policy.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AuthorityPolicy {
    grants: BTreeMap<String, bool>,
}

impl AuthorityPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build the policy from explicit host grants.
    pub fn host_configured(grants: impl IntoIterator<Item = SourceGrant>) -> Self {
        let mut policy = Self::new();
        for grant in grants {
            policy.grant(grant.source_ref, grant.settlement);
        }
        policy
    }

    pub fn grant(&mut self, source_ref: impl Into<String>, settlement: bool) {
        self.grants.insert(source_ref.into(), settlement);
    }

    pub fn admits(&self, source_ref: &str) -> bool {
        self.grants.contains_key(source_ref)
    }

    /// What this source's facts may do. An unconfigured or unprivileged source
    /// is display material.
    pub fn eligibility(&self, source_ref: &str) -> SettlementEligibility {
        match self.grants.get(source_ref) {
            Some(true) => SettlementEligibility::SettlementEligible,
            _ => SettlementEligibility::DisplayOnly,
        }
    }

    /// The refusal for an operation that needs a configured source.
    ///
    /// The recovery is actionable — install, enable or configure the source —
    /// rather than "invalid request", because a source the user has not
    /// configured is a fact about this installation.
    pub fn refusal(&self, source_ref: &str) -> ApplicationFailure {
        refusal_with(
            "analytics_source_not_admitted",
            RecoveryAction::InstallOrRetryRuntime,
        )
        .with_field("sourceRef")
        .with_presentation_arg("sourceRef", source_ref)
    }

    pub fn granted_sources(&self) -> impl Iterator<Item = (&str, bool)> {
        self.grants
            .iter()
            .map(|(source, settlement)| (source.as_str(), *settlement))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reporting_source_gains_nothing_by_saying_so() {
        let policy = AuthorityPolicy::host_configured([
            SourceGrant::new("source:agent#1", false),
            SourceGrant::new("source:gateway#1", true),
        ]);
        assert!(policy.admits("source:agent#1"));
        assert_eq!(
            policy.eligibility("source:agent#1"),
            SettlementEligibility::DisplayOnly,
            "admission is not authority"
        );
        assert_eq!(
            policy.eligibility("source:gateway#1"),
            SettlementEligibility::SettlementEligible
        );
    }

    #[test]
    fn an_unconfigured_source_is_refused_actionably_and_is_never_eligible() {
        let policy = AuthorityPolicy::new();
        assert!(!policy.admits("source:unknown#1"));
        assert_eq!(
            policy.eligibility("source:unknown#1"),
            SettlementEligibility::DisplayOnly
        );
        let failure = policy.refusal("source:unknown#1");
        assert_eq!(failure.code, "analytics_source_not_admitted");
        assert_eq!(failure.recovery, RecoveryAction::InstallOrRetryRuntime);
        assert_eq!(
            failure.presentation_args.get("sourceRef"),
            Some("source:unknown#1")
        );
    }

    #[test]
    fn the_policy_has_no_way_to_read_a_payload_quality() {
        // The API takes a source identity and nothing else: there is no path by
        // which a producer's claim about itself reaches the grant table.
        let mut policy = AuthorityPolicy::new();
        policy.grant("source:agent#1", false);
        assert_eq!(policy.granted_sources().count(), 1);
        assert!(!policy.eligibility("source:agent#1").is_settlement());
    }
}
