use super::control::{cancel_claimed, clear_active_turn, register_active_turn, take_cancelled};
use super::errors::ProtocolFailure;
use super::hooks::{ensure_hook_bridge, read_conversation_id, receipt_path_for_turn};
use super::model::{EffectiveSettings, PROCESS_POLL_INTERVAL, RECEIPT_ENV, RunResult};
use crate::parser::{PtyOutputParser, TerminalFacts, classify_terminal, valid_session_id};
#[cfg(unix)]
use crate::port::turn_event::{
    emit_agent_message_chunk, emit_agent_message_completed, emit_agent_processing, emit_turn_event,
};
use licoup_agent_drivers::runtime_adapters::{
    apply_mcp_runtime_root, apply_subagent_caller_context,
};
use licoup_agent_targets::platform::user_shell_environment::apply_to_command;
use licoup_foundation::platform::agent_workspace::resolve_local_agent_workspace;
#[cfg(not(unix))]
use licoup_foundation::platform::process_supervisor::SupervisedChild;
use licoup_foundation::platform::process_supervisor::{IO_THREAD_EXIT_GRACE, join_bounded};
#[cfg(unix)]
use licoup_foundation::platform::pty_transport::PtyEvent;
use licoup_foundation::platform::raw_execution::{
    RawExecutionDirection, RawExecutionObserver, RawExecutionScope,
};
use serde_json::{Value, json};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
#[cfg(unix)]
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The only permission execution the Antigravity CLI exposes to LicoUp is
/// `--dangerously-skip-permissions`; its canonical mode name is the flag
/// itself. An omitted policy keeps this launch default, an explicit value is
/// accepted only when it names the same execution, and every other permission
/// or approval policy is rejected before any process is started, so the
/// effective report can never disagree with the executed command.
pub(super) const DANGEROUS_SKIP_MODE: &str = "dangerously-skip-permissions";

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
    let config = match LaunchConfig::from_params(params, prompt, session_id, cwd) {
        Ok(config) => config,
        Err(failure) => return RunResult::failed(failure, started_at, false, false),
    };
    // Admission gate: the adapter package asks this host whether it currently
    // admits a new execution, and the answer is the host's own close-admission
    // barrier. A turn started while a maintenance switch holds that barrier
    // would run a vendor CLI the switch may be replacing, so the refusal is
    // reported before any process starts. A host that installed no port on the
    // package answers fail-closed and this turn never claims it was admitted.
    if !crate::port::execution::admits_execution() {
        return RunResult::failed(
            ProtocolFailure::new(
                "antigravity_execution_admission_closed",
                "Antigravity execution is not admitted while this client is changing installed state.",
                "turn/execute",
            )
            .with_session(Some(session_id)),
            started_at,
            false,
            false,
        );
    }
    // Consent gate: the vendor CLI opens a browser OAuth flow for print turns
    // while logged out. Probe first so a send never jumps to the browser. The
    // validated configuration above already rejects unsupported executions.
    if let Err(failure) = super::auth::ensure_authorized(executable) {
        return RunResult::failed(
            failure.with_session(Some(session_id)),
            started_at,
            false,
            false,
        );
    }
    if let Err(failure) = ensure_hook_bridge() {
        return RunResult::failed(
            failure.with_session(Some(session_id)),
            started_at,
            false,
            false,
        );
    }
    let receipt = match receipt_path_for_turn() {
        Ok(path) => path,
        Err(failure) => {
            return RunResult::failed(
                failure.with_session(Some(session_id)),
                started_at,
                false,
                false,
            );
        }
    };
    let mut command = Command::new(executable);
    apply_to_command(&mut command);
    config.apply_to(&mut command, &receipt);
    apply_subagent_caller_context(&mut command, params);
    apply_mcp_runtime_root(&mut command);

    let turn_id = format!(
        "agy-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );
    let control_session_id = if config.requested_session_id.is_empty() {
        params
            .get("dispatchId")
            .and_then(Value::as_str)
            .and_then(|dispatch_id| dispatch_id.rsplit(':').next())
            .filter(|candidate| valid_session_id(candidate))
            .unwrap_or_default()
    } else {
        config.requested_session_id.as_str()
    };
    // A new Antigravity conversation receives its native identity only from
    // the terminal Stop-hook receipt. Keep provider cancellation keyed by the
    // private control identity, but never publish that provisional identity as
    // a canonical stream session.
    let event_session_id = config.requested_session_id.as_str();
    if let Some(observer) = RawExecutionObserver::current() {
        for (index, arg) in command.get_args().enumerate() {
            observer.record_bytes(
                &format!("antigravity.argv.{index}"),
                RawExecutionDirection::Sent,
                arg.as_encoded_bytes(),
            );
        }
    }
    let outcome = match run_turn_process(
        command,
        timeout_ms,
        max_stdout,
        max_stderr,
        control_session_id,
        event_session_id,
        &turn_id,
    ) {
        Ok(outcome) => outcome,
        Err(failure) => {
            let _ = std::fs::remove_file(&receipt);
            let _ = take_cancelled(control_session_id);
            return RunResult::failed(
                failure.with_session(Some(session_id)),
                started_at,
                false,
                false,
            );
        }
    };
    let stdout_truncated = outcome.stdout_truncated;
    let stderr_truncated = outcome.stderr_truncated;
    let status_code = outcome.status.as_ref().and_then(ExitStatus::code);
    let receipt_session = read_conversation_id(&receipt).unwrap_or_default();
    let _ = std::fs::remove_file(&receipt);
    if take_cancelled(control_session_id) {
        let bound = [
            config.requested_session_id.as_str(),
            receipt_session.as_str(),
            control_session_id,
        ]
        .into_iter()
        .find(|candidate| valid_session_id(candidate))
        .unwrap_or_default();
        return RunResult::failed(
            ProtocolFailure::new(
                "antigravity_cli_cancelled",
                "Antigravity CLI turn was cancelled.",
                "turn/cancelled",
            )
            .with_session((!bound.is_empty()).then_some(bound))
            .with_turn_status("cancelled"),
            started_at,
            stdout_truncated,
            stderr_truncated,
        );
    }
    let terminal = match classify_terminal(TerminalFacts {
        requested_session: &config.requested_session_id,
        receipt_session: (!receipt_session.is_empty()).then_some(receipt_session.as_str()),
        output: &outcome.output,
        timed_out: outcome.timed_out,
        exit_success: outcome.status.is_some_and(|status| status.success()),
    }) {
        Ok(terminal) => terminal,
        Err(failure) => {
            return RunResult::failed(
                ProtocolFailure::new(failure.code, failure.message, failure.stage)
                    .with_session(Some(&failure.session_id)),
                started_at,
                stdout_truncated,
                stderr_truncated,
            );
        }
    };
    let native_session = terminal.session_id;
    let output = terminal.output;
    let transitions = terminal.transitions;
    // Resumed conversations emit progressive chunks while the pty stream
    // arrives because their native identity is already known. A new
    // conversation is buffered until the Stop-hook binds the native identity;
    // publish one valid final chunk before completion instead of leaking an
    // empty or provisional session id.
    #[cfg(unix)]
    {
        if event_session_id.is_empty() {
            emit_turn_event(
                "agent.turn.accepted",
                &native_session,
                &turn_id,
                json!({ "evidenceKind": "native-event" }),
            );
            emit_agent_processing(&native_session, &turn_id, "activity", None);
            emit_agent_message_chunk(&native_session, &turn_id, &output);
        }
        emit_agent_message_completed(&native_session, &turn_id, &output);
    }
    RunResult {
        ok: true,
        transitions,
        output,
        error: None,
        session_id: native_session.clone(),
        thread_id: native_session,
        turn_id,
        turn_status: "completed".to_string(),
        effective: config.effective_settings(),
        status_code,
        stdout_truncated,
        stderr_truncated,
        started_at,
    }
}

