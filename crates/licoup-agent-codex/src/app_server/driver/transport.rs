use super::super::config::ProtocolConfig;
use super::super::model::{ProtocolFailure, RunResult};
use super::io::{drain_stderr, read_protocol_messages, write_message};
use super::launch::CodexLaunchSpec;
use super::supervision::{pipe_failure, run_protocol_loop};
use crate::parser::CodexParser;
use licoup_foundation::platform::process_supervisor::{
    BoundedStdinWriter, TransportFinishFailure, finish_protocol_transport,
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
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[allow(clippy::too_many_arguments)]
pub fn execute(
    executable: &str,
    params: &Value,
    prompt: &str,
    session_id: &str,
    cwd: Option<&Path>,
    timeout_ms: u64,
    max_stdout: Option<usize>,
    max_stderr: usize,
    environment: Option<Vec<(String, String)>>,
) -> RunResult {
    let started_at = timestamp();
    let config = match ProtocolConfig::from_params(params, prompt, session_id, cwd) {
        Ok(config) => config,
        Err(failure) => return RunResult::failed(failure, started_at, None, false, false),
    };
    let launch = match environment {
        Some(environment) => CodexLaunchSpec::new(executable, cwd).with_environment(environment),
        None => CodexLaunchSpec::new(executable, cwd),
    };
    let mut child = match launch.spawn_with_context(Some(params)) {
        Ok(child) => child,
        Err(error) => {
            let message = match error.kind() {
                io::ErrorKind::NotFound => "The Codex executable is not available.",
                io::ErrorKind::PermissionDenied => "The Codex executable is not permitted to run.",
                _ => "Codex app-server could not be started.",
            };
            return RunResult::failed(
                ProtocolFailure::new("codex_app_server_start_failed", message, "process/start"),
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
            "Codex app-server stdout is unavailable.",
        );
    };
    let Some(stderr) = child.stderr() else {
        return pipe_failure(
            &mut child,
            started_at,
            "Codex app-server stderr is unavailable.",
        );
    };
    let Some(stdin) = child.stdin() else {
        return pipe_failure(
            &mut child,
            started_at,
            "Codex app-server stdin is unavailable.",
        );
    };
    let mut stdin = BoundedStdinWriter::new(stdin);

    let (sender, receiver) = mpsc::channel();
    let raw_observer = RawExecutionObserver::current();
    let stdout_binding = RawExecutionBinding::default();
    let _raw_stdout_scope = stdout_binding.bind(raw_observer.clone());
    let stdout_handle = thread::spawn(move || {
        let reader = RawExecutionReader::new(
            stdout,
            stdout_binding,
            "codex-app-server",
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

    let mut protocol = CodexParser::new(config);
    let (control_sender, control_receiver) = mpsc::sync_channel(16);
    if write_message(&mut stdin, &protocol.initial_request()).is_err() {
        let cleanup =
            finish_protocol_transport(&mut child, &mut stdin, stdout_handle, stderr_handle);
        let cleanup_failed = cleanup == Err(TransportFinishFailure::Lifecycle);
        return RunResult::failed(
            ProtocolFailure::new(
                if cleanup_failed {
                    "codex_app_server_cleanup_failed"
                } else {
                    "codex_app_server_write_failed"
                },
                if cleanup_failed {
                    "Codex app-server process cleanup could not be completed safely."
                } else {
                    "Codex app-server stopped accepting protocol messages."
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
            protocol.contextualize(ProtocolFailure::new(
                "codex_app_server_cleanup_failed",
                "Codex app-server process cleanup could not be completed safely.",
                "process/cleanup",
            )),
            started_at,
            status_code,
            stdout_was_truncated,
            stderr_was_truncated,
        );
    }
    if outcome.is_some() && cleanup == Err(TransportFinishFailure::StdinWrite) {
        return RunResult::failed(
            protocol.contextualize(ProtocolFailure::new(
                "codex_app_server_write_failed",
                "Codex app-server stopped accepting protocol messages.",
                "protocol/write",
            )),
            started_at,
            status_code,
            stdout_was_truncated,
            stderr_was_truncated,
        );
    }

    if let Some(outcome) = outcome {
        let transitions = crate::parser::completed_transitions(&outcome.output);
        return RunResult {
            ok: true,
            output: outcome.output,
            transitions,
            error: None,
            session_id: outcome.session_id,
            thread_id: outcome.thread_id,
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
            protocol.contextualize(ProtocolFailure::new(
                "codex_app_server_failed",
                "Codex app-server did not complete the request.",
                "protocol",
            ))
        }),
        started_at,
        status_code,
        stdout_was_truncated,
        stderr_was_truncated,
    )
}

fn timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
}
