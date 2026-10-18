//! V7-R4 acceptance at `component-integration` level: the C01 `EffectPort` front
//! of the existing native work-context adapters.
//!
//! What is real here: the session owner is a real `WorkContextRuntime` (the
//! existing single-writer work-context owner) with a real hermetic protocol
//! adapter, and the port under test is the production `EffectBridge`. The
//! capability source and the effect delivery are injected, which is the point of
//! the seam: the bridge's own behaviour is what is under test, and the production
//! delivery/profile wiring is exercised separately against the packaged runtime
//! registry.
//!
//! What is synthetic, stated plainly: the injected dispatch reports the delivery
//! facts. It does not launch an agent, and nothing here claims a real vendor
//! process ran.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use licoup_agent_runtime::work_context::{
    CapabilityProfile, ChildBinding, ContinuityFailureCode, HermeticProtocol,
    NativeCapabilitySnapshot, NativeCapabilitySupport, NativeWorkContextKey, ProtocolFamily,
    WorkContextConfig, WorkContextRuntime,
};
use licoup_native::platform::work_context_ports::{
    AdapterCall, AdapterResponse, AdapterTransport, AgentProfileSource, CancelOutcome,
    ControlDelivery, ControlDisposition, DeliveryOutcome, EffectBridge, EffectCapability,
    EffectControl, EffectDelivery, EffectDispatch, EffectHandle, EffectInvocation, EffectOperation,
    EffectRefusal, EffectState, EffectTurn, EffectUnknownReason, ReconcileOutcome,
    RuntimeAgentProfile, Settlement, SteerOutcome, SubmitOutcome, bind_adapter_work_context,
    effect_input_text, production_effect_bridge, unverified_snapshot,
};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn child() -> ChildBinding {
    ChildBinding {
        child_conversation_id: "conversation:child".into(),
        membership_id: "membership:child-assistant".into(),
        source_task_id: "goal:source-task".into(),
        parent_conversation_id: "conversation:parent".into(),
    }
}

fn key(matter: &str) -> NativeWorkContextKey {
    NativeWorkContextKey {
        conversation_id: child().child_conversation_id,
        membership_id: child().membership_id,
        matter_id: matter.into(),
        generation: 1,
    }
}

/// The existing work-context owner, with a real hermetic protocol adapter bound.
fn session_owner(family: ProtocolFamily, profile: CapabilityProfile) -> Arc<WorkContextRuntime> {
    let protocol = match family {
        ProtocolFamily::Codex => HermeticProtocol::codex(profile),
        ProtocolFamily::Pi => HermeticProtocol::pi(profile),
    };
    Arc::new(WorkContextRuntime::from_hermetic(
        protocol,
        WorkContextConfig::child(child()),
    ))
}

fn declarations(flags: &[(&str, bool)]) -> BTreeMap<String, bool> {
    flags
        .iter()
        .map(|(flag, value)| ((*flag).to_owned(), *value))
        .collect()
}

fn all_declared() -> BTreeMap<String, bool> {
    declarations(&[
        ("openNew", true),
        ("structuredEvents", true),
        ("cancel", true),
        ("interruptSteer", true),
    ])
}

fn profile(agent_id: &str, flags: BTreeMap<String, bool>) -> RuntimeAgentProfile {
    RuntimeAgentProfile {
        agent_id: agent_id.into(),
        driver_id: format!("{agent_id}-driver"),
        runtime_protocol: format!("{agent_id}-protocol"),
        lane_family: Some("fixture-lane".into()),
        readiness: "ready".into(),
        blocker: None,
        declarations: flags,
    }
}

struct FakeProfiles {
    entries: BTreeMap<String, RuntimeAgentProfile>,
}

impl FakeProfiles {
    fn new(profiles: Vec<RuntimeAgentProfile>) -> Self {
        Self {
            entries: profiles
                .into_iter()
                .map(|profile| (profile.agent_id.clone(), profile))
                .collect(),
        }
    }
}

impl AgentProfileSource for FakeProfiles {
    fn profile(&self, agent_id: &str) -> Option<RuntimeAgentProfile> {
        self.entries.get(agent_id.trim()).cloned()
    }
}

/// An injected adapter surface. Counts what it was asked to do, so "nothing was
/// delivered" is a fact and not a reading of the code, and records the exact
/// invocation it was handed, so "the input reached the adapter unchanged" is a
/// fact too.
struct RecordingDispatch {
    outcome: Mutex<DeliveryOutcome>,
    record: Mutex<Option<EffectDelivery>>,
    control: Mutex<ControlDelivery>,
    inputs: Mutex<Vec<EffectInvocation>>,
    deliveries: AtomicUsize,
    controls: AtomicUsize,
}

impl RecordingDispatch {
    fn new() -> Self {
        Self {
            outcome: Mutex::new(DeliveryOutcome::Unconfirmed {
                reason: EffectUnknownReason::NoRecordedResult,
            }),
            record: Mutex::new(None),
            control: Mutex::new(ControlDelivery {
                disposition: ControlDisposition::Unconfirmed,
                status: "fixture-unconfirmed".into(),
            }),
            inputs: Mutex::new(Vec::new()),
            deliveries: AtomicUsize::new(0),
            controls: AtomicUsize::new(0),
        }
    }

    fn delivering(self, outcome: DeliveryOutcome) -> Self {
        *lock(&self.outcome) = outcome;
        self
    }

    fn recording(self, delivery: Option<EffectDelivery>) -> Self {
        *lock(&self.record) = delivery;
        self
    }

    fn answering_control(self, disposition: ControlDisposition, status: &str) -> Self {
        *lock(&self.control) = ControlDelivery {
            disposition,
            status: status.into(),
        };
        self
    }

    fn deliveries(&self) -> usize {
        self.deliveries.load(Ordering::SeqCst)
    }

    fn controls(&self) -> usize {
        self.controls.load(Ordering::SeqCst)
    }

