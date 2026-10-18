//! Where a remote execution's result is settled, and what the local update gate
//! is allowed to wait for.
//!
//! A client that awaits a peer's work is not a client with unfinished local
//! work. This module keeps those two facts apart:
//!
//! * **Only locally admitted execution enters the local idle guard.** Work this
//!   host performs — including work a peer asked for, which still passes local
//!   admission — is a blocker while it is unfinished. A peer-owned execution
//!   this host merely awaits or displays is not, so an unreachable peer never
//!   holds this host's update gate.
//! * **A local observation is not a remote outcome.** Losing the carrier,
//!   losing a grant or letting time pass proves nothing about a remote stop, and
//!   none of them may move a recorded remote state or permit a blind retry.
//! * **Only an authenticated receipt settles a remote state, and only forwards.**
//!   A receipt carries the peer's own durable cursor; an older cursor is refused
//!   as stale, and a confirmed end is absorbing, so a stale writer cannot
//!   un-confirm work that ended.
//! * **Compatible local updates preserve what this host owes.** Identities,
//!   cursors and unknown-effect records survive a local update that keeps the
//!   local identity; an incompatible update changes nothing here and is refused
//!   rather than quietly abandoned, because remote abandonment belongs to the
//!   protocol owner and not to this slice.

use std::collections::BTreeMap;

use super::authority::EndpointIdentity;
use super::ledger::RequestId;
use super::replacement::ResponsibilityBinding;

/// The schema of [`SettlementRecord`].
pub const REMOTE_SETTLEMENT_RECORD_SCHEMA: &str = "licoup.endpoint-remote-settlement.v2";

/// Who performs one execution.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ExecutionOwner {
    /// This host performs it. It is the only kind that can hold the idle guard.
    LocalHost,
    /// The peer performs it and this host awaits or displays the result.
    PeerEndpoint,
}

impl ExecutionOwner {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LocalHost => "localHost",
            Self::PeerEndpoint => "peerEndpoint",
        }
    }
}

/// One execution identity: the peer plus the durable request identity.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ExecutionIdentity {
    peer: EndpointIdentity,
    request: RequestId,
}

impl ExecutionIdentity {
    #[must_use]
    pub const fn new(peer: EndpointIdentity, request: RequestId) -> Self {
        Self { peer, request }
    }

    #[must_use]
    pub const fn peer(&self) -> &EndpointIdentity {
        &self.peer
    }

    #[must_use]
    pub const fn request(&self) -> &RequestId {
        &self.request
    }
}

/// The peer's own durable cursor for one execution.
///
/// It is the peer's ordering, carried through unchanged. This host never mints
/// one and never advances one on its own.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RemoteCursor(u64);

impl RemoteCursor {
    #[must_use]
    pub const fn from_sequence(sequence: u64) -> Self {
        Self(sequence)
    }

    #[must_use]
    pub const fn sequence(self) -> u64 {
        self.0
    }
}

/// The state one remote execution is in.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RemoteOutcomeState {
    /// The peer accepted the request.
    Requested,
    /// The peer reports that it is stopping the work.
    Stopping,
    /// The peer's end was authenticated.
    Confirmed,
    /// The peer's outcome is unknown here. It stays visible as unknown.
    Unknown,
}

impl RemoteOutcomeState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Stopping => "stopping",
            Self::Confirmed => "confirmed",
            Self::Unknown => "unknown",
        }
    }

    /// Whether this state is an authenticated end.
    #[must_use]
    pub const fn is_confirmed(self) -> bool {
        matches!(self, Self::Confirmed)
    }
}

/// One authenticated receipt from the peer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuthenticatedReceipt {
    state: RemoteOutcomeState,
    cursor: RemoteCursor,
}

impl AuthenticatedReceipt {
    #[must_use]
    pub const fn new(state: RemoteOutcomeState, cursor: RemoteCursor) -> Self {
        Self { state, cursor }
    }

    #[must_use]
    pub const fn state(self) -> RemoteOutcomeState {
        self.state
    }

    #[must_use]
    pub const fn cursor(self) -> RemoteCursor {
        self.cursor
    }
}

/// One local observation that is not a remote outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalObservation {
    /// The carrier went away. The peer may still be working.
    CarrierLost,
    /// The local grant was revoked. A revocation governs new requests.
    GrantRevoked,
    /// Time passed. Nothing about a remote end follows from it.
    Elapsed { seconds: u64 },
}

impl LocalObservation {
    /// Whether this observation proves a remote outcome. It never does.
    #[must_use]
    pub const fn proves_remote_outcome(self) -> bool {
        false
    }
}

