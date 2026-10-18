use super::errors::ProtocolFailure;
use super::events::{ACP_EVENT_CHANNEL_CAPACITY, TransportEvent, read_protocol_messages};
use super::io::{drain_stderr, write_message};
use super::model::{AcpDriverSpec, CapabilityProbe, PROCESS_POLL_INTERVAL};
use super::protocol::INITIALIZE_REQUEST_ID;
use super::supervision::{LaunchSpec, acp_pipe_failure};
use licoup_foundation::core::acp::{self, AcpClientCapabilities, AcpImplementation};
use licoup_foundation::platform::process_supervisor::{
    BoundedStdinWriter, TransportFinishFailure, finish_protocol_transport,
};
use std::io::{self, BufReader};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

pub fn probe_acp(
    driver: AcpDriverSpec,
    executable: &str,
    cwd: &Path,
    timeout_ms: u64,
    max_stdout: Option<usize>,
    max_stderr: usize,
) -> Result<CapabilityProbe, ProtocolFailure> {
    probe_acp_inner(driver, executable, cwd, timeout_ms, max_stdout, max_stderr)
        .map_err(|failure| failure.namespaced(driver))
}

fn probe_acp_inner(
    driver: AcpDriverSpec,
    executable: &str,
    cwd: &Path,
    timeout_ms: u64,
    max_stdout: Option<usize>,
    max_stderr: usize,
) -> Result<CapabilityProbe, ProtocolFailure> {
    if !cwd.is_absolute() {
        return Err(ProtocolFailure::new(
            "acp_working_directory_invalid",
            "ACP conversation sessions require an absolute working directory.",
            "initialize",
        ));
    }
    let launch = LaunchSpec::new(executable, driver, cwd);
    let mut child = launch.spawn().map_err(|error| {
        let message = match error.kind() {
            io::ErrorKind::NotFound => "The requested ACP agent executable is not available.",
            io::ErrorKind::PermissionDenied => {
                "The requested ACP agent executable is not permitted to run."
            }
            _ => "The requested ACP agent could not be started.",
        };
        ProtocolFailure::new("acp_process_start_failed", message, "process/start")
    })?;
    let Some(stdout) = child.stdout() else {
        return Err(acp_pipe_failure(&mut child));
    };
    let Some(stderr) = child.stderr() else {
        return Err(acp_pipe_failure(&mut child));
    };
    let Some(stdin) = child.stdin() else {
        return Err(acp_pipe_failure(&mut child));
    };
    let mut stdin = BoundedStdinWriter::new(stdin);
    let (sender, receiver) = mpsc::sync_channel(ACP_EVENT_CHANNEL_CAPACITY);
    let stdout_handle =
        thread::spawn(move || read_protocol_messages(BufReader::new(stdout), max_stdout, sender));
    let stderr_truncated = Arc::new(AtomicBool::new(false));
    let stderr_flag = Arc::clone(&stderr_truncated);
    let stderr_handle = thread::spawn(move || drain_stderr(stderr, max_stderr, &stderr_flag));
    let request = acp::initialize_request(
        INITIALIZE_REQUEST_ID,
        &AcpImplementation::new("lico-up", env!("CARGO_PKG_VERSION")),
        AcpClientCapabilities::default(),
    )
    .map_err(|error| ProtocolFailure::from_acp(error, acp::INITIALIZE_METHOD))?;
    let result = if write_message(&mut stdin, &request).is_err() {
        Err(ProtocolFailure::new(
            "acp_protocol_write_failed",
            "The ACP agent stopped accepting protocol messages.",
            "initialize",
        ))
    } else {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            if stdin.check_health().is_err() {
                break Err(ProtocolFailure::new(
                    "acp_protocol_write_failed",
                    "The ACP agent stopped accepting protocol messages.",
                    "protocol/write",
                ));
            }
            let now = Instant::now();
            if now >= deadline {
                break Err(ProtocolFailure::new(
                    "acp_protocol_timeout",
                    "The ACP agent timed out during capability negotiation.",
                    "initialize",
                ));
            }
            match receiver.recv_timeout((deadline - now).min(PROCESS_POLL_INTERVAL)) {
                Ok(TransportEvent::Frame(line)) => {
                    let decoded = (super::parser_port::parser_for(driver.agent_id)
                        .initialize_response)(
                        &line, INITIALIZE_REQUEST_ID
                    );
                    match decoded {
                        Ok(Some(response)) => {
                            break Ok(CapabilityProbe::from_initialize(&response));
                        }
                        Ok(None) => {}
                        Err(error) if error.is_remote_error() => {
                            break Err(ProtocolFailure::new(
                                "acp_initialize_rejected",
                                "The ACP agent rejected protocol initialization.",
                                acp::INITIALIZE_METHOD,
                            ));
                        }
                        Err(error) => break Err(ProtocolFailure::from_acp(error, "protocol/read")),
                    }
                }
                Ok(TransportEvent::StdoutLimitExceeded) => {
                    break Err(ProtocolFailure::new(
                        "acp_protocol_output_limit",
                        "The ACP agent exceeded the configured protocol output limit.",
                        "protocol/read",
                    ));
                }
                Ok(TransportEvent::StdoutReadFailed) => {
                    break Err(ProtocolFailure::new(
                        "acp_protocol_read_failed",
                        "The ACP agent protocol output could not be read.",
                        "protocol/read",
                    ));
                }
                Ok(TransportEvent::StdoutClosed) | Err(RecvTimeoutError::Disconnected) => {
                    break Err(ProtocolFailure::new(
                        "acp_process_exited",
                        "The ACP agent exited during capability negotiation.",
                        "process/exit",
                    ));
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    };
    drop(receiver);
    let cleanup = finish_protocol_transport(&mut child, &mut stdin, stdout_handle, stderr_handle);
    if cleanup == Err(TransportFinishFailure::Lifecycle) {
        return Err(ProtocolFailure::new(
            "acp_process_cleanup_failed",
            "The ACP agent process cleanup could not be completed safely.",
            "process/cleanup",
        ));
    }
    if result.is_ok() && cleanup == Err(TransportFinishFailure::StdinWrite) {
        return Err(ProtocolFailure::new(
            "acp_protocol_write_failed",
            "The ACP agent stopped accepting protocol messages.",
            "protocol/write",
        ));
    }
    result
}
