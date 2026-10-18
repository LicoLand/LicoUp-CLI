use super::control::ActiveAcpControl;
use super::errors::ProtocolFailure;
use super::events::{ACP_EVENT_CHANNEL_CAPACITY, TransportEvent, read_protocol_messages};
use super::io::{drain_stderr, write_message};
use super::model::{AcpDriverSpec, CapabilityProbe, PROCESS_POLL_INTERVAL, RunResult};
use super::params::{ProtocolConfig, timestamp};
use super::protocol::{AcpProtocol, ProtocolEffect, ProtocolOutcome, ProtocolPhase};
use super::supervision::LaunchSpec;
use licoup_foundation::platform::process_supervisor::{
    BoundedStdinWriter, SupervisedChild, TransportFinishFailure, finish_protocol_transport,
};
use licoup_foundation::platform::raw_execution::{
    RawExecutionBinding, RawExecutionDirection, RawExecutionObserver, RawExecutionReader,
    RawExecutionScope,
};
use serde_json::Value;
use std::io::{self, BufReader};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

pub trait ProtocolLoopTransport {
    fn check_health(&mut self) -> io::Result<()>;
    fn write(&mut self, message: &Value) -> io::Result<()>;
    fn recv_timeout(&mut self, timeout: Duration) -> Result<TransportEvent, RecvTimeoutError>;
    fn now(&self) -> Instant;
    fn sync_control(&mut self, _session_id: Option<&str>) -> io::Result<()> {
        Ok(())
    }
}

struct StdioProtocolLoopTransport<'a> {
    stdin: &'a mut BoundedStdinWriter,
    receiver: &'a Receiver<TransportEvent>,
    active_control: ActiveAcpControl,
}

impl ProtocolLoopTransport for StdioProtocolLoopTransport<'_> {
    fn check_health(&mut self) -> io::Result<()> {
        self.stdin
            .check_health()
            .map_err(|_| io::Error::other("native agent protocol write failed"))
    }

    fn write(&mut self, message: &Value) -> io::Result<()> {
        write_message(self.stdin, message)
    }

    fn recv_timeout(&mut self, timeout: Duration) -> Result<TransportEvent, RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }

    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sync_control(&mut self, session_id: Option<&str>) -> io::Result<()> {
        self.active_control
            .sync_binding(session_id, session_id)
            .map_err(|_| io::Error::other("ACP active-turn control registry is unavailable"))?;
        self.active_control.poll(self.stdin)
    }
}

