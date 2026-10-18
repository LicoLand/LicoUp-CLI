use licoup_agent_drivers::acp_session_transport::{CapabilityProbe, drain_bounded, read_bounded};
use licoup_foundation::platform::process_supervisor::{
    IO_THREAD_EXIT_GRACE, SupervisedChild, join_bounded,
};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(50);

const ACP_CHECK_ARGS: [&str; 2] = ["acp", "--check"];
const ACP_VERSION_ARGS: [&str; 2] = ["acp", "--version"];

pub fn probe(executable: &str, timeout_ms: u64, max_output: usize) -> CapabilityProbe {
    let check = run_probe_command(executable, &ACP_CHECK_ARGS, timeout_ms, max_output);
    let Ok(check) = check else {
        return CapabilityProbe {
            error_code: Some("hermes_acp_probe_failed"),
            ..CapabilityProbe::default()
        };
    };
    let supported = String::from_utf8_lossy(&check).contains("ACP check OK");
    let version = run_probe_command(executable, &ACP_VERSION_ARGS, timeout_ms, max_output)
        .ok()
        .and_then(|bytes| first_nonempty_line(&bytes));
    CapabilityProbe {
        available: true,
        supported,
        version,
        error_code: (!supported).then_some("hermes_acp_capability_missing"),
        supports_streaming: true,
        supports_tools: true,
        supports_approvals: true,
        supports_model_override: true,
        supports_reasoning_override: false,
    }
}

fn run_probe_command(
    executable: &str,
    args: &[&str],
    timeout_ms: u64,
    max_output: usize,
) -> Result<Vec<u8>, ()> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    licoup_agent_targets::platform::user_shell_environment::apply_to_command(&mut command);
    licoup_foundation::platform::process_supervisor::configure_untrusted_agent_command(
        &mut command,
    );
    let mut child = SupervisedChild::spawn(&mut command).map_err(|_| ())?;
    let Some(stdout) = child.stdout() else {
        child.terminate_tree().map_err(|_| ())?;
        return Err(());
    };
    let Some(stderr) = child.stderr() else {
        child.terminate_tree().map_err(|_| ())?;
        return Err(());
    };
    let stdout_handle = thread::spawn(move || read_bounded(stdout, max_output));
    // stderr is bounded and drained for liveness, but its bytes never enter a result.
    let stderr_handle = thread::spawn(move || drain_bounded(stderr, max_output));
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while !stdout_handle.is_finished() && Instant::now() < deadline {
        thread::sleep(PROCESS_POLL_INTERVAL);
    }
    let timed_out = !stdout_handle.is_finished();
    let status = child.terminate_tree().map_err(|_| ())?;
    let stdout = join_bounded(stdout_handle, IO_THREAD_EXIT_GRACE).map_err(|_| ())?;
    let stderr_was_truncated = join_bounded(stderr_handle, IO_THREAD_EXIT_GRACE).map_err(|_| ())?;
    if timed_out || !status.is_some_and(|value| value.success()) || stdout.1 || stderr_was_truncated
    {
        return Err(());
    }
    Ok(stdout.0)
}

fn first_nonempty_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}
