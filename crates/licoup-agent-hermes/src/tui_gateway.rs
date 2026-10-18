//! Bounded client for Hermes' official TUI Gateway JSON-RPC stdio protocol.
//!
//! The executable and argv are fixed by `SshRuntimeConnection`; caller data is
//! carried only inside framed JSON-RPC messages. Raw guest stderr and protocol
//! error messages never cross this boundary.
//!
//! This is Hermes' own transport, so it is the package's: VENDOR-CODE-REMOVAL
//! moved it out of `licoup-native`'s platform tree together with the turn it
//! drives ([`crate::tui_gateway_driver`]) and the conversation-history
//! projection over it ([`crate::remote_gateway_history`]). The composition above
//! still picks the lane — it is the one place the runtime connection is in view
//! — and names this module's [`RUNTIME_PROTOCOL`]; nothing here reaches back
//! into the client.

use licoup_agent_drivers::acp_session_transport::{
    TransportEvent, read_protocol_messages, write_message,
};
use licoup_agent_targets::platform::user_shell_environment;
use licoup_agent_targets::platform::virtual_machine::SshRuntimeConnection;
use licoup_foundation::platform::process_supervisor::{
    BoundedStdinWriter, SupervisedChild, TransportFinishFailure, finish_protocol_transport,
};
use serde_json::{Value, json};
use std::io::BufReader;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};
use std::{process::Command, process::Stdio};

// The protocol name is vocabulary shared with the Agent inventory's
// virtual-machine projection, which reports it before any driver is launched,
// so the constant lives in the foundation below both. It is re-exported here at
// the boundary that owns the transport that speaks it: the client composition
// reads the lane's identity from this package rather than keeping a copy.
pub use licoup_foundation::core::acp::HERMES_TUI_GATEWAY_PROTOCOL as RUNTIME_PROTOCOL;
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GatewayFailure {
    Start,
    Pipe,
    Write,
    Timeout,
    InvalidJson,
    InvalidMessage,
    OutputLimit,
    Read,
    Exited,
    Rpc,
    Interaction,
    Cleanup,
}

impl GatewayFailure {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::Start => "hermes_gateway_process_start_failed",
            Self::Pipe => "hermes_gateway_process_pipe_failed",
            Self::Write => "hermes_gateway_protocol_write_failed",
            Self::Timeout => "hermes_gateway_protocol_timeout",
            Self::InvalidJson => "hermes_gateway_protocol_invalid_json",
            Self::InvalidMessage => "hermes_gateway_protocol_invalid_message",
            Self::OutputLimit => "hermes_gateway_protocol_output_limit",
            Self::Read => "hermes_gateway_protocol_read_failed",
            Self::Exited => "hermes_gateway_process_exited",
            Self::Rpc => "hermes_gateway_rpc_failed",
            Self::Interaction => "hermes_gateway_user_interaction_required",
            Self::Cleanup => "hermes_gateway_process_cleanup_failed",
        }
    }
}

pub(crate) struct GatewayClient {
    child: SupervisedChild,
    stdin: BoundedStdinWriter,
    receiver: Receiver<TransportEvent>,
    stdout_handle: Option<thread::JoinHandle<()>>,
    stderr_handle: Option<thread::JoinHandle<()>>,
    stderr_truncated: Arc<AtomicBool>,
    observed_stdout_bytes: usize,
    max_stdout_bytes: Option<usize>,
    next_request_id: i64,
    finished: bool,
}

impl GatewayClient {
    pub(crate) fn connect(
        connection: &SshRuntimeConnection,
        max_stdout_bytes: Option<usize>,
        max_stderr_bytes: usize,
    ) -> Result<Self, GatewayFailure> {
        let mut command = connection
            .launch_hermes_tui_gateway_command()
            .map_err(|_| GatewayFailure::Start)?;
        // The local ssh client observes the user shell environment (proxy,
        // agent socket, PATH); `SendEnv=-*` keeps it from crossing the wire.
        user_shell_environment::apply_to_command(&mut command);
        Self::connect_command(command, max_stdout_bytes, max_stderr_bytes)
    }

