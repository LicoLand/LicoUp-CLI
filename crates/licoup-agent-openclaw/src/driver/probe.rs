use super::io::read_bounded;
use crate::gateway_acp::model::{CapabilityProbe, PROCESS_POLL_INTERVAL};
use licoup_agent_targets::platform::user_shell_environment::apply_to_command;
use licoup_foundation::platform::process_supervisor::{
    IO_THREAD_EXIT_GRACE, SupervisedChild, configure_untrusted_agent_command, join_bounded,
};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub fn probe(executable: &str, timeout_ms: u64, max_output: usize) -> CapabilityProbe {
    let help = run_probe_command(executable, &["acp", "--help"], timeout_ms, max_output);
    let Ok(help) = help else {
        return CapabilityProbe {
            error_code: Some("openclaw_acp_probe_failed"),
            ..CapabilityProbe::default()
        };
    };
    let text = String::from_utf8_lossy(&help);
    let supported = text.contains("ACP") || text.contains("Gateway");
    let version = run_probe_command(executable, &["--version"], timeout_ms, max_output)
        .ok()
        .and_then(|bytes| first_nonempty_line(&bytes));
    CapabilityProbe {
        available: true,
        supported,
        version,
        error_code: (!supported).then_some("openclaw_acp_capability_missing"),
        supports_streaming: true,
        supports_tools: true,
        supports_approvals: true,
        supports_reasoning: true,
        supports_model_override: false,
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
        .stderr(Stdio::null());
    apply_to_command(&mut command);
    configure_untrusted_agent_command(&mut command);
    let mut child = SupervisedChild::spawn(&mut command).map_err(|_| ())?;
    let Some(stdout) = child.stdout() else {
        child.terminate_tree().map_err(|_| ())?;
        return Err(());
    };
    let stdout_handle = thread::spawn(move || read_bounded(stdout, max_output));
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while !stdout_handle.is_finished() && Instant::now() < deadline {
        thread::sleep(PROCESS_POLL_INTERVAL);
    }
    let timed_out = !stdout_handle.is_finished();
    let status = child.terminate_tree().map_err(|_| ())?;
    let stdout = join_bounded(stdout_handle, IO_THREAD_EXIT_GRACE).map_err(|_| ())?;
    if timed_out || !status.is_some_and(|value| value.success()) || stdout.1 {
        return Err(());
    }
    Ok(stdout.0)
}

pub(super) fn first_nonempty_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}
