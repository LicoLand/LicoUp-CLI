//! C01 `EffectPort` shape over the existing native work-context adapters.
//!
//! This is a bridge, not an executor. Nothing here invokes an agent on its own:
//! delivery stays with the adapter that already owns it ([`EffectDispatch`]), the
//! session's writer claim stays with the existing work-context owner
//! ([`EffectSessionOwner`]), and the per-vendor capability facts come from the
//! live runtime Agent profile instead of a vendor match in code.
//!
//! Four rules are structural here rather than promised:
//!
//! * **Capabilities are negotiated, never assumed.** Every answer carries both
//!   inputs it was derived from: what the runtime Agent profile declared, and what
//!   the bound adapter's own snapshot reports. A flag the profile does not carry
//!   is reported absent, and no source may raise another one's claim. An
//!   operation whose meaning no declaration matches is absent rather than
//!   inferred from a neighbouring flag.
//! * **An unanswered capability may be attempted; a refused one is not.**
//!   `Unverified` is the absence of a live answer, not a refusal, so the request
//!   is sent and the adapter's own answer becomes the fact. `Unsupported` and
//!   `TemporarilyUnavailable` are live statements that the channel is not usable,
//!   so nothing is sent. This is what keeps a declared vendor capability
//!   reachable while an adapter that has not yet answered keeps its honesty.
//! * **A cancel is a request.** [`CancelOutcome`] has no variant meaning "the
//!   effect was cancelled"; after an accepted request the effect's state is
//!   [`EffectState::Unknown`] with [`EffectUnknownReason::CancelUnconfirmed`],
//!   because an external effect that already happened is not undone by asking it
//!   to stop. Only a fact an adapter reported can close an effect. A request
//!   whose delivery cannot be established stays unknown: a call error or an
//!   unrecognised answer is never reported as a delivery, and only a surface
//!   that established the request never left the process may report it as not
//!   reached.
//! * **An in-flight instance is never replaced.** `submit` takes the session
//!   writer claim from the owner, so a second instance for the same session is
//!   refused by the owner itself until the effect reaches a confirmed terminal
//!   fact and the caller settles it.
//!
//! The bound adapter keeps its identity: this module consumes
//! `NativeWorkContextPort::negotiate` and does not rewrite an adapter into this
//! port shape. The bridge holds no session map of its own, so two bridges over
//! one owner conflict exactly like two direct callers of that owner.

use std::collections::BTreeMap;
use std::sync::Arc;

use licoup_agent_runtime::work_context::{
    ContinuityFailureCode, NativeCapabilitySnapshot, NativeCapabilitySupport,
    NativeWorkContextFailure, NativeWorkContextKey, NativeWorkContextPort, WorkContextRuntime,
};
use serde_json::Value;

/// One effect operation C01 names.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum EffectOperation {
    Submit,
    Observe,
    Cancel,
    Steer,
}

impl EffectOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Submit => "submit",
            Self::Observe => "observe",
            Self::Cancel => "cancel",
            Self::Steer => "steer",
        }
    }

    /// The runtime Agent profile flag that declares this operation, when the
    /// packaged driver inventory carries one.
    ///
    /// The names are the driver-inventory flags, so the answer for a vendor is
    /// data read at runtime; no vendor name appears in this bridge.
    ///
    /// `None` means the inventory has no flag whose meaning is this operation,
    /// and the answer is then absent rather than inferred from a neighbouring
    /// flag. `observe` asks for the adapter's own record of one attempt;
    /// `structuredEvents` describes events delivered during a turn, and the
    /// production lane holds no per-attempt record, so reporting `observe` as
    /// supported from it would be a claim the bound delivery surface cannot
    /// honour. A future profile that declares a real per-attempt read-back gets
    /// its flag here.
    pub fn declaration_flag(self) -> Option<&'static str> {
        match self {
            Self::Submit => Some("openNew"),
            Self::Observe => None,
            Self::Cancel => Some("cancel"),
            Self::Steer => Some("interruptSteer"),
        }
    }
}