    fn last_input(&self) -> Option<EffectInvocation> {
        lock(&self.inputs).last().cloned()
    }
}

impl EffectDispatch for RecordingDispatch {
    fn deliver(&self, invocation: &EffectInvocation) -> EffectDelivery {
        self.deliveries.fetch_add(1, Ordering::SeqCst);
        lock(&self.inputs).push(invocation.clone());
        EffectDelivery {
            outcome: lock(&self.outcome).clone(),
            status: "fixture-delivered".into(),
        }
    }

    fn read_back(&self, _handle: &EffectHandle) -> Option<EffectDelivery> {
        lock(&self.record).clone()
    }

    fn control(
        &self,
        _handle: &EffectHandle,
        _control: EffectControl,
        _instruction: Option<&str>,
    ) -> ControlDelivery {
        self.controls.fetch_add(1, Ordering::SeqCst);
        lock(&self.control).clone()
    }
}

/// A real `CodexAdapterProtocol` transport with a synthetic answer surface: it
/// reports whatever negotiated snapshot the test gives it and reaches no
/// process. This is the production adapter class, not a mock of the bridge.
struct ReportingTransport {
    snapshot: Mutex<Option<NativeCapabilitySnapshot>>,
    invocations: AtomicUsize,
}

impl ReportingTransport {
    fn reporting(snapshot: Option<NativeCapabilitySnapshot>) -> Self {
        Self {
            snapshot: Mutex::new(snapshot),
            invocations: AtomicUsize::new(0),
        }
    }
}

impl AdapterTransport for ReportingTransport {
    fn invoke(&self, _call: &AdapterCall) -> AdapterResponse {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        AdapterResponse::err("fixture-adapter-unavailable")
    }

    fn invocation_count(&self) -> u64 {
        self.invocations.load(Ordering::SeqCst) as u64
    }

