//! Who may invoke one effect after a replacement, and where a late result goes.
//!
//! [`super::RemoteSettlement`] already keeps a peer-owned execution apart from
//! work this host performs, and a compatible local update already preserves what
//! this host owes. What it did not have is the three facts a device replacement
//! turns on:
//!
//! * **Which local endpoint issued one effect.** A replacement changes the local
//!   endpoint identity, and `apply_local_update` refuses an incompatible
//!   identity for good reason — it must not abandon remote work on its own. The
//!   transfer below is the authorized operation that does move responsibility,
//!   and it moves it explicitly rather than by relaxing that check.
//! * **That an effect was already issued.** A replacement must never issue a
//!   second one. [`super::DispatchState`] records the issue, so settling an
//!   in-flight effect needs the peer's own authenticated receipt and nothing
//!   else.
//! * **Where a late result belongs.** A result for an effect the *source* device
//!   issued is reconciled idempotently by the destination and never re-dispatched.
//!   Its provenance stays the source endpoint, so the reconciliation is visibly
//!   about work this device did not start.
//!
//! The order of the decisions is the behavior:
//!
//! 1. **The binding first.** A transfer names the subject, the source device and
//!    the destination identity, and it is refused unless the source device is
//!    this host's own current local endpoint. A transfer for another subject or
//!    another source is refused rather than applied to whoever asked.
//! 2. **The destination second.** The destination identity must be the one the
//!    caller is really moving to; a transfer that names this host's own current
//!    identity changes nothing and is refused.
//! 3. **The transfer third.** Identities, cursors, unknown-effect records, the
//!    dispatch record and every original provenance are carried through
//!    unchanged. Nothing is re-derived from the new identity.
//! 4. **Dispatch last, and only through the claim.** After a transfer the
//!    destination holds an unresolved obligation, not a fresh effect: it may
//!    reconcile a late result and it may not invoke the effect again.

use super::authority::EndpointIdentity;
use super::settlement::{
    AuthenticatedReceipt, DispatchState, ExecutionIdentity, LocalIdentity, RemoteOutcomeState,
    RemoteSettlement, SettlementRefusal, TrackedExecution,
};

/// The schema of [`super::SettlementRecord`] once it carries dispatch ownership.
pub use super::settlement::REMOTE_SETTLEMENT_RECORD_SCHEMA;
/// The attribution one tracked effect carries, re-exported beside the operations
/// that move it.
pub use super::settlement::ResponsibilityWriter;

/// Which subject, source device and destination identity one transfer is about.
///
/// It is this slice's own three-part binding. The transfer component states the
/// same three facts for the evidence a staged target produces
/// (`TargetBinding`); the composition root that holds both projects one into the
/// other, so neither slice has to depend on the other's internals.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResponsibilityBinding {
    subject: String,
    source_device: String,
    destination_identity: String,
}

impl ResponsibilityBinding {
    #[must_use]
    pub fn new(
        subject: impl Into<String>,
        source_device: impl Into<String>,
        destination_identity: impl Into<String>,
    ) -> Self {
        Self {
            subject: subject.into(),
            source_device: source_device.into(),
            destination_identity: destination_identity.into(),
        }
    }

    #[must_use]
    pub fn subject(&self) -> &str {
        &self.subject
    }

    #[must_use]
    pub fn source_device(&self) -> &str {
        &self.source_device
    }

    #[must_use]
    pub fn destination_identity(&self) -> &str {
        &self.destination_identity
    }
}

