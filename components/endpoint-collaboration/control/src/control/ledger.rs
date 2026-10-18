//! The durable request ledger: what this host has already answered.
//!
//! One remote control carries a durable request identity, and this ledger is the
//! record of every identity this host has admitted. It exists for one reason:
//! **a duplicate or replayed control must not repeat an effect**. A request
//! whose identity is already recorded is answered from the record — the owner is
//! not asked again — and a request that reuses an identity with different
//! content is refused instead of being treated as fresh work.
//!
//! The record is also where the truth about an outcome lives. [`RequestState`]
//! keeps *the owner accepted the request* apart from *the end was observed*: a
//! request is never proof of exit, an unreachable owner leaves an unknown effect
//! visible as unknown, and a confirmed end is absorbing — a later failure to
//! observe cannot erase an end that was observed.
//!
//! Nothing here persists bytes. [`RemoteControlLedger::durable_record`] is the
//! whole owed set, and rebuilding from it recovers the same identities, states
//! and affected targets; the caller owns the storage.

use super::intent::{RedactedDiagnostics, RemoteWorkIntent, WorkTarget};

/// The schema of [`RemoteControlRecord`].
pub const REMOTE_CONTROL_RECORD_SCHEMA: &str = "licoup.endpoint-remote-control.v1";

/// One durable request identity, taken from the protocol's own request
/// identity. It is an identity, not authority: two requests that name it are
/// the same request, and the ledger decides what that means.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RequestId(String);

impl RequestId {
    #[must_use]
    pub fn new(identity: impl Into<String>) -> Self {
        Self(identity.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One remote control request: its durable identity and its typed intent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControlRequest {
    id: RequestId,
    intent: RemoteWorkIntent,
    diagnostics: Option<RedactedDiagnostics>,
}

impl ControlRequest {
    #[must_use]
    pub fn new(id: RequestId, intent: RemoteWorkIntent) -> Self {
        Self {
            id,
            intent,
            diagnostics: None,
        }
    }

    /// The same request with the locally produced diagnostics a force control
    /// must carry.
    #[must_use]
    pub fn with_diagnostics(mut self, diagnostics: RedactedDiagnostics) -> Self {
        self.diagnostics = Some(diagnostics);
        self
    }

    #[must_use]
    pub const fn id(&self) -> &RequestId {
        &self.id
    }

    #[must_use]
    pub const fn intent(&self) -> &RemoteWorkIntent {
        &self.intent
    }

    #[must_use]
    pub const fn diagnostics(&self) -> Option<&RedactedDiagnostics> {
        self.diagnostics.as_ref()
    }
}

/// What this host currently knows about one admitted request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestState {
    /// Admitted; the owner has not answered yet.
    Requested,
    /// The owner answered. For an inspect that answer is the observation; for a
    /// stop it is the owner's acknowledgement, which is *not* proof of exit.
    Accepted,
    /// The end was observed by this host.
    Confirmed,
    /// The owner answered but the end was not observed. It stays unconfirmed,
    /// and no elapsed time turns it into a confirmed stop.
    Unconfirmed,
    /// The owner could not be asked, or answered with a failure: whether the
    /// effect happened is unknown and stays visible as unknown.
    Unknown,
    /// The owner refused under its own policy, or could not be asked at all.
    /// Either way the request definitely did not happen, and it stays refused.
    Refused,
}

impl RequestState {
    /// The stable name a caller publishes.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Accepted => "accepted",
            Self::Confirmed => "confirmed",
            Self::Unconfirmed => "unconfirmed",
            Self::Unknown => "unknown",
            Self::Refused => "refused",
        }
    }

    /// Whether this state may be published as an observed end.
    ///
    /// Only [`Self::Confirmed`] may. Every other state — including
    /// [`Self::Accepted`] — says the request was made, not that the work ended.
    #[must_use]
    pub const fn confirms_end(self) -> bool {
        matches!(self, Self::Confirmed)
    }