struct ProcessOutcome {
    output: String,
    status: Option<ExitStatus>,
    stdout_truncated: bool,
    stderr_truncated: bool,
    timed_out: bool,
}

/// Unix turn lane: stdin+stdout on a raw pty slave so the CLI sees a real
/// terminal, the master is read incrementally, ANSI escapes are stripped, and
/// every progressive text segment is emitted as an `agent.message.chunk`.
/// stderr stays a real pipe with count-only semantics.
#[cfg(unix)]
fn run_turn_process(
    command: Command,
    timeout_ms: u64,
    max_stdout: Option<usize>,
    max_stderr: usize,
    control_session_id: &str,
    event_session_id: &str,
    turn_id: &str,
) -> Result<ProcessOutcome, ProtocolFailure> {
    let (mut child, master) =
        licoup_foundation::platform::pty_transport::spawn(command).map_err(|_| {
            ProtocolFailure::new(
                "antigravity_cli_start_failed",
                "Antigravity CLI could not be started.",
                "process/start",
            )
        })?;
    let registered = !control_session_id.is_empty();
    if registered {
        register_active_turn(control_session_id, child.pid());
    }
    let Some(stderr) = child.stderr() else {
        if registered {
            clear_active_turn(control_session_id);
        }
        let _ = child.terminate_tree();
        return Err(ProtocolFailure::new(
            "antigravity_cli_start_failed",
            "Antigravity CLI stderr is unavailable.",
            "process/start",
        ));
    };
    if !event_session_id.is_empty() {
        emit_turn_event(
            "agent.turn.accepted",
            event_session_id,
            turn_id,
            json!({ "evidenceKind": "native-event" }),
        );
    }
    let stderr_observer = RawExecutionObserver::current();
    let stderr_handle = thread::spawn(move || {
        let _raw_scope = RawExecutionScope::enter(stderr_observer);
        count_stderr(stderr, max_stderr)
    });
    let (sender, receiver) = mpsc::channel();
    let reader_handle = thread::spawn(move || {
        licoup_foundation::platform::pty_transport::read_master(master, sender, max_stdout)
    });
    let deadline = if timeout_ms == 0 {
        None
    } else {
        Some(Instant::now() + Duration::from_millis(timeout_ms))
    };
    let mut parser = PtyOutputParser::new();
    let mut stdout_truncated = false;
    let mut timed_out = false;
    loop {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            timed_out = true;
            break;
        }
        match receiver.recv_timeout(PROCESS_POLL_INTERVAL) {
            Ok(PtyEvent::Data(bytes)) => {
                if !stdout_truncated {
                    if let Some(text) = parser.push(&bytes) {
                        emit_resume_chunk(event_session_id, control_session_id, turn_id, &text);
                    }
                }
            }
            Ok(PtyEvent::Truncated) => {
                stdout_truncated = true;
            }
            Ok(PtyEvent::Closed) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
    let status = child.terminate_tree().ok().flatten();
    if registered {
        clear_active_turn(control_session_id);
    }
    let _ = join_bounded(reader_handle, IO_THREAD_EXIT_GRACE);
    let stderr_truncated = join_bounded(stderr_handle, IO_THREAD_EXIT_GRACE)
        .ok()
        .is_some_and(|truncated| truncated);
    if !stdout_truncated {
        let (output, tail) = parser.finish();
        if let Some(tail) = tail {
            emit_resume_chunk(event_session_id, control_session_id, turn_id, &tail);
        }
        return Ok(ProcessOutcome {
            output,
            status,
            stdout_truncated,
            stderr_truncated,
            timed_out,
        });
    }
    let (output, _) = parser.finish();
    Ok(ProcessOutcome {
        output,
        status,
        stdout_truncated,
        stderr_truncated,
        timed_out,
    })
}

/// Resume turns stream live because the native identity is already known.
/// After cancel is accepted, further stdout (Stop-hook drain, tail flush) must
/// not become assistant text. Pre-cancel chunks already emitted stay.
#[cfg(unix)]
fn emit_resume_chunk(event_session_id: &str, control_session_id: &str, turn_id: &str, text: &str) {
    if event_session_id.is_empty() || cancel_claimed(control_session_id) {
        return;
    }
    emit_agent_message_chunk(event_session_id, turn_id, text);
}

/// Counts stderr bytes up to `max_stderr` and reports whether the cap was hit.
/// The pty lane keeps stderr as a real pipe with count-only semantics; text is
/// captured only for the invocation-owned raw viewer.
#[cfg(unix)]
fn count_stderr(mut reader: impl Read, max_stderr: usize) -> bool {
    let mut buffer = [0u8; 8192];
    let mut counted = 0usize;
    let raw_observer = RawExecutionObserver::current();
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(_) => break,
        };
        if let Some(observer) = raw_observer.as_ref() {
            observer.record_bytes(
                "antigravity.stderr",
                RawExecutionDirection::Stderr,
                &buffer[..read],
            );
        }
        counted = counted.saturating_add(read);
        if counted >= max_stderr {
            return true;
        }
    }
    false
}

