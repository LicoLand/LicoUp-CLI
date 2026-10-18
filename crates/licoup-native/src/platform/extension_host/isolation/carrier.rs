//! The real subprocess carrier: one extension, one supervised process group with
//! one honest wire.
//!
//! What one release covers is decided by the mode, and stated in the record:
//!
//! - A **restricted** instance is one forced process (`process-fork` denied), so
//!   its observed exit *is* the whole instance: the release scope is
//!   [`ReleaseScope::Instance`], the owner can be verified, and the per-process
//!   POSIX limits really bound the instance.
//! - A **trusted local** program may fork, and a descendant that leaves the
//!   group with `setsid`/`setpgid` is not reclaimed. Its release scope is
//!   [`ReleaseScope::ProcessGroup`], the release is refused as complete
//!   (`extension_isolation_descendants_unverified`), and the owner stays
//!   unverified — a reaped root and a closed pipe are never proof of absence.
//!   [`PlatformConfinement::group_escape`] reports that boundary instead of
//!   claiming a whole tree.
//!
//! This adapts the host's [`ExtensionCarrier`] port to real processes. It starts
//! a real program with the operating-system controls reported by
//! [`PlatformConfinement`], speaks the published C09 line protocol over the
//! process's stdin/stdout, and never reports more than it observed.
//!
//! Rules the implementation is built around, because each one is easy to get
//! subtly wrong:
//!
//! - **One exchange, one deadline.** Every request is awaited with the
//!   monotonic-clock wall bound from the envelope. Exceeding it is
//!   `unresponsive`; the carrier then tears the process group down, and the
//!   host's existing fault rule isolates only that instance.
//! - **The reader owns the bounds.** Output is counted and bounded in the host
//!   process. A frame over the negotiated bound, or total output over the
//!   envelope, is an `over-budget` fault and the offending bytes are dropped
//!   instead of buffered.
//! - **Cancellation is a request.** `agent.cancel` is forwarded and its answer
//!   is mapped to [`CancelDisposition`]. No cancel settles an invocation; only a
//!   terminal event or an observed process death can.
//! - **An unanswered call after process death is unknown.** It is reported
//!   `Unknown` and never re-sent: the wire ref only ever maps to the binding the
//!   host issued on this session.
//! - **Release needs an observed exit *and* a scope.** [`ExtensionCarrier::shutdown`]
//!   records the status `wait` produced and the scope that release really
//!   covers; a teardown without an observed wait returns a refusal, and a
//!   process-group-scoped release refuses completeness even when the exit was
//!   observed, so the host keeps its owner unverified.
//! - **Shutdown asks first, then tears down.** A runtime is given the grace
//!   window to exit on its own after `extension.shutdown`; the process group is
//!   torn down after that, which is what reclaims a descendant that still holds
//!   the stdout pipe.
//!
//! What this carrier does not do: it does not parse the extension's natural
//! output, it does not retry unknown work, and it does not decide effects. The
//! host's bindings, the instance authority and the workflow owners keep those
//! accounts.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use licoup_application::ApplicationFailure;
use licoup_extension_contracts::agent::{
    Admission, AdmissionOutcome, AgentEvent, AgentEventKind, CancelOutcome,
};
use licoup_extension_contracts::profile::{self, DeclaredMethods};
use serde_json::{Value, json};

use crate::platform::process_supervisor::{
    BoundedStdinWriter, IO_THREAD_EXIT_GRACE, LifecycleFailure, StdinFailure, SupervisedChild,
    join_bounded,
};

use super::super::carrier::{
    CancelDisposition, CarrierSession, CarrierSpec, DispatchOutcome, ExtensionCarrier, FaultClass,
    InitializeRequest, InitializedProfileSet, Observation, carrier_fault, fault_name,
};
use super::super::{refusal, uncertain};
use super::capability::{IsolationMode, PlatformConfinement, Support};
use super::confinement::{self, ValidatedProgram};
use super::declaration::{
    IsolationLedger, LimitScope, NetworkGrant, ObservedExit, ReleaseScope, ResourceDeclaration,
    RevocationReason,
};
use super::limits::{self, EnforcedLimits, ResourceLimits};
use super::program::ProgramSource;

/// The reported reason an invocation has no answer after its process died.
const PROCESS_EXIT_WITHOUT_TERMINAL: &str = "extension_process_exit_before_terminal";

/// How one carrier is configured. The composition owns this value; the carrier
/// validates it against the real host before anything starts.
#[derive(Clone, Debug)]
pub struct IsolationPolicy {
    pub mode: IsolationMode,
    pub limits: ResourceLimits,
    pub network: NetworkGrant,
    /// The managed root the instance's writable directory and the record live
    /// under. It must already exist; the carrier does not invent one.
    pub managed_root: PathBuf,
    /// How long the runtime may take to exit on its own after
    /// `extension.shutdown` before the process group is torn down.
    pub shutdown_grace_ms: u64,
}

impl IsolationPolicy {
    pub fn new(mode: IsolationMode, managed_root: impl Into<PathBuf>) -> Self {
        Self {
            mode,
            limits: ResourceLimits::default(),
            network: NetworkGrant::Denied,
            managed_root: managed_root.into(),
            shutdown_grace_ms: 2_000,
        }
    }

    pub fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.limits = limits;
        self
    }

    pub fn with_network(mut self, network: NetworkGrant) -> Self {
        self.network = network;
        self
    }

    pub fn with_shutdown_grace_ms(mut self, millis: u64) -> Self {
        self.shutdown_grace_ms = millis;
        self
    }

    pub(crate) fn shutdown_grace(&self) -> Duration {
        Duration::from_millis(self.shutdown_grace_ms)
    }

    /// What the POSIX limits really cover under this mode.
    ///
    /// A restricted instance is one enforced process, so its per-process limits
    /// bound the instance. A trusted local program may have descendants, and its
    /// limits stay per process.
    pub(crate) fn limit_scope(&self) -> LimitScope {
        match self.mode {
            IsolationMode::Restricted => LimitScope::Instance,
            IsolationMode::TrustedLocal => LimitScope::Process,
        }
    }

    /// What a release under this mode can cover.
    pub(crate) fn release_scope(&self) -> ReleaseScope {
        match self.mode {
            IsolationMode::Restricted => ReleaseScope::Instance,
            IsolationMode::TrustedLocal => ReleaseScope::ProcessGroup,
        }
    }
}