/// One negotiated operation, with the evidence it was derived from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EffectCapability {
    /// The weaker of the two sources below. `Unsupported` here means absent, not
    /// "assumed absent": `declared` and `negotiated` say where it came from.
    /// A support below `Supported` is not automatically a refusal;
    /// [`EffectCapability::permits_attempt`] decides whether an attempt may be
    /// made.
    pub support: NativeCapabilitySupport,
    /// What the runtime Agent profile declared for this operation. `None` means
    /// the profile carries no such flag at all.
    pub declared: Option<bool>,
    /// What the bound adapter's own snapshot reports for this operation. `None`
    /// means the snapshot has no such dimension: it is not a confirmation.
    pub negotiated: Option<NativeCapabilitySupport>,
}

impl EffectCapability {
    /// Nothing declared it and no bound adapter confirmed it.
    pub fn absent() -> Self {
        Self {
            support: NativeCapabilitySupport::Unsupported,
            declared: None,
            negotiated: None,
        }
    }

    /// Whether the evidence permits attempting the operation now.
    ///
    /// `Supported` and `Unverified` permit an attempt: `Unverified` is the
    /// absence of a live answer, not a refusal, and the attempt itself is what
    /// produces the adapter's answer. `Unsupported` and `TemporarilyUnavailable`
    /// are live statements that the channel is not usable, so nothing is sent.
    pub fn permits_attempt(self) -> bool {
        matches!(
            self.support,
            NativeCapabilitySupport::Supported | NativeCapabilitySupport::Unverified
        )
    }
}

/// What the port answers for one dynamic instance and one bound session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectCapabilities {
    pub agent_id: String,
    pub driver_id: String,
    operations: BTreeMap<EffectOperation, EffectCapability>,
}

impl EffectCapabilities {
    pub fn capability(&self, operation: EffectOperation) -> EffectCapability {
        self.operations
            .get(&operation)
            .copied()
            .unwrap_or_else(EffectCapability::absent)
    }

    pub fn support(&self, operation: EffectOperation) -> NativeCapabilitySupport {
        self.capability(operation).support
    }
}

/// One dynamic instance, as the runtime Agent registry describes it.
///
/// This is data: the same code path serves every agent, and an agent that
/// declares nothing gets nothing assumed for it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeAgentProfile {
    pub agent_id: String,
    pub driver_id: String,
    pub runtime_protocol: String,
    pub lane_family: Option<String>,
    /// The profile's own readiness projection, reported as it stands. Capability
    /// support is negotiated from declarations; this status is evidence about the
    /// lane's release qualification and is not silently folded into that answer.
    pub readiness: String,
    pub blocker: Option<String>,
    /// Declared adapter flags, verbatim from the runtime profile.
    pub declarations: BTreeMap<String, bool>,
}

impl RuntimeAgentProfile {
    /// `None` means the profile does not carry the flag at all. Absent is not
    /// support, and it is not a refusal either: it is an absence.
    pub fn declares(&self, flag: &str) -> Option<bool> {
        self.declarations.get(flag).copied()
    }
}

/// Where dynamic instances come from. Production reads the live runtime Agent
/// registry; a test can supply any profile data it wants.
pub trait AgentProfileSource: Send + Sync {
    /// `None` when the registry has no such instance. Never a fabricated default.
    fn profile(&self, agent_id: &str) -> Option<RuntimeAgentProfile>;
}

/// The turn handles an adapter needs to address one execution instance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectTurn {
    /// The host-side handle the caller holds for this turn.
    pub host_handle: String,
    /// The vendor's own session identifier: the exact native session this effect
    /// continues. Never invented from the conversation identity.
    pub native_session_id: String,
    /// The vendor's own identifier for the turn being controlled, when one is in
    /// flight.
    pub native_turn_id: String,
}

/// One effect the caller wants admitted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectInvocation {
    /// C07 correlation identity, carried onto every state this port records.
    pub effect_id: String,
    pub attempt_token: String,
    /// The dynamic instance to consume, resolved through the runtime Agent profile.
    pub agent_id: String,
    pub session: NativeWorkContextKey,
    pub turn: EffectTurn,
    pub input: Value,
}

