use super::binaries::find_binary;
use super::catalog::target_def;
use super::parameters::param_bool;
use super::probe_pool::{run_bounded_target_probes, target_scan_concurrency};
use crate::platform::virtual_machine::{SshRuntimeConnection, supports_virtual_machine_target};
use licoup_foundation::platform::process_supervisor::run_bounded_command_output;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

const DISCOVERY_SOURCE: &str = "virtual-machine-orbstack";
const DISCOVERY_PROTOCOL: &[u8] = b"licoup-vm-discovery-v2";
const MAX_MACHINE_LIST_OUTPUT_BYTES: usize = 16 * 1024;
const MAX_MACHINE_NAME_BYTES: usize = 128;
const MAX_RUNNING_MACHINES: usize = 32;
const MAX_VM_PROBE_CONCURRENCY: usize = 4;
const MAX_VM_PROBE_OUTPUT_BYTES: usize = 8 * 1024;
const MAX_GUEST_EXECUTABLE_BYTES: usize = 1024;
const MAX_GUEST_HOME_BYTES: usize = 4096;
const MACHINE_LIST_TIMEOUT: Duration = Duration::from_secs(3);
const MACHINE_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// This script is intentionally constant. Machine names are passed as distinct
/// argv values and no caller-controlled command fragment enters the guest
/// shell. Only executable metadata from documented/common install locations is
/// returned; private configuration and history stores are never read.
const GUEST_DISCOVERY_SCRIPT: &str = r#"
if [ -z "$HOME" ] || [ "${HOME#/}" = "$HOME" ]; then
  exit 2
fi

printf 'licoup-vm-discovery-v2\0home\0%s\0' "$HOME"

system_usr_root=/usr

