//! The remote work control entry: admission before effect, and local authority.
//!
//! [`RemoteWorkControl`] is the one place a remote control request is decided.
//! It holds three things apart on purpose — the local grants
//! ([`ControlGrants`]), the durable record of what was already answered
//! ([`RemoteControlLedger`]), and the kernel's own work owners
//! ([`LocalWorkOwner`]) — so that no one of them can answer for another.
//!
//! The order of the decisions is the behavior:
//!
//! 1. **Verified ingress first.** An unverified unit is refused before anything
//!    is admitted, so it leaves no record that a second delivery could reuse.
//! 2. **The current local grant second.** Verified ingress is necessary and not
//!    sufficient; an ungranted requester is refused before admission too.
//! 3. **The ownership the intent needs third.** Force control is admitted only
//!    for a target scope this host verifies as its own and only with the locally
//!    produced redacted diagnostics; an owned child must be an owned child of
//!    the parent the request named.
//! 4. **Admission fourth.** The request is recorded before the owner is asked,
//!    so a failure after this point is re-driven against the same identity
//!    instead of becoming a second attempt at the same work.
//! 5. **The owner's own answer last.** What the owner said, and exactly what it
//!    selected, is recorded; an owner that reaches outside the selected scope is
//!    visible instead of being recorded as a clean stop.

pub mod authority;
pub mod intent;
pub mod ledger;
pub mod owner;
pub mod replacement;
pub mod settlement;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;

use authority::{ControlAuthority, ControlGrants, VerifiedIngress};
use intent::{RedactedDiagnostics, RemoteWorkIntent, StopScope, WorkTarget};
use ledger::{Admission, ControlRequest, RemoteControlLedger, RequestId, RequestState};
use owner::{LocalWorkOwner, OwnerDisposition, OwnerReport, WorkObservation};

pub use authority::EndpointIdentity;
pub use ledger::{AdmittedControl, LedgerRefusal, RemoteControlRecord};
pub use replacement::{
    DispatchClaim, ReconciliationRecord, ResponsibilityBinding, ResponsibilityRefusal,
    ResponsibilityTransfer, ResponsibilityWriter,
};
pub use settlement::{
    AuthenticatedReceipt, DispatchState, ExecutionIdentity, ExecutionOwner, LocalAdmission,
    LocalIdentity, LocalObservation, ObservationRecord, REMOTE_SETTLEMENT_RECORD_SCHEMA,
    RemoteCursor, RemoteOutcomeState, RemoteSettlement, SettlementRecord, SettlementRefusal,
    TrackedExecution,
};

/// The largest selection one stop may resolve before it is refused.
///
/// The bound is a refusal, not a truncation: a stop that would silently drop
/// part of a subtree is worse than a stop that asks its caller to be precise.
pub const MAX_SELECTED_TARGETS: usize = 64;

/// Why a remote control was refused.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ControlRefusal {
    /// The request did not arrive through the exact verified unit path.
    UnverifiedIngress,
    /// The requester holds no current grant covering the intent.
    Ungranted {
        requester: String,
        required: ControlAuthority,
        held: ControlAuthority,
    },
    /// A force control arrived without the locally produced redacted
    /// diagnostics.
    DiagnosticsRequired { request: RequestId },
    /// The target is not work this host durably owns.
    TargetNotOwned { target: WorkTarget },
    /// The named child is not an owned child of the named parent.
    NotOwnedChild {
        parent: WorkTarget,
        child: WorkTarget,
    },
    /// The request identity is already held with different content.
    ConflictingRequest { request: RequestId },
    /// The selected subtree exceeded [`MAX_SELECTED_TARGETS`].
    SelectionTooLarge { request: RequestId, bound: usize },
    /// The owner reported work outside the scope the request selected.
    ScopeExceeded {
        request: RequestId,
        affected: Vec<WorkTarget>,
    },
    /// The owner could not be asked.
    OwnerUnavailable { request: RequestId, retryable: bool },
    /// A late result named a request this host never admitted.
    RequestNotAdmitted { request: RequestId },
}

