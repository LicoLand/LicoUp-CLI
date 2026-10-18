//! Run one package's declared converter as a bounded native subprocess.
//!
//! The protocol is the package contract's, and it is deliberately small: the
//! converter is a native program inside the package payload, the tool hands it the
//! roots to read and write plus the format pair it was selected for, and the
//! converter answers with one documented result document. Nothing about the move
//! is decided here — this module owns only the invocation and its bounds.
//!
//! ```text
//! <entry> --source <dir> --target <dir> \
//!         --source-format <identity> --target-format <identity> \
//!         --result <file> [--resume]
//! ```
//!
//! * `--source` is the staged copy of the data root. The converter reads it and
//!   must not write inside it; the run that owns this module verifies that.
//! * `--target` is the directory the converted result is produced in. It is also
//!   the process's working directory.
//! * `--result` is where the converter writes its result document. A run that
//!   exits without one has not reported a conversion, whatever its exit status.
//! * `--resume` states that a previous invocation was interrupted, so the converter
//!   continues the target it already has instead of starting it over.
//!
//! The bounds are the ones a maintenance operation may impose on a program it did
//! not write: captured output is capped (and reported as truncated rather than
//! buffered without limit), the result document is size-bounded and strictly
//! shaped, the child runs in its own process group so a stop reaches the whole
//! tree, and the environment is reduced to the few variables a native program
//! needs to start on the platform. There is deliberately **no automatic
//! deadline**: an elapsed time is not a user's decision, so the caller stops a run
//! it no longer wants instead of the tool inventing a cancellation it cannot
//! attribute.

use crate::error::{CONVERTER_RUN_FAILED, ToolResult};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The result document's schema identity.
pub const RESULT_SCHEMA: &str = "licoup.package-conversion-result.v1";

/// The largest result document this tool reads.
pub const MAX_RESULT_BYTES: u64 = 64 * 1024;

/// How much of each output stream is kept before the rest is discarded.
pub const MAX_CAPTURED_OUTPUT_BYTES: usize = 64 * 1024;

/// How often a running converter is asked whether the caller still wants it.
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How long a stopped converter is given to exit before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(2);

/// The environment variables a native program is given.
///
/// Everything else the tool's own operator environment holds stays with the tool:
/// a package converter is not handed a developer's locator, custody or service
/// environment, and the run is reproducible from the arguments alone.
const PASSED_ENVIRONMENT: [&str; 9] = [
    "PATH",
    "HOME",
    "USERPROFILE",
    "TMPDIR",
    "TEMP",
    "TMP",
    "LANG",
    "LC_ALL",
    "SystemRoot",
];

/// One converter invocation.
pub struct ConverterRequest<'a> {
    /// The converter entry inside the installed package payload.
    pub entry: &'a Path,
    /// The staged source root the converter reads.
    pub source: &'a Path,
    /// The target root the converter produces.
    pub target: &'a Path,
    pub source_format: &'a str,
    pub target_format: &'a str,
    /// Where the converter writes its result document.
    pub result: &'a Path,
    /// Whether a previous invocation was interrupted.
    pub resume: bool,
    /// Asked while the converter runs; a true answer stops the process group.
    pub stop: Option<&'a dyn Fn() -> bool>,
}

/// The converter's own result document, once it is read and checked.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConverterResult {
    pub schema: String,
    pub source_format: String,
    pub target_format: String,
    pub complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub converted_records: Option<u64>,
}

/// What one invocation concluded.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConverterOutcome {
    /// The exit status, when the process reported one.
    pub exit_code: Option<i32>,
    /// Whether the caller stopped the run before it settled.
    pub stopped: bool,
    /// Whether the converter reported a complete conversion of the required pair.
    pub complete: bool,
    /// Why the run is not complete, in the tool's own vocabulary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The result document the converter wrote, when one was read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<ConverterResult>,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

impl ConverterOutcome {
    /// Whether the converter reported the required pair completely.
    pub fn settled(&self) -> bool {
        self.complete && !self.stopped
    }
}