/// Why an effect has no terminal fact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectUnknownReason {
    /// The adapter answered without a recorded result.
    NoRecordedResult,
    /// The adapter holds no read-back channel in this process.
    StatusChannelUnavailable,
    /// A cancel was requested and the adapter answered; acceptance is not a
    /// terminal fact.
    CancelUnconfirmed,
    /// A control request was attempted and no answer could be established:
    /// whether it reached the adapter is unknown.
    ControlUnconfirmed,
    /// The adapter could not be reached at all.
    AdapterUnreachable,
}

impl EffectUnknownReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoRecordedResult => "no_recorded_result",
            Self::StatusChannelUnavailable => "status_channel_unavailable",
            Self::CancelUnconfirmed => "cancel_unconfirmed",
            Self::ControlUnconfirmed => "control_unconfirmed",
            Self::AdapterUnreachable => "adapter_unreachable",
        }
    }
}

/// C03 effect states. Every one of them is reachable, and an unknown stays
/// unknown: this port never upgrades it to a terminal state by guessing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EffectState {
    /// Admitted and delivered; no terminal fact yet.
    InFlight,
    /// The adapter reported the attempt reached a terminal settlement with a
    /// result. `output` is the adapter's own report, verbatim: a caller that needs
    /// a semantic verdict reads the report, this port does not invent one.
    Succeeded { output: Value },
    /// The adapter reported the attempt ended in failure.
    Failed { code: String, retryable: bool },
    /// A confirmed fact that the external effect did not happen. This is the only
    /// state that may justify a general re-dispatch, and only an adapter that
    /// reported it can produce it.
    NotExecuted,
    /// The attempt ended by cancellation. This says nothing about whether part of
    /// the external effect already happened.
    Cancelled,
    /// No terminal fact is held.
    Unknown { reason: EffectUnknownReason },
}

impl EffectState {
    /// Whether a fact, not a request, closed this effect.
    pub fn is_confirmed_terminal(&self) -> bool {
        matches!(
            self,
            Self::Succeeded { .. } | Self::Failed { .. } | Self::NotExecuted | Self::Cancelled
        )
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown { .. })
    }
}

/// What an adapter reported for one call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeliveryOutcome {
    /// The attempt completed and materialised this result.
    Completed { output: Value },
    /// The adapter reports the attempt ended without the effect happening.
    NotExecuted,
    /// The adapter reports the attempt ended by cancellation.
    Cancelled,
    /// The adapter reports the attempt ended in failure.
    Failed { code: String, retryable: bool },
    /// The adapter accepted the request but reported no terminal fact.
    Accepted,
    /// No fact this port may treat as terminal.
    Unconfirmed { reason: EffectUnknownReason },
}

/// One delivery, with the adapter's own status token preserved for receipts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectDelivery {
    pub outcome: DeliveryOutcome,
    pub status: String,
}

impl EffectDelivery {
    pub fn unconfirmed(status: impl Into<String>, reason: EffectUnknownReason) -> Self {
        Self {
            outcome: DeliveryOutcome::Unconfirmed { reason },
            status: status.into(),
        }
    }
}

/// The control channels an effect can be steered or cancelled over.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EffectControl {
    Steer,
    Cancel,
}

/// What a control surface reported for one request.
///
/// The distinction between an answer, a pre-dispatch refusal, and an
/// unestablished request is the whole point of this type: only a surface that
/// established the request never left the process may report
/// [`ControlDisposition::NotDelivered`], and a call error or an unrecognised
/// answer is [`ControlDisposition::Unconfirmed`], never a delivery claim.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlDisposition {
    /// The adapter accepted the request for the active turn.
    Accepted,
    /// The adapter reports no active turn for the session.
    NoActiveTurn,
    /// The adapter reports the session is not bound to this process.
    SessionUnavailable,
    /// The adapter reports it does not expose this control channel. The answer
    /// is established; nothing is in effect.
    Unsupported,
    /// The surface established that the request never left this process: it was
    /// refused before any delivery attempt, so nothing was asked of the adapter.
    NotDelivered,
    /// The request was attempted, but no answer can be established. It may have
    /// reached the adapter and may have been acted on; nothing may claim it did
    /// or did not.
    Unconfirmed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControlDelivery {
    pub disposition: ControlDisposition,
    pub status: String,
}

