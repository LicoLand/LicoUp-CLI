//! What runs the Harness declaration: the SDK transport and the turn it carries.
//!
//! The Harness SDK's wire vocabulary is [`crate::parser`]'s, and this module is
//! the process half that speaks it: the `--profile sdk` transport the Harness
//! itself starts, the supervised turn over it, the bounded frame reader, the
//! projection of one finished turn, and the cleanup that folds one session's
//! transport back down. Nothing here declares a frame: a request is built by
//! [`crate::parser`], a line is classified there and a turn is settled there, so
//! the protocol has exactly one owner and this half cannot drift from it.
//!
//! The transport pool is this package's own and is keyed by the caller's session
//! identity, because the Harness records every session durably and refuses to
//! re-admit a recorded identity in a later process. One transport therefore
//! carries the sessions admitted while it lives, and cleanup names one session.
//!
//! The one fact this module cannot derive is the environment a launch observes,
//! which belongs to the host; it is asked for through
//! [`crate::port::launch_environment`] rather than read from a second copy of
//! the login-shell rules.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use licoup_agent_adapter_sdk::Transition;
use licoup_agent_adapter_sdk::adapters::NativeLineParser;
use licoup_agent_adapter_sdk::adapters::driver_registry::{
    registry_get, registry_insert_if_absent, registry_remove, registry_remove_if,
};
use licoup_foundation::platform::process_supervisor::SupervisedChild;
use licoup_foundation::platform::raw_execution::{
    RawExecutionBinding, RawExecutionBindingGuard, RawExecutionDirection, RawExecutionObserver,
    RawExecutionReader,
};
use serde_json::Value;

use crate::parser::{
    FrameError, FrameParser, ProtocolFrame, TurnParseError, TurnParser, encode_request,
    initialize_accepted, initialize_request, prompt_request, shutdown_request,
};

#[cfg(test)]
mod tests;

pub const DRIVER_ID: &str = "deepseek-harness-sdk-jsonrpc";
pub const RUNTIME_PROTOCOL: &str = "deepseek-harness-sdk-stdio-jsonrpc";
const MAX_TRANSPORTS: usize = 8;
const REGISTRY_NAMESPACE: &str = "deepseek-harness-transport";

#[derive(Clone, Debug, Default)]
pub struct EffectiveSettings {
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub permission_mode: Option<String>,
    pub sandbox: Option<Value>,
    pub approval_policy: Option<Value>,
}

#[derive(Clone, Debug)]
pub struct ProtocolFailure {
    code: &'static str,
    message: &'static str,
    stage: &'static str,
}

impl ProtocolFailure {
    fn new(code: &'static str, message: &'static str, stage: &'static str) -> Self {
        Self {
            code,
            message,
            stage,
        }
    }

    pub fn into_payload(self) -> ProtocolFailurePayload {
        ProtocolFailurePayload {
            code: self.code,
            message: self.message,
            stage: self.stage,
            user_interaction_required: false,
            request_method: None,
            session_id: None,
            thread_id: None,
            turn_id: None,
            turn_status: None,
        }
    }
}

pub struct ProtocolFailurePayload {
    pub code: &'static str,
    pub message: &'static str,
    pub stage: &'static str,
    pub user_interaction_required: bool,
    pub request_method: Option<String>,
    pub session_id: Option<String>,
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub turn_status: Option<String>,
}

#[derive(Debug)]
pub struct RunResult {
    pub ok: bool,
    pub output: String,
    pub transitions: Vec<Transition>,
    pub error: Option<ProtocolFailure>,
    pub session_id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub turn_status: String,
    pub effective: EffectiveSettings,
    pub status_code: Option<i32>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub started_at: String,
}

