use super::HERMES_SESSION_DRIVER;
use licoup_agent_drivers::acp_session_transport::{self, ControlDisposition, RunResult};
use licoup_agent_targets::platform::virtual_machine::SshRuntimeConnection;
use serde_json::Value;
use std::path::Path;

#[cfg(test)]
pub(crate) fn execute(
    executable: &str,
    params: &Value,
    prompt: &str,
    session_id: &str,
    cwd: Option<&Path>,
    timeout_ms: u64,
    max_stdout: Option<usize>,
    max_stderr: usize,
) -> RunResult {
    execute_with_connection(
        executable, None, params, prompt, session_id, cwd, timeout_ms, max_stdout, max_stderr,
    )
}

pub fn execute_with_connection(
    executable: &str,
    runtime_connection: Option<&SshRuntimeConnection>,
    params: &Value,
    prompt: &str,
    session_id: &str,
    cwd: Option<&Path>,
    timeout_ms: u64,
    max_stdout: Option<usize>,
    max_stderr: usize,
) -> RunResult {
    acp_session_transport::execute(
        HERMES_SESSION_DRIVER,
        executable,
        runtime_connection,
        params,
        prompt,
        session_id,
        cwd,
        timeout_ms,
        max_stdout,
        max_stderr,
    )
}

pub fn cancel(session_id: &str) -> ControlDisposition {
    acp_session_transport::cancel(HERMES_SESSION_DRIVER, session_id)
}

pub fn cleanup_session(session_id: &str) -> ControlDisposition {
    acp_session_transport::cleanup_session(HERMES_SESSION_DRIVER, session_id)
}
