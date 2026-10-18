//! Live on-host evidence for the adapter management catalog's native
//! capability items. A capability entry is a stable placeholder; this module
//! answers whether the capability is genuinely effective right now by matching
//! running processes and listening ports. Only bounded `ps` / `lsof` (or
//! `tasklist` / `netstat`) snapshots are taken, and only the matched pid,
//! process name, and port ever cross into the catalog payload — full command
//! lines stay inside the matcher.

use super::adapter::{NativeCapabilityKind, RuntimeAdapter};
use licoup_foundation::platform::process_supervisor::run_bounded_command_output;
use std::collections::BTreeMap;
use std::process::Command;
use std::time::Duration;

const CAPTURE_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_CAPTURE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProcessEntry {
    pub pid: u32,
    pub name: String,
    pub args: String,
}

#[derive(Clone, Debug, Default)]
pub struct LiveSnapshot {
    processes: Vec<ProcessEntry>,
    listen_ports: BTreeMap<u32, Vec<u16>>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LiveStatus {
    pub running: bool,
    pub pid: Option<u32>,
    pub process_name: Option<String>,
    pub port: Option<u16>,
}

struct MatchSpec {
    /// Normalized lowercase executable base names (no directory, no `.exe`).
    names: &'static [&'static str],
    /// Argument token that must be present (for example `serve`).
    token: Option<&'static str>,
    /// Argument tokens that must all be absent.
    exclude_tokens: &'static [&'static str],
    /// Substring that must appear in the full command line (app bundles).
    substring: Option<&'static str>,
    /// Substring that must not appear in the full command line.
    exclude_substring: Option<&'static str>,
    wants_port: bool,
}

impl MatchSpec {
    fn matches(&self, entry: &ProcessEntry) -> bool {
        if !self.names.iter().any(|name| *name == entry.name) {
            return false;
        }
        // Windows tasklist supplies no command line; argument rules pass
        // vacuously there so name evidence still surfaces. On Unix a missing
        // command line means nothing to match against.
        if entry.args.is_empty() {
            return cfg!(windows);
        }
        if let Some(token) = self.token
            && !has_token(&entry.args, token)
        {
            return false;
        }
        if self
            .exclude_tokens
            .iter()
            .any(|token| has_token(&entry.args, token))
        {
            return false;
        }
        if let Some(substring) = self.substring
            && !entry.args.contains(substring)
        {
            return false;
        }
        if let Some(substring) = self.exclude_substring
            && entry.args.contains(substring)
        {
            return false;
        }
        true
    }
}

fn has_token(args: &str, token: &str) -> bool {
    args.split_whitespace().any(|part| part == token)
}

const APP_BUNDLE_MARKER: &str = ".app/Contents/MacOS/";