/// What one local observation left behind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObservationRecord {
    observation: LocalObservation,
    state: RemoteOutcomeState,
    proves_remote_outcome: bool,
}

impl ObservationRecord {
    #[must_use]
    pub const fn observation(&self) -> LocalObservation {
        self.observation
    }

    #[must_use]
    pub const fn state(&self) -> RemoteOutcomeState {
        self.state
    }

    /// Always `false`: a local fact is not a remote outcome.
    #[must_use]
    pub const fn proves_remote_outcome(&self) -> bool {
        self.proves_remote_outcome
    }
}

/// Which local endpoint issued one effect, and which local endpoints are
/// currently allowed to invoke it.
///
/// [`Self::Unresolved`] is a real, reportable answer and not an absent value: it
/// says the effect's original issuer is not known here, which is exactly what
/// this host must report after a transfer that did not carry that fact. An
/// unresolved effect is never re-dispatched on a guess.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResponsibilityWriter {
    /// The effect was issued by this local endpoint.
    Local { dispatcher: EndpointIdentity },
    /// The effect was issued by another local endpoint of the same subject, and
    /// this host knows which one. Only this endpoint may invoke it again.
    Transferred { dispatcher: EndpointIdentity },
    /// The effect's original issuer is not known here. Nothing may invoke it.
    Unresolved,
}

impl ResponsibilityWriter {
    /// The endpoint that issued the effect, when this host knows it.
    #[must_use]
    pub const fn dispatcher(&self) -> Option<&EndpointIdentity> {
        match self {
            Self::Local { dispatcher } | Self::Transferred { dispatcher } => Some(dispatcher),
            Self::Unresolved => None,
        }
    }

    /// The stable, non-secret name of this attribution.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Local { .. } => "local",
            Self::Transferred { .. } => "transferred",
            Self::Unresolved => "unresolved",
        }
    }
}

/// Whether one effect has already been issued, and by whom.
///
/// It is the record's re-dispatch guard. A transferred effect keeps its count, so
/// the replacement cannot issue a second one: settling it needs the peer's own
/// authenticated receipt, which is what [`RemoteSettlement::record_receipt`]
/// accepts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DispatchState {
    dispatched: bool,
    in_flight: bool,
}

impl DispatchState {
    /// No effect has been issued for this execution.
    #[must_use]
    pub const fn undispatched() -> Self {
        Self {
            dispatched: false,
            in_flight: false,
        }
    }

    /// Whether an effect was ever issued for this execution.
    #[must_use]
    pub const fn was_dispatched(self) -> bool {
        self.dispatched
    }

    /// Whether an issued effect is still unsettled.
    #[must_use]
    pub const fn is_in_flight(self) -> bool {
        self.in_flight
    }

    /// An effect that was issued, and whether it is still unsettled.
    #[must_use]
    pub const fn issued(in_flight: bool) -> Self {
        Self {
            dispatched: true,
            in_flight,
        }
    }
}

/// One execution this host tracks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrackedExecution {
    identity: ExecutionIdentity,
    owner: ExecutionOwner,
    state: RemoteOutcomeState,
    cursor: RemoteCursor,
    late_receipt: Option<AuthenticatedReceipt>,
    writer: ResponsibilityWriter,
    dispatch: DispatchState,
}

impl TrackedExecution {
    /// A peer-owned execution this host requested and awaits.
    ///
    /// Merely awaiting a peer's result issues no effect here, so the record
    /// starts with nothing dispatched and no writer attributed.
    #[must_use]
    pub fn remote(identity: ExecutionIdentity, cursor: RemoteCursor) -> Self {
        Self {
            identity,
            owner: ExecutionOwner::PeerEndpoint,
            state: RemoteOutcomeState::Requested,
            cursor,
            late_receipt: None,
            writer: ResponsibilityWriter::Unresolved,
            dispatch: DispatchState::undispatched(),
        }
    }

    /// Work a peer asked this host to perform, after local admission.
    ///
    /// Local admission is the decision, not the effect: nothing has been issued
    /// for it yet, which is why the dispatch record starts empty.
    #[must_use]
    pub fn local(identity: ExecutionIdentity, cursor: RemoteCursor) -> Self {
        Self {
            identity,
            owner: ExecutionOwner::LocalHost,
            state: RemoteOutcomeState::Requested,
            cursor,
            late_receipt: None,
            writer: ResponsibilityWriter::Unresolved,
            dispatch: DispatchState::undispatched(),
        }
    }

    /// The same execution, attributed to the endpoint that may invoke it.
    ///
    /// A caller that admits work names the endpoint it admitted the work for; a
    /// record restored without that fact stays
    /// [`ResponsibilityWriter::Unresolved`] and is never invoked on a guess.
    #[must_use]
    pub fn with_writer(mut self, writer: ResponsibilityWriter) -> Self {
        self.writer = writer;
        self
    }

