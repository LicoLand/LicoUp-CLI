//! Production adapter transport: the registered driver execution and the host's
//! lane control, both injected through the port.
//!
//! This module reaches an Agent the way the composition says to reach it and
//! names no Agent of its own: the driver execution comes from
//! [`AgentProtocolRegistration::execute`], the inventory id that lane control
//! addresses comes from [`AgentProtocolRegistration::agent_id`], and steer and
//! cancel go through [`LaneReach`]. Unavailable only when the driver is
//! genuinely missing or the call fails.

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use licoup_agent_runtime::work_context::{NativeCapabilitySnapshot, NativeCapabilitySupport};
use licoup_agent_targets::port::AgentTargetPort;
use serde_json::{Value, json};

use crate::runtime_adapters::port::dispatch_timeout as resolve_dispatch_timeout;

use super::port::{
    AgentProtocolRegistration, DriverExecution, DriverFailure, DriverInvocation, LaneReach,
    unverified_snapshot,
};
use super::transport::{AdapterCall, AdapterResponse, AdapterTransport};

const DRIVER_MAX_STDOUT: usize = 1024 * 1024;
const DRIVER_MAX_STDERR: usize = 1024;

/// Reaches the registered driver execution and the host's lane steer/cancel
/// operations. Unavailable only when the driver is genuinely missing or the
/// call fails.
pub struct HostDriverTransport {
    /// The Agent this transport reaches, as the composition registered it.
    protocol: AgentProtocolRegistration,
    /// How the host reaches its own lane.
    lane: LaneReach,
    /// The inventory facts the lane operations this transport drives read.
    port: AgentTargetPort,
    executable: Mutex<Option<String>>,
    working_directory: Mutex<Option<PathBuf>>,
    invocations: AtomicU64,
    capabilities: Mutex<Option<NativeCapabilitySnapshot>>,
}

impl HostDriverTransport {
    pub fn new(protocol: AgentProtocolRegistration, lane: LaneReach) -> Self {
        Self {
            protocol,
            lane,
            port: (lane.target_port)(),
            executable: Mutex::new(None),
            working_directory: Mutex::new(None),
            invocations: AtomicU64::new(0),
            capabilities: Mutex::new(None),
        }
    }

    pub fn with_executable(self, executable: impl Into<String>) -> Self {
        *lock(&self.executable) = Some(executable.into());
        self
    }

    pub fn with_working_directory(self, working_directory: impl Into<PathBuf>) -> Self {
        *lock(&self.working_directory) = Some(working_directory.into());
        self
    }