fn match_spec(adapter: RuntimeAdapter, kind: NativeCapabilityKind) -> Option<MatchSpec> {
    use NativeCapabilityKind::{
        Acp, AppServer, Cli, Desktop, Gateway, LocalServer, Rpc, WebServer,
    };
    const BASE: MatchSpec = MatchSpec {
        names: &[],
        token: None,
        exclude_tokens: &[],
        substring: None,
        exclude_substring: None,
        wants_port: false,
    };
    let spec = match (adapter, kind) {
        (RuntimeAdapter::Antigravity, Desktop) => MatchSpec {
            names: &["antigravity"],
            substring: Some(APP_BUNDLE_MARKER),
            ..BASE
        },
        (RuntimeAdapter::Antigravity, Cli) => MatchSpec {
            names: &["agy", "antigravity"],
            exclude_substring: Some(APP_BUNDLE_MARKER),
            ..BASE
        },
        (RuntimeAdapter::ClaudeCode, Cli) => MatchSpec {
            names: &["claude"],
            ..BASE
        },
        (RuntimeAdapter::Codex, Desktop) => MatchSpec {
            names: &["chatgpt"],
            substring: Some(APP_BUNDLE_MARKER),
            ..BASE
        },
        (RuntimeAdapter::Codex, Cli) => MatchSpec {
            names: &["codex"],
            exclude_tokens: &["app-server"],
            ..BASE
        },
        (RuntimeAdapter::Codex, AppServer) => MatchSpec {
            names: &["codex"],
            token: Some("app-server"),
            ..BASE
        },
        (RuntimeAdapter::Copilot, Cli) => MatchSpec {
            names: &["copilot"],
            exclude_tokens: &["--acp"],
            ..BASE
        },
        (RuntimeAdapter::Copilot, Acp) => MatchSpec {
            names: &["copilot"],
            token: Some("--acp"),
            ..BASE
        },
        (RuntimeAdapter::Cursor, Desktop) => MatchSpec {
            names: &["cursor"],
            substring: Some(APP_BUNDLE_MARKER),
            ..BASE
        },
        (RuntimeAdapter::Cursor, Cli) => MatchSpec {
            names: &["cursor-agent"],
            ..BASE
        },
        (RuntimeAdapter::Hermes, Cli) => MatchSpec {
            names: &["hermes"],
            exclude_tokens: &["acp"],
            ..BASE
        },
        (RuntimeAdapter::Hermes, Acp) => MatchSpec {
            names: &["hermes"],
            token: Some("acp"),
            ..BASE
        },
        (RuntimeAdapter::KiloCode, Cli) => MatchSpec {
            names: &["kilo", "kilocode"],
            exclude_tokens: &["serve"],
            ..BASE
        },
        (RuntimeAdapter::KiloCode, LocalServer) => MatchSpec {
            names: &["kilo", "kilocode"],
            token: Some("serve"),
            wants_port: true,
            ..BASE
        },
        (RuntimeAdapter::KimiCode, Cli) => MatchSpec {
            names: &["kimi"],
            exclude_tokens: &["acp", "web"],
            ..BASE
        },
        (RuntimeAdapter::KimiCode, Acp) => MatchSpec {
            names: &["kimi"],
            token: Some("acp"),
            ..BASE
        },
        (RuntimeAdapter::KimiCode, WebServer) => MatchSpec {
            names: &["kimi"],
            token: Some("web"),
            wants_port: true,
            ..BASE
        },
        (RuntimeAdapter::OpenClaw, Cli) => MatchSpec {
            names: &["openclaw"],
            exclude_tokens: &["acp", "gateway"],
            ..BASE
        },
        (RuntimeAdapter::OpenClaw, Acp) => MatchSpec {
            names: &["openclaw"],
            token: Some("acp"),
            ..BASE
        },
        (RuntimeAdapter::OpenClaw, Gateway) => MatchSpec {
            names: &["openclaw"],
            token: Some("gateway"),
            wants_port: true,
            ..BASE
        },
        (RuntimeAdapter::OpenCode, Cli) => MatchSpec {
            names: &["opencode"],
            exclude_tokens: &["serve"],
            ..BASE
        },
        (RuntimeAdapter::OpenCode, LocalServer) => MatchSpec {
            names: &["opencode"],
            token: Some("serve"),
            wants_port: true,
            ..BASE
        },
        (RuntimeAdapter::Pi, Cli) => MatchSpec {
            names: &["pi"],
            exclude_tokens: &["--mode"],
            ..BASE
        },
        (RuntimeAdapter::Pi, Rpc) => MatchSpec {
            names: &["pi"],
            token: Some("rpc"),
            ..BASE
        },
        (RuntimeAdapter::LicoAgent, Cli) => MatchSpec {
            names: &["lico-agent"],
            exclude_tokens: &["--mode"],
            ..BASE
        },
        (RuntimeAdapter::LicoAgent, Rpc) => MatchSpec {
            names: &["lico-agent"],
            token: Some("rpc"),
            ..BASE
        },
        _ => return None,
    };
    Some(spec)
}