    /// Which local endpoints may invoke this effect, and which one issued it.
    #[must_use]
    pub const fn writer(&self) -> &ResponsibilityWriter {
        &self.writer
    }

    /// Whether an effect was ever issued for this execution.
    #[must_use]
    pub const fn dispatch(&self) -> DispatchState {
        self.dispatch
    }

    /// Moves this execution's attribution without touching anything else.
    pub(super) fn set_writer(&mut self, writer: ResponsibilityWriter) {
        self.writer = writer;
    }

    /// Records one issue or settlement of this execution's effect.
    pub(super) fn set_dispatch(&mut self, dispatch: DispatchState) {
        self.dispatch = dispatch;
    }

    #[must_use]
    pub const fn identity(&self) -> &ExecutionIdentity {
        &self.identity
    }

    #[must_use]
    pub const fn owner(&self) -> ExecutionOwner {
        self.owner
    }

    #[must_use]
    pub const fn state(&self) -> RemoteOutcomeState {
        self.state
    }

    /// The newest cursor this host recorded for the execution.
    #[must_use]
    pub const fn cursor(&self) -> RemoteCursor {
        self.cursor
    }

    /// The authenticated receipt that arrived after the request, when one did.
    #[must_use]
    pub const fn late_receipt(&self) -> Option<AuthenticatedReceipt> {
        self.late_receipt
    }

    /// Whether this recorded state is an authenticated end.
    #[must_use]
    pub const fn is_settled(&self) -> bool {
        self.state.is_confirmed()
    }

    /// Whether this host may re-issue the effect without a receipt.
    ///
    /// It never may: an outcome this host has not authenticated is reconciled
    /// with the peer, and re-issuing it would be a second effect on a request
    /// whose answer may already exist.
    #[must_use]
    pub const fn permits_blind_retry(&self) -> bool {
        false
    }

    /// Whether this execution is unfinished work this host performs.
    #[must_use]
    pub const fn blocks_local_update(&self) -> bool {
        matches!(self.owner, ExecutionOwner::LocalHost) && !self.state.is_confirmed()
    }
}

/// The local admission decision for work a peer asked for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LocalAdmission {
    /// The local admission accepted the work and the local idle gate applies.
    Admitted,
    /// The local admission refused it. Nothing is tracked and nothing runs.
    Refused { reason: String },
}

/// The local identity a compatible update keeps.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalIdentity {
    endpoint: EndpointIdentity,
    generation: u64,
}

impl LocalIdentity {
    #[must_use]
    pub const fn new(endpoint: EndpointIdentity, generation: u64) -> Self {
        Self {
            endpoint,
            generation,
        }
    }

    #[must_use]
    pub const fn endpoint(&self) -> &EndpointIdentity {
        &self.endpoint
    }

    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }
}

/// Why the settlement record refused a write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SettlementRefusal {
    /// The identity is already tracked.
    AlreadyTracked { identity: ExecutionIdentity },
    /// The identity is not tracked here.
    NotTracked { identity: ExecutionIdentity },
    /// The execution names the wrong owner for this entry point.
    WrongOwner {
        expected: ExecutionOwner,
        found: ExecutionOwner,
    },
    /// Work a peer asked for was refused by local admission.
    LocalAdmissionRefused { reason: String },
    /// The receipt's cursor is older than the recorded one.
    StaleReceipt {
        recorded: RemoteCursor,
        announced: RemoteCursor,
    },
    /// The recorded state does not allow the receipt's state.
    IllegalTransition {
        from: RemoteOutcomeState,
        to: RemoteOutcomeState,
    },
    /// The local update does not keep the local identity this record was built
    /// under.
    IncompatibleLocalUpdate { current: String, announced: String },
    /// The durable record carries another schema.
    RecordSchemaMismatch { schema: String },
}

impl SettlementRefusal {
    /// The stable reason a caller publishes.
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        match self {
            Self::AlreadyTracked { .. } => "endpoint_settlement_already_tracked",
            Self::NotTracked { .. } => "endpoint_settlement_not_tracked",
            Self::WrongOwner { .. } => "endpoint_settlement_wrong_owner",
            Self::LocalAdmissionRefused { .. } => "endpoint_settlement_local_admission_refused",
            Self::StaleReceipt { .. } => "endpoint_settlement_stale_receipt",
            Self::IllegalTransition { .. } => "endpoint_settlement_state_transition_refused",
            Self::IncompatibleLocalUpdate { .. } => "endpoint_settlement_incompatible_local_update",
            Self::RecordSchemaMismatch { .. } => "endpoint_settlement_record_schema_mismatch",
        }
    }
}