impl ControlRefusal {
    /// The stable reason a caller publishes.
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        match self {
            Self::UnverifiedIngress => "endpoint_remote_control_unverified_ingress",
            Self::Ungranted { .. } => "endpoint_remote_control_ungranted",
            Self::DiagnosticsRequired { .. } => "endpoint_remote_control_diagnostics_required",
            Self::TargetNotOwned { .. } => "endpoint_remote_control_target_not_owned",
            Self::NotOwnedChild { .. } => "endpoint_remote_control_child_not_owned",
            Self::ConflictingRequest { .. } => "endpoint_remote_control_request_conflict",
            Self::SelectionTooLarge { .. } => "endpoint_remote_control_selection_too_large",
            Self::ScopeExceeded { .. } => "endpoint_remote_control_scope_exceeded",
            Self::OwnerUnavailable { .. } => "endpoint_remote_control_owner_unavailable",
            Self::RequestNotAdmitted { .. } => "endpoint_remote_control_request_not_admitted",
        }
    }
}

/// What one admitted request produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ControlOutcome {
    /// The identity was already answered. The recorded state is the answer, and
    /// nothing was asked of an owner again.
    AlreadyAnswered {
        request: RequestId,
        state: RequestState,
        affected: Vec<WorkTarget>,
    },
    /// An inspection's own answer.
    Observed {
        request: RequestId,
        state: RequestState,
        observation: WorkObservation,
    },
    /// The owner acknowledged a stop request. It is not proof the work ended;
    /// [`RequestState::Accepted`] says exactly that.
    Requested {
        request: RequestId,
        state: RequestState,
        affected: Vec<WorkTarget>,
    },
    /// The owner refused under its own policy.
    Refused { request: RequestId, reason: String },
    /// The effect may or may not have happened. The same identity is re-driven;
    /// this identity is never asked of an owner a second time as new work.
    Unknown { request: RequestId, retryable: bool },
}

impl ControlOutcome {
    #[must_use]
    pub const fn request(&self) -> &RequestId {
        match self {
            Self::AlreadyAnswered { request, .. }
            | Self::Observed { request, .. }
            | Self::Requested { request, .. }
            | Self::Refused { request, .. }
            | Self::Unknown { request, .. } => request,
        }
    }

    #[must_use]
    pub const fn state(&self) -> RequestState {
        match self {
            Self::AlreadyAnswered { state, .. }
            | Self::Observed { state, .. }
            | Self::Requested { state, .. } => *state,
            Self::Refused { .. } => RequestState::Refused,
            Self::Unknown { .. } => RequestState::Unknown,
        }
    }

    /// Exactly the work this outcome records as selected.
    #[must_use]
    pub fn affected(&self) -> &[WorkTarget] {
        match self {
            Self::AlreadyAnswered { affected, .. } | Self::Requested { affected, .. } => affected,
            Self::Observed { .. } | Self::Refused { .. } | Self::Unknown { .. } => &[],
        }
    }

    /// An inspection's observation, when this outcome carries one.
    #[must_use]
    pub const fn observation(&self) -> Option<&WorkObservation> {
        match self {
            Self::Observed { observation, .. } => Some(observation),
            _ => None,
        }
    }
}

/// One late, already-authenticated answer about an admitted request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LateResult {
    /// The end was observed after the fact.
    ObservedEnd,
    /// The end was still not observed. It stays unconfirmed.
    EndNotObserved { reason: String },
}

/// The remote work control entry over one set of local work owners.
#[derive(Clone, Debug)]
pub struct RemoteWorkControl<Owner> {
    grants: ControlGrants,
    ledger: RemoteControlLedger,
    owner: Owner,
}

impl<Owner> RemoteWorkControl<Owner> {
    /// A control entry with no grants: every requester starts ungranted.
    #[must_use]
    pub fn new(owner: Owner) -> Self {
        Self {
            grants: ControlGrants::new(),
            ledger: RemoteControlLedger::new(),
            owner,
        }
    }