impl LiveSnapshot {
    pub fn capture() -> Self {
        Self {
            processes: capture_processes(),
            listen_ports: capture_listen_ports(),
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn for_testing(
        processes: Vec<ProcessEntry>,
        listen_ports: BTreeMap<u32, Vec<u16>>,
    ) -> Self {
        Self {
            processes,
            listen_ports,
        }
    }

    pub fn status(&self, adapter: RuntimeAdapter, kind: NativeCapabilityKind) -> LiveStatus {
        let Some(spec) = match_spec(adapter, kind) else {
            return LiveStatus::default();
        };
        // Deterministic evidence: the lowest matching pid wins.
        let mut matches = self
            .processes
            .iter()
            .filter(|entry| spec.matches(entry))
            .collect::<Vec<_>>();
        matches.sort_by_key(|entry| entry.pid);
        let Some(entry) = matches.first() else {
            return LiveStatus::default();
        };
        let port = if spec.wants_port {
            self.listen_ports
                .get(&entry.pid)
                .and_then(|ports| ports.first())
                .copied()
        } else {
            None
        };
        LiveStatus {
            running: true,
            pid: Some(entry.pid),
            process_name: Some(entry.name.clone()),
            port,
        }
    }
}

/// Parse `ps -axo pid=,args=` output. The executable base name comes from the
/// first argument so truncated Unix `comm` fields never hide long names such
/// as `lico-llm-gateway`.
pub fn parse_ps_entries(output: &str) -> Vec<ProcessEntry> {
    output
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            if trimmed.is_empty() {
                return None;
            }
            let split_at = trimmed.find(char::is_whitespace)?;
            let pid = trimmed[..split_at].parse::<u32>().ok()?;
            let args = trimmed[split_at..].trim().to_string();
            if args.is_empty() {
                return None;
            }
            let executable = args.split_whitespace().next().unwrap_or("");
            let name = executable_name(executable)?;
            Some(ProcessEntry { pid, name, args })
        })
        .collect()
}

/// Parse `lsof -nP -iTCP -sTCP:LISTEN` output into a pid-to-ports map.
pub fn parse_lsof_listen_ports(output: &str) -> BTreeMap<u32, Vec<u16>> {
    let mut map = BTreeMap::<u32, Vec<u16>>::new();
    for line in output.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 2 || fields[0] == "COMMAND" {
            continue;
        }
        let Ok(pid) = fields[1].parse::<u32>() else {
            continue;
        };
        let Some(port) = fields.iter().rev().find_map(|field| listen_port(field)) else {
            continue;
        };
        let ports = map.entry(pid).or_default();
        if !ports.contains(&port) {
            ports.push(port);
            ports.sort_unstable();
        }
    }
    map
}

fn listen_port(name_field: &str) -> Option<u16> {
    let (host, port) = name_field.rsplit_once(':')?;
    if host.is_empty() {
        return None;
    }
    port.parse::<u16>().ok()
}

fn executable_name(executable: &str) -> Option<String> {
    let base = executable
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(executable)
        .trim()
        .to_ascii_lowercase();
    let base = base.strip_suffix(".exe").unwrap_or(&base).to_string();
    if base.is_empty() { None } else { Some(base) }
}

#[cfg(not(windows))]
fn capture_processes() -> Vec<ProcessEntry> {
    let mut command = Command::new("ps");
    command.args(["-axo", "pid=,args="]);
    run_capture(&mut command)
        .map(|output| parse_ps_entries(&output))
        .unwrap_or_default()
}

#[cfg(windows)]
fn capture_processes() -> Vec<ProcessEntry> {
    let mut command = Command::new("tasklist");
    command.args(["/fo", "csv", "/nh"]);
    run_capture(&mut command)
        .map(|output| parse_tasklist_entries(&output))
        .unwrap_or_default()
}

#[cfg(windows)]
fn parse_tasklist_entries(output: &str) -> Vec<ProcessEntry> {
    output
        .lines()
        .filter_map(|line| {
            let fields = line.split("\",\"").collect::<Vec<_>>();
            let name = fields.first()?.trim_matches('"').to_string();
            let pid = fields.get(1)?.trim_matches('"').parse::<u32>().ok()?;
            let name = executable_name(&name)?;
            Some(ProcessEntry {
                pid,
                name,
                args: String::new(),
            })
        })
        .collect()
}

#[cfg(not(windows))]
fn capture_listen_ports() -> BTreeMap<u32, Vec<u16>> {
    let mut command = Command::new("lsof");
    command.args(["-nP", "-iTCP", "-sTCP:LISTEN"]);
    run_capture(&mut command)
        .map(|output| parse_lsof_listen_ports(&output))
        .unwrap_or_default()
}