/// Why one responsibility transfer or dispatch was refused.
///
/// Every variant is a bounded, non-secret classification: it names a class of
/// refusal and, at most, the identities and cursors a caller already holds. No
/// variant carries key material, a signature, a credential or a payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResponsibilityRefusal {
    /// The transfer names a source device that is not this host's current local
    /// endpoint, so it is about some other source.
    SourceIsNotLocal { source: String, local: String },
    /// The transfer names this host's own current identity as its destination,
    /// so it changes nothing.
    DestinationIsLocal { destination: String },
    /// One effect may be issued once. This execution was already dispatched.
    AlreadyDispatched { identity: ExecutionIdentity },
    /// The caller is not the endpoint that may invoke this effect.
    NotActiveWriter {
        identity: ExecutionIdentity,
        writer: ResponsibilityWriter,
    },
    /// A result may only be reconciled for an effect that was really issued
    /// here. An undispatched execution has no effect to settle.
    NothingToReconcile { identity: ExecutionIdentity },
    /// The receipt moves the recorded state backwards or sideways in a way the
    /// settlement already refuses. Carried through so one classification covers
    /// reconciliation.
    ReconciliationRefused { reason: SettlementRefusal },
    /// The execution is not tracked here.
    NotTracked { identity: ExecutionIdentity },
}

impl ResponsibilityRefusal {
    /// The stable, non-secret reason a caller publishes.
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        match self {
            Self::SourceIsNotLocal { .. } => "endpoint_responsibility_source_is_not_local",
            Self::DestinationIsLocal { .. } => "endpoint_responsibility_destination_is_local",
            Self::AlreadyDispatched { .. } => "endpoint_responsibility_already_dispatched",
            Self::NotActiveWriter { .. } => "endpoint_responsibility_not_active_writer",
            Self::NothingToReconcile { .. } => "endpoint_responsibility_nothing_to_reconcile",
            Self::ReconciliationRefused { .. } => "endpoint_responsibility_reconciliation_refused",
            Self::NotTracked { .. } => "endpoint_responsibility_not_tracked",
        }
    }
}

/// What one authorized transfer left behind.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResponsibilityTransfer {
    binding: ResponsibilityBinding,
    previous: LocalIdentity,
    current: LocalIdentity,
    tracked: usize,
    in_flight: usize,
    unresolved: usize,
}

impl ResponsibilityTransfer {
    /// The binding the transfer was authorized for.
    #[must_use]
    pub const fn binding(&self) -> &ResponsibilityBinding {
        &self.binding
    }

    /// The local identity responsibility moved away from.
    #[must_use]
    pub const fn previous(&self) -> &LocalIdentity {
        &self.previous
    }

    /// The local identity responsibility moved to.
    #[must_use]
    pub const fn current(&self) -> &LocalIdentity {
        &self.current
    }

    /// How many executions are still tracked. None was dropped.
    #[must_use]
    pub const fn tracked(&self) -> usize {
        self.tracked
    }

    /// How many issued effects are still unsettled.
    #[must_use]
    pub const fn in_flight(&self) -> usize {
        self.in_flight
    }

    /// How many tracked effects carry no known original issuer.
    #[must_use]
    pub const fn unresolved(&self) -> usize {
        self.unresolved
    }
}

/// One authorized issue of an effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DispatchClaim {
    identity: ExecutionIdentity,
    dispatcher: EndpointIdentity,
}

impl DispatchClaim {
    #[must_use]
    pub const fn identity(&self) -> &ExecutionIdentity {
        &self.identity
    }

    /// The endpoint that issued the effect, which is the only one that may
    /// reconcile its result.
    #[must_use]
    pub const fn dispatcher(&self) -> &EndpointIdentity {
        &self.dispatcher
    }
}

/// What one reconciliation of a late result left behind.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconciliationRecord {
    identity: ExecutionIdentity,
    state: RemoteOutcomeState,
    cursor: super::settlement::RemoteCursor,
    originated_with: Option<EndpointIdentity>,
    reconciled_by: EndpointIdentity,
}

impl ReconciliationRecord {
    #[must_use]
    pub const fn identity(&self) -> &ExecutionIdentity {
        &self.identity
    }

    #[must_use]
    pub const fn state(&self) -> RemoteOutcomeState {
        self.state
    }

    #[must_use]
    pub const fn cursor(&self) -> super::settlement::RemoteCursor {
        self.cursor
    }

