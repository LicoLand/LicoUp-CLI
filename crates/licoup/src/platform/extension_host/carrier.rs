//! The carrier seam: how the host starts, handshakes and talks to one extension
//! process, and which failures isolate it.
//!
//! A carrier is the transport half of C09. The host owns *when* an instance is
//! prepared, which generation a call is admitted to, and what a fault means; the
//! carrier owns *how* the frames travel. Keeping that split is what lets the
//! isolation carrier plug real subprocesses under OS confinement into the same
//! host without changing the catalog, the generation rules or the error chain.
//!
//! The trait is deliberately small and synchronous-per-call: the host never
//! calls a carrier while holding its own state lock, so a carrier that hangs
//! occupies its own call and nothing else. The three lifecycle calls
//! (`start`/`initialize`/`ready`) happen during preparation, before any catalog
//! commit; `dispatch`/`observe`/`cancel`/`result` carry the invocation binding
//! the host issued, so a carrier cannot invent an invocation identity.
//!
//! **Faults are classified, not inferred.** A carrier that cannot answer, has
//! crashed, exceeded its budget or answered a malformed frame returns one of
//! [`carrier_fault`]'s codes. [`classify_failure`] maps exactly those codes to a
//! [`FaultClass`]; every other failure is an ordinary refusal from the extension
//! and does not take the instance down. That asymmetry is the fault-domain
//! boundary: an extension saying "invalid request" is not an extension that must
//! be quarantined, and an unresponsive extension must not be allowed to block
//! every other capability.
//!
//! **No confinement claim.** An in-process carrier cannot stop an extension from
//! touching what the client can touch, and nothing in this module says
//! otherwise. Quarantine here means "this instance stops being routed and its
//! unsettled work is recorded unknown"; it never means "the effects it already
//! performed were undone".
//!
//! ## What a wire implementation owns
//!
//! This port is in-process; the JSON-RPC frames a generic adapter defines travel
//! inside a wire implementation of it, and that implementation owns the mapping.
//! Four identities must stay straight, because the host cannot check them from
//! here:
//!
//! - **An external `invocationRef` is not a host binding.** The wire may name
//!   invocations however it likes, but each ref must map to exactly the
//!   [`super::invocation::InvocationBinding`] it received for that call, and
//!   `observe`/`cancel`/`result` must be answered for that binding. A ref it did
//!   not receive from *this* host run — persisted, replayed or guessed — is not
//!   work: `agent.resume`/`agent.history` answer from the wire's own replay
//!   state, and never re-dispatch.
//! - **Admitted is not finished.** `agent.execute`'s `accepted` maps to
//!   [`DispatchOutcome::Admitted`]; only a terminal event or a result settles
//!   the invocation.
//! - **Cancellation is a request.** `requested`/`acknowledged`/`unsupported`/
//!   `unknown` map to [`CancelDisposition`]; none of them settles.
//! - **A missing session is not a stopped process.** A wire reports a release
//!   only after observing process exit (exit status, signal), and it keeps
//!   whatever handle it needs to kill a process it has not confirmed dead. The
//!   host keeps the matching catalogue fact as
//!   [`super::catalog::SessionOwner`] and will not treat an unverified stop as
//!   cleanup evidence.
//!
//! A host restart changes the incarnation, so no ref from a previous run is
//! admissible even if the wire still remembers it: the host refuses the handle
//! before it looks at any ref.

use std::sync::Arc;

use licoup_application::{ApplicationFailure, ContractRange};
use licoup_extension_contracts::profile::{DeclaredMethods, ProfileDeclaration};
use serde_json::Value;

use super::{refusal, uncertain};

/// Everything a carrier needs to start one instance, before any commit.
///
/// The registry epoch is deliberately absent: it does not exist until the
/// activation commits. A carrier learns the epoch from the invocations it is
/// given, not from its start request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CarrierSpec {
    pub package_id: String,
    pub package_version: String,
    pub instance_id: String,
    pub generation: u64,
    /// The published profile ids this instance is being prepared for.
    pub profiles: Vec<String>,
}

/// The opaque handle one carrier returns for its started instance.
///
/// The host stores it and hands it back to the same carrier; it never inspects
/// it. A carrier chooses its own session type and reaches it through
/// [`CarrierSession::get`], which keeps this seam object-safe without leaking a
/// carrier's internals into the catalog.
#[derive(Clone)]
pub struct CarrierSession {
    inner: Arc<dyn std::any::Any + Send + Sync>,
}