/// The existing adapter surface, injected so the bridge adds only port semantics.
///
/// A production implementation is the same lane entry the strategy actor effect
/// already calls; a test implementation can report any outcome it likes.
pub trait EffectDispatch: Send + Sync {
    /// Deliver one effect to the resolved dynamic instance.
    fn deliver(&self, invocation: &EffectInvocation) -> EffectDelivery;

    /// Read back the adapter's own record of an attempt. `None` means the adapter
    /// holds no record, which is not the same fact as failure.
    fn read_back(&self, handle: &EffectHandle) -> Option<EffectDelivery>;

    /// Deliver a control request for the attempt the handle names.
    fn control(
        &self,
        handle: &EffectHandle,
        control: EffectControl,
        instruction: Option<&str>,
    ) -> ControlDelivery;
}

/// An admitted effect, bound to the instance it was admitted on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectHandle {
    pub effect_id: String,
    pub attempt_token: String,
    /// The dynamic instance, resolved through the runtime Agent profile at
    /// admission time.
    pub agent_id: String,
    pub driver_id: String,
    pub runtime_protocol: String,
    pub session: NativeWorkContextKey,
    pub turn: EffectTurn,
    /// What the adapter reported at admission. Never a guess.
    pub state: EffectState,
}

/// Why an effect was not admitted. Every variant leaves the effect unstarted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EffectRefusal {
    /// Not shaped like an effect at all: nothing was claimed and nothing delivered.
    InvalidInvocation { reason: &'static str },
    /// The runtime Agent registry has no such dynamic instance.
    UnknownInstance { agent_id: String },
    /// The work-context owner refused the session (its own admission failure).
    SessionNotAdmitted { failure: NativeWorkContextFailure },
    /// The operation's negotiated capability is not usable: absent, temporarily
    /// unavailable, or refuted by the bound adapter. Nothing was delivered;
    /// `capability` carries the evidence for which of those it is.
    CapabilityUnavailable {
        operation: EffectOperation,
        capability: EffectCapability,
    },
    /// The session already has a writer. An in-flight execution instance is never
    /// replaced; carries the owner's own refusal.
    InFlightInstance { failure: NativeWorkContextFailure },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SubmitOutcome {
    Admitted(Box<EffectHandle>),
    Refused(EffectRefusal),
}

/// What the adapter currently reports for an attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EffectObservation {
    pub state: EffectState,
    /// The adapter's status token, or this port's token when the adapter has none.
    pub status: String,
}

/// The answer to a cancel request.
///
/// There is deliberately no "cancelled" variant: no answer to a request may be
/// recorded as the effect's completion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CancelOutcome {
    /// The control channel is not usable: the evidence refutes it before
    /// sending, or the adapter itself answered `unsupported`. No cancellation is
    /// in effect.
    Unavailable { capability: EffectCapability },
    /// The request reached the adapter and it answered. `disposition` is that
    /// answer; `state` is what the effect's record must say afterwards, which
    /// stays unknown until a separate confirmed fact arrives.
    Requested {
        disposition: ControlDisposition,
        state: EffectState,
    },
    /// No answer could be established for the request. It may have reached the
    /// adapter and may have been acted on; `state` stays unknown, the session
    /// claim stays held, and nothing is re-delivered.
    Unconfirmed {
        disposition: ControlDisposition,
        state: EffectState,
    },
    /// The surface established that the request never left this process. Nothing
    /// was asked of the adapter, so nothing is in effect.
    NotReached { disposition: ControlDisposition },
    /// A confirmed terminal fact already closed this effect; a request cannot
    /// rewrite it.
    AlreadyTerminal { state: EffectState },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SteerOutcome {
    /// The control channel is not usable: the evidence refutes it before
    /// sending, or the adapter itself answered `unsupported`. Nothing was
    /// applied.
    Unavailable { capability: EffectCapability },
    /// The request reached the adapter and it answered. `Accepted` means the
    /// instruction reached a live turn; the other dispositions are the adapter's
    /// own answer. Steering settles nothing, so the effect's state is unchanged
    /// by it.
    Delivered { disposition: ControlDisposition },
    /// No answer could be established for the request. It may have reached the
    /// adapter and may have been acted on; nothing may claim either way.
    Unconfirmed { disposition: ControlDisposition },
    /// The surface established that the request never left this process.
    NotReached { disposition: ControlDisposition },
    /// A confirmed terminal fact already closed this effect.
    AlreadyTerminal { state: EffectState },
    /// The instruction is not a steer; nothing was sent.
    InvalidInstruction,
}

