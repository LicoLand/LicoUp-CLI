//! Process snapshot enumeration and per-process resource sampling.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

/// One running process with sampled resource counters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessSnapshot {
    pub pid: i64,
    pub name: String,
    /// Resident set size in bytes.
    pub rss_bytes: u64,
    /// Cumulative disk bytes read since process start. None on platforms
    /// that cannot report process-level disk I/O (Windows).
    pub disk_read_bytes: Option<u64>,
    /// Cumulative disk bytes written since process start. None on platforms
    /// that cannot report process-level disk I/O (Windows).
    pub disk_write_bytes: Option<u64>,
}

/// Enumerates running processes with sampled resource counters.
pub fn current_process_snapshots() -> Vec<ProcessSnapshot> {
    #[cfg(target_os = "macos")]
    {
        macos_process_snapshots()
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        linux_process_snapshots()
    }
    #[cfg(windows)]
    {
        windows_process_snapshots()
    }
    #[cfg(not(any(target_os = "macos", windows, all(unix, not(target_os = "macos")))))]
    {
        Vec::new()
    }
}

/// Parses a `ps -axo pid=,rss=,comm=` payload into rss-byte entries.
/// Exposed for tests.
#[cfg(any(test, target_os = "macos"))]
pub fn parse_ps_rss_lines(text: &str) -> Vec<(i64, u64, String)> {
    let mut entries = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let (Some(pid_text), Some(rss_text), Some(name)) =
            (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let (Ok(pid), Ok(rss_kb)) = (pid_text.parse::<i64>(), rss_text.parse::<u64>()) else {
            continue;
        };
        entries.push((pid, rss_kb.saturating_mul(1024), name.to_string()));
    }
    entries
}

/// Parses a `/proc/<pid>/io` payload. Exposed for tests.
#[cfg(any(test, all(unix, not(target_os = "macos"))))]
pub fn parse_proc_io(contents: &str) -> (Option<u64>, Option<u64>) {
    let mut read = None;
    let mut write = None;
    for line in contents.lines() {
        let Some(colon) = line.find(':') else {
            continue;
        };
        let key = line[..colon].trim();
        let value = line[colon + 1..].trim();
        let Ok(parsed) = value.parse::<u64>() else {
            continue;
        };
        match key {
            "read_bytes" => read = Some(parsed),
            "write_bytes" => write = Some(parsed),
            _ => {}
        }
    }
    (read, write)
}

/// Parses a `VmRSS:` line from `/proc/<pid>/status`. Exposed for tests.
#[cfg(any(test, all(unix, not(target_os = "macos"))))]
pub fn parse_vmrss_kb(contents: &str) -> Option<u64> {
    for line in contents.lines() {
        let Some(colon) = line.find(':') else {
            continue;
        };
        if line[..colon].trim() != "VmRSS" {
            continue;
        }
        let value = line[colon + 1..].trim();
        let kb = value
            .split_whitespace()
            .next()
            .and_then(|text| text.parse::<u64>().ok())?;
        return Some(kb.saturating_mul(1024));
    }
    None
}

/// Normalizes a process name for matching against target process names.
pub fn normalize_process_name(value: &str) -> String {
    let stem = value
        .trim()
        .trim_end_matches(".exe")
        .trim_start_matches('"')
        .trim_end_matches('"');
    stem.to_ascii_lowercase()
}

/// Whether the snapshot name matches one of the target's process names or
/// the basename of its resolved binary path.
pub fn process_matches_target(
    snapshot: &ProcessSnapshot,
    process_names: &[&str],
    binary_path: Option<&str>,
) -> bool {
    let name = normalize_process_name(&snapshot.name);
    if process_names
        .iter()
        .map(|candidate| normalize_process_name(candidate))
        .any(|candidate| candidate == name)
    {
        return true;
    }
    binary_path.is_some_and(|path| {
        let basename = Path::new(path)
            .file_name()
            .map(|value| value.to_string_lossy())
            .unwrap_or_default();
        !basename.is_empty() && normalize_process_name(&basename) == name
    })
}

#[cfg(target_os = "macos")]
fn macos_process_snapshots() -> Vec<ProcessSnapshot> {
    use std::process::Command;

    let Ok(output) = Command::new("ps")
        .args(["-axo", "pid=,rss=,comm="])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut snapshots = Vec::new();
    for (pid, rss_bytes, name) in parse_ps_rss_lines(&text) {
        let (disk_read, disk_write) = macos_rusage_disk_bytes(pid);
        snapshots.push(ProcessSnapshot {
            pid,
            name,
            rss_bytes,
            disk_read_bytes: disk_read,
            disk_write_bytes: disk_write,
        });
    }
    snapshots
}

#[cfg(target_os = "macos")]
fn macos_rusage_disk_bytes(pid: i64) -> (Option<u64>, Option<u64>) {
    const RUSAGE_INFO_V4: i32 = 4;
    let mut info = unsafe { std::mem::zeroed::<libc::rusage_info_v4>() };
    let buffer = (&mut info as *mut libc::rusage_info_v4).cast::<libc::rusage_info_t>();
    let result = unsafe { libc::proc_pid_rusage(pid as libc::c_int, RUSAGE_INFO_V4, buffer) };
    if result != 0 {
        return (None, None);
    }
    (
        Some(info.ri_diskio_bytesread as u64),
        Some(info.ri_diskio_byteswritten as u64),
    )
}