impl CarrierSession {
    pub fn new<T: std::any::Any + Send + Sync>(inner: T) -> Self {
        Self {
            inner: Arc::new(inner),
        }
    }

    /// The carrier's own session value, when this is that carrier's session.
    pub fn get<T: std::any::Any + Send + Sync>(&self) -> Option<Arc<T>> {
        Arc::clone(&self.inner).downcast::<T>().ok()
    }
}

impl std::fmt::Debug for CarrierSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("CarrierSession(..)")
    }
}

/// The `extension.initialize` request: the host's range and the profile set.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitializeRequest {
    pub host_contract_range: ContractRange,
    pub profiles: Vec<ProfileDeclaration>,
}

/// What `extension.initialize` answered: the methods this instance really
/// implements, and which declared profiles it accepted.
///
/// The manifest's `DeclaredMethods` were a claim; these are the runtime's own
/// statement after the handshake, and the host uses them for the live profile
/// decision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitializedProfileSet {
    pub accepted_profiles: Vec<String>,
    pub methods: DeclaredMethods,
}

/// What one dispatch produced.
#[derive(Clone, Debug, PartialEq)]
pub enum DispatchOutcome {
    /// The extension accepted the work. Completion arrives through
    /// `observe`/`result`, and the invocation stays in flight until then.
    Admitted,
    /// The extension finished the work in the same call.
    Completed { payload: Value },
    /// The extension's own natural reply, carried verbatim. Nothing here parses
    /// it, and a body that looks like a broken envelope is still text.
    Natural(licoup_application::NaturalOutput),
}

/// What one look at an in-flight invocation observed.
#[derive(Clone, Debug, PartialEq)]
pub enum Observation {
    /// Still running. The invocation stays in flight.
    Running,
    /// Finished with this payload.
    Completed { payload: Value },
    /// The extension cannot say whether the effect happened. It is recorded
    /// unknown and never re-dispatched.
    Unknown { code: String },
    /// The extension's natural reply, carried verbatim.
    Natural(licoup_application::NaturalOutput),
}

/// What one cancellation request produced, in C09's own vocabulary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancelDisposition {
    /// The request was delivered; acknowledgement may follow.
    Requested,
    /// The extension acknowledged the cancellation. This is not a settlement:
    /// the result still decides what happened.
    Acknowledged,
    /// The extension does not implement cancellation. Visible, never simulated.
    Unsupported,
    /// The extension cannot say what happened to the work. Recorded unknown.
    Unknown,
}

/// How a carrier failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FaultClass {
    /// The extension stopped answering within its call.
    Unresponsive,
    /// The extension process died.
    Crashed,
    /// The extension exceeded a declared budget dimension.
    OverBudget,
    /// The extension's authority was revoked while it was running.
    Revoked,
    /// The extension answered something that is not the protocol.
    Protocol,
}

/// The failure one carrier fault raises.
///
/// The code is fixed per class so a caller can classify it without parsing a
/// message, and the class is also published as a presentation argument so a
/// surface can show why the instance stopped being routed.
pub fn carrier_fault(class: FaultClass) -> ApplicationFailure {
    let (code, stage) = match class {
        FaultClass::Unresponsive => ("extension_carrier_unresponsive", "extension/carrier"),
        FaultClass::Crashed => ("extension_carrier_crashed", "extension/carrier"),
        FaultClass::OverBudget => ("extension_carrier_over_budget", "extension/carrier"),
        FaultClass::Revoked => ("extension_carrier_revoked", "extension/carrier"),
        FaultClass::Protocol => ("extension_carrier_protocol", "extension/carrier"),
    };
    uncertain(code, stage).with_presentation_arg("fault", fault_name(class))
}

/// The stable name of one fault class, for a presentation argument or a note.
pub const fn fault_name(class: FaultClass) -> &'static str {
    match class {
        FaultClass::Unresponsive => "unresponsive",
        FaultClass::Crashed => "crashed",
        FaultClass::OverBudget => "over-budget",
        FaultClass::Revoked => "revoked",
        FaultClass::Protocol => "protocol",
    }
}

/// Classify a carrier failure.
///
/// Exactly the five fault codes produced by [`carrier_fault`] are faults. A
/// refusal the extension itself returned — an invalid request, a refused
/// capability — is not a fault and must not stop the instance, because an
/// extension that says "no" to one call is still the extension that serves the
/// next one.
pub fn classify_failure(failure: &ApplicationFailure) -> Option<FaultClass> {
    match failure.code.as_str() {
        "extension_carrier_unresponsive" => Some(FaultClass::Unresponsive),
        "extension_carrier_crashed" => Some(FaultClass::Crashed),
        "extension_carrier_over_budget" => Some(FaultClass::OverBudget),
        "extension_carrier_revoked" => Some(FaultClass::Revoked),
        "extension_carrier_protocol" => Some(FaultClass::Protocol),
        _ => None,
    }
}