/// The answer to a read-back of an effect whose outcome was in doubt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReconcileOutcome {
    /// A confirmed terminal fact was read back. `settlement` says what happened
    /// to the session claim: a release the owner refused is reported as
    /// `Retained`, never as a released claim.
    Settled {
        state: EffectState,
        status: String,
        settlement: Settlement,
    },
    /// No confirmed fact: the effect stays in doubt, the claim stays held, and
    /// nothing was re-delivered.
    InDoubt { state: EffectState, status: String },
}

/// What happened to the owner's writer claim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Settlement {
    /// The owner released its claims.
    Released,
    /// The claims stay in place, either because no confirmed fact exists or
    /// because the owner refused the release.
    Retained { state: EffectState },
}

/// The one owner of session exclusivity for native work.
///
/// The bridge holds no session map: every exclusivity answer, and every release,
/// comes from here. `WorkContextRuntime` is the existing owner.
pub trait EffectSessionOwner: Send + Sync {
    /// The bound adapter's own negotiation for a session.
    fn negotiate(
        &self,
        key: &NativeWorkContextKey,
    ) -> Result<NativeCapabilitySnapshot, NativeWorkContextFailure>;

    /// Take the session's writer claim.
    fn claim_writer(&self, key: &NativeWorkContextKey) -> Result<(), NativeWorkContextFailure>;

    /// Release the owner's writer claims.
    ///
    /// The owner releases wholesale; that is its own existing semantic and this
    /// port does not pretend to a finer one. The bridge only calls it once an
    /// effect holds a confirmed terminal fact, so an effect that is still in
    /// doubt keeps every claim in place.
    fn release_writer(&self) -> Result<(), NativeWorkContextFailure>;
}

/// The existing work-context owner, reused rather than reimplemented.
impl EffectSessionOwner for WorkContextRuntime {
    fn negotiate(
        &self,
        key: &NativeWorkContextKey,
    ) -> Result<NativeCapabilitySnapshot, NativeWorkContextFailure> {
        NativeWorkContextPort::negotiate(self, key)
    }

    fn claim_writer(&self, key: &NativeWorkContextKey) -> Result<(), NativeWorkContextFailure> {
        NativeWorkContextPort::claim_writer(self, key)
    }

    fn release_writer(&self) -> Result<(), NativeWorkContextFailure> {
        WorkContextRuntime::release_writer(self)
    }
}

/// The prompt text one effect input carries, by the same rule the strategy actor
/// effect already uses: an explicit `prompt` field, else the serialised input.
pub fn effect_input_text(input: &Value) -> String {
    input
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| serde_json::to_string(input).unwrap_or_default())
}

/// The C01 effect port over the existing adapter surface.
pub struct EffectBridge {
    owner: Arc<dyn EffectSessionOwner>,
    profiles: Arc<dyn AgentProfileSource>,
    dispatch: Arc<dyn EffectDispatch>,
}

impl EffectBridge {
    pub fn new(
        owner: Arc<dyn EffectSessionOwner>,
        profiles: Arc<dyn AgentProfileSource>,
        dispatch: Arc<dyn EffectDispatch>,
    ) -> Self {
        Self {
            owner,
            profiles,
            dispatch,
        }
    }

    /// What this dynamic instance can actually do on this bound session.
    pub fn capabilities(
        &self,
        agent_id: &str,
        session: &NativeWorkContextKey,
    ) -> Result<EffectCapabilities, EffectRefusal> {
        let Some(profile) = self.profiles.profile(agent_id) else {
            return Err(EffectRefusal::UnknownInstance {
                agent_id: agent_id.trim().to_owned(),
            });
        };
        self.capabilities_of(&profile, session)
    }

