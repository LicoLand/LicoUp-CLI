use super::approval_wait::{ApprovalWaitOutcome, await_external_approval};
use super::capabilities::{AcpSessionDriverSpec, PROCESS_POLL_INTERVAL, RunResult, timestamp};
use super::command::ProtocolConfig;
use super::continuity::{
    ManagedTransport, acquire_transport, handle_control_requests, register_session,
    remove_transport, set_active_session,
};
use super::errors::{ProtocolFailure, failure_requires_transport_reset};
use super::events::ConversationTransportEvent as TransportEvent;
use super::io::write_message;
use super::protocol::{ProtocolEffect, ProtocolOutcome, SessionProtocol};
use super::supervision::PersistentTransport;
use licoup_agent_targets::platform::virtual_machine::SshRuntimeConnection;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

pub fn execute(
    driver: AcpSessionDriverSpec,
    executable: &str,
    runtime_connection: Option<&SshRuntimeConnection>,
    params: &Value,
    prompt: &str,
    session_id: &str,
    cwd: Option<&Path>,
    timeout_ms: u64,
    max_stdout: Option<usize>,
    max_stderr: usize,
) -> RunResult {
    let started_at = timestamp();
    let mut config = match ProtocolConfig::from_params(params, prompt, session_id, cwd) {
        Ok(config) => config,
        Err(failure) => return RunResult::failed(failure, started_at, None, false, false),
    };
    if runtime_connection.is_none()
        && let Err(failure) = config.load_collaboration_mcp(driver.driver_id)
    {
        return RunResult::failed(failure, started_at, None, false, false);
    }
    let managed = match acquire_transport(
        driver,
        executable,
        Path::new(&config.cwd),
        timeout_ms,
        max_stdout,
        max_stderr,
        runtime_connection,
    ) {
        Ok(managed) => managed,
        Err(failure) => return RunResult::failed(failure, started_at, None, false, false),
    };
    let mut transport = match managed.transport.lock() {
        Ok(transport) => transport,
        Err(_) => {
            remove_transport(&managed, false);
            return RunResult::failed(
                ProtocolFailure::new(
                    "hermes_acp_supervisor_unavailable",
                    "Hermes ACP supervisor state is unavailable.",
                    "process/supervisor",
                ),
                started_at,
                None,
                false,
                false,
            );
        }
    };
    let raw_scope = match transport.initial_raw_observer.take() {
        Some(guard) => guard.rebind_current(),
        None => transport.raw_observer.bind_current(),
    };
    let mut protocol = SessionProtocol::new_ready(config, driver.driver_id);
    // An empty value is an internal "session/open in progress" marker. It lets
    // cleanup interrupt a new turn before Hermes has returned its native ID.
    set_active_session(&managed, Some(protocol.config.requested_session_id.clone()));
    let initial_write = protocol.session_request().and_then(|request| {
        write_message(&mut transport.stdin, &request).map_err(|_| {
            protocol.failure_with_ids(
                "hermes_acp_write_failed",
                "Hermes ACP stopped accepting protocol messages.",
                "protocol/write",
            )
        })
    });
    let (outcome, failure, stdout_was_truncated) = if let Err(failure) = initial_write {
        (None, Some(failure), false)
    } else {
        let deadline = if timeout_ms == 0 {
            None
        } else {
            Some(Instant::now() + Duration::from_millis(timeout_ms))
        };
        run_protocol_loop(
            &mut transport,
            &managed,
            &mut protocol,
            deadline,
            max_stdout,
        )
    };
    let stderr_was_truncated = transport.stderr_truncated.load(Ordering::Relaxed);
    drop(raw_scope);
    set_active_session(&managed, None);
    let reset_transport = failure
        .as_ref()
        .is_some_and(failure_requires_transport_reset);
    drop(transport);

    if let Some(outcome) = outcome {
        register_session(&outcome.session_id, &managed);
        return RunResult {
            ok: true,
            output: outcome.output,
            events: outcome.events,
            error: None,
            thread_id: outcome.session_id.clone(),
            session_id: outcome.session_id,
            turn_id: outcome.turn_id,
            turn_status: outcome.turn_status,
            effective: outcome.effective,
            status_code: None,
            stdout_truncated: stdout_was_truncated,
            stderr_truncated: stderr_was_truncated,
            started_at,
        };
    }
    let failure = failure.unwrap_or_else(|| {
        ProtocolFailure::new(
            "hermes_acp_failed",
            "Hermes ACP did not complete the request.",
            "protocol",
        )
    });
    if reset_transport {
        remove_transport(&managed, true);
    }
    RunResult::failed(
        failure,
        started_at,
        None,
        stdout_was_truncated,
        stderr_was_truncated,
    )
}