/// Non-unix turn lane: the historical pipe implementation. Reads both pipes to
/// completion with the same deadline semantics as the pty lane (0 = no
/// deadline); output is only available after the process exits.
#[cfg(not(unix))]
fn run_turn_process(
    command: Command,
    timeout_ms: u64,
    max_stdout: Option<usize>,
    max_stderr: usize,
    control_session_id: &str,
    _event_session_id: &str,
    _turn_id: &str,
) -> Result<ProcessOutcome, ProtocolFailure> {
    let mut command = command;
    command.stdin(Stdio::null()).stdout(Stdio::piped());
    let mut child = SupervisedChild::spawn(&mut command).map_err(|_| {
        ProtocolFailure::new(
            "antigravity_cli_start_failed",
            "Antigravity CLI could not be started.",
            "process/start",
        )
    })?;
    let Some(stdout) = child.stdout() else {
        let _ = child.terminate_tree();
        return Err(ProtocolFailure::new(
            "antigravity_cli_start_failed",
            "Antigravity CLI stdout is unavailable.",
            "process/start",
        ));
    };
    let Some(stderr) = child.stderr() else {
        let _ = child.terminate_tree();
        return Err(ProtocolFailure::new(
            "antigravity_cli_start_failed",
            "Antigravity CLI stderr is unavailable.",
            "process/start",
        ));
    };
    let registered = !control_session_id.is_empty();
    if registered {
        register_active_turn(control_session_id, child.pid());
    }
    let stdout_observer = RawExecutionObserver::current();
    let stdout_handle = thread::spawn(move || {
        let _raw_scope = RawExecutionScope::enter(stdout_observer);
        read_bounded(stdout, max_stdout, RawExecutionDirection::Received)
    });
    let stderr_observer = RawExecutionObserver::current();
    let stderr_handle = thread::spawn(move || {
        let _raw_scope = RawExecutionScope::enter(stderr_observer);
        read_bounded(stderr, Some(max_stderr), RawExecutionDirection::Stderr)
    });
    let deadline = if timeout_ms == 0 {
        None
    } else {
        Some(Instant::now() + Duration::from_millis(timeout_ms))
    };
    while (!stdout_handle.is_finished() || !stderr_handle.is_finished())
        && deadline.is_none_or(|deadline| Instant::now() < deadline)
    {
        thread::sleep(PROCESS_POLL_INTERVAL);
    }
    let timed_out =
        deadline.is_some() && (!stdout_handle.is_finished() || !stderr_handle.is_finished());
    let status = child.terminate_tree().ok().flatten();
    if registered {
        clear_active_turn(control_session_id);
    }
    let stdout = join_bounded(stdout_handle, IO_THREAD_EXIT_GRACE).ok();
    let stderr = join_bounded(stderr_handle, IO_THREAD_EXIT_GRACE).ok();
    Ok(ProcessOutcome {
        output: stdout
            .as_ref()
            .map(|value| value.text.trim().to_string())
            .unwrap_or_default(),
        status,
        stdout_truncated: stdout.as_ref().is_some_and(|value| value.truncated),
        stderr_truncated: stderr.as_ref().is_some_and(|value| value.truncated),
        timed_out,
    })
}