    /// The endpoint that originally issued the effect, when this host knows it.
    ///
    /// It is the provenance the late result is filed under. A transfer never
    /// rewrites it, so a result for the source device's work stays visibly the
    /// source device's work.
    #[must_use]
    pub const fn originated_with(&self) -> Option<&EndpointIdentity> {
        self.originated_with.as_ref()
    }

    /// The endpoint that reconciled the result here.
    #[must_use]
    pub const fn reconciled_by(&self) -> &EndpointIdentity {
        &self.reconciled_by
    }

    /// Whether this reconciliation was about work the reconciling device did not
    /// issue.
    #[must_use]
    pub fn is_foreign_effect(&self) -> bool {
        self.originated_with
            .as_ref()
            .is_none_or(|origin| origin != &self.reconciled_by)
    }
}

impl RemoteSettlement {
    /// Move responsibility for every tracked effect to an authorized destination.
    ///
    /// The transfer is the *only* operation that changes this host's local
    /// identity to a different endpoint. `apply_local_update` deliberately refuses
    /// that, because abandoning remote work belongs to the protocol owner; this
    /// operation therefore replaces nothing and preserves everything:
    ///
    /// * every tracked identity, cursor, recorded state and unknown-effect record
    ///   stays exactly as it was;
    /// * every effect that was already issued keeps its issue, so the destination
    ///   can never invoke it a second time;
    /// * every original issuer stays attributed, and an effect whose issuer this
    ///   host never knew stays unresolved rather than being assigned to the new
    ///   identity as if this device had started it.
    ///
    /// It records a cleanup intent nowhere and performs no effect: retiring or
    /// erasing the source is a separately authorized flow.
    pub fn transfer_responsibility(
        &mut self,
        binding: ResponsibilityBinding,
        destination: LocalIdentity,
    ) -> Result<ResponsibilityTransfer, ResponsibilityRefusal> {
        let local = self.local_identity().endpoint().clone();
        if binding.source_device != local.as_str() {
            return Err(ResponsibilityRefusal::SourceIsNotLocal {
                source: binding.source_device.clone(),
                local: local.as_str().to_owned(),
            });
        }
        if destination.endpoint() == &local {
            return Err(ResponsibilityRefusal::DestinationIsLocal {
                destination: local.as_str().to_owned(),
            });
        }
        if destination.endpoint().as_str() != binding.destination_identity {
            return Err(ResponsibilityRefusal::SourceIsNotLocal {
                source: binding.source_device.clone(),
                local: local.as_str().to_owned(),
            });
        }

        let previous = self.local_identity().clone();
        let mut in_flight = 0;
        let mut unresolved = 0;
        for execution in self.tracked_mut().values_mut() {
            // Attribution moves from "this host" to "the source device", and an
            // effect this host never attributed stays unresolved.
            if let Some(dispatcher) = execution.writer().dispatcher().cloned() {
                execution.set_writer(ResponsibilityWriter::Transferred { dispatcher });
            } else {
                unresolved += 1;
            }
            if execution.dispatch().is_in_flight() {
                in_flight += 1;
            }
        }
        self.set_local_identity(destination.clone());
        *self.responsibility_slot() = Some(binding.clone());
        Ok(ResponsibilityTransfer {
            binding,
            previous,
            current: destination,
            tracked: self.tracked_len(),
            in_flight,
            unresolved,
        })
    }