#[cfg(windows)]
fn capture_listen_ports() -> BTreeMap<u32, Vec<u16>> {
    let mut command = Command::new("netstat");
    command.args(["-ano", "-p", "tcp"]);
    run_capture(&mut command)
        .map(|output| parse_netstat_listen_ports(&output))
        .unwrap_or_default()
}

#[cfg(windows)]
fn parse_netstat_listen_ports(output: &str) -> BTreeMap<u32, Vec<u16>> {
    let mut map = BTreeMap::<u32, Vec<u16>>::new();
    for line in output.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 5 || fields[0] != "TCP" || fields[3] != "LISTENING" {
            continue;
        }
        let (Ok(pid), Some(port)) = (
            fields[4].parse::<u32>(),
            fields[1]
                .rsplit_once(':')
                .and_then(|(_, p)| p.parse::<u16>().ok()),
        ) else {
            continue;
        };
        let ports = map.entry(pid).or_default();
        if !ports.contains(&port) {
            ports.push(port);
            ports.sort_unstable();
        }
    }
    map
}

fn run_capture(command: &mut Command) -> Option<String> {
    let result = run_bounded_command_output(command, CAPTURE_TIMEOUT, MAX_CAPTURE_BYTES).ok()?;
    if result.timed_out || result.truncated || !result.status.is_some_and(|status| status.success())
    {
        return None;
    }
    Some(String::from_utf8_lossy(&result.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(pid: u32, args: &str) -> ProcessEntry {
        let executable = args.split_whitespace().next().unwrap_or("");
        ProcessEntry {
            pid,
            name: executable_name(executable).unwrap_or_default(),
            args: args.to_string(),
        }
    }

    #[test]
    fn ps_entries_use_full_command_line_basename() {
        let entries = parse_ps_entries(
            "  501 fixture/bin/codex app-server\n502 /Applications/ChatGPT.app/Contents/MacOS/ChatGPT\n",
        );
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].pid, 501);
        assert_eq!(entries[0].name, "codex");
        assert_eq!(entries[1].name, "chatgpt");
        assert!(parse_ps_entries("garbage without pid").is_empty());
    }

    #[test]
    fn lsof_listen_ports_are_indexed_by_pid() {
        let ports = parse_lsof_listen_ports(
            "COMMAND     PID USER   FD   TYPE DEVICE SIZE/OFF NODE NAME\nopencode  88426 fixture   12u  IPv6 *:4096 (LISTEN)\nopencode  88426 fixture   13u  IPv4 127.0.0.1:4096 (LISTEN)\n",
        );
        assert_eq!(ports.get(&88426), Some(&vec![4096]));
    }

    #[test]
    fn cli_and_server_kinds_are_distinguished_by_argument_tokens() {
        let snapshot = LiveSnapshot::for_testing(
            vec![
                entry(100, "opencode"),
                entry(101, "opencode serve --port 4096"),
            ],
            BTreeMap::from([(101, vec![4096])]),
        );
        let cli = snapshot.status(RuntimeAdapter::OpenCode, NativeCapabilityKind::Cli);
        assert_eq!(cli.pid, Some(100));
        assert_eq!(cli.port, None);
        let server = snapshot.status(RuntimeAdapter::OpenCode, NativeCapabilityKind::LocalServer);
        assert_eq!(server.pid, Some(101));
        assert_eq!(server.port, Some(4096));
        assert_eq!(server.process_name.as_deref(), Some("opencode"));
    }

    #[test]
    fn kimi_acp_and_web_server_are_distinct_native_capabilities() {
        let snapshot = LiveSnapshot::for_testing(
            vec![
                entry(109, "kimi"),
                entry(110, "kimi acp"),
                entry(111, "kimi web --port 58627 --no-open"),
            ],
            BTreeMap::from([(111, vec![58627])]),
        );
        let cli = snapshot.status(RuntimeAdapter::KimiCode, NativeCapabilityKind::Cli);
        assert_eq!(cli.pid, Some(109));
        assert_eq!(cli.port, None);
        let acp = snapshot.status(RuntimeAdapter::KimiCode, NativeCapabilityKind::Acp);
        assert_eq!(acp.pid, Some(110));
        assert_eq!(acp.port, None);
        let server = snapshot.status(RuntimeAdapter::KimiCode, NativeCapabilityKind::WebServer);
        assert_eq!(server.pid, Some(111));
        assert_eq!(server.port, Some(58627));
        assert_eq!(server.process_name.as_deref(), Some("kimi"));
        assert!(
            !snapshot
                .status(RuntimeAdapter::KimiCode, NativeCapabilityKind::LocalServer,)
                .running
        );
    }

    #[test]
    fn codex_app_server_is_one_stdio_capability_and_not_a_cli_turn() {
        let snapshot =
            LiveSnapshot::for_testing(vec![entry(200, "codex app-server")], BTreeMap::new());
        assert!(
            !snapshot
                .status(RuntimeAdapter::Codex, NativeCapabilityKind::Cli)
                .running
        );
        assert!(
            snapshot
                .status(RuntimeAdapter::Codex, NativeCapabilityKind::AppServer)
                .running
        );
    }

    #[test]
    fn desktop_apps_match_bundle_path_and_cli_excludes_it() {
        let snapshot = LiveSnapshot::for_testing(
            vec![
                entry(300, "fixture/Antigravity.app/Contents/MacOS/Antigravity"),
                entry(301, "agy"),
            ],
            BTreeMap::new(),
        );
        let desktop = snapshot.status(RuntimeAdapter::Antigravity, NativeCapabilityKind::Desktop);
        assert_eq!(desktop.pid, Some(300));
        let cli = snapshot.status(RuntimeAdapter::Antigravity, NativeCapabilityKind::Cli);
        assert_eq!(cli.pid, Some(301));
    }

    #[test]
    fn openclaw_gateway_is_distinct_from_cli_and_acp() {
        let snapshot = LiveSnapshot::for_testing(
            vec![
                entry(400, "openclaw"),
                entry(401, "openclaw acp --url ws://127.0.0.1:24189"),
                entry(402, "openclaw gateway --port 24189 run"),
            ],
            BTreeMap::from([(402, vec![24189])]),
        );
        assert_eq!(
            snapshot
                .status(RuntimeAdapter::OpenClaw, NativeCapabilityKind::Cli)
                .pid,
            Some(400)
        );
        assert_eq!(
            snapshot
                .status(RuntimeAdapter::OpenClaw, NativeCapabilityKind::Acp)
                .pid,
            Some(401)
        );
        let gateway = snapshot.status(RuntimeAdapter::OpenClaw, NativeCapabilityKind::Gateway);
        assert_eq!(gateway.pid, Some(402));
        assert_eq!(gateway.port, Some(24189));
    }

    #[test]
    fn lowest_matching_pid_is_deterministic_evidence() {
        let snapshot = LiveSnapshot::for_testing(
            vec![entry(500, "claude"), entry(42, "claude")],
            BTreeMap::new(),
        );
        assert_eq!(
            snapshot
                .status(RuntimeAdapter::ClaudeCode, NativeCapabilityKind::Cli)
                .pid,
            Some(42)
        );
    }

    #[test]
    fn unmatched_capabilities_report_not_running() {
        let snapshot = LiveSnapshot::for_testing(vec![], BTreeMap::new());
        let status = snapshot.status(RuntimeAdapter::Hermes, NativeCapabilityKind::TuiGateway);
        assert!(!status.running);
        assert_eq!(status.pid, None);
        assert_eq!(status.port, None);
    }

    #[test]
    fn pi_rpc_mode_is_separated_from_plain_cli() {
        let snapshot = LiveSnapshot::for_testing(
            vec![entry(600, "pi --mode rpc"), entry(601, "pi")],
            BTreeMap::new(),
        );
        assert_eq!(
            snapshot
                .status(RuntimeAdapter::Pi, NativeCapabilityKind::Rpc)
                .pid,
            Some(600)
        );
        assert_eq!(
            snapshot
                .status(RuntimeAdapter::Pi, NativeCapabilityKind::Cli)
                .pid,
            Some(601)
        );
    }
}
