//! Generic CLI/PTY fallback for catalog agents without a dedicated driver.
//!
//! Dedicated `*_driver` adapters stay highest capability. This lane is used
//! only after `adapter_for_agent` returns None and a CLI registration exists.

use crate::domain::cli_registration::{CliRegistration, StreamMode};
use crate::platform::process_supervisor::{IO_THREAD_EXIT_GRACE, SupervisedChild, join_bounded};
use crate::platform::raw_execution::{RawExecutionDirection, RawExecutionObserver};
use crate::platform::runtime_adapters::RuntimeAdapterError;
use crate::platform::turn_event_emit::emit_agent_message_chunk;
use serde_json::Value;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(crate) const DRIVER_ID: &str = "generic-cli";
pub(crate) const PTY_PROTOCOL: &str = "cli-pty-v1";
pub(crate) const STDIO_PROTOCOL: &str = "cli-stdio-v1";

#[derive(Debug)]
pub(crate) struct RunResult {
    pub ok: bool,
    pub output: String,
    pub status_code: Option<i32>,
    pub timed_out: bool,
    pub stdout_truncated: bool,
    pub started_at: String,
    pub runtime_protocol: &'static str,
}

pub(crate) fn execute(
    registration: &CliRegistration,
    executable: &str,
    params: &Value,
    text: &str,
    cwd: Option<&Path>,
    timeout_ms: u64,
    max_stdout: Option<usize>,
) -> Result<RunResult, RuntimeAdapterError> {
    let started_at = unix_millis();
    // The generic CLI/PTY lane has no structured permission control. A
    // profile that carries one must fail closed rather than silently drop it.
    if text_setting(params, &["permissionMode", "permission_mode"]).is_some() {
        return Err(RuntimeAdapterError::InvalidRuntimeSetting {
            field: "permissionMode",
        });
    }
    let model = text_setting(params, &["model"]);
    let effort = text_setting(params, &["reasoningEffort", "effort"]);
    let (args, write_stdin) = substitute_args(&registration.args, text, model, effort);
    let mut command = Command::new(executable);
    command.args(&args);
    if let Some(observer) = RawExecutionObserver::current() {
        for (index, arg) in args.iter().enumerate() {
            observer.record(
                &format!("generic-cli.argv.{index}"),
                RawExecutionDirection::Sent,
                arg,
            );
        }
    }
    if let Some(workspace) = cwd {
        command.current_dir(workspace);
    }
    let stream_mode = effective_stream_mode(registration.stream_mode);
    let runtime_protocol = match stream_mode {
        StreamMode::Pty => PTY_PROTOCOL,
        StreamMode::Stdio => STDIO_PROTOCOL,
    };
    let output = match stream_mode {
        StreamMode::Pty => run_pty(command, text, write_stdin, timeout_ms, max_stdout)?,
        StreamMode::Stdio => run_stdio(command, text, write_stdin, timeout_ms, max_stdout)?,
    };
    Ok(RunResult {
        ok: output.ok && !output.timed_out,
        output: output.text,
        status_code: output.status_code,
        timed_out: output.timed_out,
        stdout_truncated: output.truncated,
        started_at,
        runtime_protocol,
    })
}

pub(crate) fn resolve_executable(
    registration: &CliRegistration,
    params: &Value,
) -> Result<String, RuntimeAdapterError> {
    if let Some(requested) = text_setting(params, &["binary", "binaryPath", "executable"]) {
        return existing_file(requested).or_else(|_| {
            if Path::new(requested).is_absolute() {
                Err(RuntimeAdapterError::ExecutableUnavailable)
            } else {
                discovered_or_command(registration)
            }
        });
    }
    discovered_or_command(registration)
}

fn discovered_or_command(registration: &CliRegistration) -> Result<String, RuntimeAdapterError> {
    if let Some(discovered) = crate::domain::targets::available_runtime_executable(
        &crate::target_port::agent_target_port(),
        &registration.id,
    )
    .or_else(|| crate::domain::targets::agent_cli_executable(&registration.id))
    {
        return discovered
            .to_str()
            .map(str::to_owned)
            .ok_or(RuntimeAdapterError::ExecutableUnavailable);
    }
    if Path::new(&registration.command).is_absolute() {
        return existing_file(&registration.command);
    }
    if registration.command.trim().is_empty() {
        return Err(RuntimeAdapterError::ExecutableUnavailable);
    }
    Ok(registration.command.clone())
}

fn existing_file(path: &str) -> Result<String, RuntimeAdapterError> {
    let canonical =
        std::fs::canonicalize(path).map_err(|_| RuntimeAdapterError::ExecutableUnavailable)?;
    if !canonical.is_file() {
        return Err(RuntimeAdapterError::ExecutableUnavailable);
    }
    canonical
        .to_str()
        .map(str::to_owned)
        .ok_or(RuntimeAdapterError::ExecutableUnavailable)
}

