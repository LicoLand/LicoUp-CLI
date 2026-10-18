//! Public `resource-usage scan` orchestration.

use super::matcher::match_processes_to_targets;
use super::process_snapshot::{ProcessSnapshot, current_process_snapshots, snapshots_from_params};
use crate::domain::targets;
use anyhow::Result;
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};

pub const RESOURCE_USAGE_SCHEMA_VERSION: u64 = 1;

/// Scans running agent processes and reports their resource usage.
#[cfg_attr(not(test), allow(unused_imports))]
pub fn scan(params: &Value) -> Result<Value> {
    let target_scan =
        targets::scan_targets_with_params(&crate::target_port::agent_target_port(), params)?;
    let candidates = injected_targets(params).unwrap_or_else(|| {
        target_scan
            .get("candidates")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    });
    let mut matched = Vec::new();
    for candidate in candidates {
        let Some(target) = candidate.get("target").and_then(Value::as_str) else {
            continue;
        };
        let label = candidate
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or(target)
            .to_string();
        let binary_path = candidate
            .get("binaryPath")
            .and_then(Value::as_str)
            .map(str::to_string);
        matched.push((target.to_string(), label, binary_path));
    }

    let snapshots = snapshots_from_params(params).unwrap_or_else(current_process_snapshots);
    let agents = match_processes_to_targets(&matched, &snapshots);

    let agent_values = agents
        .into_iter()
        .map(|agent| {
            let processes = agent
                .processes
                .iter()
                .map(|process| process_to_json(process))
                .collect::<Vec<_>>();
            json!({
                "target": agent.target,
                "label": agent.label,
                "running": !processes.is_empty(),
                "processes": processes,
                "totalRssBytes": agent.total_rss_bytes(),
                "totalDiskReadBytes": agent.total_disk_read_bytes(),
                "totalDiskWriteBytes": agent.total_disk_write_bytes(),
            })
        })
        .collect::<Vec<_>>();

    let total_rss: u64 = agent_values
        .iter()
        .filter_map(|agent| agent.get("totalRssBytes").and_then(Value::as_u64))
        .sum();

    Ok(json!({
        "ok": true,
        "schemaVersion": RESOURCE_USAGE_SCHEMA_VERSION,
        "generatedAt": timestamp_rfc3339(),
        "agents": agent_values,
        "summary": {
            "agentCount": agent_values.len(),
            "runningAgentCount": agent_values
                .iter()
                .filter(|agent| agent.get("running") == Some(&Value::Bool(true)))
                .count(),
            "totalRssBytes": total_rss
        }
    }))
}

/// Injected target candidates for deterministic tests. Returns None when
/// absent so production scans use the live target discovery.
fn injected_targets(params: &Value) -> Option<Vec<Value>> {
    let value = params.get("targetsJson")?;
    let value = value.as_str()?;
    serde_json::from_str(value).ok()
}

fn process_to_json(process: &ProcessSnapshot) -> Value {
    let mut value = json!({
        "pid": process.pid,
        "name": process.name,
        "rssBytes": process.rss_bytes,
    });
    if let Some(read) = process.disk_read_bytes {
        value["diskReadBytes"] = json!(read);
    }
    if let Some(write) = process.disk_write_bytes {
        value["diskWriteBytes"] = json!(write);
    }
    value
}

fn timestamp_rfc3339() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = duration.as_secs() as i64;
    let nanos = duration.subsec_nanos();
    // RFC3339 without external time formatting machinery.
    format!("{seconds}.{nanos:09}Z")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn injected_targets_json() -> String {
        serde_json::to_string(&[
            json!({"target": "codex", "label": "Codex", "binaryPath": null}),
            json!({"target": "cursor", "label": "Cursor", "binaryPath": null}),
            json!({"target": "openclaw", "label": "OpenClaw", "binaryPath": null}),
        ])
        .unwrap()
    }

    fn snapshot(pid: i64, name: &str, rss: u64) -> Value {
        json!({
            "pid": pid,
            "name": name,
            "rssBytes": rss,
            "diskReadBytes": 1000,
            "diskWriteBytes": 2000,
        })
    }

    #[test]
    fn scan_reports_running_and_idle_agents_from_injected_snapshots() {
        let params = json!({
            "targetsJson": injected_targets_json(),
            "processSnapshotJson": serde_json::to_string(&[
                snapshot(10, "codex", 4096),
                snapshot(20, "Finder", 512),
            ]).unwrap(),
        });
        let report = scan(&params).unwrap();
        assert_eq!(report["ok"], true);
        assert_eq!(report["schemaVersion"], 1);
        let agents = report["agents"].as_array().unwrap();
        assert_eq!(agents.len(), 3);
        let codex = agents.iter().find(|a| a["target"] == "codex").unwrap();
        assert_eq!(codex["running"], true);
        assert_eq!(codex["totalRssBytes"], 4096);
        assert_eq!(codex["totalDiskReadBytes"], 1000);
        assert_eq!(codex["totalDiskWriteBytes"], 2000);
        assert_eq!(codex["processes"][0]["pid"], 10);
        let openclaw = agents.iter().find(|a| a["target"] == "openclaw").unwrap();
        assert_eq!(openclaw["running"], false);
        assert_eq!(openclaw["totalRssBytes"], 0);
        assert_eq!(openclaw["totalDiskReadBytes"], Value::Null);
        assert_eq!(report["summary"]["runningAgentCount"], 1);
        assert_eq!(report["summary"]["totalRssBytes"], 4096);
    }

    #[test]
    fn scan_supports_none_io_counters() {
        let params = json!({
            "targetsJson": injected_targets_json(),
            "processSnapshotJson": serde_json::to_string(&[
                json!({"pid": 1, "name": "codex", "rssBytes": 10})
            ]).unwrap(),
        });
        let report = scan(&params).unwrap();
        let codex = report["agents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["target"] == "codex")
            .unwrap();
        assert_eq!(codex["totalDiskReadBytes"], Value::Null);
        assert_eq!(codex["totalDiskWriteBytes"], Value::Null);
    }
}