    /// Admit one effect, or refuse it without starting anything.
    pub fn submit(&self, invocation: &EffectInvocation) -> SubmitOutcome {
        if let Err(reason) = validate_invocation(invocation) {
            return SubmitOutcome::Refused(EffectRefusal::InvalidInvocation { reason });
        }
        let Some(profile) = self.profiles.profile(&invocation.agent_id) else {
            return SubmitOutcome::Refused(EffectRefusal::UnknownInstance {
                agent_id: invocation.agent_id.trim().to_owned(),
            });
        };
        let capabilities = match self.capabilities_of(&profile, &invocation.session) {
            Ok(capabilities) => capabilities,
            Err(refusal) => return SubmitOutcome::Refused(refusal),
        };
        let submit = capabilities.capability(EffectOperation::Submit);
        if !submit.permits_attempt() {
            return SubmitOutcome::Refused(EffectRefusal::CapabilityUnavailable {
                operation: EffectOperation::Submit,
                capability: submit,
            });
        }
        // The session's writer claim comes from the owner. This is the only place
        // this bridge keeps an instance from being replaced, and it does that by
        // asking the owner that already answers the question.
        if let Err(failure) = self.owner.claim_writer(&invocation.session) {
            return SubmitOutcome::Refused(if failure.code == ContinuityFailureCode::WriterBusy {
                EffectRefusal::InFlightInstance { failure }
            } else {
                EffectRefusal::SessionNotAdmitted { failure }
            });
        }
        let delivery = self.dispatch.deliver(invocation);
        SubmitOutcome::Admitted(Box::new(EffectHandle {
            effect_id: invocation.effect_id.clone(),
            attempt_token: invocation.attempt_token.clone(),
            agent_id: profile.agent_id,
            driver_id: profile.driver_id,
            runtime_protocol: profile.runtime_protocol,
            session: invocation.session.clone(),
            turn: invocation.turn.clone(),
            state: state_from_delivery(&delivery),
        }))
    }

    /// Read back what the adapter currently reports for an effect.
    pub fn observe(&self, handle: &EffectHandle) -> EffectObservation {
        match self.dispatch.read_back(handle) {
            Some(delivery) => EffectObservation {
                state: state_from_delivery(&delivery),
                status: delivery.status,
            },
            None => EffectObservation {
                state: EffectState::Unknown {
                    reason: EffectUnknownReason::StatusChannelUnavailable,
                },
                status: "adapter_record_absent".to_owned(),
            },
        }
    }

    /// Request cancellation. A request is not a completion.
    pub fn cancel(&self, handle: &EffectHandle) -> CancelOutcome {
        if handle.state.is_confirmed_terminal() {
            return CancelOutcome::AlreadyTerminal {
                state: handle.state.clone(),
            };
        }
        let capability = self.operation_capability(handle, EffectOperation::Cancel);
        if !capability.permits_attempt() {
            return CancelOutcome::Unavailable { capability };
        }
        let delivery = self.dispatch.control(handle, EffectControl::Cancel, None);
        match delivery.disposition {
            // The adapter answered. Its answer is a fact about the request, never
            // about the effect's fate: the state stays unknown until a separate
            // confirmed fact arrives.
            ControlDisposition::Accepted
            | ControlDisposition::NoActiveTurn
            | ControlDisposition::SessionUnavailable => CancelOutcome::Requested {
                disposition: delivery.disposition,
                state: EffectState::Unknown {
                    reason: EffectUnknownReason::CancelUnconfirmed,
                },
            },
            // The adapter itself says it exposes no such channel.
            ControlDisposition::Unsupported => CancelOutcome::Unavailable { capability },
            // The request was attempted with no establishable answer: it may
            // have reached the adapter, so it may neither be called delivered
            // nor treated as never asked.
            ControlDisposition::Unconfirmed => CancelOutcome::Unconfirmed {
                disposition: delivery.disposition,
                state: EffectState::Unknown {
                    reason: EffectUnknownReason::ControlUnconfirmed,
                },
            },
            // Only a surface that established the pre-dispatch refusal may say
            // nothing was asked.
            ControlDisposition::NotDelivered => CancelOutcome::NotReached {
                disposition: delivery.disposition,
            },
        }
    }