    fn negotiated_capabilities(&self) -> Option<NativeCapabilitySnapshot> {
        lock(&self.snapshot).clone()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn bridge(
    owner: &Arc<WorkContextRuntime>,
    dispatch: &Arc<RecordingDispatch>,
    profiles: Vec<RuntimeAgentProfile>,
) -> EffectBridge {
    let owner_port = Arc::clone(owner);
    let dispatch_port = Arc::clone(dispatch);
    EffectBridge::new(
        owner_port,
        Arc::new(FakeProfiles::new(profiles)),
        dispatch_port,
    )
}

fn invocation(agent_id: &str, effect_id: &str, matter: &str) -> EffectInvocation {
    EffectInvocation {
        effect_id: effect_id.into(),
        attempt_token: format!("{effect_id}-attempt-1"),
        agent_id: agent_id.into(),
        session: key(matter),
        turn: EffectTurn {
            host_handle: format!("turn:{effect_id}"),
            native_session_id: format!("native-session:{matter}"),
            native_turn_id: format!("native-turn:{effect_id}"),
        },
        input: json!({ "prompt": "fixture prompt" }),
    }
}

fn admitted(outcome: SubmitOutcome) -> EffectHandle {
    match outcome {
        SubmitOutcome::Admitted(handle) => *handle,
        SubmitOutcome::Refused(refusal) => panic!("expected an admitted effect, got {refusal:?}"),
    }
}

// ---------------------------------------------------------------------------
// Capability negotiation
// ---------------------------------------------------------------------------

#[test]
fn capabilities_follow_the_profile_data_not_the_agent_name() {
    let owner = session_owner(ProtocolFamily::Codex, CapabilityProfile::High);
    let dispatch = Arc::new(RecordingDispatch::new());
    let mut beta_flags = all_declared();
    beta_flags.insert("cancel".into(), false);
    beta_flags.insert("interruptSteer".into(), false);
    let port = bridge(
        &owner,
        &dispatch,
        vec![
            profile("alpha", all_declared()),
            profile("beta", beta_flags),
            // A namespaced id is just a string: nothing branches on the name.
            profile("vendor.example/harness", all_declared()),
        ],
    );

    let alpha = port.capabilities("alpha", &key("matter:one")).unwrap();
    let namespaced = port
        .capabilities("vendor.example/harness", &key("matter:one"))
        .unwrap();
    assert_eq!(alpha.agent_id, "alpha");
    assert_eq!(alpha.driver_id, "alpha-driver");
    assert_eq!(namespaced.driver_id, "vendor.example/harness-driver");
    for operation in [EffectOperation::Submit, EffectOperation::Cancel] {
        assert_eq!(alpha.support(operation), NativeCapabilitySupport::Supported);
        assert_eq!(
            alpha.capability(operation),
            namespaced.capability(operation),
            "same declarations must produce the same answer for a different id"
        );
    }

    let beta = port.capabilities("beta", &key("matter:one")).unwrap();
    let cancel = beta.capability(EffectOperation::Cancel);
    assert_eq!(cancel.support, NativeCapabilitySupport::Unsupported);
    assert_eq!(cancel.declared, Some(false));
    assert_eq!(
        beta.support(EffectOperation::Steer),
        NativeCapabilitySupport::Unsupported
    );
}

#[test]
fn an_undeclared_capability_is_reported_absent_not_assumed() {
    let owner = session_owner(ProtocolFamily::Codex, CapabilityProfile::High);
    let dispatch = Arc::new(RecordingDispatch::new());
    // Declares admission only: no read-back, no control channels.
    let port = bridge(
        &owner,
        &dispatch,
        vec![profile("minimal", declarations(&[("openNew", true)]))],
    );

    let capabilities = port.capabilities("minimal", &key("matter:one")).unwrap();
    // No declaration and no adapter dimension for it: absent. `observe` has no
    // inventory flag of its own, and `structuredEvents` is not one: it describes
    // events delivered during a turn, not the per-attempt record this asks for.
    let observe = capabilities.capability(EffectOperation::Observe);
    assert_eq!(observe.support, NativeCapabilitySupport::Unsupported);
    assert_eq!(observe.declared, None);
    assert_eq!(observe.negotiated, None);
    assert!(!observe.permits_attempt());
    // The live adapter reports a control channel no declaration backs. That is
    // reported as unverified evidence, never as support; it is not a refusal
    // either, so an attempt may still produce the adapter's own answer.
    let cancel = capabilities.capability(EffectOperation::Cancel);
    assert_eq!(cancel.declared, None);
    assert_eq!(cancel.negotiated, Some(NativeCapabilitySupport::Supported));
    assert_eq!(cancel.support, NativeCapabilitySupport::Unverified);
    assert!(cancel.permits_attempt());
}

#[test]
fn a_live_adapter_can_lower_a_declaration_and_never_raise_one() {
    // The bound adapter is a real hermetic Pi/Low protocol: no in-flight steer.
    let owner = session_owner(ProtocolFamily::Pi, CapabilityProfile::Low);
    let dispatch = Arc::new(RecordingDispatch::new());
    let port = bridge(
        &owner,
        &dispatch,
        vec![profile("declared-high", all_declared())],
    );

    let capabilities = port
        .capabilities("declared-high", &key("matter:one"))
        .unwrap();
    let steer = capabilities.capability(EffectOperation::Steer);
    assert_eq!(steer.declared, Some(true));
    assert_eq!(steer.negotiated, Some(NativeCapabilitySupport::Unsupported));
    assert_eq!(steer.support, NativeCapabilitySupport::Unsupported);
    assert!(!steer.permits_attempt());
    assert_eq!(
        capabilities.support(EffectOperation::Cancel),
        NativeCapabilitySupport::Supported
    );
}

/// The production composition shape: a real `CodexAdapterProtocol` over a
/// transport that has not answered yet. The declaration is not permanently
/// refused by the unproven live adapter; the attempt produces the adapter's own
/// answer. An adapter that explicitly refutes the channel still sends nothing.
#[test]
fn a_declared_control_survives_an_unproven_live_adapter_and_is_still_refused_when_refuted() {
    let config = WorkContextConfig::child(child());
    let unproven = Arc::new(ReportingTransport::reporting(None));
    let owner = Arc::new(bind_adapter_work_context(
        ProtocolFamily::Codex,
        WorkContextConfig::child(child()),
        unproven.clone(),
        None,
    ));
    let dispatch = Arc::new(
        RecordingDispatch::new()
            .delivering(DeliveryOutcome::Accepted)
            .answering_control(ControlDisposition::Accepted, "cancel_requested"),
    );
    let port = bridge(
        &owner,
        &dispatch,
        vec![profile("codex-declared", all_declared())],
    );

    let capabilities = port
        .capabilities("codex-declared", &key("matter:one"))
        .unwrap();
    let cancel = capabilities.capability(EffectOperation::Cancel);
    assert_eq!(cancel.declared, Some(true));
    assert_eq!(cancel.negotiated, Some(NativeCapabilitySupport::Unverified));
    assert_eq!(cancel.support, NativeCapabilitySupport::Unverified);
    assert!(cancel.permits_attempt());
    assert_eq!(
        unproven.invocation_count(),
        0,
        "negotiation consults the bound adapter without reaching a process"
    );

    let handle = admitted(port.submit(&invocation("codex-declared", "effect:1", "matter:docs")));
    match port.cancel(&handle) {
        CancelOutcome::Requested { disposition, state } => {
            assert_eq!(disposition, ControlDisposition::Accepted);
            assert_eq!(
                state,
                EffectState::Unknown {
                    reason: EffectUnknownReason::CancelUnconfirmed
                }
            );
        }
        other => panic!("expected the adapter's answer to be reported, got {other:?}"),
    }
    assert_eq!(dispatch.controls(), 1);

    // An adapter that has answered `unsupported` for cancel refutes the
    // declaration: nothing is sent, and the answer says so.
    let mut refuting = unverified_snapshot();
    refuting.cancel = NativeCapabilitySupport::Unsupported;
    let refuting_owner = Arc::new(bind_adapter_work_context(
        ProtocolFamily::Codex,
        config,
        Arc::new(ReportingTransport::reporting(Some(refuting))),
        None,
    ));
    let refuting_dispatch = Arc::new(
        RecordingDispatch::new()
            .delivering(DeliveryOutcome::Accepted)
            .answering_control(ControlDisposition::Accepted, "cancel_requested"),
    );
    let refuting_port = bridge(
        &refuting_owner,
        &refuting_dispatch,
        vec![profile("codex-declared", all_declared())],
    );
    let handle =
        admitted(refuting_port.submit(&invocation("codex-declared", "effect:1", "matter:docs")));
    match refuting_port.cancel(&handle) {
        CancelOutcome::Unavailable { capability } => {
            assert_eq!(capability.declared, Some(true));
            assert_eq!(
                capability.negotiated,
                Some(NativeCapabilitySupport::Unsupported)
            );
            assert_eq!(capability.support, NativeCapabilitySupport::Unsupported);
        }
        other => panic!("expected the live refutation to refuse the request, got {other:?}"),
    }
    assert_eq!(refuting_dispatch.controls(), 0);
}

// ---------------------------------------------------------------------------
// Submit
// ---------------------------------------------------------------------------

#[test]
fn an_unknown_instance_is_refused_without_delivering() {
    let owner = session_owner(ProtocolFamily::Codex, CapabilityProfile::High);
    let dispatch = Arc::new(RecordingDispatch::new());
    let port = bridge(&owner, &dispatch, Vec::new());

    assert_eq!(
        port.capabilities("nobody", &key("matter:one")),
        Err(EffectRefusal::UnknownInstance {
            agent_id: "nobody".into()
        })
    );
    match port.submit(&invocation("nobody", "effect:1", "matter:one")) {
        SubmitOutcome::Refused(EffectRefusal::UnknownInstance { agent_id }) => {
            assert_eq!(agent_id, "nobody");
        }
        other => panic!("expected an unknown-instance refusal, got {other:?}"),
    }
    assert_eq!(dispatch.deliveries(), 0);
}

#[test]
fn an_absent_capability_is_refused_without_delivering() {
    let owner = session_owner(ProtocolFamily::Codex, CapabilityProfile::High);
    let dispatch = Arc::new(RecordingDispatch::new());
    let port = bridge(
        &owner,
        &dispatch,
        vec![
            profile("read-only", declarations(&[("openNew", false)])),
            profile("writer", all_declared()),
        ],
    );

    match port.submit(&invocation("read-only", "effect:1", "matter:one")) {
        SubmitOutcome::Refused(EffectRefusal::CapabilityUnavailable {
            operation,
            capability,
        }) => {
            assert_eq!(operation, EffectOperation::Submit);
            assert_eq!(capability.support, NativeCapabilitySupport::Unsupported);
            assert_eq!(capability.declared, Some(false));
        }
        other => panic!("expected a capability refusal, got {other:?}"),
    }
    assert_eq!(dispatch.deliveries(), 0);
    // The refusal covers that call only: another instance with the capability
    // still gets in on the same session.
    let admitted = admitted(port.submit(&invocation("writer", "effect:2", "matter:one")));
    assert_eq!(admitted.effect_id, "effect:2");
    assert_eq!(dispatch.deliveries(), 1);
}

#[test]
fn an_invalid_invocation_is_refused_before_any_claim_is_taken() {
    let owner = session_owner(ProtocolFamily::Codex, CapabilityProfile::High);
    let dispatch = Arc::new(RecordingDispatch::new());
    let port = bridge(&owner, &dispatch, vec![profile("alpha", all_declared())]);

    let mut unshaped = invocation("alpha", "effect:1", "matter:one");
    unshaped.input = json!({ "prompt": "   " });
    match port.submit(&unshaped) {
        SubmitOutcome::Refused(EffectRefusal::InvalidInvocation { reason }) => {
            assert_eq!(reason, "input_invalid");
        }
        other => panic!("expected an invalid-invocation refusal, got {other:?}"),
    }
    assert_eq!(dispatch.deliveries(), 0);
    // The session was never claimed, so a shaped effect still gets in.
    let handle = admitted(port.submit(&invocation("alpha", "effect:2", "matter:one")));
    assert_eq!(handle.effect_id, "effect:2");
    assert_eq!(dispatch.deliveries(), 1);
}

// ---------------------------------------------------------------------------
// In-flight instances
// ---------------------------------------------------------------------------

#[test]
fn an_in_flight_instance_is_never_replaced() {
    let owner = session_owner(ProtocolFamily::Codex, CapabilityProfile::High);
    let dispatch = Arc::new(RecordingDispatch::new().delivering(DeliveryOutcome::Accepted));
    let port = bridge(&owner, &dispatch, vec![profile("alpha", all_declared())]);

    let first = admitted(port.submit(&invocation("alpha", "effect:1", "matter:docs")));
    assert_eq!(first.state, EffectState::InFlight);
    assert_eq!(dispatch.deliveries(), 1);

    // A second instance for the same session is refused by the owner, not by a
    // lock invented here.
    match port.submit(&invocation("alpha", "effect:2", "matter:docs")) {
        SubmitOutcome::Refused(EffectRefusal::InFlightInstance { failure }) => {
            assert_eq!(failure.code, ContinuityFailureCode::WriterBusy);
        }
        other => panic!("expected an in-flight refusal, got {other:?}"),
    }
    assert_eq!(
        dispatch.deliveries(),
        1,
        "no replacement instance was delivered"
    );

    // Two bridges over one owner conflict exactly like two direct callers of the
    // owner: the exclusivity is the owner's, not the bridge's.
    let second_bridge = bridge(&owner, &dispatch, vec![profile("alpha", all_declared())]);
    match second_bridge.submit(&invocation("alpha", "effect:3", "matter:docs")) {
        SubmitOutcome::Refused(EffectRefusal::InFlightInstance { failure }) => {
            assert_eq!(failure.code, ContinuityFailureCode::WriterBusy);
        }
        other => panic!("expected the owner's refusal, got {other:?}"),
    }

    // A separate owner is a separate session authority.
    let other_owner = session_owner(ProtocolFamily::Codex, CapabilityProfile::High);
    let other_bridge = bridge(
        &other_owner,
        &dispatch,
        vec![profile("alpha", all_declared())],
    );
    let handle = admitted(other_bridge.submit(&invocation("alpha", "effect:4", "matter:docs")));
    assert_eq!(handle.effect_id, "effect:4");
    assert_eq!(dispatch.deliveries(), 2);

    // Settling on the first bridge releases through the owner, so serial reuse
    // works again.
    let settled = port.settle(&EffectState::Succeeded {
        output: Value::String("done".into()),
    });
    assert_eq!(
        settled,
        Settlement::Released,
        "a confirmed terminal fact releases the owner's claims"
    );
    let reused = admitted(port.submit(&invocation("alpha", "effect:5", "matter:docs")));
    assert_eq!(reused.effect_id, "effect:5");
}

#[test]
fn an_unresolved_effect_keeps_the_session_claim() {
    let owner = session_owner(ProtocolFamily::Codex, CapabilityProfile::High);
    let dispatch = Arc::new(RecordingDispatch::new());
    let port = bridge(&owner, &dispatch, vec![profile("alpha", all_declared())]);

    let unknown = EffectState::Unknown {
        reason: EffectUnknownReason::NoRecordedResult,
    };
    assert_eq!(
        port.settle(&unknown),
        Settlement::Retained { state: unknown }
    );
    assert_eq!(
        port.settle(&EffectState::InFlight),
        Settlement::Retained {
            state: EffectState::InFlight
        }
    );
}

// ---------------------------------------------------------------------------
// Cancel and steer
// ---------------------------------------------------------------------------

#[test]
fn cancel_is_a_request_and_never_reports_completion() {
    let owner = session_owner(ProtocolFamily::Codex, CapabilityProfile::High);
    let dispatch = Arc::new(
        RecordingDispatch::new()
            .delivering(DeliveryOutcome::Accepted)
            .answering_control(ControlDisposition::Accepted, "cancel_requested"),
    );
    let port = bridge(&owner, &dispatch, vec![profile("alpha", all_declared())]);
    let handle = admitted(port.submit(&invocation("alpha", "effect:1", "matter:docs")));

    match port.cancel(&handle) {
        CancelOutcome::Requested { disposition, state } => {
            assert_eq!(disposition, ControlDisposition::Accepted);
            assert_eq!(
                state,
                EffectState::Unknown {
                    reason: EffectUnknownReason::CancelUnconfirmed
                }
            );
            assert!(state.is_unknown());
            assert!(!state.is_confirmed_terminal());
        }
        other => panic!("expected a requested cancel, got {other:?}"),
    }
    assert_eq!(dispatch.controls(), 1);
}

#[test]
fn cancel_without_the_capability_sends_nothing() {
    let owner = session_owner(ProtocolFamily::Codex, CapabilityProfile::High);
    let dispatch = Arc::new(
        RecordingDispatch::new()
            .delivering(DeliveryOutcome::Accepted)
            .answering_control(ControlDisposition::Accepted, "cancel_requested"),
    );
    let mut declared_without_cancel = all_declared();
    declared_without_cancel.insert("cancel".into(), false);
    let port = bridge(
        &owner,
        &dispatch,
        vec![profile("no-cancel", declared_without_cancel)],
    );
    let handle = admitted(port.submit(&invocation("no-cancel", "effect:1", "matter:docs")));

    match port.cancel(&handle) {
        CancelOutcome::Unavailable { capability } => {
            assert_eq!(capability.support, NativeCapabilitySupport::Unsupported);
            assert_eq!(capability.declared, Some(false));
        }
        other => panic!("expected an unavailable cancel, got {other:?}"),
    }
    assert_eq!(dispatch.controls(), 0);

    // An instance the registry does not know cannot cancel either.
    let unknown_handle = EffectHandle {
        agent_id: "nobody".into(),
        ..handle.clone()
    };
    match port.cancel(&unknown_handle) {
        CancelOutcome::Unavailable { capability } => {
            assert_eq!(capability, EffectCapability::absent());
        }
        other => panic!("expected an absent capability, got {other:?}"),
    }
    assert_eq!(dispatch.controls(), 0);
}

#[test]
fn cancel_cannot_rewrite_a_confirmed_terminal_fact() {
    let owner = session_owner(ProtocolFamily::Codex, CapabilityProfile::High);
    let dispatch = Arc::new(
        RecordingDispatch::new()
            .delivering(DeliveryOutcome::Completed {
                output: Value::String("finished".into()),
            })
            .answering_control(ControlDisposition::Accepted, "cancel_requested"),
    );
    let port = bridge(&owner, &dispatch, vec![profile("alpha", all_declared())]);
    let handle = admitted(port.submit(&invocation("alpha", "effect:1", "matter:docs")));
    assert_eq!(
        handle.state,
        EffectState::Succeeded {
            output: Value::String("finished".into())
        }
    );

    match port.cancel(&handle) {
        CancelOutcome::AlreadyTerminal { state } => {
            assert_eq!(
                state,
                EffectState::Succeeded {
                    output: Value::String("finished".into())
                }
            );
        }
        other => panic!("expected the confirmed fact to stand, got {other:?}"),
    }
    // The external effect already happened: nothing was asked to undo it.
    assert_eq!(dispatch.controls(), 0);
}

#[test]
fn steer_follows_the_declared_channel() {
    let owner = session_owner(ProtocolFamily::Codex, CapabilityProfile::High);
    let dispatch = Arc::new(
        RecordingDispatch::new()
            .delivering(DeliveryOutcome::Accepted)
            .answering_control(ControlDisposition::Accepted, "accepted"),
    );
    let mut declared_without_steer = all_declared();
    declared_without_steer.insert("interruptSteer".into(), false);
    let port = bridge(
        &owner,
        &dispatch,
        vec![
            profile("alpha", all_declared()),
            profile("no-steer", declared_without_steer),
        ],
    );

    let handle = admitted(port.submit(&invocation("alpha", "effect:1", "matter:docs")));
    match port.steer(&handle, "please stop and summarise") {
        SteerOutcome::Delivered { disposition } => {
            assert_eq!(disposition, ControlDisposition::Accepted);
        }
        other => panic!("expected a delivered steer, got {other:?}"),
    }
    assert_eq!(dispatch.controls(), 1);

    let no_steer = admitted(port.submit(&invocation("no-steer", "effect:2", "matter:two")));
    match port.steer(&no_steer, "please stop and summarise") {
        SteerOutcome::Unavailable { capability } => {
            assert_eq!(capability.support, NativeCapabilitySupport::Unsupported);
        }
        other => panic!("expected an unavailable steer, got {other:?}"),
    }
    assert_eq!(
        port.steer(&no_steer, "   "),
        SteerOutcome::InvalidInstruction
    );
    assert_eq!(dispatch.controls(), 1);
}

// ---------------------------------------------------------------------------
// Observe and reconcile
// ---------------------------------------------------------------------------

#[test]
fn reconcile_never_redelivers_and_keeps_in_doubt_effects_claimed() {
    let owner = session_owner(ProtocolFamily::Codex, CapabilityProfile::High);
    let dispatch = Arc::new(
        RecordingDispatch::new()
            .delivering(DeliveryOutcome::Accepted)
            .recording(None),
    );
    let port = bridge(&owner, &dispatch, vec![profile("alpha", all_declared())]);
    let handle = admitted(port.submit(&invocation("alpha", "effect:1", "matter:docs")));

    match port.reconcile(&handle) {
        ReconcileOutcome::InDoubt { state, status } => {
            assert_eq!(
                state,
                EffectState::Unknown {
                    reason: EffectUnknownReason::StatusChannelUnavailable
                }
            );
            assert_eq!(status, "adapter_record_absent");
        }
        other => panic!("expected an in-doubt reconcile, got {other:?}"),
    }
    assert_eq!(dispatch.deliveries(), 1, "reconcile must not re-deliver");
    assert_eq!(dispatch.controls(), 0);

    // The effect is still unresolved, so nothing may start over it.
    match port.submit(&invocation("alpha", "effect:2", "matter:docs")) {
        SubmitOutcome::Refused(EffectRefusal::InFlightInstance { failure }) => {
            assert_eq!(failure.code, ContinuityFailureCode::WriterBusy);
        }
        other => panic!("expected the claim to survive, got {other:?}"),
    }
    assert_eq!(dispatch.deliveries(), 1);
}

#[test]
fn reconcile_with_a_confirmed_fact_settles_through_the_owner() {
    let owner = session_owner(ProtocolFamily::Codex, CapabilityProfile::High);
    let dispatch = Arc::new(
        RecordingDispatch::new()
            .delivering(DeliveryOutcome::Accepted)
            .recording(Some(EffectDelivery {
                outcome: DeliveryOutcome::NotExecuted,
                status: "not-executed".into(),
            })),
    );
    let port = bridge(&owner, &dispatch, vec![profile("alpha", all_declared())]);
    let handle = admitted(port.submit(&invocation("alpha", "effect:1", "matter:docs")));

    match port.reconcile(&handle) {
        ReconcileOutcome::Settled {
            state,
            status,
            settlement,
        } => {
            assert_eq!(state, EffectState::NotExecuted);
            assert_eq!(status, "not-executed");
            assert_eq!(
                settlement,
                Settlement::Released,
                "the owner released its claim for the confirmed fact"
            );
        }
        other => panic!("expected a settled reconcile, got {other:?}"),
    }
    assert_eq!(dispatch.deliveries(), 1);
    // The claim was released through the owner, so the session can be written again.
    let next = admitted(port.submit(&invocation("alpha", "effect:2", "matter:docs")));
    assert_eq!(next.effect_id, "effect:2");
}

#[test]
fn observe_reports_the_adapter_record_and_never_invents_one() {
    let owner = session_owner(ProtocolFamily::Codex, CapabilityProfile::High);
    let dispatch = Arc::new(
        RecordingDispatch::new()
            .delivering(DeliveryOutcome::Accepted)
            .recording(Some(EffectDelivery {
                outcome: DeliveryOutcome::Failed {
                    code: "vendor.example/quota".into(),
                    retryable: false,
                },
                status: "failed".into(),
            })),
    );
    let port = bridge(&owner, &dispatch, vec![profile("alpha", all_declared())]);
    let handle = admitted(port.submit(&invocation("alpha", "effect:1", "matter:docs")));

    let observed = port.observe(&handle);
    assert_eq!(
        observed.state,
        EffectState::Failed {
            code: "vendor.example/quota".into(),
            retryable: false
        }
    );
    assert_eq!(observed.status, "failed");

    let unnamed = EffectHandle {
        agent_id: "nobody".into(),
        ..handle
    };
    // An adapter with no record cannot be read as a failure.
    let absent = bridge(
        &owner,
        &Arc::new(RecordingDispatch::new()),
        vec![profile("alpha", all_declared())],
    );
    assert!(absent.observe(&unnamed).state.is_unknown());
}

// ---------------------------------------------------------------------------
// Production composition
// ---------------------------------------------------------------------------

/// The production registry validates its packaged inventory against the
/// installed host composition, exactly as the binary's own entry installs it
/// before the first dispatch. An integration binary that never dispatched has
/// no composition yet, so one dispatch entry composes it — the refusal the
/// empty request earns is beside the point.
fn compose_runtime() {
    let _ = licoup_native::dispatch_agent_operation("history", &serde_json::json!({}));
}

#[test]
fn the_production_port_negotiates_from_the_live_runtime_registry() {
    compose_runtime();
    let owner = session_owner(ProtocolFamily::Codex, CapabilityProfile::High);
    let owner_port = Arc::clone(&owner);
    let port = production_effect_bridge(owner_port);

    // Two vendors, one code path: the difference below is packaged profile data,
    // not a vendor branch in the bridge.
    let codex = port.capabilities("codex", &key("matter:one")).unwrap();
    assert_eq!(codex.driver_id, "codex-app-server");
    assert_eq!(
        codex.support(EffectOperation::Submit),
        NativeCapabilitySupport::Supported
    );
    assert_eq!(
        codex.support(EffectOperation::Cancel),
        NativeCapabilitySupport::Supported
    );
    assert_eq!(
        codex.capability(EffectOperation::Cancel).declared,
        Some(true)
    );

    let pi = port.capabilities("pi", &key("matter:one")).unwrap();
    assert_eq!(pi.driver_id, "pi-rpc");
    assert_eq!(pi.capability(EffectOperation::Cancel).declared, Some(false));
    assert_eq!(
        pi.support(EffectOperation::Cancel),
        NativeCapabilitySupport::Unsupported
    );
    assert_eq!(
        pi.support(EffectOperation::Steer),
        NativeCapabilitySupport::Supported
    );

    let harness = port
        .capabilities("deepseek-harness", &key("matter:one"))
        .unwrap();
    assert_eq!(
        harness.support(EffectOperation::Cancel),
        NativeCapabilitySupport::Unsupported
    );
    assert_eq!(
        harness.support(EffectOperation::Steer),
        NativeCapabilitySupport::Unsupported
    );
    // `observe` is absent for every packaged vendor: the inventory's
    // `structuredEvents` flag describes events delivered during a turn, not the
    // per-attempt record `observe` asks for, so no declaration is read for it.
    // The production lane holds no such record either, and the answer must not
    // claim a channel the bound surface cannot answer.
    let observe = harness.capability(EffectOperation::Observe);
    assert_eq!(observe.support, NativeCapabilitySupport::Unsupported);
    assert_eq!(observe.declared, None);
    assert_eq!(observe.negotiated, None);
    assert!(!observe.permits_attempt());

    assert_eq!(
        port.capabilities("not-a-runtime-adapter", &key("matter:one")),
        Err(EffectRefusal::UnknownInstance {
            agent_id: "not-a-runtime-adapter".into()
        })
    );
}

// ---------------------------------------------------------------------------
// Truthful failure reporting
// ---------------------------------------------------------------------------

#[test]
fn a_transport_failure_stays_unknown_and_is_never_re_dispatched() {
    let owner = session_owner(ProtocolFamily::Codex, CapabilityProfile::High);
    let dispatch = Arc::new(
        RecordingDispatch::new().delivering(DeliveryOutcome::Unconfirmed {
            reason: EffectUnknownReason::AdapterUnreachable,
        }),
    );
    let port = bridge(&owner, &dispatch, vec![profile("alpha", all_declared())]);

    let handle = admitted(port.submit(&invocation("alpha", "effect:1", "matter:docs")));
    // A connection failure is not a fact that the effect did not happen.
    assert_eq!(
        handle.state,
        EffectState::Unknown {
            reason: EffectUnknownReason::AdapterUnreachable
        }
    );
    assert_eq!(dispatch.deliveries(), 1);

    // The session claim survives, so the effect is not re-dispatched.
    match port.submit(&invocation("alpha", "effect:2", "matter:docs")) {
        SubmitOutcome::Refused(EffectRefusal::InFlightInstance { failure }) => {
            assert_eq!(failure.code, ContinuityFailureCode::WriterBusy);
        }
        other => panic!("expected the unresolved effect to hold the claim, got {other:?}"),
    }
    assert_eq!(dispatch.deliveries(), 1);
    // And an unknown cannot settle the claim either.
    assert_eq!(
        port.settle(&handle.state),
        Settlement::Retained {
            state: handle.state.clone()
        }
    );
    match port.submit(&invocation("alpha", "effect:3", "matter:docs")) {
        SubmitOutcome::Refused(EffectRefusal::InFlightInstance { .. }) => {}
        other => panic!("expected the claim to stay held, got {other:?}"),
    }
    assert_eq!(dispatch.deliveries(), 1);
}

#[test]
fn a_control_request_with_no_establishable_answer_stays_unconfirmed() {
    let owner = session_owner(ProtocolFamily::Codex, CapabilityProfile::High);
    let dispatch = Arc::new(
        RecordingDispatch::new()
            .delivering(DeliveryOutcome::Accepted)
            .answering_control(ControlDisposition::Unconfirmed, "lane_dispatch_failed"),
    );
    let port = bridge(&owner, &dispatch, vec![profile("alpha", all_declared())]);
    let handle = admitted(port.submit(&invocation("alpha", "effect:1", "matter:docs")));

    match port.cancel(&handle) {
        CancelOutcome::Unconfirmed { disposition, state } => {
            assert_eq!(disposition, ControlDisposition::Unconfirmed);
            assert_eq!(
                state,
                EffectState::Unknown {
                    reason: EffectUnknownReason::ControlUnconfirmed
                },
                "an unestablishable answer is neither a completion nor a refusal"
            );
            assert!(!state.is_confirmed_terminal());
        }
        other => panic!("expected an unconfirmed control answer, got {other:?}"),
    }
    match port.steer(&handle, "please summarise") {
        SteerOutcome::Unconfirmed { disposition } => {
            assert_eq!(disposition, ControlDisposition::Unconfirmed);
        }
        other => panic!("expected an unconfirmed steer answer, got {other:?}"),
    }
    assert_eq!(dispatch.controls(), 2);

    // Nothing was released and nothing was re-dispatched: the effect is still in
    // doubt and the claim still holds.
    assert_eq!(
        port.settle(&handle.state),
        Settlement::Retained {
            state: handle.state.clone()
        }
    );
    match port.submit(&invocation("alpha", "effect:2", "matter:docs")) {
        SubmitOutcome::Refused(EffectRefusal::InFlightInstance { failure }) => {
            assert_eq!(failure.code, ContinuityFailureCode::WriterBusy);
        }
        other => panic!("expected the claim to survive, got {other:?}"),
    }
    assert_eq!(dispatch.deliveries(), 1);
}

#[test]
fn only_an_established_pre_dispatch_refusal_is_not_reached() {
    let owner = session_owner(ProtocolFamily::Codex, CapabilityProfile::High);
    let dispatch = Arc::new(
        RecordingDispatch::new()
            .delivering(DeliveryOutcome::Accepted)
            .answering_control(ControlDisposition::NotDelivered, "pre_dispatch_refused"),
    );
    let port = bridge(&owner, &dispatch, vec![profile("alpha", all_declared())]);
    let handle = admitted(port.submit(&invocation("alpha", "effect:1", "matter:docs")));

    assert_eq!(
        port.cancel(&handle),
        CancelOutcome::NotReached {
            disposition: ControlDisposition::NotDelivered
        },
        "a surface that established the pre-dispatch refusal is reported as such"
    );
    assert_eq!(
        port.steer(&handle, "please summarise"),
        SteerOutcome::NotReached {
            disposition: ControlDisposition::NotDelivered
        }
    );
    assert_eq!(dispatch.controls(), 2);
}

#[test]
fn ordinary_prose_is_carried_verbatim_and_no_verdict_is_invented() {
    let owner = session_owner(ProtocolFamily::Codex, CapabilityProfile::High);
    let dispatch = Arc::new(
        RecordingDispatch::new().delivering(DeliveryOutcome::Completed {
            output: Value::String("a plain answer, not JSON".into()),
        }),
    );
    let port = bridge(&owner, &dispatch, vec![profile("alpha", all_declared())]);

    let mut natural = invocation("alpha", "effect:1", "matter:one");
    natural.input = json!({ "prompt": "just talk to me\n\nsecond paragraph" });
    let handle = admitted(port.submit(&natural));
    assert_eq!(
        handle.state,
        EffectState::Succeeded {
            output: Value::String("a plain answer, not JSON".into())
        },
        "the adapter's report is carried, not interpreted"
    );
    let recorded = dispatch.last_input().expect("the invocation was recorded");
    assert_eq!(
        effect_input_text(&recorded.input),
        "just talk to me\n\nsecond paragraph",
        "the input reaches the adapter unchanged"
    );

    // JSON-looking text is text too: nothing parses it or demands an envelope.
    let mut jsonish = invocation("alpha", "effect:2", "matter:two");
    jsonish.input = json!({ "prompt": "{\"status\":\"done\"}" });
    let handle = admitted(port.submit(&jsonish));
    assert!(matches!(handle.state, EffectState::Succeeded { .. }));
    let recorded = dispatch.last_input().expect("the invocation was recorded");
    assert_eq!(effect_input_text(&recorded.input), "{\"status\":\"done\"}");
    // An input without a prompt field is serialised as-is rather than dropped.
    let mut structured = invocation("alpha", "effect:3", "matter:three");
    structured.input = json!({ "task": "summarise", "lang": "zh" });
    assert_eq!(
        effect_input_text(&structured.input),
        "{\"lang\":\"zh\",\"task\":\"summarise\"}"
    );
}

#[test]
fn a_session_the_owner_will_not_admit_claims_nothing() {
    let owner = session_owner(ProtocolFamily::Codex, CapabilityProfile::High);
    let dispatch = Arc::new(RecordingDispatch::new());
    let port = bridge(&owner, &dispatch, vec![profile("alpha", all_declared())]);

    let mut foreign = invocation("alpha", "effect:1", "matter:docs");
    foreign.session.conversation_id = "conversation:foreign".into();
    match port.capabilities("alpha", &foreign.session) {
        Err(EffectRefusal::SessionNotAdmitted { failure }) => {
            assert_eq!(failure.code, ContinuityFailureCode::IdentityConflict);
        }
        other => panic!("expected the owner's refusal, got {other:?}"),
    }
    match port.submit(&foreign) {
        SubmitOutcome::Refused(EffectRefusal::SessionNotAdmitted { failure }) => {
            assert_eq!(failure.code, ContinuityFailureCode::IdentityConflict);
        }
        other => panic!("expected the owner's refusal, got {other:?}"),
    }
    assert_eq!(dispatch.deliveries(), 0);
}

/// The production dispatch path, not an injected surface: `runtime_lane` binds
/// the real lane control. The session below is not bound to this process, which
/// is the exact "the host answered but no turn is here" case, and the lane
/// answers it without launching anything.
#[test]
fn the_production_control_path_reaches_the_lane_and_reports_its_answer() {
    compose_runtime();
    let owner = session_owner(ProtocolFamily::Codex, CapabilityProfile::High);
    let port = production_effect_bridge(owner);
    let handle = EffectHandle {
        effect_id: "effect:lane".into(),
        attempt_token: "effect:lane-attempt-1".into(),
        agent_id: "codex".into(),
        driver_id: "codex-app-server".into(),
        runtime_protocol: "codex-app-server".into(),
        session: key("matter:docs"),
        turn: EffectTurn {
            host_handle: "turn:lane".into(),
            native_session_id: "native-session:not-bound-in-this-process".into(),
            native_turn_id: "native-turn:not-bound-in-this-process".into(),
        },
        state: EffectState::InFlight,
    };

    match port.cancel(&handle) {
        CancelOutcome::Requested { disposition, state } => {
            assert_eq!(disposition, ControlDisposition::SessionUnavailable);
            assert_eq!(
                state,
                EffectState::Unknown {
                    reason: EffectUnknownReason::CancelUnconfirmed
                },
                "the lane's answer is a fact about the request, never a completion"
            );
        }
        other => panic!("expected the lane's own answer, got {other:?}"),
    }
    match port.steer(&handle, "please summarise") {
        SteerOutcome::Delivered { disposition } => {
            assert_eq!(disposition, ControlDisposition::SessionUnavailable);
        }
        other => panic!("expected the lane's own answer, got {other:?}"),
    }

    // The real lane also fails calls it rejects before any process is touched.
    // That failure is only an error: it cannot establish whether the request
    // reached the adapter, so it is reported as unconfirmed, never as a
    // pre-dispatch refusal and never as delivered.
    let sessionless = EffectHandle {
        turn: EffectTurn {
            host_handle: "turn:lane".into(),
            native_session_id: String::new(),
            native_turn_id: String::new(),
        },
        ..handle
    };
    match port.cancel(&sessionless) {
        CancelOutcome::Unconfirmed { disposition, state } => {
            assert_eq!(disposition, ControlDisposition::Unconfirmed);
            assert_eq!(
                state,
                EffectState::Unknown {
                    reason: EffectUnknownReason::ControlUnconfirmed
                }
            );
        }
        other => panic!("expected the failed lane call to stay unconfirmed, got {other:?}"),
    }
    match port.steer(&sessionless, "please summarise") {
        SteerOutcome::Unconfirmed { disposition } => {
            assert_eq!(disposition, ControlDisposition::Unconfirmed);
        }
        other => panic!("expected the failed lane call to stay unconfirmed, got {other:?}"),
    }
}
