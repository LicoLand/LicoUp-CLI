//! Cursor Agent CLI auto-update watcher.
//!
//! `cursor-agent` may auto-update itself when a turn starts: it downloads a
//! new version from `downloads.cursor.com` (curl piped into tar extracting
//! into `versions/.<version>`) while a lock file `.install.lock` exists in
//! its install root. During the update the CLI emits no stream-json output,
//! so a send would otherwise look stuck. This watcher detects the state from
//! the vendor's own signals and reports phase transitions so the client can
//! render a progress card.
//!
//! Signals (verified against the real CLI):
//! - lock file `$root/.install.lock` — empty existence marker; the vendor's
//!   own error text instructs removal when no update is running.
//! - staging dir `$root/versions/.<version>` — present while extracting;
//!   renamed to `versions/<version>` on completion.
//! - update processes: `curl ...downloads.cursor.com...agent-cli-package...`
//!   and `tar ... -C .../versions/.<version>` as descendants of the turn.
//!
//! No vendor signal exposes a real percentage, so progress is reported as
//! phases (preparing/downloading/installing) and the client renders an
//! indeterminate bar. Cleanup is limited to removing a stale lock file when
//! no update is running; `versions/` contents are never touched.

use licoup_foundation::platform::process_supervisor::run_bounded_command_output;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Poll cadence inside the turn consume loop.
pub(super) const UPDATE_WATCH_INTERVAL: Duration = Duration::from_secs(1);
/// A lock older than this with no update process and no staging dir is stale.
const STALE_LOCK_GRACE: Duration = Duration::from_secs(15);
/// Bounds for the `ps` descendant scan (matches live_status.rs constants).
const PS_TIMEOUT: Duration = Duration::from_secs(2);
const PS_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Observable phase of a cursor-agent update.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum UpdatePhase {
    Preparing,
    Downloading,
    Installing,
}

impl UpdatePhase {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            UpdatePhase::Preparing => "preparing",
            UpdatePhase::Downloading => "downloading",
            UpdatePhase::Installing => "installing",
        }
    }
}

/// A state transition worth surfacing to the conversation.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum UpdateChange {
    Started {
        version: Option<String>,
        phase: UpdatePhase,
    },
    Phase {
        version: Option<String>,
        phase: UpdatePhase,
    },
    Completed {
        version: Option<String>,
    },
    Interrupted {
        version: Option<String>,
    },
}

/// One parsed `ps -axo pid=,ppid=,command=` line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ProcessLine {
    pid: u32,
    ppid: u32,
    command: String,
}

pub(super) struct AgentUpdateWatcher {
    root: PathBuf,
    state: WatchState,
    process_reader: Box<dyn FnMut() -> Vec<ProcessLine>>,
}

#[derive(Debug)]
enum WatchState {
    Idle,
    Updating {
        version: Option<String>,
        last_phase: Option<UpdatePhase>,
    },
}

/// One observation snapshot used by the state machine.
struct Observation {
    lock_modified: Option<SystemTime>,
    lock_fresh: bool,
    staging: Option<String>,
    update_process: Option<(UpdateProcessKind, Option<String>)>,
}

impl AgentUpdateWatcher {
    pub(super) fn new(root: PathBuf) -> Self {
        Self::with_process_reader(root, default_ps_scan)
    }

    fn with_process_reader(
        root: PathBuf,
        process_reader: impl FnMut() -> Vec<ProcessLine> + 'static,
    ) -> Self {
        Self {
            root,
            state: WatchState::Idle,
            process_reader: Box::new(process_reader),
        }
    }