    /// Deliver a steer instruction to the attempt the handle names.
    pub fn steer(&self, handle: &EffectHandle, instruction: &str) -> SteerOutcome {
        if instruction.trim().is_empty() || instruction.len() > 1024 * 1024 {
            return SteerOutcome::InvalidInstruction;
        }
        if handle.state.is_confirmed_terminal() {
            return SteerOutcome::AlreadyTerminal {
                state: handle.state.clone(),
            };
        }
        let capability = self.operation_capability(handle, EffectOperation::Steer);
        if !capability.permits_attempt() {
            return SteerOutcome::Unavailable { capability };
        }
        let delivery = self
            .dispatch
            .control(handle, EffectControl::Steer, Some(instruction));
        match delivery.disposition {
            ControlDisposition::Accepted
            | ControlDisposition::NoActiveTurn
            | ControlDisposition::SessionUnavailable => SteerOutcome::Delivered {
                disposition: delivery.disposition,
            },
            ControlDisposition::Unsupported => SteerOutcome::Unavailable { capability },
            ControlDisposition::Unconfirmed => SteerOutcome::Unconfirmed {
                disposition: delivery.disposition,
            },
            ControlDisposition::NotDelivered => SteerOutcome::NotReached {
                disposition: delivery.disposition,
            },
        }
    }

    /// Read back an in-doubt effect. Nothing is re-delivered, and the session's
    /// claim is released only for a confirmed terminal fact.
    pub fn reconcile(&self, handle: &EffectHandle) -> ReconcileOutcome {
        let Some(delivery) = self.dispatch.read_back(handle) else {
            return ReconcileOutcome::InDoubt {
                state: EffectState::Unknown {
                    reason: EffectUnknownReason::StatusChannelUnavailable,
                },
                status: "adapter_record_absent".to_owned(),
            };
        };
        let state = state_from_delivery(&delivery);
        if state.is_confirmed_terminal() {
            let settlement = self.settle(&state);
            ReconcileOutcome::Settled {
                state,
                status: delivery.status,
                settlement,
            }
        } else {
            ReconcileOutcome::InDoubt {
                state,
                status: delivery.status,
            }
        }
    }

    /// Release the session's writer claim through the owner, but only for a
    /// confirmed terminal fact. An unknown effect keeps the claim: that is what
    /// stops a replacement instance from starting over an unresolved effect.
    pub fn settle(&self, state: &EffectState) -> Settlement {
        if state.is_confirmed_terminal() && self.owner.release_writer().is_ok() {
            return Settlement::Released;
        }
        Settlement::Retained {
            state: state.clone(),
        }
    }

    fn capabilities_of(
        &self,
        profile: &RuntimeAgentProfile,
        session: &NativeWorkContextKey,
    ) -> Result<EffectCapabilities, EffectRefusal> {
        // A refusal carries a code, not a claim: when the owner will not even
        // negotiate this session, this port asserts nothing about it.
        let snapshot = match self.owner.negotiate(session) {
            Ok(snapshot) => snapshot,
            Err(failure) => return Err(EffectRefusal::SessionNotAdmitted { failure }),
        };
        Ok(negotiate_capabilities(profile, &snapshot))
    }

    fn operation_capability(
        &self,
        handle: &EffectHandle,
        operation: EffectOperation,
    ) -> EffectCapability {
        let Some(profile) = self.profiles.profile(&handle.agent_id) else {
            return EffectCapability::absent();
        };
        match self.capabilities_of(&profile, &handle.session) {
            Ok(capabilities) => capabilities.capability(operation),
            Err(_) => EffectCapability::absent(),
        }
    }
}

