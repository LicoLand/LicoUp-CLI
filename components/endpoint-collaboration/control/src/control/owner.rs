//! The consumer-owned port the kernel's own work owners implement.
//!
//! The kernel already owns every effect a remote control can ask for: the
//! persistent conversation turn, the durable workflow run, the Subagent MCP
//! dispatch claim and the supervised lane session each resolve their own durable
//! owner, and the kernel's force control terminates only a process group whose
//! durable ownership record it re-verifies at execution time. This slice
//! therefore declares *what it needs to ask* and never performs an effect
//! itself, which is what keeps remote business semantics out of the kernel's
//! composition and the kernel's owners out of this package.
//!
//! Two answers are deliberately kept apart:
//!
//! * [`OwnerDisposition`] is what the owner *said*. Accepting a stop is not the
//!   same fact as the work having ended, and this port has no method that would
//!   let a caller claim the second from the first.
//! * [`OwnerReport::affected`] is exactly what the owner selected. The composer
//!   compares it against the scope the request selected, so an owner that
//!   reaches past its own subtree is visible instead of being recorded as a
//!   clean stop.

use super::intent::{RedactedDiagnostics, StopScope, WorkTarget};

/// Why one owner could not answer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerFailure {
    /// The owner could not be asked. No effect happened.
    Unavailable,
    /// The owner may or may not have performed the effect.
    Uncertain,
}

impl OwnerFailure {
    /// Whether the caller must re-drive the same already-admitted request.
    ///
    /// [`Self::Uncertain`] does: the effect may have happened, and the ledger
    /// answers a re-drive from the recorded admission instead of asking the
    /// owner a second time.
    #[must_use]
    pub const fn retryable(self) -> bool {
        matches!(self, Self::Uncertain)
    }
}

/// What one owner observed about one target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkObservation {
    target: WorkTarget,
    active: bool,
    children: Vec<WorkTarget>,
}

impl WorkObservation {
    #[must_use]
    pub fn new(target: WorkTarget, active: bool, mut children: Vec<WorkTarget>) -> Self {
        children.sort();
        children.dedup();
        Self {
            target,
            active,
            children,
        }
    }

    #[must_use]
    pub const fn target(&self) -> &WorkTarget {
        &self.target
    }

    /// Whether the owner observed the work still running. Only `false` is an
    /// observation of an end.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// The descendants the owner durably owns for this target.
    #[must_use]
    pub fn children(&self) -> &[WorkTarget] {
        &self.children
    }
}

/// What one owner did with one request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OwnerDisposition {
    /// The owner accepted the request. It is not proof the work ended.
    Accepted,
    /// The owner does not own this target, so there was nothing to stop.
    NotOwned,
    /// The owner refused under its own policy. The reason is the owner's own
    /// stable code, not peer-supplied text.
    Refused { reason: String },
}

/// One owner's answer: its disposition and exactly what it selected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnerReport {
    disposition: OwnerDisposition,
    affected: Vec<WorkTarget>,
}

impl OwnerReport {
    #[must_use]
    pub fn new(disposition: OwnerDisposition, mut affected: Vec<WorkTarget>) -> Self {
        affected.sort();
        affected.dedup();
        Self {
            disposition,
            affected,
        }
    }

    /// An accepted request that selected exactly the given targets.
    #[must_use]
    pub fn accepted(affected: Vec<WorkTarget>) -> Self {
        Self::new(OwnerDisposition::Accepted, affected)
    }

    /// A request for a target this owner does not own.
    #[must_use]
    pub fn not_owned() -> Self {
        Self::new(OwnerDisposition::NotOwned, Vec::new())
    }

    #[must_use]
    pub const fn disposition(&self) -> &OwnerDisposition {
        &self.disposition
    }

    #[must_use]
    pub fn affected(&self) -> &[WorkTarget] {
        &self.affected
    }
}

/// The kernel's work owners, as this slice needs them.
///
/// Every method is a question about work this host owns. None of them
/// authenticates a peer, reads protected material or accepts a caller's claim
/// about ownership: the implementation resolves its own durable records.
pub trait LocalWorkOwner {
    /// Whether this host durably owns the work named by `target`.
    fn owns(&self, target: &WorkTarget) -> Result<bool, OwnerFailure>;

    /// Read one target's current state and its durably owned descendants.
    fn observe(&mut self, target: &WorkTarget) -> Result<WorkObservation, OwnerFailure>;

    /// Ask one owner to stop the selected work.
    ///
    /// The owner resolves the target against its own durable records, stops
    /// exactly the selected subtree or the named owned child, and reports what
    /// it selected. It must never widen to a shared, external or unrelated
    /// process.
    fn request_stop(
        &mut self,
        target: &WorkTarget,
        scope: StopScope,
    ) -> Result<OwnerReport, OwnerFailure>;

    /// Ask the owner's own force control to terminate one verified owned target
    /// scope, carrying the locally produced redacted diagnostics.
    fn force_stop(
        &mut self,
        target: &WorkTarget,
        diagnostics: &RedactedDiagnostics,
    ) -> Result<OwnerReport, OwnerFailure>;
}

#[cfg(test)]
mod tests {
    use super::{OwnerDisposition, OwnerFailure, OwnerReport, WorkObservation};
    use crate::control::intent::{WorkOwner, WorkTarget};

    fn target(scope: &str) -> WorkTarget {
        WorkTarget::new(WorkOwner::WorkflowRun, scope)
    }

    #[test]
    fn only_an_uncertain_owner_failure_is_retryable() {
        assert!(!OwnerFailure::Unavailable.retryable());
        assert!(OwnerFailure::Uncertain.retryable());
    }

    #[test]
    fn an_observation_carries_the_target_and_its_owned_children_deduplicated() {
        let observation = WorkObservation::new(
            target("run-1"),
            true,
            vec![target("run-2"), target("run-1"), target("run-2")],
        );

        assert_eq!(observation.target(), &target("run-1"));
        assert!(observation.is_active());
        assert_eq!(observation.children(), &[target("run-1"), target("run-2")]);
    }

    #[test]
    fn accepting_a_request_is_not_an_observation_of_an_end() {
        let accepted = OwnerReport::accepted(vec![target("run-1"), target("run-1")]);
        assert_eq!(accepted.disposition(), &OwnerDisposition::Accepted);
        assert_eq!(accepted.affected(), &[target("run-1")]);

        let not_owned = OwnerReport::not_owned();
        assert_eq!(not_owned.disposition(), &OwnerDisposition::NotOwned);
        assert!(not_owned.affected().is_empty());
    }
}
