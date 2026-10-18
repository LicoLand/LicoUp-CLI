use super::active_control::{ActiveTurnGuard, SteerRequest, bind};
use super::errors::ProtocolFailure;
use super::io::{TransportEvent, drain_stderr, read_protocol_messages, write_message};
use super::model::{PROCESS_POLL_INTERVAL, RunResult};
use super::params::ProtocolConfig;
use super::supervision::LaunchSpec;
use crate::parser::{
    PendingInteraction, PiProtocol, ProtocolEffect, ProtocolOutcome, classify_steer_response,
    completed_transitions, decode_jsonl_line, encode_steer,
};
use crate::port::turn_event::emit_turn_event;
use licoup_foundation::platform::process_supervisor::{
    BoundedStdinWriter, SupervisedChild, TransportFinishFailure, finish_protocol_transport,
};
use serde_json::Value;
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError};
use std::thread;
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
    let config = match ProtocolConfig::from_params(params, prompt, session_id, cwd) {
        Ok(config) => config,
        Err(failure) => return RunResult::failed(failure, started_at, None, false, false),
    };
    let launch = LaunchSpec::new(executable, Path::new(&config.cwd));
    let mut child = match launch.spawn() {
        Ok(child) => child,
        Err(error) => {
            let message = match error.kind() {
                io::ErrorKind::NotFound => "The Pi Agent executable is not available.",
                io::ErrorKind::PermissionDenied => {
                    "The Pi Agent executable is not permitted to run."
                }
                _ => "Pi RPC could not be started.",
            };
            return RunResult::failed(
                ProtocolFailure::new("pi_rpc_start_failed", message, "process/start"),
                started_at,
                None,
                false,
                false,
            );
        }
    };
    let Some(stdout) = child.stdout() else {
        return pipe_failure(&mut child, started_at, "Pi RPC stdout is unavailable.");
    };
    let Some(stderr) = child.stderr() else {
        return pipe_failure(&mut child, started_at, "Pi RPC stderr is unavailable.");
    };
    let Some(stdin) = child.stdin() else {
        return pipe_failure(&mut child, started_at, "Pi RPC stdin is unavailable.");
    };
    let mut stdin = BoundedStdinWriter::new(stdin);

    let (sender, receiver) = mpsc::channel();
    let stdout_observer =
        licoup_foundation::platform::raw_execution::RawExecutionObserver::current();
    let stdout_handle = thread::spawn(move || {
        let _raw_scope =
            licoup_foundation::platform::raw_execution::RawExecutionScope::enter(stdout_observer);
        read_protocol_messages(stdout, max_stdout, sender)
    });
    let stderr_truncated = Arc::new(AtomicBool::new(false));
    let stderr_flag = Arc::clone(&stderr_truncated);
    let stderr_observer =
        licoup_foundation::platform::raw_execution::RawExecutionObserver::current();
    let stderr_handle = thread::spawn(move || {
        let _raw_scope =
            licoup_foundation::platform::raw_execution::RawExecutionScope::enter(stderr_observer);
        drain_stderr(stderr, max_stderr, &stderr_flag)
    });

    let mut protocol = PiProtocol::new(config);
    let (control_sender, control_receiver) = mpsc::sync_channel(16);
    let initial = protocol.initial_request();
    if write_message(&mut stdin, &initial).is_err() {
        let cleanup =
            finish_protocol_transport(&mut child, &mut stdin, stdout_handle, stderr_handle);
        let cleanup_failed = cleanup == Err(TransportFinishFailure::Lifecycle);
        return RunResult::failed(
            ProtocolFailure::new(
                if cleanup_failed {
                    "pi_rpc_cleanup_failed"
                } else {
                    "pi_rpc_write_failed"
                },
                if cleanup_failed {
                    "Pi RPC process cleanup could not be completed safely."
                } else {
                    "Pi RPC stopped accepting protocol messages."
                },
                if cleanup_failed {
                    "process/cleanup"
                } else {
                    "protocol/write"
                },
            ),
            started_at,
            None,
            false,
            stderr_truncated.load(Ordering::Relaxed),
        );
    }

    let deadline = if timeout_ms == 0 {
        None
    } else {
        Some(Instant::now() + Duration::from_millis(timeout_ms))
    };
    let (outcome, failure, status_code, stdout_was_truncated) = run_protocol_loop(
        &mut stdin,
        &receiver,
        &control_sender,
        &control_receiver,
        &mut protocol,
        deadline,
    );

    let cleanup = finish_protocol_transport(&mut child, &mut stdin, stdout_handle, stderr_handle);
    let stderr_was_truncated = stderr_truncated.load(Ordering::Relaxed);

    if cleanup == Err(TransportFinishFailure::Lifecycle) {
        return RunResult::failed(
            ProtocolFailure::new(
                "pi_rpc_cleanup_failed",
                "Pi RPC process cleanup could not be completed safely.",
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
                "pi_rpc_write_failed",
                "Pi RPC stopped accepting protocol messages.",
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
            transitions: completed_transitions(&outcome.output),
            output: outcome.output,
            error: None,
            thread_id: outcome.session_id.clone(),
            session_id: outcome.session_id,
            turn_id: outcome.turn_id,
            turn_status: outcome.turn_status,
            effective: outcome.effective,
            status_code,
            stdout_truncated: stdout_was_truncated,
            stderr_truncated: stderr_was_truncated,
            started_at,
        };
    }
    RunResult::failed(
        failure.unwrap_or_else(|| {
            ProtocolFailure::new(
                "pi_rpc_failed",
                "Pi RPC did not complete the request.",
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
    control_sender: &SyncSender<SteerRequest>,
    control_receiver: &Receiver<SteerRequest>,
    protocol: &mut PiProtocol,
    mut deadline: Option<Instant>,
) -> (
    Option<ProtocolOutcome>,
    Option<ProtocolFailure>,
    Option<i32>,
    bool,
) {
    let mut active_guard: Option<ActiveTurnGuard> = None;
    let mut pending_steers = HashMap::<String, SyncSender<bool>>::new();
    let mut pending_interaction: Option<(PendingInteraction, Instant)> = None;
    loop {
        if let Some((session_id, turn_id)) = protocol.active_turn_binding() {
            if active_guard.is_none() {
                active_guard = bind(session_id, turn_id, control_sender.clone());
                if active_guard.is_some() {
                    emit_turn_event(
                        "dispatch.turn.bound",
                        session_id,
                        turn_id,
                        serde_json::json!({"nativeSteer": true}),
                    );
                }
            }
            loop {
                match control_receiver.try_recv() {
                    Ok(request) => {
                        let (text, acknowledged) = request.into_parts();
                        let (request_id, message) = encode_steer(text);
                        if write_message(stdin, &message).is_err() {
                            let _ = acknowledged.send(false);
                            return (
                                None,
                                Some(protocol.failure_with_ids(
                                    "pi_rpc_write_failed",
                                    "Pi RPC stopped accepting turn guidance.",
                                    "turn/steer",
                                )),
                                None,
                                false,
                            );
                        }
                        pending_steers.insert(request_id, acknowledged);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => break,
                }
            }
        }
        if stdin.check_health().is_err() {
            return (
                None,
                Some(protocol.failure_with_ids(
                    "pi_rpc_write_failed",
                    "Pi RPC stopped accepting protocol messages.",
                    "protocol/write",
                )),
                None,
                false,
            );
        }
        let resolved_interaction = match pending_interaction.as_ref() {
            Some((interaction, parked_at)) => match interaction.try_response(protocol) {
                Ok(Some(response)) => Some((response, *parked_at)),
                Ok(None) => None,
                Err(failure) => return (None, Some(failure), None, false),
            },
            None => None,
        };
        if let Some((response, parked_at)) = resolved_interaction {
            deadline = extend_deadline_for_pause(deadline, parked_at, Instant::now());
            pending_interaction.take();
            if write_message(stdin, &response).is_err() {
                return (
                    None,
                    Some(protocol.failure_with_ids(
                        "pi_rpc_write_failed",
                        "Pi RPC stopped accepting the interaction response.",
                        "extension-ui/response",
                    )),
                    None,
                    false,
                );
            }
        }
        let received = match receiver.try_recv() {
            Ok(event) => Ok(event),
            Err(TryRecvError::Disconnected) => Err(RecvTimeoutError::Disconnected),
            Err(TryRecvError::Empty) => {
                let now = Instant::now();
                if pending_interaction.is_none() && deadline.is_some_and(|deadline| now >= deadline)
                {
                    return (
                        None,
                        Some(protocol.failure_with_ids(
                            "pi_rpc_timeout",
                            "Pi RPC timed out before the turn completed.",
                            "turn/wait",
                        )),
                        None,
                        false,
                    );
                }
                let wait = if pending_interaction.is_some() {
                    PROCESS_POLL_INTERVAL
                } else {
                    deadline
                        .map(|deadline| {
                            deadline
                                .saturating_duration_since(now)
                                .min(PROCESS_POLL_INTERVAL)
                        })
                        .unwrap_or(PROCESS_POLL_INTERVAL)
                };
                receiver.recv_timeout(wait)
            }
        };
        match received {
            Ok(TransportEvent::Line { line, received_at }) => {
                let message = match decode_jsonl_line(&line) {
                    Ok(Some(message)) => message,
                    Ok(None) => continue,
                    Err(()) => {
                        return (
                            None,
                            Some(protocol.failure_with_ids(
                                "pi_rpc_invalid_json",
                                "Pi RPC returned an invalid protocol frame.",
                                "protocol/read",
                            )),
                            None,
                            false,
                        );
                    }
                };
                if let Some(response) = classify_steer_response(&message)
                    && let Some(acknowledged) = pending_steers.remove(&response.request_id)
                {
                    let _ = acknowledged.send(response.accepted);
                    continue;
                }
                for effect in protocol.handle_message(message) {
                    match effect {
                        ProtocolEffect::Send(payload) => {
                            if write_message(stdin, &payload).is_err() {
                                return (
                                    None,
                                    Some(protocol.failure_with_ids(
                                        "pi_rpc_write_failed",
                                        "Pi RPC stopped accepting protocol messages.",
                                        "protocol/write",
                                    )),
                                    None,
                                    false,
                                );
                            }
                        }
                        ProtocolEffect::Interact(interaction) => {
                            if pending_interaction.is_some() {
                                return (
                                    None,
                                    Some(protocol.failure_with_ids(
                                        "pi_interaction_concurrent_unsupported",
                                        "Pi Agent requested another dialog before the active interaction was resolved.",
                                        "extension-ui/request",
                                    )),
                                    None,
                                    false,
                                );
                            }
                            // The turn budget pauses when the interaction frame
                            // reaches the transport, not when a busy executor
                            // thread eventually observes the queued frame.
                            pending_interaction = Some((interaction, received_at));
                        }
                        ProtocolEffect::Complete(outcome) => {
                            return (Some(*outcome), None, None, false);
                        }
                        ProtocolEffect::Fail(failure) => {
                            return (None, Some(failure), None, false);
                        }
                    }
                }
            }
            Ok(TransportEvent::StdoutLimitExceeded) => {
                return (
                    None,
                    Some(protocol.failure_with_ids(
                        "pi_rpc_output_limit",
                        "Pi RPC exceeded the bounded stdout limit.",
                        "protocol/read",
                    )),
                    None,
                    true,
                );
            }
            Ok(TransportEvent::StdoutReadFailed) => {
                return (
                    None,
                    Some(protocol.failure_with_ids(
                        "pi_rpc_read_failed",
                        "Pi RPC stdout could not be read.",
                        "protocol/read",
                    )),
                    None,
                    false,
                );
            }
            Ok(TransportEvent::StdoutClosed) => {
                return (
                    None,
                    Some(protocol.failure_with_ids(
                        "pi_rpc_exited",
                        "Pi RPC exited before the turn completed.",
                        "process/exit",
                    )),
                    None,
                    false,
                );
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                return (
                    None,
                    Some(protocol.failure_with_ids(
                        "pi_rpc_exited",
                        "Pi RPC exited before the turn completed.",
                        "process/exit",
                    )),
                    None,
                    false,
                );
            }
        }
    }
}

pub(super) fn extend_deadline_for_pause(
    deadline: Option<Instant>,
    parked_at: Instant,
    resumed_at: Instant,
) -> Option<Instant> {
    let deadline = deadline?;
    Some(
        deadline
            .checked_add(resumed_at.saturating_duration_since(parked_at))
            .unwrap_or(deadline),
    )
}

pub(super) fn pipe_failure(
    child: &mut SupervisedChild,
    started_at: String,
    _message: &str,
) -> RunResult {
    let _ = child.terminate_tree();
    RunResult::failed(
        ProtocolFailure::new(
            "pi_rpc_pipe_failed",
            "Pi RPC pipes are unavailable.",
            "process/start",
        ),
        started_at,
        None,
        false,
        false,
    )
}

pub(super) fn timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
}
