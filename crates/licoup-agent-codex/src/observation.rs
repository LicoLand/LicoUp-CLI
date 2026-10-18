//! Read-only evidence for Codex conversations owned by another app-server.
//!
//! It is this package's observation, not the client's: that Codex retains a
//! rollout file for an active app-server is a Codex protocol fact, so a client
//! that carries no Codex package carries no Codex process scan either. The
//! client asks this package for the answer and combines it with its own
//! conversation state.
//!
//! Codex `thread/list` reports another app-server's threads as `notLoaded`, so
//! it cannot answer whether the Codex desktop client is still running a turn.
//! On Unix, an active or loaded app-server retains its rollout file. This
//! module captures those open rollout paths once per conversation refresh;
//! the history owner combines that process evidence with Codex's persisted
//! task lifecycle before exposing a `running` fact.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Command;
#[cfg(unix)]
use std::time::Duration;

#[cfg(unix)]
use licoup_foundation::platform::process_supervisor::run_bounded_command_output;

#[cfg(unix)]
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(3);
#[cfg(unix)]
const MAX_CAPTURE_BYTES: usize = 4 * 1024 * 1024;

pub fn open_rollout_paths() -> BTreeSet<PathBuf> {
    capture_open_rollout_paths()
        .into_iter()
        .map(|path| fs::canonicalize(&path).unwrap_or(path))
        .collect()
}

#[cfg(unix)]
fn capture_open_rollout_paths() -> BTreeSet<PathBuf> {
    let mut process_command = Command::new("ps");
    process_command.args(["-axo", "pid=,comm="]);
    let Ok(process_result) =
        run_bounded_command_output(&mut process_command, CAPTURE_TIMEOUT, MAX_CAPTURE_BYTES)
    else {
        return BTreeSet::new();
    };
    if process_result.timed_out
        || process_result.truncated
        || !process_result.status.is_some_and(|status| status.success())
    {
        return BTreeSet::new();
    }
    let process_ids = parse_codex_process_ids(&String::from_utf8_lossy(&process_result.stdout));
    if process_ids.is_empty() {
        return BTreeSet::new();
    }

    let mut command = Command::new("lsof");
    // Resolve exact process identities first. `lsof -c codex` performs its own
    // system-wide process scan and added more than a second to every catalog
    // refresh on a busy desktop. The PID intersection observes the same open
    // rollouts without exporting command lines or unrelated process files.
    command.args(["-n", "-F", "n", "-a", "-p", &process_ids.join(",")]);
    let Ok(result) = run_bounded_command_output(&mut command, CAPTURE_TIMEOUT, MAX_CAPTURE_BYTES)
    else {
        return BTreeSet::new();
    };
    if result.timed_out || result.truncated || !result.status.is_some_and(|status| status.success())
    {
        return BTreeSet::new();
    }
    parse_open_rollout_paths(&String::from_utf8_lossy(&result.stdout))
}

#[cfg(unix)]
fn parse_codex_process_ids(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.trim().splitn(2, char::is_whitespace);
            let process_id = fields.next()?.trim();
            let executable = fields.next()?.trim();
            if process_id.is_empty() || !process_id.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            let executable = Path::new(executable)
                .file_name()
                .and_then(|name| name.to_str())?
                .to_ascii_lowercase();
            matches!(executable.as_str(), "codex" | "codex-code-mode-host")
                .then(|| process_id.to_string())
        })
        .collect()
}

#[cfg(windows)]
fn capture_open_rollout_paths() -> BTreeSet<PathBuf> {
    // Windows has no built-in equivalent that can identify another process's
    // exact open rollout without adding a privileged helper. Fail closed; a
    // LicoUp-owned turn is still projected by the client controller.
    BTreeSet::new()
}

fn parse_open_rollout_paths(output: &str) -> BTreeSet<PathBuf> {
    output
        .lines()
        .filter_map(|line| line.strip_prefix('n'))
        .map(Path::new)
        .filter(|path| path.is_absolute())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("rollout-"))
        })
        .map(Path::to_path_buf)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsof_field_output_keeps_only_absolute_codex_rollouts() {
        let parsed = parse_open_rollout_paths(
            "p100\nccodex\nn/fixture/sessions/rollout-2026-08-13-thread.jsonl\n\
             n/fixture/config.toml\nnrelative/rollout-thread.jsonl\n\
             n/fixture/sessions/notes.jsonl\n",
        );

        assert_eq!(parsed.len(), 1);
        assert!(parsed.contains(Path::new(
            "/fixture/sessions/rollout-2026-08-13-thread.jsonl"
        )));
    }

    #[cfg(unix)]
    #[test]
    fn process_scan_keeps_only_exact_codex_runtime_executables() {
        let parsed = parse_codex_process_ids(
            "  100 /fixture/bin/codex\n\
               101 /fixture/bin/codex-code-mode-host\n\
               102 /Applications/Codex (Renderer)\n\
               103 /fixture/bin/codex-helper\n\
               invalid /fixture/bin/codex\n",
        );

        assert_eq!(parsed, ["100", "101"]);
    }
}
