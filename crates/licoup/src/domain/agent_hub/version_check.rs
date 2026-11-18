//! Per-agent installed-version probes. Missing agents stay blank; never "unknown".

use super::argv::{self, ArgvKind};
use super::contract::{AgentRecipe, InstallChannel};
use super::version;
use serde_json::Value;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

pub fn installed_version(
    agent: &AgentRecipe,
    channel: Option<&InstallChannel>,
    present: bool,
    live_lookup: bool,
    params: &Value,
    executable_binding: Option<&Path>,
) -> String {
    if !present {
        return String::new();
    }
    if let Some(injected) = injected_probe(params, &agent.id) {
        return parse_output(&agent.id, &injected, "");
    }
    if !live_lookup {
        return String::new();
    }
    let (Some(channel), Some(executable_binding)) = (channel, executable_binding) else {
        return String::new();
    };
    if channel.verify_argv.is_empty() {
        return String::new();
    }
    let recipe_program = &channel.verify_argv[0];
    let args = &channel.verify_argv[1..];
    if !verify_names_the_agent(recipe_program, channel, agent, executable_binding)
        || !binding_belongs_to_agent(executable_binding, agent)
        || argv::validate_program_args(recipe_program, args, ArgvKind::Lifecycle).is_err()
    {
        return String::new();
    }
    let output = match run_probe(executable_binding, args) {
        Some(output) => output,
        None => return String::new(),
    };
    parse_output(&agent.id, &output.stdout, &output.stderr)
}

pub fn parse_output(agent_id: &str, stdout: &str, stderr: &str) -> String {
    let combined = if stdout.trim().is_empty() {
        stderr
    } else {
        stdout
    };
    let parsed = match agent_id {
        "codex" => parse_codex(combined),
        "cursor" => parse_cursor(combined),
        "opencode" => parse_opencode(combined),
        "claude-code" => parse_claude_code(combined),
        "pi" => parse_pi(combined),
        "openclaw" => parse_openclaw(combined),
        "hermes" => parse_hermes(combined),
        "antigravity" => parse_antigravity(combined),
        "deepseek-harness" => parse_deepseek_harness(combined),
        _ => String::new(),
    };
    reject_unknown(parsed)
}

fn parse_codex(raw: &str) -> String {
    parse_exact_version(raw, &["codex-cli", "codex"])
}

fn parse_cursor(raw: &str) -> String {
    let candidate = strip_prefix(raw, &["cursor-agent", "cursor"]).trim();
    let semver = parse_exact_version(raw, &["cursor-agent", "cursor"]);
    if !semver.is_empty() {
        return semver;
    }
    if is_cursor_date_hash_version(candidate) {
        candidate.to_owned()
    } else {
        String::new()
    }
}

pub(crate) fn is_cursor_date_hash_version(candidate: &str) -> bool {
    let Some((date, hash)) = candidate.split_once('-') else {
        return false;
    };
    if hash.len() != 7 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return false;
    }
    let parts = date.split('.').collect::<Vec<_>>();
    if parts.len() != 3
        || parts[0].len() != 4
        || parts[1].len() != 2
        || parts[2].len() != 2
        || parts
            .iter()
            .any(|part| !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return false;
    }
    let Ok(year) = parts[0].parse::<u16>() else {
        return false;
    };
    let Ok(month) = parts[1].parse::<u8>() else {
        return false;
    };
    let Ok(day) = parts[2].parse::<u8>() else {
        return false;
    };
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(400) || (year.is_multiple_of(4) && !year.is_multiple_of(100)) => {
            29
        }
        2 => 28,
        _ => return false,
    };
    year != 0 && day != 0 && day <= days_in_month
}

fn parse_opencode(raw: &str) -> String {
    version::concrete_display(strip_prefix(raw, &["opencode", "OpenCode"]))
}

fn parse_claude_code(raw: &str) -> String {
    version::concrete_display(strip_prefix(raw, &["claude", "Claude Code", "Claude"]))
}

fn parse_pi(raw: &str) -> String {
    version::concrete_display(strip_prefix(raw, &["pi-agent", "pi"]))
}

fn parse_openclaw(raw: &str) -> String {
    version::concrete_display(strip_prefix(raw, &["openclaw"]))
}

fn parse_hermes(raw: &str) -> String {
    version::concrete_display(strip_prefix(raw, &["hermes-agent", "hermes"]))
}

fn parse_antigravity(raw: &str) -> String {
    parse_exact_version(raw, &["agy", "antigravity"])
}

fn parse_deepseek_harness(raw: &str) -> String {
    version::concrete_display(strip_prefix(raw, &["deepseek-harness", "dsh"]))
}

fn strip_prefix<'a>(raw: &'a str, prefixes: &[&str]) -> &'a str {
    let line = raw
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    for prefix in prefixes {
        if line == *prefix {
            return "";
        }
        if let Some(rest) = line
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_prefix(char::is_whitespace))
        {
            return rest.trim();
        }
    }
    line
}

fn parse_exact_version(raw: &str, prefixes: &[&str]) -> String {
    let candidate = strip_prefix(raw, prefixes).trim();
    let parsed = version::concrete_display(candidate);
    if parsed.is_empty() || candidate.trim_start_matches(['v', 'V']) != parsed {
        String::new()
    } else {
        parsed
    }
}

fn reject_unknown(value: String) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("unknown") || trimmed == "未知" {
        return String::new();
    }
    trimmed.to_string()
}

pub(crate) fn injected_probe(params: &Value, agent_id: &str) -> Option<String> {
    params
        .get("versionProbes")
        .and_then(Value::as_object)
        .and_then(|map| map.get(agent_id))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

struct ProbeOutput {
    stdout: String,
    stderr: String,
}

/// The verify program a channel names for the executable it installs.
pub(crate) const INSTALL_VERIFY_PROGRAM: &str = "{install}";

/// Whether one verify step's program names this Agent's own executable.
///
/// A recipe that names the executable directly matches that name, exactly as
/// before. A recipe that verifies `{install}` names the file at the channel's
/// declared install destination: the name to match is then the declared binary,
/// and a channel that declares no destination stays blank instead of probing a
/// path it never stated.
fn verify_names_the_agent(
    program: &str,
    channel: &InstallChannel,
    agent: &AgentRecipe,
    binding: &Path,
) -> bool {
    if program != INSTALL_VERIFY_PROGRAM {
        return agent
            .binary_names
            .iter()
            .any(|name| name.eq_ignore_ascii_case(program));
    }
    let Some(install) = channel.install.as_ref() else {
        return false;
    };
    let Some(file_name) = binding.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    install
        .binary
        .values()
        .any(|binary| binary.eq_ignore_ascii_case(file_name))
}

fn binding_belongs_to_agent(binding: &Path, agent: &AgentRecipe) -> bool {
    let Some(file_name) = binding.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let normalized = file_name.strip_suffix(".exe").unwrap_or(file_name);
    agent
        .binary_names
        .iter()
        .any(|name| name.eq_ignore_ascii_case(normalized))
}

fn run_probe(program: &Path, args: &[String]) -> Option<ProbeOutput> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let output = child.wait_with_output().ok()?;
                return Some(ProbeOutput {
                    stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
                });
            }
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(40));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}