    /// Whether this state still leaves an effect that may or may not have
    /// happened.
    #[must_use]
    pub const fn is_uncertain(self) -> bool {
        matches!(self, Self::Unknown | Self::Unconfirmed)
    }
}

/// One admitted request, with the answer this host holds for it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmittedControl {
    request: ControlRequest,
    state: RequestState,
    affected: Vec<WorkTarget>,
}

impl AdmittedControl {
    #[must_use]
    pub const fn request(&self) -> &ControlRequest {
        &self.request
    }

    #[must_use]
    pub const fn state(&self) -> RequestState {
        self.state
    }

    /// Exactly the work this host recorded as selected by the request.
    #[must_use]
    pub fn affected(&self) -> &[WorkTarget] {
        &self.affected
    }

    #[must_use]
    pub const fn diagnostics(&self) -> Option<&RedactedDiagnostics> {
        self.request.diagnostics()
    }
}

/// What one admission did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Admission {
    /// The identity was not held: it is now admitted at [`RequestState::Requested`].
    Admitted,
    /// The identity was already held with the same intent. The recorded state is
    /// the whole answer, and no owner is asked again.
    Replayed {
        state: RequestState,
        affected: Vec<WorkTarget>,
    },
    /// The identity was already held with different content. This is not the
    /// same request, and treating it as new work would let a second delivery
    /// reuse an answered identity.
    Conflicting,
}

/// Why the ledger refused a write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LedgerRefusal {
    /// No admitted request carries this identity.
    NotAdmitted,
    /// The recorded state does not allow this one.
    IllegalTransition {
        from: RequestState,
        to: RequestState,
    },
}

impl LedgerRefusal {
    /// The stable reason a caller publishes.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::NotAdmitted => "endpoint_remote_control_request_not_admitted",
            Self::IllegalTransition { .. } => "endpoint_remote_control_state_transition_refused",
        }
    }
}

/// The durable projection of every request this host has answered.
///
/// It is the whole persisted state of the ledger: rebuilding from this record
/// recovers the same identities, states and affected targets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteControlRecord {
    schema: String,
    generation: u64,
    admitted: Vec<AdmittedControl>,
}

impl RemoteControlRecord {
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub fn admitted(&self) -> &[AdmittedControl] {
        &self.admitted
    }

    /// Whether the record carries the schema this build writes.
    #[must_use]
    pub fn schema_matches(&self) -> bool {
        self.schema == REMOTE_CONTROL_RECORD_SCHEMA
    }
}

/// The remote control requests this host has admitted.
#[derive(Clone, Debug)]
pub struct RemoteControlLedger {
    generation: u64,
    admitted: Vec<AdmittedControl>,
}

impl Default for RemoteControlLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl RemoteControlLedger {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            generation: 0,
            admitted: Vec::new(),
        }
    }

    /// The ledger a previous run recorded.
    ///
    /// The record is taken as it was written: recovery neither invents nor drops
    /// an admitted request.
    #[must_use]
    pub fn restored(record: RemoteControlRecord) -> Self {
        Self {
            generation: record.generation,
            admitted: record.admitted,
        }
    }

    /// The durable projection of every request this host has answered.
    #[must_use]
    pub fn durable_record(&self) -> RemoteControlRecord {
        RemoteControlRecord {
            schema: REMOTE_CONTROL_RECORD_SCHEMA.to_owned(),
            generation: self.generation,
            admitted: self.admitted.clone(),
        }
    }

    /// The durable generation of the admitted set.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub fn admitted(&self) -> &[AdmittedControl] {
        &self.admitted
    }

    /// The entry one identity holds here.
    #[must_use]
    pub fn entry(&self, id: &RequestId) -> Option<&AdmittedControl> {
        self.admitted.iter().find(|entry| &entry.request.id == id)
    }

    /// Whether one identity has already been answered.
    #[must_use]
    pub fn is_admitted(&self, id: &RequestId) -> bool {
        self.entry(id).is_some()
    }

    /// Admit one request, or answer it from what is already recorded.
    pub(crate) fn admit(&mut self, request: ControlRequest) -> Admission {
        if let Some(entry) = self.admitted.iter().find(|e| e.request.id == request.id) {
            if entry.request.intent == request.intent {
                return Admission::Replayed {
                    state: entry.state,
                    affected: entry.affected.clone(),
                };
            }
            return Admission::Conflicting;
        }
        self.generation += 1;
        self.admitted.push(AdmittedControl {
            request,
            state: RequestState::Requested,
            affected: Vec::new(),
        });
        Admission::Admitted
    }

    /// Record what this host now knows about one admitted request.
    pub(crate) fn record(
        &mut self,
        id: &RequestId,
        state: RequestState,
        mut affected: Vec<WorkTarget>,
    ) -> Result<(), LedgerRefusal> {
        let Some(entry) = self.admitted.iter_mut().find(|e| &e.request.id == id) else {
            return Err(LedgerRefusal::NotAdmitted);
        };
        if !transition_allowed(entry.state, state) {
            return Err(LedgerRefusal::IllegalTransition {
                from: entry.state,
                to: state,
            });
        }
        affected.sort();
        affected.dedup();
        entry.state = state;
        if !affected.is_empty() {
            entry.affected = affected;
        }
        Ok(())
    }
}