fn substitute_args(
    args: &[String],
    prompt: &str,
    model: Option<&str>,
    effort: Option<&str>,
) -> (Vec<String>, bool) {
    let mut resolved = Vec::with_capacity(args.len());
    let mut used_prompt = false;
    for arg in args {
        if arg.contains("{prompt}") {
            used_prompt = true;
        }
        resolved.push(
            arg.replace("{prompt}", prompt)
                .replace("{model}", model.unwrap_or(""))
                .replace("{effort}", effort.unwrap_or("")),
        );
    }
    (resolved, !used_prompt)
}

fn effective_stream_mode(requested: StreamMode) -> StreamMode {
    match requested {
        StreamMode::Pty if cfg!(unix) => StreamMode::Pty,
        _ => StreamMode::Stdio,
    }
}

struct Captured {
    text: String,
    ok: bool,
    timed_out: bool,
    truncated: bool,
    status_code: Option<i32>,
}

fn run_stdio(
    mut command: Command,
    prompt: &str,
    write_stdin: bool,
    timeout_ms: u64,
    max_stdout: Option<usize>,
) -> Result<Captured, RuntimeAdapterError> {
    command
        .stdin(if write_stdin {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = SupervisedChild::spawn(&mut command)
        .map_err(|_| RuntimeAdapterError::ExecutableUnavailable)?;
    if write_stdin && let Some(mut stdin) = child.stdin() {
        let mut payload = prompt.as_bytes().to_vec();
        if !prompt.ends_with('\n') {
            payload.push(b'\n');
        }
        if let Some(observer) = RawExecutionObserver::current() {
            observer.record_bytes("generic-cli.stdin", RawExecutionDirection::Sent, &payload);
        }
        let _ = stdin.write_all(&payload);
    }
    let stdout = child
        .stdout()
        .ok_or(RuntimeAdapterError::ConversationDispatchFailed)?;
    collect_output(child, stdout, timeout_ms, max_stdout, true)
}

#[cfg(unix)]
fn run_pty(
    mut command: Command,
    prompt: &str,
    write_stdin: bool,
    timeout_ms: u64,
    max_stdout: Option<usize>,
) -> Result<Captured, RuntimeAdapterError> {
    command.stderr(Stdio::piped());
    let (child, mut master) = crate::platform::pty_transport::spawn(command)
        .map_err(|_| RuntimeAdapterError::ExecutableUnavailable)?;
    if write_stdin {
        let _ = master.write_all(prompt.as_bytes());
        if !prompt.ends_with('\n') {
            let _ = master.write_all(b"\n");
        }
    }
    collect_output(child, master, timeout_ms, max_stdout, false)
}

#[cfg(not(unix))]
fn run_pty(
    command: Command,
    prompt: &str,
    write_stdin: bool,
    timeout_ms: u64,
    max_stdout: Option<usize>,
) -> Result<Captured, RuntimeAdapterError> {
    run_stdio(command, prompt, write_stdin, timeout_ms, max_stdout)
}

fn collect_output<R: Read + Send + 'static>(
    mut child: SupervisedChild,
    reader: R,
    timeout_ms: u64,
    max_stdout: Option<usize>,
    capture_stdout: bool,
) -> Result<Captured, RuntimeAdapterError> {
    let stderr_observer = RawExecutionObserver::current();
    let stderr_handle = child.stderr().map(|mut stderr| {
        thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            loop {
                match stderr.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        if let Some(observer) = stderr_observer.as_ref() {
                            observer.record_bytes(
                                "generic-cli.stderr",
                                RawExecutionDirection::Stderr,
                                &buffer[..count],
                            );
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        })
    });
    let stdout_observer = capture_stdout.then(RawExecutionObserver::current).flatten();
    let (sender, receiver) = mpsc::channel::<Option<Vec<u8>>>();
    thread::spawn(move || {
        let mut reader = reader;
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    let _ = sender.send(None);
                    break;
                }
                Ok(count) => {
                    if let Some(observer) = stdout_observer.as_ref() {
                        observer.record_bytes(
                            "generic-cli.stdout",
                            RawExecutionDirection::Received,
                            &buffer[..count],
                        );
                    }
                    if sender.send(Some(buffer[..count].to_vec())).is_err() {
                        break;
                    }
                }
                Err(_) => {
                    let _ = sender.send(None);
                    break;
                }
            }
        }
    });
    let deadline = (timeout_ms > 0).then(|| Instant::now() + Duration::from_millis(timeout_ms));
    let mut output = String::new();
    let mut truncated = false;
    let mut timed_out = false;
    loop {
        let remaining = deadline.map(|end| end.saturating_duration_since(Instant::now()));
        if remaining == Some(Duration::ZERO) {
            timed_out = true;
            break;
        }
        let received = match remaining {
            Some(wait) => match receiver.recv_timeout(wait) {
                Ok(chunk) => chunk,
                Err(RecvTimeoutError::Timeout) => {
                    timed_out = true;
                    break;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            },
            None => match receiver.recv() {
                Ok(chunk) => chunk,
                Err(_) => break,
            },
        };
        let Some(bytes) = received else {
            break;
        };
        let chunk = String::from_utf8_lossy(&bytes);
        if !chunk.is_empty() {
            emit_agent_message_chunk("", "", &chunk);
            if let Some(limit) = max_stdout {
                let remaining = limit.saturating_sub(output.len());
                if remaining == 0 {
                    truncated = true;
                    continue;
                }
                if chunk.len() > remaining {
                    output.push_str(&chunk[..remaining]);
                    truncated = true;
                    continue;
                }
            }
            output.push_str(&chunk);
        }
    }
    let status_code = if timed_out {
        let _ = child.terminate_tree();
        child
            .try_wait()
            .ok()
            .flatten()
            .and_then(|status| status.code())
    } else {
        let deadline = Instant::now() + Duration::from_millis(200);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => break status.code(),
                _ if Instant::now() >= deadline => {
                    let _ = child.terminate_tree();
                    break child
                        .try_wait()
                        .ok()
                        .flatten()
                        .and_then(|status| status.code());
                }
                _ => thread::sleep(Duration::from_millis(10)),
            }
        }
    };
    if let Some(handle) = stderr_handle {
        let _ = join_bounded(handle, IO_THREAD_EXIT_GRACE);
    }
    Ok(Captured {
        ok: !timed_out && status_code.unwrap_or(0) == 0,
        text: output,
        timed_out,
        truncated,
        status_code,
    })
}

