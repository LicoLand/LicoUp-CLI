use super::io::{TransportEvent, drain_stderr, read_protocol_messages, write_message};
use super::supervision::{LaunchSpec, resolve_gateway_endpoint};
use crate::gateway_acp::errors::ProtocolFailure;
use crate::gateway_acp::model::{PROCESS_POLL_INTERVAL, RunResult};
use crate::gateway_acp::params::ProtocolConfig;
use crate::parser::protocol::{OpenClawProtocol, ProtocolEffect, ProtocolOutcome, ProtocolPhase};
use crate::policy::attach_mode;
use crate::port::turn_event;
use licoup_agent_drivers::acp_driver_runtime::ActiveAcpControl;
use licoup_agent_targets::platform::virtual_machine::SshRuntimeConnection;
use licoup_foundation::platform::process_supervisor::{
    BoundedStdinWriter, SupervisedChild, TransportFinishFailure, finish_protocol_transport,
};
use licoup_foundation::platform::raw_execution::{
    RawExecutionBinding, RawExecutionDirection, RawExecutionObserver, RawExecutionReader,
    RawExecutionScope,
};
use serde_json::{Value, json};
use std::io::{self, BufReader};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Run one OpenClaw turn without a runtime connection and with no local MCP
/// registration.
///
/// It is this package's own suite entry: the protocol fixtures below exercise
/// framing rather than installation-backed registration, so the host's one
/// question is answered with an empty list at the call site instead of through a
/// second composition.
#[cfg(test)]
pub(super) fn execute(
    executable: &str,
    params: &Value,
    prompt: &str,
    session_id: &str,
    cwd: Option<&Path>,
    timeout_ms: u64,
    max_stdout: Option<usize>,
    max_stderr: usize,
) -> RunResult {
    execute_with_connection(
        executable,
        None,
        || Ok(Vec::new()),
        params,
        prompt,
        session_id,
        cwd,
        timeout_ms,
        max_stdout,
        max_stderr,
    )
}