/// The durable projection of every execution this host tracks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettlementRecord {
    schema: String,
    local_identity: LocalIdentity,
    tracked: Vec<TrackedExecution>,
    responsibility: Option<ResponsibilityBinding>,
}

impl SettlementRecord {
    #[must_use]
    pub fn tracked(&self) -> &[TrackedExecution] {
        &self.tracked
    }

    #[must_use]
    pub const fn local_identity(&self) -> &LocalIdentity {
        &self.local_identity
    }

    #[must_use]
    pub fn schema_matches(&self) -> bool {
        self.schema == REMOTE_SETTLEMENT_RECORD_SCHEMA
    }
}

/// Every execution this host tracks, and what its local update gate may wait for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteSettlement {
    local_identity: LocalIdentity,
    tracked: BTreeMap<ExecutionIdentity, TrackedExecution>,
    responsibility: Option<ResponsibilityBinding>,
}

impl RemoteSettlement {
    #[must_use]
    pub fn new(local_identity: LocalIdentity) -> Self {
        Self {
            local_identity,
            tracked: BTreeMap::new(),
            responsibility: None,
        }
    }

    /// The settlement a previous run recorded.
    pub fn restored(record: SettlementRecord) -> Result<Self, SettlementRefusal> {
        if !record.schema_matches() {
            return Err(SettlementRefusal::RecordSchemaMismatch {
                schema: record.schema,
            });
        }
        Ok(Self {
            local_identity: record.local_identity,
            tracked: record
                .tracked
                .into_iter()
                .map(|execution| (execution.identity.clone(), execution))
                .collect(),
            responsibility: record.responsibility,
        })
    }

    /// The durable projection of everything this host tracks.
    #[must_use]
    pub fn durable_record(&self) -> SettlementRecord {
        SettlementRecord {
            schema: REMOTE_SETTLEMENT_RECORD_SCHEMA.to_owned(),
            local_identity: self.local_identity.clone(),
            tracked: self.tracked.values().cloned().collect(),
            responsibility: self.responsibility.clone(),
        }
    }

    /// The replacement this host's responsibility was last transferred by, if any.
    ///
    /// A record written before responsibility carried dispatch ownership has no
    /// binding and is not adopted, so the format stays single.
    #[must_use]
    pub const fn responsibility(&self) -> Option<&ResponsibilityBinding> {
        self.responsibility.as_ref()
    }

    /// How many executions this host tracks.
    #[must_use]
    pub fn tracked_len(&self) -> usize {
        self.tracked.len()
    }

    /// Every tracked execution, ordered by identity.
    #[must_use]
    pub fn tracked(&self) -> Vec<&TrackedExecution> {
        self.tracked.values().collect()
    }

    /// The tracked executions, for the operations that move their attribution.
    pub(super) fn tracked_mut(&mut self) -> &mut BTreeMap<ExecutionIdentity, TrackedExecution> {
        &mut self.tracked
    }

    /// Replaces the local identity after an authorized transfer.
    pub(super) fn set_local_identity(&mut self, identity: LocalIdentity) {
        self.local_identity = identity;
    }

    /// The slot an authorized transfer records its binding in.
    pub(super) fn responsibility_slot(&mut self) -> &mut Option<ResponsibilityBinding> {
        &mut self.responsibility
    }

    #[must_use]
    pub const fn local_identity(&self) -> &LocalIdentity {
        &self.local_identity
    }

    /// One tracked execution.
    #[must_use]
    pub fn execution(&self, identity: &ExecutionIdentity) -> Option<&TrackedExecution> {
        self.tracked.get(identity)
    }

    /// Track a peer-owned execution this host requested and now awaits.
    ///
    /// Merely awaiting or displaying a peer's result creates no local work, so
    /// this never becomes a blocker for a local update.
    pub fn await_peer(&mut self, execution: TrackedExecution) -> Result<(), SettlementRefusal> {
        if execution.owner != ExecutionOwner::PeerEndpoint {
            return Err(SettlementRefusal::WrongOwner {
                expected: ExecutionOwner::PeerEndpoint,
                found: execution.owner,
            });
        }
        self.track(execution)
    }

