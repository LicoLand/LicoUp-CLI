use super::super::*;
use anyhow::anyhow;
use licoup_conversation::continuity::{
    TRUSTED_RESPONSE_MODE_ASSISTANT_TURN, apply_admitted_validation_failure_facts,
    project_admitted_known_text_fields, public_admitted_output,
    redact_live_runtime_event_with_assembly,
};
use licoup_native::domain::assistant_continuity::execution::CONTINUITY_KIND_USER_POSTED;
use licoup_native::domain::client_conversation::{
    ConversationRuntimeScope, ConversationStore, DispatchState, SubagentDispatchClaim,
    SubagentDispatchClaimState,
};
use licoup_native::ffi::generated::client_error::ClientError;
use licoup_native::platform::runtime_adapters::RuntimeAdapterError;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex, Weak,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

pub(super) const MAX_CONCURRENT_SENDS: usize = 16;
const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(5);
const MAX_TRACKED_TURNS: usize = 64;
const DEFAULT_TURN_CACHE_BYTES: usize = 16 * 1024 * 1024;
const REPLAY_PAGE_SIZE: usize = 256;

#[path = "conversation/execution.rs"]
mod execution;
pub(super) use execution::spawn_execution;

#[derive(Clone)]
pub(crate) struct PersistentConversationRuntime {
    inner: Arc<PersistentConversationRuntimeInner>,
}

struct PersistentConversationRuntimeInner {
    turns: Mutex<HashMap<String, Arc<PersistentTurn>>>,
    execution_observers: Mutex<HashMap<(String, String), Arc<execution::ExecutionObservation>>>,
    turns_changed: Condvar,
    clients: AtomicUsize,
    store: ConversationStore,
    cache_budget: usize,
    /// Fired-once guard for subagent caller callbacks, keyed by claim id. One
    /// completion signal per claim is the contract: terminal settlement and
    /// the timeout watchdog race here and the loser stays silent. Deadlines
    /// are durable on the claim; host boot re-arms the in-memory watcher.
    subagent_callback_fired: Mutex<BTreeSet<String>>,
    /// Watchdog deadlines (claim/dispatch id → fire-at) for claimed subagent
    /// dispatches after the writable timeout policy resolves. One watcher
    /// thread wakes at the earliest registered deadline.
    subagent_watchdog: Mutex<BTreeMap<String, Instant>>,
    subagent_watchdog_changed: Condvar,
    subagent_watchdog_spawned: AtomicBool,
    settlement_hook: Mutex<Option<Arc<dyn Fn(&str, &Value) -> Result<(), String> + Send + Sync>>>,
    live_turn_observer: Mutex<Option<Arc<dyn Fn(&str, &str, &str, &str) + Send + Sync>>>,
    host_stop_requested: AtomicBool,
}

pub(super) struct PersistentTurn {
    scope: ConversationRuntimeScope,
    agent_id: String,
    session_id: Mutex<String>,
    turn_id: Mutex<String>,
    state: Mutex<PersistentTurnState>,
    cancel_requested: AtomicBool,
    changed: Condvar,
    store: ConversationStore,
    cache_budget: usize,
    /// Back-reference used to dispatch the subagent caller callback after a
    /// terminal settlement. Weak so a finished turn never keeps the runtime
    /// alive.
    runtime: Weak<PersistentConversationRuntimeInner>,
    continuity_kind: Option<String>,
    admitted_assistant_turn: bool,
}

#[derive(Default)]
struct PersistentTurnState {
    cache: VecDeque<CachedFrame>,
    cache_bytes: usize,
    high_water: u64,
    execution_generation: u64,
    raw_capture_failed: bool,
    native_provenance_keys: BTreeSet<(String, String, String)>,
    terminal: Option<PersistentTerminal>,
    live_raw_text: String,
}

impl PersistentTurnState {
    fn take_live_assembly(&mut self, event: &Value) -> String {
        let kind = event.get("event").and_then(Value::as_str).unwrap_or("");
        let text = event
            .pointer("/payload/text")
            .and_then(Value::as_str)
            .unwrap_or("");
        match kind {
            "agent.message.chunk" => {
                self.live_raw_text.push_str(text);
                self.live_raw_text.clone()
            }
            "agent.message.completed" => {
                if text.is_empty() {
                    self.live_raw_text.clone()
                } else {
                    text.to_owned()
                }
            }
            _ => self.live_raw_text.clone(),
        }
    }
}

fn redact_store_live_event(event: Value, assembled: &mut String) -> Value {
    let kind = event.get("event").and_then(Value::as_str).unwrap_or("");
    let text = event
        .pointer("/payload/text")
        .and_then(Value::as_str)
        .unwrap_or("");
    let assembly = match kind {
        "agent.message.chunk" => {
            assembled.push_str(text);
            assembled.clone()
        }
        "agent.message.completed" if !text.is_empty() => text.to_owned(),
        _ => assembled.clone(),
    };
    if kind == "agent.message.chunk" || kind == "agent.message.completed" {
        redact_live_runtime_event_with_assembly(&event, &assembly)
    } else {
        event
    }
}

#[derive(Clone)]
struct CachedFrame {
    cursor: u64,
    encoded_bytes: usize,
    event: Value,
}

#[derive(Clone)]
struct PersistentTerminal {
    ok: bool,
    payload: Value,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PersistentTurnAdmission {
    Public,
    Host,
}

impl PersistentTurnAdmission {
    fn continuity_kind(self, params: &Value) -> Option<String> {
        match self {
            Self::Public => None,
            Self::Host => params
                .get("continuityKind")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
        }
    }

    fn admits_assistant_turn(self, params: &Value) -> bool {
        self.continuity_kind(params).as_deref() == Some(CONTINUITY_KIND_USER_POSTED)
    }
}

impl PersistentConversationRuntime {
    pub(crate) fn new(store: ConversationStore) -> Self {
        Self::with_cache_budget(store, DEFAULT_TURN_CACHE_BYTES)
    }

    fn with_cache_budget(store: ConversationStore, cache_budget: usize) -> Self {
        let runtime = Self {
            inner: Arc::new(PersistentConversationRuntimeInner {
                turns: Mutex::new(HashMap::new()),
                execution_observers: Mutex::new(HashMap::new()),
                turns_changed: Condvar::new(),
                clients: AtomicUsize::new(0),
                store,
                cache_budget,
                subagent_callback_fired: Mutex::new(BTreeSet::new()),
                subagent_watchdog: Mutex::new(BTreeMap::new()),
                subagent_watchdog_changed: Condvar::new(),
                subagent_watchdog_spawned: AtomicBool::new(false),
                settlement_hook: Mutex::new(None),
                live_turn_observer: Mutex::new(None),
                host_stop_requested: AtomicBool::new(false),
            }),
        };
        runtime.rearm_persisted_watchdogs();
        runtime
    }

    pub(crate) fn client_connected(&self) {
        let _turns = self
            .inner
            .turns
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.inner.clients.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn client_disconnected(&self) {
        let _turns = self
            .inner
            .turns
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.inner.clients.fetch_sub(1, Ordering::AcqRel);
        self.inner.turns_changed.notify_all();
    }

    pub(crate) fn set_settlement_hook(
        &self,
        hook: impl Fn(&str, &Value) -> Result<(), String> + Send + Sync + 'static,
    ) {
        *self
            .inner
            .settlement_hook
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::new(hook));
    }

    pub(crate) fn set_live_turn_observer(
        &self,
        observer: impl Fn(&str, &str, &str, &str) + Send + Sync + 'static,
    ) {
        *self
            .inner
            .live_turn_observer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::new(observer));
    }

    pub(crate) fn inspect_turn(&self, handle: &str) -> Option<(String, String)> {
        let turn = self.turn(handle)?;
        Some((
            turn.session_id.lock().ok()?.clone(),
            turn.turn_id.lock().ok()?.clone(),
        ))
    }

    #[allow(dead_code)]
    pub(crate) fn idle(&self) -> bool {
        self.inner.clients.load(Ordering::Acquire) == 0
            && self.inner.turns.lock().is_ok_and(|turns| {
                turns.values().all(|turn| {
                    turn.state
                        .lock()
                        .is_ok_and(|state| state.terminal.is_some())
                })
            })
    }

    pub(crate) fn request_host_stop(&self) {
        self.inner
            .host_stop_requested
            .store(true, Ordering::Release);
        self.inner.turns_changed.notify_all();
    }

    pub(crate) fn is_host_stop_requested(&self) -> bool {
        self.inner.host_stop_requested.load(Ordering::Acquire)
    }

    #[allow(dead_code)]
    pub(crate) fn has_active_turns(&self) -> bool {
        let turns = self
            .inner
            .turns
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        turns.values().any(|turn| {
            turn.state
                .lock()
                .map(|state| state.terminal.is_none())
                .unwrap_or(false)
        })
    }

