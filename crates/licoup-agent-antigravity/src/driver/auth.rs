//! Bounded Antigravity account authorization gate.
//!
//! The vendor `agy` CLI auto-opens a browser OAuth flow whenever a print-mode
//! turn runs without a login (`printmode.go` → `auth_manager` → `browser`).
//! Product policy requires explicit consent for external authorization, so
//! the send lane probes the login state first and returns a structured
//! auth-required failure instead of spawning a turn that would jump to the
//! browser. Only the explicit `authorize` action below may start the vendor
//! OAuth flow. No token, account, or browser detail is read or returned.

use super::errors::ProtocolFailure;
use super::model::{DRIVER_ID, PROCESS_POLL_INTERVAL};
use licoup_agent_targets::platform::user_shell_environment::apply_to_command;
use licoup_foundation::platform::process_supervisor::{
    IO_THREAD_EXIT_GRACE, SupervisedChild, join_bounded,
};
use serde_json::{Value, json};
use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const AUTH_PROBE_TIMEOUT: Duration = Duration::from_secs(15);
const AUTHORIZE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const MAX_PROBE_OUTPUT_BYTES: usize = 8 * 1024;
const DEFAULT_EXECUTABLE: &str = "agy";
const AUTHORIZE_PROMPT: &str = "Confirm the Antigravity sign-in by replying with exactly: OK";

pub(super) const AUTH_REQUIRED_CODE: &str = "antigravity_auth_required";

/// Fail closed when the vendor CLI reports a missing login. An inconclusive
/// probe never blocks a send; the turn lane keeps its own bounded failures.
pub(super) fn ensure_authorized(executable: &str) -> Result<(), ProtocolFailure> {
    match probe_authorization(executable) {
        AuthProbe::Authorized | AuthProbe::Inconclusive => Ok(()),
        AuthProbe::AuthorizationRequired => Err(ProtocolFailure::new(
            AUTH_REQUIRED_CODE,
            "Antigravity requires Google account authorization before sending.",
            "session/auth",
        )
        .with_user_interaction()),
    }
}

/// Explicit, user-consented vendor OAuth start. This is the only LicoUp path
/// allowed to open the browser: the vendor CLI runs one bounded print turn,
/// which starts its interactive OAuth flow when logged out. The follow-up
/// probe reports whether authorization completed.
pub fn authorize(executable: Option<&str>) -> Result<Value, &'static str> {
    let executable = executable
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_EXECUTABLE);
    if let AuthProbe::Authorized = probe_authorization(executable) {
        return Ok(authorize_report(true));
    }
    let mut command = Command::new(executable);
    // The explicit vendor OAuth flow is a CLI invocation of this adapter: it
    // observes the same user shell environment as a terminal launch (proxy,
    // login state), per the environment-equivalence invariant.
    apply_to_command(&mut command);
    command
        .arg(format!("--print={AUTHORIZE_PROMPT}"))
        .arg("--dangerously-skip-permissions")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if run_bounded(&mut command, AUTHORIZE_TIMEOUT, MAX_PROBE_OUTPUT_BYTES).is_none() {
        return Err("antigravity_authorize_unavailable");
    }
    let authorized = matches!(probe_authorization(executable), AuthProbe::Authorized);
    Ok(authorize_report(authorized))
}

fn authorize_report(authorized: bool) -> Value {
    json!({
        "ok": true,
        "adapterId": "antigravity",
        "driverId": DRIVER_ID,
        "action": "authorize",
        "authorized": authorized,
        "status": if authorized { "authorized" } else { "authorization_incomplete" },
        "privacy": "aggregate-only"
    })
}

enum AuthProbe {
    Authorized,
    AuthorizationRequired,
    Inconclusive,
}

fn probe_authorization(executable: &str) -> AuthProbe {
    let mut command = Command::new(executable);
    // The authorization probe is a CLI invocation of this adapter: it observes
    // the same user shell environment as a terminal launch (proxy, login
    // state), per the environment-equivalence invariant.
    apply_to_command(&mut command);
    command
        .arg("models")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let Some((success, output)) =
        run_bounded(&mut command, AUTH_PROBE_TIMEOUT, MAX_PROBE_OUTPUT_BYTES)
    else {
        return AuthProbe::Inconclusive;
    };
    if success {
        return AuthProbe::Authorized;
    }
    let lowered = output.to_ascii_lowercase();
    if lowered.contains("please sign in") || lowered.contains("not logged into antigravity") {
        AuthProbe::AuthorizationRequired
    } else {
        AuthProbe::Inconclusive
    }
}

/// Spawn and join a bounded child. Returns `None` on spawn failure or
/// timeout; otherwise the exit success flag and combined sanitized output.
fn run_bounded(
    command: &mut Command,
    timeout: Duration,
    max_output: usize,
) -> Option<(bool, String)> {
    let mut child = SupervisedChild::spawn(command).ok()?;
    let stdout = child.stdout()?;
    let stderr = child.stderr()?;
    let stdout_handle = thread::spawn(move || read_bounded(stdout, max_output));
    let stderr_handle = thread::spawn(move || read_bounded(stderr, max_output));
    let deadline = Instant::now() + timeout;
    while (!stdout_handle.is_finished() || !stderr_handle.is_finished())
        && Instant::now() < deadline
    {
        thread::sleep(PROCESS_POLL_INTERVAL);
    }
    if !stdout_handle.is_finished() || !stderr_handle.is_finished() {
        let _ = child.terminate_tree();
        let _ = join_bounded(stdout_handle, IO_THREAD_EXIT_GRACE);
        let _ = join_bounded(stderr_handle, IO_THREAD_EXIT_GRACE);
        return None;
    }
    let status = child.terminate_tree().ok().flatten();
    let stdout = join_bounded(stdout_handle, IO_THREAD_EXIT_GRACE).ok()?;
    let stderr = join_bounded(stderr_handle, IO_THREAD_EXIT_GRACE).ok()?;
    Some((
        status.is_some_and(|value| value.success()),
        format!("{}\n{}", stdout.text, stderr.text),
    ))
}

struct BoundedRead {
    text: String,
}

fn read_bounded(mut reader: impl Read, max_output: usize) -> BoundedRead {
    let mut buffer = vec![0_u8; 8192.min(max_output.max(1))];
    let mut collected = Vec::new();
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(_) => break,
        };
        if collected.len() >= max_output {
            break;
        }
        let take = read.min(max_output - collected.len());
        collected.extend_from_slice(&buffer[..take]);
        if take < read {
            break;
        }
    }
    BoundedRead {
        text: String::from_utf8_lossy(&collected).into_owned(),
    }
}