impl RunResult {
    fn failed(failure: ProtocolFailure, started_at: String) -> Self {
        let transitions =
            crate::parser::failure_transitions(failure.code, failure.stage, failure.message);
        Self {
            ok: false,
            output: String::new(),
            transitions,
            error: Some(failure),
            session_id: String::new(),
            thread_id: String::new(),
            turn_id: String::new(),
            turn_status: "failed".to_string(),
            effective: EffectiveSettings::default(),
            status_code: None,
            stdout_truncated: false,
            stderr_truncated: false,
            started_at,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TransportConfig {
    executable: String,
    cwd: PathBuf,
    provider: String,
    model: String,
    reasoning_effort: Option<String>,
    max_tokens: Option<u64>,
    output_limit: Option<usize>,
    stderr_limit: usize,
}

struct ManagedTransport {
    config: TransportConfig,
    state: Mutex<Option<TransportState>>,
}

struct TransportState {
    child: SupervisedChild,
    stdin: ChildStdin,
    receiver: mpsc::Receiver<std::result::Result<ProtocolFrame, FrameError>>,
    next_request_id: u64,
    /// Session identity admitted by the harness process. The SDK runtime
    /// records every session durably and refuses to re-admit a recorded
    /// identity in a later process, so each spawned transport claims its own
    /// derivation of the caller's session id; continuity holds for exactly
    /// the transport's lifetime.
    harness_session_id: String,
    raw_execution: RawExecutionBinding,
    initial_raw_execution: Option<RawExecutionBindingGuard>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CleanupDisposition {
    Accepted,
    SessionUnavailable,
    Unavailable,
}

pub fn execute(
    executable: &str,
    params: &Value,
    prompt: &str,
    session_id: &str,
    cwd: Option<&Path>,
    timeout_ms: u64,
    max_stdout: Option<usize>,
    max_stderr: usize,
) -> RunResult {
    let started_at = timestamp();
    if params
        .get("privateInstructions")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
    {
        return RunResult::failed(
            failure(
                "deepseek_harness_private_instructions_unsupported",
                "DeepSeek Harness SDK does not expose a separate private-instruction channel.",
                "params/privateInstructions",
            ),
            started_at,
        );
    }
    let Some(cwd) = cwd.filter(|path| path.is_absolute()) else {
        return RunResult::failed(
            failure(
                "deepseek_harness_absolute_cwd_required",
                "DeepSeek Harness requires an absolute workspace directory.",
                "params/cwd",
            ),
            started_at,
        );
    };
    let Some(model) = params
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return RunResult::failed(
            failure(
                "deepseek_harness_model_required",
                "DeepSeek Harness requires an explicit official model id.",
                "params/model",
            ),
            started_at,
        );
    };
    let provider = params
        .get("provider")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("deepseek-official");
    let session_id = if session_id.trim().is_empty() {
        uuid::Uuid::new_v4().to_string()
    } else {
        session_id.trim().to_string()
    };
    let config = TransportConfig {
        executable: executable.to_string(),
        cwd: cwd.to_path_buf(),
        provider: provider.to_string(),
        model: model.to_string(),
        reasoning_effort: params
            .get("reasoningEffort")
            .or_else(|| params.get("effort"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        max_tokens: params
            .get("maxTokens")
            .and_then(Value::as_u64)
            .filter(|value| *value > 0),
        output_limit: max_stdout,
        stderr_limit: max_stderr,
    };
    let deadline = (timeout_ms != 0).then(|| Instant::now() + Duration::from_millis(timeout_ms));
    let transport = match transport_for_session(&session_id, &config, deadline) {
        Ok(transport) => transport,
        Err(error) => return RunResult::failed(error, started_at),
    };
    let outcome = {
        let mut state = match transport.state.lock() {
            Ok(state) => state,
            Err(_) => {
                evict_transport(&session_id, &transport);
                return RunResult::failed(transport_unavailable(), started_at);
            }
        };
        match state.as_mut() {
            Some(state) => {
                let _raw_execution = match state.initial_raw_execution.take() {
                    Some(guard) => guard.rebind_current(),
                    None => state.raw_execution.bind_current(),
                };
                execute_turn(state, prompt, &session_id, config.output_limit, deadline)
            }
            None => Err(transport_unavailable()),
        }
    };
    let parsed = match outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            evict_transport(&session_id, &transport);
            shutdown_transport(&transport);
            return RunResult::failed(error, started_at);
        }
    };
    RunResult {
        ok: true,
        transitions: parsed.transitions,
        output: parsed.output,
        error: None,
        session_id: session_id.clone(),
        thread_id: session_id,
        turn_id: parsed.turn_id,
        turn_status: "completed".to_string(),
        effective: EffectiveSettings {
            cwd: Some(config.cwd.to_string_lossy().into_owned()),
            model: Some(config.model),
            reasoning_effort: config.reasoning_effort,
            ..EffectiveSettings::default()
        },
        status_code: None,
        stdout_truncated: false,
        stderr_truncated: false,
        started_at,
    }
}

pub fn cleanup_session(session_id: &str) -> CleanupDisposition {
    let transport = registry_remove::<Arc<ManagedTransport>>(REGISTRY_NAMESPACE, session_id.trim());
    let Some(transport) = transport else {
        return CleanupDisposition::SessionUnavailable;
    };
    if shutdown_transport(&transport) {
        CleanupDisposition::Accepted
    } else {
        CleanupDisposition::Unavailable
    }
}

fn transport_for_session(
    session_id: &str,
    config: &TransportConfig,
    deadline: Option<Instant>,
) -> std::result::Result<Arc<ManagedTransport>, ProtocolFailure> {
    if let Some(transport) = registry_get::<Arc<ManagedTransport>>(REGISTRY_NAMESPACE, session_id) {
        if transport.config != *config {
            return Err(failure(
                "deepseek_harness_session_config_changed",
                "DeepSeek Harness cannot resume a session after its executable or initialization settings changed.",
                "session/config",
            ));
        }
        return Ok(transport);
    }
    let harness_session_id = process_scoped_session_id(&session_id);
    let transport = Arc::new(spawn_transport(config, deadline, harness_session_id)?);
    let inserted = registry_insert_if_absent(
        REGISTRY_NAMESPACE,
        session_id,
        Arc::clone(&transport),
        MAX_TRANSPORTS,
    );
    match inserted {
        Ok(Ok(())) => Ok(transport),
        Ok(Err(existing)) => {
            shutdown_transport(&transport);
            if existing.config == *config {
                Ok(existing)
            } else {
                Err(failure(
                    "deepseek_harness_session_config_changed",
                    "DeepSeek Harness cannot resume a session after its executable or initialization settings changed.",
                    "session/config",
                ))
            }
        }
        Err(()) => {
            shutdown_transport(&transport);
            Err(failure(
                "deepseek_harness_transport_capacity_exceeded",
                "The bounded DeepSeek Harness transport pool is full.",
                "process/capacity",
            ))
        }
    }
}

fn spawn_transport(
    config: &TransportConfig,
    deadline: Option<Instant>,
    harness_session_id: String,
) -> std::result::Result<ManagedTransport, ProtocolFailure> {
    let mut command = Command::new(&config.executable);
    crate::port::launch_environment::apply_to_command(&mut command);
    command
        .args(["--profile", "sdk"])
        .current_dir(&config.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = SupervisedChild::spawn(&mut command).map_err(|_| {
        failure(
            "deepseek_harness_jsonrpc_carrier_unavailable",
            "The official DeepSeek Harness JSON-RPC carrier is unavailable.",
            "process/start",
        )
    })?;
    let Some(mut stdin) = child.stdin() else {
        let _ = child.terminate_tree();
        return Err(transport_unavailable());
    };
    let Some(stdout) = child.stdout() else {
        let _ = child.terminate_tree();
        return Err(transport_unavailable());
    };
    let raw_execution = RawExecutionBinding::default();
    let initial_raw_execution = Some(raw_execution.bind_current());
    if let Some(mut stderr) = child.stderr() {
        let stderr_observer = raw_execution.clone();
        std::thread::spawn(move || {
            // Drain to EOF; only the invocation-owned local viewer retains it.
            let mut bytes = [0u8; 8192];
            loop {
                match stderr.read(&mut bytes) {
                    Ok(0) => break,
                    Ok(read) => stderr_observer.record_bytes(
                        "deepseek-harness",
                        RawExecutionDirection::Stderr,
                        &bytes[..read],
                    ),
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        });
    }
    let (sender, receiver) = mpsc::channel();
    let output_limit = config.output_limit;
    let stdout = RawExecutionReader::new(
        stdout,
        raw_execution.clone(),
        "deepseek-harness",
        RawExecutionDirection::Received,
    );
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let frame = match read_protocol_frame(&mut reader, output_limit) {
                Ok(Some(frame)) => frame,
                Ok(None) => break,
                Err(failure) => {
                    let _ = sender.send(Err(failure));
                    return;
                }
            };
            if sender.send(Ok(frame)).is_err() {
                return;
            }
        }
    });
    let initialize = initialize_request(
        &config.cwd.to_string_lossy(),
        &config.provider,
        &config.model,
        config.reasoning_effort.as_deref(),
        config.max_tokens,
    );
    if write_frame(&mut stdin, &initialize).is_err() {
        let _ = child.terminate_tree();
        return Err(failure(
            "deepseek_harness_transport_write_failed",
            "DeepSeek Harness stopped accepting protocol requests.",
            "protocol/write",
        ));
    }
    let initialized = loop {
        let Some(Ok(frame)) = next_frame(&receiver, deadline) else {
            break false;
        };
        if let Some(accepted) = initialize_accepted(&frame) {
            break accepted;
        }
    };
    if !initialized {
        let _ = child.terminate_tree();
        return Err(failure(
            "deepseek_harness_initialize_failed",
            "DeepSeek Harness rejected the fixed SDK handshake.",
            "protocol/initialize",
        ));
    }
    Ok(ManagedTransport {
        config: config.clone(),
        state: Mutex::new(Some(TransportState {
            child,
            stdin,
            receiver,
            next_request_id: 1,
            harness_session_id,
            raw_execution,
            initial_raw_execution,
        })),
    })
}

fn execute_turn(
    state: &mut TransportState,
    prompt: &str,
    session_id: &str,
    output_limit: Option<usize>,
    deadline: Option<Instant>,
) -> std::result::Result<crate::parser::TurnResult, ProtocolFailure> {
    let request_id = format!("prompt-{}", state.next_request_id);
    state.next_request_id = state.next_request_id.saturating_add(1);
    let request = prompt_request(&request_id, &state.harness_session_id, prompt);
    write_frame(&mut state.stdin, &request).map_err(|_| {
        failure(
            "deepseek_harness_prompt_failed",
            "DeepSeek Harness did not admit the prompt.",
            "protocol/prompt",
        )
    })?;
    let mut parser = TurnParser::new(&request_id, &state.harness_session_id);
    let mut output_bytes = 0usize;
    loop {
        let frame = next_frame(&state.receiver, deadline).ok_or_else(turn_incomplete)??;
        output_bytes = output_bytes.saturating_add(frame.wire_bytes());
        if output_limit.is_some_and(|limit| output_bytes > limit) {
            return Err(output_limit_exceeded());
        }
        let outcome = parser.ingest(frame);
        for message in parser.take_completed_messages() {
            licoup_foundation::platform::turn_event_emit::emit_agent_message_completed_for_unit(
                session_id,
                &message.turn_id,
                &message.unit_id,
                &message.text,
            );
        }
        match outcome {
            Ok(Some(result)) => return Ok(result),
            Ok(None) => {}
            Err(TurnParseError::Incomplete) => return Err(turn_incomplete()),
            Err(TurnParseError::PromptRejected) => {
                return Err(failure(
                    "deepseek_harness_prompt_rejected",
                    "DeepSeek Harness rejected the prompt before admission.",
                    "protocol/prompt",
                ));
            }
            Err(TurnParseError::SessionMismatch) => {
                return Err(failure(
                    "deepseek_harness_session_mismatch",
                    "DeepSeek Harness returned protocol activity for a different session.",
                    "protocol/session",
                ));
            }
        }
    }
}

/// Read one newline-delimited frame without allowing `read_line` to allocate
/// past the protocol cap. An oversized frame terminates its transport.
fn read_protocol_frame(
    reader: &mut impl BufRead,
    limit: Option<usize>,
) -> std::result::Result<Option<ProtocolFrame>, FrameError> {
    let mut bytes = Vec::with_capacity(limit.unwrap_or(8192).min(8192));
    loop {
        let available = reader.fill_buf().map_err(|_| FrameError::InvalidJson)?;
        if available.is_empty() {
            if bytes.is_empty() {
                return Ok(None);
            }
            break;
        }
        let take = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if limit.is_some_and(|limit| bytes.len().saturating_add(take) > limit) {
            return Err(FrameError::OutputLimit);
        }
        bytes.extend_from_slice(&available[..take]);
        reader.consume(take);
        if bytes.last() == Some(&b'\n') {
            break;
        }
    }
    while bytes
        .last()
        .is_some_and(|byte| matches!(*byte, b'\n' | b'\r'))
    {
        bytes.pop();
    }
    FrameParser.parse_line(&bytes).map(Some)
}

fn write_frame(stdin: &mut impl Write, value: &Value) -> std::io::Result<()> {
    let encoded = encode_request(value).map_err(std::io::Error::other)?;
    if let Some(observer) = RawExecutionObserver::current() {
        observer.record_bytes("deepseek-harness", RawExecutionDirection::Sent, &encoded);
    }
    stdin.write_all(&encoded)?;
    stdin.flush()
}

fn next_frame(
    receiver: &mpsc::Receiver<std::result::Result<ProtocolFrame, FrameError>>,
    deadline: Option<Instant>,
) -> Option<std::result::Result<ProtocolFrame, ProtocolFailure>> {
    let result = match deadline {
        Some(deadline) => receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .ok(),
        None => receiver.recv().ok(),
    }?;
    Some(result.map_err(|kind| match kind {
        FrameError::InvalidJson => failure(
            "deepseek_harness_invalid_json",
            "DeepSeek Harness emitted an invalid protocol frame.",
            "protocol/output",
        ),
        FrameError::OutputLimit => output_limit_exceeded(),
    }))
}

fn evict_transport(session_id: &str, expected: &Arc<ManagedTransport>) {
    registry_remove_if::<Arc<ManagedTransport>>(REGISTRY_NAMESPACE, session_id, |current| {
        Arc::ptr_eq(current, expected)
    });
}

fn shutdown_transport(transport: &ManagedTransport) -> bool {
    let Ok(mut state) = transport.state.lock() else {
        return false;
    };
    let Some(mut state) = state.take() else {
        return true;
    };
    let wrote = write_frame(&mut state.stdin, &shutdown_request()).is_ok();
    drop(state.stdin);
    let terminated = state
        .child
        .finish_or_terminate_tree(Duration::from_millis(250))
        .is_ok();
    let _ = wrote;
    terminated
}

fn failure(code: &'static str, message: &'static str, stage: &'static str) -> ProtocolFailure {
    ProtocolFailure::new(code, message, stage)
}
/// Derive the session identity a freshly spawned harness process will record.
/// Keeping the caller's id as the prefix preserves the visible link between a
/// conversation and its harness session records.
fn process_scoped_session_id(session_id: &str) -> String {
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    format!("{session_id}-{}", &nonce[..8])
}
fn transport_unavailable() -> ProtocolFailure {
    failure(
        "deepseek_harness_transport_unavailable",
        "The supervised DeepSeek Harness transport is unavailable.",
        "protocol/transport",
    )
}
fn turn_incomplete() -> ProtocolFailure {
    failure(
        "deepseek_harness_turn_incomplete",
        "DeepSeek Harness closed before the admitted activity reached idle.",
        "protocol/terminal",
    )
}
fn output_limit_exceeded() -> ProtocolFailure {
    failure(
        "deepseek_harness_output_limit_exceeded",
        "DeepSeek Harness exceeded the bounded protocol output limit.",
        "protocol/output",
    )
}
fn timestamp() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}