/// Run one OpenClaw turn against the Gateway.
///
/// `local_mcp` is the caller's answer to the one question this package may not
/// decide: *which* MCP servers the turn registers. It is a lazy argument rather
/// than a port so both halves stay visible at the one call site that needs it,
/// and it runs only after every request field has been accepted — a malformed
/// request still reports its own typed failure. A turn that attaches through a
/// runtime connection carries its own registration, so the argument is not
/// consulted at all on that branch.
pub fn execute_with_connection(
    executable: &str,
    runtime_connection: Option<&SshRuntimeConnection>,
    local_mcp: impl FnOnce() -> Result<Vec<Value>, ProtocolFailure>,
    params: &Value,
    prompt: &str,
    session_id: &str,
    cwd: Option<&Path>,
    timeout_ms: u64,
    max_stdout: Option<usize>,
    max_stderr: usize,
) -> RunResult {
    let started_at = timestamp();
    let config = match if runtime_connection.is_some() {
        ProtocolConfig::from_params_without_local_mcp(params, prompt, session_id, cwd)
    } else {
        // The caller answers this package's MCP question from its own plugin
        // configuration; the package calls it only after the request itself has
        // been accepted.
        ProtocolConfig::from_params(params, prompt, session_id, cwd, local_mcp)
    } {
        Ok(config) => config,
        Err(failure) => return RunResult::failed(failure, started_at, None, false, false),
    };
    let launch = if let Some(runtime_connection) = runtime_connection {
        // The stream frame contract requires a non-empty session identity.
        // A fresh send has no native session key yet, so the attach event is
        // emitted only when one is already bound (resume or explicit key);
        // fresh sends are correlated by the later `dispatch.turn.bound`.
        if let Some(session_key) = config.native_session_key.as_deref() {
            turn_event::emit_turn_event(
                "dispatch.gateway.attached",
                session_key,
                &config.turn_id,
                json!({
                    "wsUrlHostClass": "virtual-machine",
                    "attachMode": "ssh-stdio"
                }),
            );
        }
        LaunchSpec::for_virtual_machine(runtime_connection, Path::new(&config.cwd))
    } else {
        let gateway = match resolve_gateway_endpoint(executable, params) {
            Ok(endpoint) => endpoint,
            Err(failure) => return RunResult::failed(failure, started_at, None, false, false),
        };
        if let Some(session_key) = config.native_session_key.as_deref() {
            turn_event::emit_turn_event(
                "dispatch.gateway.attached",
                session_key,
                &config.turn_id,
                json!({
                    "wsUrlHostClass": "loopback",
                    "port": gateway.port,
                    "attachMode": attach_mode(gateway.port)
                }),
            );
        }
        LaunchSpec::for_gateway_attach(executable, Path::new(&config.cwd), &gateway.ws_url)
    };
    let mut child = match launch.spawn() {
        Ok(child) => child,
        Err(error) => {
            let message = match error.kind() {
                io::ErrorKind::NotFound => "The OpenClaw executable is not available.",
                io::ErrorKind::PermissionDenied => {
                    "The OpenClaw executable is not permitted to run."
                }
                _ => "OpenClaw ACP could not be started against the Gateway.",
            };
            return RunResult::failed(
                ProtocolFailure::new("openclaw_acp_start_failed", message, "process/start"),
                started_at,
                None,
                false,
                false,
            );
        }
    };
    let Some(stdout) = child.stdout() else {
        return pipe_failure(
            &mut child,
            started_at,
            "OpenClaw ACP stdout is unavailable.",
        );
    };
    let Some(stderr) = child.stderr() else {
        return pipe_failure(
            &mut child,
            started_at,
            "OpenClaw ACP stderr is unavailable.",
        );
    };
    let Some(stdin) = child.stdin() else {
        return pipe_failure(&mut child, started_at, "OpenClaw ACP stdin is unavailable.");
    };
    let mut stdin = BoundedStdinWriter::new(stdin);

    let (sender, receiver) = mpsc::channel();
    let stdout_observer = RawExecutionObserver::current();
    let stdout_handle = thread::spawn(move || {
        let binding = RawExecutionBinding::default();
        let _guard = binding.bind(stdout_observer);
        let stdout =
            RawExecutionReader::new(stdout, binding, "openclaw", RawExecutionDirection::Received);
        read_protocol_messages(BufReader::new(stdout), max_stdout, sender)
    });
    let stderr_truncated = Arc::new(AtomicBool::new(false));
    let stderr_flag = Arc::clone(&stderr_truncated);
    let stderr_observer = RawExecutionObserver::current();
    let stderr_handle = thread::spawn(move || {
        let _scope = RawExecutionScope::enter(stderr_observer);
        drain_stderr(stderr, max_stderr, &stderr_flag)
    });

    let mut protocol = OpenClawProtocol::new(config);
    let initial_request = match protocol.initial_request() {
        Ok(request) => request,
        Err(failure) => {
            let cleanup =
                finish_protocol_transport(&mut child, &mut stdin, stdout_handle, stderr_handle);
            let cleanup_failed = cleanup == Err(TransportFinishFailure::Lifecycle);
            return RunResult::failed(
                if cleanup_failed {
                    ProtocolFailure::new(
                        "openclaw_acp_cleanup_failed",
                        "OpenClaw ACP process cleanup could not be completed safely.",
                        "process/cleanup",
                    )
                } else {
                    failure
                },
                started_at,
                None,
                false,
                stderr_truncated.load(Ordering::Relaxed),
            );
        }
    };
    if write_message(&mut stdin, &initial_request).is_err() {
        let cleanup =
            finish_protocol_transport(&mut child, &mut stdin, stdout_handle, stderr_handle);
        let cleanup_failed = cleanup == Err(TransportFinishFailure::Lifecycle);
        return RunResult::failed(
            ProtocolFailure::new(
                if cleanup_failed {
                    "openclaw_acp_cleanup_failed"
                } else {
                    "openclaw_acp_write_failed"
                },
                if cleanup_failed {
                    "OpenClaw ACP process cleanup could not be completed safely."
                } else {
                    "OpenClaw ACP stopped accepting protocol messages."
                },
                if cleanup_failed {
                    "process/cleanup"
                } else {
                    "initialize"
                },
            ),
            started_at,
            None,
            false,
            stderr_truncated.load(Ordering::Relaxed),
        );
    }

    // timeoutMs 0 opts out of any turn deadline (see runtime_adapters/dispatch),
    // so only a non-zero window gets a concrete deadline.
    let deadline = (timeout_ms != 0).then(|| Instant::now() + Duration::from_millis(timeout_ms));
    let mut active_control = ActiveAcpControl::new("openclaw-acp");
    let (outcome, failure, status_code, stdout_was_truncated) = run_protocol_loop(
        &mut stdin,
        &receiver,
        &mut protocol,
        &mut active_control,
        deadline,
    );

    let cleanup = finish_protocol_transport(&mut child, &mut stdin, stdout_handle, stderr_handle);
    let stderr_was_truncated = stderr_truncated.load(Ordering::Relaxed);

    if cleanup == Err(TransportFinishFailure::Lifecycle) {
        return RunResult::failed(
            ProtocolFailure::new(
                "openclaw_acp_cleanup_failed",
                "OpenClaw ACP process cleanup could not be completed safely.",
                "process/cleanup",
            ),
            started_at,
            status_code,
            stdout_was_truncated,
            stderr_was_truncated,
        );
    }
    if outcome.is_some() && cleanup == Err(TransportFinishFailure::StdinWrite) {
        return RunResult::failed(
            ProtocolFailure::new(
                "openclaw_acp_write_failed",
                "OpenClaw ACP stopped accepting protocol messages.",
                "protocol/write",
            ),
            started_at,
            status_code,
            stdout_was_truncated,
            stderr_was_truncated,
        );
    }

    if let Some(outcome) = outcome {
        return RunResult {
            ok: true,
            output: outcome.output,
            error: None,
            thread_id: outcome.session_id.clone(),
            session_id: outcome.session_id,
            turn_id: outcome.turn_id,
            turn_status: outcome.turn_status,
            effective: outcome.effective,
            transitions: outcome.transitions,
            status_code,
            stdout_truncated: stdout_was_truncated,
            stderr_truncated: stderr_was_truncated,
            started_at,
        };
    }
    RunResult::failed(
        failure.unwrap_or_else(|| {
            ProtocolFailure::new(
                "openclaw_acp_failed",
                "OpenClaw ACP did not complete the request.",
                "protocol",
            )
        }),
        started_at,
        status_code,
        stdout_was_truncated,
        stderr_was_truncated,
    )
}

