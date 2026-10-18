use super::io::read_bounded;
use super::model::PROCESS_POLL_INTERVAL;
use crate::protocol::CapabilityProbe;
use licoup_foundation::platform::process_supervisor::{
    IO_THREAD_EXIT_GRACE, SupervisedChild, join_bounded,
};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub fn probe(executable: &str, timeout_ms: u64, max_output: usize) -> CapabilityProbe {
    let version = run_probe_command(executable, "--version", timeout_ms, max_output);
    let help = run_probe_command(executable, "--help", timeout_ms, max_output);
    if version.is_none() && help.is_none() {
        CapabilityProbe::default()
    } else {
        CapabilityProbe::official(version == Some(true), help == Some(true))
    }
}

fn run_probe_command(
    executable: &str,
    argument: &str,
    timeout_ms: u64,
    max_output: usize,
) -> Option<bool> {
    let mut command = Command::new(executable);
    command
        .arg(argument)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    licoup_agent_targets::platform::user_shell_environment::apply_to_command(&mut command);
    licoup_foundation::platform::process_supervisor::configure_untrusted_agent_command(
        &mut command,
    );
    let mut child = SupervisedChild::spawn(&mut command).ok()?;
    let stdout = child.stdout()?;
    let stderr = child.stderr()?;
    let stdout_handle = thread::spawn(move || read_bounded(stdout, max_output));
    let stderr_handle = thread::spawn(move || read_bounded(stderr, max_output));
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while (!stdout_handle.is_finished() || !stderr_handle.is_finished())
        && Instant::now() < deadline
    {
        thread::sleep(PROCESS_POLL_INTERVAL);
    }
    let timed_out = !stdout_handle.is_finished() || !stderr_handle.is_finished();
    let status = child.terminate_tree().ok().flatten();
    let stdout_truncated = join_bounded(stdout_handle, IO_THREAD_EXIT_GRACE).ok()?;
    let stderr_truncated = join_bounded(stderr_handle, IO_THREAD_EXIT_GRACE).ok()?;
    Some(
        !timed_out
            && status.is_some_and(|value| value.success())
            && !stdout_truncated
            && !stderr_truncated,
    )
}