    /// A control entry over a recovered ledger. The grants are not part of the
    /// record: a restart re-decides who may control work here.
    #[must_use]
    pub fn restored(owner: Owner, ledger: RemoteControlLedger) -> Self {
        Self {
            grants: ControlGrants::new(),
            ledger,
            owner,
        }
    }

    #[must_use]
    pub const fn grants(&self) -> &ControlGrants {
        &self.grants
    }

    #[must_use]
    pub const fn ledger(&self) -> &RemoteControlLedger {
        &self.ledger
    }

    #[must_use]
    pub const fn owner(&self) -> &Owner {
        &self.owner
    }

    #[must_use]
    pub const fn owner_mut(&mut self) -> &mut Owner {
        &mut self.owner
    }

    /// Grant one requester control here.
    pub fn grant(&mut self, requester: &EndpointIdentity, authority: ControlAuthority) {
        self.grants.grant(requester, authority);
    }

    /// Withdraw one requester's control here.
    pub fn revoke(&mut self, requester: &EndpointIdentity) -> bool {
        self.grants.revoke(requester)
    }

    /// The admitted requests this host has not observed an end for.
    ///
    /// They stay visible across a restart: an unknown effect is a fact this host
    /// owes its user, not a gap to be tidied away.
    #[must_use]
    pub fn unsettled(&self) -> Vec<&AdmittedControl> {
        self.ledger
            .admitted()
            .iter()
            .filter(|entry| !entry.state().confirms_end())
            .collect()
    }
}

impl<Owner: LocalWorkOwner> RemoteWorkControl<Owner> {
    /// Decide and perform one remote control request.
    pub fn handle(
        &mut self,
        request: ControlRequest,
        ingress: &VerifiedIngress,
    ) -> Result<ControlOutcome, ControlRefusal> {
        if !ingress.is_verified() {
            return Err(ControlRefusal::UnverifiedIngress);
        }
        let required = required_authority(request.intent());
        let held = self.grants.authority_of(ingress.requester());
        if !held.permits(required) {
            return Err(ControlRefusal::Ungranted {
                requester: ingress.requester().as_str().to_owned(),
                required,
                held,
            });
        }
        let id = request.id().clone();
        let intent = request.intent().clone();
        let diagnostics = request.diagnostics().cloned();
        if matches!(intent, RemoteWorkIntent::ForceStop { .. }) && diagnostics.is_none() {
            return Err(ControlRefusal::DiagnosticsRequired { request: id });
        }
        let selection = self.selection_of(&intent, &id)?;

        // Admission precedes every effect: from here on, a second delivery of
        // this identity is answered from the record.
        match self.ledger.admit(request) {
            Admission::Admitted => {}
            Admission::Replayed { state, affected } => {
                return Ok(ControlOutcome::AlreadyAnswered {
                    request: id,
                    state,
                    affected,
                });
            }
            Admission::Conflicting => {
                return Err(ControlRefusal::ConflictingRequest { request: id });
            }
        }

        match intent {
            RemoteWorkIntent::Inspect { target } => self.perform_inspect(id, target),
            RemoteWorkIntent::Stop { target } => {
                self.perform_stop(id, target, StopScope::Subtree, &selection)
            }
            RemoteWorkIntent::StopOwnedChild { child, .. } => {
                let scope = StopScope::OwnedChild(child.clone());
                self.perform_stop(id, child, scope, &selection)
            }
            RemoteWorkIntent::ForceStop { target } => {
                let diagnostics = diagnostics.expect("a force control carries diagnostics");
                self.perform_force(id, target, &diagnostics, &selection)
            }
        }
    }

