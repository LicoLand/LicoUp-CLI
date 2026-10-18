use super::approval::{PendingApproval, park_external_approval};
use super::control::ControlRequest;
use super::failure::{ProtocolFailure, supervisor_failure};
use super::io::{TransportEvent, write_message};
use super::model::{PROCESS_POLL_INTERVAL, RunResult};
use super::reset::requires_transport_reset;
use super::supervision::{
    ManagedTransport, bind_session, lookup_session_transport, record_success, remove_transport,
    set_active_session, spawn_transport,
};
use super::transport::PersistentTransport;
use crate::protocol::DriverConfig;
use crate::protocol::parser::{
    ClaudeCodeParser, ClaudeEffect, ProtocolFinishReport, interrupt_request, steer_message,
};
use licoup_agent_adapter_sdk::adapters::NativeLineParser;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{RecvTimeoutError, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
    let config = match DriverConfig::from_params(params, prompt, session_id, cwd) {
        Ok(config) => config,
        Err(failure) => return RunResult::failed(failure, started_at, false, false),
    };
    let managed = if config.requested_session_id.is_empty() {
        match spawn_transport(executable, &config, cwd, max_stderr) {
            Ok(managed) => managed,
            Err(failure) => return RunResult::failed(failure, started_at, false, false),
        }
    } else if let Some(managed) = lookup_session_transport(&config.requested_session_id) {
        if !managed.identity.compatible_with(executable, &config, cwd) {
            // The live process is pinned to its launch configuration (model,
            // effort, permission mode, cwd, allowlist). A configuration change
            // never fails the turn: release the old process and launch a fresh
            // one that resumes the same conversation with the new settings.
            remove_transport(&managed, true);
            match spawn_transport(executable, &config, cwd, max_stderr) {
                Ok(managed) => managed,
                Err(failure) => return RunResult::failed(failure, started_at, false, false),
            }
        } else {
            managed
        }
    } else {
        // No live process owns this conversation in the client process: launch
        // a fresh Claude Code process that resumes the persisted transcript via
        // --resume. The turn loop verifies the returned conversation identity.
        match spawn_transport(executable, &config, cwd, max_stderr) {
            Ok(managed) => managed,
            Err(failure) => return RunResult::failed(failure, started_at, false, false),
        }
    };
    let mut transport = match managed.transport.lock() {
        Ok(transport) => transport,
        Err(_) => {
            remove_transport(&managed, false);
            return RunResult::failed(supervisor_failure(), started_at, false, false);
        }
    };
    let raw_execution = transport.bind_raw_execution();
    let known_session = managed
        .native_session_id
        .lock()
        .ok()
        .and_then(|value| value.clone());
    if !config.requested_session_id.is_empty()
        && known_session
            .as_deref()
            .is_some_and(|known| known != config.requested_session_id.as_str())
    {
        drop(raw_execution);
        drop(transport);
        remove_transport(&managed, true);
        return RunResult::failed(
            ProtocolFailure::new(
                "claude_code_session_mismatch",
                "The live Claude Code process is not bound to the requested conversation.",
                "session/resume",
            )
            .with_session(Some(&config.requested_session_id))
            .with_turn(&config.turn_id),
            started_at,
            false,
            false,
        );
    }
    // A freshly launched resume transport has no bound session yet; the turn
    // loop verifies the CLI returns the requested conversation before binding.
    let expected_session = known_session.clone().or_else(|| {
        (!config.requested_session_id.is_empty()).then(|| config.requested_session_id.clone())
    });
    set_active_session(&managed, expected_session.clone());
    let message = match config.stdin_message() {
        Ok(message) => message,
        Err(_) => {
            set_active_session(&managed, None);
            return RunResult::failed(
                ProtocolFailure::new(
                    "claude_code_input_encode_failed",
                    "Claude Code input could not be encoded.",
                    "request/encode",
                )
                .with_turn(&config.turn_id),
                started_at,
                false,
                false,
            );
        }
    };
    let initial_write = write_message(&mut transport.stdin, &message);
    let (outcome, failure, stdout_truncated) = if initial_write.is_err() {
        (
            None,
            Some(
                ProtocolFailure::new(
                    "claude_code_write_failed",
                    "Claude Code stopped accepting streamed user messages.",
                    "protocol/write",
                )
                .with_session(known_session.as_deref())
                .with_turn(&config.turn_id),
            ),
            false,
        )
    } else {
        let deadline = if timeout_ms == 0 {
            // timeoutMs 0 opts out of the turn deadline entirely; the agent
            // runs until the turn completes, however long that takes.
            None
        } else {
            Some(Instant::now() + Duration::from_millis(timeout_ms))
        };
        run_turn_loop(
            &mut transport,
            &managed,
            &config,
            expected_session,
            deadline,
            max_stdout,
        )
    };
    let stderr_truncated = transport.stderr_truncated.load(Ordering::Relaxed);
    set_active_session(&managed, None);
    drop(raw_execution);
    drop(transport);
    if let Some(outcome) = outcome {
        if let Err(failure) = bind_session(&managed, &outcome.session_id) {
            remove_transport(&managed, true);
            return RunResult::failed(
                failure.with_turn(&outcome.turn_id),
                started_at,
                false,
                stderr_truncated,
            );
        }
        record_success(
            &managed,
            &outcome.turn_id,
            &config.prompt,
            outcome.events,
            &outcome.output,
        );
        let transitions = crate::protocol::parser::completed_transitions(&outcome.output);
        return RunResult {
            ok: true,
            output: outcome.output,
            transitions,
            error: None,
            thread_id: outcome.session_id.clone(),
            session_id: outcome.session_id,
            turn_id: outcome.turn_id,
            turn_status: "completed".to_string(),
            effective: outcome.effective,
            status_code: None,
            stdout_truncated,
            stderr_truncated,
            started_at,
        };
    }
    let failure = failure.unwrap_or_else(|| {
        ProtocolFailure::new(
            "claude_code_turn_failed",
            "Claude Code did not complete the requested turn.",
            "turn/completed",
        )
        .with_turn(&config.turn_id)
    });
    if requires_transport_reset(&failure) {
        remove_transport(&managed, true);
    }
    RunResult::failed(failure, started_at, stdout_truncated, stderr_truncated)
}