/// The port one extension carrier implements.
///
/// Every method takes the session the carrier itself created, so a carrier owns
/// its own state and this trait stays object-safe. The host calls no carrier
/// method while holding its state lock.
pub trait ExtensionCarrier: Send + Sync {
    /// Start the instance's transport. No business call happens here.
    fn start(&self, spec: &CarrierSpec) -> Result<CarrierSession, ApplicationFailure>;

    /// `extension.initialize`: negotiate the host protocol and the profile set.
    fn initialize(
        &self,
        session: &CarrierSession,
        request: &InitializeRequest,
    ) -> Result<InitializedProfileSet, ApplicationFailure>;

    /// `extension.ready`: the instance is prepared and may be committed.
    fn ready(&self, session: &CarrierSession) -> Result<(), ApplicationFailure>;

    /// One admitted invocation.
    fn dispatch(
        &self,
        session: &CarrierSession,
        binding: &super::invocation::InvocationBinding,
        request: &Value,
    ) -> Result<DispatchOutcome, ApplicationFailure>;

    /// One look at an in-flight invocation, bound to the generation that
    /// admitted it.
    fn observe(
        &self,
        session: &CarrierSession,
        binding: &super::invocation::InvocationBinding,
    ) -> Result<Observation, ApplicationFailure>;

    /// Request cancellation, pinned to the original generation.
    fn cancel(
        &self,
        session: &CarrierSession,
        binding: &super::invocation::InvocationBinding,
    ) -> Result<CancelDisposition, ApplicationFailure>;

    /// The result of an invocation, pinned to the original generation.
    fn result(
        &self,
        session: &CarrierSession,
        binding: &super::invocation::InvocationBinding,
    ) -> Result<Observation, ApplicationFailure>;

    /// `extension.shutdown`: release the instance's transport.
    fn shutdown(&self, session: &CarrierSession) -> Result<(), ApplicationFailure>;
}

/// A carrier that cannot start anything, for callers that never activate an
/// extension: the desktop with no optional package installed still gets a
/// catalog that says so.
pub struct NoCarrier;

impl ExtensionCarrier for NoCarrier {
    fn start(&self, _: &CarrierSpec) -> Result<CarrierSession, ApplicationFailure> {
        Err(refusal(
            "extension_carrier_unavailable",
            "extension/carrier",
        ))
    }

    fn initialize(
        &self,
        _: &CarrierSession,
        _: &InitializeRequest,
    ) -> Result<InitializedProfileSet, ApplicationFailure> {
        Err(refusal(
            "extension_carrier_unavailable",
            "extension/carrier",
        ))
    }

    fn ready(&self, _: &CarrierSession) -> Result<(), ApplicationFailure> {
        Err(refusal(
            "extension_carrier_unavailable",
            "extension/carrier",
        ))
    }

    fn dispatch(
        &self,
        _: &CarrierSession,
        _: &super::invocation::InvocationBinding,
        _: &Value,
    ) -> Result<DispatchOutcome, ApplicationFailure> {
        Err(refusal(
            "extension_carrier_unavailable",
            "extension/carrier",
        ))
    }

    fn observe(
        &self,
        _: &CarrierSession,
        _: &super::invocation::InvocationBinding,
    ) -> Result<Observation, ApplicationFailure> {
        Err(refusal(
            "extension_carrier_unavailable",
            "extension/carrier",
        ))
    }

    fn cancel(
        &self,
        _: &CarrierSession,
        _: &super::invocation::InvocationBinding,
    ) -> Result<CancelDisposition, ApplicationFailure> {
        Err(refusal(
            "extension_carrier_unavailable",
            "extension/carrier",
        ))
    }

    fn result(
        &self,
        _: &CarrierSession,
        _: &super::invocation::InvocationBinding,
    ) -> Result<Observation, ApplicationFailure> {
        Err(refusal(
            "extension_carrier_unavailable",
            "extension/carrier",
        ))
    }

    fn shutdown(&self, _: &CarrierSession) -> Result<(), ApplicationFailure> {
        Err(refusal(
            "extension_carrier_unavailable",
            "extension/carrier",
        ))
    }
}