    /// Polls the vendor signals once and returns a transition, if any.
    pub(super) fn watch(&mut self, root_pid: u32) -> Option<UpdateChange> {
        let lock_path = self.root.join(".install.lock");
        let lock_modified = lock_modified_time(&lock_path);
        let staging = staging_version(&self.root);
        // The process scan is the expensive part; run it once per poll, and
        // only when a lock or staging signal is present (or already updating).
        let updating_locked = matches!(self.state, WatchState::Updating { .. });
        let update_process = if lock_modified.is_some() || staging.is_some() || updating_locked {
            update_processes(&mut self.process_reader, root_pid)
        } else {
            None
        };
        let lock_fresh = lock_modified.is_some_and(|modified| {
            SystemTime::now()
                .duration_since(modified)
                .is_ok_and(|age| age <= STALE_LOCK_GRACE)
        });
        let observation = Observation {
            lock_modified,
            lock_fresh,
            staging,
            update_process,
        };

        // Take the state out so transitions can freely build the next one
        // without holding borrows across `self.state` assignment.
        let state = std::mem::replace(&mut self.state, WatchState::Idle);
        let (next_state, change) = match state {
            WatchState::Idle => {
                // A staging directory is supporting phase/version evidence,
                // not start authority. Interrupted installs may leave one
                // behind indefinitely, so require a live lock or updater.
                let starting = observation.lock_fresh || observation.update_process.is_some();
                if starting {
                    let (version, phase) = observe_phase(&observation);
                    (
                        WatchState::Updating {
                            version: version.clone(),
                            last_phase: Some(phase),
                        },
                        Some(UpdateChange::Started { version, phase }),
                    )
                } else if observation.lock_modified.is_some() {
                    // Stale lock with no update activity: silent
                    // vendor-sanctioned cleanup (no spurious card).
                    let _ = std::fs::remove_file(&lock_path);
                    (WatchState::Idle, None)
                } else {
                    (WatchState::Idle, None)
                }
            }
            WatchState::Updating {
                version,
                last_phase,
            } => {
                let no_activity = observation.lock_modified.is_none()
                    && observation.staging.is_none()
                    && observation.update_process.is_none();
                if no_activity {
                    // Lock and staging gone: the install finished (the CLI may
                    // re-exec, so the process tree is not the ground truth).
                    (WatchState::Idle, Some(UpdateChange::Completed { version }))
                } else if !observation.lock_fresh && observation.update_process.is_none() {
                    // A staging directory can survive an interrupted install.
                    // Without a fresh lock or updater process it is residue,
                    // not evidence that this turn is still updating.
                    if observation.lock_modified.is_some() {
                        let _ = std::fs::remove_file(&lock_path);
                    }
                    let version = observation.staging.or(version);
                    (
                        WatchState::Idle,
                        Some(UpdateChange::Interrupted { version }),
                    )
                } else if observation.lock_modified.is_some()
                    && observation.staging.is_none()
                    && observation.update_process.is_none()
                {
                    if observation.lock_fresh {
                        // The vendor spawns the downloader right after the
                        // lock; a sub-second gap must not interrupt.
                        let phase = UpdatePhase::Preparing;
                        let changed = last_phase != Some(phase);
                        let change = changed.then(|| UpdateChange::Phase {
                            version: version.clone(),
                            phase,
                        });
                        (
                            WatchState::Updating {
                                version,
                                last_phase: Some(phase),
                            },
                            change,
                        )
                    } else {
                        // Stale lock with no activity: interrupted; remove it.
                        let _ = std::fs::remove_file(&lock_path);
                        (
                            WatchState::Idle,
                            Some(UpdateChange::Interrupted { version }),
                        )
                    }
                } else {
                    let (observed_version, phase) = observe_phase(&observation);
                    let version = observed_version.or(version);
                    let changed = last_phase != Some(phase);
                    let change = changed.then(|| UpdateChange::Phase {
                        version: version.clone(),
                        phase,
                    });
                    (
                        WatchState::Updating {
                            version,
                            last_phase: Some(phase),
                        },
                        change,
                    )
                }
            }
        };
        self.state = next_state;
        change
    }
}