fn text_setting<'a>(params: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| {
        params
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
}

fn unix_millis() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
}

#[cfg(unix)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::cli_registration::{CliRegistration, StreamMode};
    use serde_json::json;
    use std::path::{MAIN_SEPARATOR, PathBuf};

    fn echo_registration() -> CliRegistration {
        CliRegistration {
            id: "echo-fixture".to_owned(),
            label: "Echo".to_owned(),
            command: "/bin/echo".to_owned(),
            args: vec!["{prompt}".to_owned()],
            stream_mode: StreamMode::Stdio,
        }
    }

    #[test]
    fn stdio_lane_echoes_prompt_from_args() {
        let registration = echo_registration();
        let result = execute(
            &registration,
            "/bin/echo",
            &json!({"agentId": "echo-fixture"}),
            "wave-seven-role",
            None,
            5_000,
            None,
        )
        .expect("echo execute");
        assert!(result.ok);
        assert!(result.output.contains("wave-seven-role"));
        assert_eq!(result.runtime_protocol, STDIO_PROTOCOL);
        assert!(!result.stdout_truncated);
    }

    #[test]
    fn stdio_lane_reads_prompt_from_stdin_when_args_have_no_placeholder() {
        let registration = CliRegistration {
            id: "cat-fixture".to_owned(),
            label: "Cat".to_owned(),
            command: "/bin/cat".to_owned(),
            args: Vec::new(),
            stream_mode: StreamMode::Stdio,
        };
        let result = execute(
            &registration,
            "/bin/cat",
            &json!({}),
            "stdin-prompt",
            None,
            5_000,
            None,
        )
        .expect("cat execute");
        assert!(result.ok);
        assert!(result.output.contains("stdin-prompt"));
    }

    #[test]
    fn raw_stdio_keeps_stdin_stdout_and_stderr_outside_the_output_cap() {
        let records = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = std::sync::Arc::clone(&records);
        let observer = RawExecutionObserver::new(move |source, direction, raw| {
            sink.lock()
                .unwrap()
                .push((source.to_owned(), direction, raw.to_owned()));
            Ok(())
        });
        let _scope = crate::platform::raw_execution::RawExecutionScope::enter(Some(observer));
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "cat; printf 'metadata-tail' >&2"]);
        let result = run_stdio(command, "complete-prompt", true, 5_000, Some(4)).unwrap();
        assert_eq!(result.text, "comp");
        assert!(result.truncated);
        let records = records.lock().unwrap();
        for (source, direction, expected) in [
            (
                "generic-cli.stdin",
                RawExecutionDirection::Sent,
                "complete-prompt\n",
            ),
            (
                "generic-cli.stdout",
                RawExecutionDirection::Received,
                "complete-prompt\n",
            ),
            (
                "generic-cli.stderr",
                RawExecutionDirection::Stderr,
                "metadata-tail",
            ),
        ] {
            assert_eq!(
                records
                    .iter()
                    .filter(|record| record.0 == source && record.1 == direction)
                    .map(|record| record.2.as_str())
                    .collect::<String>(),
                expected
            );
        }
    }

    #[test]
    fn missing_absolute_command_is_unavailable() {
        let missing = PathBuf::from(MAIN_SEPARATOR.to_string()).join("missing-licoup-generic-cli");
        let registration = CliRegistration {
            id: "missing-fixture".to_owned(),
            label: "Missing".to_owned(),
            command: missing.to_string_lossy().into_owned(),
            args: Vec::new(),
            stream_mode: StreamMode::Stdio,
        };
        let error = resolve_executable(&registration, &json!({})).unwrap_err();
        assert_eq!(error, RuntimeAdapterError::ExecutableUnavailable);
    }
}