fn resolve_workspace(params: &Value, cwd: Option<&Path>) -> Option<PathBuf> {
    let requested = params
        .get("cwd")
        .or_else(|| params.get("workingDirectory"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| cwd.map(Path::to_path_buf));
    resolve_local_agent_workspace("antigravity", requested.as_deref())
}

/// One validated launch configuration. argv and effective settings are both
/// projected from this struct, so an accepted setting can never diverge from
/// the native command that actually executes.
#[derive(Debug)]
struct LaunchConfig {
    prompt: String,
    requested_session_id: String,
    workspace: PathBuf,
    model: Option<String>,
    reasoning_effort: Option<String>,
    sandbox: bool,
    permission_mode: String,
    /// The normalized permission/approval policy label the caller selected.
    /// None when the caller omitted the policy; the executed vendor mode never
    /// differs, so this field is only the caller's explicit label.
    approval_policy: Option<String>,
}

impl LaunchConfig {
    fn from_params(
        params: &Value,
        prompt: &str,
        session_id: &str,
        cwd: Option<&Path>,
    ) -> Result<Self, ProtocolFailure> {
        if params
            .get("privateInstructions")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
        {
            return Err(launch_failure(
                "antigravity_private_instructions_unsupported",
                "Antigravity CLI does not expose a separate private-instruction channel.",
                "params/privateInstructions",
                session_id,
            ));
        }
        if prompt.trim().is_empty() {
            return Err(launch_failure(
                "antigravity_cli_empty_prompt",
                "Antigravity CLI requires a non-empty message.",
                "request/validate",
                session_id,
            ));
        }
        let Some(workspace) = resolve_workspace(params, cwd) else {
            return Err(launch_failure(
                "antigravity_cli_workspace_unavailable",
                "Antigravity CLI requires a bounded local workspace directory.",
                "params/cwd",
                session_id,
            ));
        };
        let requested_session_id = session_id.trim().to_string();
        if !requested_session_id.is_empty() && !valid_session_id(&requested_session_id) {
            return Err(launch_failure(
                "antigravity_cli_session_invalid",
                "Antigravity CLI resume requires a safe native conversation identifier.",
                "session/resume",
                &requested_session_id,
            ));
        }
        let permission_policy = normalize_permission_policy(params)
            .map_err(|failure| failure.with_session(Some(session_id)))?;
        let permission_mode = permission_policy
            .clone()
            .unwrap_or_else(|| DANGEROUS_SKIP_MODE.to_string());
        Ok(Self {
            prompt: prompt.to_string(),
            requested_session_id,
            workspace,
            model: text_param(params, &["model"]),
            reasoning_effort: text_param(params, &["reasoningEffort", "effort"]),
            sandbox: sandbox_flag(params)
                .map_err(|failure| failure.with_session(Some(session_id)))?,
            permission_mode,
            approval_policy: permission_policy,
        })
    }

    fn apply_to(&self, command: &mut Command, receipt_path: &Path) {
        command
            .arg(format!("--print={}", self.prompt))
            .arg(format!("--{}", self.permission_mode))
            .current_dir(&self.workspace)
            .env(RECEIPT_ENV, receipt_path)
            .stderr(Stdio::piped());
        if !self.requested_session_id.is_empty() {
            command.arg(format!("--conversation={}", self.requested_session_id));
        }
        if let Some(model) = self.model.as_deref() {
            command.arg(format!("--model={model}"));
        }
        if let Some(effort) = self.reasoning_effort.as_deref() {
            command.arg(format!("--effort={effort}"));
        }
        if self.sandbox {
            command.arg("--sandbox");
        }
        command.arg(format!("--add-dir={}", self.workspace.display()));
    }

    fn effective_settings(&self) -> EffectiveSettings {
        EffectiveSettings {
            cwd: Some(self.workspace.display().to_string()),
            model: self.model.clone(),
            reasoning_effort: self.reasoning_effort.clone(),
            permission_mode: Some(self.permission_mode.clone()),
            sandbox: Some(Value::Bool(self.sandbox)),
            approval_policy: self.approval_policy.clone().map(Value::String),
        }
    }
}

/// Returns the caller's explicit permission/approval policy after normalizing
/// it to the executed vendor mode, or None when no policy was supplied. An
/// explicit value is accepted only when it names the actual
/// `--dangerously-skip-permissions` execution; any safer or different policy is
/// rejected rather than falsely reported as applied.
fn normalize_permission_policy(params: &Value) -> Result<Option<String>, ProtocolFailure> {
    for key in [
        "permissionMode",
        "permission_mode",
        "approvalPolicy",
        "approval_policy",
    ] {
        let Some(value) = params.get(key).filter(|value| !value.is_null()) else {
            continue;
        };
        let Some(raw) = value.as_str().map(str::trim).filter(|raw| !raw.is_empty()) else {
            return Err(launch_failure(
                "antigravity_permission_policy_unsupported",
                "Antigravity CLI supports only the dangerously-skip-permissions policy; the requested policy cannot be executed and is not reported as applied.",
                "request/validate",
                "",
            ));
        };
        return if raw == DANGEROUS_SKIP_MODE {
            Ok(Some(DANGEROUS_SKIP_MODE.to_string()))
        } else {
            Err(launch_failure(
                "antigravity_permission_policy_unsupported",
                "Antigravity CLI supports only the dangerously-skip-permissions policy; the requested policy cannot be executed and is not reported as applied.",
                "request/validate",
                "",
            ))
        };
    }
    Ok(None)
}

/// The vendor sandbox selection is boolean: present means `--sandbox`. Any
/// other explicit choice is rejected before launch rather than silently
/// dropped while a different execution is reported.
fn sandbox_flag(params: &Value) -> Result<bool, ProtocolFailure> {
    match params.get("sandbox") {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => Err(launch_failure(
            "antigravity_sandbox_unsupported",
            "Antigravity sandbox selection must be a boolean; the requested value cannot be executed.",
            "request/validate",
            "",
        )),
    }
}

fn launch_failure(
    code: &'static str,
    message: &'static str,
    stage: &'static str,
    session_id: &str,
) -> ProtocolFailure {
    ProtocolFailure::new(code, message, stage).with_session(Some(session_id))
}

fn text_param(params: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(value) = params.get(*key).and_then(Value::as_str) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

#[cfg(not(unix))]
struct BoundedRead {
    text: String,
    truncated: bool,
}

#[cfg(not(unix))]
fn read_bounded(
    mut reader: impl Read,
    max_output: Option<usize>,
    direction: RawExecutionDirection,
) -> BoundedRead {
    // None means unbounded: read everything the agent produces.
    let max_output = max_output.unwrap_or(usize::MAX);
    let mut buffer = vec![0u8; 8192.min(max_output.max(1))];
    let mut collected = Vec::new();
    let raw_observer = RawExecutionObserver::current();
    let mut truncated = false;
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(_) => break,
        };
        if let Some(observer) = raw_observer.as_ref() {
            observer.record_bytes("antigravity.stdio", direction, &buffer[..read]);
        }
        if collected.len() >= max_output {
            truncated = true;
            break;
        }
        let remaining = max_output - collected.len();
        let take = read.min(remaining);
        collected.extend_from_slice(&buffer[..take]);
        if take < read {
            truncated = true;
            break;
        }
    }
    BoundedRead {
        text: String::from_utf8_lossy(&collected).into_owned(),
        truncated,
    }
}

fn timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
}