#[cfg(all(unix, not(target_os = "macos")))]
fn linux_process_snapshots() -> Vec<ProcessSnapshot> {
    use std::fs;

    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut snapshots = Vec::new();
    for entry in entries.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i64>() else {
            continue;
        };
        let root = entry.path();
        let name = fs::read_to_string(root.join("comm"))
            .ok()
            .map(|text| text.trim().to_string())
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        let rss_bytes = fs::read_to_string(root.join("status"))
            .ok()
            .and_then(|status| parse_vmrss_kb(&status))
            .unwrap_or(0);
        let (disk_read, disk_write) = fs::read_to_string(root.join("io"))
            .ok()
            .map(|io| parse_proc_io(&io))
            .unwrap_or((None, None));
        snapshots.push(ProcessSnapshot {
            pid,
            name,
            rss_bytes,
            disk_read_bytes: disk_read,
            disk_write_bytes: disk_write,
        });
    }
    snapshots
}

#[cfg(windows)]
fn windows_process_snapshots() -> Vec<ProcessSnapshot> {
    use std::process::Command;

    let Ok(output) = Command::new("tasklist")
        .args(["/fo", "csv", "/nh"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut snapshots = Vec::new();
    for line in text.lines() {
        let Some((name, pid, mem)) = parse_tasklist_line(line) else {
            continue;
        };
        snapshots.push(ProcessSnapshot {
            pid,
            name,
            rss_bytes: mem,
            disk_read_bytes: None,
            disk_write_bytes: None,
        });
    }
    snapshots
}

#[cfg(windows)]
fn parse_tasklist_line(line: &str) -> Option<(String, i64, u64)> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in line.trim().chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                fields.push(std::mem::take(&mut current));
            }
            _ => current.push(ch),
        }
    }
    fields.push(current);
    let name = fields.first()?.trim().to_string();
    if name.is_empty() {
        return None;
    }
    let pid = fields.get(1)?.trim().parse::<i64>().ok()?;
    // "Mem Usage" column, e.g. "123,456 K" (US locale) or "123456 K".
    let empty = String::new();
    let mem_text = fields.get(4).unwrap_or(&empty);
    let normalized = mem_text.replace(',', "");
    let kb = normalized
        .trim()
        .trim_end_matches([' ', 'K', 'k'])
        .trim()
        .parse::<u64>()
        .ok()?;
    Some((name, pid, kb.saturating_mul(1024)))
}

/// Serializes snapshots as a JSON array, used for injected test snapshots.
#[cfg(test)]
pub fn snapshots_to_json(snapshots: &[ProcessSnapshot]) -> Value {
    serde_json::to_value(snapshots).unwrap_or(Value::Array(Vec::new()))
}

/// Parses injected snapshots from params. Returns None when absent.
pub fn snapshots_from_params(params: &Value) -> Option<Vec<ProcessSnapshot>> {
    let value = params.get("processSnapshotJson")?;
    let value = value.as_str()?;
    serde_json::from_str(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_rss_lines_parse_kb_into_bytes() {
        let entries = parse_ps_rss_lines("  123   4567 codex\n  9  100 app\n");
        assert_eq!(
            entries,
            vec![
                (123, 4567 * 1024, "codex".to_string()),
                (9, 100 * 1024, "app".to_string())
            ]
        );
    }

    #[test]
    fn ps_rss_lines_skip_malformed() {
        assert_eq!(parse_ps_rss_lines("abc def ghi\n  1  2\n"), Vec::new());
    }

    #[test]
    fn proc_io_parses_read_and_write_bytes() {
        let (read, write) = parse_proc_io("read_bytes: 5120\nwrite_bytes: 6144\nrchar: 1\n");
        assert_eq!(read, Some(5120));
        assert_eq!(write, Some(6144));
    }

    #[test]
    fn proc_io_missing_counters_are_none() {
        let (read, write) = parse_proc_io("rchar: 1\n");
        assert_eq!(read, None);
        assert_eq!(write, None);
    }

    #[test]
    fn vmrss_parses_kb_into_bytes() {
        assert_eq!(parse_vmrss_kb("VmRSS:\t      512 kB\n"), Some(512 * 1024));
        assert_eq!(parse_vmrss_kb("VmSize: 100 kB\n"), None);
    }

    #[test]
    fn process_matching_uses_names_and_binary_basename() {
        let snapshot = ProcessSnapshot {
            pid: 1,
            name: "Codex".to_string(),
            rss_bytes: 0,
            disk_read_bytes: None,
            disk_write_bytes: None,
        };
        assert!(process_matches_target(&snapshot, &["codex"], None));
        assert!(!process_matches_target(&snapshot, &["cursor"], None));
        let path_snapshot = ProcessSnapshot {
            pid: 2,
            name: "cursor".to_string(),
            rss_bytes: 0,
            disk_read_bytes: None,
            disk_write_bytes: None,
        };
        assert!(process_matches_target(
            &path_snapshot,
            &[],
            Some("/Applications/Cursor.app/Contents/MacOS/Cursor")
        ));
    }

    #[test]
    fn windows_tasklist_line_parses_mem_usage_kb() {
        #[cfg(windows)]
        {
            let (name, pid, mem) =
                parse_tasklist_line("\"Cursor.exe\",\"1234\",\"Console\",\"1\",\"12,345 K\"")
                    .unwrap();
            assert_eq!(name, "Cursor.exe");
            assert_eq!(pid, 1234);
            assert_eq!(mem, 12_345 * 1024);
        }
    }

    #[test]
    fn injected_snapshots_round_trip_through_json() {
        let snapshots = vec![ProcessSnapshot {
            pid: 42,
            name: "codex".to_string(),
            rss_bytes: 1024,
            disk_read_bytes: Some(10),
            disk_write_bytes: None,
        }];
        let json = snapshots_to_json(&snapshots);
        let parsed = serde_json::from_value::<Vec<ProcessSnapshot>>(json).unwrap();
        assert_eq!(parsed, snapshots);
    }
}
