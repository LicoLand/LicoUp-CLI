//! The Codex and Pi protocol adapters, and the driver execution each reaches.
//!
//! This module answers the work-context seam's port for the two Agents this
//! host composes today: how each one's protocol adapter is built and how each
//! one's driver is executed. It is not the seam — that is
//! `licoup-agent-drivers` — and it is not where either Agent stays: the Codex
//! half travels to `licoup-agent-codex` and the Pi half to `licoup-agent-pi`
//! when those crates exist.
//!
//! Requests and response rules match the live parsers. Transport is injected; a
//! stored binding row is never treated as resume success.

use std::sync::Arc;

use licoup_agent_drivers::{
    AdapterCall, AdapterResponse, AdapterTransport, AgentProtocolRegistration, DriverExecution,
    DriverFailure, DriverInvocation, unverified_snapshot,
};
use licoup_agent_runtime::work_context::{
    CapabilityProfile, ForkInheritance, IsolationReview, NativeCapabilitySnapshot,
    NativeControlRequest, NativeFidelity, NativeProtocolAdapter, NativeWorkContextKey,
    ParallelPolicy, ProtocolFamily, ProtocolMethods, ProtocolOutcome, SessionPresence,
    WorkContextConfig, WorkContextRuntime, identity_conflict, invalid_request, native_binding_lost,
    protocol_methods, reconciliation_required, unsupported_capability, work_context_runtime,
};
use licoup_conversation::{ConversationStore, PrivateRuntimeBinding};
use serde_json::{Value, json};

pub fn pi_session_id_missing(session_id: &str) -> bool {
    session_id.trim().is_empty()
}

struct StoredNativeBinding {
    session_id: String,
    working_directory: Option<String>,
    source_path: Option<String>,
}

fn stored_native_binding(
    store: Option<&ConversationStore>,
    key: &NativeWorkContextKey,
) -> Option<StoredNativeBinding> {
    let store = store?;
    let PrivateRuntimeBinding {
        runtime_session_id,
        runtime_conversation_path,
        working_directory,
        ..
    } = store
        .private_runtime_binding(&key.conversation_id, &key.membership_id)
        .ok()
        .flatten()?;
    Some(StoredNativeBinding {
        session_id: runtime_session_id,
        working_directory,
        source_path: runtime_conversation_path,
    })
}

fn stored_session_id(store: Option<&ConversationStore>, key: &NativeWorkContextKey) -> String {
    stored_native_binding(store, key)
        .map(|binding| binding.session_id)
        .filter(|id| !id.trim().is_empty())
        .unwrap_or_default()
}

fn scope_object(key: &NativeWorkContextKey, binding: Option<&StoredNativeBinding>) -> Value {
    let mut params = json!({
        "conversationId": key.conversation_id,
        "membershipId": key.membership_id,
        "matterId": key.matter_id,
        "generation": key.generation,
    });
    if let Some(binding) = binding {
        if let Some(cwd) = binding
            .working_directory
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            params["workingDirectory"] = json!(cwd);
        }
        if let Some(path) = binding
            .source_path
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            params["sourcePath"] = json!(path);
        }
    }
    params
}