fn validate_invocation(invocation: &EffectInvocation) -> Result<(), &'static str> {
    if invocation.effect_id.trim().is_empty() || invocation.attempt_token.trim().is_empty() {
        return Err("identity_incomplete");
    }
    if invocation.agent_id.trim().is_empty() {
        return Err("instance_missing");
    }
    if invocation.turn.native_session_id.trim().is_empty() {
        return Err("native_session_missing");
    }
    let prompt = effect_input_text(&invocation.input);
    if prompt.trim().is_empty() || prompt.len() > 1024 * 1024 {
        return Err("input_invalid");
    }
    Ok(())
}

/// The only place a delivery becomes a state.
fn state_from_delivery(delivery: &EffectDelivery) -> EffectState {
    match &delivery.outcome {
        DeliveryOutcome::Completed { output } => EffectState::Succeeded {
            output: output.clone(),
        },
        DeliveryOutcome::NotExecuted => EffectState::NotExecuted,
        DeliveryOutcome::Cancelled => EffectState::Cancelled,
        DeliveryOutcome::Failed { code, retryable } => EffectState::Failed {
            code: code.clone(),
            retryable: *retryable,
        },
        DeliveryOutcome::Accepted => EffectState::InFlight,
        DeliveryOutcome::Unconfirmed { reason } => EffectState::Unknown { reason: *reason },
    }
}

/// The bound adapter's own dimension for an operation, when its snapshot has one.
///
/// `None` means the snapshot says nothing about the operation; it is not a
/// confirmation.
fn snapshot_support(
    operation: EffectOperation,
    snapshot: &NativeCapabilitySnapshot,
) -> Option<NativeCapabilitySupport> {
    match operation {
        EffectOperation::Cancel => Some(snapshot.cancel),
        EffectOperation::Steer => Some(snapshot.steer),
        // The work-context snapshot has no dimension for admitting a new effect
        // or for reading one back, and inventing one would be a claim the bound
        // adapter never made.
        EffectOperation::Submit | EffectOperation::Observe => None,
    }
}

/// The weaker of two claims: a declaration can only be lowered by the live
/// adapter, never raised.
fn weaker(
    left: NativeCapabilitySupport,
    right: NativeCapabilitySupport,
) -> NativeCapabilitySupport {
    use NativeCapabilitySupport::*;
    match (left, right) {
        (Unsupported, _) | (_, Unsupported) => Unsupported,
        (TemporarilyUnavailable, _) | (_, TemporarilyUnavailable) => TemporarilyUnavailable,
        (Unverified, _) | (_, Unverified) => Unverified,
        (Supported, Supported) => Supported,
    }
}

fn negotiate_capabilities(
    profile: &RuntimeAgentProfile,
    snapshot: &NativeCapabilitySnapshot,
) -> EffectCapabilities {
    let mut operations = BTreeMap::new();
    for operation in [
        EffectOperation::Submit,
        EffectOperation::Observe,
        EffectOperation::Cancel,
        EffectOperation::Steer,
    ] {
        let declared = operation
            .declaration_flag()
            .and_then(|flag| profile.declares(flag));
        let negotiated = snapshot_support(operation, snapshot);
        // Absent is reported as absent: an undeclared flag never becomes support.
        let declared_support = match declared {
            Some(true) => NativeCapabilitySupport::Supported,
            Some(false) | None => NativeCapabilitySupport::Unsupported,
        };
        let support = match (declared, negotiated) {
            (Some(_), Some(negotiated)) => weaker(declared_support, negotiated),
            // The snapshot carries no dimension, so it neither raises nor lowers
            // the declaration.
            (Some(_), None) => declared_support,
            // The live adapter reports a channel no declaration backs. Confirmed
            // work is not the same fact as a declared capability, so this stays
            // unverified rather than being read as either answer.
            (None, Some(NativeCapabilitySupport::Supported)) => NativeCapabilitySupport::Unverified,
            (None, Some(negotiated)) => negotiated,
            (None, None) => NativeCapabilitySupport::Unsupported,
        };
        operations.insert(
            operation,
            EffectCapability {
                support,
                declared,
                negotiated,
            },
        );
    }
    EffectCapabilities {
        agent_id: profile.agent_id.clone(),
        driver_id: profile.driver_id.clone(),
        operations,
    }
}