    fn agent_id(&self) -> &'static str {
        self.protocol.agent_id
    }

    fn resolve_executable(&self, call: &AdapterCall) -> Result<String, AdapterResponse> {
        if let Some(configured) = lock(&self.executable).clone() {
            return Ok(configured);
        }
        if let Some(from_params) = call
            .params
            .get("executable")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Ok(from_params.to_owned());
        }
        if call_cwd(call).is_some() {
            return Err(AdapterResponse::err("adapter-executable-unconfigured"));
        }
        Err(AdapterResponse::err(format!(
            "adapter-executable-unconfigured:{}",
            call.method
        )))
    }

    fn mark_supported(&self, method: &str) {
        let mut snapshot = lock(&self.capabilities)
            .clone()
            .unwrap_or_else(unverified_snapshot);
        match method {
            "thread/resume" | "session/resume" => {
                snapshot.exact_resume = NativeCapabilitySupport::Supported;
            }
            "steer" | "turn/steer" => {
                snapshot.steer = NativeCapabilitySupport::Supported;
            }
            "cancel" | "turn/interrupt" => {
                snapshot.cancel = NativeCapabilitySupport::Supported;
            }
            _ => {}
        }
        *lock(&self.capabilities) = Some(snapshot);
    }

    fn run_driver(&self, call: &AdapterCall, session_id: &str) -> AdapterResponse {
        let executable = match self.resolve_executable(call) {
            Ok(path) => path,
            Err(response) => return response,
        };
        let cwd = call_cwd(call).or_else(|| lock(&self.working_directory).clone());
        let timeout_ms = match resolve_dispatch_timeout(&call.params) {
            Ok(timeout_ms) => timeout_ms,
            Err(()) => {
                return AdapterResponse {
                    ok: false,
                    result: json!({
                        "code": "invalid_request",
                        "stage": "timeout",
                    }),
                    error_message: Some("invalid_request:timeout".to_owned()),
                };
            }
        };
        let prompt = call
            .params
            .get("text")
            .or_else(|| call.params.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let execution = (self.protocol.execute)(&DriverInvocation {
            executable: &executable,
            params: &call.params,
            prompt,
            session_id,
            cwd: cwd.as_deref(),
            timeout_ms,
            max_stdout: Some(DRIVER_MAX_STDOUT),
            max_stderr: DRIVER_MAX_STDERR,
        });
        map_driver_result(execution)
    }

    fn run_lane(&self, operation: &str, call: &AdapterCall, session_id: &str) -> AdapterResponse {
        let mut params = call.params.clone();
        if params.get("agent").and_then(Value::as_str).is_none() {
            params["agent"] = json!(self.agent_id());
        }
        if params.get("sessionId").and_then(Value::as_str).is_none() && !session_id.is_empty() {
            params["sessionId"] = json!(session_id);
        }
        match (self.lane.dispatch)(&self.port, operation, &params) {
            Ok(value) => {
                if value.get("ok").and_then(Value::as_bool) == Some(true) {
                    AdapterResponse::ok(value)
                } else {
                    AdapterResponse {
                        ok: false,
                        result: value.clone(),
                        error_message: Some(
                            value
                                .get("status")
                                .and_then(Value::as_str)
                                .unwrap_or("lane-failed")
                                .to_owned(),
                        ),
                    }
                }
            }
            Err(_) => AdapterResponse::err("lane-dispatch-failed"),
        }
    }
}

impl AdapterTransport for HostDriverTransport {
    fn invoke(&self, call: &AdapterCall) -> AdapterResponse {
        self.invocations.fetch_add(1, Ordering::SeqCst);
        if resolve_dispatch_timeout(&call.params).is_err() {
            return AdapterResponse {
                ok: false,
                result: json!({
                    "code": "invalid_request",
                    "stage": "timeout",
                }),
                error_message: Some("invalid_request:timeout".to_owned()),
            };
        }
        let session_id = call
            .params
            .get("threadId")
            .or_else(|| call.params.get("sessionId"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let response = match call.method {
            "thread/resume" | "session/resume" | "thread/start" | "session/new"
            | "thread/unarchive" => self.run_driver(call, session_id),
            "steer" | "turn/steer" | "session/steer" => self.run_lane("steer", call, session_id),
            "cancel" | "turn/interrupt" | "session/cancel" => {
                self.run_lane("cancel", call, session_id)
            }
            _ => AdapterResponse::err(format!("unsupported-method:{}", call.method)),
        };
        if response.ok {
            self.mark_supported(call.method);
        }
        response
    }

    fn invocation_count(&self) -> u64 {
        self.invocations.load(Ordering::SeqCst)
    }

    fn negotiated_capabilities(&self) -> Option<NativeCapabilitySnapshot> {
        lock(&self.capabilities).clone()
    }
}

fn map_driver_result(execution: DriverExecution) -> AdapterResponse {
    if execution.ok {
        return AdapterResponse::ok(json!({
            "thread": { "id": execution.thread_id },
            "sessionId": execution.session_id,
            "output": execution.output,
        }));
    }
    let DriverFailure {
        code,
        stage,
        message,
        identity,
    } = execution.failure.unwrap_or(DriverFailure {
        code: "adapter-failed",
        stage: "process",
        message: "The adapter driver failed.",
        identity: None,
    });
    AdapterResponse {
        ok: false,
        result: json!({
            "code": code,
            "stage": stage,
            "threadId": identity,
        }),
        error_message: Some(format!("{code}:{stage}:{message}")),
    }
}

fn call_cwd(call: &AdapterCall) -> Option<PathBuf> {
    call.params
        .get("workingDirectory")
        .or_else(|| call.params.get("cwd"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