    /// Issue one effect, exactly once, from the endpoint that owns it.
    ///
    /// This is the only place an effect is issued, so "zero duplicate dispatch"
    /// is a property of this call and not of a caller's bookkeeping:
    ///
    /// * a settled execution issues nothing (the work already ended);
    /// * an already-issued execution issues nothing — the destination must
    ///   reconcile the existing effect instead
    ///   ([`Self::reconcile_late_result`]);
    /// * a caller that is not the attributed issuer issues nothing, even when the
    ///   execution is unknown to it.
    ///
    /// A successful claim attributes the effect to `self.local_identity()` and
    /// leaves it in flight until a peer receipt settles it.
    pub fn claim_dispatch(
        &mut self,
        identity: &ExecutionIdentity,
    ) -> Result<DispatchClaim, ResponsibilityRefusal> {
        let local = self.local_identity().endpoint().clone();
        let execution = self.tracked_mut().get_mut(identity).ok_or_else(|| {
            ResponsibilityRefusal::NotTracked {
                identity: identity.clone(),
            }
        })?;
        if execution.state().is_confirmed() {
            return Err(ResponsibilityRefusal::AlreadyDispatched {
                identity: identity.clone(),
            });
        }
        if execution.dispatch().was_dispatched() {
            return Err(ResponsibilityRefusal::AlreadyDispatched {
                identity: identity.clone(),
            });
        }
        if execution.writer().dispatcher() != Some(&local) {
            return Err(ResponsibilityRefusal::NotActiveWriter {
                identity: identity.clone(),
                writer: execution.writer().clone(),
            });
        }
        execution.set_dispatch(DispatchState::issued(true));
        Ok(DispatchClaim {
            identity: identity.clone(),
            dispatcher: local,
        })
    }

    /// Record a late authenticated result against an effect already issued here.
    ///
    /// Reconciliation is idempotent and restricted: it needs an issued effect, it
    /// goes through the same forward-only receipt rule the settlement already
    /// enforces, and it never invokes the effect again. The claim is attributed to
    /// the endpoint that issued it, so a result for the source device's work is
    /// filed under the source device even when the destination reconciles it.
    pub fn reconcile_late_result(
        &mut self,
        identity: &ExecutionIdentity,
        receipt: AuthenticatedReceipt,
    ) -> Result<ReconciliationRecord, ResponsibilityRefusal> {
        let reconciled_by = self.local_identity().endpoint().clone();
        let execution = self.tracked_mut().get_mut(identity).ok_or_else(|| {
            ResponsibilityRefusal::NotTracked {
                identity: identity.clone(),
            }
        })?;
        if !execution.dispatch().was_dispatched() {
            return Err(ResponsibilityRefusal::NothingToReconcile {
                identity: identity.clone(),
            });
        }
        let originated_with = execution.writer().dispatcher().cloned();
        let state = receipt.state();
        let cursor = receipt.cursor();
        if let Err(refusal) = self.record_receipt(identity, receipt) {
            return Err(ResponsibilityRefusal::ReconciliationRefused { reason: refusal });
        }
        if let Some(execution) = self.tracked_mut().get_mut(identity) {
            execution.set_dispatch(DispatchState::issued(!state.is_confirmed()));
        }
        Ok(ReconciliationRecord {
            identity: identity.clone(),
            state,
            cursor,
            originated_with,
            reconciled_by,
        })
    }

    /// Issued effects still awaiting an authenticated result.
    #[must_use]
    pub fn in_flight(&self) -> Vec<&TrackedExecution> {
        self.tracked()
            .into_iter()
            .filter(|execution| execution.dispatch().is_in_flight())
            .collect()
    }

