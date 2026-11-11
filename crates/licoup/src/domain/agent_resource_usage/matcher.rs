//! Pure matching of process snapshots to discovered agent targets.

use super::process_snapshot::{ProcessSnapshot, process_matches_target};

/// One agent's matched running processes with aggregate counters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentProcesses {
    pub target: String,
    pub label: String,
    pub processes: Vec<ProcessSnapshot>,
}

impl AgentProcesses {
    pub fn total_rss_bytes(&self) -> u64 {
        self.processes.iter().map(|p| p.rss_bytes).sum()
    }

    pub fn total_disk_read_bytes(&self) -> Option<u64> {
        aggregate_optional(self.processes.iter().map(|p| p.disk_read_bytes))
    }

    pub fn total_disk_write_bytes(&self) -> Option<u64> {
        aggregate_optional(self.processes.iter().map(|p| p.disk_write_bytes))
    }
}

fn aggregate_optional<I>(values: I) -> Option<u64>
where
    I: Iterator<Item = Option<u64>>,
{
    let mut sum = 0u64;
    let mut saw_value = false;
    for value in values {
        let Some(value) = value else {
            return None;
        };
        saw_value = true;
        sum = sum.saturating_add(value);
    }
    if saw_value { Some(sum) } else { None }
}

/// Matches every running process against every target definition.
///
/// A process may match at most one target; first match wins. Processes that
/// match no target are ignored. Targets with no matched process are reported
/// with an empty process list so the caller can mark them as not running.
pub fn match_processes_to_targets(
    targets: &[(String, String, Option<String>)],
    snapshots: &[ProcessSnapshot],
) -> Vec<AgentProcesses> {
    let mut used = vec![false; snapshots.len()];
    let mut agents = Vec::with_capacity(targets.len());
    for (target, label, binary_path) in targets {
        let mut processes = Vec::new();
        for (index, snapshot) in snapshots.iter().enumerate() {
            if used[index] {
                continue;
            }
            let names: &[&str] = target_process_names(target);
            if process_matches_target(snapshot, names, binary_path.as_deref()) {
                processes.push(snapshot.clone());
                used[index] = true;
            }
        }
        processes.sort_by_key(|p| p.pid);
        agents.push(AgentProcesses {
            target: target.clone(),
            label: label.clone(),
            processes,
        });
    }
    agents
}

/// Canonical process-name candidates per target id. Kept in sync with
/// `targets::catalog::target_defs`.
fn target_process_names(target: &str) -> &'static [&'static str] {
    match target {
        "openclaw" => &["openclaw", "openclaw.exe"],
        "claude-code" => &["claude", "claude.exe"],
        "codex" => &["codex", "codex.exe"],
        "cursor" => &["Cursor", "Cursor.exe"],
        "kimi-code" => &["kimi", "kimi.exe"],
        "grok" => &["grok", "grok.exe", "xai-grok-pager", "xai-grok-pager.exe"],
        "command-code" => &["command-code", "command-code.exe", "cmdc", "cmdc.exe"],
        "kilo-code" => &["kilo", "kilo-code", "kilo-code.exe"],
        "pi" => &["pi", "pi.exe"],
        "code" => &["code", "code.exe"],
        "opencode" => &["opencode", "opencode.exe"],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(pid: i64, name: &str, rss: u64) -> ProcessSnapshot {
        ProcessSnapshot {
            pid,
            name: name.to_string(),
            rss_bytes: rss,
            disk_read_bytes: Some(0),
            disk_write_bytes: Some(0),
        }
    }

    #[test]
    fn matches_running_processes_and_marks_missing_agents() {
        let targets = vec![
            ("codex".to_string(), "Codex".to_string(), None),
            ("cursor".to_string(), "Cursor".to_string(), None),
            ("openclaw".to_string(), "OpenClaw".to_string(), None),
        ];
        let snapshots = vec![
            snapshot(10, "codex", 100),
            snapshot(20, "Cursor", 200),
            snapshot(30, "codex", 50),
        ];
        let agents = match_processes_to_targets(&targets, &snapshots);
        assert_eq!(agents.len(), 3);
        let codex = agents.iter().find(|a| a.target == "codex").unwrap();
        assert_eq!(codex.processes.len(), 2);
        assert_eq!(codex.total_rss_bytes(), 150);
        let openclaw = agents.iter().find(|a| a.target == "openclaw").unwrap();
        assert!(openclaw.processes.is_empty());
        assert_eq!(openclaw.total_rss_bytes(), 0);
    }

    #[test]
    fn first_target_claim_wins_for_shared_process_names() {
        let targets = vec![
            ("codex".to_string(), "Codex".to_string(), None),
            (
                "custom".to_string(),
                "Custom".to_string(),
                Some("codex".to_string()),
            ),
        ];
        let snapshots = vec![snapshot(1, "codex", 10)];
        let agents = match_processes_to_targets(&targets, &snapshots);
        let codex = agents.iter().find(|a| a.target == "codex").unwrap();
        let custom = agents.iter().find(|a| a.target == "custom").unwrap();
        assert_eq!(codex.processes.len(), 1);
        assert!(custom.processes.is_empty());
    }

    #[test]
    fn unmatched_processes_are_ignored() {
        let targets = vec![("codex".to_string(), "Codex".to_string(), None)];
        let snapshots = vec![snapshot(1, "Finder", 10), snapshot(2, "codex", 20)];
        let agents = match_processes_to_targets(&targets, &snapshots);
        assert_eq!(agents[0].processes.len(), 1);
        assert_eq!(agents[0].processes[0].pid, 2);
    }

    #[test]
    fn aggregate_optional_is_none_when_any_process_lacks_io_counters() {
        let agent = AgentProcesses {
            target: "codex".to_string(),
            label: "Codex".to_string(),
            processes: vec![
                ProcessSnapshot {
                    pid: 1,
                    name: "codex".to_string(),
                    rss_bytes: 0,
                    disk_read_bytes: Some(1),
                    disk_write_bytes: None,
                },
                ProcessSnapshot {
                    pid: 2,
                    name: "codex".to_string(),
                    rss_bytes: 0,
                    disk_read_bytes: Some(2),
                    disk_write_bytes: None,
                },
            ],
        };
        assert_eq!(agent.total_disk_read_bytes(), Some(3));
        assert_eq!(agent.total_disk_write_bytes(), None);
    }
}