    #[cfg(test)]
    pub(crate) fn drain_admitted_turns(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut turns_guard = self
            .inner
            .turns
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            let active = turns_guard.values().any(|turn| {
                turn.state
                    .lock()
                    .map(|state| state.terminal.is_none())
                    .unwrap_or(false)
            });
            if !active {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            let remaining = deadline - now;
            let (next_guard, wait_res) = self
                .inner
                .turns_changed
                .wait_timeout(turns_guard, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            turns_guard = next_guard;
            if wait_res.timed_out() {
                let still_active = turns_guard.values().any(|turn| {
                    turn.state
                        .lock()
                        .map(|state| state.terminal.is_none())
                        .unwrap_or(false)
                });
                return !still_active;
            }
        }
    }

    /// Wait until every admitted turn has reached a terminal state. Host
    /// relocation and explicit shutdown use this as a quiescence condition;
    /// elapsed time is never treated as proof that writers have stopped.
    pub(crate) fn wait_for_admitted_turns(&self) {
        let mut turns_guard = self
            .inner
            .turns
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        loop {
            let active = turns_guard.values().any(|turn| {
                turn.state
                    .lock()
                    .map(|state| state.terminal.is_none())
                    .unwrap_or(false)
            });
            if !active {
                return;
            }
            turns_guard = self
                .inner
                .turns_changed
                .wait(turns_guard)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    /// Wait until all admitted RPC sessions have left. The count transition
    /// and waiter use the same mutex, so a disconnect notification cannot be
    /// lost between the condition check and Condvar wait.
    pub(crate) fn wait_for_clients_to_disconnect(&self) {
        let mut turns_guard = self
            .inner
            .turns
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while self.inner.clients.load(Ordering::Acquire) != 0 {
            turns_guard = self
                .inner
                .turns_changed
                .wait(turns_guard)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    /// True while a Membership-scoped turn is registered and not terminal.
    /// The designated-Assistant wake uses this to keep a notice timeline-only
    /// when a turn of that membership is already in flight; it never stacks a
    /// second turn for one wake.
    fn live_turn_for_membership(&self, membership_id: &str) -> bool {
        self.inner.turns.lock().is_ok_and(|turns| {
            turns.values().any(|turn| {
                turn.scope.membership_id == membership_id
                    && turn
                        .state
                        .lock()
                        .is_ok_and(|state| state.terminal.is_none())
            })
        })
    }

    fn begin(&self, params: &Value) -> std::result::Result<Arc<PersistentTurn>, ClientError> {
        self.begin_with(params, PersistentTurnAdmission::Public)
    }

    /// Resolve the native context for one Membership-scoped dispatch at the
    /// single runtime boundary. Callback, wake, Graph, and ordinary dispatch
    /// producers may omit the private binding because it is host-owned; the
    /// native adapter still receives the exact session and its working
    /// location when the Membership already has one. An explicit session is
    /// preserved so adapter-owned identity validation remains authoritative.
    fn resolve_dispatch_params(&self, params: &Value) -> Value {
        let Some(object) = params.as_object() else {
            return params.clone();
        };
        let conversation_id = object
            .get("conversationId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let membership_id = object
            .get("membershipId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let binding = match (conversation_id, membership_id) {
            (Some(conversation_id), Some(membership_id)) => self
                .inner
                .store
                .private_runtime_binding(conversation_id, membership_id)
                .ok()
                .flatten(),
            _ => None,
        };
        let mut resolved = params.clone();
        let Some(resolved_object) = resolved.as_object_mut() else {
            return resolved;
        };
        if let Some(binding) = binding.as_ref() {
            let binding_session = binding.runtime_session_id.trim();
            let current_session = resolved_object
                .get("sessionId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if current_session.is_none() && !binding_session.is_empty() {
                resolved_object.insert(
                    "sessionId".to_owned(),
                    Value::String(binding_session.to_owned()),
                );
            }
            let session_matches_binding = resolved_object
                .get("sessionId")
                .and_then(Value::as_str)
                .map(str::trim)
                .is_some_and(|session| session == binding_session);
            if session_matches_binding {
                if resolved_object
                    .get("sourcePath")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .is_none()
                {
                    if let Some(path) = binding
                        .runtime_conversation_path
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        resolved_object.insert("sourcePath".to_owned(), json!(path));
                    }
                }
                if resolved_object
                    .get("workingDirectory")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .is_none()
                {
                    if let Some(cwd) = binding
                        .working_directory
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        resolved_object.insert("workingDirectory".to_owned(), json!(cwd));
                    }
                }
            }
        }
        if let Some(membership_id) = membership_id
            && let Ok(Some(profile)) = self.inner.store.membership_profile(membership_id)
        {
            if resolved_object
                .get("model")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none()
            {
                if let Some(model) = profile
                    .preferred_model
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    resolved_object.insert("model".to_owned(), json!(model));
                }
            }
            if resolved_object
                .get("reasoningEffort")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none()
            {
                if let Some(reasoning) = profile
                    .preferred_reasoning_effort
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    resolved_object.insert("reasoningEffort".to_owned(), json!(reasoning));
                }
            }
            if resolved_object
                .get("workingDirectory")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none()
            {
                if let Some(environment) = profile
                    .preferred_environment
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    resolved_object.insert("workingDirectory".to_owned(), json!(environment));
                }
            }
            if !resolved_object.contains_key("requiredCapabilities")
                && !profile.required_capabilities.is_empty()
            {
                resolved_object.insert(
                    "requiredCapabilities".to_owned(),
                    json!(profile.required_capabilities),
                );
            }
        }
        resolved
    }

    /// Keep an opened turn's admitted session stable until it settles. The
    /// store binding may be updated by another lifecycle observer between
    /// open and run; the handle is the authority for this already-admitted
    /// dispatch.
    fn params_for_turn(&self, turn: &Arc<PersistentTurn>, params: &Value) -> Value {
        let mut resolved = params.clone();
        let session_id = turn.session_id.lock().expect("turn session lock").clone();
        let has_session = resolved
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_some();
        if !has_session
            && !session_id.trim().is_empty()
            && let Some(object) = resolved.as_object_mut()
        {
            object.insert("sessionId".to_owned(), Value::String(session_id));
        }
        self.resolve_dispatch_params(&resolved)
    }

    fn begin_with(
        &self,
        params: &Value,
        admission: PersistentTurnAdmission,
    ) -> std::result::Result<Arc<PersistentTurn>, ClientError> {
        if self.is_host_stop_requested() {
            return Err(stdio_rpc_client_error("conversation_host_shutting_down"));
        }
        let params = self.resolve_dispatch_params(params);
        let agent_id = params
            .get("agent")
            .or_else(|| params.get("agentId"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let session_id = params
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let text = params
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut turns = self.inner.turns.lock().expect("turn registry lock");
        if turns.len() >= MAX_TRACKED_TURNS {
            let completed = turns
                .iter()
                .find(|(_, turn)| {
                    turn.state
                        .lock()
                        .is_ok_and(|state| state.terminal.is_some())
                })
                .map(|(handle, _)| handle.clone());
            if let Some(completed) = completed {
                turns.remove(&completed);
            } else {
                return Err(stdio_rpc_client_error("conversation_capacity_exhausted"));
            }
        }
        let scope = self
            .inner
            .store
            .prepare_runtime_dispatch(
                agent_id,
                session_id,
                text,
                params.get("conversationId").and_then(Value::as_str),
                params.get("membershipId").and_then(Value::as_str),
                params.get("causationId").and_then(Value::as_str),
                params.get("dispatchId").and_then(Value::as_str),
            )
            .map_err(|_| stdio_rpc_client_error("conversation_persistence_failed"))?;
        let continuity_kind = admission.continuity_kind(&params);
        self.inner
            .store
            .record_runtime_request(&scope, &params)
            .map_err(|_| stdio_rpc_client_error("conversation_persistence_failed"))?;
        let admitted_assistant_turn = admission.admits_assistant_turn(&params);
        if admitted_assistant_turn {
            self.inner
                .store
                .admit_runtime_response_mode(&scope, TRUSTED_RESPONSE_MODE_ASSISTANT_TURN)
                .map_err(|_| stdio_rpc_client_error("conversation_persistence_failed"))?;
        }
        let turn = Arc::new(PersistentTurn {
            scope: scope.clone(),
            agent_id: agent_id.to_owned(),
            session_id: Mutex::new(session_id.to_owned()),
            turn_id: Mutex::new(String::new()),
            state: Mutex::new(PersistentTurnState::default()),
            cancel_requested: AtomicBool::new(false),
            changed: Condvar::new(),
            store: self.inner.store.clone(),
            cache_budget: self.inner.cache_budget,
            runtime: Arc::downgrade(&self.inner),
            continuity_kind,
            admitted_assistant_turn,
        });
        turns.insert(scope.dispatch_id.clone(), Arc::clone(&turn));
        self.inner.turns_changed.notify_all();
        // Only a claimed dispatch with a finite resolved timeout gets a
        // settlement watchdog. A zero resolved timeout is unbounded, and an
        // ordinary unclaimed dispatch never registers one.
        let timeout_ms =
            licoup_native::domain::dispatch_timeout_policy::resolve_dispatch_timeout(&params)
                .unwrap_or(0);
        if timeout_ms > 0
            && matches!(
                self.inner.store.subagent_claim(&scope.dispatch_id),
                Ok(Some(_))
            )
        {
            let deadline_unix_ms = unix_now_ms().saturating_add(timeout_ms as i64);
            let _ = self
                .inner
                .store
                .set_subagent_watchdog_deadline(&scope.dispatch_id, deadline_unix_ms);
            self.register_subagent_watchdog(
                scope.dispatch_id.clone(),
                Duration::from_millis(timeout_ms),
            );
        }
        Ok(turn)
    }

    fn turn(&self, handle: &str) -> Option<Arc<PersistentTurn>> {
        self.inner.turns.lock().ok()?.get(handle).cloned()
    }

    #[cfg(test)]
    pub(crate) fn live_message_texts(&self, handle: &str) -> Vec<String> {
        let Some(turn) = self.turn(handle) else {
            return Vec::new();
        };
        let Ok(state) = turn.state.lock() else {
            return Vec::new();
        };
        state
            .cache
            .iter()
            .filter_map(|frame| {
                let kind = frame.event.get("event").and_then(Value::as_str)?;
                if kind != "agent.message.chunk" && kind != "agent.message.completed" {
                    return None;
                }
                frame
                    .event
                    .pointer("/payload/text")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn evict_turn_cache(&self, handle: &str) {
        if let Some(turn) = self.turn(handle)
            && let Ok(mut state) = turn.state.lock()
        {
            state.cache.clear();
            state.cache_bytes = 0;
        }
    }

    #[cfg(test)]
    pub(crate) fn public_terminal(&self, handle: &str) -> Option<(bool, Value)> {
        let turn = self.turn(handle)?;
        let terminal = stored_public_terminal(&turn)?;
        Some((terminal.ok, terminal.payload))
    }

    #[cfg(test)]
    pub(crate) fn turn_high_water(&self, handle: &str) -> Option<u64> {
        Some(self.turn(handle)?.state.lock().ok()?.high_water)
    }

    pub(super) fn scoped_control_params(
        &self,
        params: &Value,
    ) -> std::result::Result<Value, ClientError> {
        let handle = params
            .get("turnHandle")
            .and_then(Value::as_str)
            .filter(|value| valid_turn_handle(value))
            .ok_or_else(|| stdio_rpc_client_error("invalid_turn_handle"))?;
        let conversation_id = params
            .get("conversationId")
            .and_then(Value::as_str)
            .filter(|value| valid_turn_handle(value))
            .ok_or_else(|| stdio_rpc_client_error("invalid_conversation_scope"))?;
        let turn = self
            .turn(handle)
            .ok_or_else(|| stdio_rpc_client_error("turn_not_found"))?;
        if turn.scope.conversation_id != conversation_id {
            return Err(stdio_rpc_client_error("turn_scope_mismatch"));
        }
        if turn
            .state
            .lock()
            .map_err(|_| stdio_rpc_client_error("turn_unavailable"))?
            .terminal
            .is_some()
        {
            return Err(stdio_rpc_client_error("turn_not_active"));
        }
        if params
            .get("agent")
            .or_else(|| params.get("agentId"))
            .and_then(Value::as_str)
            .is_some_and(|agent| agent.trim() != turn.agent_id)
        {
            return Err(stdio_rpc_client_error("turn_scope_mismatch"));
        }
        let mut resolved = params.clone();
        let object = resolved
            .as_object_mut()
            .ok_or_else(|| stdio_rpc_client_error("invalid_params"))?;
        object.insert("agent".to_owned(), Value::String(turn.agent_id.clone()));
        object.insert(
            "sessionId".to_owned(),
            Value::String(turn.session_id.lock().expect("turn session lock").clone()),
        );
        let turn_id = turn.turn_id.lock().expect("turn id lock").clone();
        if !turn_id.is_empty() {
            object.insert("turnId".to_owned(), Value::String(turn_id));
        }
        Ok(resolved)
    }

    /// Bind cancellation to the persistent turn immediately. Native session
    /// discovery may finish after dispatch acceptance, so an early request is
    /// retained and retried from the next committed runtime frame instead of
    /// racing an empty session identity.
    pub(super) fn request_cancel(&self, params: &Value) -> std::result::Result<Value, ClientError> {
        let resolved = self.scoped_control_params(params)?;
        let handle = resolved
            .get("turnHandle")
            .and_then(Value::as_str)
            .ok_or_else(|| stdio_rpc_client_error("invalid_turn_handle"))?;
        let turn = self
            .turn(handle)
            .ok_or_else(|| stdio_rpc_client_error("turn_not_found"))?;
        turn.cancel_requested.store(true, Ordering::Release);
        Ok(Self::attempt_deferred_cancel(&turn).unwrap_or_else(|| {
            json!({
                "ok": true,
                "status": "cancel_requested",
                "turnHandle": turn.scope.dispatch_id,
            })
        }))
    }

    /// Cancel one already-open turn by its handle for the workflow drive. The
    /// handle resolves its own conversation scope, so the drive can never
    /// retarget another turn, and a turn with no live owner reports that fact
    /// instead of an assumed stop.
    pub(crate) fn cancel_opened_turn(
        &self,
        handle: &str,
    ) -> licoup_native::domain::workflow_runtime::TurnCancelDisposition {
        use licoup_native::domain::workflow_runtime::TurnCancelDisposition;
        let Some(turn) = self.turn(handle) else {
            return TurnCancelDisposition::Unavailable;
        };
        let settled = turn
            .state
            .lock()
            .map(|state| state.terminal.is_some())
            .unwrap_or(true);
        if settled {
            return TurnCancelDisposition::NoActiveTurn;
        }
        let params = json!({
            "turnHandle": turn.scope.dispatch_id,
            "conversationId": turn.scope.conversation_id,
            "agent": turn.agent_id,
        });
        match self.request_cancel(&params) {
            Ok(response) => match response.get("ok").and_then(Value::as_bool) {
                Some(true) => TurnCancelDisposition::Accepted,
                _ => match response.get("status").and_then(Value::as_str) {
                    Some("not_active") => TurnCancelDisposition::NoActiveTurn,
                    _ => TurnCancelDisposition::Unavailable,
                },
            },
            Err(_) => TurnCancelDisposition::Unavailable,
        }
    }

    fn attempt_deferred_cancel(turn: &Arc<PersistentTurn>) -> Option<Value> {
        if turn
            .cancel_requested
            .compare_exchange(true, false, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return None;
        }
        let session_id = turn.session_id.lock().ok()?.clone();
        if session_id.is_empty() {
            turn.cancel_requested.store(true, Ordering::Release);
            return None;
        }
        let turn_id = turn.turn_id.lock().ok()?.clone();
        let mut cancel_params = json!({
            "agent": turn.agent_id.as_str(),
            "sessionId": session_id,
        });
        if !turn_id.is_empty() {
            cancel_params["turnId"] = json!(turn_id);
        }
        let response = match licoup_native::cancel_agent_turn(&cancel_params) {
            Ok(response) => response,
            Err(_) => {
                turn.cancel_requested.store(true, Ordering::Release);
                return None;
            }
        };
        if response.get("ok").and_then(Value::as_bool) == Some(true) {
            return Some(response);
        }
        if response.get("status").and_then(Value::as_str) == Some("not_active") {
            turn.cancel_requested.store(true, Ordering::Release);
            return None;
        }
        Some(response)
    }

    /// Open one Membership-scoped dispatch: register the PersistentTurn and
    /// commit its Conversation facts before any native work starts. The
    /// returned handle is the dispatch identity the caller attaches or runs.
    pub(crate) fn open_turn(
        &self,
        params: &Value,
    ) -> std::result::Result<String, RuntimeAdapterError> {
        self.open_turn_with(params, PersistentTurnAdmission::Public)
    }

    pub(crate) fn open_admitted_turn(
        &self,
        params: &Value,
    ) -> std::result::Result<String, RuntimeAdapterError> {
        self.open_turn_with(params, PersistentTurnAdmission::Host)
    }

    fn open_turn_with(
        &self,
        params: &Value,
        admission: PersistentTurnAdmission,
    ) -> std::result::Result<String, RuntimeAdapterError> {
        let turn = self.begin_accepted(params, admission)?;
        Ok(turn.scope.dispatch_id.clone())
    }

    /// Run one previously opened turn to its terminal state. Registration is
    /// never repeated here; an unknown handle fails closed.
    pub(crate) fn run_open_turn(
        &self,
        handle: &str,
        params: &Value,
        portable_data_dir: Option<PathBuf>,
    ) -> std::result::Result<Value, RuntimeAdapterError> {
        let Some(turn) = self.turn(handle) else {
            return Err(RuntimeAdapterError::ConversationDispatchFailed);
        };
        self.run_started_turn(turn, params, portable_data_dir)
    }

    /// Settle one opened turn that will never run. The dispatch completion
    /// authority writes the terminal state with a typed abandonment code so a
    /// registered entry turn can never linger as active.
    pub(crate) fn abandon_turn(&self, handle: &str) {
        let Some(turn) = self.turn(handle) else {
            return;
        };
        let settled = turn
            .state
            .lock()
            .map(|state| state.terminal.is_some())
            .unwrap_or(true);
        if settled {
            return;
        }
        let terminal = PersistentTerminal {
            ok: false,
            payload: json!({
                "ok": false,
                "error": {
                    "code": "conversation_dispatch_failed",
                    "stage": "conversation/dispatch",
                }
            }),
        };
        if Self::finish(&turn, terminal.clone()).is_err() {
            Self::force_terminal(&turn, terminal);
        }
    }

    /// Begin a Membership-scoped PersistentTurn and return immediately so the
    /// caller can attach. Drive continues on a host thread with the same sink
    /// as a blocking open-plus-run turn.
    pub(crate) fn start_background(
        &self,
        params: &Value,
        portable_data_dir: Option<PathBuf>,
    ) -> std::result::Result<Value, RuntimeAdapterError> {
        self.start_background_with(params, portable_data_dir, PersistentTurnAdmission::Public)
    }

    pub(crate) fn start_admitted_background(
        &self,
        params: &Value,
        portable_data_dir: Option<PathBuf>,
    ) -> std::result::Result<Value, RuntimeAdapterError> {
        self.start_background_with(params, portable_data_dir, PersistentTurnAdmission::Host)
    }

    fn start_background_with(
        &self,
        params: &Value,
        portable_data_dir: Option<PathBuf>,
        admission: PersistentTurnAdmission,
    ) -> std::result::Result<Value, RuntimeAdapterError> {
        let handle = match admission {
            PersistentTurnAdmission::Public => self.open_turn(params)?,
            PersistentTurnAdmission::Host => self.open_admitted_turn(params)?,
        };
        let Some(turn) = self.turn(&handle) else {
            self.abandon_turn(&handle);
            return Err(RuntimeAdapterError::ConversationDispatchFailed);
        };
        let receipt = json!({
            "ok": true,
            "accepted": true,
            "turnHandle": turn.scope.dispatch_id,
            "conversationId": turn.scope.conversation_id,
            "membershipId": turn.scope.membership_id,
        });
        let runtime = self.clone();
        let params = params.clone();
        if std::thread::Builder::new()
            .name("conversation-dispatch".to_owned())
            .spawn(move || {
                let _ = runtime.run_started_turn(turn, &params, portable_data_dir);
            })
            .is_err()
        {
            self.abandon_turn(&handle);
            return Err(RuntimeAdapterError::ConversationDispatchFailed);
        }
        Ok(receipt)
    }

    pub(crate) fn steer_sync(
        &self,
        params: &Value,
    ) -> std::result::Result<Value, RuntimeAdapterError> {
        let params = self
            .scoped_control_params(params)
            .map_err(|_| RuntimeAdapterError::ConversationDispatchFailed)?;
        licoup_native::steer_agent_turn(&params)
    }

    fn begin_accepted(
        &self,
        params: &Value,
        admission: PersistentTurnAdmission,
    ) -> std::result::Result<Arc<PersistentTurn>, RuntimeAdapterError> {
        let turn = self
            .begin_with(params, admission)
            .map_err(|_| RuntimeAdapterError::ConversationDispatchFailed)?;
        if Self::record_event(
            &turn,
            json!({
                "event": "agent.turn.accepted",
                "sessionId": "",
                "turnId": "",
                "payload": {
                    "status": "accepted",
                    "lifecyclePrefix": ["submitted", "accepted"]
                }
            }),
        )
        .is_err()
        {
            persist_runtime_failure(
                &turn,
                &stdio_rpc_client_error("conversation_persistence_failed"),
            );
            return Err(RuntimeAdapterError::ConversationDispatchFailed);
        }
        Ok(turn)
    }

    fn run_started_turn(
        &self,
        turn: Arc<PersistentTurn>,
        params: &Value,
        portable_data_dir: Option<PathBuf>,
    ) -> std::result::Result<Value, RuntimeAdapterError> {
        let params = self.params_for_turn(&turn, params);
        let continuation_dir = portable_data_dir.clone();
        let persistence_failed = Arc::new(AtomicBool::new(false));
        let sink_failed = Arc::clone(&persistence_failed);
        let sink_turn = Arc::clone(&turn);
        licoup_native::platform::install_stream_sink(Box::new(move |event| {
            Self::persist_frame(&sink_turn, event, &sink_failed);
        }));
        let stream_guard = licoup_native::platform::StreamSinkGuard;
        let raw_observer = Self::raw_execution_observer(&turn);
        let raw_scope = licoup_native::platform::raw_execution::RawExecutionScope::enter(Some(
            raw_observer.clone(),
        ));
        let execution = catch_unwind(AssertUnwindSafe(|| {
            let _guard = PortableDataDirOverrideGuard::set(portable_data_dir);
            licoup_native::send_agent_turn(&params)
        }));
        drop(stream_guard);
        drop(raw_scope);
        Self::close_raw_execution_observer(&turn, &raw_observer);

        let result = match execution {
            Ok(Ok(value)) => {
                if Self::finish(
                    &turn,
                    PersistentTerminal {
                        ok: true,
                        payload: value.clone(),
                    },
                )
                .is_err()
                {
                    persist_runtime_failure(
                        &turn,
                        &stdio_rpc_client_error("conversation_persistence_failed"),
                    );
                    return Err(RuntimeAdapterError::ConversationDispatchFailed);
                }
                Ok(value)
            }
            Ok(Err(error)) => {
                persist_runtime_failure(
                    &turn,
                    &licoup_native::platform::runtime_adapters::client_error::client_error(&error),
                );
                Err(error)
            }
            Err(_) => {
                persist_runtime_failure(
                    &turn,
                    &stdio_rpc_client_error(if persistence_failed.load(Ordering::Acquire) {
                        "conversation_persistence_failed"
                    } else {
                        "command_panicked"
                    }),
                );
                Err(RuntimeAdapterError::ConversationDispatchFailed)
            }
        };
        self.start_next_boundary_turn(
            &turn.scope.conversation_id,
            &turn.scope.membership_id,
            continuation_dir,
        );
        result
    }

    fn start_next_boundary_turn(
        &self,
        conversation_id: &str,
        membership_id: &str,
        portable_data_dir: Option<PathBuf>,
    ) {
        let Ok(Some(context)) = self
            .inner
            .store
            .claim_next_pending_direct_turn(conversation_id, membership_id)
        else {
            return;
        };
        let Ok(params) = direct_turn_params(&context) else {
            let diagnostic = r#"{"code":"runtime_instruction_policy_unavailable","stage":"conversation/dispatch"}"#;
            let _ = self
                .inner
                .store
                .fail_direct_turn_unless_dispatched(&context.turn.id, diagnostic);
            return;
        };
        if self
            .start_admitted_background(&params, portable_data_dir)
            .is_err()
        {
            let diagnostic =
                r#"{"code":"conversation_dispatch_failed","stage":"conversation/dispatch"}"#;
            let _ = self
                .inner
                .store
                .fail_direct_turn_unless_dispatched(&context.turn.id, diagnostic);
        }
    }

    pub(super) fn active(&self, params: &Value) -> Value {
        const MAX_CHANGE_WAIT: Duration = Duration::from_secs(2);

        let agent = params
            .get("agent")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let session = params
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let conversation_id = params
            .get("conversationId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let wait = Duration::from_millis(
            params
                .get("waitForChangeMs")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .min(MAX_CHANGE_WAIT.as_millis() as u64),
        );
        let deadline = Instant::now() + wait;
        let mut turns = self.inner.turns.lock().expect("turn registry lock");
        loop {
            let active = turns
                .values()
                .filter_map(|turn| {
                    let state = turn.state.lock().ok()?;
                    if state.terminal.is_some()
                        || (!agent.is_empty() && turn.agent_id != agent)
                        || (!conversation_id.is_empty()
                            && turn.scope.conversation_id != conversation_id)
                    {
                        return None;
                    }
                    let turn_session = turn.session_id.lock().ok()?.clone();
                    if !session.is_empty() && turn_session != session {
                        return None;
                    }
                    Some(json!({
                        "turnHandle": turn.scope.dispatch_id,
                        "conversationId": turn.scope.conversation_id,
                        "membershipId": turn.scope.membership_id,
                        "agent": turn.agent_id,
                        "sessionId": turn_session,
                        "turnId": turn.turn_id.lock().ok()?.clone(),
                        "highWater": state.high_water,
                    }))
                })
                .collect::<Vec<_>>();
            if !active.is_empty() || wait.is_zero() || Instant::now() >= deadline {
                return json!({"turns": active});
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            let (next, timed_out) = self
                .inner
                .turns_changed
                .wait_timeout(turns, remaining)
                .expect("turn registry lock");
            turns = next;
            if timed_out.timed_out() {
                return json!({"turns": []});
            }
        }
    }

    /// Persist one emitted turn frame through the host-facing stream sink. A
    /// SQLite write failure is recorded as a typed persistence-failure signal
    /// and the frame is dropped; the surrounding turn then settles with a
    /// `conversation_persistence_failed` error delta while the stdio frame
    /// loop keeps serving. Only interior invariants may assert; frame
    /// persistence is a store failure and must never unwind the boundary.
    fn persist_frame(turn: &Arc<PersistentTurn>, event: Value, persistence_failed: &AtomicBool) {
        if Self::record_event(turn, event).is_err() {
            persistence_failed.store(true, Ordering::Release);
        }
    }

    fn raw_execution_observer(
        turn: &Arc<PersistentTurn>,
    ) -> licoup_native::platform::raw_execution::RawExecutionObserver {
        let turn = Arc::clone(turn);
        licoup_native::platform::raw_execution::RawExecutionObserver::new(
            move |source, direction, text| {
                let mut state = turn.state.lock().expect("turn state lock");
                if state.terminal.is_some() {
                    return Ok(());
                }
                turn.store.append_runtime_local_raw_frame(
                    &turn.scope,
                    &format!("protocol.{source}.{}", direction.as_str()),
                    text,
                )?;
                state.execution_generation += 1;
                turn.changed.notify_all();
                Ok(())
            },
        )
    }

    fn close_raw_execution_observer(
        turn: &Arc<PersistentTurn>,
        observer: &licoup_native::platform::raw_execution::RawExecutionObserver,
    ) {
        observer.close();
        turn.state
            .lock()
            .expect("turn state lock")
            .raw_capture_failed |= observer.had_failure();
    }

    fn record_event(
        turn: &Arc<PersistentTurn>,
        mut event: Value,
    ) -> licoup_native::domain::client_conversation::StoreResult<Value> {
        Self::record_native_provenance(turn, &event, false)?;
        if let Some(session_id) = event.get("sessionId").and_then(Value::as_str) {
            if !session_id.trim().is_empty() {
                *turn.session_id.lock().expect("turn session lock") = session_id.trim().to_owned();
            }
        }
        if let Some(turn_id) = event.get("turnId").and_then(Value::as_str) {
            if !turn_id.trim().is_empty() {
                let bound = turn_id.trim().to_owned();
                *turn.turn_id.lock().expect("turn id lock") = bound.clone();
                if let Some(inner) = turn.runtime.upgrade() {
                    if let Some(observer) = inner
                        .live_turn_observer
                        .lock()
                        .ok()
                        .and_then(|guard| guard.clone())
                    {
                        observer(
                            &turn.scope.conversation_id,
                            &turn.scope.membership_id,
                            &turn.scope.dispatch_id,
                            &bound,
                        );
                    }
                }
            }
        }
        if !ConversationStore::runtime_frame_commits_cursor(&event) {
            let mut state = turn.state.lock().expect("turn state lock");
            turn.store.append_runtime_local_frame(&turn.scope, &event)?;
            state.execution_generation += 1;
            let assembled = state.take_live_assembly(&event);
            turn.changed.notify_all();
            // User-speech is already a Canonical Message Event. Live observers
            // may still see the delta, but it must not occupy a replay cursor.
            let live_event = if turn.admitted_assistant_turn {
                redact_live_runtime_event_with_assembly(&event, &assembled)
            } else {
                event
            };
            drop(state);
            let _ = Self::attempt_deferred_cancel(turn);
            return Ok(live_event);
        }
        // Serialize cursor allocation through canonical persistence and the
        // disposable cache update. Computing the cursor under a short lock and
        // releasing it before the store write lets concurrent adapter emitters
        // persist the same cursor before either detects the race.
        let mut state = turn.state.lock().expect("turn state lock");
        let cursor = state.high_water + 1;
        if let Some(object) = event.as_object_mut() {
            object.insert(
                "turnHandle".to_owned(),
                Value::String(turn.scope.dispatch_id.clone()),
            );
            object.insert(
                "conversationId".to_owned(),
                Value::String(turn.scope.conversation_id.clone()),
            );
            object.insert("cursor".to_owned(), Value::from(cursor));
            object.insert(
                "membershipId".to_owned(),
                Value::String(turn.scope.membership_id.clone()),
            );
        }
        turn.store
            .append_runtime_frame(&turn.scope, cursor, &event)?;
        let session_id = turn.session_id.lock().expect("turn session lock").clone();
        turn.store
            .bind_runtime_session(&turn.scope, &turn.agent_id, &session_id, None, None)?;
        let assembled = state.take_live_assembly(&event);
        let live_event = if turn.admitted_assistant_turn {
            redact_live_runtime_event_with_assembly(&event, &assembled)
        } else {
            event.clone()
        };
        let encoded_bytes = serde_json::to_vec(&live_event)?.len();
        state.high_water = cursor;
        state.execution_generation += 1;
        state.cache_bytes = state.cache_bytes.saturating_add(encoded_bytes);
        state.cache.push_back(CachedFrame {
            cursor,
            encoded_bytes,
            event: live_event.clone(),
        });
        while state.cache_bytes > turn.cache_budget {
            let Some(evicted) = state.cache.pop_front() else {
                break;
            };
            state.cache_bytes = state.cache_bytes.saturating_sub(evicted.encoded_bytes);
        }
        turn.changed.notify_all();
        drop(state);
        let _ = Self::attempt_deferred_cancel(turn);
        Ok(live_event)
    }

    fn record_native_provenance(
        turn: &Arc<PersistentTurn>,
        value: &Value,
        terminal: bool,
    ) -> Result<()> {
        let explicit_turn = value
            .get("nativeTurnId")
            .or_else(|| value.pointer("/payload/nativeTurnId"));
        // Codex's closed parser sets this result only from turn/start's
        // result.turn.id. Other adapters may use host-generated turn ids.
        let native_turn = explicit_turn
            .or_else(|| {
                (terminal
                    && value.get("driverId").and_then(Value::as_str) == Some("codex-app-server"))
                .then(|| value.get("turnId"))
                .flatten()
            })
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty());
        let reply = matches!(
            value.get("event").and_then(Value::as_str),
            Some("agent.message.chunk" | "agent.message.completed")
        );
        let message_id = reply
            .then(|| {
                value
                    .get("sourceMessageId")
                    .or_else(|| value.pointer("/payload/sourceMessageId"))
            })
            .flatten()
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty());
        if native_turn.is_none() && message_id.is_none() {
            return Ok(());
        }
        let Some(session_id) = value
            .get("nativeSessionId")
            .or_else(|| value.get("sessionId"))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            return Ok(());
        };
        let mut state = turn.state.lock().expect("turn state lock");
        let keys = [("turn", native_turn), ("message", message_id)]
            .into_iter()
            .filter_map(|(kind, id)| {
                id.map(|id| (session_id.to_owned(), kind.to_owned(), id.to_owned()))
            })
            .collect::<Vec<_>>();
        if keys
            .iter()
            .all(|key| state.native_provenance_keys.contains(key))
        {
            return Ok(());
        }
        turn.store.record_native_execution_provenance(
            &turn.scope,
            &turn.agent_id,
            session_id,
            native_turn,
            message_id,
        )?;
        state.native_provenance_keys.extend(keys);
        Ok(())
    }

    fn finish(turn: &Arc<PersistentTurn>, mut terminal: PersistentTerminal) -> Result<()> {
        Self::record_native_provenance(turn, &terminal.payload, true)?;
        // Terminal settlement is one write: serialize persistence and the
        // in-memory projection so a later observer/transport closure cannot
        // race and replace the first exact native outcome.
        let mut persistent_state = turn.state.lock().expect("turn state lock");
        if persistent_state.terminal.is_some() {
            return Ok(());
        }
        if persistent_state.raw_capture_failed {
            let failure = json!({"complete":false,"errorCode":"conversation_persistence_failed"});
            if let Some(payload) = terminal.payload.as_object_mut() {
                payload.insert("licoUpExecutionCapture".to_owned(), failure);
            } else {
                turn.store.append_runtime_local_raw_frame(
                    &turn.scope,
                    "capture.error",
                    &failure.to_string(),
                )?;
            }
        }
        let response_ok = terminal
            .payload
            .get("ok")
            .and_then(Value::as_bool)
            .unwrap_or(terminal.ok);
        let turn_status = terminal
            .payload
            .get("turnStatus")
            .or_else(|| {
                terminal
                    .payload
                    .get("error")
                    .and_then(|error| error.get("turnStatus"))
            })
            .and_then(Value::as_str);
        let state = if terminal.ok && response_ok {
            DispatchState::Completed
        } else if turn_status == Some("cancelled") {
            DispatchState::Cancelled
        } else {
            DispatchState::Failed
        };
        let error_code = (state != DispatchState::Completed)
            .then(|| {
                terminal
                    .payload
                    .get("code")
                    .or_else(|| {
                        terminal
                            .payload
                            .get("error")
                            .and_then(|error| error.get("code"))
                    })
                    .and_then(Value::as_str)
            })
            .flatten();
        if let Some(session_id) = terminal
            .payload
            .get("nativeSessionId")
            .or_else(|| terminal.payload.get("sessionId"))
            .and_then(Value::as_str)
        {
            turn.store.bind_runtime_session(
                &turn.scope,
                &turn.agent_id,
                session_id,
                terminal
                    .payload
                    .get("sourcePath")
                    .or_else(|| terminal.payload.get("conversationPath"))
                    .and_then(Value::as_str),
                terminal
                    .payload
                    .get("workingDirectory")
                    .and_then(Value::as_str),
            )?;
        }
        let persisted_state = turn.store.finish_runtime_dispatch(
            &turn.scope,
            &terminal.payload,
            state,
            error_code,
        )?;
        let mut callback_payload = terminal.payload.clone();
        if persisted_state != state {
            if turn.admitted_assistant_turn
                && state == DispatchState::Completed
                && persisted_state == DispatchState::Failed
            {
                // The Conversation store is the only authority that can
                // reject a trusted response envelope. Ordinary prose is
                // already accepted there and never reaches this branch.
                apply_admitted_validation_failure_facts(&mut callback_payload);
            } else {
                callback_payload["ok"] = json!(false);
                callback_payload["turnStatus"] = json!(persisted_state.as_str());
            }
        }
        if callback_payload
            .get("conversationId")
            .and_then(Value::as_str)
            .is_none()
        {
            callback_payload["conversationId"] = json!(turn.scope.conversation_id);
        }
        if callback_payload
            .get("membershipId")
            .and_then(Value::as_str)
            .is_none()
        {
            callback_payload["membershipId"] = json!(turn.scope.membership_id);
        }
        let settlement_causation = settlement_source_event_id(turn);
        let existing_causation = callback_payload.get("causationId").and_then(Value::as_str);
        if existing_causation.is_none() || existing_causation == Some(turn.scope.event_id.as_str())
        {
            callback_payload["causationId"] = json!(settlement_causation);
        }
        if callback_payload
            .get("dispatchId")
            .and_then(Value::as_str)
            .is_none()
        {
            callback_payload["dispatchId"] = json!(turn.scope.dispatch_id);
        }
        if callback_payload
            .get("continuityKind")
            .and_then(Value::as_str)
            .is_none()
        {
            if let Some(kind) = turn.continuity_kind.as_deref() {
                callback_payload["continuityKind"] = json!(kind);
            }
        }
        persistent_state.terminal = Some(public_persistent_terminal(
            &terminal,
            state,
            persisted_state,
            turn.admitted_assistant_turn,
        ));
        turn.changed.notify_all();
        drop(persistent_state);
        // The delegated PersistentTurn settled: deliver the single completion
        // signal to the caller membership. Turns without a durable subagent
        // claim — including every callback turn — never trigger a callback.
        if let Some(inner) = turn.runtime.upgrade() {
            inner.turns_changed.notify_all();
            PersistentConversationRuntime {
                inner: inner.clone(),
            }
            .notify_subagent_terminal(
                &turn.scope.dispatch_id,
                persisted_state,
                &callback_payload,
            );
            if let Some(hook) = inner
                .settlement_hook
                .lock()
                .ok()
                .and_then(|guard| guard.clone())
            {
                let _ = hook(&turn.scope.conversation_id, &callback_payload);
            }
        }
        Ok(())
    }

    fn rearm_persisted_watchdogs(&self) {
        let Ok(pending) = self.inner.store.pending_subagent_watchdogs() else {
            return;
        };
        let now = unix_now_ms();
        for (dispatch_id, deadline_unix_ms) in pending {
            let remaining_ms = u64::try_from(deadline_unix_ms.saturating_sub(now)).unwrap_or(0);
            self.register_subagent_watchdog(dispatch_id, Duration::from_millis(remaining_ms));
        }
    }

    /// Register one watchdog deadline for a claimed subagent dispatch and
    /// start the single watcher thread on first use. The watcher is lazily
    /// spawned so a runtime that never hosts a claimed dispatch never carries
    /// the thread; it parks on the condvar between deadlines.
    fn register_subagent_watchdog(&self, dispatch_id: String, timeout: Duration) {
        {
            let mut registry = self
                .inner
                .subagent_watchdog
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            registry.insert(dispatch_id, Instant::now() + timeout);
        }
        self.inner.subagent_watchdog_changed.notify_all();
        if !self
            .inner
            .subagent_watchdog_spawned
            .swap(true, Ordering::AcqRel)
        {
            let inner = Arc::clone(&self.inner);
            if std::thread::Builder::new()
                .name("subagent-watchdog".to_owned())
                .spawn(move || Self::subagent_watchdog_main(inner))
                .is_err()
            {
                self.inner
                    .subagent_watchdog_spawned
                    .store(false, Ordering::Release);
            }
        }
    }

    fn subagent_watchdog_main(inner: Arc<PersistentConversationRuntimeInner>) {
        const LIVENESS_POLL: Duration = Duration::from_secs(30);
        let mut registry = inner
            .subagent_watchdog
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        loop {
            let now = Instant::now();
            let earliest = registry.values().min().copied();
            if earliest.is_some_and(|deadline| deadline <= now) {
                let expired: Vec<String> = registry
                    .iter()
                    .filter(|(_, deadline)| **deadline <= now)
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in &expired {
                    registry.remove(id);
                }
                drop(registry);
                for id in expired {
                    Self::fire_subagent_timeout(&inner, &id);
                }
                registry = inner
                    .subagent_watchdog
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                continue;
            }
            let wait = earliest
                .map(|deadline| deadline.saturating_duration_since(now))
                .unwrap_or(LIVENESS_POLL)
                .min(LIVENESS_POLL);
            let (guard, _) = inner
                .subagent_watchdog_changed
                .wait_timeout(registry, wait)
                .unwrap_or_else(|poison| poison.into_inner());
            registry = guard;
        }
    }

    /// Timeout fallback: the deadline elapsed and the claim is still
    /// non-terminal, so the caller receives the same callback carrying the
    /// current claim state. A claim that already settled is left to the
    /// terminal-settlement callback.
    fn fire_subagent_timeout(inner: &Arc<PersistentConversationRuntimeInner>, dispatch_id: &str) {
        let claim = match inner.store.subagent_claim(dispatch_id) {
            Ok(Some(claim)) => claim,
            _ => return,
        };
        if matches!(
            claim.state,
            SubagentDispatchClaimState::Completed
                | SubagentDispatchClaimState::Failed
                | SubagentDispatchClaimState::Cancelled
        ) {
            return;
        }
        let runtime = PersistentConversationRuntime {
            inner: Arc::clone(inner),
        };
        runtime.dispatch_subagent_callback(&claim, claim.state.as_str(), None);
    }

    /// Terminal finish path: the delegated turn settled, so notify the caller
    /// membership once and retire any pending watchdog deadline for the claim.
    fn notify_subagent_terminal(
        &self,
        dispatch_id: &str,
        state: DispatchState,
        terminal_payload: &Value,
    ) {
        let claim = match self.inner.store.subagent_claim(dispatch_id) {
            Ok(Some(claim)) => claim,
            _ => return,
        };
        self.inner
            .subagent_watchdog
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&claim.id);
        let _ = self.inner.store.clear_subagent_watchdog_deadline(&claim.id);
        self.dispatch_subagent_callback(&claim, state.as_str(), Some(terminal_payload));
    }

    /// Dispatch the one callback turn to the caller membership through the
    /// same Membership-scoped PersistentTurn door as every other dispatch.
    /// The fired-once guard is the single-signal contract; the claim lookup
    /// upstream is the no-recursion guard because callback dispatches never
    /// carry a claim identity.
    fn dispatch_subagent_callback(
        &self,
        claim: &SubagentDispatchClaim,
        state: &str,
        terminal_payload: Option<&Value>,
    ) {
        {
            let mut fired = self
                .inner
                .subagent_callback_fired
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if !fired.insert(claim.id.clone()) {
                return;
            }
        }
        let Some(params) = licoup_native::domain::subagents::subagent_callback_plan(
            &self.inner.store,
            claim,
            state,
            terminal_payload,
        ) else {
            return;
        };
        let runtime = self.clone();
        let _ = std::thread::Builder::new()
            .name("subagent-callback".to_owned())
            .spawn(move || {
                let _ = runtime.start_admitted_background(&params, None);
            });
    }

    fn force_terminal(turn: &Arc<PersistentTurn>, terminal: PersistentTerminal) {
        let mut state = turn.state.lock().expect("turn state lock");
        if state.terminal.is_none() {
            state.terminal = Some(terminal);
            turn.changed.notify_all();
            if let Some(inner) = turn.runtime.upgrade() {
                inner.turns_changed.notify_all();
            }
        }
    }
}

fn direct_turn_params(
    context: &licoup_native::domain::client_conversation::DirectTurnExecutionContext,
) -> std::result::Result<Value, &'static str> {
    let delivery =
        licoup_native::platform::runtime_adapters::compose_generated_instruction_delivery(
            &context.agent_id,
            &context.source_content,
            context.private_instructions(),
        )?;
    let mut params = json!({
        "agentId": context.agent_id,
        "agent": context.agent_id,
        "text": delivery.text,
        "streamEvents": true,
        // 0 means "use the writable policy default", not unbounded.
        "timeoutMs": 0,
        "conversationId": context.turn.conversation_id,
        "membershipId": context.turn.membership_id,
        "causationId": context.turn.source_event_id,
        "dispatchId": context.turn.id,
    });
    if let (Some(field), Some(guidance)) = (delivery.field, delivery.guidance) {
        params[field] = json!(guidance);
    }
    if context.is_assistant {
        params["continuityKind"] = json!(CONTINUITY_KIND_USER_POSTED);
    }
    if !context.source_attachments.is_empty() {
        params["attachments"] =
            licoup_native::domain::client_conversation::dispatch_attachments_param(
                &context.source_attachments,
            );
    }
    for (key, value) in [
        ("sessionId", context.runtime_session_id.as_deref()),
        ("sourcePath", context.runtime_conversation_path.as_deref()),
        ("workingDirectory", context.working_directory.as_deref()),
        ("model", context.preferred_model.as_deref()),
        (
            "reasoningEffort",
            context.preferred_reasoning_effort.as_deref(),
        ),
    ] {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            params[key] = json!(value);
        }
    }
    Ok(params)
}

pub(super) fn spawn_send<W>(
    writer: Arc<StdioRpcWriter<W>>,
    request_id: String,
    workflow_id: String,
    params: Value,
    portable_data_dir: Option<PathBuf>,
    runtime: PersistentConversationRuntime,
) -> std::result::Result<std::thread::JoinHandle<()>, ClientError>
where
    W: Write + Send + 'static,
{
    let turn = runtime.begin(&params)?;
    let handle = turn.scope.dispatch_id.clone();
    std::thread::Builder::new()
        .name("conversation-send".to_owned())
        .spawn(move || {
            let _ = execute(
                &writer,
                StdioRpcFrameLane::Stream,
                &request_id,
                &workflow_id,
                "send",
                params,
                portable_data_dir,
                true,
                Some(turn),
            );
        })
        .map_err(|_| {
            runtime.abandon_turn(&handle);
            stdio_rpc_client_error("agent_conversation_dispatch_failed")
        })
}

pub(super) fn spawn_attach<W>(
    writer: Arc<StdioRpcWriter<W>>,
    request_id: String,
    workflow_id: String,
    params: Value,
    runtime: PersistentConversationRuntime,
) -> std::result::Result<std::thread::JoinHandle<()>, ClientError>
where
    W: Write + Send + 'static,
{
    let handle = params
        .get("turnHandle")
        .and_then(Value::as_str)
        .filter(|value| valid_turn_handle(value))
        .ok_or_else(|| stdio_rpc_client_error("invalid_turn_handle"))?;
    let conversation_id = params
        .get("conversationId")
        .and_then(Value::as_str)
        .filter(|value| valid_turn_handle(value))
        .ok_or_else(|| stdio_rpc_client_error("invalid_conversation_scope"))?;
    let after_cursor = params
        .get("afterCursor")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let turn = runtime
        .turn(handle)
        .ok_or_else(|| stdio_rpc_client_error("turn_not_found"))?;
    if turn.scope.conversation_id != conversation_id {
        return Err(stdio_rpc_client_error("turn_scope_mismatch"));
    }
    let high_water = turn
        .state
        .lock()
        .map_err(|_| stdio_rpc_client_error("turn_unavailable"))?
        .high_water;
    if after_cursor > high_water {
        return Err(stdio_rpc_client_error("cursor_ahead"));
    }
    std::thread::Builder::new()
        .name("conversation-attach".to_owned())
        .spawn(move || {
            let _ = replay_turn(&writer, &request_id, &workflow_id, &turn, after_cursor);
        })
        .map_err(|_| stdio_rpc_client_error("agent_conversation_dispatch_failed"))
}

fn replay_turn<W: Write>(
    writer: &Arc<StdioRpcWriter<W>>,
    request_id: &str,
    workflow_id: &str,
    turn: &Arc<PersistentTurn>,
    mut cursor: u64,
) -> Result<()> {
    let mut request_sequence = 0_u64;
    loop {
        let (captured_high_water, cached, terminal) = {
            let state = turn.state.lock().expect("turn state lock");
            let cache_floor = state
                .cache
                .front()
                .map(|frame| frame.cursor)
                .unwrap_or(state.high_water.saturating_add(1));
            let cached = (cursor.saturating_add(1) >= cache_floor).then(|| {
                state
                    .cache
                    .iter()
                    .filter(|frame| frame.cursor > cursor)
                    .map(|frame| frame.event.clone())
                    .collect::<Vec<_>>()
            });
            (state.high_water, cached, state.terminal.clone())
        };
        while cursor < captured_high_water {
            let frames = if let Some(cached) = cached.as_ref() {
                cached
                    .iter()
                    .filter(|event| event.get("cursor").and_then(Value::as_u64) > Some(cursor))
                    .cloned()
                    .collect::<Vec<_>>()
            } else {
                turn.store
                    .runtime_frames_after(
                        &turn.scope,
                        cursor,
                        captured_high_water,
                        REPLAY_PAGE_SIZE,
                    )?
                    .into_iter()
                    .scan(String::new(), |assembled, event| {
                        Some(if turn.admitted_assistant_turn {
                            redact_store_live_event(event, assembled)
                        } else {
                            event
                        })
                    })
                    .collect()
            };
            if frames.is_empty() {
                return Err(anyhow!("canonical_replay_gap"));
            }
            for event in frames {
                let next_cursor = event
                    .get("cursor")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| anyhow!("canonical_replay_cursor_missing"))?;
                if next_cursor != cursor + 1 || next_cursor > captured_high_water {
                    return Err(anyhow!("canonical_replay_cursor_invalid"));
                }
                request_sequence += 1;
                write_stdio_rpc_event(
                    writer.stream(),
                    request_id,
                    workflow_id,
                    request_sequence,
                    event,
                )?;
                cursor = next_cursor;
            }
        }
        if let Some(terminal) = terminal {
            request_sequence += 1;
            return write_persistent_terminal(
                writer,
                StdioRpcFrameLane::Stream,
                request_id,
                workflow_id,
                request_sequence,
                &terminal,
            )
            .map_err(Into::into);
        }
        let mut state = turn.state.lock().expect("turn state lock");
        while state.high_water == cursor && state.terminal.is_none() {
            state = turn.changed.wait(state).expect("turn state lock");
        }
    }
}