    /// Record one late, already-authenticated answer about an admitted request.
    ///
    /// A late result never re-issues anything: it updates the record of the
    /// identity that already exists, and it cannot confirm an end this host did
    /// not admit.
    pub fn record_late_result(
        &mut self,
        request: &RequestId,
        result: LateResult,
    ) -> Result<ControlOutcome, ControlRefusal> {
        let state = match result {
            LateResult::ObservedEnd => RequestState::Confirmed,
            LateResult::EndNotObserved { reason: _ } => RequestState::Unconfirmed,
        };
        match self.ledger.record(request, state, Vec::new()) {
            Ok(()) => Ok(ControlOutcome::Requested {
                request: request.clone(),
                state,
                affected: self.affected_of(request),
            }),
            Err(LedgerRefusal::NotAdmitted) => Err(ControlRefusal::RequestNotAdmitted {
                request: request.clone(),
            }),
            Err(LedgerRefusal::IllegalTransition { .. }) => Ok(ControlOutcome::AlreadyAnswered {
                request: request.clone(),
                state: self
                    .ledger
                    .entry(request)
                    .map_or(RequestState::Unknown, |entry| entry.state()),
                affected: self.affected_of(request),
            }),
        }
    }

    fn affected_of(&self, request: &RequestId) -> Vec<WorkTarget> {
        self.ledger
            .entry(request)
            .map(|entry| entry.affected().to_vec())
            .unwrap_or_default()
    }

    /// The exact targets one intent selects, verified against this host's own
    /// ownership before anything is admitted.
    fn selection_of(
        &mut self,
        intent: &RemoteWorkIntent,
        id: &RequestId,
    ) -> Result<Vec<WorkTarget>, ControlRefusal> {
        match intent {
            RemoteWorkIntent::Inspect { .. } => Ok(Vec::new()),
            RemoteWorkIntent::ForceStop { target } => match self.owner.owns(target) {
                Ok(true) => Ok(vec![target.clone()]),
                Ok(false) => Err(ControlRefusal::TargetNotOwned {
                    target: target.clone(),
                }),
                Err(failure) => Err(ControlRefusal::OwnerUnavailable {
                    request: id.clone(),
                    retryable: failure.retryable(),
                }),
            },
            RemoteWorkIntent::StopOwnedChild { parent, child } => {
                match self.owner.observe(parent) {
                    Ok(observation) if observation.children().contains(child) => {
                        Ok(vec![child.clone()])
                    }
                    Ok(_) => Err(ControlRefusal::NotOwnedChild {
                        parent: parent.clone(),
                        child: child.clone(),
                    }),
                    Err(failure) => Err(ControlRefusal::OwnerUnavailable {
                        request: id.clone(),
                        retryable: failure.retryable(),
                    }),
                }
            }
            RemoteWorkIntent::Stop { target } => self.subtree(target, id),
        }
    }

    /// The target and every descendant this host durably owns for it.
    fn subtree(
        &mut self,
        target: &WorkTarget,
        id: &RequestId,
    ) -> Result<Vec<WorkTarget>, ControlRefusal> {
        let mut selected = vec![target.clone()];
        let mut visited = BTreeSet::new();
        visited.insert(target.clone());
        let mut frontier = vec![target.clone()];
        while let Some(current) = frontier.pop() {
            let children = match self.owner.observe(&current) {
                Ok(observation) => observation.children().to_vec(),
                Err(failure) => {
                    return Err(ControlRefusal::OwnerUnavailable {
                        request: id.clone(),
                        retryable: failure.retryable(),
                    });
                }
            };
            for child in children {
                if !visited.insert(child.clone()) {
                    continue;
                }
                selected.push(child.clone());
                if selected.len() > MAX_SELECTED_TARGETS {
                    return Err(ControlRefusal::SelectionTooLarge {
                        request: id.clone(),
                        bound: MAX_SELECTED_TARGETS,
                    });
                }
                frontier.push(child);
            }
        }
        selected.sort();
        Ok(selected)
    }

    fn perform_inspect(
        &mut self,
        id: RequestId,
        target: WorkTarget,
    ) -> Result<ControlOutcome, ControlRefusal> {
        match self.owner.observe(&target) {
            Ok(observation) => {
                let state = if observation.is_active() {
                    RequestState::Accepted
                } else {
                    RequestState::Confirmed
                };
                self.ledger
                    .record(&id, state, Vec::new())
                    .expect("the request was just admitted");
                Ok(ControlOutcome::Observed {
                    request: id,
                    state,
                    observation,
                })
            }
            Err(failure) => {
                let state = failure_state(failure);
                self.ledger
                    .record(&id, state, Vec::new())
                    .expect("the request was just admitted");
                Err(ControlRefusal::OwnerUnavailable {
                    request: id,
                    retryable: failure.retryable(),
                })
            }
        }
    }