/// What one live instance really did, for diagnostics and evidence.
///
/// Counters are incremented by the carrier's own threads; nothing here is
/// reported by the extension.
pub struct InstanceFacts {
    pub instance_id: String,
    pub package_id: String,
    pub package_version: String,
    pub generation: u64,
    pub pid: u32,
    pub mode: IsolationMode,
    pub confinement: Support,
    pub network: NetworkGrant,
    pub enforced: EnforcedLimits,
    pub bytes_stdout: AtomicU64,
    pub bytes_stderr: AtomicU64,
    /// The last few hundred bytes of the extension's stderr, kept for
    /// diagnostics only and bounded like everything else the reader keeps.
    stderr_tail: Mutex<String>,
    pub frames_rejected: AtomicU64,
    pub terminal_events: AtomicU64,
    pub dispatched: AtomicU64,
    fault: Mutex<Option<String>>,
    exit: Mutex<Option<ObservedExit>>,
    released: AtomicBool,
    forced: AtomicBool,
    unconfirmed_release: AtomicBool,
    pipe_held_after_release: AtomicBool,
    release_scope: Mutex<Option<ReleaseScope>>,
}

impl InstanceFacts {
    /// The classified fault this instance hit, when its wire broke.
    pub fn fault(&self) -> Option<String> {
        lock(&self.fault).clone()
    }

    /// The exit the carrier observed, if any.
    pub fn exit(&self) -> Option<ObservedExit> {
        *lock(&self.exit)
    }

    /// Whether a release was recorded for this instance.
    pub fn released(&self) -> bool {
        self.released.load(Ordering::SeqCst)
    }

    /// Whether the released process was ended by a signal rather than exiting
    /// on its own.
    pub fn release_was_forced(&self) -> bool {
        self.forced.load(Ordering::SeqCst)
    }

    /// Whether this instance had to be stopped without an observed exit and
    /// without a written release (for example, its grant could not be recorded).
    /// The pid is the evidence an operator has to reconcile.
    pub fn release_unconfirmed(&self) -> bool {
        self.unconfirmed_release.load(Ordering::SeqCst)
    }

    /// Whether the instance's stdout was still held after teardown, meaning a
    /// descendant left the process group and was not reclaimed. The release
    /// covers the supervised process; this says what was left. `false` is not
    /// proof of absence: a descendant may close stdio and keep running.
    pub fn pipe_held_after_release(&self) -> bool {
        self.pipe_held_after_release.load(Ordering::SeqCst)
    }

    /// What the recorded release covers, once one was written.
    pub fn release_scope(&self) -> Option<ReleaseScope> {
        *lock(&self.release_scope)
    }

    pub fn bytes_stdout(&self) -> u64 {
        self.bytes_stdout.load(Ordering::SeqCst)
    }

    pub fn bytes_stderr(&self) -> u64 {
        self.bytes_stderr.load(Ordering::SeqCst)
    }