find_target() {
  command_name=$1
  shift

  command_path=$(command -v "$command_name" 2>/dev/null || true)
  case "$command_path" in
    /*)
      if [ -f "$command_path" ] && [ -x "$command_path" ]; then
        printf '%s' "$command_path"
        return
      fi
      ;;
  esac

  for command_path in "$@"; do
    if [ -f "$command_path" ] && [ -x "$command_path" ]; then
      printf '%s' "$command_path"
      return
    fi
  done
}

openclaw_path=$(find_target openclaw \
  "$HOME/.openclaw/bin/openclaw" \
  "$HOME/.local/bin/openclaw" \
  "$HOME/.npm-global/bin/openclaw" \
  "$HOME/.local/share/pnpm/openclaw" \
  "$HOME/.bun/bin/openclaw" \
  "$HOME/.volta/bin/openclaw" \
  "$HOME/.nix-profile/bin/openclaw" \
  "$system_usr_root/local/bin/openclaw" \
  "$system_usr_root/bin/openclaw" \
  /snap/bin/openclaw)
if [ -n "$openclaw_path" ]; then
  printf 'openclaw\0%s\0' "$openclaw_path"
fi

hermes_path=$(find_target hermes \
  "$HOME/.local/bin/hermes" \
  "$HOME/.hermes/hermes-agent/venv/bin/hermes" \
  "$HOME/.hermes/bin/hermes" \
  "$HOME/.nix-profile/bin/hermes" \
  "$system_usr_root/local/bin/hermes" \
  "$system_usr_root/bin/hermes")
if [ -n "$hermes_path" ]; then
  if "$hermes_path" acp --check >/dev/null 2>&1; then
    printf 'hermes\0%s\0' "$hermes_path"
  else
    hermes_gateway_python=
    hermes_bin_dir=$(dirname "$hermes_path")
    for python_path in \
      "$HOME/.hermes/hermes-agent/venv/bin/python" \
      "$hermes_bin_dir/python" \
      "$hermes_bin_dir/python3"
    do
      if [ -x "$python_path" ] && \
        "$python_path" -c 'import importlib.util; raise SystemExit(0 if importlib.util.find_spec("tui_gateway.entry") else 1)' \
          >/dev/null 2>&1
      then
        hermes_gateway_python=$python_path
        break
      fi
    done
    if [ -n "$hermes_gateway_python" ]; then
      printf 'hermesGateway\0%s\0' "$hermes_gateway_python"
    else
      printf 'hermes\0%s\0' "$hermes_path"
    fi
  fi
fi
"#;

#[derive(Clone, Debug)]
pub(super) struct AutomaticVmTarget {
    pub(super) label: String,
    pub(super) runtime_connection: SshRuntimeConnection,
}

#[derive(Debug, Default)]
pub(super) struct VmDiscovery {
    pub(super) targets: BTreeMap<String, AutomaticVmTarget>,
    pub(super) scope_available: bool,
    pub(super) diagnostics: Vec<Value>,
}

#[derive(Clone, Debug)]
enum CommandCapture {
    Complete(Vec<u8>),
    Failed,
    TimedOut,
    Truncated,
    Unsuccessful,
}

trait OrbStackProbeRunner: Sync {
    fn list_running_machines(&self) -> CommandCapture;
    fn probe_machine(&self, machine: &str) -> CommandCapture;
}

struct SystemOrbStackProbeRunner {
    orb: PathBuf,
}

impl OrbStackProbeRunner for SystemOrbStackProbeRunner {
    fn list_running_machines(&self) -> CommandCapture {
        let mut command = Command::new(&self.orb);
        command.args(["list", "--running", "--quiet"]);
        capture_command(
            &mut command,
            MACHINE_LIST_TIMEOUT,
            MAX_MACHINE_LIST_OUTPUT_BYTES,
        )
    }

    fn probe_machine(&self, machine: &str) -> CommandCapture {
        let mut command = Command::new(&self.orb);
        command
            .arg("-m")
            .arg(machine)
            .args(["sh", "-lc", GUEST_DISCOVERY_SCRIPT]);
        capture_command(
            &mut command,
            MACHINE_PROBE_TIMEOUT,
            MAX_VM_PROBE_OUTPUT_BYTES,
        )
    }
}

fn capture_command(command: &mut Command, timeout: Duration, max_output: usize) -> CommandCapture {
    let Ok(output) = run_bounded_command_output(command, timeout, max_output) else {
        return CommandCapture::Failed;
    };
    if output.timed_out {
        return CommandCapture::TimedOut;
    }
    if output.truncated {
        return CommandCapture::Truncated;
    }
    if !output.status.is_some_and(|status| status.success()) {
        return CommandCapture::Unsuccessful;
    }
    CommandCapture::Complete(output.stdout)
}

pub(super) fn discover_virtual_machine_targets(
    params: &Value,
    requested_targets: &[&str],
) -> VmDiscovery {
    if param_bool(params, "includeAccessibleEnvironments") != Some(true)
        || !requested_targets
            .iter()
            .any(|target| supports_virtual_machine_target(target))
    {
        return VmDiscovery::default();
    }
    let Some(orb) = find_binary(&["orb"]) else {
        return VmDiscovery::default();
    };
    discover_with_runner(
        params,
        requested_targets,
        &SystemOrbStackProbeRunner { orb },
    )
}

fn discover_with_runner(
    params: &Value,
    requested_targets: &[&str],
    runner: &dyn OrbStackProbeRunner,
) -> VmDiscovery {
    if param_bool(params, "includeAccessibleEnvironments") != Some(true) {
        return VmDiscovery::default();
    }
    let requested = requested_targets
        .iter()
        .copied()
        .filter(|target| supports_virtual_machine_target(target))
        .collect::<BTreeSet<_>>();
    if requested.is_empty() {
        return VmDiscovery::default();
    }

    let mut discovery = VmDiscovery {
        scope_available: true,
        ..VmDiscovery::default()
    };
    let list_output = match runner.list_running_machines() {
        CommandCapture::Complete(output) => output,
        other => {
            discovery
                .diagnostics
                .push(discovery_diagnostic("machine-list", capture_status(&other)));
            return discovery;
        }
    };
    let Some(machine_list) = parse_running_machines(&list_output) else {
        discovery
            .diagnostics
            .push(discovery_diagnostic("machine-list", "invalid-output"));
        return discovery;
    };
    if machine_list.filtered {
        discovery
            .diagnostics
            .push(discovery_diagnostic("machine-list", "filtered"));
    }
    if machine_list.limited {
        discovery
            .diagnostics
            .push(discovery_diagnostic("machine-list", "bounded"));
    }
    if machine_list.machines.is_empty() {
        return discovery;
    }

    let concurrency =
        target_scan_concurrency(params, machine_list.machines.len()).min(MAX_VM_PROBE_CONCURRENCY);
    let probed =
        run_bounded_target_probes(machine_list.machines, concurrency, |machine: String| {
            Ok::<_, anyhow::Error>((machine.clone(), runner.probe_machine(&machine)))
        });
    let Ok(probed) = probed else {
        discovery
            .diagnostics
            .push(discovery_diagnostic("machine-probe", "worker-failed"));
        return discovery;
    };

    let mut selected = BTreeMap::<String, (u8, AutomaticVmTarget)>::new();
    let mut probe_failed = false;
    for (machine, capture) in probed {
        let CommandCapture::Complete(output) = capture else {
            probe_failed = true;
            continue;
        };
        let Some(probe) = parse_probe_output(&output) else {
            probe_failed = true;
            continue;
        };
        for (target, executable) in probe.executables {
            if !requested.contains(target.as_str()) {
                continue;
            }
            let Some(candidate) = automatic_target(&target, &machine, &executable, &probe.home)
            else {
                probe_failed = true;
                continue;
            };
            let priority = preferred_machine_priority(&target, &machine);
            match selected.get(&target) {
                Some((current_priority, _)) if *current_priority <= priority => {}
                _ => {
                    selected.insert(target, (priority, candidate));
                }
            }
        }
    }
    if probe_failed {
        discovery
            .diagnostics
            .push(discovery_diagnostic("machine-probe", "partial"));
    }
    discovery.targets = selected
        .into_iter()
        .map(|(target, (_, candidate))| (target, candidate))
        .collect();
    discovery
}

fn capture_status(capture: &CommandCapture) -> &'static str {
    match capture {
        CommandCapture::Complete(_) => "complete",
        CommandCapture::Failed => "command-failed",
        CommandCapture::TimedOut => "timeout",
        CommandCapture::Truncated => "output-too-large",
        CommandCapture::Unsuccessful => "command-exited",
    }
}

fn discovery_diagnostic(stage: &str, status: &str) -> Value {
    json!({
        "source": DISCOVERY_SOURCE,
        "stage": stage,
        "status": status,
    })
}

struct ParsedMachineList {
    machines: Vec<String>,
    filtered: bool,
    limited: bool,
}

fn parse_running_machines(output: &[u8]) -> Option<ParsedMachineList> {
    let text = std::str::from_utf8(output).ok()?;
    let mut names = BTreeSet::new();
    let mut filtered = false;
    for line in text.lines() {
        let name = line.trim();
        if name.is_empty() {
            continue;
        }
        if valid_machine_name(name) {
            names.insert(name.to_string());
        } else {
            filtered = true;
        }
    }
    let limited = names.len() > MAX_RUNNING_MACHINES;
    Some(ParsedMachineList {
        machines: names.into_iter().take(MAX_RUNNING_MACHINES).collect(),
        filtered,
        limited,
    })
}

fn valid_machine_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_MACHINE_NAME_BYTES
        && name.trim() == name
        && name != "."
        && name != ".."
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

struct GuestProbe {
    home: String,
    executables: BTreeMap<String, GuestExecutable>,
}

struct GuestExecutable {
    path: String,
    runtime_protocol: Option<&'static str>,
}

fn parse_probe_output(output: &[u8]) -> Option<GuestProbe> {
    let mut fields = output.split(|byte| *byte == 0).collect::<Vec<_>>();
    if fields.last().is_some_and(|field| field.is_empty()) {
        fields.pop();
    }
    if fields.first().copied() != Some(DISCOVERY_PROTOCOL)
        || fields.len() < 3
        || (fields.len() - 1) % 2 != 0
    {
        return None;
    }

    let mut home = None;
    let mut executables = BTreeMap::new();
    for pair in fields[1..].chunks_exact(2) {
        let key = std::str::from_utf8(pair[0]).ok()?;
        let value = std::str::from_utf8(pair[1]).ok()?;
        match key {
            "home" if home.is_none() && valid_guest_path(value, MAX_GUEST_HOME_BYTES, true) => {
                home = Some(value.to_string());
            }
            "openclaw"
                if !executables.contains_key("openclaw")
                    && valid_guest_path(value, MAX_GUEST_EXECUTABLE_BYTES, false) =>
            {
                executables.insert(
                    "openclaw".to_string(),
                    GuestExecutable {
                        path: value.to_string(),
                        runtime_protocol: None,
                    },
                );
            }
            "hermes"
                if !executables.contains_key("hermes")
                    && valid_guest_path(value, MAX_GUEST_EXECUTABLE_BYTES, false) =>
            {
                executables.insert(
                    "hermes".to_string(),
                    GuestExecutable {
                        path: value.to_string(),
                        runtime_protocol: None,
                    },
                );
            }
            "hermesGateway"
                if !executables.contains_key("hermes")
                    && valid_guest_path(value, MAX_GUEST_EXECUTABLE_BYTES, false) =>
            {
                executables.insert(
                    "hermes".to_string(),
                    GuestExecutable {
                        path: value.to_string(),
                        runtime_protocol: Some("hermes-tui-gateway"),
                    },
                );
            }
            _ => return None,
        }
    }
    Some(GuestProbe {
        home: home?,
        executables,
    })
}

fn valid_guest_path(value: &str, max_bytes: usize, allow_root: bool) -> bool {
    if value.is_empty()
        || value.len() > max_bytes
        || value.trim() != value
        || !value.starts_with('/')
        || value.bytes().any(|byte| byte.is_ascii_control())
        || (!allow_root && value == "/")
    {
        return false;
    }
    if value == "/" {
        return true;
    }
    value[1..]
        .split('/')
        .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

fn automatic_target(
    target: &str,
    machine: &str,
    executable: &GuestExecutable,
    home: &str,
) -> Option<AutomaticVmTarget> {
    let def = target_def(target).ok()?;
    let mut connection = json!({
        "kind": "ssh",
        "host": "orb",
        "user": machine,
        "remoteExecutable": executable.path,
        "workingDirectory": home,
    });
    if let Some(runtime_protocol) = executable.runtime_protocol {
        connection["runtimeProtocol"] = json!(runtime_protocol);
    }
    let connection = SshRuntimeConnection::from_value(Some(&connection), target)
        .ok()
        .flatten()?;
    Some(AutomaticVmTarget {
        label: format!("{} · {}", def.label, machine),
        runtime_connection: connection,
    })
}

fn preferred_machine_priority(target: &str, machine: &str) -> u8 {
    let preferred = match target {
        "openclaw" => "kate",
        "hermes" => "serena",
        _ => return 1,
    };
    if machine.eq_ignore_ascii_case(preferred) {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeRunner {
        list: CommandCapture,
        probes: BTreeMap<String, CommandCapture>,
        list_calls: AtomicUsize,
        probe_calls: AtomicUsize,
    }

    impl FakeRunner {
        fn new(list: CommandCapture, probes: BTreeMap<String, CommandCapture>) -> Self {
            Self {
                list,
                probes,
                list_calls: AtomicUsize::new(0),
                probe_calls: AtomicUsize::new(0),
            }
        }
    }

    impl OrbStackProbeRunner for FakeRunner {
        fn list_running_machines(&self) -> CommandCapture {
            self.list_calls.fetch_add(1, Ordering::SeqCst);
            self.list.clone()
        }

        fn probe_machine(&self, machine: &str) -> CommandCapture {
            self.probe_calls.fetch_add(1, Ordering::SeqCst);
            self.probes
                .get(machine)
                .cloned()
                .unwrap_or(CommandCapture::Failed)
        }
    }

    fn encoded_probe(home: &str, executables: &[(&str, &str)]) -> Vec<u8> {
        let mut output = Vec::new();
        for field in std::iter::once(("licoup-vm-discovery-v2", "home"))
            .flat_map(|(protocol, home_key)| [protocol, home_key, home])
            .chain(
                executables
                    .iter()
                    .flat_map(|(target, path)| [*target, *path]),
            )
        {
            output.extend_from_slice(field.as_bytes());
            output.push(0);
        }
        output
    }

    fn guest_path(segments: &[&str]) -> String {
        format!("/{}", segments.join("/"))
    }

    #[test]
    fn disabled_accessible_environment_scan_never_invokes_runner() {
        let runner = FakeRunner::new(CommandCapture::Failed, BTreeMap::new());
        let result = discover_with_runner(
            &json!({"includeAccessibleEnvironments": false}),
            &["openclaw", "hermes"],
            &runner,
        );
        assert!(!result.scope_available);
        assert!(result.targets.is_empty());
        assert_eq!(runner.list_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn machine_list_is_validated_sorted_deduplicated_and_bounded() {
        let mut lines = vec![
            "zeta".to_string(),
            "alpha".to_string(),
            "alpha".to_string(),
            "-option".to_string(),
            "bad@host".to_string(),
            "..".to_string(),
        ];
        lines.extend((0..MAX_RUNNING_MACHINES + 2).map(|index| format!("vm-{index:02}")));
        let parsed = parse_running_machines(lines.join("\n").as_bytes()).unwrap();
        assert!(parsed.filtered);
        assert!(parsed.limited);
        assert_eq!(parsed.machines.len(), MAX_RUNNING_MACHINES);
        assert_eq!(parsed.machines[0], "alpha");
        assert!(parsed.machines.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn official_guest_paths_become_strict_ssh_runtime_targets() {
        let list = CommandCapture::Complete(b"other\nKate\nSerena\n".to_vec());
        let kate_home = guest_path(&["users", "kate"]);
        let kate_openclaw = guest_path(&["users", "kate", ".openclaw", "bin", "openclaw"]);
        let serena_home = guest_path(&["users", "serena"]);
        let serena_python = guest_path(&[
            "users",
            "serena",
            ".hermes",
            "hermes-agent",
            "venv",
            "bin",
            "python",
        ]);
        let other_home = guest_path(&["users", "other"]);
        let system_openclaw = guest_path(&["usr", "local", "bin", "openclaw"]);
        let system_hermes = guest_path(&["usr", "local", "bin", "hermes"]);
        let probes = BTreeMap::from([
            (
                "Kate".to_string(),
                CommandCapture::Complete(encoded_probe(
                    &kate_home,
                    &[("openclaw", &kate_openclaw)],
                )),
            ),
            (
                "Serena".to_string(),
                CommandCapture::Complete(encoded_probe(
                    &serena_home,
                    &[("hermesGateway", &serena_python)],
                )),
            ),
            (
                "other".to_string(),
                CommandCapture::Complete(encoded_probe(
                    &other_home,
                    &[("openclaw", &system_openclaw), ("hermes", &system_hermes)],
                )),
            ),
        ]);
        let runner = FakeRunner::new(list, probes);
        let result = discover_with_runner(
            &json!({"includeAccessibleEnvironments": true}),
            &["openclaw", "hermes"],
            &runner,
        );

        assert!(result.scope_available);
        assert_eq!(result.targets.len(), 2);
        let openclaw = result.targets.get("openclaw").unwrap();
        let hermes = result.targets.get("hermes").unwrap();
        assert!(openclaw.label.ends_with("Kate"));
        assert!(hermes.label.ends_with("Serena"));
        assert_eq!(
            openclaw.runtime_connection.to_value()["user"],
            json!("Kate")
        );
        assert_eq!(hermes.runtime_connection.to_value()["host"], json!("orb"));
        assert_eq!(
            hermes.runtime_connection.to_value()["runtimeProtocol"],
            json!("hermes-tui-gateway")
        );
        assert_eq!(runner.probe_calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn malformed_or_command_shaped_probe_fields_are_rejected() {
        let agent_home = guest_path(&["users", "agent"]);
        assert!(
            parse_probe_output(&encoded_probe(&agent_home, &[("openclaw", "openclaw")])).is_none()
        );
        let traversing_path = format!("{agent_home}/../bin/openclaw");
        assert!(
            parse_probe_output(&encoded_probe(
                &agent_home,
                &[("openclaw", &traversing_path)]
            ))
            .is_none()
        );
        let multiline_home = format!("{agent_home}\nother");
        let system_hermes = guest_path(&["usr", "local", "bin", "hermes"]);
        assert!(
            parse_probe_output(&encoded_probe(
                &multiline_home,
                &[("hermes", &system_hermes)]
            ))
            .is_none()
        );
        assert!(GUEST_DISCOVERY_SCRIPT.contains("$HOME/.openclaw/bin/openclaw"));
        assert!(GUEST_DISCOVERY_SCRIPT.contains("$HOME/.hermes/hermes-agent/venv/bin/hermes"));
    }
}