    /// Track work a peer asked this host to perform.
    ///
    /// The request still passes this host's own admission first: a refused
    /// admission tracks nothing and runs nothing.
    pub fn admit_peer_requested(
        &mut self,
        execution: TrackedExecution,
        admission: LocalAdmission,
    ) -> Result<(), SettlementRefusal> {
        if execution.owner != ExecutionOwner::LocalHost {
            return Err(SettlementRefusal::WrongOwner {
                expected: ExecutionOwner::LocalHost,
                found: execution.owner,
            });
        }
        match admission {
            LocalAdmission::Admitted => self.track(execution),
            LocalAdmission::Refused { reason } => {
                Err(SettlementRefusal::LocalAdmissionRefused { reason })
            }
        }
    }

    fn track(&mut self, execution: TrackedExecution) -> Result<(), SettlementRefusal> {
        if self.tracked.contains_key(&execution.identity) {
            return Err(SettlementRefusal::AlreadyTracked {
                identity: execution.identity,
            });
        }
        self.tracked.insert(execution.identity.clone(), execution);
        Ok(())
    }

    /// Record one authenticated receipt against a tracked execution.
    ///
    /// Only a receipt moves a recorded state, and only forwards: an older cursor
    /// is refused as stale and a confirmed end is absorbing.
    pub fn record_receipt(
        &mut self,
        identity: &ExecutionIdentity,
        receipt: AuthenticatedReceipt,
    ) -> Result<&TrackedExecution, SettlementRefusal> {
        let execution =
            self.tracked
                .get_mut(identity)
                .ok_or_else(|| SettlementRefusal::NotTracked {
                    identity: identity.clone(),
                })?;
        if receipt.cursor < execution.cursor {
            return Err(SettlementRefusal::StaleReceipt {
                recorded: execution.cursor,
                announced: receipt.cursor,
            });
        }
        if !receipt_transition_allowed(execution.state, receipt.state) {
            return Err(SettlementRefusal::IllegalTransition {
                from: execution.state,
                to: receipt.state,
            });
        }
        execution.state = receipt.state;
        execution.cursor = receipt.cursor;
        execution.late_receipt = Some(receipt);
        Ok(execution)
    }

    /// Note one local observation. It never moves a recorded remote state.
    pub fn note_local_observation(
        &mut self,
        identity: &ExecutionIdentity,
        observation: LocalObservation,
    ) -> Result<ObservationRecord, SettlementRefusal> {
        let execution =
            self.tracked
                .get(identity)
                .ok_or_else(|| SettlementRefusal::NotTracked {
                    identity: identity.clone(),
                })?;
        Ok(ObservationRecord {
            observation,
            state: execution.state,
            proves_remote_outcome: observation.proves_remote_outcome(),
        })
    }

    /// Apply one local update.
    ///
    /// A compatible update — the same local endpoint at the same or a newer
    /// generation — preserves every tracked identity, cursor and unknown-effect
    /// record. An incompatible one changes nothing here: this slice does not
    /// abandon remote work on its own.
    pub fn apply_local_update(
        &mut self,
        update: LocalIdentity,
    ) -> Result<usize, SettlementRefusal> {
        let compatible = update.endpoint == self.local_identity.endpoint
            && update.generation >= self.local_identity.generation;
        if !compatible {
            return Err(SettlementRefusal::IncompatibleLocalUpdate {
                current: format!(
                    "{}@{}",
                    self.local_identity.endpoint.as_str(),
                    self.local_identity.generation
                ),
                announced: format!("{}@{}", update.endpoint.as_str(), update.generation),
            });
        }
        self.local_identity = update;
        Ok(self.tracked.len())
    }

    /// Unfinished work this host performs: what the local update gate reads.
    #[must_use]
    pub fn blockers(&self) -> Vec<&TrackedExecution> {
        self.tracked
            .values()
            .filter(|execution| execution.blocks_local_update())
            .collect()
    }

    /// Whether this host has unfinished work of its own.
    #[must_use]
    pub fn holds_local_work(&self) -> bool {
        self.tracked
            .values()
            .any(TrackedExecution::blocks_local_update)
    }

    /// Peer-owned executions this host awaits or displays.
    #[must_use]
    pub fn awaiting_peer(&self) -> Vec<&TrackedExecution> {
        self.tracked
            .values()
            .filter(|execution| execution.owner == ExecutionOwner::PeerEndpoint)
            .collect()
    }

    /// Executions whose outcome this host has not authenticated.
    #[must_use]
    pub fn unsettled(&self) -> Vec<&TrackedExecution> {
        self.tracked
            .values()
            .filter(|execution| !execution.is_settled())
            .collect()
    }
}