    fn connect_command(
        mut command: Command,
        max_stdout_bytes: Option<usize>,
        max_stderr_bytes: usize,
    ) -> Result<Self, GatewayFailure> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = SupervisedChild::spawn(&mut command).map_err(|_| GatewayFailure::Start)?;
        let stdout = child.stdout().ok_or(GatewayFailure::Pipe)?;
        let stderr = child.stderr().ok_or(GatewayFailure::Pipe)?;
        let stdin = child.stdin().ok_or(GatewayFailure::Pipe)?;
        let stdin = BoundedStdinWriter::new(stdin);
        let (sender, receiver) = mpsc::channel();
        let stdout_handle =
            thread::spawn(move || read_protocol_messages(BufReader::new(stdout), sender));
        let stderr_truncated = Arc::new(AtomicBool::new(false));
        let stderr_flag = Arc::clone(&stderr_truncated);
        let stderr_handle = thread::spawn(move || {
            licoup_agent_drivers::acp_session_transport::drain_stderr(
                stderr,
                max_stderr_bytes,
                stderr_flag.as_ref(),
            )
        });
        Ok(Self {
            child,
            stdin,
            receiver,
            stdout_handle: Some(stdout_handle),
            stderr_handle: Some(stderr_handle),
            stderr_truncated,
            observed_stdout_bytes: 0,
            max_stdout_bytes,
            next_request_id: 1,
            finished: false,
        })
    }

    #[cfg(test)]
    pub(crate) fn connect_test_command(
        command: Command,
        max_stdout_bytes: Option<usize>,
        max_stderr_bytes: usize,
    ) -> Result<Self, GatewayFailure> {
        Self::connect_command(command, max_stdout_bytes, max_stderr_bytes)
    }

    pub(crate) fn wait_ready(&mut self, deadline: Option<Instant>) -> Result<(), GatewayFailure> {
        let message = self.next_message(deadline)?;
        if event_type(&message) == Some("gateway.ready") && event_session_id(&message).is_none() {
            return Ok(());
        }
        Err(GatewayFailure::InvalidMessage)
    }

    pub(crate) fn request<F>(
        &mut self,
        method: &str,
        params: Value,
        deadline: Option<Instant>,
        mut observe: F,
    ) -> Result<Value, GatewayFailure>
    where
        F: FnMut(&Value) -> Result<(), GatewayFailure>,
    {
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        let request = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
            "params": params,
        });
        write_message(&mut self.stdin, &request).map_err(|_| GatewayFailure::Write)?;
        loop {
            let message = self.next_message(deadline)?;
            if message.get("id").and_then(Value::as_i64) == Some(request_id) {
                if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
                    return Err(GatewayFailure::InvalidMessage);
                }
                if message.get("error").is_some() {
                    return Err(GatewayFailure::Rpc);
                }
                return message
                    .get("result")
                    .cloned()
                    .ok_or(GatewayFailure::InvalidMessage);
            }
            if message.get("id").is_some() {
                return Err(GatewayFailure::InvalidMessage);
            }
            observe(&message)?;
        }
    }

    pub(crate) fn next_message(
        &mut self,
        deadline: Option<Instant>,
    ) -> Result<Value, GatewayFailure> {
        loop {
            self.stdin
                .check_health()
                .map_err(|_| GatewayFailure::Write)?;
            let wait = match deadline {
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        return Err(GatewayFailure::Timeout);
                    }
                    (deadline - now).min(PROCESS_POLL_INTERVAL)
                }
                // No turn deadline: wait a bounded poll slice so the health and
                // output bounds still check, but never against the clock.
                None => PROCESS_POLL_INTERVAL,
            };
            match self.receiver.recv_timeout(wait) {
                Ok(TransportEvent::Message { message, bytes }) => {
                    if let Some(max_stdout_bytes) = self.max_stdout_bytes {
                        self.observed_stdout_bytes =
                            self.observed_stdout_bytes.saturating_add(bytes);
                        if self.observed_stdout_bytes > max_stdout_bytes {
                            return Err(GatewayFailure::OutputLimit);
                        }
                    }
                    return Ok(message);
                }
                Ok(TransportEvent::InvalidJson) => return Err(GatewayFailure::InvalidJson),
                Ok(TransportEvent::LineLimitExceeded) => {
                    return Err(GatewayFailure::OutputLimit);
                }
                Ok(TransportEvent::StdoutReadFailed) => return Err(GatewayFailure::Read),
                Ok(TransportEvent::StdoutClosed) | Err(RecvTimeoutError::Disconnected) => {
                    return Err(GatewayFailure::Exited);
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }

    pub(crate) fn finish(&mut self) -> Result<(), GatewayFailure> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        let stdout_handle = self.stdout_handle.take().ok_or(GatewayFailure::Cleanup)?;
        let stderr_handle = self.stderr_handle.take().ok_or(GatewayFailure::Cleanup)?;
        let cleanup = finish_protocol_transport(
            &mut self.child,
            &mut self.stdin,
            stdout_handle,
            stderr_handle,
        );
        if cleanup == Err(TransportFinishFailure::Lifecycle) {
            return Err(GatewayFailure::Cleanup);
        }
        if self.stderr_truncated.load(Ordering::Relaxed) {
            return Err(GatewayFailure::OutputLimit);
        }
        if cleanup == Err(TransportFinishFailure::StdinWrite) {
            return Err(GatewayFailure::Write);
        }
        Ok(())
    }
}

impl Drop for GatewayClient {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

pub(crate) fn event_type(message: &Value) -> Option<&str> {
    (message.get("jsonrpc").and_then(Value::as_str) == Some("2.0")
        && message.get("method").and_then(Value::as_str) == Some("event"))
    .then(|| message.pointer("/params/type").and_then(Value::as_str))
    .flatten()
}

pub(crate) fn event_session_id(message: &Value) -> Option<&str> {
    message
        .pointer("/params/session_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub(crate) fn event_payload(message: &Value) -> Option<&Value> {
    message.pointer("/params/payload")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_envelope_requires_jsonrpc_event_shape() {
        let message = json!({
            "jsonrpc": "2.0",
            "method": "event",
            "params": {
                "type": "message.delta",
                "session_id": "live-1",
                "payload": {"text": "hello"}
            }
        });
        assert_eq!(event_type(&message), Some("message.delta"));
        assert_eq!(event_session_id(&message), Some("live-1"));
        assert_eq!(
            event_payload(&message).and_then(|value| value.get("text")),
            Some(&json!("hello"))
        );
        assert_eq!(event_type(&json!({"method": "event"})), None);
    }
}