pub(super) fn run_protocol_loop(
    stdin: &mut BoundedStdinWriter,
    receiver: &Receiver<TransportEvent>,
    protocol: &mut OpenClawProtocol,
    active_control: &mut ActiveAcpControl,
    deadline: Option<Instant>,
) -> (
    Option<ProtocolOutcome>,
    Option<ProtocolFailure>,
    Option<i32>,
    bool,
) {
    loop {
        if active_control
            .sync_binding(protocol.binding.native_id(), protocol.binding.protocol_id())
            .is_err()
            || active_control.poll(stdin).is_err()
        {
            return (
                None,
                Some(protocol.failure_with_ids(
                    "openclaw_acp_control_unavailable",
                    "OpenClaw ACP active-turn control is unavailable.",
                    "turn/control",
                )),
                None,
                false,
            );
        }
        if stdin.check_health().is_err() {
            return (
                None,
                Some(protocol.failure_with_ids(
                    "openclaw_acp_write_failed",
                    "OpenClaw ACP stopped accepting protocol messages.",
                    "protocol/write",
                )),
                None,
                false,
            );
        }
        let now = Instant::now();
        if deadline.is_some_and(|deadline| now >= deadline) {
            let mut failure = protocol.failure_with_ids(
                "openclaw_acp_timeout",
                "OpenClaw ACP timed out before the turn completed.",
                "turn/wait",
            );
            failure.turn_status = Some("timeout".to_string());
            return (None, Some(failure), None, false);
        }
        match receiver.recv_timeout(deadline.map_or(PROCESS_POLL_INTERVAL, |deadline| {
            (deadline - now).min(PROCESS_POLL_INTERVAL)
        })) {
            Ok(TransportEvent::Frame(line)) => {
                let phase_before = protocol.phase;
                for effect in protocol.handle_frame(&line).effects {
                    match effect {
                        ProtocolEffect::Send(message) => {
                            if write_message(stdin, &message).is_err() {
                                return (
                                    None,
                                    Some(protocol.failure_with_ids(
                                        "openclaw_acp_write_failed",
                                        "OpenClaw ACP stopped accepting protocol messages.",
                                        "protocol/write",
                                    )),
                                    None,
                                    false,
                                );
                            }
                        }
                        ProtocolEffect::Complete(outcome) => {
                            return (Some(*outcome), None, None, false);
                        }
                        ProtocolEffect::Fail(failure) => {
                            return (None, Some(failure), None, false);
                        }
                    }
                }
                if phase_before != ProtocolPhase::AwaitPrompt
                    && protocol.phase == ProtocolPhase::AwaitPrompt
                    && let (Some(external_session_id), Some(protocol_session_id)) =
                        (protocol.binding.native_id(), protocol.binding.protocol_id())
                {
                    if active_control
                        .sync_binding(Some(external_session_id), Some(protocol_session_id))
                        .is_err()
                        || active_control.poll(stdin).is_err()
                    {
                        return (
                            None,
                            Some(protocol.failure_with_ids(
                                "openclaw_acp_control_unavailable",
                                "OpenClaw ACP active-turn control is unavailable.",
                                "turn/control",
                            )),
                            None,
                            false,
                        );
                    }
                    turn_event::emit_turn_event(
                        "dispatch.turn.bound",
                        external_session_id,
                        &protocol.config.turn_id,
                        json!({}),
                    );
                }
            }
            Ok(TransportEvent::StdoutLimitExceeded) => {
                return (
                    None,
                    Some(ProtocolFailure::new(
                        "openclaw_acp_output_limit",
                        "OpenClaw ACP exceeded the configured protocol output limit.",
                        "protocol/read",
                    )),
                    None,
                    true,
                );
            }
            Ok(TransportEvent::StdoutReadFailed) => {
                return (
                    None,
                    Some(ProtocolFailure::new(
                        "openclaw_acp_read_failed",
                        "OpenClaw ACP protocol output could not be read.",
                        "protocol/read",
                    )),
                    None,
                    false,
                );
            }
            Ok(TransportEvent::StdoutClosed) | Err(RecvTimeoutError::Disconnected) => {
                return (
                    None,
                    Some(protocol.failure_with_ids(
                        "openclaw_acp_exited",
                        "OpenClaw ACP exited before the turn completed.",
                        "process/exit",
                    )),
                    None,
                    false,
                );
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

pub(super) fn pipe_failure(
    child: &mut SupervisedChild,
    started_at: String,
    message: &'static str,
) -> RunResult {
    let failure = if child.terminate_tree().is_ok() {
        ProtocolFailure::new("openclaw_acp_pipe_failed", message, "process/start")
    } else {
        ProtocolFailure::new(
            "openclaw_acp_cleanup_failed",
            "OpenClaw ACP process cleanup could not be completed safely.",
            "process/cleanup",
        )
    };
    RunResult::failed(failure, started_at, None, false, false)
}

pub(super) fn timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
}