fn valid_turn_handle(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

pub(super) fn execute<W>(
    writer: &Arc<StdioRpcWriter<W>>,
    lane: StdioRpcFrameLane,
    request_id: &str,
    workflow_id: &str,
    operation: &str,
    params: Value,
    portable_data_dir: Option<PathBuf>,
    stream_events: bool,
    persistent_turn: Option<Arc<PersistentTurn>>,
) -> Result<()>
where
    W: Write + Send + 'static,
{
    // Structured native UI replies use the already trusted, live-turn-scoped
    // steer RPC. They never become prompt text: the callback token and exact
    // structured response resolve one parked transport generation directly.
    if operation == "steer"
        && let Some(token) = params
            .get("adapterCallbackTokenRef")
            .and_then(Value::as_str)
        && let Some(response) = params.get("interactionResponse").cloned()
    {
        let session_id = params
            .get("sessionId")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("native_interaction_session_id_missing"))?;
        let turn_id = params
            .get("turnId")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("native_interaction_turn_id_missing"))?;
        let resolved = licoup_native::platform::resolve_scoped_native_agent_interaction(
            token,
            Some(session_id),
            Some(turn_id),
            response,
        )
        .map_err(anyhow::Error::msg)?;
        write_stdio_rpc_terminal_success(
            writer.lane_target(lane),
            request_id,
            workflow_id,
            1,
            resolved,
        )?;
        return Ok(());
    }
    let (initial_sequence, observer_is_connected) = if let Some(turn) = persistent_turn.as_ref() {
        let accepted = match PersistentConversationRuntime::record_event(
            turn,
            json!({
                "event": "agent.turn.accepted",
                "sessionId": "",
                "turnId": "",
                "payload": {
                    "status": "accepted",
                    "lifecyclePrefix": ["submitted", "accepted"]
                }
            }),
        ) {
            Ok(accepted) => accepted,
            Err(_) => {
                let error = stdio_rpc_client_error("conversation_persistence_failed");
                let terminal = PersistentTerminal {
                    ok: false,
                    payload: serde_json::to_value(&error)
                        .unwrap_or_else(|_| json!({"code": "conversation_persistence_failed"})),
                };
                if PersistentConversationRuntime::finish(turn, terminal.clone()).is_err() {
                    PersistentConversationRuntime::force_terminal(turn, terminal);
                }
                write_stdio_rpc_terminal_error(
                    writer.lane_target(lane),
                    request_id,
                    workflow_id,
                    1,
                    &error,
                )?;
                return Ok(());
            }
        };
        (
            1,
            write_stdio_rpc_event(
                writer.lane_target(lane),
                request_id,
                workflow_id,
                1,
                accepted,
            )
            .is_ok(),
        )
    } else {
        (0, true)
    };
    let sequence = Arc::new(AtomicU64::new(initial_sequence));
    let observer_connected = Arc::new(AtomicBool::new(observer_is_connected));
    let persistence_failed = Arc::new(AtomicBool::new(false));
    let stream_guard = stream_events.then(|| {
        let writer = Arc::clone(writer);
        let request_id = request_id.to_owned();
        let workflow_id = workflow_id.to_owned();
        let sequence = Arc::clone(&sequence);
        let observer_connected = Arc::clone(&observer_connected);
        let persistence_failed = Arc::clone(&persistence_failed);
        let persistent_turn = persistent_turn.clone();
        licoup_native::platform::install_stream_sink(Box::new(move |event| {
            let event = if let Some(turn) = persistent_turn.as_ref() {
                match PersistentConversationRuntime::record_event(turn, event) {
                    Ok(event) => event,
                    Err(_) => {
                        // A SQLite write failure is a typed persistence-failure
                        // signal, never a panic: drop the frame and let the
                        // turn settle with a `conversation_persistence_failed`
                        // delta.
                        persistence_failed.store(true, Ordering::Release);
                        return;
                    }
                }
            } else {
                event
            };
            let next = sequence.load(Ordering::Acquire) + 1;
            if observer_connected.load(Ordering::Acquire)
                && write_stdio_rpc_event(writer.stream(), &request_id, &workflow_id, next, event)
                    .is_ok()
            {
                sequence.store(next, Ordering::Release);
            } else {
                observer_connected.store(false, Ordering::Release);
            }
        }));
        licoup_native::platform::StreamSinkGuard
    });
    let execution = catch_unwind(AssertUnwindSafe(|| {
        let _guard = PortableDataDirOverrideGuard::set(portable_data_dir);
        licoup_native::dispatch_agent_operation(operation, &params)
            .map(licoup_native::ffi::commands::CliExecution::Json)
    }));
    drop(stream_guard);
    let terminal_sequence = sequence.fetch_add(1, Ordering::AcqRel) + 1;
    match execution {
        Ok(Ok(licoup_native::ffi::commands::CliExecution::Json(value))) => {
            if let Some(turn) = persistent_turn.as_ref() {
                if PersistentConversationRuntime::finish(
                    turn,
                    PersistentTerminal {
                        ok: true,
                        payload: value.clone(),
                    },
                )
                .is_err()
                {
                    return finish_error(
                        writer,
                        lane,
                        request_id,
                        workflow_id,
                        terminal_sequence,
                        persistent_turn.as_ref(),
                        observer_connected.load(Ordering::Acquire),
                        stdio_rpc_client_error("conversation_persistence_failed"),
                    )
                    .map_err(Into::into);
                }
            }
            if observer_connected.load(Ordering::Acquire) {
                if let Some(public) = persistent_turn
                    .as_ref()
                    .and_then(|turn| stored_public_terminal(turn))
                {
                    write_persistent_terminal(
                        writer,
                        lane,
                        request_id,
                        workflow_id,
                        terminal_sequence,
                        &public,
                    )
                } else {
                    write_stdio_rpc_terminal_success(
                        writer.lane_target(lane),
                        request_id,
                        workflow_id,
                        terminal_sequence,
                        value,
                    )
                }
            } else {
                Ok(())
            }
        }
        Ok(Err(error)) => finish_error(
            writer,
            lane,
            request_id,
            workflow_id,
            terminal_sequence,
            persistent_turn.as_ref(),
            observer_connected.load(Ordering::Acquire),
            licoup_native::platform::runtime_adapters::client_error::client_error(&error),
        ),
        Err(_) => finish_error(
            writer,
            lane,
            request_id,
            workflow_id,
            terminal_sequence,
            persistent_turn.as_ref(),
            observer_connected.load(Ordering::Acquire),
            stdio_rpc_client_error(if persistence_failed.load(Ordering::Acquire) {
                "conversation_persistence_failed"
            } else {
                "command_panicked"
            }),
        ),
        Ok(Ok(_)) => finish_error(
            writer,
            lane,
            request_id,
            workflow_id,
            terminal_sequence,
            persistent_turn.as_ref(),
            observer_connected.load(Ordering::Acquire),
            stdio_rpc_client_error("command_failed"),
        ),
    }?;
    Ok(())
}

fn finish_error<W: Write>(
    writer: &Arc<StdioRpcWriter<W>>,
    lane: StdioRpcFrameLane,
    request_id: &str,
    workflow_id: &str,
    sequence: u64,
    turn: Option<&Arc<PersistentTurn>>,
    observer_connected: bool,
    error: ClientError,
) -> io::Result<()> {
    if let Some(turn) = turn {
        let terminal = PersistentTerminal {
            ok: false,
            payload: serde_json::to_value(&error)
                .unwrap_or_else(|_| json!({"code": "command_failed"})),
        };
        if PersistentConversationRuntime::finish(turn, terminal.clone()).is_err() {
            PersistentConversationRuntime::force_terminal(turn, terminal);
        }
    }
    if observer_connected {
        write_stdio_rpc_terminal_error(
            writer.lane_target(lane),
            request_id,
            workflow_id,
            sequence,
            &error,
        )
    } else {
        Ok(())
    }
}

fn stored_public_terminal(turn: &Arc<PersistentTurn>) -> Option<PersistentTerminal> {
    turn.state.lock().ok()?.terminal.clone()
}

fn settlement_source_event_id(turn: &PersistentTurn) -> String {
    turn.store
        .event(&turn.scope.conversation_id, &turn.scope.event_id)
        .ok()
        .flatten()
        .and_then(|event| event.causation_id)
        .filter(|id| !id.trim().is_empty())
        .unwrap_or_else(|| turn.scope.event_id.clone())
}

fn public_persistent_terminal(
    original: &PersistentTerminal,
    recorded_state: DispatchState,
    persisted_state: DispatchState,
    admitted_assistant_turn: bool,
) -> PersistentTerminal {
    if !admitted_assistant_turn {
        return original.clone();
    }
    let mut payload = original.payload.clone();
    match persisted_state {
        DispatchState::Failed => {
            if recorded_state == DispatchState::Completed {
                apply_admitted_validation_failure_facts(&mut payload);
            } else {
                payload["ok"] = json!(false);
            }
            project_admitted_known_text_fields(&mut payload, "");
            PersistentTerminal { ok: false, payload }
        }
        DispatchState::Completed => {
            let reply = payload
                .get("output")
                .and_then(Value::as_str)
                .and_then(public_admitted_output)
                .unwrap_or_default();
            project_admitted_known_text_fields(&mut payload, &reply);
            payload["ok"] = json!(true);
            PersistentTerminal { ok: true, payload }
        }
        DispatchState::Cancelled => {
            project_admitted_known_text_fields(&mut payload, "");
            PersistentTerminal {
                ok: original.ok,
                payload,
            }
        }
        _ => original.clone(),
    }
}

fn persist_runtime_failure(turn: &Arc<PersistentTurn>, error: &ClientError) {
    let terminal = PersistentTerminal {
        ok: false,
        payload: serde_json::to_value(error)
            .unwrap_or_else(|_| json!({"code": "conversation_dispatch_failed"})),
    };
    if PersistentConversationRuntime::finish(turn, terminal.clone()).is_err() {
        PersistentConversationRuntime::force_terminal(turn, terminal);
    }
}

fn write_persistent_terminal<W: Write>(
    writer: &Arc<StdioRpcWriter<W>>,
    lane: StdioRpcFrameLane,
    request_id: &str,
    workflow_id: &str,
    sequence: u64,
    terminal: &PersistentTerminal,
) -> io::Result<()> {
    let frame = if terminal.ok {
        json!({
            "protocol": STDIO_RPC_PROTOCOL,
            "id": request_id,
            "workflowId": workflow_id,
            "kind": "terminal",
            "sequence": sequence,
            "ok": true,
            "result": terminal.payload,
        })
    } else {
        json!({
            "protocol": STDIO_RPC_PROTOCOL,
            "id": request_id,
            "workflowId": workflow_id,
            "kind": "terminal",
            "sequence": sequence,
            "ok": false,
            "error": terminal.payload,
        })
    };
    if writer
        .lane_target(lane)
        .write_frame(STDIO_RPC_MAX_RESPONSE_BYTES, &frame)?
    {
        Ok(())
    } else {
        Err(io::Error::other("conversation terminal exceeds limit"))
    }
}