    fn perform_stop(
        &mut self,
        id: RequestId,
        target: WorkTarget,
        scope: StopScope,
        selection: &[WorkTarget],
    ) -> Result<ControlOutcome, ControlRefusal> {
        match self.owner.request_stop(&target, scope) {
            Ok(report) => self.record_report(id, target, report, selection),
            Err(failure) => {
                let state = failure_state(failure);
                self.ledger
                    .record(&id, state, Vec::new())
                    .expect("the request was just admitted");
                Err(ControlRefusal::OwnerUnavailable {
                    request: id,
                    retryable: failure.retryable(),
                })
            }
        }
    }

    fn perform_force(
        &mut self,
        id: RequestId,
        target: WorkTarget,
        diagnostics: &RedactedDiagnostics,
        selection: &[WorkTarget],
    ) -> Result<ControlOutcome, ControlRefusal> {
        match self.owner.force_stop(&target, diagnostics) {
            Ok(report) => self.record_report(id, target, report, selection),
            Err(failure) => {
                let state = failure_state(failure);
                self.ledger
                    .record(&id, state, Vec::new())
                    .expect("the request was just admitted");
                Err(ControlRefusal::OwnerUnavailable {
                    request: id,
                    retryable: failure.retryable(),
                })
            }
        }
    }

    fn record_report(
        &mut self,
        id: RequestId,
        target: WorkTarget,
        report: OwnerReport,
        selection: &[WorkTarget],
    ) -> Result<ControlOutcome, ControlRefusal> {
        match report.disposition() {
            OwnerDisposition::Accepted => {
                let affected = report.affected().to_vec();
                if affected.iter().any(|item| !selection.contains(item)) {
                    // The effect may already have happened. It stays visible as
                    // unknown with exactly what the owner reported, and the
                    // caller is told the scope was exceeded instead of being
                    // handed a clean stop.
                    self.ledger
                        .record(&id, RequestState::Unknown, affected.clone())
                        .expect("the request was just admitted");
                    return Err(ControlRefusal::ScopeExceeded {
                        request: id,
                        affected,
                    });
                }
                self.ledger
                    .record(&id, RequestState::Accepted, affected.clone())
                    .expect("the request was just admitted");
                Ok(ControlOutcome::Requested {
                    request: id,
                    state: RequestState::Accepted,
                    affected,
                })
            }
            OwnerDisposition::NotOwned => {
                self.ledger
                    .record(&id, RequestState::Refused, Vec::new())
                    .expect("the request was just admitted");
                Err(ControlRefusal::TargetNotOwned { target })
            }
            OwnerDisposition::Refused { reason } => {
                self.ledger
                    .record(&id, RequestState::Refused, Vec::new())
                    .expect("the request was just admitted");
                Ok(ControlOutcome::Refused {
                    request: id,
                    reason: reason.clone(),
                })
            }
        }
    }
}

/// The authority one intent requires at least.
#[must_use]
pub fn required_authority(intent: &RemoteWorkIntent) -> ControlAuthority {
    match intent {
        RemoteWorkIntent::Inspect { .. } => ControlAuthority::Inspect,
        RemoteWorkIntent::Stop { .. } | RemoteWorkIntent::StopOwnedChild { .. } => {
            ControlAuthority::Stop
        }
        RemoteWorkIntent::ForceStop { .. } => ControlAuthority::ForceStop,
    }
}

/// The state one owner failure leaves behind.
///
/// A definite failure means the request did not happen, so it is recorded as
/// refused and stays refused; an uncertain one may have happened, so it stays
/// visible as unknown. Neither is ever a confirmed end.
fn failure_state(failure: owner::OwnerFailure) -> RequestState {
    if failure.retryable() {
        RequestState::Unknown
    } else {
        RequestState::Refused
    }
}