/// Whether one recorded state may move to another.
///
/// The two rules that matter: a confirmed end is absorbing, so nothing can turn
/// an observed end back into an unobserved one, and a refused request stays
/// refused, so a later delivery cannot re-open it as if it had been admitted.
fn transition_allowed(from: RequestState, to: RequestState) -> bool {
    if from == to {
        return true;
    }
    match from {
        RequestState::Requested => matches!(
            to,
            RequestState::Accepted
                | RequestState::Confirmed
                | RequestState::Unconfirmed
                | RequestState::Unknown
                | RequestState::Refused
        ),
        RequestState::Accepted => matches!(
            to,
            RequestState::Confirmed | RequestState::Unconfirmed | RequestState::Unknown
        ),
        RequestState::Unconfirmed => {
            matches!(to, RequestState::Confirmed | RequestState::Unknown)
        }
        RequestState::Unknown => matches!(to, RequestState::Confirmed | RequestState::Unconfirmed),
        RequestState::Confirmed | RequestState::Refused => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ControlRequest, LedgerRefusal, REMOTE_CONTROL_RECORD_SCHEMA, RemoteControlLedger,
        RequestId, RequestState, transition_allowed,
    };
    use crate::control::intent::{RemoteWorkIntent, WorkOwner, WorkTarget};

    fn target(scope: &str) -> WorkTarget {
        WorkTarget::new(WorkOwner::WorkflowRun, scope)
    }

    fn stop(id: &str, scope: &str) -> ControlRequest {
        ControlRequest::new(
            RequestId::new(id),
            RemoteWorkIntent::Stop {
                target: target(scope),
            },
        )
    }

    #[test]
    fn admitting_one_identity_once_and_replaying_it_asks_for_nothing_new() {
        let mut ledger = RemoteControlLedger::new();
        assert_eq!(
            ledger.admit(stop("request-1", "run-1")),
            super::Admission::Admitted
        );
        assert_eq!(ledger.generation(), 1);
        ledger
            .record(
                &RequestId::new("request-1"),
                RequestState::Accepted,
                vec![target("run-1")],
            )
            .expect("recorded");

        let replay = ledger.admit(stop("request-1", "run-1"));
        assert_eq!(
            replay,
            super::Admission::Replayed {
                state: RequestState::Accepted,
                affected: vec![target("run-1")],
            },
            "a re-delivered request is answered from the record"
        );
        assert_eq!(ledger.generation(), 1, "a replay admits nothing new");
    }

    #[test]
    fn reusing_one_identity_for_other_work_is_a_conflict_not_new_work() {
        let mut ledger = RemoteControlLedger::new();
        ledger.admit(stop("request-1", "run-1"));

        assert_eq!(
            ledger.admit(stop("request-1", "run-2")),
            super::Admission::Conflicting
        );
    }

    #[test]
    fn an_end_that_was_observed_is_absorbing() {
        let mut ledger = RemoteControlLedger::new();
        ledger.admit(stop("request-1", "run-1"));
        let id = RequestId::new("request-1");
        ledger
            .record(&id, RequestState::Confirmed, vec![target("run-1")])
            .expect("recorded");

        assert_eq!(
            ledger.record(&id, RequestState::Unconfirmed, Vec::new()),
            Err(LedgerRefusal::IllegalTransition {
                from: RequestState::Confirmed,
                to: RequestState::Unconfirmed,
            }),
            "a lost observation cannot erase an observed end"
        );
        assert_eq!(ledger.entry(&id).unwrap().state(), RequestState::Confirmed);
    }

    #[test]
    fn an_owner_acknowledgement_is_not_an_observed_end() {
        assert!(!RequestState::Accepted.confirms_end());
        assert!(!RequestState::Requested.confirms_end());
        assert!(!RequestState::Unknown.confirms_end());
        assert!(RequestState::Confirmed.confirms_end());
        assert!(RequestState::Unknown.is_uncertain());
        assert!(RequestState::Unconfirmed.is_uncertain());
    }

    #[test]
    fn a_refused_request_cannot_be_reopened_and_an_unadmitted_one_cannot_be_recorded() {
        let mut ledger = RemoteControlLedger::new();
        ledger.admit(stop("request-1", "run-1"));
        let id = RequestId::new("request-1");
        ledger
            .record(&id, RequestState::Refused, Vec::new())
            .expect("recorded");

        assert_eq!(
            ledger.record(&id, RequestState::Confirmed, vec![target("run-1")]),
            Err(LedgerRefusal::IllegalTransition {
                from: RequestState::Refused,
                to: RequestState::Confirmed,
            })
        );
        assert_eq!(
            ledger.record(
                &RequestId::new("request-2"),
                RequestState::Confirmed,
                Vec::new()
            ),
            Err(LedgerRefusal::NotAdmitted)
        );
        assert_eq!(
            LedgerRefusal::NotAdmitted.reason(),
            "endpoint_remote_control_request_not_admitted"
        );
    }

    #[test]
    fn the_durable_record_round_trips_every_admitted_request() {
        let mut ledger = RemoteControlLedger::new();
        ledger.admit(stop("request-1", "run-1"));
        ledger.admit(stop("request-2", "run-2"));
        ledger
            .record(
                &RequestId::new("request-1"),
                RequestState::Confirmed,
                vec![target("run-1")],
            )
            .expect("recorded");

        let record = ledger.durable_record();
        assert!(record.schema_matches());
        assert_eq!(record.generation(), 2);

        let mut restored = RemoteControlLedger::restored(record);
        assert_eq!(restored.generation(), 2);
        assert_eq!(
            restored
                .entry(&RequestId::new("request-1"))
                .unwrap()
                .state(),
            RequestState::Confirmed
        );
        assert_eq!(
            restored.admit(stop("request-1", "run-1")),
            super::Admission::Replayed {
                state: RequestState::Confirmed,
                affected: vec![target("run-1")],
            },
            "a restart answers the re-delivery from the recovered record"
        );
        // The recovered ledger still refuses a conflicting reuse.
        assert_eq!(
            restored.admit(stop("request-2", "run-9")),
            super::Admission::Conflicting
        );
    }

    #[test]
    fn only_the_declared_transitions_are_allowed() {
        assert!(transition_allowed(
            RequestState::Requested,
            RequestState::Accepted
        ));
        assert!(transition_allowed(
            RequestState::Unconfirmed,
            RequestState::Confirmed
        ));
        assert!(transition_allowed(
            RequestState::Unknown,
            RequestState::Unconfirmed
        ));
        assert!(!transition_allowed(
            RequestState::Accepted,
            RequestState::Requested
        ));
        assert!(!transition_allowed(
            RequestState::Refused,
            RequestState::Unknown
        ));
        assert_eq!(
            REMOTE_CONTROL_RECORD_SCHEMA,
            "licoup.endpoint-remote-control.v1"
        );
    }
}