/// Whether a recorded remote state may move to the receipt's state.
///
/// A confirmed end is absorbing: no later receipt un-confirms it.
fn receipt_transition_allowed(from: RemoteOutcomeState, to: RemoteOutcomeState) -> bool {
    if from == to {
        return true;
    }
    match from {
        RemoteOutcomeState::Requested => matches!(
            to,
            RemoteOutcomeState::Stopping
                | RemoteOutcomeState::Confirmed
                | RemoteOutcomeState::Unknown
        ),
        RemoteOutcomeState::Stopping => matches!(
            to,
            RemoteOutcomeState::Confirmed | RemoteOutcomeState::Unknown
        ),
        RemoteOutcomeState::Unknown => matches!(
            to,
            RemoteOutcomeState::Stopping | RemoteOutcomeState::Confirmed
        ),
        RemoteOutcomeState::Confirmed => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AuthenticatedReceipt, ExecutionIdentity, ExecutionOwner, LocalAdmission, LocalIdentity,
        LocalObservation, REMOTE_SETTLEMENT_RECORD_SCHEMA, RemoteCursor, RemoteOutcomeState,
        RemoteSettlement, SettlementRefusal, TrackedExecution,
    };
    use crate::control::authority::EndpointIdentity;
    use crate::control::ledger::RequestId;

    fn identity(request: &str) -> ExecutionIdentity {
        ExecutionIdentity::new(EndpointIdentity::new("endpoint-b"), RequestId::new(request))
    }

    fn local_identity(generation: u64) -> LocalIdentity {
        LocalIdentity::new(EndpointIdentity::new("endpoint-a"), generation)
    }

    fn settlement() -> RemoteSettlement {
        RemoteSettlement::new(local_identity(1))
    }

    fn receipt(state: RemoteOutcomeState, sequence: u64) -> AuthenticatedReceipt {
        AuthenticatedReceipt::new(state, RemoteCursor::from_sequence(sequence))
    }

    #[test]
    fn an_unreachable_peer_does_not_hold_the_local_update_gate() {
        let mut settlement = settlement();
        settlement
            .await_peer(TrackedExecution::remote(
                identity("request-1"),
                RemoteCursor::from_sequence(1),
            ))
            .expect("a peer-owned execution is tracked");

        assert!(!settlement.holds_local_work());
        assert!(settlement.blockers().is_empty());
        assert_eq!(settlement.awaiting_peer().len(), 1);
        assert_eq!(settlement.unsettled().len(), 1);
    }

    #[test]
    fn a_real_unfinished_local_task_does_hold_the_gate_until_it_is_confirmed() {
        let mut settlement = settlement();
        settlement
            .admit_peer_requested(
                TrackedExecution::local(identity("request-1"), RemoteCursor::from_sequence(1)),
                LocalAdmission::Admitted,
            )
            .expect("locally admitted work is tracked");

        assert!(settlement.holds_local_work());
        assert_eq!(settlement.blockers().len(), 1);

        settlement
            .record_receipt(
                &identity("request-1"),
                receipt(RemoteOutcomeState::Confirmed, 2),
            )
            .expect("an authenticated end settles it");
        assert!(!settlement.holds_local_work());
        assert!(settlement.unsettled().is_empty());
    }

    #[test]
    fn local_admission_still_decides_whether_peer_requested_work_runs_here() {
        let mut settlement = settlement();

        assert_eq!(
            settlement.admit_peer_requested(
                TrackedExecution::local(identity("request-1"), RemoteCursor::from_sequence(1)),
                LocalAdmission::Refused {
                    reason: "localAdmissionRefused".to_owned()
                }
            ),
            Err(SettlementRefusal::LocalAdmissionRefused {
                reason: "localAdmissionRefused".to_owned()
            })
        );
        assert!(settlement.execution(&identity("request-1")).is_none());
        assert!(!settlement.holds_local_work());
    }

    #[test]
    fn a_peer_owned_execution_cannot_be_admitted_as_local_work_and_the_reverse() {
        let mut settlement = settlement();

        assert_eq!(
            settlement.await_peer(TrackedExecution::local(
                identity("request-1"),
                RemoteCursor::from_sequence(1)
            )),
            Err(SettlementRefusal::WrongOwner {
                expected: ExecutionOwner::PeerEndpoint,
                found: ExecutionOwner::LocalHost,
            })
        );
        assert_eq!(
            settlement.admit_peer_requested(
                TrackedExecution::remote(identity("request-1"), RemoteCursor::from_sequence(1)),
                LocalAdmission::Admitted,
            ),
            Err(SettlementRefusal::WrongOwner {
                expected: ExecutionOwner::LocalHost,
                found: ExecutionOwner::PeerEndpoint,
            })
        );
    }

    #[test]
    fn local_carrier_loss_revocation_and_elapsed_time_prove_no_remote_outcome() {
        let mut settlement = settlement();
        settlement
            .await_peer(TrackedExecution::remote(
                identity("request-1"),
                RemoteCursor::from_sequence(1),
            ))
            .expect("tracked");

        for observation in [
            LocalObservation::CarrierLost,
            LocalObservation::GrantRevoked,
            LocalObservation::Elapsed { seconds: 86_400 },
        ] {
            assert!(!observation.proves_remote_outcome());
            let record = settlement
                .note_local_observation(&identity("request-1"), observation)
                .expect("the identity is tracked");
            assert_eq!(record.state(), RemoteOutcomeState::Requested);
            assert!(!record.proves_remote_outcome());
        }
        assert_eq!(
            settlement
                .execution(&identity("request-1"))
                .unwrap()
                .state(),
            RemoteOutcomeState::Requested,
            "no local observation moved the recorded state"
        );
    }

    #[test]
    fn a_stale_writer_cannot_move_the_recorded_state_backwards() {
        let mut settlement = settlement();
        settlement
            .await_peer(TrackedExecution::remote(
                identity("request-1"),
                RemoteCursor::from_sequence(1),
            ))
            .expect("tracked");
        settlement
            .record_receipt(
                &identity("request-1"),
                receipt(RemoteOutcomeState::Stopping, 5),
            )
            .expect("a newer cursor is accepted");

        assert_eq!(
            settlement.record_receipt(
                &identity("request-1"),
                receipt(RemoteOutcomeState::Requested, 4)
            ),
            Err(SettlementRefusal::StaleReceipt {
                recorded: RemoteCursor::from_sequence(5),
                announced: RemoteCursor::from_sequence(4),
            })
        );
        assert_eq!(
            settlement
                .execution(&identity("request-1"))
                .unwrap()
                .state(),
            RemoteOutcomeState::Stopping
        );
    }

    #[test]
    fn cancel_and_result_ordering_is_explicit_and_a_confirmed_end_is_absorbing() {
        let mut settlement = settlement();
        settlement
            .await_peer(TrackedExecution::remote(
                identity("request-1"),
                RemoteCursor::from_sequence(1),
            ))
            .expect("tracked");

        settlement
            .record_receipt(
                &identity("request-1"),
                receipt(RemoteOutcomeState::Stopping, 2),
            )
            .expect("stopping follows requested");
        settlement
            .record_receipt(
                &identity("request-1"),
                receipt(RemoteOutcomeState::Confirmed, 3),
            )
            .expect("a result that arrives after the cancel confirms the end");

        assert_eq!(
            settlement.record_receipt(
                &identity("request-1"),
                receipt(RemoteOutcomeState::Requested, 4)
            ),
            Err(SettlementRefusal::IllegalTransition {
                from: RemoteOutcomeState::Confirmed,
                to: RemoteOutcomeState::Requested,
            }),
            "a later writer cannot un-confirm an authenticated end"
        );
        assert_eq!(
            settlement
                .execution(&identity("request-1"))
                .unwrap()
                .state(),
            RemoteOutcomeState::Confirmed
        );
    }

    #[test]
    fn a_receipt_for_an_untracked_identity_is_refused() {
        let mut settlement = settlement();
        assert_eq!(
            settlement.record_receipt(
                &identity("request-9"),
                receipt(RemoteOutcomeState::Confirmed, 1)
            ),
            Err(SettlementRefusal::NotTracked {
                identity: identity("request-9")
            })
        );
    }

    #[test]
    fn a_compatible_local_update_preserves_identities_cursors_and_unknown_effects() {
        let mut settlement = settlement();
        settlement
            .await_peer(TrackedExecution::remote(
                identity("request-1"),
                RemoteCursor::from_sequence(7),
            ))
            .expect("tracked");
        settlement
            .record_receipt(
                &identity("request-1"),
                receipt(RemoteOutcomeState::Unknown, 8),
            )
            .expect("an unknown outcome is recorded");

        let preserved = settlement
            .apply_local_update(local_identity(2))
            .expect("the same local endpoint at a newer generation is compatible");

        assert_eq!(preserved, 1);
        let execution = settlement.execution(&identity("request-1")).unwrap();
        assert_eq!(execution.state(), RemoteOutcomeState::Unknown);
        assert_eq!(execution.cursor(), RemoteCursor::from_sequence(8));
        assert_eq!(settlement.local_identity().generation(), 2);
    }

    #[test]
    fn an_incompatible_local_update_changes_nothing_here() {
        let mut settlement = settlement();
        settlement
            .await_peer(TrackedExecution::remote(
                identity("request-1"),
                RemoteCursor::from_sequence(7),
            ))
            .expect("tracked");

        for update in [
            LocalIdentity::new(EndpointIdentity::new("endpoint-c"), 1),
            local_identity(0),
        ] {
            assert!(matches!(
                settlement.apply_local_update(update),
                Err(SettlementRefusal::IncompatibleLocalUpdate { .. })
            ));
        }
        assert_eq!(settlement.local_identity().generation(), 1);
        assert_eq!(settlement.awaiting_peer().len(), 1);
    }

    #[test]
    fn neither_a_restart_nor_a_local_update_fabricates_a_settlement() {
        let mut settlement = settlement();
        settlement
            .await_peer(TrackedExecution::remote(
                identity("request-1"),
                RemoteCursor::from_sequence(1),
            ))
            .expect("tracked");
        settlement
            .record_receipt(
                &identity("request-1"),
                receipt(RemoteOutcomeState::Unknown, 2),
            )
            .expect("recorded");

        let record = settlement.durable_record();
        assert!(record.schema_matches());
        assert_eq!(record.tracked().len(), 1);

        let mut recovered =
            RemoteSettlement::restored(record).expect("the record carries this schema");
        assert_eq!(
            recovered.execution(&identity("request-1")).unwrap().state(),
            RemoteOutcomeState::Unknown,
            "a restart does not turn an unknown effect into a confirmed one"
        );
        assert_eq!(recovered.unsettled().len(), 1);
        assert!(
            !recovered
                .execution(&identity("request-1"))
                .unwrap()
                .permits_blind_retry()
        );

        recovered
            .record_receipt(
                &identity("request-1"),
                receipt(RemoteOutcomeState::Confirmed, 3),
            )
            .expect("an authenticated receipt settles it after the restart");
        assert!(recovered.unsettled().is_empty());
    }

    #[test]
    fn a_record_from_another_schema_is_not_adopted() {
        let mut settlement = settlement();
        settlement
            .await_peer(TrackedExecution::remote(
                identity("request-1"),
                RemoteCursor::from_sequence(1),
            ))
            .expect("tracked");
        let mut record = settlement.durable_record();
        record.schema = "licoup.endpoint-remote-settlement.v1".to_owned();

        // A record that predates dispatch ownership is not adopted either: it
        // cannot say which local endpoint issued an effect, and guessing would be
        // exactly the second dispatch the ownership record exists to prevent.
        assert_eq!(
            RemoteSettlement::restored(record),
            Err(SettlementRefusal::RecordSchemaMismatch {
                schema: "licoup.endpoint-remote-settlement.v1".to_owned()
            })
        );
        assert_eq!(
            REMOTE_SETTLEMENT_RECORD_SCHEMA,
            "licoup.endpoint-remote-settlement.v2"
        );
    }

    #[test]
    fn tracking_one_identity_twice_is_refused() {
        let mut settlement = settlement();
        settlement
            .await_peer(TrackedExecution::remote(
                identity("request-1"),
                RemoteCursor::from_sequence(1),
            ))
            .expect("tracked");

        assert_eq!(
            settlement.await_peer(TrackedExecution::remote(
                identity("request-1"),
                RemoteCursor::from_sequence(2)
            )),
            Err(SettlementRefusal::AlreadyTracked {
                identity: identity("request-1")
            })
        );
    }

    #[test]
    fn every_refusal_has_a_distinct_stable_reason() {
        let reasons = [
            SettlementRefusal::AlreadyTracked {
                identity: identity("request-1"),
            }
            .reason(),
            SettlementRefusal::NotTracked {
                identity: identity("request-1"),
            }
            .reason(),
            SettlementRefusal::WrongOwner {
                expected: ExecutionOwner::LocalHost,
                found: ExecutionOwner::PeerEndpoint,
            }
            .reason(),
            SettlementRefusal::LocalAdmissionRefused {
                reason: "refused".to_owned(),
            }
            .reason(),
            SettlementRefusal::StaleReceipt {
                recorded: RemoteCursor::from_sequence(2),
                announced: RemoteCursor::from_sequence(1),
            }
            .reason(),
            SettlementRefusal::IllegalTransition {
                from: RemoteOutcomeState::Confirmed,
                to: RemoteOutcomeState::Requested,
            }
            .reason(),
            SettlementRefusal::IncompatibleLocalUpdate {
                current: "endpoint-a@1".to_owned(),
                announced: "endpoint-c@1".to_owned(),
            }
            .reason(),
            SettlementRefusal::RecordSchemaMismatch {
                schema: "other".to_owned(),
            }
            .reason(),
        ];
        for reason in reasons {
            assert!(reason.starts_with("endpoint_settlement_"), "{reason}");
        }
        let mut unique = reasons.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), reasons.len());
    }
}