fn reject_caller_session_override(params: &Value, stored_session: &str) -> Option<ProtocolOutcome> {
    let caller = params
        .get("sessionId")
        .or_else(|| params.get("nativeSessionId"))
        .or_else(|| params.get("threadId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match caller {
        Some(session) if session != stored_session => Some(ProtocolOutcome::failed(
            "identity/validate",
            identity_conflict(),
        )),
        _ => None,
    }
}

fn classify_adapter_failure(method: &'static str, response: &AdapterResponse) -> ProtocolOutcome {
    let code = response
        .result
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("");
    let stage = response
        .result
        .get("stage")
        .and_then(Value::as_str)
        .unwrap_or("");
    let message = response.error_message.as_deref().unwrap_or("");
    let failure = if is_invalid_timeout(code, message) {
        invalid_request()
    } else if is_unsupported_control(code, message) {
        unsupported_capability()
    } else if is_binding_loss(code, stage, message) {
        native_binding_lost()
    } else {
        reconciliation_required()
    };
    ProtocolOutcome::failed(method, failure)
}

fn is_invalid_timeout(code: &str, message: &str) -> bool {
    code == "invalid_request" || message.contains("invalid_request:timeout")
}

fn is_unsupported_control(code: &str, message: &str) -> bool {
    code.contains("unsupported")
        || message.contains("unsupported-method")
        || message.contains("dispatch_cancel_unsupported")
        || message.contains("dispatch_steer_unsupported")
}

fn is_binding_loss(code: &str, stage: &str, message: &str) -> bool {
    matches!(
        code,
        "pi_session_id_missing"
            | "pi_session_not_found"
            | "pi_session_identity_ambiguous"
            | "pi_session_identity_mismatch"
            | "pi_session_switch_failed"
            | "codex_thread_resume_identity_mismatch"
            | "codex_resume_source_identity_mismatch"
            | "codex_thread_unarchive_identity_mismatch"
            | "native_binding_lost"
    ) || message.contains("is archived")
        || message.contains("lost:")
        || ((stage.contains("session/resume") || message.contains("session/resume"))
            && (code.contains("not_found")
                || message.contains("not_found")
                || message.contains("pi_session_not_found")
                || message.contains("pi_session_id_missing")
                || message.contains("pi_session_identity_ambiguous")))
}

pub struct CodexAdapterProtocol {
    store: Option<ConversationStore>,
    transport: Arc<dyn AdapterTransport>,
}

impl CodexAdapterProtocol {
    pub fn new(store: Option<ConversationStore>, transport: Arc<dyn AdapterTransport>) -> Self {
        Self { store, transport }
    }

    fn call(&self, method: &'static str, params: Value) -> AdapterResponse {
        self.transport.invoke(&AdapterCall { method, params })
    }

    fn control_params(&self, request: &NativeControlRequest) -> Result<Value, ProtocolOutcome> {
        let stored = stored_native_binding(self.store.as_ref(), &request.key);
        let session_id = stored
            .as_ref()
            .map(|binding| binding.session_id.as_str())
            .unwrap_or("");
        if session_id.is_empty() {
            return Err(ProtocolOutcome::failed(
                self.methods().steer,
                native_binding_lost(),
            ));
        }
        let mut params = scope_object(&request.key, stored.as_ref());
        if let Some(failure) = reject_caller_session_override(&params, session_id) {
            return Err(failure);
        }
        params["agent"] = json!("codex");
        params["threadId"] = json!(session_id);
        params["sessionId"] = json!(session_id);
        params["turnHandle"] = json!(request.host_handle());
        params["turnId"] = json!(request.native_turn_id());
        if let Some(content) = request.steer_content() {
            params["text"] = json!(content);
        }
        Ok(params)
    }
}

impl NativeProtocolAdapter for CodexAdapterProtocol {
    fn family(&self) -> ProtocolFamily {
        ProtocolFamily::Codex
    }

    fn profile(&self) -> CapabilityProfile {
        CapabilityProfile::High
    }

    fn capabilities(&self) -> NativeCapabilitySnapshot {
        self.transport
            .negotiated_capabilities()
            .unwrap_or_else(unverified_snapshot)
    }

    fn methods(&self) -> ProtocolMethods {
        protocol_methods(ProtocolFamily::Codex)
    }

    fn fidelity(&self, child_author: &NativeFidelity) -> NativeFidelity {
        child_author.clone()
    }

    fn isolation(&self) -> IsolationReview {
        IsolationReview::unknown()
    }

    fn parallel_policy(&self) -> ParallelPolicy {
        ParallelPolicy::HonestQueue
    }

    fn session_presence(&self, key: &NativeWorkContextKey) -> SessionPresence {
        if stored_session_id(self.store.as_ref(), key).is_empty() {
            SessionPresence::Unknown
        } else {
            SessionPresence::Present
        }
    }

    fn session_conflict_key(&self, key: &NativeWorkContextKey) -> Option<String> {
        let session = stored_session_id(self.store.as_ref(), key);
        if session.is_empty() {
            None
        } else {
            Some(format!("codex:{session}"))
        }
    }

    fn exact_resume(&self, key: &NativeWorkContextKey) -> ProtocolOutcome {
        let method = self.methods().exact_resume;
        let stored = stored_native_binding(self.store.as_ref(), key);
        let thread_id = stored
            .as_ref()
            .map(|binding| binding.session_id.as_str())
            .unwrap_or("");
        if thread_id.is_empty() {
            return ProtocolOutcome::failed(method, native_binding_lost());
        }
        let mut params = scope_object(key, stored.as_ref());
        params["threadId"] = json!(thread_id);
        params["sessionId"] = json!(thread_id);
        params["agent"] = json!("codex");
        let response = self.call(method, params);
        if !response.ok {
            return classify_adapter_failure(method, &response);
        }
        if let Some(returned) = response
            .result
            .get("thread")
            .and_then(|thread| thread.get("id"))
            .and_then(Value::as_str)
        {
            if returned != thread_id {
                return ProtocolOutcome::failed(method, native_binding_lost());
            }
        }
        ProtocolOutcome::applied(method)
    }

    fn start_new(&self, key: &NativeWorkContextKey) -> ProtocolOutcome {
        let method = self.methods().start_new;
        let stored = stored_native_binding(self.store.as_ref(), key);
        let mut params = scope_object(key, stored.as_ref());
        params["openMode"] = json!("new");
        params["agent"] = json!("codex");
        let response = self.call(method, params);
        if response.ok {
            ProtocolOutcome::applied(method)
        } else {
            classify_adapter_failure(method, &response)
        }
    }

    fn fork(&self, _key: &NativeWorkContextKey, _inheritance: &ForkInheritance) -> ProtocolOutcome {
        ProtocolOutcome::failed(self.methods().fork, unsupported_capability())
    }

    fn compact(&self, _key: &NativeWorkContextKey) -> ProtocolOutcome {
        ProtocolOutcome::failed(self.methods().compact, unsupported_capability())
    }

    fn steer(&self, request: &NativeControlRequest) -> ProtocolOutcome {
        let method = self.methods().steer;
        let params = match self.control_params(request) {
            Ok(params) => params,
            Err(mut outcome) => {
                outcome.method = method;
                return outcome;
            }
        };
        let response = self.call(method, params);
        if response.ok {
            ProtocolOutcome::applied(method)
        } else {
            classify_adapter_failure(method, &response)
        }
    }

    fn cancel(&self, request: &NativeControlRequest) -> ProtocolOutcome {
        let method = self.methods().cancel;
        let params = match self.control_params(request) {
            Ok(params) => params,
            Err(mut outcome) => {
                outcome.method = method;
                return outcome;
            }
        };
        let response = self.call(method, params);
        if response.ok {
            ProtocolOutcome::applied(method)
        } else {
            classify_adapter_failure(method, &response)
        }
    }
}

pub struct PiAdapterProtocol {
    store: Option<ConversationStore>,
    transport: Arc<dyn AdapterTransport>,
}

impl PiAdapterProtocol {
    pub fn new(store: Option<ConversationStore>, transport: Arc<dyn AdapterTransport>) -> Self {
        Self { store, transport }
    }

    fn call(&self, method: &'static str, params: Value) -> AdapterResponse {
        self.transport.invoke(&AdapterCall { method, params })
    }

    fn control_params(&self, request: &NativeControlRequest) -> Result<Value, ProtocolOutcome> {
        let stored = stored_native_binding(self.store.as_ref(), &request.key);
        let session_id = stored
            .as_ref()
            .map(|binding| binding.session_id.as_str())
            .unwrap_or("");
        if pi_session_id_missing(session_id) {
            return Err(ProtocolOutcome::failed(
                self.methods().steer,
                native_binding_lost(),
            ));
        }
        let mut params = scope_object(&request.key, stored.as_ref());
        if let Some(failure) = reject_caller_session_override(&params, session_id) {
            return Err(failure);
        }
        params["agent"] = json!("pi");
        params["sessionId"] = json!(session_id);
        params["turnHandle"] = json!(request.host_handle());
        params["turnId"] = json!(request.native_turn_id());
        if let Some(content) = request.steer_content() {
            params["text"] = json!(content);
        }
        Ok(params)
    }
}

impl NativeProtocolAdapter for PiAdapterProtocol {
    fn family(&self) -> ProtocolFamily {
        ProtocolFamily::Pi
    }

    fn profile(&self) -> CapabilityProfile {
        CapabilityProfile::High
    }

    fn capabilities(&self) -> NativeCapabilitySnapshot {
        self.transport
            .negotiated_capabilities()
            .unwrap_or_else(unverified_snapshot)
    }

    fn methods(&self) -> ProtocolMethods {
        protocol_methods(ProtocolFamily::Pi)
    }

    fn fidelity(&self, child_author: &NativeFidelity) -> NativeFidelity {
        child_author.clone()
    }

    fn isolation(&self) -> IsolationReview {
        IsolationReview::unknown()
    }

    fn parallel_policy(&self) -> ParallelPolicy {
        ParallelPolicy::HonestQueue
    }

    fn session_presence(&self, key: &NativeWorkContextKey) -> SessionPresence {
        if stored_session_id(self.store.as_ref(), key).is_empty() {
            SessionPresence::Unknown
        } else {
            SessionPresence::Present
        }
    }

    fn session_conflict_key(&self, key: &NativeWorkContextKey) -> Option<String> {
        let session = stored_session_id(self.store.as_ref(), key);
        if session.is_empty() {
            None
        } else {
            Some(format!("pi:{session}"))
        }
    }

    fn exact_resume(&self, key: &NativeWorkContextKey) -> ProtocolOutcome {
        let method = self.methods().exact_resume;
        let stored = stored_native_binding(self.store.as_ref(), key);
        let session_id = stored
            .as_ref()
            .map(|binding| binding.session_id.as_str())
            .unwrap_or("");
        if pi_session_id_missing(session_id) {
            return ProtocolOutcome::failed(method, native_binding_lost());
        }
        let mut params = scope_object(key, stored.as_ref());
        params["sessionId"] = json!(session_id);
        params["agent"] = json!("pi");
        let response = self.call(method, params);
        if !response.ok {
            return classify_adapter_failure(method, &response);
        }
        if let Some(returned) = response.result.get("sessionId").and_then(Value::as_str) {
            if !returned.is_empty() && returned != session_id {
                return ProtocolOutcome::failed(method, native_binding_lost());
            }
        }
        ProtocolOutcome::applied(method)
    }

    fn start_new(&self, key: &NativeWorkContextKey) -> ProtocolOutcome {
        let method = self.methods().start_new;
        let stored = stored_native_binding(self.store.as_ref(), key);
        let mut params = scope_object(key, stored.as_ref());
        params["openMode"] = json!("new");
        params["agent"] = json!("pi");
        let response = self.call(method, params);
        if response.ok {
            ProtocolOutcome::applied(method)
        } else {
            classify_adapter_failure(method, &response)
        }
    }

    fn fork(&self, _key: &NativeWorkContextKey, _inheritance: &ForkInheritance) -> ProtocolOutcome {
        ProtocolOutcome::failed(self.methods().fork, unsupported_capability())
    }

    fn compact(&self, _key: &NativeWorkContextKey) -> ProtocolOutcome {
        ProtocolOutcome::failed(self.methods().compact, unsupported_capability())
    }

    fn steer(&self, request: &NativeControlRequest) -> ProtocolOutcome {
        let method = self.methods().steer;
        let params = match self.control_params(request) {
            Ok(params) => params,
            Err(mut outcome) => {
                outcome.method = method;
                return outcome;
            }
        };
        let response = self.call(method, params);
        if response.ok {
            ProtocolOutcome::applied(method)
        } else {
            classify_adapter_failure(method, &response)
        }
    }

    fn cancel(&self, request: &NativeControlRequest) -> ProtocolOutcome {
        let method = self.methods().cancel;
        let params = match self.control_params(request) {
            Ok(params) => params,
            Err(mut outcome) => {
                outcome.method = method;
                return outcome;
            }
        };
        let response = self.call(method, params);
        if response.ok {
            ProtocolOutcome::applied(method)
        } else {
            classify_adapter_failure(method, &response)
        }
    }
}

// ---------------------------------------------------------------------------
// The port this module answers
// ---------------------------------------------------------------------------

/// The Codex half of the port: how this host builds the Codex protocol adapter
/// and executes the Codex driver.
pub(super) const CODEX: AgentProtocolRegistration = AgentProtocolRegistration {
    agent_id: "codex",
    bind: bind_codex,
    execute: execute_codex,
};

/// The Pi half of the port.
pub(super) const PI: AgentProtocolRegistration = AgentProtocolRegistration {
    agent_id: "pi",
    bind: bind_pi,
    execute: execute_pi,
};

fn bind_codex(
    config: WorkContextConfig,
    store: Option<ConversationStore>,
    transport: Arc<dyn AdapterTransport>,
) -> WorkContextRuntime {
    work_context_runtime(CodexAdapterProtocol::new(store, transport), config)
}

fn bind_pi(
    config: WorkContextConfig,
    store: Option<ConversationStore>,
    transport: Arc<dyn AdapterTransport>,
) -> WorkContextRuntime {
    work_context_runtime(PiAdapterProtocol::new(store, transport), config)
}

fn execute_codex(invocation: &DriverInvocation<'_>) -> DriverExecution {
    let result = licoup_agent_codex::app_server::driver::execute(
        invocation.executable,
        invocation.params,
        invocation.prompt,
        invocation.session_id,
        invocation.cwd,
        invocation.timeout_ms,
        invocation.max_stdout,
        invocation.max_stderr,
        Some(crate::platform::codex_app_server_environment()),
    );
    let failure = result.error.as_ref().map(|error| DriverFailure {
        code: error.code,
        stage: error.stage,
        message: error.message,
        identity: error.thread_id.clone(),
    });
    DriverExecution {
        ok: result.ok,
        thread_id: result.thread_id,
        session_id: result.session_id,
        output: result.output,
        failure,
    }
}

fn execute_pi(invocation: &DriverInvocation<'_>) -> DriverExecution {
    let result = licoup_agent_pi::driver::execute(
        invocation.executable,
        invocation.params,
        invocation.prompt,
        invocation.session_id,
        invocation.cwd,
        invocation.timeout_ms,
        invocation.max_stdout,
        invocation.max_stderr,
    );
    let failure = result.error.as_ref().map(|error| DriverFailure {
        code: error.code,
        stage: error.stage,
        message: error.message,
        identity: error.session_id.clone(),
    });
    DriverExecution {
        ok: result.ok,
        thread_id: result.thread_id,
        session_id: result.session_id,
        output: result.output,
        failure,
    }
}