pub fn execute_acp(
    driver: AcpDriverSpec,
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
    let mut config = match ProtocolConfig::from_params(params, prompt, session_id, cwd) {
        Ok(config) => config,
        Err(failure) => {
            return RunResult::failed(
                driver,
                failure,
                started_at,
                None,
                false,
                false,
                CapabilityProbe::default(),
                Vec::new(),
            );
        }
    };
    if let Err(failure) = config.load_collaboration_mcp(driver.agent_id) {
        return RunResult::failed(
            driver,
            failure,
            started_at,
            None,
            false,
            false,
            CapabilityProbe::default(),
            Vec::new(),
        );
    }
    let launch = match LaunchSpec::for_execution(
        executable,
        driver,
        Path::new(&config.cwd),
        &mut config.settings,
    ) {
        Ok(launch) => launch,
        Err(failure) => {
            return RunResult::failed(
                driver,
                failure,
                started_at,
                None,
                false,
                false,
                CapabilityProbe::default(),
                Vec::new(),
            );
        }
    };
    let mut child = match launch.spawn() {
        Ok(child) => child,
        Err(error) => {
            let message = match error.kind() {
                io::ErrorKind::NotFound => "The requested ACP agent executable is not available.",
                io::ErrorKind::PermissionDenied => {
                    "The requested ACP agent executable is not permitted to run."
                }
                _ => "The requested ACP agent could not be started.",
            };
            return RunResult::failed(
                driver,
                ProtocolFailure::new("acp_process_start_failed", message, "process/start"),
                started_at,
                None,
                false,
                false,
                CapabilityProbe::default(),
                Vec::new(),
            );
        }
    };

    let Some(stdout) = child.stdout() else {
        return pipe_failure(driver, &mut child, started_at);
    };
    let Some(stderr) = child.stderr() else {
        return pipe_failure(driver, &mut child, started_at);
    };
    let Some(stdin) = child.stdin() else {
        return pipe_failure(driver, &mut child, started_at);
    };
    let mut stdin = BoundedStdinWriter::new(stdin);

    let (sender, receiver) = mpsc::sync_channel(ACP_EVENT_CHANNEL_CAPACITY);
    let raw_observer = RawExecutionObserver::current();
    let stdout_binding = RawExecutionBinding::default();
    let _raw_stdout_scope = stdout_binding.bind(raw_observer.clone());
    let stdout_handle = thread::spawn(move || {
        let reader = RawExecutionReader::new(
            stdout,
            stdout_binding,
            "acp",
            RawExecutionDirection::Received,
        );
        read_protocol_messages(BufReader::new(reader), max_stdout, sender)
    });
    let stderr_truncated = Arc::new(AtomicBool::new(false));
    let stderr_flag = Arc::clone(&stderr_truncated);
    let stderr_handle = thread::spawn(move || {
        let _raw_scope = RawExecutionScope::enter(raw_observer);
        drain_stderr(stderr, max_stderr, &stderr_flag)
    });

    let mut protocol = AcpProtocol::new(config, driver.agent_id);
    let initial_request = match protocol.initial_request() {
        Ok(request) => request,
        Err(failure) => {
            let cleanup =
                finish_protocol_transport(&mut child, &mut stdin, stdout_handle, stderr_handle);
            let cleanup_failed = cleanup == Err(TransportFinishFailure::Lifecycle);
            return RunResult::failed(
                driver,
                if cleanup_failed {
                    ProtocolFailure::new(
                        "acp_process_cleanup_failed",
                        "The ACP agent process cleanup could not be completed safely.",
                        "process/cleanup",
                    )
                } else {
                    failure
                },
                started_at,
                None,
                false,
                stderr_truncated.load(Ordering::Relaxed),
                CapabilityProbe::default(),
                Vec::new(),
            );
        }
    };
    if write_message(&mut stdin, &initial_request).is_err() {
        let cleanup =
            finish_protocol_transport(&mut child, &mut stdin, stdout_handle, stderr_handle);
        let cleanup_failed = cleanup == Err(TransportFinishFailure::Lifecycle);
        return RunResult::failed(
            driver,
            ProtocolFailure::new(
                if cleanup_failed {
                    "acp_process_cleanup_failed"
                } else {
                    "acp_protocol_write_failed"
                },
                if cleanup_failed {
                    "The ACP agent process cleanup could not be completed safely."
                } else {
                    "The ACP agent stopped accepting protocol messages."
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
            CapabilityProbe::default(),
            Vec::new(),
        );
    }

    // timeoutMs 0 opts out of any turn deadline (see runtime_adapters/dispatch),
    // so only a non-zero window gets a concrete deadline.
    let deadline = (timeout_ms != 0).then(|| Instant::now() + Duration::from_millis(timeout_ms));
    let (outcome, failure, status_code, stdout_was_truncated) = {
        let mut transport = StdioProtocolLoopTransport {
            stdin: &mut stdin,
            receiver: &receiver,
            active_control: ActiveAcpControl::new(driver.agent_id),
        };
        run_protocol_loop(&mut transport, &mut protocol, deadline)
    };
    drop(receiver);
    let capabilities = protocol.capabilities.clone();
    let events = std::mem::take(&mut protocol.events);

    let cleanup = finish_protocol_transport(&mut child, &mut stdin, stdout_handle, stderr_handle);
    let stderr_was_truncated = stderr_truncated.load(Ordering::Relaxed);

    if cleanup == Err(TransportFinishFailure::Lifecycle) {
        return RunResult::failed(
            driver,
            ProtocolFailure::new(
                "acp_process_cleanup_failed",
                "The ACP agent process cleanup could not be completed safely.",
                "process/cleanup",
            ),
            started_at,
            status_code,
            stdout_was_truncated,
            stderr_was_truncated,
            capabilities,
            events,
        );
    }
    if outcome.is_some() && cleanup == Err(TransportFinishFailure::StdinWrite) {
        return RunResult::failed(
            driver,
            ProtocolFailure::new(
                "acp_protocol_write_failed",
                "The ACP agent stopped accepting protocol messages.",
                "protocol/write",
            ),
            started_at,
            status_code,
            stdout_was_truncated,
            stderr_was_truncated,
            capabilities,
            events,
        );
    }

    if let Some(outcome) = outcome {
        let mut effective = outcome.effective;
        launch.apply_effective_settings(&mut effective);
        return RunResult {
            ok: true,
            output: outcome.output,
            error: None,
            session_id: outcome.session_id,
            thread_id: outcome.thread_id,
            turn_id: outcome.turn_id,
            turn_status: outcome.turn_status,
            effective,
            capabilities: outcome.capabilities,
            transitions: outcome.transitions,
            status_code,
            stdout_truncated: stdout_was_truncated,
            stderr_truncated: stderr_was_truncated,
            started_at,
            runtime_protocol: driver.runtime_protocol,
            driver_id: driver.agent_id,
        };
    }

    RunResult::failed(
        driver,
        failure.unwrap_or_else(|| {
            ProtocolFailure::new(
                "acp_protocol_failed",
                "The ACP agent did not complete the request.",
                "protocol",
            )
            .with_session(protocol.session_id.as_deref())
        }),
        started_at,
        status_code,
        stdout_was_truncated,
        stderr_was_truncated,
        capabilities,
        events,
    )
}

fn pipe_failure(
    driver: AcpDriverSpec,
    child: &mut SupervisedChild,
    started_at: String,
) -> RunResult {
    let cleanup_ok = child.terminate_tree().is_ok();
    RunResult::failed(
        driver,
        ProtocolFailure::new(
            if cleanup_ok {
                "acp_process_pipe_failed"
            } else {
                "acp_process_cleanup_failed"
            },
            if cleanup_ok {
                "The ACP agent protocol pipes are unavailable."
            } else {
                "The ACP agent process cleanup could not be completed safely."
            },
            if cleanup_ok {
                "process/start"
            } else {
                "process/cleanup"
            },
        ),
        started_at,
        None,
        false,
        false,
        CapabilityProbe::default(),
        Vec::new(),
    )
}

pub fn run_protocol_loop<T: ProtocolLoopTransport>(
    transport: &mut T,
    protocol: &mut AcpProtocol,
    hard_deadline: Option<Instant>,
) -> (
    Option<ProtocolOutcome>,
    Option<ProtocolFailure>,
    Option<i32>,
    bool,
) {
    loop {
        if transport
            .sync_control(protocol.session_id.as_deref())
            .is_err()
        {
            let failure = ProtocolFailure::new(
                "acp_control_transport_unavailable",
                "The ACP active-turn control channel is unavailable.",
                "turn/control",
            )
            .with_session(protocol.session_id.as_deref());
            return (None, Some(failure), None, false);
        }
        if transport.check_health().is_err() {
            let failure = ProtocolFailure::new(
                "acp_protocol_write_failed",
                "The ACP agent stopped accepting protocol messages.",
                "protocol/write",
            )
            .with_session(protocol.session_id.as_deref());
            return (None, Some(failure), None, false);
        }
        let now = transport.now();
        if hard_deadline.is_some_and(|deadline| now >= deadline) {
            return protocol_timeout(protocol);
        }
        let wait = hard_deadline
            .map(|deadline| deadline.saturating_duration_since(now))
            .unwrap_or(PROCESS_POLL_INTERVAL)
            .min(PROCESS_POLL_INTERVAL);
        let received = transport.recv_timeout(wait);
        let observed_at = transport.now();
        if hard_deadline.is_some_and(|deadline| observed_at >= deadline) {
            return protocol_timeout(protocol);
        }
        match received {
            Ok(TransportEvent::Frame(line)) => {
                let phase_before = protocol.phase;
                let parsed = protocol.handle_frame(&line);
                if let Some(result) = apply_protocol_effects(transport, protocol, parsed.effects) {
                    return result;
                }
                if phase_before != ProtocolPhase::AwaitPrompt
                    && protocol.phase == ProtocolPhase::AwaitPrompt
                    && let Some(session_id) = protocol.session_id.as_deref()
                {
                    if transport.sync_control(Some(session_id)).is_err() {
                        let failure = ProtocolFailure::new(
                            "acp_control_transport_unavailable",
                            "The ACP active-turn control channel is unavailable.",
                            "turn/control",
                        )
                        .with_session(Some(session_id));
                        return (None, Some(failure), None, false);
                    }
                    licoup_foundation::platform::turn_event_emit::emit_turn_event(
                        "dispatch.turn.bound",
                        session_id,
                        &protocol.turn_id,
                        serde_json::json!({}),
                    );
                }
            }
            Ok(TransportEvent::StdoutLimitExceeded) => {
                return (
                    None,
                    Some(
                        ProtocolFailure::new(
                            "acp_protocol_output_limit",
                            "The ACP agent exceeded the configured protocol output limit.",
                            "protocol/read",
                        )
                        .with_session(protocol.session_id.as_deref()),
                    ),
                    None,
                    true,
                );
            }
            Ok(TransportEvent::StdoutReadFailed) => {
                return (
                    None,
                    Some(ProtocolFailure::new(
                        "acp_protocol_read_failed",
                        "The ACP agent protocol output could not be read.",
                        "protocol/read",
                    )),
                    None,
                    false,
                );
            }
            Ok(TransportEvent::StdoutClosed) | Err(RecvTimeoutError::Disconnected) => {
                return (
                    None,
                    Some(ProtocolFailure::new(
                        "acp_process_exited",
                        "The ACP agent exited before the turn completed.",
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

fn apply_protocol_effects<T: ProtocolLoopTransport>(
    transport: &mut T,
    protocol: &AcpProtocol,
    effects: Vec<ProtocolEffect>,
) -> Option<(
    Option<ProtocolOutcome>,
    Option<ProtocolFailure>,
    Option<i32>,
    bool,
)> {
    for effect in effects {
        match effect {
            ProtocolEffect::Send(message) => {
                if transport.write(&message).is_err() {
                    let failure = ProtocolFailure::new(
                        "acp_protocol_write_failed",
                        "The ACP agent stopped accepting protocol messages.",
                        "protocol/write",
                    )
                    .with_session(protocol.session_id.as_deref());
                    return Some((None, Some(failure), None, false));
                }
            }
            ProtocolEffect::Complete(outcome) => {
                return Some((Some(*outcome), None, None, false));
            }
            ProtocolEffect::Fail(failure) => {
                return Some((None, Some(failure), None, false));
            }
        }
    }
    None
}

fn protocol_timeout(
    protocol: &AcpProtocol,
) -> (
    Option<ProtocolOutcome>,
    Option<ProtocolFailure>,
    Option<i32>,
    bool,
) {
    let failure = ProtocolFailure::new(
        "acp_protocol_timeout",
        "The ACP agent timed out before the turn completed.",
        "session/prompt",
    )
    .with_session(protocol.session_id.as_deref());
    (None, Some(failure), None, false)
}