/// Stop one admitted task through its actual owner.
///
/// The durable workflow run is reached through the same strategy service
/// composition the RPC strategy arm uses, so the run drive signals its exact
/// live turn. The persistent conversation turn and the supervised lane session
/// keep their existing owners; this composition adds no second control path.
/// A process that hosts neither owner reports the owner as unavailable instead
/// of fabricating a cancellation.
pub(super) fn stop_task(
    params: &Value,
    runtime: Option<&PersistentConversationRuntime>,
    portable_data_dir: Option<PathBuf>,
) -> Value {
    use licoup_native::domain::workflow_runtime::TurnCancelDisposition;
    use licoup_native::platform::stop_control::{OwnerStopDisposition, StopTarget, WorkStopPorts};
    let turn_runtime = runtime.cloned();
    let turn_port = move |target: &StopTarget| -> OwnerStopDisposition {
        let Some(runtime) = turn_runtime.as_ref() else {
            return OwnerStopDisposition::Unavailable;
        };
        let Some(handle) = target.turn_handle.as_deref() else {
            return OwnerStopDisposition::NotActive;
        };
        match runtime.cancel_opened_turn(handle) {
            TurnCancelDisposition::Accepted => OwnerStopDisposition::Acknowledged,
            TurnCancelDisposition::NoActiveTurn => OwnerStopDisposition::NotActive,
            TurnCancelDisposition::Unavailable => OwnerStopDisposition::Unavailable,
        }
    };
    let run_runtime = runtime.cloned();
    let run_port = move |target: &StopTarget| -> OwnerStopDisposition {
        let Some(run_id) = target.run_id.as_deref() else {
            return OwnerStopDisposition::NotActive;
        };
        // Without the persistent host runtime the drive that owns the live
        // turn does not exist in this process; no cancellation can reach it.
        let Some(runtime) = run_runtime.as_ref() else {
            return OwnerStopDisposition::Unavailable;
        };
        let Ok(root) = licoup_foundation::platform::paths::portable_data_dir() else {
            return OwnerStopDisposition::Unavailable;
        };
        let Ok(service) = licoup_native::domain::workflow_runtime::StrategyService::open(&root)
        else {
            return OwnerStopDisposition::Unavailable;
        };
        let service = service
            .with_actor_turn_port(strategy_turn_port(
                runtime.clone(),
                portable_data_dir.clone(),
            ))
            .with_assistant_wake_port(assistant_wake_port(
                runtime.clone(),
                portable_data_dir.clone(),
            ));
        // An Assistant-owned run cancels through its own action; the durable
        // admission of that action also requires its route receipt.
        let assistant_owned = service
            .store()
            .run(run_id)
            .map(|snapshot| {
                snapshot.assistant_membership_id.is_some() && snapshot.route_receipt.is_some()
            })
            .unwrap_or(false);
        let action = if assistant_owned {
            "strategy.assistant.workflow.cancel"
        } else {
            "strategy.run.cancel"
        };
        let request = json!({
            "action": action,
            "runId": run_id,
            "correlationId": target.correlation_id,
        });
        match service.execute(request) {
            Ok(response) if response.get("ok").and_then(Value::as_bool) == Some(true) => {
                let settled = service
                    .store()
                    .run(run_id)
                    .map(|snapshot| {
                        !snapshot.commands.values().any(|command| {
                            matches!(
                                command.status,
                                licoup_workflow::CommandStatus::InDoubt
                                    | licoup_workflow::CommandStatus::CancelRequested
                            )
                        })
                    })
                    .unwrap_or(false);
                if settled {
                    OwnerStopDisposition::Acknowledged
                } else {
                    OwnerStopDisposition::Requested
                }
            }
            _ => OwnerStopDisposition::Unavailable,
        }
    };
    let ports = WorkStopPorts {
        turn: runtime
            .is_some()
            .then_some(&turn_port as &(dyn Fn(&StopTarget) -> OwnerStopDisposition + Sync)),
        run: Some(&run_port as &(dyn Fn(&StopTarget) -> OwnerStopDisposition + Sync)),
    };
    licoup_native::platform::stop_control::stop_work(params, &ports)
}

pub(super) fn has_capacity(workers: &[std::thread::JoinHandle<()>]) -> bool {
    workers.len() < MAX_CONCURRENT_SENDS
}

/// The strategy drive's Conversation-dispatch port, composed once where the
/// persistent host runtime already exists. Strategy commands cross the host
/// admission boundary before they run; open registers a turn, run executes an
/// opened turn, and abandon settles one that will never run. An absent runtime
/// keeps the strategy service fail closed.
pub(super) fn strategy_turn_port(
    runtime: PersistentConversationRuntime,
    portable_data_dir: Option<PathBuf>,
) -> licoup_native::domain::workflow_runtime::ActorTurnPort {
    let open_runtime = runtime.clone();
    let run_runtime = runtime.clone();
    let cancel_runtime = runtime.clone();
    let run_dir = portable_data_dir;
    licoup_native::domain::workflow_runtime::ActorTurnPort {
        open: Arc::new(move |params| {
            open_runtime.open_admitted_turn(params).map_err(|error| {
                licoup_native::domain::workflow_runtime::ActorTurnError::dispatch_failed(
                    error.to_string(),
                )
            })
        }),
        run: Arc::new(move |handle, params| {
            run_runtime
                .run_open_turn(handle, params, run_dir.clone())
                .map_err(|error| {
                    licoup_native::domain::workflow_runtime::ActorTurnError::dispatch_failed(
                        error.to_string(),
                    )
                })
        }),
        cancel: Arc::new(move |handle| cancel_runtime.cancel_opened_turn(handle)),
        abandon: Arc::new(move |handle| runtime.abandon_turn(handle)),
    }
}

/// The designated-Assistant notice port: a notice already durable on the
/// Conversation timeline wakes exactly one new Assistant turn, and only when
/// no turn of that membership is in flight. Composition shares the persistent
/// host runtime; without it the strategy service keeps notices timeline-only.
pub(super) fn assistant_wake_port(
    runtime: PersistentConversationRuntime,
    portable_data_dir: Option<PathBuf>,
) -> licoup_native::domain::workflow_runtime::AssistantWakePort {
    licoup_native::domain::workflow_runtime::AssistantWakePort {
        wake: Arc::new(move |conversation_id, membership_id, notice| {
            if runtime.live_turn_for_membership(membership_id) {
                return Ok(());
            }
            let agent_id = {
                let conversation = runtime
                    .inner
                    .store
                    .get(conversation_id)
                    .map_err(|_| "assistant_wake_conversation_unavailable".to_owned())?;
                conversation
                    .memberships
                    .iter()
                    .find(|membership| membership.id == membership_id)
                    .and_then(|membership| membership.principal.agent_id.clone())
                    .ok_or_else(|| "assistant_wake_agent_unavailable".to_owned())?
            };
            let params = json!({
                "agent": agent_id,
                "agentId": agent_id,
                "text": assistant_notice_text(notice),
                "streamEvents": true,
                "conversationId": conversation_id,
                "membershipId": membership_id,
                "causationId": notice.get("runId").cloned().unwrap_or(Value::Null),
            });
            let wake_runtime = runtime.clone();
            let wake_dir = portable_data_dir.clone();
            std::thread::Builder::new()
                .name("assistant-wake".to_owned())
                .spawn(move || {
                    let _ = wake_runtime.start_admitted_background(&params, wake_dir);
                })
                .map_err(|error| error.to_string())?;
            Ok(())
        }),
    }
}

/// The turn input for a notice carries the identifier event only: its kind,
/// run/state/visit identifiers, and edge mode. Worker transcripts, prompts,
/// paths and tool results never enter the Assistant turn.
fn assistant_notice_text(notice: &Value) -> String {
    let payload = serde_json::to_string(notice).unwrap_or_else(|_| "{}".to_owned());
    format!(
        "Adaptive Flywheel notice for the designated Assistant — identifier-only; read this Conversation for details: {payload}"
    )
}

pub(super) fn join_until_completion(workers: &mut Vec<std::thread::JoinHandle<()>>) {
    while !workers.is_empty() {
        reap_finished(workers);
        if !workers.is_empty() {
            std::thread::sleep(SHUTDOWN_POLL_INTERVAL);
        }
    }
}