    /// The bounded stderr tail, for diagnosing a failed start.
    pub fn stderr_tail(&self) -> String {
        lock(&self.stderr_tail).clone()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// The wire state one session keeps, separate from the process handle so a
/// reader thread never needs the child lock.
struct WireState {
    next_id: u64,
    responses: BTreeMap<u64, Value>,
    ready: Option<Vec<String>>,
    fault: Option<FaultClass>,
    exit: Option<ObservedExit>,
    shutting_down: bool,
    terminal: BTreeMap<String, Value>,
    refs: BTreeSet<String>,
    resume_supported: bool,
}

/// One started instance.
struct WireSession {
    facts: Arc<InstanceFacts>,
    declaration: ResourceDeclaration,
    limits: ResourceLimits,
    shutdown_grace: Duration,
    child: Mutex<SupervisedChild>,
    stdin: Mutex<Option<BoundedStdinWriter>>,
    state: Mutex<WireState>,
    wake: Condvar,
    shutdown_lock: Mutex<()>,
    release_outcome: Mutex<Option<ReleaseOutcome>>,
    stdout_reader: Mutex<Option<JoinHandle<()>>>,
    stderr_reader: Mutex<Option<JoinHandle<()>>>,
    released: AtomicBool,
}

impl WireSession {
    fn max_frame_bytes(&self) -> u64 {
        self.limits.max_frame_bytes
    }

    fn note_fault(&self, class: FaultClass, detail: &'static str) {
        let mut state = lock(&self.state);
        if state.fault.is_none() {
            state.fault = Some(class);
        }
        drop(state);
        let mut fault = lock(&self.facts.fault);
        if fault.is_none() {
            *fault = Some(format!("{}: {detail}", fault_name(class)));
        }
        drop(fault);
        self.wake.notify_all();
    }

    fn send_request(&self, method: &str, params: Value) -> Result<u64, ApplicationFailure> {
        let id = {
            let mut state = lock(&self.state);
            if let Some(class) = state.fault {
                return Err(carrier_fault(class));
            }
            if state.exit.is_some() {
                return Err(carrier_fault(FaultClass::Crashed));
            }
            state.next_id += 1;
            state.next_id
        };
        let frame = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut line = serde_json::to_vec(&frame).map_err(|_| {
            refusal("extension_isolation_frame_invalid", "extension/isolation").with_field("frame")
        })?;
        line.push(b'\n');
        if line.len() as u64 > self.max_frame_bytes() {
            // Our own outbound frame cannot be answered within the negotiated
            // bound, so it must not be sent at all.
            self.note_fault(FaultClass::OverBudget, "outbound_frame_over_bound");
            return Err(carrier_fault(FaultClass::OverBudget));
        }
        let mut stdin = lock(&self.stdin);
        let Some(writer) = stdin.as_mut() else {
            return Err(carrier_fault(FaultClass::Crashed));
        };
        match writer.enqueue(line) {
            Ok(()) => Ok(id),
            Err(StdinFailure::Busy) => {
                self.note_fault(FaultClass::Unresponsive, "stdin_backpressure");
                Err(carrier_fault(FaultClass::Unresponsive))
            }
            Err(_) => {
                let class = self.wire_fault_for_dead_process(FaultClass::Crashed);
                Err(carrier_fault(class))
            }
        }
    }

    /// When a write fails, take the process's own state as the evidence: an
    /// exited process is `crashed`; a live but unreadable pipe is `unresponsive`.
    fn wire_fault_for_dead_process(&self, fallback: FaultClass) -> FaultClass {
        let exit = {
            let mut child = lock(&self.child);
            child.try_wait().ok().flatten()
        };
        match exit {
            Some(status) => {
                self.record_exit(ObservedExit::from_status(status));
                FaultClass::Crashed
            }
            None => fallback,
        }
    }

    fn record_exit(&self, exit: ObservedExit) {
        let mut state = lock(&self.state);
        if state.exit.is_none() {
            state.exit = Some(exit);
        }
        drop(state);
        let mut slot = lock(&self.facts.exit);
        if slot.is_none() {
            *slot = Some(exit);
        }
    }

    fn await_response(&self, id: u64) -> Result<Value, ApplicationFailure> {
        let deadline = Instant::now() + self.limits.call_wall();
        let mut state = lock(&self.state);
        loop {
            if let Some(frame) = state.responses.remove(&id) {
                return Ok(frame);
            }
            if let Some(class) = state.fault {
                return Err(carrier_fault(class));
            }
            if state.exit.is_some() {
                return Err(carrier_fault(FaultClass::Crashed));
            }
            let now = Instant::now();
            if now >= deadline {
                if state.fault.is_none() {
                    state.fault = Some(FaultClass::Unresponsive);
                }
                drop(state);
                let mut fault = lock(&self.facts.fault);
                if fault.is_none() {
                    *fault = Some(format!(
                        "{}: call_wall_exceeded",
                        fault_name(FaultClass::Unresponsive)
                    ));
                }
                drop(fault);
                self.wake.notify_all();
                return Err(carrier_fault(FaultClass::Unresponsive));
            }
            let (guard, _) = self
                .wake
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state = guard;
        }
    }

    fn call_answer(&self, method: &str, params: Value) -> Result<WireAnswer, ApplicationFailure> {
        let id = self.send_request(method, params)?;
        let frame = self.await_response(id)?;
        if let Some(error) = frame.get("error") {
            let code = error
                .get("code")
                .and_then(Value::as_i64)
                .map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".to_owned());
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect::<String>();
            return Ok(WireAnswer::Refused { code, message });
        }
        Ok(WireAnswer::Result(
            frame.get("result").cloned().unwrap_or(Value::Null),
        ))
    }

    fn call(&self, method: &str, params: Value) -> Result<Value, ApplicationFailure> {
        match self.call_answer(method, params)? {
            WireAnswer::Result(result) => Ok(result),
            WireAnswer::Refused { code, message } => Err(refusal(
                "extension_isolation_wire_refusal",
                "extension/isolation",
            )
            .with_presentation_arg("method", method)
            .with_presentation_arg("wireCode", &code)
            .with_presentation_arg("wireMessage", &message)),
        }
    }

    fn wire_ref(&self, binding: &super::super::invocation::InvocationBinding) -> Option<String> {
        let state = lock(&self.state);
        state
            .refs
            .contains(binding.invocation_id())
            .then(|| binding.invocation_id().to_owned())
    }

    fn terminal_body(&self, wire_ref: &str) -> Option<Value> {
        lock(&self.state).terminal.get(wire_ref).cloned()
    }

    fn process_exited(&self) -> bool {
        lock(&self.state).exit.is_some()
    }

    fn resume_supported(&self) -> bool {
        lock(&self.state).resume_supported
    }

    fn ingest_frame(&self, line: &[u8]) {
        let Ok(frame) = serde_json::from_slice::<Value>(line) else {
            self.note_fault(FaultClass::Protocol, "malformed_frame");
            return;
        };
        if let Some(id_value) = frame.get("id") {
            match id_value.as_u64() {
                Some(id) => {
                    lock(&self.state).responses.insert(id, frame);
                    self.wake.notify_all();
                }
                None => self.note_fault(FaultClass::Protocol, "unmatchable_response_id"),
            }
            return;
        }
        let method = frame.get("method").and_then(Value::as_str);
        match method {
            Some(profile::METHOD_READY) => {
                let profiles = frame
                    .get("params")
                    .and_then(|params| params.get("profiles"))
                    .and_then(Value::as_array)
                    .map(|profiles| {
                        profiles
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                lock(&self.state).ready = Some(profiles);
                self.wake.notify_all();
            }
            Some(profile::METHOD_AGENT_EVENT) => {
                let Some(params) = frame.get("params") else {
                    self.note_fault(FaultClass::Protocol, "event_without_params");
                    return;
                };
                let Ok(event) = serde_json::from_value::<AgentEvent>(params.clone()) else {
                    self.note_fault(FaultClass::Protocol, "malformed_agent_event");
                    return;
                };
                self.facts.terminal_events.fetch_add(
                    u64::from(event.kind == AgentEventKind::Terminal),
                    Ordering::SeqCst,
                );
                if event.kind == AgentEventKind::Terminal {
                    lock(&self.state)
                        .terminal
                        .insert(event.invocation_ref.clone(), event.body);
                }
            }
            Some(profile::METHOD_USAGE_PUBLISH) => {
                // Additive C11 traffic is counted nowhere here: usage facts
                // belong to their own owner, not to the isolation record.
            }
            Some(_) => {
                // A method this build does not know is additive, never a fault.
            }
            None => self.note_fault(FaultClass::Protocol, "frame_without_method"),
        }
    }

    /// The stdout reader: bounds frames and total output, then records the wire
    /// as closed.
    fn read_stdout(self: &Arc<Self>, mut reader: std::process::ChildStdout) {
        let max_frame = self.max_frame_bytes();
        let max_total = self.limits.max_stdout_bytes;
        let mut chunk = [0u8; 8192];
        let mut line: Vec<u8> = Vec::new();
        let mut discarding = false;
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => {
                    let total = self
                        .facts
                        .bytes_stdout
                        .fetch_add(read as u64, Ordering::SeqCst)
                        + read as u64;
                    if total > max_total && max_total > 0 {
                        self.note_fault(FaultClass::OverBudget, "stdout_total_over_budget");
                        // Close our end of the pipe: the extension's writes stop
                        // being accepted instead of being drained forever, so the
                        // carrier never becomes the sink for an unbounded stream.
                        break;
                    }
                    for byte in &chunk[..read] {
                        if *byte == b'\n' {
                            if !discarding && !line.is_empty() {
                                self.ingest_frame(&line);
                            }
                            line.clear();
                            discarding = false;
                        } else if !discarding {
                            line.push(*byte);
                            if line.len() as u64 > max_frame {
                                discarding = true;
                                line.clear();
                                self.facts.frames_rejected.fetch_add(1, Ordering::SeqCst);
                                self.note_fault(FaultClass::OverBudget, "frame_over_bound");
                            }
                        }
                    }
                }
                Err(_) => break,
            }
        }
        self.wire_closed();
    }

    fn read_stderr(self: &Arc<Self>, mut reader: std::process::ChildStderr) {
        const DIAGNOSTIC_TAIL_BYTES: usize = 512;
        let max_total = self.limits.max_stderr_bytes;
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => {
                    self.facts
                        .bytes_stderr
                        .fetch_add(read as u64, Ordering::SeqCst);
                    if max_total > 0 && self.facts.bytes_stderr.load(Ordering::SeqCst) > max_total {
                        // Diagnostics are dropped, never buffered past the bound.
                        continue;
                    }
                    let text = String::from_utf8_lossy(&chunk[..read]);
                    let mut tail = lock(&self.facts.stderr_tail);
                    tail.push_str(&text);
                    if tail.len() > DIAGNOSTIC_TAIL_BYTES {
                        let cut = tail.len() - DIAGNOSTIC_TAIL_BYTES;
                        let boundary = tail
                            .char_indices()
                            .map(|(index, _)| index)
                            .find(|index| *index >= cut)
                            .unwrap_or(tail.len());
                        *tail = tail.split_off(boundary);
                    }
                }
                Err(_) => break,
            }
        }
    }

