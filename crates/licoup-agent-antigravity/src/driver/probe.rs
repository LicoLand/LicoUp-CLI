use super::model::{CapabilityProbe, PROCESS_POLL_INTERVAL};
use licoup_agent_targets::platform::user_shell_environment::apply_to_command;
use licoup_foundation::platform::process_supervisor::{
    IO_THREAD_EXIT_GRACE, SupervisedChild, configure_untrusted_agent_command, join_bounded,
};
use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub fn probe(executable: &str, timeout_ms: u64, max_output: usize) -> CapabilityProbe {
    let version = run_probe_command(executable, "--version", timeout_ms, max_output);
    let help = run_probe_command_with_text(executable, "--help", timeout_ms, max_output);
    match (version, help) {
        (Some(version_ok), Some((help_ok, help_text))) => {
            CapabilityProbe::official(version_ok, help_ok, &help_text)
        }
        _ => CapabilityProbe::unavailable(),
    }
}

fn run_probe_command(
    executable: &str,
    argument: &str,
    timeout_ms: u64,
    max_output: usize,
) -> Option<bool> {
    run_probe_command_with_text(executable, argument, timeout_ms, max_output).map(|(ok, _)| ok)
}

fn run_probe_command_with_text(
    executable: &str,
    argument: &str,
    timeout_ms: u64,
    max_output: usize,
) -> Option<(bool, String)> {
    let mut command = Command::new(executable);
    command
        .arg(argument)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_to_command(&mut command);
    configure_untrusted_agent_command(&mut command);
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
    let stdout = join_bounded(stdout_handle, IO_THREAD_EXIT_GRACE).ok()?;
    let stderr = join_bounded(stderr_handle, IO_THREAD_EXIT_GRACE).ok()?;
    Some((
        !timed_out
            && status.is_some_and(|value| value.success())
            && !stdout.truncated
            && !stderr.truncated,
        stdout.text,
    ))
}

struct BoundedRead {
    text: String,
    truncated: bool,
}

fn read_bounded(mut reader: impl Read, max_output: usize) -> BoundedRead {
    let mut buffer = vec![0u8; 8192.min(max_output.max(1))];
    let mut collected = Vec::new();
    let mut truncated = false;
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(_) => break,
        };
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