/// Start the converter, wait for it, and read what it reported.
///
/// The refusal a caller receives is only about the *invocation*: a missing entry,
/// a process that could not be started, a process that could not be stopped. A
/// converter that ran and failed, stopped or reported an incomplete conversion is
/// an outcome, not an error, so the run records it and stays resumable.
pub fn run(request: &ConverterRequest<'_>) -> ToolResult<ConverterOutcome> {
    if !request.entry.is_file() {
        return Err(crate::error::CONVERTER_ENTRY_UNEXECUTABLE);
    }
    if !request.source.is_dir() || !request.target.is_dir() {
        return Err(CONVERTER_RUN_FAILED);
    }
    let mut command = Command::new(request.entry);
    command
        .arg("--source")
        .arg(request.source)
        .arg("--target")
        .arg(request.target)
        .arg("--source-format")
        .arg(request.source_format)
        .arg("--target-format")
        .arg(request.target_format)
        .arg("--result")
        .arg(request.result);
    if request.resume {
        command.arg("--resume");
    }
    command
        .current_dir(request.target)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.env_clear();
    for name in PASSED_ENVIRONMENT {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }

    use command_group::CommandGroup;
    let mut child = command.group_spawn().map_err(|_| CONVERTER_RUN_FAILED)?;

    let stdout_handle = capture(child.inner().stdout.take());
    let stderr_handle = capture(child.inner().stderr.take());

    let mut stopped = false;
    let exit = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if request.stop.is_some_and(|stop| stop()) {
            stopped = true;
            stop_group(&mut child);
            break child.try_wait().ok().flatten();
        }
        std::thread::sleep(STOP_POLL_INTERVAL);
    };

    let stdout = stdout_handle.join().unwrap_or_default();
    let stderr = stderr_handle.join().unwrap_or_default();

    let result = read_result(request.result);
    let mut outcome = ConverterOutcome {
        exit_code: exit.and_then(|status| status.code()),
        stopped,
        complete: false,
        reason: None,
        result: None,
        stdout_bytes: stdout.bytes,
        stderr_bytes: stderr.bytes,
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
    };
    if stopped {
        outcome.reason = Some(crate::error::PACKAGE_CONVERSION_STOPPED.code().to_string());
        return Ok(outcome);
    }
    if !exit.is_some_and(|status| status.success()) {
        outcome.reason = Some(CONVERTER_RUN_FAILED.code().to_string());
        return Ok(outcome);
    }
    let Some(result) = result else {
        outcome.reason = Some(crate::error::CONVERTER_RESULT_INVALID.code().to_string());
        return Ok(outcome);
    };
    if result.schema != RESULT_SCHEMA
        || result.source_format != request.source_format
        || result.target_format != request.target_format
    {
        outcome.result = Some(result);
        outcome.reason = Some(crate::error::CONVERTER_RESULT_INVALID.code().to_string());
        return Ok(outcome);
    }
    outcome.complete = result.complete;
    if !result.complete {
        outcome.reason = Some(crate::error::CONVERTER_RESULT_INCOMPLETE.code().to_string());
    }
    outcome.result = Some(result);
    Ok(outcome)
}

/// Read one bounded result document, or `None` when there is not one to read.
fn read_result(path: &Path) -> Option<ConverterResult> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_RESULT_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// What one captured stream held.
#[derive(Default)]
struct Captured {
    bytes: u64,
    truncated: bool,
}

/// Drain one stream on its own thread, keeping the first bounded bytes.
///
/// The whole stream is drained even after the bound is reached: a child that keeps
/// writing must not block on a full pipe just because this tool stopped reading.
fn capture(stream: Option<impl Read + Send + 'static>) -> std::thread::JoinHandle<Captured> {
    let handle = std::thread::spawn(move || {
        let mut captured = Captured::default();
        let Some(mut stream) = stream else {
            return captured;
        };
        let mut buffer = [0_u8; 8 * 1024];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    captured.bytes += read as u64;
                    if captured.bytes > MAX_CAPTURED_OUTPUT_BYTES as u64 {
                        captured.truncated = true;
                    }
                }
            }
        }
        captured
    });
    handle
}

/// Stop the converter and everything it started.
fn stop_group(child: &mut command_group::GroupChild) {
    #[cfg(unix)]
    {
        use command_group::{Signal, UnixChildExt};
        let _ = child.signal(Signal::SIGTERM);
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
    let deadline = Instant::now() + STOP_GRACE;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) => std::thread::sleep(STOP_POLL_INTERVAL),
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}