    /// Tracked effects whose original issuer is not known here.
    ///
    /// They stay visible and stay uninvokable: nothing may re-issue an effect on
    /// a guess about who started it.
    #[must_use]
    pub fn unresolved_responsibility(&self) -> Vec<&TrackedExecution> {
        self.tracked()
            .into_iter()
            .filter(|execution| matches!(execution.writer(), ResponsibilityWriter::Unresolved))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    //! Composition fixtures: one in-flight effect, then a replacement, then a
    //! late result.
    //!
    //! Each case drives the real operations over a real settlement and asserts
    //! the three properties the composition exists for: an effect is dispatched
    //! exactly once, work whose outcome nobody authenticated stays visible and
    //! uninvokable, and exactly one local endpoint is ever the writer of an
    //! effect — including after responsibility has moved.

    use super::{
        DispatchClaim, ReconciliationRecord, ResponsibilityBinding, ResponsibilityRefusal,
        ResponsibilityTransfer, ResponsibilityWriter,
    };
    use crate::control::authority::EndpointIdentity;
    use crate::control::ledger::RequestId;
    use crate::control::settlement::{
        AuthenticatedReceipt, ExecutionIdentity, LocalAdmission, LocalIdentity, LocalObservation,
        RemoteCursor, RemoteOutcomeState, RemoteSettlement, TrackedExecution,
    };

    fn peer() -> EndpointIdentity {
        EndpointIdentity::new("endpoint-peer")
    }

    fn identity(request: &str) -> ExecutionIdentity {
        ExecutionIdentity::new(peer(), RequestId::new(request))
    }

    /// The device that is being replaced.
    fn source_identity() -> LocalIdentity {
        LocalIdentity::new(EndpointIdentity::new("endpoint-source"), 1)
    }

    /// The replacement the subject activated.
    fn destination_identity() -> LocalIdentity {
        LocalIdentity::new(EndpointIdentity::new("endpoint-destination"), 1)
    }

    fn binding() -> ResponsibilityBinding {
        ResponsibilityBinding::new("subject-a", "endpoint-source", "endpoint-destination")
    }

    fn receipt(state: RemoteOutcomeState, sequence: u64) -> AuthenticatedReceipt {
        AuthenticatedReceipt::new(state, RemoteCursor::from_sequence(sequence))
    }

    /// A settlement on the source device with one locally admitted effect.
    fn settlement_with_local_work(request: &str) -> RemoteSettlement {
        let mut settlement = RemoteSettlement::new(source_identity());
        settlement
            .admit_peer_requested(
                TrackedExecution::local(identity(request), RemoteCursor::from_sequence(1))
                    .with_writer(ResponsibilityWriter::Local {
                        dispatcher: EndpointIdentity::new("endpoint-source"),
                    }),
                LocalAdmission::Admitted,
            )
            .expect("locally admitted work is tracked");
        settlement
    }

    #[test]
    fn an_effect_is_dispatched_exactly_once() {
        let mut settlement = settlement_with_local_work("request-1");
        let claimed = settlement
            .claim_dispatch(&identity("request-1"))
            .expect("the first claim issues the effect");
        assert_eq!(claimed.dispatcher().as_str(), "endpoint-source");
        assert_eq!(settlement.in_flight().len(), 1);

        // Every later claim is refused, which is what "zero duplicate dispatch"
        // means: the destination reconciles instead of re-issuing.
        assert_eq!(
            settlement.claim_dispatch(&identity("request-1")),
            Err(ResponsibilityRefusal::AlreadyDispatched {
                identity: identity("request-1")
            })
        );
        assert_eq!(settlement.in_flight().len(), 1);
    }

    #[test]
    fn a_local_update_may_not_move_responsibility_but_a_transfer_may() {
        let mut settlement = settlement_with_local_work("request-1");
        settlement
            .claim_dispatch(&identity("request-1"))
            .expect("the effect is issued");

        // An ordinary local update to a different endpoint still changes nothing:
        // abandoning remote work is not this slice's decision.
        assert!(
            settlement
                .apply_local_update(destination_identity())
                .is_err()
        );
        assert_eq!(
            settlement
                .execution(&identity("request-1"))
                .unwrap()
                .writer()
                .dispatcher()
                .map(EndpointIdentity::as_str),
            Some("endpoint-source"),
            "a refused update moves no attribution"
        );

        let transfer = settlement
            .transfer_responsibility(binding(), destination_identity())
            .expect("the authorized transfer moves responsibility");
        assert_eq!(transfer.tracked(), 1);
        assert_eq!(transfer.in_flight(), 1);
        assert_eq!(transfer.unresolved(), 0);
        assert_eq!(transfer.previous().endpoint().as_str(), "endpoint-source");
        assert_eq!(
            transfer.current().endpoint().as_str(),
            "endpoint-destination"
        );
        assert_eq!(
            settlement
                .responsibility()
                .map(ResponsibilityBinding::subject),
            Some("subject-a")
        );
    }

    #[test]
    fn a_transfer_for_another_source_or_destination_is_refused() {
        let mut settlement = settlement_with_local_work("request-1");

        assert_eq!(
            settlement.transfer_responsibility(
                ResponsibilityBinding::new("subject-a", "endpoint-other", "endpoint-destination"),
                destination_identity(),
            ),
            Err(ResponsibilityRefusal::SourceIsNotLocal {
                source: "endpoint-other".to_owned(),
                local: "endpoint-source".to_owned(),
            })
        );
        assert_eq!(
            settlement.transfer_responsibility(binding(), source_identity()),
            Err(ResponsibilityRefusal::DestinationIsLocal {
                destination: "endpoint-source".to_owned(),
            })
        );
        assert_eq!(
            settlement.transfer_responsibility(
                ResponsibilityBinding::new("subject-a", "endpoint-source", "endpoint-elsewhere"),
                destination_identity(),
            ),
            Err(ResponsibilityRefusal::SourceIsNotLocal {
                source: "endpoint-source".to_owned(),
                local: "endpoint-source".to_owned(),
            })
        );

        // Nothing moved: the source is still the writer and the work is intact.
        assert_eq!(
            settlement.local_identity().endpoint().as_str(),
            "endpoint-source"
        );
        assert_eq!(settlement.tracked_len(), 1);
    }

    #[test]
    fn the_destination_never_re_issues_an_effect_it_inherited() {
        let mut settlement = settlement_with_local_work("request-1");
        settlement
            .claim_dispatch(&identity("request-1"))
            .expect("the source issues the effect");
        settlement
            .transfer_responsibility(binding(), destination_identity())
            .expect("responsibility moves");

        // The destination holds an unresolved obligation, not a fresh effect.
        assert_eq!(
            settlement.claim_dispatch(&identity("request-1")),
            Err(ResponsibilityRefusal::AlreadyDispatched {
                identity: identity("request-1")
            })
        );
        assert_eq!(settlement.in_flight().len(), 1);

        // What it does instead is reconcile the result the source's effect
        // produces, filed under the source device's provenance.
        let reconciled = settlement
            .reconcile_late_result(
                &identity("request-1"),
                receipt(RemoteOutcomeState::Confirmed, 2),
            )
            .expect("the destination reconciles the late result");
        assert_eq!(
            reconciled.originated_with().map(EndpointIdentity::as_str),
            Some("endpoint-source")
        );
        assert_eq!(reconciled.reconciled_by().as_str(), "endpoint-destination");
        assert!(
            reconciled.is_foreign_effect(),
            "the result is visibly about work this device did not start"
        );
        assert_eq!(reconciled.state(), RemoteOutcomeState::Confirmed);
        assert!(settlement.in_flight().is_empty());
        assert!(settlement.unsettled().is_empty());
    }

    #[test]
    fn reconciling_the_same_late_result_twice_changes_nothing_the_second_time() {
        let mut settlement = settlement_with_local_work("request-1");
        settlement
            .claim_dispatch(&identity("request-1"))
            .expect("the effect is issued");
        settlement
            .transfer_responsibility(binding(), destination_identity())
            .expect("responsibility moves");

        let first = settlement
            .reconcile_late_result(
                &identity("request-1"),
                receipt(RemoteOutcomeState::Confirmed, 2),
            )
            .expect("the late result reconciles");
        assert_eq!(first.state(), RemoteOutcomeState::Confirmed);

        // The identical delivery is idempotent: it is accepted, and it leaves the
        // recorded state and cursor exactly where they were.
        let second = settlement
            .reconcile_late_result(
                &identity("request-1"),
                receipt(RemoteOutcomeState::Confirmed, 2),
            )
            .expect("an exact repeat of the same delivery reconciles to the same state");
        assert_eq!(second.state(), first.state());
        assert_eq!(second.cursor(), first.cursor());
        assert_eq!(
            settlement
                .execution(&identity("request-1"))
                .unwrap()
                .cursor(),
            RemoteCursor::from_sequence(2)
        );

        // And an older delivery is refused as stale, as it always was.
        assert!(matches!(
            settlement.reconcile_late_result(
                &identity("request-1"),
                receipt(RemoteOutcomeState::Stopping, 1)
            ),
            Err(ResponsibilityRefusal::ReconciliationRefused { .. })
        ));
    }

    #[test]
    fn unknown_work_survives_a_replacement_without_being_assigned_to_the_destination() {
        let mut fixture = SettlementFixture::new();
        // One effect whose outcome nobody authenticated.
        fixture.track_local("request-unknown", 1);
        fixture.claim("request-unknown");
        fixture.record_unknown("request-unknown");
        // And one effect that was issued and is still in flight.
        fixture.track_local("request-in-flight", 1);
        fixture.claim("request-in-flight");

        let before = fixture.snapshot();
        let transfer = fixture.transfer();
        assert_eq!(transfer.tracked(), 2);
        // Both issued effects are still unsettled: one because the peer reported
        // an unknown outcome, one because no result has arrived at all.
        assert_eq!(transfer.in_flight(), 2);
        assert!(fixture.is_in_flight("request-unknown"));
        assert!(fixture.is_in_flight("request-in-flight"));

        // Every recorded state and cursor is exactly what it was.
        assert_eq!(fixture.snapshot(), before);
        assert_eq!(
            fixture.state_of("request-unknown"),
            RemoteOutcomeState::Unknown,
            "an unknown outcome survives the replacement as unknown"
        );
        assert!(
            !fixture
                .execution("request-unknown")
                .expect("tracked")
                .permits_blind_retry(),
            "a replacement never licenses a blind retry"
        );
        assert_eq!(
            fixture.unresolved_responsibility().len(),
            0,
            "the issued effect keeps the source device's attribution, not an unknown one"
        );
        assert_eq!(
            fixture
                .execution("request-unknown")
                .expect("tracked")
                .writer()
                .as_str(),
            "transferred"
        );
    }

    #[test]
    fn a_replacement_does_not_turn_an_in_flight_effect_into_a_settled_one() {
        let mut fixture = SettlementFixture::new();
        fixture.track_local("request-in-flight", 1);
        fixture.claim("request-in-flight");
        assert!(fixture.is_in_flight("request-in-flight"));

        fixture.transfer();
        assert!(
            fixture.is_in_flight("request-in-flight"),
            "the obligation is carried, not discharged"
        );
        assert!(fixture.settlement.unsettled().len() == 1);
    }

    #[test]
    fn an_effect_with_no_known_issuer_is_never_invoked_on_a_guess() {
        let mut settlement = RemoteSettlement::new(source_identity());
        settlement
            .admit_peer_requested(
                TrackedExecution::local(identity("request-1"), RemoteCursor::from_sequence(1)),
                LocalAdmission::Admitted,
            )
            .expect("tracked");

        // Nothing has issued this effect, and this host never knew who would.
        assert_eq!(
            settlement.claim_dispatch(&identity("request-1")),
            Err(ResponsibilityRefusal::NotActiveWriter {
                identity: identity("request-1"),
                writer: ResponsibilityWriter::Unresolved,
            })
        );
        assert_eq!(
            settlement.reconcile_late_result(
                &identity("request-1"),
                receipt(RemoteOutcomeState::Confirmed, 1)
            ),
            Err(ResponsibilityRefusal::NothingToReconcile {
                identity: identity("request-1")
            }),
            "there is no effect to settle"
        );
    }

    #[test]
    fn a_result_for_an_untracked_execution_is_never_reconciled() {
        let mut settlement = RemoteSettlement::new(source_identity());
        assert_eq!(
            settlement.reconcile_late_result(
                &identity("request-9"),
                receipt(RemoteOutcomeState::Confirmed, 1)
            ),
            Err(ResponsibilityRefusal::NotTracked {
                identity: identity("request-9")
            })
        );
    }

    #[test]
    fn every_responsibility_refusal_has_a_distinct_stable_reason() {
        let reasons = [
            ResponsibilityRefusal::SourceIsNotLocal {
                source: "a".to_owned(),
                local: "b".to_owned(),
            }
            .reason(),
            ResponsibilityRefusal::DestinationIsLocal {
                destination: "a".to_owned(),
            }
            .reason(),
            ResponsibilityRefusal::AlreadyDispatched {
                identity: identity("request-1"),
            }
            .reason(),
            ResponsibilityRefusal::NotActiveWriter {
                identity: identity("request-1"),
                writer: ResponsibilityWriter::Unresolved,
            }
            .reason(),
            ResponsibilityRefusal::NothingToReconcile {
                identity: identity("request-1"),
            }
            .reason(),
            ResponsibilityRefusal::ReconciliationRefused {
                reason: crate::control::settlement::SettlementRefusal::NotTracked {
                    identity: identity("request-1"),
                },
            }
            .reason(),
            ResponsibilityRefusal::NotTracked {
                identity: identity("request-1"),
            }
            .reason(),
        ];
        for reason in reasons {
            assert!(
                reason.starts_with("endpoint_responsibility_"),
                "{reason} must name its own class"
            );
        }
        let mut unique = reasons.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), reasons.len(), "the reasons must be distinct");
    }

    /// One settlement driven through the whole composition, so each fixture reads
    /// as the sequence it is rather than as bookkeeping.
    struct SettlementFixture {
        settlement: RemoteSettlement,
    }

    impl SettlementFixture {
        fn new() -> Self {
            Self {
                settlement: RemoteSettlement::new(source_identity()),
            }
        }

        /// Tracks locally admitted work this device performs.
        fn track_local(&mut self, request: &str, cursor: u64) {
            self.settlement
                .admit_peer_requested(
                    TrackedExecution::local(identity(request), RemoteCursor::from_sequence(cursor))
                        .with_writer(ResponsibilityWriter::Local {
                            dispatcher: EndpointIdentity::new("endpoint-source"),
                        }),
                    LocalAdmission::Admitted,
                )
                .expect("local work is tracked");
        }

        /// Tracks a peer-owned execution this host awaits.
        fn track_remote(&mut self, request: &str, cursor: u64) {
            self.settlement
                .await_peer(TrackedExecution::remote(
                    identity(request),
                    RemoteCursor::from_sequence(cursor),
                ))
                .expect("peer work is tracked");
        }

        fn claim(&mut self, request: &str) -> DispatchClaim {
            self.settlement
                .claim_dispatch(&identity(request))
                .expect("the first claim issues the effect")
        }

        /// Records that the peer authenticated an outcome this host cannot
        /// determine from its own side.
        fn record_unknown(&mut self, request: &str) -> ReconciliationRecord {
            self.settlement
                .reconcile_late_result(&identity(request), receipt(RemoteOutcomeState::Unknown, 6))
                .expect("the peer's authenticated outcome is recorded")
        }

        fn state_of(&self, request: &str) -> RemoteOutcomeState {
            self.settlement
                .execution(&identity(request))
                .expect("tracked")
                .state()
        }

        fn execution(&self, request: &str) -> Option<&TrackedExecution> {
            self.settlement.execution(&identity(request))
        }

        fn unresolved_responsibility(&self) -> Vec<&TrackedExecution> {
            self.settlement.unresolved_responsibility()
        }

        /// Whether this host still owes an authenticated outcome for `request`.
        fn is_in_flight(&self, request: &str) -> bool {
            self.settlement
                .in_flight()
                .into_iter()
                .any(|execution| execution.identity() == &identity(request))
        }

        /// The recorded facts a replacement must carry through unchanged.
        fn snapshot(&self) -> Vec<(String, RemoteOutcomeState, RemoteCursor, bool)> {
            let mut snapshot = self
                .settlement
                .tracked()
                .into_iter()
                .map(|execution| {
                    (
                        execution.identity().request().as_str().to_owned(),
                        execution.state(),
                        execution.cursor(),
                        execution.dispatch().was_dispatched(),
                    )
                })
                .collect::<Vec<_>>();
            snapshot.sort();
            snapshot
        }

        fn transfer(&mut self) -> ResponsibilityTransfer {
            self.settlement
                .transfer_responsibility(binding(), destination_identity())
                .expect("the authorized transfer moves responsibility")
        }
    }
}