    /// The stdout pipe closed: either the process exited or nothing more will be
    /// written. Both make outstanding calls unanswerable.
    fn wire_closed(&self) {
        let status = {
            let mut child = lock(&self.child);
            child.try_wait().ok().flatten()
        };
        if let Some(status) = status {
            self.record_exit(ObservedExit::from_status(status));
        }
        let mut state = lock(&self.state);
        if state.fault.is_none() && !state.shutting_down {
            state.fault = Some(FaultClass::Crashed);
            drop(state);
            let mut fault = lock(&self.facts.fault);
            if fault.is_none() {
                *fault = Some(format!("{}: wire_closed", fault_name(FaultClass::Crashed)));
            }
            drop(fault);
        } else {
            drop(state);
        }
        self.wake.notify_all();
    }

    /// Whether every writer of the stdout pipe is gone.
    fn pipe_closed(&self) -> bool {
        lock(&self.stdout_reader)
            .as_ref()
            .map(JoinHandle::is_finished)
            .unwrap_or(true)
    }
}

/// The two answers a wire call can produce: a result, or the extension's own
/// refusal. A refusal is not a fault: the process is still the one serving the
/// next call.
enum WireAnswer {
    Result(Value),
    Refused { code: String, message: String },
}

/// The untrusted-process carrier.
pub struct IsolatedProcessCarrier {
    programs: Arc<dyn ProgramSource>,
    policy: Arc<IsolationPolicy>,
    ledger: Arc<IsolationLedger>,
    confinement: PlatformConfinement,
    facts: Mutex<BTreeMap<String, Arc<InstanceFacts>>>,
}

impl IsolatedProcessCarrier {
    /// Build a carrier for one managed root.
    ///
    /// Refuses here — before any instance exists — when the policy asks for
    /// confinement or limits this host cannot enforce, so a restricted request
    /// never quietly becomes an unconfined run.
    pub fn new(
        programs: Arc<dyn ProgramSource>,
        policy: IsolationPolicy,
    ) -> Result<Arc<Self>, ApplicationFailure> {
        let confinement = PlatformConfinement::detect();
        if policy.mode.claims_confinement() && !confinement.supports_restricted() {
            return Err(confinement::confinement_unavailable(&confinement));
        }
        limits::check_supported(&policy.limits, &confinement)?;
        let ledger = Arc::new(IsolationLedger::open(&policy.managed_root)?);
        Ok(Arc::new(Self {
            programs,
            policy: Arc::new(policy),
            ledger,
            confinement,
            facts: Mutex::new(BTreeMap::new()),
        }))
    }

    /// What this host can enforce, as probed at construction.
    pub fn capabilities(&self) -> &PlatformConfinement {
        &self.confinement
    }

    pub fn policy(&self) -> &Arc<IsolationPolicy> {
        &self.policy
    }

    pub fn ledger(&self) -> &Arc<IsolationLedger> {
        &self.ledger
    }

    /// The live facts of one instance.
    pub fn facts(&self, instance_id: &str) -> Option<Arc<InstanceFacts>> {
        lock(&self.facts).get(instance_id).cloned()
    }

    /// Record that new capability is withdrawn for one package or instance.
    ///
    /// The composition calls this when it withdraws admission (the host's own
    /// `revoke` does not know about the OS envelope). It records the decision
    /// and nothing about effects: work already admitted stays settled by its
    /// original owner.
    pub fn revoke(
        &self,
        package_id: &str,
        instance_id: Option<&str>,
        reason: RevocationReason,
    ) -> Result<(), ApplicationFailure> {
        let generation = instance_id.and_then(|id| self.facts(id).map(|facts| facts.generation));
        self.ledger
            .record_revocation(package_id, instance_id, generation, reason)
    }

