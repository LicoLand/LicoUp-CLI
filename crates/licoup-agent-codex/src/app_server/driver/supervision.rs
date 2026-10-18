use super::super::limits::PROCESS_POLL_INTERVAL;
use super::super::model::{ProtocolEffect, ProtocolFailure, ProtocolOutcome, RunResult};
use super::active_control::{ActiveTurnGuard, ControlRequest, bind};
use super::io::{TransportEvent, write_message};
use crate::parser::CodexEffect;
use crate::parser::CodexParser;
use licoup_foundation::platform::process_supervisor::{BoundedStdinWriter, SupervisedChild};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TryRecvError};
use std::time::Instant;

pub(super) fn run_protocol_loop(
    stdin: &mut BoundedStdinWriter,
    receiver: &Receiver<TransportEvent>,
    control_sender: &SyncSender<ControlRequest>,
    control_receiver: &Receiver<ControlRequest>,
    protocol: &mut CodexParser,
    deadline: Option<Instant>,
) -> (
    Option<ProtocolOutcome>,
    Option<ProtocolFailure>,
    Option<i32>,
    bool,
) {
    let mut active_guard: Option<ActiveTurnGuard> = None;
    let mut pending_steers = HashMap::<String, SyncSender<bool>>::new();
    loop {
        if let Some((thread_id, turn_id)) = protocol.active_turn_binding() {
            if active_guard.is_none() {
                active_guard = bind(thread_id, turn_id, control_sender.clone());
                if active_guard.is_some() {
                    crate::port::turn_event::emit_turn_event(
                        "dispatch.turn.bound",
                        thread_id,
                        turn_id,
                        serde_json::json!({"nativeSteer": true}),
                    );
                }
            }
            loop {
                match control_receiver.try_recv() {
                    Ok(request) => {
                        let (request_id, message, acknowledged) =
                            request.into_protocol(thread_id, turn_id);
                        if write_message(stdin, &message).is_err() {
                            let _ = acknowledged.send(false);
                            return (
                                None,
                                Some(protocol.contextualize(ProtocolFailure::new(
                                    "codex_app_server_write_failed",
                                    "Codex app-server stopped accepting turn guidance.",
                                    "turn/steer",
                                ))),
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
                Some(protocol.contextualize(ProtocolFailure::new(
                    "codex_app_server_write_failed",
                    "Codex app-server stopped accepting protocol messages.",
                    "protocol/write",
                ))),
                None,
                false,
            );
        }
        let now = Instant::now();
        if deadline.is_some_and(|deadline| now >= deadline) {
            return (
                None,
                Some(protocol.contextualize(ProtocolFailure::new(
                    "codex_app_server_timeout",
                    "Codex app-server timed out before the turn completed.",
                    "turn/wait",
                ))),
                None,
                false,
            );
        }
        let wait = deadline
            .map(|deadline| (deadline - now).min(PROCESS_POLL_INTERVAL))
            .unwrap_or(PROCESS_POLL_INTERVAL);
        match receiver.recv_timeout(wait) {
            Ok(TransportEvent::Line(line)) => {
                let effects = match protocol.parse_line(&line) {
                    Ok(effects) => effects,
                    Err(failure) => return (None, Some(failure), None, false),
                };
                for effect in effects {
                    let CodexEffect::Protocol(effect) = effect else {
                        if let CodexEffect::SteerResponse {
                            request_id,
                            accepted,
                        } = effect
                            && let Some(acknowledged) = pending_steers.remove(&request_id)
                        {
                            let _ = acknowledged.send(accepted);
                        }
                        continue;
                    };
                    match effect {
                        ProtocolEffect::Send(message) => {
                            if write_message(stdin, &message).is_err() {
                                return (
                                    None,
                                    Some(protocol.contextualize(ProtocolFailure::new(
                                        "codex_app_server_write_failed",
                                        "Codex app-server stopped accepting protocol messages.",
                                        "protocol/write",
                                    ))),
                                    None,
                                    false,
                                );
                            }
                        }
                        ProtocolEffect::Complete(outcome) => {
                            return (Some(*outcome), None, None, false);
                        }
                        ProtocolEffect::Fail(failure) => {
                            return (None, Some(protocol.contextualize(failure)), None, false);
                        }
                    }
                }
            }
            Ok(TransportEvent::StdoutLimitExceeded) => {
                return read_failure(
                    protocol,
                    "codex_app_server_output_limit",
                    "Codex app-server exceeded the configured protocol output limit.",
                    true,
                );
            }
            Ok(TransportEvent::StdoutReadFailed) => {
                return read_failure(
                    protocol,
                    "codex_app_server_read_failed",
                    "Codex app-server protocol output could not be read.",
                    false,
                );
            }
            Ok(TransportEvent::StdoutClosed) | Err(RecvTimeoutError::Disconnected) => {
                return (
                    None,
                    Some(protocol.contextualize(ProtocolFailure::new(
                        "codex_app_server_exited",
                        "Codex app-server exited before the turn completed.",
                        "process/exit",
                    ))),
                    None,
                    false,
                );
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

fn read_failure(
    protocol: &CodexParser,
    code: &'static str,
    message: &'static str,
    stdout_truncated: bool,
) -> (
    Option<ProtocolOutcome>,
    Option<ProtocolFailure>,
    Option<i32>,
    bool,
) {
    (
        None,
        Some(protocol.contextualize(ProtocolFailure::new(code, message, "protocol/read"))),
        None,
        stdout_truncated,
    )
}

pub(super) fn pipe_failure(
    child: &mut SupervisedChild,
    started_at: String,
    message: &'static str,
) -> RunResult {
    let failure = if child.terminate_tree().is_ok() {
        ProtocolFailure::new("codex_app_server_pipe_failed", message, "process/start")
    } else {
        ProtocolFailure::new(
            "codex_app_server_cleanup_failed",
            "Codex app-server process cleanup could not be completed safely.",
            "process/cleanup",
        )
    };
    RunResult::failed(failure, started_at, None, false, false)
}