/// Resolves the cursor-agent install root: test override env var first, then
/// `$HOME/.local/share/cursor-agent`. A missing HOME degrades to an empty
/// root where every probe fails — the watcher becomes a silent no-op.
pub(super) fn cursor_agent_install_dir() -> PathBuf {
    resolve_install_dir(
        std::env::var_os("LICO_CURSOR_AGENT_INSTALL_DIR"),
        std::env::var_os("HOME"),
    )
}

fn resolve_install_dir(
    override_value: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> PathBuf {
    if let Some(value) = override_value {
        let path = PathBuf::from(value);
        if !path.as_os_str().is_empty() {
            return path;
        }
    }
    home.map(PathBuf::from)
        .map(|home| home.join(".local").join("share").join("cursor-agent"))
        .unwrap_or_default()
}

/// Latest staging version in `$root/versions/` (dirs named `.`-prefixed).
fn staging_version(root: &Path) -> Option<String> {
    let entries = std::fs::read_dir(root.join("versions")).ok()?;
    let mut found: Option<String> = None;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(stripped) = name.strip_prefix('.') {
            if !stripped.is_empty() && entry.path().is_dir() {
                found = Some(stripped.to_string());
            }
        }
    }
    found
}

fn lock_modified_time(lock_path: &Path) -> Option<SystemTime> {
    let metadata = std::fs::metadata(lock_path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    metadata.modified().ok()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UpdateProcessKind {
    Curl,
    Tar,
}

/// Finds the first update-process descendant of `root_pid` (BFS over the
/// ppid tree), returning its kind and a curl-derived version candidate.
fn update_processes(
    reader: &mut Box<dyn FnMut() -> Vec<ProcessLine>>,
    root_pid: u32,
) -> Option<(UpdateProcessKind, Option<String>)> {
    let lines = reader();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for line in &lines {
        children.entry(line.ppid).or_default().push(line.pid);
    }
    let mut queue: Vec<u32> = children.get(&root_pid).cloned().unwrap_or_default();
    let mut seen = HashSet::new();
    let mut curl_version: Option<String> = None;
    let mut found_kind: Option<UpdateProcessKind> = None;
    while let Some(pid) = queue.pop() {
        if !seen.insert(pid) {
            continue;
        }
        if let Some(line) = lines.iter().find(|line| line.pid == pid) {
            if is_update_process(&line.command) {
                let kind = if line.command.contains("downloads.cursor.com") {
                    UpdateProcessKind::Curl
                } else {
                    UpdateProcessKind::Tar
                };
                if kind == UpdateProcessKind::Curl {
                    curl_version = version_from_curl_url(&line.command);
                }
                if found_kind.is_none() {
                    found_kind = Some(kind);
                }
            }
            if let Some(children) = children.get(&pid) {
                queue.extend(children.iter().copied());
            }
        }
    }
    found_kind.map(|kind| (kind, curl_version))
}

/// Narrow matcher for cursor-agent update processes. Deliberately excludes
/// agent-spawned tools that merely mention cursor-agent. The extractor is
/// recognized by the staging marker `versions/.` (only the update staging
/// directory is dot-prefixed) plus the tar binary.
fn is_update_process(command: &str) -> bool {
    command.contains("downloads.cursor.com")
        || command.contains("agent-cli-package")
        || (command.contains("tar") && command.contains("versions/."))
}

fn version_from_curl_url(command: &str) -> Option<String> {
    let marker = "downloads.cursor.com/lab/";
    let start = command.find(marker)? + marker.len();
    let rest = &command[start..];
    let end = rest.find('/')?;
    let version = &rest[..end];
    (!version.is_empty()).then(|| version.to_string())
}

/// Combines staging and process signals into (version, phase).
fn observe_phase(observation: &Observation) -> (Option<String>, UpdatePhase) {
    let phase = match observation.update_process.as_ref().map(|(kind, _)| *kind) {
        Some(UpdateProcessKind::Curl) => UpdatePhase::Downloading,
        Some(UpdateProcessKind::Tar) => UpdatePhase::Installing,
        None if observation.staging.is_some() => UpdatePhase::Installing,
        None => UpdatePhase::Preparing,
    };
    let version = observation.staging.clone().or_else(|| {
        observation
            .update_process
            .as_ref()
            .and_then(|(_, version)| version.clone())
    });
    (version, phase)
}

fn default_ps_scan() -> Vec<ProcessLine> {
    let mut command = std::process::Command::new("ps");
    command.args(["-axo", "pid=,ppid=,command="]);
    let Ok(result) = run_bounded_command_output(&mut command, PS_TIMEOUT, PS_MAX_BYTES) else {
        return Vec::new();
    };
    if result.timed_out || result.truncated || !result.status.is_some_and(|status| status.success())
    {
        return Vec::new();
    }
    parse_ps_lines(&String::from_utf8_lossy(&result.stdout))
}

/// Parses `pid ppid command...` lines; skips malformed and empty commands.
fn parse_ps_lines(output: &str) -> Vec<ProcessLine> {
    let mut lines = Vec::new();
    for raw in output.lines() {
        // Leading whitespace would otherwise become empty split segments and
        // misalign pid/ppid/command.
        let mut parts = raw
            .trim_start()
            .splitn(3, char::is_whitespace)
            .filter(|part| !part.is_empty());
        let (Some(pid), Some(ppid), Some(command)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let (Ok(pid), Ok(ppid)) = (pid.parse::<u32>(), ppid.parse::<u32>()) else {
            continue;
        };
        if command.is_empty() {
            continue;
        }
        lines.push(ProcessLine {
            pid,
            ppid,
            command: command.to_string(),
        });
    }
    lines
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Arc, Mutex};
    use std::time::UNIX_EPOCH;

    fn temp_root(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("lico-cursor-update-watcher-{label}-{stamp}"));
        fs::create_dir_all(root.join("versions")).unwrap();
        root
    }

    fn touch_lock(root: &std::path::Path) {
        fs::write(root.join(".install.lock"), b"").unwrap();
    }

    fn backdate_lock(root: &std::path::Path, age: Duration) {
        let path = root.join(".install.lock");
        let file = fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::now() - age).unwrap();
    }

    fn staging(root: &std::path::Path, version: &str) {
        fs::create_dir_all(root.join("versions").join(format!(".{version}"))).unwrap();
    }

    fn curl_line(pid: u32, ppid: u32, version: &str) -> ProcessLine {
        ProcessLine {
            pid,
            ppid,
            command: format!(
                "curl -fSL -s https://downloads.cursor.com/lab/{version}/darwin/arm64/agent-cli-package.tar.gz"
            ),
        }
    }

    fn tar_line(pid: u32, ppid: u32) -> ProcessLine {
        ProcessLine {
            pid,
            ppid,
            command:
                "tar --strip-components=1 -xzf - -C /fixture-root/versions/.2026.08.04-aaa8809"
                    .to_string(),
        }
    }

    fn reader(
        processes: &Arc<Mutex<Vec<ProcessLine>>>,
    ) -> impl FnMut() -> Vec<ProcessLine> + 'static {
        let processes = Arc::clone(processes);
        move || processes.lock().unwrap().clone()
    }

    #[test]
    fn stale_lock_is_removed_without_card_when_idle() {
        let root = temp_root("stale-idle");
        touch_lock(&root);
        backdate_lock(&root, Duration::from_secs(60));
        let mut watcher = AgentUpdateWatcher::with_process_reader(root.clone(), || vec![]);
        assert_eq!(watcher.watch(1001), None);
        assert!(!root.join(".install.lock").exists());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn lock_alone_starts_update_with_preparing_phase() {
        let root = temp_root("lock-only");
        touch_lock(&root);
        let mut watcher = AgentUpdateWatcher::with_process_reader(root.clone(), || vec![]);
        assert_eq!(
            watcher.watch(1001),
            Some(UpdateChange::Started {
                version: None,
                phase: UpdatePhase::Preparing,
            })
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn staging_dir_yields_version_and_installing_phase() {
        let root = temp_root("staging");
        touch_lock(&root);
        staging(&root, "2026.08.04-aaa8809");
        let mut watcher = AgentUpdateWatcher::with_process_reader(root.clone(), || vec![]);
        assert_eq!(
            watcher.watch(1001),
            Some(UpdateChange::Started {
                version: Some("2026.08.04-aaa8809".to_string()),
                phase: UpdatePhase::Installing,
            })
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn orphaned_staging_dir_does_not_start_an_update() {
        let root = temp_root("orphaned-staging");
        staging(&root, "2026.08.04-aaa8809");
        let mut watcher = AgentUpdateWatcher::with_process_reader(root.clone(), || vec![]);
        assert_eq!(watcher.watch(1001), None);
        assert_eq!(watcher.watch(1001), None);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn curl_to_tar_transitions_phase_in_one_watcher() {
        let root = temp_root("curl-tar");
        touch_lock(&root);
        let processes = Arc::new(Mutex::new(vec![curl_line(
            2002,
            1001,
            "2026.08.04-aaa8809",
        )]));
        let mut watcher = AgentUpdateWatcher::with_process_reader(root.clone(), reader(&processes));
        assert_eq!(
            watcher.watch(1001),
            Some(UpdateChange::Started {
                version: Some("2026.08.04-aaa8809".to_string()),
                phase: UpdatePhase::Downloading,
            })
        );
        // No change while still downloading.
        assert_eq!(watcher.watch(1001), None);
        // curl exits; tar keeps extracting -> phase transition.
        *processes.lock().unwrap() = vec![tar_line(2003, 1001)];
        assert_eq!(
            watcher.watch(1001),
            Some(UpdateChange::Phase {
                version: Some("2026.08.04-aaa8809".to_string()),
                phase: UpdatePhase::Installing,
            })
        );
        // Everything finishes.
        *processes.lock().unwrap() = Vec::new();
        fs::remove_file(root.join(".install.lock")).unwrap();
        assert_eq!(
            watcher.watch(1001),
            Some(UpdateChange::Completed {
                version: Some("2026.08.04-aaa8809".to_string()),
            })
        );
        assert_eq!(watcher.watch(1001), None);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn phase_change_only_emitted_on_transition() {
        let root = temp_root("phase-transition");
        touch_lock(&root);
        let mut watcher = AgentUpdateWatcher::with_process_reader(root.clone(), || vec![]);
        assert_eq!(
            watcher.watch(1001),
            Some(UpdateChange::Started {
                version: None,
                phase: UpdatePhase::Preparing,
            })
        );
        assert_eq!(watcher.watch(1001), None);
        assert_eq!(watcher.watch(1001), None);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn completion_emitted_once_when_signals_clear() {
        let root = temp_root("complete");
        touch_lock(&root);
        staging(&root, "2026.08.04-aaa8809");
        let mut watcher = AgentUpdateWatcher::with_process_reader(root.clone(), || vec![]);
        assert!(watcher.watch(1001).is_some());
        // Both signals disappear (install finished, staging renamed).
        fs::remove_file(root.join(".install.lock")).unwrap();
        fs::remove_dir_all(root.join("versions").join(".2026.08.04-aaa8809")).unwrap();
        assert_eq!(
            watcher.watch(1001),
            Some(UpdateChange::Completed {
                version: Some("2026.08.04-aaa8809".to_string()),
            })
        );
        assert_eq!(watcher.watch(1001), None);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn lock_disappears_without_staging_completes() {
        let root = temp_root("lock-gone");
        touch_lock(&root);
        let mut watcher = AgentUpdateWatcher::with_process_reader(root.clone(), || vec![]);
        assert!(watcher.watch(1001).is_some());
        fs::remove_file(root.join(".install.lock")).unwrap();
        assert_eq!(
            watcher.watch(1001),
            Some(UpdateChange::Completed { version: None })
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn stale_lock_interrupts_and_is_removed() {
        let root = temp_root("interrupt");
        touch_lock(&root);
        let mut watcher = AgentUpdateWatcher::with_process_reader(root.clone(), || vec![]);
        assert!(watcher.watch(1001).is_some());
        backdate_lock(&root, Duration::from_secs(60));
        assert_eq!(
            watcher.watch(1001),
            Some(UpdateChange::Interrupted { version: None })
        );
        assert!(!root.join(".install.lock").exists());
        assert_eq!(watcher.watch(1001), None);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn stale_lock_with_orphaned_staging_interrupts_once() {
        let root = temp_root("stale-staging");
        touch_lock(&root);
        staging(&root, "2026.08.04-aaa8809");
        let mut watcher = AgentUpdateWatcher::with_process_reader(root.clone(), || vec![]);
        assert!(watcher.watch(1001).is_some());
        backdate_lock(&root, Duration::from_secs(60));
        assert_eq!(
            watcher.watch(1001),
            Some(UpdateChange::Interrupted {
                version: Some("2026.08.04-aaa8809".to_string()),
            })
        );
        assert!(!root.join(".install.lock").exists());
        assert_eq!(watcher.watch(1001), None);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn fresh_lock_without_process_stays_updating() {
        let root = temp_root("fresh-stay");
        touch_lock(&root);
        let mut watcher = AgentUpdateWatcher::with_process_reader(root.clone(), || vec![]);
        assert!(watcher.watch(1001).is_some());
        // Fresh lock, no process, no staging: no interruption, no change.
        assert_eq!(watcher.watch(1001), None);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn parse_ps_lines_parses_and_skips_garbage() {
        let lines = parse_ps_lines(
            "  1 0 /sbin/launchd\n1001 900 /bin/sh -c cursor-agent --print\n2002 1001 curl -fSL -s https://downloads.cursor.com/lab/v1/darwin/arm64/agent-cli-package.tar.gz\nnot-a-pid 1 something\n",
        );
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1].pid, 1001);
        assert_eq!(lines[1].ppid, 900);
        assert!(lines[2].command.contains("downloads.cursor.com"));
    }

    #[test]
    fn is_update_process_narrow_match() {
        assert!(is_update_process(
            "curl -fSL -s https://downloads.cursor.com/lab/v1/darwin/arm64/agent-cli-package.tar.gz"
        ));
        assert!(is_update_process(
            "tar --strip-components=1 -xzf - -C /fixture-root/versions/.v1"
        ));
        assert!(!is_update_process(
            "cursor-agent --print --output-format stream-json --trust --force"
        ));
        assert!(!is_update_process(
            "/bin/sh -c echo cursor-agent versions tar"
        ));
        assert!(!is_update_process("node index.js --resume abc123"));
    }

    #[test]
    fn version_from_curl_url_extracts_between_lab_and_slash() {
        assert_eq!(
            version_from_curl_url(
                "curl -fSL -s https://downloads.cursor.com/lab/2026.08.04-aaa8809/darwin/arm64/agent-cli-package.tar.gz"
            ),
            Some("2026.08.04-aaa8809".to_string())
        );
        assert_eq!(version_from_curl_url("no marker here"), None);
    }

    #[test]
    fn install_dir_resolution_prefers_override() {
        let override_value = Some(std::ffi::OsString::from("/fixture-root/fake-cursor-agent"));
        let home = Some(std::ffi::OsString::from("/fixture-root/tester"));
        assert_eq!(
            resolve_install_dir(override_value.clone(), home.clone()),
            PathBuf::from("/fixture-root/fake-cursor-agent")
        );
        assert_eq!(
            resolve_install_dir(None, home),
            PathBuf::from("/fixture-root/tester/.local/share/cursor-agent")
        );
        assert_eq!(resolve_install_dir(None, None), PathBuf::from(""));
    }
}
