use super::control::ControlRequest;
use super::failure::{ProtocolFailure, pipe_failure};
use super::io::{TransportEvent, drain_stderr, read_protocol_messages};
use crate::protocol::LaunchIdentity;
use licoup_foundation::platform::process_supervisor::{
    BoundedStdinWriter, SupervisedChild, TransportFinishFailure, finish_protocol_transport,
};
use licoup_foundation::platform::raw_execution::{
    RawExecutionBinding, RawExecutionBindingGuard, RawExecutionDirection, RawExecutionReader,
};
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver};
use std::thread;

#[derive(Debug)]
pub(super) struct PersistentTransport {
    child: SupervisedChild,
    pub(super) stdin: BoundedStdinWriter,
    pub(super) receiver: Receiver<TransportEvent>,
    pub(super) control_receiver: Receiver<ControlRequest>,
    stdout_handle: Option<thread::JoinHandle<()>>,
    stderr_handle: Option<thread::JoinHandle<()>>,
    pub(super) stderr_truncated: Arc<AtomicBool>,
    raw_execution: RawExecutionBinding,
    initial_raw_execution: Option<RawExecutionBindingGuard>,
    closed: bool,
    join_probe_path: Option<PathBuf>,
}

impl PersistentTransport {
    pub(super) fn spawn(
        identity: &LaunchIdentity,
        control_receiver: Receiver<ControlRequest>,
        max_stderr: usize,
    ) -> Result<Self, ProtocolFailure> {
        let mut child = super::launch::spawn(identity).map_err(|error| {
            let message = match error.kind() {
                std::io::ErrorKind::NotFound => "The Claude Code executable is not available.",
                std::io::ErrorKind::PermissionDenied => {
                    "The Claude Code executable is not permitted to run."
                }
                _ => "Claude Code could not be started.",
            };
            ProtocolFailure::new("claude_code_start_failed", message, "process/start")
        })?;
        let stdout = child.stdout().ok_or_else(pipe_failure)?;
        let stderr = child.stderr().ok_or_else(pipe_failure)?;
        let stdin = child.stdin().ok_or_else(pipe_failure)?;
        let raw_execution = RawExecutionBinding::default();
        let initial_raw_execution = Some(raw_execution.bind_current());
        let (sender, receiver) = mpsc::channel();
        let stdout = RawExecutionReader::new(
            stdout,
            raw_execution.clone(),
            "claude-code",
            RawExecutionDirection::Received,
        );
        let stdout_handle =
            thread::spawn(move || read_protocol_messages(BufReader::new(stdout), sender));
        let stderr_truncated = Arc::new(AtomicBool::new(false));
        let stderr_flag = Arc::clone(&stderr_truncated);
        let stderr_observer = raw_execution.clone();
        let stderr_handle =
            thread::spawn(move || drain_stderr(stderr, max_stderr, &stderr_flag, &stderr_observer));
        Ok(Self {
            child,
            stdin: BoundedStdinWriter::new(stdin),
            receiver,
            control_receiver,
            stdout_handle: Some(stdout_handle),
            stderr_handle: Some(stderr_handle),
            stderr_truncated,
            raw_execution,
            initial_raw_execution,
            closed: false,
            join_probe_path: std::env::var_os("LICO_TEST_CLAUDE_TRANSPORT_JOIN_PROBE_ROOT")
                .and_then(|_| identity.cwd.as_ref())
                .map(|cwd| cwd.join("fake-claude-transport-workers.joined")),
        })
    }

    pub(super) fn bind_raw_execution(&mut self) -> RawExecutionBindingGuard {
        match self.initial_raw_execution.take() {
            Some(guard) => guard.rebind_current(),
            None => self.raw_execution.bind_current(),
        }
    }

    pub(super) fn shutdown(&mut self) -> Result<(), TransportFinishFailure> {
        if self.closed {
            return Ok(());
        }
        let stdout = self
            .stdout_handle
            .take()
            .ok_or(TransportFinishFailure::Lifecycle)?;
        let stderr = self
            .stderr_handle
            .take()
            .ok_or(TransportFinishFailure::Lifecycle)?;
        finish_protocol_transport(&mut self.child, &mut self.stdin, stdout, stderr)?;
        self.closed = true;
        if let Some(path) = self.join_probe_path.as_ref() {
            std::fs::write(path, b"joined").map_err(|_| TransportFinishFailure::Lifecycle)?;
        }
        Ok(())
    }
}

impl Drop for PersistentTransport {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}