fn run_turn_loop(
    transport: &mut PersistentTransport,
    managed: &Arc<ManagedTransport>,
    config: &DriverConfig,
    known_session: Option<String>,
    mut deadline: Option<Instant>,
    max_stdout: Option<usize>,
) -> (Option<ProtocolFinishReport>, Option<ProtocolFailure>, bool) {
    let mut state = ClaudeCodeParser::new(config, &managed.identity, known_session);
    let mut observed_bytes = 0usize;
    let mut pending_approval: Option<(PendingApproval, Instant)> = None;
    loop {
        if let Some(failure) = handle_control_requests(transport, &mut state) {
            return (None, Some(failure), false);
        }
        if transport.stdin.check_health().is_err() {
            return (
                None,
                Some(state.failure(
                    "claude_code_write_failed",
                    "Claude Code stopped accepting streamed messages.",
                    "protocol/write",
                )),
                false,
            );
        }
        let resolved_approval = match pending_approval.as_ref() {
            Some((approval, parked_at)) => match approval.try_resolve(transport) {
                Ok(Some(_allow)) => Some(*parked_at),
                Ok(None) => None,
                Err(failure) => return (None, Some(failure), false),
            },
            None => None,
        };
        if let Some(parked_at) = resolved_approval {
            if let Some(current) = deadline {
                deadline = current.checked_add(parked_at.elapsed()).or(Some(current));
            }
            pending_approval.take();
        }
        let now = Instant::now();
        if pending_approval.is_none() && deadline.is_some_and(|deadline| now >= deadline) {
            let mut failure = state.failure(
                "claude_code_timeout",
                "Claude Code timed out before the turn completed.",
                "turn/wait",
            );
            failure.turn_status = Some("timeout".to_string());
            return (None, Some(failure), false);
        }
        let wait = if pending_approval.is_some() {
            PROCESS_POLL_INTERVAL
        } else {
            deadline
                .map(|deadline| (deadline - now).min(PROCESS_POLL_INTERVAL))
                .unwrap_or(PROCESS_POLL_INTERVAL)
        };
        match transport.receiver.recv_timeout(wait) {
            Ok(TransportEvent::Line(line)) => {
                let bytes = line.len();
                if let Some(max_stdout) = max_stdout {
                    observed_bytes = observed_bytes.saturating_add(bytes);
                    if observed_bytes > max_stdout {
                        return (
                            None,
                            Some(state.failure(
                                "claude_code_output_limit",
                                "Claude Code exceeded the configured structured output limit.",
                                "protocol/read",
                            )),
                            true,
                        );
                    }
                }
                let effect = match state.parse_line(&line) {
                    Ok(Some(effect)) => effect,
                    Ok(None) => continue,
                    Err(failure) => return (None, Some(failure), false),
                };
                match effect {
                    ClaudeEffect::Permission(request) => {
                        if pending_approval.is_some() {
                            return (
                                None,
                                Some(state.failure(
                                    "claude_code_interaction_concurrent_unsupported",
                                    "Claude Code requested another permission before the active interaction was resolved.",
                                    "server/request",
                                )),
                                false,
                            );
                        }
                        let session_id = state
                            .state()
                            .observed_session_id
                            .as_deref()
                            .or(state.state().expected_session_id.as_deref())
                            .unwrap_or_default();
                        let approval =
                            match park_external_approval(session_id, &config.turn_id, &request) {
                                Ok(approval) => approval,
                                Err(failure) => return (None, Some(failure), false),
                            };
                        pending_approval = Some((approval, Instant::now()));
                    }
                    ClaudeEffect::Control { response } => {
                        if let Some(response) = response
                            && write_message(&mut transport.stdin, &response).is_err()
                        {
                            return (
                                None,
                                Some(state.failure(
                                    "claude_code_write_failed",
                                    "Claude Code stopped accepting control responses.",
                                    "permission/response",
                                )),
                                false,
                            );
                        }
                    }
                    ClaudeEffect::ProtocolFinished(report) => return (Some(report), None, false),
                    ClaudeEffect::Progress { session_id } => {
                        if let Some(session_id) = session_id.as_deref() {
                            if let Err(failure) = bind_session(managed, session_id) {
                                return (None, Some(failure.with_turn(&config.turn_id)), false);
                            }
                            set_active_session(managed, Some(session_id.to_string()));
                            licoup_foundation::platform::turn_event_emit::emit_turn_event(
                                "dispatch.turn.bound",
                                session_id,
                                &config.turn_id,
                                serde_json::json!({"nativeSteer": true}),
                            );
                        }
                    }
                }
            }
            Ok(TransportEvent::LineLimitExceeded) => {
                return (
                    None,
                    Some(state.failure(
                        "claude_code_output_limit",
                        "Claude Code exceeded the hard structured-event limit.",
                        "protocol/read",
                    )),
                    true,
                );
            }
            Ok(TransportEvent::StdoutReadFailed) => {
                return (
                    None,
                    Some(state.failure(
                        "claude_code_read_failed",
                        "Claude Code structured output could not be read.",
                        "protocol/read",
                    )),
                    false,
                );
            }
            Ok(TransportEvent::StdoutClosed) | Err(RecvTimeoutError::Disconnected) => {
                if state.cancel_was_requested() {
                    let mut failure = state.failure(
                        "claude_code_turn_cancelled",
                        "Claude Code turn was cancelled by the user.",
                        "turn/cancelled",
                    );
                    failure.turn_status = Some("cancelled".to_string());
                    return (None, Some(failure), false);
                }
                return (
                    None,
                    Some(state.failure(
                        "claude_code_exited",
                        "Claude Code exited before the turn completed.",
                        "process/exit",
                    )),
                    false,
                );
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

fn handle_control_requests(
    transport: &mut PersistentTransport,
    state: &mut ClaudeCodeParser<'_>,
) -> Option<ProtocolFailure> {
    loop {
        match transport.control_receiver.try_recv() {
            Ok(ControlRequest::Cancel {
                session_id,
                acknowledged,
            }) => {
                let current = state
                    .state()
                    .observed_session_id
                    .as_deref()
                    .or(state.state().expected_session_id.as_deref());
                let matches = current == Some(session_id.as_str());
                if matches {
                    // Bind the user cancel before the interrupt reaches the
                    // process so an is_error terminal cannot become
                    // claude_code_turn_failed (LU-NA-3975).
                    state.mark_cancel_requested();
                }
                let written =
                    matches && write_message(&mut transport.stdin, &interrupt_request()).is_ok();
                let _ = acknowledged.send(written);
                if matches && !written {
                    return Some(state.failure(
                        "claude_code_write_failed",
                        "Claude Code stopped accepting an interrupt request.",
                        "turn/cancel",
                    ));
                }
            }
            Ok(ControlRequest::Steer {
                session_id,
                text,
                acknowledged,
            }) => {
                let current = state
                    .state()
                    .observed_session_id
                    .as_deref()
                    .or(state.state().expected_session_id.as_deref());
                let matches = current == Some(session_id.as_str());
                let written = matches
                    && steer_message(&text).is_some_and(|message| {
                        write_message(&mut transport.stdin, &message).is_ok()
                    });
                let _ = acknowledged.send(written);
                if matches && !written {
                    return Some(state.failure(
                        "claude_code_write_failed",
                        "Claude Code stopped accepting streamed guidance.",
                        "turn/steer",
                    ));
                }
            }
            Ok(ControlRequest::Cleanup { acknowledged }) => {
                let _ = write_message(&mut transport.stdin, &interrupt_request());
                let _ = acknowledged.send(true);
                return Some(state.failure(
                    "claude_code_cleanup_requested",
                    "Claude Code transport cleanup was requested.",
                    "process/cleanup",
                ));
            }
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => {
                return Some(state.failure(
                    "claude_code_supervisor_unavailable",
                    "Claude Code supervisor control channel is unavailable.",
                    "process/supervisor",
                ));
            }
        }
    }
}

pub(super) fn timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
}