    fn launch(&self, spec: &CarrierSpec) -> Result<Arc<WireSession>, ApplicationFailure> {
        let program = self.programs.resolve(spec)?;
        let validated =
            confinement::validate_program(self.policy.mode, &self.policy.managed_root, &program)?;
        let (mut command, confinement) = confinement::build_command(
            self.policy.mode,
            self.policy.network,
            &validated,
            &self.confinement,
        )?;
        let enforced = limits::apply(&mut command, &self.policy.limits, &self.confinement)?;

        command.env_clear();
        for (key, value) in scrubbed_environment(&self.policy, &validated) {
            command.env(key, value);
        }
        command
            .current_dir(&validated.working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = SupervisedChild::spawn(&mut command).map_err(|error| {
            refusal("extension_isolation_spawn_failed", "extension/isolation")
                .with_field("executable")
                .with_presentation_arg("kind", &format!("{:?}", error.kind()))
        })?;
        let pid = child.pid();
        // The process exists from here on, so its facts exist from here on too:
        // every failure path below must leave the pid, the observed exit (if
        // any) and an unconfirmed-release marker where an operator can find
        // them, instead of only returning an error.
        let facts = Arc::new(InstanceFacts {
            instance_id: spec.instance_id.clone(),
            package_id: spec.package_id.clone(),
            package_version: spec.package_version.clone(),
            generation: spec.generation,
            pid,
            mode: self.policy.mode,
            confinement: confinement.clone(),
            network: self.policy.network,
            enforced,
            bytes_stdout: AtomicU64::new(0),
            bytes_stderr: AtomicU64::new(0),
            stderr_tail: Mutex::new(String::new()),
            frames_rejected: AtomicU64::new(0),
            terminal_events: AtomicU64::new(0),
            dispatched: AtomicU64::new(0),
            fault: Mutex::new(None),
            exit: Mutex::new(None),
            released: AtomicBool::new(false),
            forced: AtomicBool::new(false),
            unconfirmed_release: AtomicBool::new(false),
            pipe_held_after_release: AtomicBool::new(false),
            release_scope: Mutex::new(None),
        });
        lock(&self.facts).insert(spec.instance_id.clone(), Arc::clone(&facts));

        let Some(stdout) = child.stdout() else {
            let cleanup = child.terminate_tree();
            return Err(unrecorded_process_failure(
                refusal("extension_isolation_spawn_failed", "extension/isolation")
                    .with_field("stdout"),
                pid,
                cleanup,
                &facts,
            ));
        };
        let Some(stderr) = child.stderr() else {
            let cleanup = child.terminate_tree();
            return Err(unrecorded_process_failure(
                refusal("extension_isolation_spawn_failed", "extension/isolation")
                    .with_field("stderr"),
                pid,
                cleanup,
                &facts,
            ));
        };
        let Some(stdin) = child.stdin() else {
            let cleanup = child.terminate_tree();
            return Err(unrecorded_process_failure(
                refusal("extension_isolation_spawn_failed", "extension/isolation")
                    .with_field("stdin"),
                pid,
                cleanup,
                &facts,
            ));
        };

        let declaration = ResourceDeclaration {
            package_id: spec.package_id.clone(),
            package_version: spec.package_version.clone(),
            instance_id: spec.instance_id.clone(),
            generation: spec.generation,
            mode: self.policy.mode,
            confinement: confinement.clone(),
            read_roots: display_paths(&validated.read_roots),
            write_root: validated.write_root.display().to_string(),
            executable: validated.executable.display().to_string(),
            exec_paths: display_paths(&validated.exec_paths),
            network: self.policy.network,
            limits: self.policy.limits,
            enforced,
            limit_scope: self.policy.limit_scope(),
            declared_at_unix_ms: crate::platform::extension_packages::now_unix_ms(),
        };
        // The grant that governs the process is recorded before the handshake.
        // A record that cannot be written stops the instance, and the stopping
        // is evidence, not an assumption: the wait decides, and a request to
        // terminate is never treated as an exit.
        if let Err(failure) = self.ledger.record_grant(&declaration) {
            let cleanup = child.terminate_tree();
            return Err(unrecorded_process_failure(failure, pid, cleanup, &facts));
        }

        let session = Arc::new(WireSession {
            facts: Arc::clone(&facts),
            declaration,
            limits: self.policy.limits,
            shutdown_grace: self.policy.shutdown_grace(),
            child: Mutex::new(child),
            stdin: Mutex::new(Some(BoundedStdinWriter::new(stdin))),
            state: Mutex::new(WireState {
                next_id: 0,
                responses: BTreeMap::new(),
                ready: None,
                fault: None,
                exit: None,
                shutting_down: false,
                terminal: BTreeMap::new(),
                refs: BTreeSet::new(),
                resume_supported: false,
            }),
            wake: Condvar::new(),
            shutdown_lock: Mutex::new(()),
            release_outcome: Mutex::new(None),
            stdout_reader: Mutex::new(None),
            stderr_reader: Mutex::new(None),
            released: AtomicBool::new(false),
        });

        let stdout_handle = thread::spawn({
            let session = Arc::clone(&session);
            move || session.read_stdout(stdout)
        });
        let stderr_handle = thread::spawn({
            let session = Arc::clone(&session);
            move || session.read_stderr(stderr)
        });
        *lock(&session.stdout_reader) = Some(stdout_handle);
        *lock(&session.stderr_reader) = Some(stderr_handle);
        lock(&self.facts).insert(spec.instance_id.clone(), facts);
        Ok(session)
    }

    fn session(&self, session: &CarrierSession) -> Result<Arc<WireSession>, ApplicationFailure> {
        match session.get::<Arc<WireSession>>() {
            Some(inner) => Ok((*inner).clone()),
            None => Err(
                refusal("extension_isolation_session_foreign", "extension/isolation")
                    .with_field("session"),
            ),
        }
    }

    fn initialize(
        &self,
        session: &Arc<WireSession>,
        request: &InitializeRequest,
    ) -> Result<InitializedProfileSet, ApplicationFailure> {
        let profiles: Vec<Value> = request
            .profiles
            .iter()
            .map(|profile| {
                json!({
                    "id": profile.id,
                    "major": profile.major,
                    "capabilities": profile.capabilities,
                })
            })
            .collect();
        let answer = session.call(
            profile::METHOD_INITIALIZE,
            json!({
                "protocol": {
                    "major": request.host_contract_range.major,
                    "minimumMinor": request.host_contract_range.minimum_minor,
                },
                "maxFrameBytes": session.limits.max_frame_bytes,
                "profiles": profiles,
            }),
        )?;
        let accepted_profiles: Vec<String> = answer
            .get("profiles")
            .and_then(Value::as_array)
            .map(|profiles| {
                profiles
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();

        // The runtime's own description is the live fact for the optional
        // abilities; it is asked before readiness so the profile decision has it.
        let (methods, resume_supported) =
            match session.call_answer(profile::METHOD_AGENT_DESCRIBE, json!({})) {
                Ok(WireAnswer::Result(description)) => describe_methods(&description),
                _ => (baseline_methods(), false),
            };
        {
            let mut state = lock(&session.state);
            state.resume_supported = resume_supported;
        }
        Ok(InitializedProfileSet {
            accepted_profiles,
            methods,
        })
    }

    fn await_ready(&self, session: &Arc<WireSession>) -> Result<(), ApplicationFailure> {
        let wall = Duration::from_millis(session.limits.call_wall_ms.min(2_000));
        let deadline = Instant::now() + wall;
        let mut state = lock(&session.state);
        loop {
            if state.ready.is_some() {
                return Ok(());
            }
            if let Some(class) = state.fault {
                return Err(carrier_fault(class));
            }
            if state.exit.is_some() {
                return Err(carrier_fault(FaultClass::Crashed));
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(carrier_fault(FaultClass::Protocol));
            }
            let (guard, _) = session
                .wake
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state = guard;
        }
    }

    fn dispatch(
        &self,
        session: &Arc<WireSession>,
        binding: &super::super::invocation::InvocationBinding,
        request: &Value,
    ) -> Result<DispatchOutcome, ApplicationFailure> {
        let wire_ref = binding.invocation_id().to_owned();
        lock(&session.state).refs.insert(wire_ref.clone());
        let answer = session.call_answer(
            profile::METHOD_AGENT_EXECUTE,
            json!({ "invocationRef": wire_ref, "input": request_input(request) }),
        )?;
        session.facts.dispatched.fetch_add(1, Ordering::SeqCst);
        match answer {
            WireAnswer::Refused { code, message } => Err(refusal(
                "extension_isolation_execute_refused",
                "extension/isolation",
            )
            .with_presentation_arg("wireCode", &code)
            .with_presentation_arg("wireMessage", &message)),
            WireAnswer::Result(result) => {
                let admission: Admission = serde_json::from_value(result)
                    .map_err(|_| carrier_fault(FaultClass::Protocol))?;
                match admission.outcome() {
                    // Admission is never completion: the terminal event or the
                    // observed process death decides the outcome. A receipt this
                    // carrier cannot turn into a settled result (not-started,
                    // duplicate, unknown) leaves the host's binding admitted, so
                    // the call is only ever settled by its own owner — never by
                    // a re-dispatch from here.
                    AdmissionOutcome::Accepted
                    | AdmissionOutcome::Duplicate
                    | AdmissionOutcome::NotStarted
                    | AdmissionOutcome::Unknown => Ok(DispatchOutcome::Admitted),
                }
            }
        }
    }

    fn poll(
        &self,
        session: &Arc<WireSession>,
        wire_ref: &str,
    ) -> Result<Observation, ApplicationFailure> {
        if let Some(body) = session.terminal_body(wire_ref) {
            return Ok(Observation::Completed { payload: body });
        }
        // A broken wire is a fault the host must classify, not an answer this
        // carrier invents; the terminal record above is the only completion.
        {
            let state = lock(&session.state);
            if let Some(class) = state.fault {
                return Err(carrier_fault(class));
            }
        }
        if session.process_exited() {
            return Ok(Observation::Unknown {
                code: PROCESS_EXIT_WITHOUT_TERMINAL.to_owned(),
            });
        }
        if session.resume_supported() {
            // An extension that declares resume may answer from its own record.
            // A refusal is expected from a runtime that records nothing; it is
            // not a fault, and the carrier's own terminal record stays the
            // decider.
            if let Ok(WireAnswer::Refused { .. }) = session.call_answer(
                profile::METHOD_AGENT_OBSERVE,
                json!({ "priorInvocationRef": wire_ref, "cursor": "" }),
            ) {
                return Ok(Observation::Running);
            }
            if let Some(body) = session.terminal_body(wire_ref) {
                return Ok(Observation::Completed { payload: body });
            }
        }
        Ok(Observation::Running)
    }

    fn cancel(
        &self,
        session: &Arc<WireSession>,
        binding: &super::super::invocation::InvocationBinding,
    ) -> Result<CancelDisposition, ApplicationFailure> {
        let wire_ref = binding.invocation_id().to_owned();
        match session.call_answer(
            profile::METHOD_AGENT_CANCEL,
            json!({ "invocationRef": wire_ref }),
        )? {
            // The published SDK answers an unimplemented optional method with
            // JSON-RPC "method not found" (-32601), and other implementations
            // name it in the message. Both are the extension saying it has no
            // cancel, which is a fact to report rather than a fault.
            WireAnswer::Refused { code, message }
                if code == "-32601" || message == "unsupported_method" =>
            {
                Ok(CancelDisposition::Unsupported)
            }
            WireAnswer::Refused { .. } => Ok(CancelDisposition::Unknown),
            WireAnswer::Result(result) => {
                // The wire carries the outcome inside a receipt object; the
                // contract enum itself is the bare outcome.
                let outcome: CancelOutcome =
                    serde_json::from_value(result.get("outcome").cloned().unwrap_or(Value::Null))
                        .map_err(|_| carrier_fault(FaultClass::Protocol))?;
                Ok(match outcome {
                    CancelOutcome::Requested => CancelDisposition::Requested,
                    CancelOutcome::Acknowledged => CancelDisposition::Acknowledged,
                    CancelOutcome::Unsupported => CancelDisposition::Unsupported,
                    CancelOutcome::Unknown => CancelDisposition::Unknown,
                })
            }
        }
    }

    fn shutdown(&self, session: &Arc<WireSession>) -> Result<(), ApplicationFailure> {
        let _guard = lock(&session.shutdown_lock);
        // Take the cached outcome into a local first: a `match` on a locked
        // scrutinee would hold the guard through the branch that stores it.
        let cached = *lock(&session.release_outcome);
        let outcome = match cached {
            Some(outcome) => outcome,
            None => {
                let outcome = self.release_once(session)?;
                *lock(&session.release_outcome) = Some(outcome);
                outcome
            }
        };
        // What a release covers is a property of the mode, not of a lucky wait:
        // a restricted instance is one process, so its observed exit is the
        // whole instance. A trusted local program may have descendants that left
        // the group, and a closed pipe never proves otherwise, so its owner must
        // stay unverified.
        match outcome.scope {
            ReleaseScope::Instance => Ok(()),
            ReleaseScope::ProcessGroup => Err(descendants_unverified(&outcome)),
        }
    }

    /// Stop the instance once and record what was observed.
    ///
    /// Returns the evidence; whether that evidence is enough to call the whole
    /// instance released is decided by the caller from the scope, never from the
    /// root's exit alone.
    fn release_once(
        &self,
        session: &Arc<WireSession>,
    ) -> Result<ReleaseOutcome, ApplicationFailure> {
        {
            let mut state = lock(&session.state);
            state.shutting_down = true;
        }
        // Ask for an ordered exit; the answer is not evidence, the observed exit
        // status is.
        let _ = session.call_answer(profile::METHOD_SHUTDOWN, json!({}));
        {
            let mut stdin = lock(&session.stdin);
            if let Some(mut writer) = stdin.take() {
                let _ = writer.finish(IO_THREAD_EXIT_GRACE);
            }
        }
        // Give every writer of the stdout pipe the grace window to finish. The
        // root stays unreaped while we wait, so its process-group id cannot be
        // recycled before the group is torn down; that is what reclaims a
        // descendant still holding the pipe.
        let deadline = Instant::now() + session.shutdown_grace;
        while !session.pipe_closed() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        let observed = {
            let mut child = lock(&session.child);
            child.terminate_tree()
        };
        let (exit, forced) = match observed {
            Ok(Some(status)) => {
                let exit = ObservedExit::from_status(status);
                (exit, exit.signal.is_some())
            }
            Ok(None) | Err(_) => {
                session.note_fault(FaultClass::Unresponsive, "release_unconfirmed");
                return Err(confinement::release_unconfirmed("exit_not_observed"));
            }
        };
        session.record_exit(exit);
        // The process group was signalled. Give its own members a bounded moment
        // to close the pipe, then ask whether any writer is still alive: that
        // residue is a descendant that left the group. A closed pipe is residue
        // evidence only — it never proves that no descendant survives.
        let held_deadline = Instant::now() + IO_THREAD_EXIT_GRACE;
        while !session.pipe_closed() && Instant::now() < held_deadline {
            thread::sleep(Duration::from_millis(20));
        }
        let pipe_held = !session.pipe_closed();
        let stdout = lock(&session.stdout_reader).take();
        if let Some(handle) = stdout {
            let _ = join_bounded(handle, IO_THREAD_EXIT_GRACE);
        }
        let stderr = lock(&session.stderr_reader).take();
        if let Some(handle) = stderr {
            let _ = join_bounded(handle, IO_THREAD_EXIT_GRACE);
        }
        let outcome = ReleaseOutcome {
            exit,
            forced,
            pipe_held,
            scope: self.policy.release_scope(),
        };
        session
            .facts
            .pipe_held_after_release
            .store(pipe_held, Ordering::SeqCst);
        *lock(&session.facts.release_scope) = Some(outcome.scope);
        self.ledger
            .record_release(&session.declaration, exit, forced, pipe_held, outcome.scope)?;
        session.released.store(true, Ordering::SeqCst);
        session.facts.released.store(true, Ordering::SeqCst);
        session.facts.forced.store(forced, Ordering::SeqCst);
        Ok(outcome)
    }
}

impl ExtensionCarrier for IsolatedProcessCarrier {
    fn start(&self, spec: &CarrierSpec) -> Result<CarrierSession, ApplicationFailure> {
        let session = self.launch(spec)?;
        Ok(CarrierSession::new(Arc::clone(&session)))
    }

    fn initialize(
        &self,
        session: &CarrierSession,
        request: &InitializeRequest,
    ) -> Result<InitializedProfileSet, ApplicationFailure> {
        let session = self.session(session)?;
        self.initialize(&session, request)
    }

    fn ready(&self, session: &CarrierSession) -> Result<(), ApplicationFailure> {
        let session = self.session(session)?;
        self.await_ready(&session)
    }

    fn dispatch(
        &self,
        session: &CarrierSession,
        binding: &super::super::invocation::InvocationBinding,
        request: &Value,
    ) -> Result<DispatchOutcome, ApplicationFailure> {
        let session = self.session(session)?;
        self.dispatch(&session, binding, request)
    }

    fn observe(
        &self,
        session: &CarrierSession,
        binding: &super::super::invocation::InvocationBinding,
    ) -> Result<Observation, ApplicationFailure> {
        let session = self.session(session)?;
        let Some(wire_ref) = session.wire_ref(binding) else {
            return Err(unknown_binding(binding));
        };
        self.poll(&session, &wire_ref)
    }

    fn cancel(
        &self,
        session: &CarrierSession,
        binding: &super::super::invocation::InvocationBinding,
    ) -> Result<CancelDisposition, ApplicationFailure> {
        let session = self.session(session)?;
        if session.wire_ref(binding).is_none() {
            return Err(unknown_binding(binding));
        }
        self.cancel(&session, binding)
    }

    fn result(
        &self,
        session: &CarrierSession,
        binding: &super::super::invocation::InvocationBinding,
    ) -> Result<Observation, ApplicationFailure> {
        let session = self.session(session)?;
        let Some(wire_ref) = session.wire_ref(binding) else {
            return Err(unknown_binding(binding));
        };
        self.poll(&session, &wire_ref)
    }

    fn shutdown(&self, session: &CarrierSession) -> Result<(), ApplicationFailure> {
        let session = self.session(session)?;
        self.shutdown(&session)
    }
}

/// What one release attempt observed.
#[derive(Clone, Copy, Debug)]
struct ReleaseOutcome {
    exit: ObservedExit,
    forced: bool,
    pipe_held: bool,
    scope: ReleaseScope,
}

/// The refusal for a release that covers only the supervised process group.
///
/// The process's exit *was* observed; what is not proven is that nothing else
/// survived it. The owner must stay unverified, because a closed pipe and a
/// reaped root are not evidence about a descendant that left the group.
fn descendants_unverified(outcome: &ReleaseOutcome) -> ApplicationFailure {
    uncertain(
        "extension_isolation_descendants_unverified",
        "extension/isolation",
    )
    .with_field("scope")
    .with_presentation_arg("scope", outcome.scope.id())
    .with_presentation_arg("processExit", &format!("{:?}", outcome.exit))
    .with_presentation_arg(
        "processForced",
        if outcome.forced { "true" } else { "false" },
    )
    .with_presentation_arg(
        "stdoutPipeHeld",
        if outcome.pipe_held { "true" } else { "false" },
    )
}

/// What the carrier knows after trying to clean up a process it had to stop
/// outside the normal release path.
#[derive(Debug)]
enum CleanupEvidence {
    /// The wait reported an exit status: the process is gone and this is the
    /// evidence.
    Observed(ObservedExit),
    /// No exit was observed. The request to terminate is not evidence, so the
    /// process must be treated as possibly alive.
    Unconfirmed,
}

fn cleanup_evidence(
    cleanup: Result<Option<std::process::ExitStatus>, LifecycleFailure>,
) -> CleanupEvidence {
    match cleanup {
        Ok(Some(status)) => CleanupEvidence::Observed(ObservedExit::from_status(status)),
        Ok(None) | Err(_) => CleanupEvidence::Unconfirmed,
    }
}

/// Build the failure for a process that existed while its envelope could not be
/// recorded or completed.
///
/// Never reports "no live process" from a kill request: an observed exit is
/// recorded as a fact, and anything else is an uncertain failure that names the
/// pid, because the process may still be running unrecorded.
fn unrecorded_process_failure(
    failure: ApplicationFailure,
    pid: u32,
    cleanup: Result<Option<std::process::ExitStatus>, LifecycleFailure>,
    facts: &InstanceFacts,
) -> ApplicationFailure {
    match cleanup_evidence(cleanup) {
        CleanupEvidence::Observed(exit) => {
            {
                let mut slot = lock(&facts.exit);
                if slot.is_none() {
                    *slot = Some(exit);
                }
            }
            failure
                .with_presentation_arg("processPid", &pid.to_string())
                .with_presentation_arg("processExitObserved", &format!("{exit:?}"))
        }
        CleanupEvidence::Unconfirmed => {
            facts.unconfirmed_release.store(true, Ordering::SeqCst);
            uncertain(
                "extension_isolation_unrecorded_process",
                "extension/isolation",
            )
            .with_field("record")
            .with_presentation_arg("processPid", &pid.to_string())
            .with_presentation_arg("recordFailure", &failure.code)
        }
    }
}

/// The refusal for a binding this session never dispatched.
fn unknown_binding(binding: &super::super::invocation::InvocationBinding) -> ApplicationFailure {
    refusal(
        "extension_isolation_unknown_invocation",
        "extension/isolation",
    )
    .with_field("invocationId")
    .with_presentation_arg("invocationId", binding.invocation_id())
}

/// The minimal environment a child is given: no inherited host environment
/// beyond path and locale, a home and temporary directory inside its own
/// writable root, and whatever the package declares on top.
fn scrubbed_environment(
    policy: &IsolationPolicy,
    program: &ValidatedProgram,
) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = Vec::new();
    for key in ["PATH", "LANG", "LC_ALL", "LC_CTYPE"] {
        if let Some(value) = std::env::var_os(key) {
            if !value.is_empty() {
                env.push((key.to_owned(), value.to_string_lossy().into_owned()));
            }
        }
    }
    let home = program.write_root.display().to_string();
    env.push(("HOME".to_owned(), home.clone()));
    env.push(("TMPDIR".to_owned(), home.clone()));
    // The working directory is pinned to the writable root; say so explicitly
    // so a shell does not have to guess it from an environment it no longer
    // inherits.
    env.push((
        "PWD".to_owned(),
        program.working_directory.display().to_string(),
    ));
    env.push(("TERM".to_owned(), "dumb".to_owned()));
    env.push(("NO_COLOR".to_owned(), "1".to_owned()));
    if policy.mode == IsolationMode::TrustedLocal {
        // A trusted local program keeps the user's own configuration surface;
        // it is the user's software, not third-party code under confinement.
        for key in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"] {
            if let Some(value) = std::env::var_os(key) {
                if !value.is_empty() {
                    env.push((key.to_owned(), value.to_string_lossy().into_owned()));
                }
            }
        }
    }
    for (key, value) in &program.env {
        if key.is_empty() || key.contains('=') || key.contains('\0') || value.contains('\0') {
            continue;
        }
        env.retain(|(existing, _)| existing != key);
        env.push((key.clone(), value.clone()));
    }
    env
}

/// The text an `agent.execute` carries. A request that already names its input
/// keeps it verbatim; anything else is carried as compact JSON text, which the
/// extension may treat as opaque content.
fn request_input(request: &Value) -> String {
    match request {
        Value::String(text) => text.clone(),
        Value::Object(map) => match map.get("input") {
            Some(Value::String(text)) => text.clone(),
            _ => serde_json::to_string(request).unwrap_or_default(),
        },
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// The methods the baseline three-call Agent implements.
fn baseline_methods() -> DeclaredMethods {
    DeclaredMethods::new([
        profile::METHOD_INITIALIZE,
        profile::METHOD_READY,
        profile::METHOD_SHUTDOWN,
        profile::METHOD_AGENT_DESCRIBE,
        profile::METHOD_AGENT_EXECUTE,
        profile::METHOD_AGENT_EVENT,
    ])
}

/// The methods a described runtime really offers: the baseline plus the optional
/// abilities it says are supported. An ability it does not claim is not
/// declared, so nothing downstream promises it.
fn describe_methods(description: &Value) -> (DeclaredMethods, bool) {
    let mut names: Vec<&str> = vec![
        profile::METHOD_INITIALIZE,
        profile::METHOD_READY,
        profile::METHOD_SHUTDOWN,
        profile::METHOD_AGENT_DESCRIBE,
        profile::METHOD_AGENT_EXECUTE,
        profile::METHOD_AGENT_EVENT,
    ];
    let cancel_supported = description.get("cancel").and_then(Value::as_str) == Some("supported");
    let resume_supported = description.get("resume").and_then(Value::as_str) == Some("supported");
    if cancel_supported {
        names.push(profile::METHOD_AGENT_CANCEL);
    }
    if resume_supported {
        names.push(profile::METHOD_AGENT_OBSERVE);
        names.push(profile::METHOD_AGENT_RESUME);
    }
    (DeclaredMethods::new(names), resume_supported)
}

fn display_paths(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn facts() -> InstanceFacts {
        InstanceFacts {
            instance_id: "instance-test".to_owned(),
            package_id: "acme.test/ext".to_owned(),
            package_version: "1.0.0".to_owned(),
            generation: 1,
            pid: 4242,
            mode: IsolationMode::TrustedLocal,
            confinement: Support::unavailable("test"),
            network: NetworkGrant::Denied,
            enforced: ResourceLimits::default().enforced(true),
            bytes_stdout: AtomicU64::new(0),
            bytes_stderr: AtomicU64::new(0),
            stderr_tail: Mutex::new(String::new()),
            frames_rejected: AtomicU64::new(0),
            terminal_events: AtomicU64::new(0),
            dispatched: AtomicU64::new(0),
            fault: Mutex::new(None),
            exit: Mutex::new(None),
            released: AtomicBool::new(false),
            forced: AtomicBool::new(false),
            unconfirmed_release: AtomicBool::new(false),
            pipe_held_after_release: AtomicBool::new(false),
            release_scope: Mutex::new(None),
        }
    }

    /// A request to terminate is not an exit: only a wait result is evidence.
    #[test]
    fn a_kill_request_alone_is_not_an_observed_exit() {
        assert!(matches!(
            cleanup_evidence(Err(LifecycleFailure::Wait)),
            CleanupEvidence::Unconfirmed
        ));
        assert!(matches!(
            cleanup_evidence(Ok(None)),
            CleanupEvidence::Unconfirmed
        ));
    }

    #[test]
    fn a_wait_result_is_recorded_as_the_exit() {
        let status = Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .status()
            .expect("a real exit status for the mapping");
        match cleanup_evidence(Ok(Some(status))) {
            CleanupEvidence::Observed(exit) => {
                assert!(exit.success);
                assert_eq!(exit.code, Some(0));
            }
            CleanupEvidence::Unconfirmed => panic!("a wait result is evidence"),
        }
    }

    #[test]
    fn an_unconfirmed_cleanup_never_claims_a_release() {
        let facts = facts();
        let failure = unrecorded_process_failure(
            refusal(
                "extension_isolation_record_unavailable",
                "extension/isolation",
            ),
            4242,
            Ok(None),
            &facts,
        );
        assert_eq!(failure.code, "extension_isolation_unrecorded_process");
        assert_eq!(failure.presentation_args.get("processPid"), Some("4242"));
        assert_eq!(
            failure.presentation_args.get("recordFailure"),
            Some("extension_isolation_record_unavailable")
        );
        assert!(facts.release_unconfirmed());
        assert!(facts.exit().is_none());
        assert!(!facts.released());
    }

    #[test]
    fn an_observed_cleanup_keeps_the_record_failure_and_the_exit() {
        let facts = facts();
        let status = Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .status()
            .expect("a real exit status");
        let failure = unrecorded_process_failure(
            refusal(
                "extension_isolation_record_unavailable",
                "extension/isolation",
            ),
            4242,
            Ok(Some(status)),
            &facts,
        );
        assert_eq!(failure.code, "extension_isolation_record_unavailable");
        assert_eq!(failure.presentation_args.get("processPid"), Some("4242"));
        assert!(facts.exit().is_some_and(|exit| exit.success));
        assert!(!facts.release_unconfirmed());
        assert!(
            !facts.released(),
            "no record was written, so no release is claimed"
        );
    }
}