fn run_protocol_loop(
    transport: &mut PersistentTransport,
    managed: &Arc<ManagedTransport>,
    protocol: &mut SessionProtocol,
    deadline: Option<Instant>,
    max_stdout: Option<usize>,
) -> (Option<ProtocolOutcome>, Option<ProtocolFailure>, bool) {
    let mut observed_bytes = 0usize;
    loop {
        if let Some(failure) = handle_control_requests(transport, protocol) {
            return (None, Some(failure), false);
        }
        if transport.stdin.check_health().is_err() {
            return (
                None,
                Some(protocol.failure_with_ids(
                    "hermes_acp_write_failed",
                    "Hermes ACP stopped accepting protocol messages.",
                    "protocol/write",
                )),
                false,
            );
        }
        let now = Instant::now();
        if deadline.is_some_and(|deadline| now >= deadline) {
            let mut failure = protocol.failure_with_ids(
                "hermes_acp_timeout",
                "Hermes ACP timed out before the turn completed.",
                "turn/wait",
            );
            failure.turn_status = Some("timeout".to_string());
            return (None, Some(failure), false);
        }
        let wait = deadline
            .map(|deadline| (deadline - now).min(PROCESS_POLL_INTERVAL))
            .unwrap_or(PROCESS_POLL_INTERVAL);
        match transport.receiver.recv_timeout(wait) {
            Ok(TransportEvent::Frame { line, bytes }) => {
                if let Some(max_stdout) = max_stdout {
                    observed_bytes = observed_bytes.saturating_add(bytes);
                    if observed_bytes > max_stdout {
                        return (
                            None,
                            Some(protocol.failure_with_ids(
                                "hermes_acp_output_limit",
                                "Hermes ACP exceeded the configured protocol output limit.",
                                "protocol/read",
                            )),
                            true,
                        );
                    }
                }
                for effect in protocol.handle_frame(&line).effects {
                    match effect {
                        ProtocolEffect::Send(message) => {
                            if write_message(&mut transport.stdin, &message).is_err() {
                                return (
                                    None,
                                    Some(protocol.failure_with_ids(
                                        "hermes_acp_write_failed",
                                        "Hermes ACP stopped accepting protocol messages.",
                                        "protocol/write",
                                    )),
                                    false,
                                );
                            }
                        }
                        ProtocolEffect::Complete(outcome) => {
                            return (Some(outcome), None, false);
                        }
                        ProtocolEffect::Fail(failure) => {
                            return (None, Some(failure), false);
                        }
                        ProtocolEffect::AwaitExternalApproval {
                            request_id,
                            display_summary,
                            option_id,
                            requested_tools,
                        } => {
                            match await_external_approval(
                                transport,
                                protocol,
                                &request_id,
                                &display_summary,
                                option_id.as_deref(),
                                &requested_tools,
                                deadline,
                            ) {
                                Ok(ApprovalWaitOutcome::Allowed) => {}
                                Ok(ApprovalWaitOutcome::Denied) => {
                                    // Denial was written; continue until prompt returns cancelled.
                                }
                                Err(failure) => return (None, Some(failure), false),
                            }
                        }
                    }
                }
                if let Ok(mut active) = managed.active_session.lock() {
                    *active = protocol.session_id.clone();
                }
                if let Some(session_id) = protocol.session_id.as_deref() {
                    register_session(session_id, managed);
                }
            }
            Ok(TransportEvent::LineLimitExceeded) => {
                return (
                    None,
                    Some(ProtocolFailure::new(
                        "hermes_acp_output_limit",
                        "Hermes ACP exceeded the configured protocol output limit.",
                        "protocol/read",
                    )),
                    true,
                );
            }
            Ok(TransportEvent::StdoutReadFailed) => {
                return (
                    None,
                    Some(ProtocolFailure::new(
                        "hermes_acp_read_failed",
                        "Hermes ACP protocol output could not be read.",
                        "protocol/read",
                    )),
                    false,
                );
            }
            Ok(TransportEvent::StdoutClosed) | Err(RecvTimeoutError::Disconnected) => {
                return (
                    None,
                    Some(protocol.failure_with_ids(
                        "hermes_acp_exited",
                        "Hermes ACP exited before the turn completed.",
                        "process/exit",
                    )),
                    false,
                );
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}