fn unix_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
fn join_until(workers: &mut Vec<std::thread::JoinHandle<()>>, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        reap_finished(workers);
        if workers.is_empty() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

pub(super) fn reap_finished(workers: &mut Vec<std::thread::JoinHandle<()>>) {
    let mut index = 0;
    while index < workers.len() {
        if workers[index].is_finished() {
            let worker = workers.swap_remove(index);
            let _ = worker.join();
        } else {
            index += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use licoup_conversation::continuity::{
        ASSISTANT_TURN_INVALID_ERROR, ContinuityAssistantTurnResponse,
        ContinuityCommitmentProposal, ContinuityFollowThroughKind,
        ContinuityInterpretationProposal, ContinuityMatterSubject, ContinuityReadPort,
        ContinuitySpeechAct, ContinuityTaskChildAdmission, ContinuityWriteEnvelope,
        TRUSTED_RESPONSE_MODE_ASSISTANT_TURN, list_all_parent_grants, settlement_applied,
    };
    use licoup_native::domain::client_conversation::{
        ConversationService, DirectTurn, DirectTurnExecutionContext, DispatchSessionMode,
        EventPartKind, ImageAttachment, ImageAttachmentReference, MembershipAccess,
        PersistentRuntimePorts, Principal, PrincipalKind, RuntimeBinding,
        SubagentDispatchClaimState, TurnState,
    };
    use serde_json::{Value, json};

    static FAKE_CODEX_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn runtime(cache_budget: usize) -> PersistentConversationRuntime {
        PersistentConversationRuntime::with_cache_budget(
            ConversationStore::open_in_memory().unwrap(),
            cache_budget,
        )
    }

    fn decode_replay_frames(writer: &Arc<StdioRpcWriter<Vec<u8>>>) -> Vec<Value> {
        String::from_utf8(writer.lock_for_test().clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect()
    }

    fn assert_contiguous_store_fallback_replay<'a>(
        frames: &'a [Value],
        high_water: u64,
    ) -> &'a Value {
        assert!(
            high_water > 0,
            "store-fallback from cursor 0 must have real nonterminal frames"
        );
        assert!(
            frames.len() as u64 >= high_water + 1,
            "replay must deliver the committed range then a terminal: {frames:?}"
        );
        let terminal = frames
            .last()
            .expect("replay must end with a terminal frame");
        assert_eq!(
            terminal.get("kind").and_then(Value::as_str),
            Some("terminal"),
            "{terminal}"
        );
        let event_frames = &frames[..frames.len() - 1];
        assert_eq!(
            event_frames.len() as u64,
            high_water,
            "replay must deliver every committed cursor through high_water: {frames:?}"
        );
        let cursors: Vec<u64> = event_frames
            .iter()
            .map(|frame| {
                frame
                    .pointer("/event/cursor")
                    .and_then(Value::as_u64)
                    .expect("committed replay frame must carry a cursor")
            })
            .collect();
        assert_eq!(
            cursors,
            (1..=high_water).collect::<Vec<_>>(),
            "replay cursors must be contiguous through high_water"
        );
        terminal
    }

    fn public_terminal_code(payload: &Value) -> Option<&str> {
        payload
            .get("code")
            .and_then(Value::as_str)
            .or_else(|| payload.pointer("/error/code").and_then(Value::as_str))
    }

    fn public_terminal_stage(payload: &Value) -> Option<&str> {
        payload
            .get("stage")
            .and_then(Value::as_str)
            .or_else(|| payload.pointer("/error/stage").and_then(Value::as_str))
    }

    /// A boundary-queued continuation of a post with image attachments must
    /// carry the same attachment references to the member adapter admission.
    #[test]
    fn direct_turn_params_carry_image_attachment_references() {
        let mut context = DirectTurnExecutionContext {
            turn: DirectTurn {
                id: "turn:synthetic".to_owned(),
                conversation_id: "conversation:synthetic".to_owned(),
                source_event_id: "event:synthetic".to_owned(),
                membership_id: "membership:agent".to_owned(),
                state: TurnState::Claimed,
                ordinal: 0,
            },
            agent_id: "codex".to_owned(),
            source_content: "exact user-authored text".to_owned(),
            source_attachments: Vec::new(),
            is_assistant: false,
            preferred_model: None,
            preferred_reasoning_effort: None,
            preferred_permission_mode: None,
            preferred_instructions: None,
            preferred_context_window: None,
            runtime_session_id: None,
            runtime_conversation_path: None,
            working_directory: None,
        };
        assert!(
            direct_turn_params(&context)
                .unwrap()
                .get("attachments")
                .is_none()
        );

        context.source_attachments = vec![ImageAttachmentReference {
            part_id: "part:image-1".to_owned(),
            attachment: ImageAttachment {
                path: "fixtures/mockup.png".to_owned(),
                name: "mockup.png".to_owned(),
                media_type: "image/png".to_owned(),
                byte_size: 12,
            },
        }];
        let params = direct_turn_params(&context).unwrap();
        assert_eq!(
            params["attachments"],
            json!([{
                "id": "part:image-1",
                "name": "mockup.png",
                "mediaType": "image/png",
                "path": "fixtures/mockup.png",
            }])
        );
    }

    #[test]
    fn assistant_boundary_continuation_keeps_guidance_private_and_user_text_exact() {
        let context = DirectTurnExecutionContext {
            turn: DirectTurn {
                id: "turn:synthetic".to_owned(),
                conversation_id: "conversation:synthetic".to_owned(),
                source_event_id: "event:synthetic".to_owned(),
                membership_id: "membership:assistant".to_owned(),
                state: TurnState::Claimed,
                ordinal: 0,
            },
            agent_id: "codex".to_owned(),
            source_content: "exact user-authored text".to_owned(),
            source_attachments: Vec::new(),
            is_assistant: true,
            preferred_model: None,
            preferred_reasoning_effort: None,
            preferred_permission_mode: None,
            preferred_instructions: None,
            preferred_context_window: None,
            runtime_session_id: None,
            runtime_conversation_path: None,
            working_directory: None,
        };

        let params = direct_turn_params(&context).unwrap();
        let text = params["text"].as_str().unwrap();
        assert_eq!(text, "exact user-authored text");
        let instructions = context.private_instructions().unwrap();
        assert_eq!(params["developerInstructions"], instructions);
        assert_eq!(params.get("privateInstructions"), None);

        let mut ordinary = context;
        ordinary.is_assistant = false;
        let ordinary_params = direct_turn_params(&ordinary).unwrap();
        assert_eq!(
            ordinary_params["text"].as_str().unwrap(),
            "exact user-authored text"
        );
        assert!(ordinary_params.get("privateInstructions").is_none());
    }

    #[test]
    fn direct_turn_params_pass_profile_reasoning_effort() {
        let context = DirectTurnExecutionContext {
            turn: DirectTurn {
                id: "turn:synthetic".to_owned(),
                conversation_id: "conversation:synthetic".to_owned(),
                source_event_id: "event:synthetic".to_owned(),
                membership_id: "membership:agent".to_owned(),
                state: TurnState::Claimed,
                ordinal: 0,
            },
            agent_id: "claude-code".to_owned(),
            source_content: "exact user-authored text".to_owned(),
            source_attachments: Vec::new(),
            is_assistant: false,
            preferred_model: Some("opus-5".to_owned()),
            preferred_reasoning_effort: Some("xhigh".to_owned()),
            preferred_permission_mode: None,
            preferred_instructions: None,
            preferred_context_window: None,
            runtime_session_id: None,
            runtime_conversation_path: None,
            working_directory: None,
        };
        let params = direct_turn_params(&context).unwrap();
        assert_eq!(params["model"], "opus-5");
        assert_eq!(params["reasoningEffort"], "xhigh");
        assert_eq!(params["timeoutMs"], 0);
        assert!(params.get("timeoutUnbounded").is_none());

        let empty = DirectTurnExecutionContext {
            preferred_model: None,
            preferred_reasoning_effort: None,
            ..context
        };
        let empty_params = direct_turn_params(&empty).unwrap();
        assert!(empty_params.get("reasoningEffort").is_none());
    }

    #[test]
    fn persistent_runtime_replays_after_cursor_in_order() {
        let runtime = runtime(1);
        let turn = runtime
            .begin(&json!({
                "agent": "synthetic",
                "sessionId": "session-1",
                "text": "synthetic prompt"
            }))
            .unwrap();
        for ordinal in 1..=3 {
            PersistentConversationRuntime::record_event(
                &turn,
                json!({
                    "event": "agent.message.chunk",
                    "sessionId": "session-1",
                    "turnId": "native-turn-1",
                    "payload": {"ordinal": ordinal}
                }),
            )
            .unwrap();
        }
        PersistentConversationRuntime::finish(
            &turn,
            PersistentTerminal {
                ok: true,
                payload: json!({
                    "ok": true,
                    "sessionId": "session-1",
                    "output": "final answer"
                }),
            },
        )
        .unwrap();

        let state = turn.state.lock().unwrap();
        assert!(state.cache_bytes <= 1);
        assert!(state.cache.is_empty());
        drop(state);

        let writer = Arc::new(StdioRpcWriter::new(Vec::<u8>::new()));
        replay_turn(&writer, "request-attach", "workflow-1", &turn, 1).unwrap();
        let output = String::from_utf8(writer.lock_for_test().clone()).unwrap();
        let frames = output
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0]["event"]["cursor"], 2);
        assert_eq!(frames[1]["event"]["cursor"], 3);
        assert_eq!(frames[2]["kind"], "terminal");
        assert_eq!(frames[0]["sequence"], 1);
        assert_eq!(frames[2]["sequence"], 3);

        let canonical = turn
            .store
            .page_events(&turn.scope.conversation_id, None, 20)
            .unwrap();
        let assistant = canonical
            .events
            .iter()
            .find(|event| event.id == turn.scope.event_id)
            .unwrap();
        assert!(assistant.finalized);
        assert_eq!(
            assistant.correlation_id.as_deref(),
            Some(turn.scope.dispatch_id.as_str())
        );
        assert!(
            assistant
                .parts
                .iter()
                .any(|part| { part.kind == EventPartKind::Text && part.content == "final answer" })
        );
        assert!(
            assistant
                .parts
                .iter()
                .all(|part| !part.content.contains("turnHandle"))
        );
    }

    #[test]
    fn record_event_does_not_advance_cursor_for_user_speech_frames() {
        let runtime = runtime(DEFAULT_TURN_CACHE_BYTES);
        let turn = runtime
            .begin(&json!({
                "agent": "synthetic",
                "sessionId": "session-1",
                "text": "synthetic prompt"
            }))
            .unwrap();
        PersistentConversationRuntime::record_event(
            &turn,
            json!({
                "event": licoup_conversation::projection::USER_MESSAGE_EVENT_KIND,
                "sessionId": "session-1",
                "turnId": "native-turn-1",
                "payload": {"text": "follow up", "role": "user"}
            }),
        )
        .unwrap();
        assert_eq!(
            turn.state.lock().unwrap().high_water,
            0,
            "user-speech must not occupy a replay cursor"
        );
        PersistentConversationRuntime::record_event(
            &turn,
            json!({
                "event": "agent.message.chunk",
                "sessionId": "session-1",
                "turnId": "native-turn-1",
                "payload": {"ordinal": 1, "text": "Reply"}
            }),
        )
        .unwrap();
        assert_eq!(turn.state.lock().unwrap().high_water, 1);
        let frames = turn
            .store
            .runtime_frames_after(&turn.scope, 0, 1, 8)
            .unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0]["cursor"], 1);
        assert_eq!(frames[0]["event"], "agent.message.chunk");
        PersistentConversationRuntime::finish(
            &turn,
            PersistentTerminal {
                ok: true,
                payload: json!({"ok": true, "output": "Reply"}),
            },
        )
        .unwrap();
        runtime.evict_turn_cache(&turn.scope.dispatch_id);
        let writer = Arc::new(StdioRpcWriter::new(Vec::<u8>::new()));
        replay_turn(&writer, "request-user-speech", "workflow-1", &turn, 0).unwrap();
        let replayed = decode_replay_frames(&writer);
        assert_contiguous_store_fallback_replay(&replayed, 1);
    }

    #[test]
    fn persistent_runtime_serializes_concurrent_cursor_persistence() {
        const EMITTERS: usize = 8;
        let runtime = runtime(DEFAULT_TURN_CACHE_BYTES);
        let turn = runtime
            .begin(&json!({"agent": "synthetic", "text": "synthetic prompt"}))
            .unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(EMITTERS));
        let mut emitters = Vec::with_capacity(EMITTERS);
        for ordinal in 0..EMITTERS {
            let turn = Arc::clone(&turn);
            let barrier = Arc::clone(&barrier);
            emitters.push(std::thread::spawn(move || {
                barrier.wait();
                PersistentConversationRuntime::record_event(
                    &turn,
                    json!({
                        "event": "agent.message.chunk",
                        "payload": {"ordinal": ordinal}
                    }),
                )
                .unwrap();
            }));
        }
        for emitter in emitters {
            emitter.join().unwrap();
        }

        let state = turn.state.lock().unwrap();
        assert_eq!(state.high_water, EMITTERS as u64);
        drop(state);
        let frames = turn
            .store
            .runtime_frames_after(&turn.scope, 0, EMITTERS as u64, EMITTERS)
            .unwrap();
        assert_eq!(frames.len(), EMITTERS);
        assert_eq!(
            frames
                .iter()
                .filter_map(|frame| frame.get("cursor").and_then(Value::as_u64))
                .collect::<Vec<_>>(),
            (1..=EMITTERS as u64).collect::<Vec<_>>()
        );
    }

    #[test]
    fn persistent_runtime_active_discovery_is_scoped_without_content() {
        let runtime = runtime(DEFAULT_TURN_CACHE_BYTES);
        let turn = runtime
            .begin(&json!({
                "agent": "synthetic",
                "sessionId": "session-1",
                "text": "synthetic prompt"
            }))
            .unwrap();
        PersistentConversationRuntime::record_event(
            &turn,
            json!({
                "event": "agent.turn.processing",
                "sessionId": "session-1",
                "turnId": "native-turn-1",
                "payload": {"private": "not projected by discovery"}
            }),
        )
        .unwrap();

        let active = runtime.active(&json!({"agent": "synthetic", "sessionId": "session-1"}));
        assert_eq!(active["turns"].as_array().unwrap().len(), 1);
        let encoded = serde_json::to_string(&active).unwrap();
        assert!(!encoded.contains("not projected"));
        assert_eq!(active["turns"][0]["highWater"], 1);
        assert_eq!(
            active["turns"][0]["conversationId"],
            turn.scope.conversation_id
        );
    }

    #[test]
    fn persistent_runtime_active_discovery_waits_for_registration_signal() {
        let runtime = runtime(DEFAULT_TURN_CACHE_BYTES);
        let waiter = runtime.clone();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let wait_barrier = Arc::clone(&barrier);
        let waiting = std::thread::spawn(move || {
            wait_barrier.wait();
            waiter.active(&json!({
                "agent": "synthetic",
                "waitForChangeMs": 1000
            }))
        });
        barrier.wait();
        let turn = runtime
            .begin(&json!({"agent": "synthetic", "text": "synthetic prompt"}))
            .unwrap();

        let active = waiting.join().unwrap();
        assert_eq!(active["turns"][0]["turnHandle"], turn.scope.dispatch_id);
    }

    #[test]
    fn persistent_runtime_resolves_controls_only_for_the_canonical_scope() {
        let runtime = runtime(DEFAULT_TURN_CACHE_BYTES);
        let turn = runtime
            .begin(&json!({
                "agent": "synthetic",
                "sessionId": "session-1",
                "text": "synthetic prompt"
            }))
            .unwrap();
        PersistentConversationRuntime::record_event(
            &turn,
            json!({
                "event": "agent.turn.processing",
                "sessionId": "session-1",
                "turnId": "native-turn-1",
                "payload": {"status": "processing"}
            }),
        )
        .unwrap();

        let resolved = runtime
            .scoped_control_params(&json!({
                "turnHandle": turn.scope.dispatch_id,
                "conversationId": turn.scope.conversation_id,
                "text": "focus"
            }))
            .unwrap();
        assert_eq!(resolved["agent"], "synthetic");
        assert_eq!(resolved["sessionId"], "session-1");
        assert_eq!(resolved["turnId"], "native-turn-1");
        assert!(
            runtime
                .scoped_control_params(&json!({
                    "turnHandle": turn.scope.dispatch_id,
                    "conversationId": "conversation:other"
                }))
                .is_err()
        );
        assert!(
            runtime
                .scoped_control_params(&json!({"turnHandle": turn.scope.dispatch_id}))
                .is_err()
        );
    }

    #[test]
    fn persistent_runtime_retains_cancel_requested_before_native_binding() {
        let runtime = runtime(DEFAULT_TURN_CACHE_BYTES);
        let turn = runtime
            .begin(&json!({
                "agent": "cursor",
                "text": "synthetic prompt"
            }))
            .unwrap();

        let pending = runtime
            .request_cancel(&json!({
                "turnHandle": turn.scope.dispatch_id,
                "conversationId": turn.scope.conversation_id,
                "agentId": "cursor"
            }))
            .unwrap();
        assert_eq!(pending["ok"], true);
        assert_eq!(pending["status"], "cancel_requested");
        assert!(turn.cancel_requested.load(Ordering::Acquire));
    }

    #[test]
    fn persistent_runtime_reuses_group_conversation_ownership() {
        let store = ConversationStore::open_in_memory().unwrap();
        let service = ConversationService::from_store(store.clone());
        let group = service
            .execute(json!({
                "action": "conversation.create",
                "title": "Persistent Group",
                "owner": {"id": "human:local", "kind": "human", "displayName": "You"},
                "members": [
                    {"principal": {"id": "agent:synthetic", "kind": "agent", "displayName": "Synthetic", "agentId": "synthetic"}, "access": "member"}
                ]
            }))
            .unwrap();
        let conversation_id = group["id"].as_str().unwrap();
        let membership_id = group["memberships"]
            .as_array()
            .unwrap()
            .iter()
            .find(|membership| membership["principal"]["kind"] == "agent")
            .unwrap()["id"]
            .as_str()
            .unwrap();
        store
            .runtime_binding_with_private_location(
                RuntimeBinding {
                    id: "binding:persistent-runtime".to_owned(),
                    conversation_id: conversation_id.to_owned(),
                    membership_id: membership_id.to_owned(),
                    lane: "conversation".to_owned(),
                    availability: "available".to_owned(),
                    safe_reason: None,
                },
                Some("native-session-existing"),
                Some("conversation.md"),
                Some("/workspace/project"),
            )
            .unwrap();
        let runtime = PersistentConversationRuntime::with_cache_budget(
            store.clone(),
            DEFAULT_TURN_CACHE_BYTES,
        );
        let turn = runtime
            .begin(&json!({
                "agent": "synthetic",
                "text": "group prompt",
                "conversationId": conversation_id,
                "membershipId": membership_id,
                "causationId": "event:source"
            }))
            .unwrap();

        assert_eq!(turn.scope.conversation_id, conversation_id);
        assert_eq!(turn.scope.membership_id, membership_id);
        assert_eq!(
            turn.session_id.lock().unwrap().as_str(),
            "native-session-existing"
        );
        let dispatch = store
            .dispatch_record(&turn.scope.dispatch_id)
            .unwrap()
            .expect("group dispatch record");
        assert_eq!(dispatch.session_mode, DispatchSessionMode::Resume);
        let native_params = runtime.params_for_turn(
            &turn,
            &json!({
                "agent": "synthetic",
                "text": "group prompt",
                "conversationId": conversation_id,
                "membershipId": membership_id,
            }),
        );
        assert_eq!(native_params["sessionId"], "native-session-existing");
        assert_eq!(native_params["sourcePath"], "conversation.md");
        assert_eq!(native_params["workingDirectory"], "/workspace/project");
        assert_eq!(
            runtime.active(&json!({"conversationId": conversation_id}))["turns"][0]["turnHandle"],
            turn.scope.dispatch_id
        );
    }

    struct SubagentFixture {
        conversation_id: String,
        caller_membership: String,
        target_membership: String,
        claim_id: String,
    }

    /// One admitted caller→target edge with the durable claim in `running`,
    /// using non-driver agent ids so the callback's lane dispatch fails fast
    /// instead of reaching a real provider executable.
    fn subagent_fixture(store: &ConversationStore) -> SubagentFixture {
        let owner = Principal {
            id: "human:owner".into(),
            kind: PrincipalKind::Human,
            display_name: "Owner".into(),
            agent_id: None,
            created_at_unix_ms: 1,
        };
        let members = ["caller-agent", "target-agent"].map(|agent_id| {
            (
                Principal {
                    id: format!("agent:{agent_id}"),
                    kind: PrincipalKind::Agent,
                    display_name: agent_id.into(),
                    agent_id: Some(agent_id.into()),
                    created_at_unix_ms: 1,
                },
                MembershipAccess::Member,
            )
        });
        let conversation = store
            .create_conversation_with_members("Subagent Callback", owner, &members)
            .unwrap();
        let membership = |agent_id: &str| {
            conversation
                .memberships
                .iter()
                .find(|membership| membership.principal.agent_id.as_deref() == Some(agent_id))
                .unwrap()
                .id
                .clone()
        };
        let caller_membership = membership("caller-agent");
        let target_membership = membership("target-agent");
        let claim = store
            .claim_subagent_dispatch(
                &conversation.id,
                &caller_membership,
                &target_membership,
                None,
            )
            .unwrap();
        store
            .update_subagent_claim_state(&claim.id, SubagentDispatchClaimState::Running)
            .unwrap();
        SubagentFixture {
            conversation_id: conversation.id,
            caller_membership,
            target_membership,
            claim_id: claim.id,
        }
    }

    fn callback_events(
        store: &ConversationStore,
        conversation_id: &str,
    ) -> Vec<licoup_native::domain::client_conversation::ConversationEvent> {
        store
            .page_events(conversation_id, None, 50)
            .unwrap()
            .events
            .into_iter()
            .filter(|event| {
                event.causation_id.as_deref()
                    == Some(licoup_native::domain::subagents::CALLBACK_CAUSATION_ID)
            })
            .collect()
    }

    fn wait_for_callback_events(
        store: &ConversationStore,
        conversation_id: &str,
        expected: usize,
    ) -> Vec<licoup_native::domain::client_conversation::ConversationEvent> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let events = callback_events(store, conversation_id);
            if events.len() >= expected || Instant::now() >= deadline {
                return events;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn subagent_callback_and_wake_reuse_caller_membership_session() {
        let store = ConversationStore::open_in_memory().unwrap();
        let fixture = subagent_fixture(&store);
        store
            .runtime_binding_with_private_location(
                RuntimeBinding {
                    id: "binding:callback-caller".to_owned(),
                    conversation_id: fixture.conversation_id.clone(),
                    membership_id: fixture.caller_membership.clone(),
                    lane: "conversation".to_owned(),
                    availability: "available".to_owned(),
                    safe_reason: None,
                },
                Some("caller-native-session"),
                Some("caller-conversation.md"),
                Some("/workspace/caller"),
            )
            .unwrap();
        let runtime = PersistentConversationRuntime::with_cache_budget(
            store.clone(),
            DEFAULT_TURN_CACHE_BYTES,
        );
        let target_turn = runtime
            .begin(&json!({
                "agent": "target-agent",
                "text": "delegated prompt",
                "conversationId": fixture.conversation_id.as_str(),
                "membershipId": fixture.target_membership.as_str(),
                "causationId": "subagent-mcp",
                "dispatchId": fixture.claim_id.as_str(),
            }))
            .unwrap();
        PersistentConversationRuntime::finish(
            &target_turn,
            PersistentTerminal {
                ok: true,
                payload: json!({"ok": true, "output": "delegated final answer"}),
            },
        )
        .unwrap();

        let callback_dispatch = {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(dispatch) = store
                    .latest_send_dispatch(&fixture.conversation_id, &fixture.caller_membership)
                    .unwrap()
                {
                    break dispatch;
                }
                assert!(
                    Instant::now() < deadline,
                    "callback dispatch must be accepted by the persistent runtime"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        assert_eq!(callback_dispatch.session_mode, DispatchSessionMode::Resume);
        assert_eq!(
            runtime
                .inspect_turn(&callback_dispatch.id)
                .expect("callback PersistentTurn")
                .0,
            "caller-native-session"
        );

        let callback_dispatch = {
            let callback_id = callback_dispatch.id.clone();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let dispatch = store
                    .dispatch_record(&callback_id)
                    .unwrap()
                    .expect("callback dispatch record");
                if matches!(
                    dispatch.state,
                    DispatchState::Completed | DispatchState::Failed | DispatchState::Cancelled
                ) {
                    break dispatch;
                }
                assert!(Instant::now() < deadline, "callback dispatch must settle");
                std::thread::sleep(Duration::from_millis(10));
            }
        };

        let port = assistant_wake_port(runtime.clone(), None);
        (port.wake)(
            &fixture.conversation_id,
            &fixture.caller_membership,
            &json!({
                "kind": "strategy-flow-settled",
                "runId": "run-callback-wake",
                "stateId": "resume",
                "stateVisit": 1,
                "mode": "flow",
            }),
        )
        .unwrap();
        let wake_dispatch = {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(dispatch) = store
                    .latest_send_dispatch(&fixture.conversation_id, &fixture.caller_membership)
                    .unwrap()
                    .filter(|dispatch| dispatch.id != callback_dispatch.id)
                {
                    break dispatch;
                }
                assert!(
                    Instant::now() < deadline,
                    "wake dispatch must be accepted by the persistent runtime"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        assert_eq!(wake_dispatch.session_mode, DispatchSessionMode::Resume);
        assert_eq!(
            runtime
                .inspect_turn(&wake_dispatch.id)
                .expect("wake PersistentTurn"),
            ("caller-native-session".to_owned(), String::new())
        );
    }

    /// A claimed delegated turn settles: the claim moves to the matching
    /// terminal state eagerly and exactly one callback turn is dispatched to
    /// the caller membership through the same PersistentTurn door.
    #[test]
    fn subagent_terminal_finish_fires_one_caller_callback_turn() {
        let store = ConversationStore::open_in_memory().unwrap();
        let fixture = subagent_fixture(&store);
        let runtime = PersistentConversationRuntime::with_cache_budget(
            store.clone(),
            DEFAULT_TURN_CACHE_BYTES,
        );
        let turn = runtime
            .begin(&json!({
                "agent": "target-agent",
                "text": "delegated prompt",
                "timeoutMs": 60_000,
                "conversationId": fixture.conversation_id.as_str(),
                "membershipId": fixture.target_membership.as_str(),
                "causationId": "subagent-mcp",
                "dispatchId": fixture.claim_id.as_str(),
            }))
            .unwrap();
        assert_eq!(
            runtime.inner.subagent_watchdog.lock().unwrap().len(),
            1,
            "a claimed dispatch with timeoutMs registers a watchdog deadline"
        );
        PersistentConversationRuntime::record_event(
            &turn,
            json!({
                "event": "agent.message.chunk",
                "sessionId": "native-session-1",
                "turnId": "turn-1",
                "payload": {"text": "delegated final answer"}
            }),
        )
        .unwrap();
        PersistentConversationRuntime::finish(
            &turn,
            PersistentTerminal {
                ok: true,
                payload: json!({"ok": true, "output": "delegated final answer"}),
            },
        )
        .unwrap();

        // Eager claim writeback: `subagent_claim` is a direct row read that
        // never reconciles.
        assert_eq!(
            store
                .subagent_claim(&fixture.claim_id)
                .unwrap()
                .unwrap()
                .state,
            SubagentDispatchClaimState::Completed
        );
        assert!(
            runtime.inner.subagent_watchdog.lock().unwrap().is_empty(),
            "terminal settlement retires the pending watchdog deadline"
        );

        let callbacks = wait_for_callback_events(&store, &fixture.conversation_id, 1);
        assert_eq!(callbacks.len(), 1);
        assert_eq!(
            callbacks[0].author_membership_id.as_deref(),
            Some(fixture.caller_membership.as_str())
        );
        // The fired-once guard holds: the completion signal is one callback.
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(callback_events(&store, &fixture.conversation_id).len(), 1);
    }

    /// The delegated turn never settles: at the configured deadline the
    /// watchdog fires the same caller callback while the claim is still
    /// running, and the later terminal settlement stays silent.
    #[test]
    fn subagent_watchdog_fires_current_state_callback_after_deadline() {
        let store = ConversationStore::open_in_memory().unwrap();
        let fixture = subagent_fixture(&store);
        let runtime = PersistentConversationRuntime::with_cache_budget(
            store.clone(),
            DEFAULT_TURN_CACHE_BYTES,
        );
        let turn = runtime
            .begin(&json!({
                "agent": "target-agent",
                "text": "delegated prompt",
                "timeoutMs": 1_000,
                "conversationId": fixture.conversation_id.as_str(),
                "membershipId": fixture.target_membership.as_str(),
                "causationId": "subagent-mcp",
                "dispatchId": fixture.claim_id.as_str(),
            }))
            .unwrap();

        let callbacks = wait_for_callback_events(&store, &fixture.conversation_id, 1);
        assert_eq!(callbacks.len(), 1);
        assert_eq!(
            callbacks[0].author_membership_id.as_deref(),
            Some(fixture.caller_membership.as_str())
        );
        assert_eq!(
            store
                .subagent_claim(&fixture.claim_id)
                .unwrap()
                .unwrap()
                .state,
            SubagentDispatchClaimState::Running,
            "the timeout fallback reports the current state, not a terminal one"
        );
        assert!(runtime.inner.subagent_watchdog.lock().unwrap().is_empty());

        PersistentConversationRuntime::finish(
            &turn,
            PersistentTerminal {
                ok: true,
                payload: json!({"ok": true, "output": "late answer"}),
            },
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            callback_events(&store, &fixture.conversation_id).len(),
            1,
            "the terminal settlement after a watchdog fire must not re-notify"
        );
        assert_eq!(
            store
                .subagent_claim(&fixture.claim_id)
                .unwrap()
                .unwrap()
                .state,
            SubagentDispatchClaimState::Completed
        );
    }

    /// Without a durable claim row a dispatch never registers a watchdog and
    /// its terminal finish stays silent: callback turns themselves can never
    /// recurse into further callbacks.
    #[test]
    fn unclaimed_turn_with_timeout_never_registers_a_subagent_callback() {
        let store = ConversationStore::open_in_memory().unwrap();
        let runtime = PersistentConversationRuntime::with_cache_budget(
            store.clone(),
            DEFAULT_TURN_CACHE_BYTES,
        );
        let turn = runtime
            .begin(&json!({
                "agent": "synthetic",
                "text": "synthetic prompt",
                "timeoutMs": 1_000,
            }))
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(runtime.inner.subagent_watchdog.lock().unwrap().is_empty());
        PersistentConversationRuntime::finish(
            &turn,
            PersistentTerminal {
                ok: true,
                payload: json!({"ok": true}),
            },
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(callback_events(&store, &turn.scope.conversation_id).is_empty());
    }

    #[test]
    fn persistent_runtime_completed_turn_leaves_active_discovery() {
        let runtime = runtime(DEFAULT_TURN_CACHE_BYTES);
        let turn = runtime
            .begin(&json!({"agent": "synthetic", "text": "synthetic prompt"}))
            .unwrap();
        PersistentConversationRuntime::finish(
            &turn,
            PersistentTerminal {
                ok: true,
                payload: json!({"ok": true}),
            },
        )
        .unwrap();
        assert!(
            runtime.active(&json!({"agent": "synthetic"}))["turns"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn later_observer_failure_cannot_overwrite_first_native_terminal() {
        let runtime = runtime(DEFAULT_TURN_CACHE_BYTES);
        let turn = runtime
            .begin(&json!({"agent": "synthetic", "text": "synthetic prompt"}))
            .unwrap();
        let exact = PersistentTerminal {
            ok: false,
            payload: json!({
                "ok": false,
                "terminalTransition": {
                    "kind": "failed",
                    "code": "exact_native_failure",
                    "stage": "native/turn"
                }
            }),
        };
        PersistentConversationRuntime::finish(&turn, exact).unwrap();
        PersistentConversationRuntime::finish(
            &turn,
            PersistentTerminal {
                ok: false,
                payload: json!({"code": "later_transport_failed"}),
            },
        )
        .unwrap();
        PersistentConversationRuntime::force_terminal(
            &turn,
            PersistentTerminal {
                ok: false,
                payload: json!({"code": "observer_disconnected"}),
            },
        );
        let state = turn.state.lock().unwrap();
        assert_eq!(
            state.terminal.as_ref().unwrap().payload["terminalTransition"]["code"],
            "exact_native_failure"
        );
    }

    #[test]
    fn persistent_runtime_is_not_idle_with_client_or_active_turn() {
        let runtime = runtime(DEFAULT_TURN_CACHE_BYTES);
        assert!(runtime.idle());
        runtime.client_connected();
        assert!(!runtime.idle());
        runtime.client_disconnected();
        let turn = runtime
            .begin(&json!({"agent": "synthetic", "text": "synthetic prompt"}))
            .unwrap();
        assert!(!runtime.idle());
        PersistentConversationRuntime::finish(
            &turn,
            PersistentTerminal {
                ok: true,
                payload: json!({"ok": true}),
            },
        )
        .unwrap();
        assert!(runtime.idle());
    }

    #[test]
    fn persistent_runtime_projects_adapter_failure_as_failed_canonical_turn() {
        let runtime = runtime(DEFAULT_TURN_CACHE_BYTES);
        let turn = runtime
            .begin(&json!({"agent": "synthetic", "text": "synthetic prompt"}))
            .unwrap();
        PersistentConversationRuntime::finish(
            &turn,
            PersistentTerminal {
                // The RPC itself completed normally, but the adapter response
                // reports a failed Agent turn.
                ok: true,
                payload: json!({
                    "ok": false,
                    "error": {
                        "code": "synthetic_failure",
                        "stage": "conversation/dispatch"
                    }
                }),
            },
        )
        .unwrap();

        let canonical = turn
            .store
            .page_events(&turn.scope.conversation_id, None, 20)
            .unwrap();
        let assistant = canonical
            .events
            .iter()
            .find(|event| event.id == turn.scope.event_id)
            .unwrap();
        assert!(assistant.finalized);
        assert!(assistant.parts.iter().any(|part| {
            part.kind == EventPartKind::Diagnostic && part.content.contains("synthetic_failure")
        }));
        assert!(
            runtime.active(&json!({"conversationId": turn.scope.conversation_id}))["turns"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn conversation_worker_capacity_is_bounded() {
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let wait = Arc::new(Mutex::new(wait));
        let mut workers = Vec::new();
        for _ in 0..MAX_CONCURRENT_SENDS {
            let wait = Arc::clone(&wait);
            workers.push(std::thread::spawn(move || {
                let _ = wait.lock().unwrap().recv();
            }));
        }

        assert!(!has_capacity(&workers));
        for _ in 0..MAX_CONCURRENT_SENDS {
            release.send(()).unwrap();
        }
        assert!(join_until(&mut workers, Duration::from_secs(1)));
    }

    #[test]
    fn conversation_worker_shutdown_has_a_deadline() {
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let mut workers = vec![std::thread::spawn(move || {
            let _ = wait.recv();
        })];

        assert!(!join_until(&mut workers, Duration::from_millis(20)));
        release.send(()).unwrap();
        assert!(join_until(&mut workers, Duration::from_secs(1)));
    }

    /// Runs one `conversation::execute` reply on [lane] from another thread and
    /// reports whether it reached the writer inside [within].
    ///
    /// `steer` fails at its first parameter lookup, so this drives the same
    /// reply path the request loop uses without needing a live turn, and the
    /// deadline is what makes the assertion falsifiable: a reply that waits for
    /// a saturated bulk budget fails instead of hanging the suite.
    fn spawn_execute_reply(
        writer: &Arc<StdioRpcWriter<Vec<u8>>>,
        lane: StdioRpcFrameLane,
    ) -> std::sync::mpsc::Receiver<Result<(), String>> {
        let writer = Arc::clone(writer);
        let (finished, reached) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let dispatched = execute(
                &writer,
                lane,
                "request-steer",
                "workflow-1",
                "steer",
                json!({}),
                None,
                false,
                None,
            );
            let _ = finished.send(dispatched.map_err(|error| error.to_string()));
        });
        reached
    }

    fn execute_reply_finishes_within(
        writer: &Arc<StdioRpcWriter<Vec<u8>>>,
        lane: StdioRpcFrameLane,
        within: std::time::Duration,
    ) -> bool {
        spawn_execute_reply(writer, lane)
            .recv_timeout(within)
            .is_ok()
    }

    /// The reply lane of a request is the lane of its reply.
    ///
    /// The request loop writes the reply of a control request inline and cannot
    /// read the next request until it did, so that reply must not queue behind
    /// the bulk write budget; the same call for a worker lane must queue, or the
    /// budget is not the producer's backpressure at all.
    #[test]
    fn producer_control_reply_is_not_queued_behind_the_bulk_budget() {
        let writer = Arc::new(StdioRpcWriter::new(Vec::<u8>::new()));
        let held: Vec<_> = (0..STDIO_RPC_BULK_WRITE_MAX_FRAMES)
            .map(|_| writer.bulk_budget().enter(1024))
            .collect();
        assert!(
            execute_reply_finishes_within(
                &writer,
                StdioRpcFrameLane::Control,
                std::time::Duration::from_secs(5),
            ),
            "the inline reply of a control request waited for the bulk budget"
        );
        let worker_reached = spawn_execute_reply(&writer, StdioRpcFrameLane::Stream);
        assert!(
            worker_reached
                .recv_timeout(std::time::Duration::from_secs(1))
                .is_err(),
            "a worker reply took the writer without waiting for the bulk budget"
        );
        drop(held);
        assert!(
            worker_reached
                .recv_timeout(std::time::Duration::from_secs(5))
                .is_ok(),
            "the worker reply did not settle once the bulk budget drained"
        );
        let frames = String::from_utf8(writer.lock_for_test().clone()).expect("utf8 frames");
        assert_eq!(
            frames.lines().count(),
            2,
            "each reply is one whole frame: {frames}"
        );
        assert!(
            frames
                .lines()
                .all(|line| serde_json::from_str::<Value>(line).is_ok()),
            "every reply frame is one whole json line: {frames}"
        );
        assert!(frames.contains("request-steer"));
    }

    /// SQLite write failure injected into frame persistence: the host-facing
    /// persistence path settles the turn with a typed error delta and the
    /// runtime keeps accepting turns instead of unwinding the process.
    #[test]
    fn stdio_rpc_resilience_sqlite_write_failure_emits_error_delta_and_loop_survives() {
        let store = ConversationStore::open_in_memory().unwrap();
        let runtime = PersistentConversationRuntime::with_cache_budget(
            store.clone(),
            DEFAULT_TURN_CACHE_BYTES,
        );
        let turn = runtime
            .begin(&json!({"agent": "synthetic", "text": "synthetic prompt"}))
            .unwrap();
        // Inject the write failure behind the connection pool: remove the
        // registered dispatch rows so the next persisted frame fails closed.
        {
            let connection = rusqlite::Connection::open(store.db_path()).unwrap();
            connection
                .execute(
                    "DELETE FROM conversation_dispatches WHERE id=?1",
                    [turn.scope.dispatch_id.as_str()],
                )
                .unwrap();
        }

        let writer = Arc::new(StdioRpcWriter::new(Vec::<u8>::new()));
        let dispatched = execute(
            &writer,
            StdioRpcFrameLane::Stream,
            "request-resilience",
            "workflow-resilience",
            "send",
            json!({"agent": "synthetic", "text": "synthetic prompt"}),
            None,
            true,
            Some(turn),
        );
        assert!(
            dispatched.is_ok(),
            "frame persistence failure must not unwind the loop"
        );

        let output = String::from_utf8(writer.lock_for_test().clone()).unwrap();
        let frames = output
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        let terminal = frames
            .iter()
            .find(|frame| frame["id"] == "request-resilience")
            .expect("terminal delta frame");
        assert_eq!(terminal["ok"], false);
        assert!(terminal.get("result").is_none());
        // The known problem code degrades into the canonical ClientError
        // vocabulary; the exact external code is the host metadata codec for
        // frame-persistence failures.
        assert_eq!(terminal["error"]["code"], "command_failed");

        // The loop survived: the same runtime registers and persists a fresh,
        // healthy turn afterwards.
        let next = runtime
            .begin(&json!({"agent": "synthetic", "text": "next prompt"}))
            .unwrap();
        PersistentConversationRuntime::record_event(
            &next,
            json!({
                "event": "agent.message.chunk",
                "payload": {"ordinal": 1}
            }),
        )
        .unwrap();
    }

    /// A panicking dispatch is converted into a `command_panicked` error delta
    /// at the frame-loop boundary and the loop keeps serving the next request.
    #[test]
    fn stdio_rpc_resilience_frame_loop_survives_panicking_dispatch() {
        let mut input = Vec::new();
        for frame in [
            json!({
                "protocol": STDIO_RPC_PROTOCOL,
                "id": "request-panic",
                "workflowId": "workflow-1",
                "method": "execute",
                "args": ["boom"],
            }),
            json!({
                "protocol": STDIO_RPC_PROTOCOL,
                "id": "request-ok",
                "workflowId": "workflow-1",
                "method": "execute",
                "args": ["ok"],
            }),
        ] {
            input.extend_from_slice(&serde_json::to_vec(&frame).unwrap());
            input.push(b'\n');
        }
        let runtime = runtime(DEFAULT_TURN_CACHE_BYTES);
        let output =
            serve_stdio_rpc_with_runtime(
                std::io::Cursor::new(input),
                Vec::new(),
                |args,
                 _|
                 -> std::result::Result<
                    licoup_native::ffi::commands::CliExecution,
                    anyhow::Error,
                > {
                    if args.first().map(String::as_str) == Some("boom") {
                        // Deliberate panic injection: the frame-loop boundary
                        // must convert it into a `command_panicked` error
                        // delta.
                        std::panic::panic_any("synthetic dispatch panic");
                    }
                    Ok(licoup_native::ffi::commands::CliExecution::Json(
                        json!({"ok": true}),
                    ))
                },
                runtime,
            )
            .unwrap();
        let text = String::from_utf8(output).unwrap();
        let frames = text
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0]["id"], "request-panic");
        assert_eq!(frames[0]["ok"], false);
        assert_eq!(frames[0]["error"]["code"], "command_panicked");
        assert!(frames[0].get("result").is_none());
        assert_eq!(frames[1]["id"], "request-ok");
        assert_eq!(frames[1]["ok"], true);
        assert_eq!(frames[1]["result"]["ok"], true);
    }

    #[test]
    fn detached_conversation_host_waits_for_active_work_to_complete() {
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let (joined, joined_rx) = std::sync::mpsc::channel::<()>();
        let worker = std::thread::spawn(move || {
            let _ = wait.recv();
        });

        let host = std::thread::spawn(move || {
            let mut workers = vec![worker];
            join_until_completion(&mut workers);
            joined.send(()).unwrap();
        });

        assert!(joined_rx.recv_timeout(Duration::from_millis(20)).is_err());
        release.send(()).unwrap();
        joined_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        host.join().unwrap();
    }

    #[test]
    fn persistent_runtime_drain_admitted_turns_waits_for_completion() {
        let runtime = runtime(64);
        assert!(runtime.drain_admitted_turns(Duration::from_millis(50)));

        let scope = ConversationRuntimeScope {
            conversation_id: "conv-drain".into(),
            membership_id: "mem-drain".into(),
            event_id: "evt-drain".into(),
            dispatch_id: "disp-drain".into(),
        };
        let turn = Arc::new(PersistentTurn {
            scope: scope.clone(),
            agent_id: "agent-drain".into(),
            session_id: Mutex::new("sess-drain".into()),
            turn_id: Mutex::new("turn-drain".into()),
            state: Mutex::new(PersistentTurnState::default()),
            cancel_requested: AtomicBool::new(false),
            changed: Condvar::new(),
            store: runtime.inner.store.clone(),
            cache_budget: 1024,
            runtime: Arc::downgrade(&runtime.inner),
            continuity_kind: None,
            admitted_assistant_turn: false,
        });
        runtime
            .inner
            .turns
            .lock()
            .unwrap()
            .insert(scope.dispatch_id.clone(), Arc::clone(&turn));

        // Active turn causes drain to wait and time out if not terminal
        assert!(!runtime.drain_admitted_turns(Duration::from_millis(30)));

        // Once turn becomes terminal, drain succeeds
        let turn_clone = Arc::clone(&turn);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            PersistentConversationRuntime::force_terminal(
                &turn_clone,
                PersistentTerminal {
                    ok: true,
                    payload: json!({"output": "done"}),
                },
            );
        });
        assert!(runtime.drain_admitted_turns(Duration::from_secs(1)));
    }

    #[test]
    fn host_stop_requested_rejects_new_turn_admission() {
        let runtime = runtime(64);
        assert!(!runtime.is_host_stop_requested());
        runtime.request_host_stop();
        assert!(runtime.is_host_stop_requested());

        let params = json!({
            "agent": "fixture-agent",
            "sessionId": "sess-1",
            "text": "test turn",
        });
        let adapter_err = runtime.open_turn(&params).unwrap_err();
        assert_eq!(adapter_err, RuntimeAdapterError::ConversationDispatchFailed);
    }

    #[test]
    fn host_boot_rearms_persisted_watchdog_deadlines() {
        let store = ConversationStore::open_in_memory().unwrap();
        let owner = Principal {
            id: "human:owner".into(),
            kind: PrincipalKind::Human,
            display_name: "Owner".into(),
            agent_id: None,
            created_at_unix_ms: 1,
        };
        let conversation = store.create_conversation("Project", owner).unwrap();
        let caller = store
            .add_member(
                &conversation.id,
                Principal {
                    id: "agent:codex".into(),
                    kind: PrincipalKind::Agent,
                    display_name: "Codex".into(),
                    agent_id: Some("codex".into()),
                    created_at_unix_ms: 1,
                },
                MembershipAccess::Member,
            )
            .unwrap();
        let target = store
            .add_member(
                &conversation.id,
                Principal {
                    id: "agent:cursor".into(),
                    kind: PrincipalKind::Agent,
                    display_name: "Cursor".into(),
                    agent_id: Some("cursor".into()),
                    created_at_unix_ms: 1,
                },
                MembershipAccess::Member,
            )
            .unwrap();
        let claim = store
            .claim_subagent_dispatch(&conversation.id, &caller.id, &target.id, None)
            .unwrap();
        store
            .set_subagent_watchdog_deadline(&claim.id, unix_now_ms() + 60_000)
            .unwrap();

        let runtime = PersistentConversationRuntime::with_cache_budget(store, 1024);
        let armed = runtime
            .inner
            .subagent_watchdog
            .lock()
            .unwrap()
            .contains_key(&claim.id);
        assert!(armed);
    }

    #[test]
    fn live_turn_lookup_tracks_active_memberships() {
        let runtime = runtime(64);
        let store = runtime.inner.store.clone();
        let conversation = store
            .create_conversation(
                "Project",
                Principal {
                    id: "human:owner".into(),
                    kind: PrincipalKind::Human,
                    display_name: "Owner".into(),
                    agent_id: None,
                    created_at_unix_ms: 1,
                },
            )
            .unwrap();
        let membership = store
            .add_member(
                &conversation.id,
                Principal {
                    id: "agent:entry".into(),
                    kind: PrincipalKind::Agent,
                    display_name: "Entry".into(),
                    agent_id: Some("entry-agent".into()),
                    created_at_unix_ms: 1,
                },
                MembershipAccess::Member,
            )
            .unwrap();
        let params = json!({
            "agent": "entry-agent",
            "text": "Adaptive Flywheel notice",
            "conversationId": conversation.id,
            "membershipId": membership.id,
        });
        let handle = runtime.open_turn(&params).unwrap();
        assert!(runtime.live_turn_for_membership(&membership.id));
        assert!(!runtime.live_turn_for_membership("membership-elsewhere"));
        runtime.abandon_turn(&handle);
        assert!(!runtime.live_turn_for_membership(&membership.id));
    }

    #[test]
    fn assistant_wake_skips_memberships_with_a_live_turn() {
        let runtime = runtime(64);
        let store = runtime.inner.store.clone();
        let conversation = store
            .create_conversation(
                "Project",
                Principal {
                    id: "human:owner".into(),
                    kind: PrincipalKind::Human,
                    display_name: "Owner".into(),
                    agent_id: None,
                    created_at_unix_ms: 1,
                },
            )
            .unwrap();
        let membership = store
            .add_member(
                &conversation.id,
                Principal {
                    id: "agent:entry".into(),
                    kind: PrincipalKind::Agent,
                    display_name: "Entry".into(),
                    agent_id: Some("entry-agent".into()),
                    created_at_unix_ms: 1,
                },
                MembershipAccess::Member,
            )
            .unwrap();
        let params = json!({
            "agent": "entry-agent",
            "text": "Adaptive Flywheel notice",
            "conversationId": conversation.id,
            "membershipId": membership.id,
        });
        let handle = runtime.open_turn(&params).unwrap();
        let port = assistant_wake_port(runtime.clone(), None);
        let notice = json!({
            "kind": "strategy-flow-settled",
            "runId": "run-1",
            "stateId": "greet",
            "stateVisit": 1,
            "mode": "flow",
        });
        let before = runtime.inner.turns.lock().unwrap().len();
        (port.wake)(&conversation.id, &membership.id, &notice).unwrap();
        let after = runtime.inner.turns.lock().unwrap().len();
        assert_eq!(
            before, after,
            "a live membership turn keeps the notice timeline-only"
        );
        runtime.abandon_turn(&handle);
    }

    #[test]
    fn strategy_turn_port_routes_open_through_host_admission() {
        let runtime = runtime(DEFAULT_TURN_CACHE_BYTES);
        let service = ConversationService::from_store(runtime.inner.store.clone());
        let (conversation_id, _owner, agent) =
            create_designated_group(&service, "Strategy admission");
        let port = strategy_turn_port(runtime.clone(), None);
        let handle = (port.open)(&json!({
            "agent": "codex",
            "text": "run the graph actor",
            "conversationId": conversation_id,
            "membershipId": agent,
            "causationId": "run:strategy-admission",
            "continuityKind": CONTINUITY_KIND_USER_POSTED,
        }))
        .unwrap();
        let turn = runtime.turn(&handle).expect("strategy PersistentTurn");
        assert!(
            turn.admitted_assistant_turn,
            "strategy actor admission must use the host-only turn boundary"
        );
        runtime.abandon_turn(&handle);
    }

    fn compile_fake_codex() -> std::path::PathBuf {
        use std::process::Command;
        use std::time::{SystemTime, UNIX_EPOCH};

        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("fake_codex_app_server.rs");
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("lico-ca-c2-fake-codex-{suffix}"));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let executable = temp_dir.join(format!("fake-codex{}", std::env::consts::EXE_SUFFIX));
        let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
        let compile = Command::new(rustc)
            .arg("--edition=2024")
            .arg(&fixture)
            .arg("-o")
            .arg(&executable)
            .status()
            .expect("fake Codex fixture should compile");
        assert!(compile.success(), "fake Codex fixture failed to compile");
        executable
    }

    /// Wait until the runtime has persisted the `dispatch.turn.bound` frame for
    /// an admitted native turn. That frame is emitted only once the official
    /// lane's process-local in-flight control channel is registered, so it is
    /// the first observable point at which a live steer can be accepted. The
    /// runtime reports the native `turnId` earlier (as soon as the app-server
    /// acknowledges `turn/start`), so polling `inspect_turn` alone leaves a
    /// window in which a steer is answered `session_unavailable`.
    fn wait_for_bound_native_turn(
        service: &ConversationService,
        child_conversation_id: &str,
        child_membership_id: &str,
        dispatch_id: &str,
        expected_turn_id: &str,
    ) {
        const NATIVE_TURN_BIND_WAIT: Duration = Duration::from_secs(20);
        let deadline = Instant::now() + NATIVE_TURN_BIND_WAIT;
        while !native_turn_is_bound(
            service,
            child_conversation_id,
            child_membership_id,
            dispatch_id,
            expected_turn_id,
        ) {
            assert!(
                Instant::now() < deadline,
                "waited {NATIVE_TURN_BIND_WAIT:?} for admitted native turn {expected_turn_id} to \
                 bind its live control channel (no dispatch.turn.bound frame persisted for \
                 dispatch {dispatch_id})"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn native_turn_is_bound(
        service: &ConversationService,
        child_conversation_id: &str,
        child_membership_id: &str,
        dispatch_id: &str,
        expected_turn_id: &str,
    ) -> bool {
        let Ok(Some(turn_event)) = service
            .store()
            .agent_turn_event_for_dispatch(child_conversation_id, dispatch_id)
        else {
            return false;
        };
        let scope = licoup_native::domain::client_conversation::ConversationRuntimeScope {
            dispatch_id: dispatch_id.to_owned(),
            conversation_id: child_conversation_id.to_owned(),
            membership_id: child_membership_id.to_owned(),
            event_id: turn_event.id,
        };
        service
            .store()
            .runtime_frames_after(&scope, 0, i64::MAX as u64, 512)
            .map(|frames| {
                frames.iter().any(|frame| {
                    frame.get("event").and_then(Value::as_str) == Some("dispatch.turn.bound")
                        && frame.get("turnId").and_then(Value::as_str) == Some(expected_turn_id)
                })
            })
            .unwrap_or(false)
    }

    fn with_test_executable(params: &Value, executable: &std::path::Path) -> Value {
        let mut value = params.clone();
        if let Some(object) = value.as_object_mut() {
            let path = json!(executable.to_string_lossy());
            object.insert("executable".to_owned(), path.clone());
            object.insert("binary".to_owned(), path);
        }
        value
    }

    fn visible_text(store: &ConversationStore, conversation_id: &str, event_id: &str) -> String {
        store
            .event(conversation_id, event_id)
            .unwrap()
            .unwrap()
            .parts
            .iter()
            .filter(|part| part.kind == EventPartKind::Text)
            .map(|part| part.content.as_str())
            .collect()
    }

    fn question_proposal_json(conversation_id: &str) -> String {
        serde_json::to_string(&ContinuityInterpretationProposal {
            envelope: ContinuityWriteEnvelope {
                conversation_id: conversation_id.to_owned(),
                source_event_refs: Vec::new(),
                observed_revision: 0,
                designation_epoch: 0,
                request_id: "request:question".into(),
            },
            matter_associations: Vec::new(),
            speech_act: ContinuitySpeechAct::Question,
            commitment_proposals: Vec::new(),
            agreement_proposals: Vec::new(),
            capability_needs: Vec::new(),
            uncertainty_reasons: Vec::new(),
            requested_reads: Vec::new(),
            task_child_admission: None,
        })
        .unwrap()
    }

    fn assistant_envelope_json(reply: &str, proposal_json: &str) -> String {
        let proposal: ContinuityInterpretationProposal =
            serde_json::from_str(proposal_json).unwrap();
        serde_json::to_string(&ContinuityAssistantTurnResponse {
            reply_text: reply.to_owned(),
            interpretation_proposal: proposal,
        })
        .unwrap()
    }

    fn create_designated_group(
        service: &ConversationService,
        title: &str,
    ) -> (String, String, String) {
        let group = service
            .execute(json!({
                "action": "conversation.create",
                "title": title,
                "owner": {"id": "human:local", "kind": "human", "displayName": "You"},
                "members": [{
                    "principal": {
                        "id": "agent:codex",
                        "kind": "agent",
                        "displayName": "Codex",
                        "agentId": "codex"
                    },
                    "access": "member"
                }]
            }))
            .unwrap();
        let conversation_id = group["id"].as_str().unwrap().to_owned();
        let owner = group["memberships"]
            .as_array()
            .unwrap()
            .iter()
            .find(|membership| membership["principal"]["kind"] == "human")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let agent = group["memberships"]
            .as_array()
            .unwrap()
            .iter()
            .find(|membership| membership["principal"]["kind"] == "agent")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let revision = service.store().get(&conversation_id).unwrap().revision;
        service
            .execute(json!({
                "action": "conversation.assistant.set",
                "conversationId": conversation_id,
                "ownerMembershipId": owner,
                "expectedRevision": revision,
                "membershipId": agent,
            }))
            .unwrap();
        (conversation_id, owner, agent)
    }

    fn bind_fake_codex_parent_runtime(
        store: ConversationStore,
        executable: std::path::PathBuf,
        start_kinds: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) -> (
        PersistentConversationRuntime,
        ConversationService,
        std::sync::mpsc::Receiver<(String, Value, Result<Value, String>)>,
    ) {
        let runtime = PersistentConversationRuntime::new(store.clone());
        let (tx, rx) = std::sync::mpsc::channel::<(String, Value, Result<Value, String>)>();
        let send_runtime = runtime.clone();
        let send_executable = executable;
        let start_kinds_for_send = start_kinds;
        let service = ConversationService::from_store(store).bind_conversation_runtime(
            PersistentRuntimePorts::new(
                move |params: &Value| {
                    let kind = params
                        .get("continuityKind")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    start_kinds_for_send
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .push(kind.clone());
                    let params = with_test_executable(params, &send_executable);
                    if kind == "child-work" {
                        Ok(json!({
                            "ok": true,
                            "accepted": true,
                            "turnHandle": "turn:child-stub",
                        }))
                    } else {
                        send_runtime.start_admitted_background(&params, None)
                    }
                },
                |_conversation_id: &str| json!([]),
                |_params: &Value| Ok(json!({ "ok": true })),
                |_params: &Value| Ok(json!({ "ok": true, "output": "" })),
                |_request: Value| Ok(json!({})),
            ),
        );
        let hooked = service.clone();
        runtime.set_settlement_hook(move |conversation_id, payload| {
            let result = hooked
                .after_runtime_settlement(conversation_id, payload)
                .map_err(|err| err.to_string());
            let _ = tx.send((conversation_id.to_owned(), payload.clone(), result.clone()));
            result.map(|_| ())
        });
        (runtime, service, rx)
    }

    fn typed_child_proposal_json(conversation_id: &str) -> String {
        serde_json::to_string(&ContinuityInterpretationProposal {
            envelope: ContinuityWriteEnvelope {
                conversation_id: conversation_id.to_owned(),
                source_event_refs: Vec::new(),
                observed_revision: 0,
                designation_epoch: 0,
                request_id: "request:goal:matter:notes".into(),
            },
            matter_associations: Vec::new(),
            speech_act: ContinuitySpeechAct::Delegation,
            commitment_proposals: vec![ContinuityCommitmentProposal {
                matter_id: Some("matter:notes".into()),
                subject: ContinuityMatterSubject::New,
                expected_result: "Prepare notes".into(),
                criteria: Vec::new(),
                create_goal: true,
            }],
            agreement_proposals: Vec::new(),
            capability_needs: Vec::new(),
            uncertainty_reasons: Vec::new(),
            requested_reads: Vec::new(),
            task_child_admission: Some(ContinuityTaskChildAdmission {
                goal_id: "goal:matter:notes".into(),
                parent_conversation_id: conversation_id.to_owned(),
                speech_act: ContinuitySpeechAct::Delegation,
                follow_through_kind: ContinuityFollowThroughKind::Durable,
                observed_child_conversation_id: None,
                observed_card_anchor: None,
                request_id: "request:admit:goal:matter:notes".into(),
            }),
        })
        .unwrap()
    }

    #[test]
    fn persistent_turn_child_work_finishes_through_fake_lowest_codex() {
        let _guard = FAKE_CODEX_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let executable = compile_fake_codex();
        let mut result_path = executable.clone();
        result_path.set_extension("result.json");
        std::fs::write(&result_path, "VERTICAL-CHILD-RECEIPT").unwrap();

        let store = ConversationStore::open_in_memory().unwrap();
        let runtime = PersistentConversationRuntime::new(store.clone());
        let (tx, rx) = std::sync::mpsc::channel::<(String, Value)>();
        let service = ConversationService::from_store(store);
        let send_runtime = runtime.clone();
        let send_executable = executable.clone();
        let service = service.bind_conversation_runtime(PersistentRuntimePorts::new(
            move |params: &Value| {
                let params = with_test_executable(params, &send_executable);
                if params.get("continuityKind").and_then(Value::as_str) == Some("child-work") {
                    send_runtime.start_admitted_background(&params, None)
                } else {
                    Ok(json!({
                        "ok": true,
                        "accepted": true,
                        "turnHandle": "turn:parent",
                    }))
                }
            },
            |_conversation_id: &str| json!([]),
            |_params: &Value| Ok(json!({ "ok": true })),
            move |params: &Value| {
                let conversation_id = params
                    .get("conversationId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                Ok(json!({
                    "ok": true,
                    "output": typed_child_proposal_json(conversation_id),
                }))
            },
            |_request: Value| Ok(json!({})),
        ));
        let hooked = service.clone();
        runtime.set_settlement_hook(move |conversation_id, payload| {
            let result = hooked
                .after_runtime_settlement(conversation_id, payload)
                .map(|_| ())
                .map_err(|err| err.to_string());
            let _ = tx.send((conversation_id.to_owned(), payload.clone()));
            result
        });

        let group = service
            .execute(json!({
                "action": "conversation.create",
                "title": "Vertical parent",
                "owner": {"id": "human:local", "kind": "human", "displayName": "You"},
                "members": [{
                    "principal": {
                        "id": "agent:codex",
                        "kind": "agent",
                        "displayName": "Codex",
                        "agentId": "codex"
                    },
                    "access": "member"
                }]
            }))
            .unwrap();
        let conversation_id = group["id"].as_str().unwrap().to_owned();
        let owner = group["memberships"]
            .as_array()
            .unwrap()
            .iter()
            .find(|membership| membership["principal"]["kind"] == "human")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let agent = group["memberships"]
            .as_array()
            .unwrap()
            .iter()
            .find(|membership| membership["principal"]["kind"] == "agent")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let revision = service.store().get(&conversation_id).unwrap().revision;
        service
            .execute(json!({
                "action": "conversation.assistant.set",
                "conversationId": conversation_id,
                "ownerMembershipId": owner,
                "expectedRevision": revision,
                "membershipId": agent,
            }))
            .unwrap();
        let posted = service
            .execute(json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner,
                "content": "prepare notes for the trip",
            }))
            .unwrap();
        let event_id = posted["event"]["id"].as_str().unwrap().to_owned();
        service
            .execute(json!({
                "action": "conversation.dispatch.after-post",
                "conversationId": conversation_id,
                "eventId": event_id,
            }))
            .unwrap();
        service
            .after_runtime_settlement(
                &conversation_id,
                &json!({
                    "output": assistant_envelope_json(
                        "I'll prepare the notes in a child conversation.",
                        &typed_child_proposal_json(&conversation_id),
                    ),
                    "membershipId": agent,
                    "causationId": event_id,
                    "dispatchId": "dispatch:parent-vertical",
                }),
            )
            .unwrap();
        let relation = service
            .store()
            .list_child_relations(&conversation_id, None, 8)
            .unwrap()
            .remove(0);
        let child_id = relation.child_conversation_id.clone();
        let child_member = service
            .store()
            .get(&child_id)
            .unwrap()
            .assistant_membership_id
            .expect("child assistant");
        let (settled_conversation, settled_payload) = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("fake Codex finish must invoke the settlement hook");
        assert_eq!(settled_conversation, child_id);
        let dispatch_id = settled_payload
            .get("dispatchId")
            .and_then(Value::as_str)
            .expect("finish must stamp dispatchId");
        assert!(settlement_applied(service.store(), &child_id, dispatch_id).unwrap());
        let event = service
            .store()
            .agent_turn_event_for_dispatch(&child_id, dispatch_id)
            .unwrap()
            .expect("canonical child turn event");
        assert_eq!(
            event.author_membership_id.as_deref(),
            Some(child_member.as_str())
        );
        assert_eq!(event.correlation_id.as_deref(), Some(dispatch_id));
        let duplicates = service
            .store()
            .page_events(&child_id, None, 50)
            .unwrap()
            .events
            .iter()
            .filter(|item| item.correlation_id.as_deref() == Some(dispatch_id))
            .count();
        assert_eq!(duplicates, 1, "finish must reuse the admitted turn Event");
        let _ = std::fs::remove_file(result_path);
        let _ = std::fs::remove_dir_all(executable.parent().unwrap());
    }

    #[test]
    fn persistent_turn_child_work_delivers_granted_source_to_fake_lowest_codex() {
        let _guard = FAKE_CODEX_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let executable = compile_fake_codex();
        let mut result_path = executable.clone();
        result_path.set_extension("result.json");
        std::fs::write(&result_path, "VERTICAL-CHILD-RECEIPT").unwrap();

        let store = ConversationStore::open_in_memory().unwrap();
        let runtime = PersistentConversationRuntime::new(store.clone());
        let (tx, rx) = std::sync::mpsc::channel::<(String, Value)>();
        let service = ConversationService::from_store(store);
        let send_runtime = runtime.clone();
        let send_executable = executable.clone();
        let started_kinds = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let started_kinds_for_send = started_kinds.clone();
        let service = service.bind_conversation_runtime(PersistentRuntimePorts::new(
            move |params: &Value| {
                let kind = params
                    .get("continuityKind")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                started_kinds_for_send
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .push(kind.clone());
                let params = with_test_executable(params, &send_executable);
                if kind == "child-work" {
                    send_runtime.start_admitted_background(&params, None)
                } else {
                    Ok(json!({
                        "ok": true,
                        "accepted": true,
                        "turnHandle": "turn:parent",
                    }))
                }
            },
            |_conversation_id: &str| json!([]),
            |_params: &Value| Ok(json!({ "ok": true })),
            move |params: &Value| {
                let conversation_id = params
                    .get("conversationId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                Ok(json!({
                    "ok": true,
                    "output": typed_child_proposal_json(conversation_id),
                }))
            },
            |_request: Value| Ok(json!({})),
        ));
        let hooked = service.clone();
        runtime.set_settlement_hook(move |conversation_id, payload| {
            let result = hooked
                .after_runtime_settlement(conversation_id, payload)
                .map(|_| ())
                .map_err(|err| err.to_string());
            let _ = tx.send((conversation_id.to_owned(), payload.clone()));
            result
        });

        let (conversation_id, owner, agent) = create_designated_group(&service, "Vertical grants");
        let secret = service
            .execute(json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner,
                "content": "OUT-OF-SCOPE-SECRET chitchat only",
            }))
            .unwrap();
        let secret_id = secret["event"]["id"].as_str().unwrap().to_owned();
        service
            .execute(json!({
                "action": "conversation.dispatch.after-post",
                "conversationId": conversation_id,
                "eventId": secret_id,
            }))
            .unwrap();
        service
            .after_runtime_settlement(
                &conversation_id,
                &json!({
                    "output": assistant_envelope_json(
                        "Ordinary chitchat.",
                        &question_proposal_json(&conversation_id),
                    ),
                    "membershipId": agent,
                    "causationId": secret_id,
                    "dispatchId": "dispatch:parent-chitchat",
                }),
            )
            .unwrap();

        let posted = service
            .execute(json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner,
                "content": "PRODUCTION-GRANT-SENTINEL prepare notes",
            }))
            .unwrap();
        let event_id = posted["event"]["id"].as_str().unwrap().to_owned();
        service
            .execute(json!({
                "action": "conversation.dispatch.after-post",
                "conversationId": conversation_id,
                "eventId": event_id,
            }))
            .unwrap();
        service
            .after_runtime_settlement(
                &conversation_id,
                &json!({
                    "output": assistant_envelope_json(
                        "I'll prepare the notes in a child conversation.",
                        &typed_child_proposal_json(&conversation_id),
                    ),
                    "membershipId": agent,
                    "causationId": event_id,
                    "dispatchId": "dispatch:parent-vertical-grant",
                }),
            )
            .unwrap();
        let relation = service
            .store()
            .list_child_relations(&conversation_id, None, 8)
            .unwrap()
            .remove(0);
        let child_id = relation.child_conversation_id.clone();
        let child_member = service
            .store()
            .get(&child_id)
            .unwrap()
            .assistant_membership_id
            .expect("child assistant");
        let grants = list_all_parent_grants(service.store()).unwrap();
        assert!(
            grants.iter().any(|grant| {
                grant.recipient_conversation_id == child_id
                    && grant.recipient_membership_id == child_member
                    && grant
                        .source_refs
                        .iter()
                        .any(|source| source.opaque_id == event_id)
            }),
            "admitted child grant must bind the posted sentinel event"
        );
        assert!(
            !grants.iter().any(|grant| {
                grant
                    .source_refs
                    .iter()
                    .any(|source| source.opaque_id == secret_id)
            }),
            "chitchat without admission must not receive a grant"
        );
        let (settled_conversation, settled_payload) = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("fake Codex finish must invoke the settlement hook");
        assert_eq!(settled_conversation, child_id);
        let dispatch_id = settled_payload
            .get("dispatchId")
            .and_then(Value::as_str)
            .expect("finish must stamp dispatchId");
        assert!(
            started_kinds
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .iter()
                .any(|kind| kind == "child-work"),
            "child-work must reach PersistentTurn start_admitted_background"
        );
        assert!(settlement_applied(service.store(), &child_id, dispatch_id).unwrap());
        let event = service
            .store()
            .agent_turn_event_for_dispatch(&child_id, dispatch_id)
            .unwrap()
            .expect("canonical child turn event");
        assert_eq!(
            event.author_membership_id.as_deref(),
            Some(child_member.as_str())
        );
        assert_eq!(event.correlation_id.as_deref(), Some(dispatch_id));
        let (session_id, turn_id) = runtime
            .inspect_turn(dispatch_id)
            .expect("PersistentTurn remains inspectable after finish");
        assert!(
            !session_id.is_empty() || !turn_id.is_empty(),
            "native session or turn identity must be non-empty"
        );
        let frames = service
            .store()
            .runtime_frames_after(
                &licoup_native::domain::client_conversation::ConversationRuntimeScope {
                    dispatch_id: dispatch_id.to_owned(),
                    conversation_id: child_id.clone(),
                    membership_id: child_member.clone(),
                    event_id: event.id.clone(),
                },
                0,
                i64::MAX as u64,
                64,
            )
            .unwrap();
        assert!(
            !frames.is_empty(),
            "PersistentTurn must persist a non-empty native frame"
        );
        let mut seen = executable.clone();
        seen.set_extension("turn-start.seen");
        let seen = std::fs::read_to_string(&seen).unwrap_or_default();
        assert_eq!(
            seen.trim(),
            "1",
            "lowest fake process must observe the granted sentinel"
        );
        let mut leak = executable.clone();
        leak.set_extension("leak.seen");
        assert!(
            !leak.exists(),
            "lowest fake process must not observe out-of-scope material"
        );
        let _ = std::fs::remove_file(result_path);
        let _ = std::fs::remove_dir_all(executable.parent().unwrap());
    }

    #[test]
    fn persistent_turn_child_work_steers_live_codex_turn_and_rejects_stale_handles() {
        static ENVIRONMENT_PORTS: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        ENVIRONMENT_PORTS.get_or_init(|| {
            let _ = licoup_native::install_environment_ports();
        });
        let _guard = FAKE_CODEX_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let executable = compile_fake_codex();
        let mut steer_mode = executable.clone();
        steer_mode.set_extension("steer-mode");
        std::fs::write(&steer_mode, "1").unwrap();
        let mut cancel_mode = executable.clone();
        cancel_mode.set_extension("cancel-mode");
        std::fs::write(&cancel_mode, "1").unwrap();
        let mut interrupt_path = executable.clone();
        interrupt_path.set_extension("interrupt.json");
        let _ = std::fs::remove_file(&interrupt_path);
        let thread_id = format!("fake-cancel-thread-{}", std::process::id());
        let turn_id = format!("fake-cancel-turn-{}", std::process::id());
        let mut identity_path = executable.clone();
        identity_path.set_extension("identity");
        std::fs::write(&identity_path, format!("{thread_id}\n{turn_id}\n")).unwrap();

        let store = ConversationStore::open_in_memory().unwrap();
        let runtime = PersistentConversationRuntime::new(store.clone());
        let start_count = std::sync::Arc::new(AtomicUsize::new(0));
        let service = ConversationService::from_store(store.clone());
        let send_runtime = runtime.clone();
        let active_runtime = runtime.clone();
        let steer_runtime = runtime.clone();
        let cancel_runtime = runtime.clone();
        let inspect_runtime = runtime.clone();
        let start_count_for_send = start_count.clone();
        let send_executable = executable.clone();
        let steer_executable = executable.clone();
        let cancel_executable = executable.clone();
        let service = service.bind_conversation_runtime(
            PersistentRuntimePorts::new(
                move |params: &Value| {
                    let params = with_test_executable(params, &send_executable);
                    if params.get("continuityKind").and_then(Value::as_str) == Some("child-work") {
                        start_count_for_send.fetch_add(1, Ordering::SeqCst);
                        send_runtime.start_admitted_background(&params, None)
                    } else {
                        Ok(json!({
                            "ok": true,
                            "accepted": true,
                            "turnHandle": "turn:parent",
                        }))
                    }
                },
                move |conversation_id: &str| {
                    active_runtime.active(&json!({ "conversationId": conversation_id }))
                },
                move |params: &Value| {
                    steer_runtime.steer_sync(&with_test_executable(params, &steer_executable))
                },
                move |params: &Value| {
                    let conversation_id = params
                        .get("conversationId")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    Ok(json!({
                        "ok": true,
                        "output": typed_child_proposal_json_with_result(
                            conversation_id,
                            "fake-codex-steer-prompt",
                        ),
                    }))
                },
                |_request: Value| Ok(json!({})),
            )
            .with_cancel(move |params: &Value| {
                cancel_runtime
                    .request_cancel(&with_test_executable(params, &cancel_executable))
                    .map_err(|_| RuntimeAdapterError::ConversationDispatchFailed)
            }),
        );
        if let Some(host) = service.continuity().cloned() {
            let observer_host = host.clone();
            runtime.set_live_turn_observer(
                move |conversation_id, membership_id, dispatch_id, native| {
                    observer_host.update_live_native_turn(
                        conversation_id,
                        membership_id,
                        dispatch_id,
                        native,
                    );
                },
            );
            host.bind_work_turn_inspect(std::sync::Arc::new(move |handle| {
                inspect_runtime.inspect_turn(handle)
            }));
        }

        let group = service
            .execute(json!({
                "action": "conversation.create",
                "title": "Vertical steer parent",
                "owner": {"id": "human:local", "kind": "human", "displayName": "You"},
                "members": [{
                    "principal": {
                        "id": "agent:codex",
                        "kind": "agent",
                        "displayName": "Codex",
                        "agentId": "codex"
                    },
                    "access": "member"
                }]
            }))
            .unwrap();
        let conversation_id = group["id"].as_str().unwrap().to_owned();
        let owner = group["memberships"]
            .as_array()
            .unwrap()
            .iter()
            .find(|membership| membership["principal"]["kind"] == "human")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let agent = group["memberships"]
            .as_array()
            .unwrap()
            .iter()
            .find(|membership| membership["principal"]["kind"] == "agent")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let revision = service.store().get(&conversation_id).unwrap().revision;
        service
            .execute(json!({
                "action": "conversation.assistant.set",
                "conversationId": conversation_id,
                "ownerMembershipId": owner,
                "expectedRevision": revision,
                "membershipId": agent,
            }))
            .unwrap();
        let posted = service
            .execute(json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner,
                "content": "prepare notes for the trip",
            }))
            .unwrap();
        let event_id = posted["event"]["id"].as_str().unwrap().to_owned();
        service
            .execute(json!({
                "action": "conversation.dispatch.after-post",
                "conversationId": conversation_id,
                "eventId": event_id,
            }))
            .unwrap();
        service
            .after_runtime_settlement(
                &conversation_id,
                &json!({
                    "output": assistant_envelope_json(
                        "I'll prepare the notes in a child conversation.",
                        &typed_child_proposal_json_with_result(
                            &conversation_id,
                            "fake-codex-steer-prompt",
                        ),
                    ),
                    "membershipId": agent,
                    "causationId": event_id,
                    "dispatchId": "dispatch:parent-vertical-steer",
                }),
            )
            .unwrap();
        let relation = service
            .store()
            .list_child_relations(&conversation_id, None, 8)
            .unwrap()
            .remove(0);
        let child_id = relation.child_conversation_id.clone();
        let child_member = service
            .store()
            .get(&child_id)
            .unwrap()
            .assistant_membership_id
            .expect("child assistant");
        service.claim_continuity_owner().unwrap();
        let _ = service.attend_due().unwrap();
        let accepted = licoup_conversation::continuity::read_child_work_accepted(
            service.store(),
            &conversation_id,
            &relation.goal_id,
            1,
        )
        .unwrap()
        .expect("accepted child work");
        let dispatch_id = accepted
            .get("dispatchId")
            .and_then(Value::as_str)
            .expect("accepted dispatch")
            .to_owned();
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if runtime
                .inspect_turn(&dispatch_id)
                .is_some_and(|(_, native_turn_id)| native_turn_id == turn_id)
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            runtime
                .inspect_turn(&dispatch_id)
                .map(|(_, native_turn_id)| native_turn_id),
            Some(turn_id.clone()),
            "live native turn must bind before control"
        );
        // The runtime reports the native turn id as soon as the app-server
        // acknowledges `turn/start`, but the lane only registers its live
        // in-flight control channel later. Wait for the bound frame so the
        // reopened host can actually steer the admitted turn.
        wait_for_bound_native_turn(&service, &child_id, &child_member, &dispatch_id, &turn_id);
        assert_eq!(start_count.load(Ordering::SeqCst), 1);
        let send_runtime = runtime.clone();
        let active_runtime = runtime.clone();
        let steer_runtime = runtime.clone();
        let cancel_runtime = runtime.clone();
        let inspect_runtime = runtime.clone();
        let start_count_for_reopen = start_count.clone();
        let service = ConversationService::from_store(store).bind_conversation_runtime(
            PersistentRuntimePorts::new(
                move |params: &Value| {
                    if params.get("continuityKind").and_then(Value::as_str) == Some("child-work") {
                        start_count_for_reopen.fetch_add(1, Ordering::SeqCst);
                        send_runtime.start_admitted_background(params, None)
                    } else {
                        Ok(json!({
                            "ok": true,
                            "accepted": true,
                            "turnHandle": "turn:parent",
                        }))
                    }
                },
                move |conversation_id: &str| {
                    active_runtime.active(&json!({ "conversationId": conversation_id }))
                },
                move |params: &Value| steer_runtime.steer_sync(params),
                move |params: &Value| {
                    let conversation_id = params
                        .get("conversationId")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    Ok(json!({
                        "ok": true,
                        "output": typed_child_proposal_json_with_result(
                            conversation_id,
                            "fake-codex-steer-prompt",
                        ),
                    }))
                },
                |_request: Value| Ok(json!({})),
            )
            .with_cancel(move |params: &Value| {
                cancel_runtime
                    .request_cancel(params)
                    .map_err(|_| RuntimeAdapterError::ConversationDispatchFailed)
            }),
        );
        if let Some(host) = service.continuity().cloned() {
            let observer_host = host.clone();
            runtime.set_live_turn_observer(
                move |conversation_id, membership_id, dispatch_id, native| {
                    observer_host.update_live_native_turn(
                        conversation_id,
                        membership_id,
                        dispatch_id,
                        native,
                    );
                },
            );
            host.bind_work_turn_inspect(std::sync::Arc::new(move |handle| {
                inspect_runtime.inspect_turn(handle)
            }));
        }
        service.claim_continuity_owner().unwrap();
        let _ = service.attend_due().unwrap();
        assert_eq!(
            start_count.load(Ordering::SeqCst),
            1,
            "reopened host must restore the started PersistentTurn without a second start"
        );
        let host = service.continuity().cloned().unwrap();
        assert_eq!(
            host.steer_admitted_child_follow_up(
                &child_id,
                &child_member,
                "turn:stale-other",
                "fake-codex-steer-guidance",
            ),
            licoup_native::domain::assistant_continuity::ChildControlDisposition::Conflict
        );
        assert_eq!(
            host.cancel_admitted_child_turn(&conversation_id, &agent, &dispatch_id),
            licoup_native::domain::assistant_continuity::ChildControlDisposition::Ordinary
        );
        assert!(
            !interrupt_path.exists(),
            "wrong-scope control must not interrupt the live native turn"
        );
        assert_eq!(
            host.steer_admitted_child_follow_up(
                &child_id,
                &child_member,
                &dispatch_id,
                "fake-codex-steer-guidance",
            ),
            licoup_native::domain::assistant_continuity::ChildControlDisposition::Accepted,
            "reopened live PersistentTurn must accept the admitted steer"
        );
        let cancel = host.cancel_admitted_child_turn(&child_id, &child_member, &dispatch_id);
        assert_eq!(
            cancel,
            licoup_native::domain::assistant_continuity::ChildControlDisposition::Accepted,
            "live Codex cancel must be Accepted on the admitted PersistentTurn owner"
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        let interrupt = loop {
            if let Ok(contents) = std::fs::read_to_string(&interrupt_path)
                && let Ok(interrupt) = serde_json::from_str::<Value>(&contents)
            {
                break interrupt;
            }
            assert!(
                Instant::now() < deadline,
                "native interrupt receipt must become a complete JSON document"
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(interrupt["method"], "turn/interrupt");
        assert_eq!(interrupt["threadId"], thread_id);
        assert_eq!(interrupt["turnId"], turn_id);
        let _ = std::fs::remove_file(steer_mode);
        let _ = std::fs::remove_file(cancel_mode);
        let _ = std::fs::remove_file(identity_path);
        let _ = std::fs::remove_file(interrupt_path);
        let _ = std::fs::remove_dir_all(executable.parent().unwrap());
    }

    #[test]
    fn ordinary_question_produces_readable_reply_through_fake_codex() {
        let _guard = FAKE_CODEX_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let executable = compile_fake_codex();
        let store_root = executable.parent().unwrap().join("ordinary-store");
        std::fs::create_dir_all(&store_root).unwrap();
        let reply = "标准正态分布的均值为 0，方差为 1。";
        let store = ConversationStore::open(&store_root).unwrap();
        let start_kinds = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (runtime, service, rx) =
            bind_fake_codex_parent_runtime(store, executable.clone(), start_kinds.clone());
        let (conversation_id, owner, agent) =
            create_designated_group(&service, "Ordinary question");
        let envelope = assistant_envelope_json(reply, &question_proposal_json(&conversation_id));
        let mut result_path = executable.clone();
        result_path.set_extension("result.json");
        std::fs::write(&result_path, &envelope).unwrap();
        let posted = service
            .execute(json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner,
                "content": "解释一下正态分布。",
            }))
            .unwrap();
        service
            .execute(json!({
                "action": "conversation.dispatch.after-post",
                "conversationId": conversation_id,
                "eventId": posted["event"]["id"],
            }))
            .unwrap();
        let (settled_conversation, settled_payload, _settlement) = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("ordinary fake Codex finish must settle");
        assert_eq!(settled_conversation, conversation_id);
        assert_ne!(
            settled_payload.get("ok").and_then(Value::as_bool),
            Some(false)
        );
        let dispatch_id = settled_payload["dispatchId"].as_str().expect("dispatchId");
        let event = service
            .store()
            .agent_turn_event_for_dispatch(&conversation_id, dispatch_id)
            .unwrap()
            .expect("parent turn event");
        assert_eq!(event.author_membership_id.as_deref(), Some(agent.as_str()));
        assert_eq!(
            visible_text(service.store(), &conversation_id, &event.id),
            reply
        );
        assert_eq!(
            service
                .store()
                .runtime_response_mode(
                    &licoup_native::domain::client_conversation::ConversationRuntimeScope {
                        dispatch_id: dispatch_id.to_owned(),
                        conversation_id: conversation_id.clone(),
                        membership_id: event.author_membership_id.clone().unwrap_or_default(),
                        event_id: event.id.clone(),
                    }
                )
                .unwrap()
                .as_deref(),
            Some(TRUSTED_RESPONSE_MODE_ASSISTANT_TURN),
            "authorized after-post must record the internal admitted response mode"
        );
        assert!(!visible_text(service.store(), &conversation_id, &event.id).contains("speechAct"));
        assert!(
            service
                .store()
                .list_child_relations(&conversation_id, None, 8)
                .unwrap()
                .is_empty()
        );
        let kinds = start_kinds
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        assert_eq!(kinds, vec!["user-posted".to_owned()]);
        let handle = dispatch_id.to_owned();
        for text in runtime.live_message_texts(&handle) {
            assert!(
                !text.contains("interpretationProposal") && !text.contains("speechAct"),
                "live payload must not publish the private envelope: {text}"
            );
        }
        let _ = runtime;
        let _ = std::fs::remove_file(result_path);
        let _ = std::fs::remove_dir_all(executable.parent().unwrap());
    }

    #[test]
    fn durable_delegation_applies_private_proposal_once_through_fake_codex() {
        let _guard = FAKE_CODEX_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let executable = compile_fake_codex();
        let store_root = executable.parent().unwrap().join("delegation-store");
        std::fs::create_dir_all(&store_root).unwrap();
        let reply = "我会在子对话里准备讲义，先把资料边界说清楚。";
        let store = ConversationStore::open(&store_root).unwrap();
        let start_kinds = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (runtime, service, rx) =
            bind_fake_codex_parent_runtime(store, executable.clone(), start_kinds.clone());
        let (conversation_id, owner, agent) =
            create_designated_group(&service, "Durable delegation");
        let envelope = assistant_envelope_json(reply, &typed_child_proposal_json(&conversation_id));
        let mut result_path = executable.clone();
        result_path.set_extension("result.json");
        std::fs::write(&result_path, &envelope).unwrap();
        let posted = service
            .execute(json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner,
                "content": "帮我筹备团队分享。",
            }))
            .unwrap();
        let posted_id = posted["event"]["id"]
            .as_str()
            .expect("posted event")
            .to_owned();
        service
            .execute(json!({
                "action": "conversation.dispatch.after-post",
                "conversationId": conversation_id,
                "eventId": posted["event"]["id"],
            }))
            .unwrap();
        let (settled_conversation, settled_payload, settlement) = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("delegation fake Codex finish must settle");
        assert_eq!(settled_conversation, conversation_id);
        assert_eq!(
            settled_payload.get("causationId").and_then(Value::as_str),
            Some(posted_id.as_str()),
            "settlement must keep the user-posted source event, not the agent turn Event"
        );
        let settlement =
            settlement.unwrap_or_else(|err| panic!("parent proposal settlement must apply: {err}"));
        assert!(
            settlement.is_object(),
            "parent proposal settlement must return a structured drain: {settlement}"
        );
        let dispatch_id = settled_payload["dispatchId"].as_str().expect("dispatchId");
        let event = service
            .store()
            .agent_turn_event_for_dispatch(&conversation_id, dispatch_id)
            .unwrap()
            .expect("parent turn event");
        assert_eq!(event.author_membership_id.as_deref(), Some(agent.as_str()));
        assert_eq!(
            visible_text(service.store(), &conversation_id, &event.id),
            reply
        );
        let relations = service
            .store()
            .list_child_relations(&conversation_id, None, 8)
            .unwrap();
        assert_eq!(relations.len(), 1, "exactly one child card");
        let kinds = start_kinds
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        assert_eq!(
            kinds.iter().filter(|kind| *kind == "user-posted").count(),
            1
        );
        let _ = runtime;
        let _ = std::fs::remove_file(result_path);
        let _ = std::fs::remove_dir_all(executable.parent().unwrap());
    }

    #[test]
    fn chunked_envelope_never_publishes_private_text_and_reopens() {
        let _guard = FAKE_CODEX_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let executable = compile_fake_codex();
        let mut chunk_mode = executable.clone();
        chunk_mode.set_extension("chunk-mode");
        std::fs::write(&chunk_mode, "1").unwrap();
        let store_root = executable.parent().unwrap().join("chunk-store");
        std::fs::create_dir_all(&store_root).unwrap();
        let reply = "He said \"use {\\\"ok\\\":true}\" then 均值 0.";
        let store = ConversationStore::open(&store_root).unwrap();
        let start_kinds = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (runtime, service, rx) =
            bind_fake_codex_parent_runtime(store, executable.clone(), start_kinds);
        let (conversation_id, owner, _agent) =
            create_designated_group(&service, "Chunked envelope");
        let envelope = assistant_envelope_json(reply, &question_proposal_json(&conversation_id));
        let mut result_path = executable.clone();
        result_path.set_extension("result.json");
        std::fs::write(&result_path, &envelope).unwrap();
        let posted = service
            .execute(json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner,
                "content": "stream the envelope",
            }))
            .unwrap();
        service
            .execute(json!({
                "action": "conversation.dispatch.after-post",
                "conversationId": conversation_id,
                "eventId": posted["event"]["id"],
            }))
            .unwrap();
        let (_settled_conversation, settled_payload, _settlement) = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("chunked fake Codex finish must settle");
        let dispatch_id = settled_payload["dispatchId"].as_str().expect("dispatchId");
        let event = service
            .store()
            .agent_turn_event_for_dispatch(&conversation_id, dispatch_id)
            .unwrap()
            .expect("parent turn event");
        assert_eq!(
            visible_text(service.store(), &conversation_id, &event.id),
            reply
        );
        for text in runtime.live_message_texts(dispatch_id) {
            assert!(
                !text.contains("interpretationProposal"),
                "streamed live text leaked private envelope: {text}"
            );
            assert!(text.is_empty() || text == reply);
        }
        runtime.evict_turn_cache(dispatch_id);
        let frames = service
            .store()
            .runtime_frames_after(
                &licoup_native::domain::client_conversation::ConversationRuntimeScope {
                    dispatch_id: dispatch_id.to_owned(),
                    conversation_id: conversation_id.clone(),
                    membership_id: event.author_membership_id.clone().unwrap_or_default(),
                    event_id: event.id.clone(),
                },
                0,
                i64::MAX as u64,
                64,
            )
            .unwrap();
        for frame in frames {
            let redacted = licoup_conversation::continuity::redact_live_runtime_event(&frame);
            if let Some(text) = redacted.pointer("/payload/text").and_then(Value::as_str) {
                assert!(
                    !text.contains("interpretationProposal"),
                    "store-fallback replay must redact private envelope"
                );
            }
        }
        drop(service);
        drop(runtime);
        let reopened = ConversationStore::open(&store_root).unwrap();
        assert_eq!(visible_text(&reopened, &conversation_id, &event.id), reply);
        assert!(!visible_text(&reopened, &conversation_id, &event.id).contains("speechAct"));
        let _ = std::fs::remove_file(result_path);
        let _ = std::fs::remove_file(chunk_mode);
        let _ = std::fs::remove_dir_all(executable.parent().unwrap());
    }

    #[test]
    fn unadmitted_fake_codex_preserves_proposal_looking_json() {
        let _guard = FAKE_CODEX_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let executable = compile_fake_codex();
        let exact = r#"{"speechAct":"question","envelope":{"conversationId":"conversation:one"},"commitmentProposals":[]} trailing prose stays."#;
        let mut result_path = executable.clone();
        result_path.set_extension("result.json");
        std::fs::write(&result_path, exact).unwrap();
        let store = ConversationStore::open_in_memory().unwrap();
        let runtime = PersistentConversationRuntime::new(store.clone());
        let (tx, rx) = std::sync::mpsc::channel::<(String, Value)>();
        let send_runtime = runtime.clone();
        let send_executable = executable.clone();
        let service = ConversationService::from_store(store).bind_conversation_runtime(
            PersistentRuntimePorts::new(
                move |params: &Value| {
                    send_runtime
                        .start_background(&with_test_executable(params, &send_executable), None)
                },
                |_conversation_id: &str| json!([]),
                |_params: &Value| Ok(json!({ "ok": true })),
                |_params: &Value| Ok(json!({ "ok": true, "output": "" })),
                |_request: Value| Ok(json!({})),
            ),
        );
        runtime.set_settlement_hook(move |conversation_id, payload| {
            let _ = tx.send((conversation_id.to_owned(), payload.clone()));
            Ok(())
        });
        let (conversation_id, _owner, agent) = create_designated_group(&service, "Unadmitted chat");
        let started = runtime
            .start_background(
                &with_test_executable(
                    &json!({
                        "agent": "codex",
                        "agentId": "codex",
                        "text": "print json",
                        "streamEvents": true,
                        "conversationId": conversation_id,
                        "membershipId": agent,
                    }),
                    &executable,
                ),
                None,
            )
            .unwrap();
        let (_settled_conversation, settled_payload) = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("unadmitted fake Codex finish must settle");
        let dispatch_id = settled_payload
            .get("dispatchId")
            .and_then(Value::as_str)
            .or_else(|| started["turnHandle"].as_str())
            .expect("dispatchId");
        let event = service
            .store()
            .agent_turn_event_for_dispatch(&conversation_id, dispatch_id)
            .unwrap()
            .expect("unadmitted turn event");
        assert_eq!(
            visible_text(service.store(), &conversation_id, &event.id),
            exact
        );
        let _ = std::fs::remove_file(result_path);
        let _ = std::fs::remove_dir_all(executable.parent().unwrap());
    }

    #[test]
    fn untyped_admitted_fake_codex_output_is_raw_reply() {
        let _guard = FAKE_CODEX_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let executable = compile_fake_codex();
        let store_root = executable.parent().unwrap().join("untyped-store");
        std::fs::create_dir_all(&store_root).unwrap();
        let store = ConversationStore::open(&store_root).unwrap();
        let start_kinds = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (runtime, service, rx) =
            bind_fake_codex_parent_runtime(store, executable.clone(), start_kinds);
        let (conversation_id, owner, _agent) =
            create_designated_group(&service, "Untyped envelope");
        let raw = typed_child_proposal_json(&conversation_id);
        let mut result_path = executable.clone();
        result_path.set_extension("result.json");
        std::fs::write(&result_path, &raw).unwrap();
        let posted = service
            .execute(json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner,
                "content": "this should stay visible",
            }))
            .unwrap();
        service
            .execute(json!({
                "action": "conversation.dispatch.after-post",
                "conversationId": conversation_id,
                "eventId": posted["event"]["id"],
            }))
            .unwrap();
        let (settled_conversation, settled_payload, _settlement) = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("untyped fake Codex finish must settle");
        assert_eq!(settled_conversation, conversation_id);
        assert_eq!(
            settled_payload.get("ok").and_then(Value::as_bool),
            Some(true)
        );
        assert_ne!(
            settled_payload.get("code").and_then(Value::as_str),
            Some(ASSISTANT_TURN_INVALID_ERROR)
        );
        let dispatch_id = settled_payload["dispatchId"].as_str().expect("dispatchId");
        let event = service
            .store()
            .agent_turn_event_for_dispatch(&conversation_id, dispatch_id)
            .unwrap()
            .expect("completed turn event");
        assert_eq!(
            visible_text(service.store(), &conversation_id, &event.id),
            raw
        );
        assert!(
            service
                .store()
                .list_child_relations(&conversation_id, None, 8)
                .unwrap()
                .is_empty(),
            "raw conversation is not Goal acceptance"
        );
        let _ = runtime;
        let _ = std::fs::remove_file(result_path);
        let _ = std::fs::remove_dir_all(executable.parent().unwrap());
    }

    #[test]
    fn admitted_fake_codex_public_terminal_and_evicted_replay_use_reply_only() {
        let _guard = FAKE_CODEX_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let executable = compile_fake_codex();
        let store_root = executable.parent().unwrap().join("public-terminal-store");
        std::fs::create_dir_all(&store_root).unwrap();
        let reply = "公开回复只保留 replyText。";
        let store = ConversationStore::open(&store_root).unwrap();
        let start_kinds = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (runtime, service, rx) =
            bind_fake_codex_parent_runtime(store, executable.clone(), start_kinds);
        let (conversation_id, owner, _agent) = create_designated_group(&service, "Public terminal");
        let envelope = assistant_envelope_json(reply, &question_proposal_json(&conversation_id));
        let mut result_path = executable.clone();
        result_path.set_extension("result.json");
        std::fs::write(&result_path, &envelope).unwrap();
        let posted = service
            .execute(json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner,
                "content": "ask for a public reply",
            }))
            .unwrap();
        service
            .execute(json!({
                "action": "conversation.dispatch.after-post",
                "conversationId": conversation_id,
                "eventId": posted["event"]["id"],
            }))
            .unwrap();
        let (_settled_conversation, settled_payload, _settlement) = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("admitted fake Codex finish must settle");
        let dispatch_id = settled_payload["dispatchId"].as_str().expect("dispatchId");
        assert!(
            settled_payload
                .get("output")
                .and_then(Value::as_str)
                .is_some_and(|output| output.contains("interpretationProposal")),
            "private host settlement must retain the typed proposal exactly once"
        );
        let (ok, public_payload) = runtime
            .public_terminal(dispatch_id)
            .expect("finish must store the public terminal");
        assert!(
            ok,
            "successful admitted public terminal must be ok: {public_payload}"
        );
        assert_eq!(
            public_payload.get("output").and_then(Value::as_str),
            Some(reply),
            "public terminal output must be replyText only: {public_payload}"
        );
        assert!(
            !public_payload
                .to_string()
                .contains("interpretationProposal"),
            "public terminal must not leak the private proposal: {public_payload}"
        );

        runtime.evict_turn_cache(dispatch_id);
        let turn = runtime
            .turn(dispatch_id)
            .expect("turn remains after eviction");
        let high_water = runtime
            .turn_high_water(dispatch_id)
            .expect("successful admitted turn keeps high_water");
        let writer = Arc::new(StdioRpcWriter::new(Vec::<u8>::new()));
        replay_turn(&writer, "attach-public", "workflow-public", &turn, 0).unwrap();
        let frames = decode_replay_frames(&writer);
        let terminal = assert_contiguous_store_fallback_replay(&frames, high_water);
        assert_eq!(terminal["ok"], true, "{terminal}");
        assert_eq!(
            terminal["result"]["output"].as_str(),
            Some(reply),
            "store-fallback attach must write the public reply: {terminal}"
        );
        assert!(
            !terminal.to_string().contains("interpretationProposal"),
            "evicted attach/replay must not leak the private envelope: {terminal}"
        );
        for frame in &frames {
            if let Some(text) = frame
                .pointer("/payload/text")
                .or_else(|| frame.pointer("/event/payload/text"))
                .and_then(Value::as_str)
            {
                assert!(
                    !text.contains("interpretationProposal"),
                    "store-fallback replay frames must stay public: {frame}"
                );
            }
        }
        let _ = runtime;
        let _ = std::fs::remove_file(result_path);
        let _ = std::fs::remove_dir_all(executable.parent().unwrap());
    }

    #[test]
    fn untyped_admitted_public_terminal_is_raw_after_fake_codex() {
        let _guard = FAKE_CODEX_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let executable = compile_fake_codex();
        let store_root = executable.parent().unwrap().join("public-untyped-store");
        std::fs::create_dir_all(&store_root).unwrap();
        let store = ConversationStore::open(&store_root).unwrap();
        let start_kinds = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (runtime, service, rx) =
            bind_fake_codex_parent_runtime(store, executable.clone(), start_kinds);
        let (conversation_id, owner, _agent) =
            create_designated_group(&service, "Public untyped terminal");
        let raw = typed_child_proposal_json(&conversation_id);
        let mut result_path = executable.clone();
        result_path.set_extension("result.json");
        std::fs::write(&result_path, &raw).unwrap();
        let posted = service
            .execute(json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner,
                "content": "this should stay visible publicly",
            }))
            .unwrap();
        service
            .execute(json!({
                "action": "conversation.dispatch.after-post",
                "conversationId": conversation_id,
                "eventId": posted["event"]["id"],
            }))
            .unwrap();
        let (_settled_conversation, settled_payload, _settlement) = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("untyped fake Codex finish must settle");
        let dispatch_id = settled_payload["dispatchId"].as_str().expect("dispatchId");
        assert_ne!(
            settled_payload.get("code").and_then(Value::as_str),
            Some(ASSISTANT_TURN_INVALID_ERROR)
        );
        let (ok, public_payload) = runtime
            .public_terminal(dispatch_id)
            .expect("untyped admitted finish must store a public terminal");
        assert!(
            ok,
            "untyped admitted public terminal must stay ok: {public_payload}"
        );
        assert_eq!(
            public_payload.get("output").and_then(Value::as_str),
            Some(raw.as_str()),
            "public terminal must keep the raw conversation: {public_payload}"
        );
        runtime.evict_turn_cache(dispatch_id);
        let turn = runtime.turn(dispatch_id).expect("completed turn remains");
        let high_water = runtime
            .turn_high_water(dispatch_id)
            .expect("untyped turn keeps its high-water after eviction");
        let writer = Arc::new(StdioRpcWriter::new(Vec::<u8>::new()));
        replay_turn(&writer, "attach-untyped", "workflow-untyped", &turn, 0).unwrap();
        let frames = decode_replay_frames(&writer);
        let terminal = assert_contiguous_store_fallback_replay(&frames, high_water);
        assert_eq!(terminal["ok"], true, "{terminal}");
        assert_eq!(
            terminal["result"]["output"].as_str(),
            Some(raw.as_str()),
            "untyped admitted attach must keep the raw conversation: {terminal}"
        );
        let _ = runtime;
        let _ = std::fs::remove_file(result_path);
        let _ = std::fs::remove_dir_all(executable.parent().unwrap());
    }

    #[test]
    fn admitted_native_failed_fake_codex_public_terminal_preserves_code_from_zero() {
        let _guard = FAKE_CODEX_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let executable = compile_fake_codex();
        let store_root = executable.parent().unwrap().join("native-failed-store");
        std::fs::create_dir_all(&store_root).unwrap();
        let store = ConversationStore::open(&store_root).unwrap();
        let start_kinds = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (runtime, service, rx) =
            bind_fake_codex_parent_runtime(store, executable.clone(), start_kinds);
        let (conversation_id, owner, _agent) =
            create_designated_group(&service, "Native failed terminal");
        let posted = service
            .execute(json!({
                "action": "conversation.message.post",
                "conversationId": conversation_id,
                "authorMembershipId": owner,
                "content": "this should fail natively",
            }))
            .unwrap();
        service
            .execute(json!({
                "action": "conversation.dispatch.after-post",
                "conversationId": conversation_id,
                "eventId": posted["event"]["id"],
            }))
            .unwrap();
        let (_settled_conversation, settled_payload, _settlement) = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("native fake Codex failure must settle");
        let dispatch_id = settled_payload["dispatchId"].as_str().expect("dispatchId");
        let (ok, public_payload) = runtime
            .public_terminal(dispatch_id)
            .expect("native failed finish must store a public terminal");
        assert!(
            !ok,
            "native failed public terminal must not be ok=true: {public_payload}"
        );
        let code = public_terminal_code(&public_payload).expect("native failure keeps a code");
        assert_ne!(
            code, ASSISTANT_TURN_INVALID_ERROR,
            "genuine native failure must not be rewritten as envelope validation: {public_payload}"
        );
        assert!(
            !code.is_empty(),
            "native failure must keep its code: {public_payload}"
        );
        assert!(
            public_terminal_stage(&public_payload).is_some_and(|stage| !stage.is_empty()),
            "native failure must keep its stage: {public_payload}"
        );
        runtime.evict_turn_cache(dispatch_id);
        let turn = runtime.turn(dispatch_id).expect("failed turn remains");
        let high_water = runtime
            .turn_high_water(dispatch_id)
            .expect("native failed turn keeps high_water");
        let writer = Arc::new(StdioRpcWriter::new(Vec::<u8>::new()));
        replay_turn(&writer, "attach-native-failed", "workflow-native", &turn, 0).unwrap();
        let frames = decode_replay_frames(&writer);
        let terminal = assert_contiguous_store_fallback_replay(&frames, high_water);
        assert_eq!(terminal["ok"], false, "{terminal}");
        assert_eq!(
            public_terminal_code(&terminal["error"]),
            Some(code),
            "evicted native-failed attach must keep the same native code: {terminal}"
        );
        let _ = runtime;
        let _ = std::fs::remove_dir_all(executable.parent().unwrap());
    }

    #[test]
    fn admitted_cancelled_fragmented_known_text_fields_stay_public_after_replay() {
        let runtime = runtime(DEFAULT_TURN_CACHE_BYTES);
        let turn = runtime
            .begin_with(
                &json!({
                    "agent": "synthetic",
                    "sessionId": "session-cancel",
                    "text": "cancel while streaming",
                    "continuityKind": "user-posted",
                }),
                PersistentTurnAdmission::Host,
            )
            .unwrap();
        assert!(
            turn.admitted_assistant_turn,
            "cancelled proof must use the admitted writer"
        );
        PersistentConversationRuntime::record_event(
            &turn,
            json!({
                "event": "agent.message.chunk",
                "sessionId": "session-cancel",
                "turnId": "native-turn-cancel",
                "payload": {
                    "text": "{\"replyText\":\"partial\",\"interpretationProposal\":"
                }
            }),
        )
        .unwrap();
        PersistentConversationRuntime::finish(
            &turn,
            PersistentTerminal {
                ok: false,
                payload: json!({
                    "ok": false,
                    "turnStatus": "cancelled",
                    "output": "{\"replyText\":\"partial\",\"interpretationProposal\":",
                    "events": [
                        {
                            "kind": "text",
                            "text": "{\"replyText\":\"partial\",\"interpretationProposal\":"
                        },
                        {
                            "kind": "tool",
                            "name": "read",
                            "text": "keep-tool-fact"
                        }
                    ],
                    "terminalTransition": {
                        "kind": "text",
                        "text": "{\"interpretationProposal\":"
                    },
                    "author": "agent:codex",
                    "evidence": [{"kind": "artifact", "id": "art:1"}]
                }),
            },
        )
        .unwrap();
        let (ok, public_payload) = runtime
            .public_terminal(&turn.scope.dispatch_id)
            .expect("cancelled finish must store a public terminal");
        assert!(
            !ok,
            "cancelled public terminal stays non-ok: {public_payload}"
        );
        assert_eq!(
            public_payload.get("turnStatus").and_then(Value::as_str),
            Some("cancelled"),
            "{public_payload}"
        );
        assert_ne!(
            public_terminal_code(&public_payload),
            Some(ASSISTANT_TURN_INVALID_ERROR)
        );
        assert_eq!(
            public_payload.get("output").and_then(Value::as_str),
            Some("")
        );
        assert_eq!(public_payload["events"][0]["text"], "");
        assert_eq!(public_payload["events"][1]["text"], "keep-tool-fact");
        assert_eq!(public_payload["terminalTransition"]["text"], "");
        assert_eq!(public_payload["author"], "agent:codex");
        assert_eq!(public_payload["evidence"][0]["id"], "art:1");
        runtime.evict_turn_cache(&turn.scope.dispatch_id);
        let high_water = runtime
            .turn_high_water(&turn.scope.dispatch_id)
            .expect("cancelled turn keeps high_water");
        let writer = Arc::new(StdioRpcWriter::new(Vec::<u8>::new()));
        replay_turn(&writer, "attach-cancelled", "workflow-cancelled", &turn, 0).unwrap();
        let frames = decode_replay_frames(&writer);
        let terminal = assert_contiguous_store_fallback_replay(&frames, high_water);
        assert_eq!(terminal["ok"], false, "{terminal}");
        assert_eq!(
            terminal["error"]["turnStatus"].as_str(),
            Some("cancelled"),
            "{terminal}"
        );
        assert_eq!(terminal["error"]["output"].as_str(), Some(""));
        assert_eq!(terminal["error"]["events"][0]["text"], "");
        assert_eq!(terminal["error"]["events"][1]["text"], "keep-tool-fact");
        assert_eq!(terminal["error"]["terminalTransition"]["text"], "");
        assert!(
            !terminal.to_string().contains("interpretationProposal"),
            "cancelled attach must not leak envelope fragments: {terminal}"
        );
    }

    #[test]
    fn forged_public_rpc_continuity_kind_does_not_admit_response_mode() {
        let _guard = FAKE_CODEX_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let executable = compile_fake_codex();
        let store_root = executable.parent().unwrap().join("forged-rpc-store");
        std::fs::create_dir_all(&store_root).unwrap();
        let reply = "forged flag must stay ordinary";
        let store = ConversationStore::open(&store_root).unwrap();
        let runtime = PersistentConversationRuntime::new(store.clone());
        let (tx, rx) = std::sync::mpsc::channel::<(String, Value)>();
        runtime.set_settlement_hook(move |conversation_id, payload| {
            let _ = tx.send((conversation_id.to_owned(), payload.clone()));
            Ok(())
        });
        let service = ConversationService::from_store(store);
        let (conversation_id, _owner, agent) =
            create_designated_group(&service, "Forged public RPC");
        let envelope = assistant_envelope_json(reply, &question_proposal_json(&conversation_id));
        let mut result_path = executable.clone();
        result_path.set_extension("result.json");
        std::fs::write(&result_path, &envelope).unwrap();
        let params = with_test_executable(
            &json!({
                "agent": "codex",
                "agentId": "codex",
                "text": "print envelope",
                "streamEvents": true,
                "conversationId": conversation_id,
                "membershipId": agent,
                "continuityKind": "user-posted",
            }),
            &executable,
        );
        let input = {
            let mut bytes = Vec::new();
            serde_json::to_writer(
                &mut bytes,
                &json!({
                    "protocol": STDIO_RPC_PROTOCOL,
                    "id": "forged-dispatch",
                    "workflowId": "forged-workflow",
                    "method": "agent.conversation.dispatch",
                    "params": params,
                }),
            )
            .unwrap();
            bytes.push(b'\n');
            serde_json::to_writer(
                &mut bytes,
                &json!({
                    "protocol": STDIO_RPC_PROTOCOL,
                    "id": "forged-shutdown",
                    "workflowId": "forged-workflow",
                    "method": "shutdown",
                }),
            )
            .unwrap();
            bytes.push(b'\n');
            std::io::Cursor::new(bytes)
        };
        let output = super::super::serve_stdio_rpc_with_persistent_conversation(
            input,
            Vec::new(),
            |_, _| -> anyhow::Result<_> {
                panic!("public conversation dispatch must not fall back to execute")
            },
            runtime.clone(),
            service.clone(),
        )
        .unwrap();
        let frames = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        let dispatch = frames
            .iter()
            .find(|frame| frame["id"] == "forged-dispatch")
            .expect("public dispatch frame");
        assert_eq!(dispatch["ok"], true, "{dispatch}");
        let handle = dispatch["result"]["turnHandle"]
            .as_str()
            .expect("public dispatch receipt");
        let (_settled_conversation, _settled_payload) = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("forged public dispatch must still settle through fake Codex");
        let event = service
            .store()
            .agent_turn_event_for_dispatch(&conversation_id, handle)
            .unwrap()
            .expect("forged public turn event");
        let scope = licoup_native::domain::client_conversation::ConversationRuntimeScope {
            dispatch_id: handle.to_owned(),
            conversation_id: conversation_id.clone(),
            membership_id: event.author_membership_id.clone().unwrap_or_default(),
            event_id: event.id.clone(),
        };
        assert_eq!(
            service.store().runtime_response_mode(&scope).unwrap(),
            None,
            "public RPC continuityKind must not admit the private response mode"
        );
        assert_eq!(
            visible_text(service.store(), &conversation_id, &event.id),
            envelope,
            "forged public admission must remain ordinary pass-through"
        );
        let _ = runtime;
        let _ = std::fs::remove_file(result_path);
        let _ = std::fs::remove_dir_all(executable.parent().unwrap());
    }

    fn typed_child_proposal_json_with_result(
        conversation_id: &str,
        expected_result: &str,
    ) -> String {
        let mut proposal = serde_json::from_str::<ContinuityInterpretationProposal>(
            &typed_child_proposal_json(conversation_id),
        )
        .unwrap();
        if let Some(commitment) = proposal.commitment_proposals.first_mut() {
            commitment.expected_result = expected_result.to_owned();
        }
        serde_json::to_string(&proposal).unwrap()
    }
}
