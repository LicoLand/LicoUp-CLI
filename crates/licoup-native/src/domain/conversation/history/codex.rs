//! Codex rollout runtime observation and lineage helpers — persisted
//! decoding itself lives in `acop-codex` behind the store codec.

use super::*;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const CODEX_RUNTIME_TAIL_BYTES: u64 = 8 * 1024 * 1024;
pub(super) const CODEX_RUNTIME_SCAN_CHUNK_BYTES: u64 = 64 * 1024;

#[derive(Debug)]
pub(super) struct CodexRuntimeObservation {
    open_rollouts: BTreeSet<PathBuf>,
}

impl CodexRuntimeObservation {
    /// Capture this refresh's environment evidence through the installed port.
    pub(super) fn capture() -> Self {
        Self {
            open_rollouts: super::runtime_observation::port().open_rollouts(),
        }
    }

    pub(super) fn is_running(&self, path: &Path) -> bool {
        let normalized = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.open_rollouts.contains(&normalized) && codex_rollout_has_active_task(path)
    }
}

/// Codex persists task lifecycle facts in the rollout that owns the thread.
/// Scan backwards in small chunks and stop at the newest lifecycle record.
/// The byte window is explicit so a pathological transcript cannot turn a
/// sidebar refresh into an unbounded read; completed tasks normally resolve
/// from the first chunk at the end of the file.
pub(super) fn codex_rollout_has_active_task(path: &Path) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let Ok(metadata) = file.metadata() else {
        return false;
    };
    let scan_start = metadata.len().saturating_sub(CODEX_RUNTIME_TAIL_BYTES);
    let mut cursor = metadata.len();
    let mut later_line_prefix = Vec::<u8>::new();
    while cursor > scan_start {
        let chunk_start = cursor
            .saturating_sub(CODEX_RUNTIME_SCAN_CHUNK_BYTES)
            .max(scan_start);
        let chunk_len = (cursor - chunk_start) as usize;
        if file.seek(SeekFrom::Start(chunk_start)).is_err() {
            return false;
        }
        let mut chunk = vec![0; chunk_len];
        if file.read_exact(&mut chunk).is_err() {
            return false;
        }
        chunk.extend_from_slice(&later_line_prefix);

        let complete_start = if chunk_start == 0 {
            0
        } else if let Some(first_newline) = chunk.iter().position(|byte| *byte == b'\n') {
            later_line_prefix = chunk[..first_newline].to_vec();
            first_newline + 1
        } else {
            later_line_prefix = chunk;
            cursor = chunk_start;
            continue;
        };
        if let Some(running) = newest_codex_task_lifecycle(&chunk[complete_start..]) {
            return running;
        }
        cursor = chunk_start;
    }
    false
}

fn newest_codex_task_lifecycle(lines: &[u8]) -> Option<bool> {
    for line in lines.rsplit(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("event_msg") {
            continue;
        }
        match value
            .get("payload")
            .and_then(|payload| payload.get("type"))
            .and_then(Value::as_str)
        {
            Some("task_started") => return Some(true),
            Some(
                "task_complete" | "task_cancelled" | "turn_aborted" | "turn_cancelled"
                | "turn_completed",
            ) => return Some(false),
            _ => {}
        }
    }
    None
}

pub(super) fn mark_codex_runtime_activity(
    sessions: &mut [Value],
    observation: &CodexRuntimeObservation,
) {
    for session in sessions {
        let running = session
            .get("sourcePath")
            .and_then(Value::as_str)
            .filter(|path| !path.trim().is_empty())
            .is_some_and(|path| observation.is_running(Path::new(path)));
        let Some(object) = session.as_object_mut() else {
            continue;
        };
        if running {
            object.insert("running".to_string(), json!(true));
        } else {
            object.remove("running");
        }
    }
}

pub(super) fn codex_rollout_lineage(
    payload: &Value,
    session_id: &str,
) -> (Option<String>, bool, Option<String>) {
    let parent_session_id = find_nested_string(
        payload,
        &[
            "forked_from_id",
            "forkedFromId",
            "parent_session_id",
            "parentSessionId",
            "parent_thread_id",
            "parentThreadId",
        ],
        0,
    )
    .filter(|parent_id| parent_id != session_id);
    let is_subagent = contains_nested_key(payload, "subagent", 0)
        || contains_nested_key(payload, "thread_spawn", 0)
        || contains_nested_key(payload, "threadSpawn", 0);
    let subagent_title = find_nested_string(
        payload,
        &["agent_nickname", "agentNickname", "agent_role", "agentRole"],
        0,
    );
    (parent_session_id, is_subagent, subagent_title)
}

pub(super) fn find_nested_string(value: &Value, keys: &[&str], _depth: usize) -> Option<String> {
    let mut pending = vec![value];
    while let Some(candidate) = pending.pop() {
        match candidate {
            Value::Object(object) => {
                for key in keys {
                    if let Some(text) = object.get(*key).and_then(Value::as_str) {
                        if !text.trim().is_empty() {
                            return Some(text.to_string());
                        }
                    }
                }
                pending.extend(object.values().rev());
            }
            Value::Array(items) => pending.extend(items.iter().rev()),
            _ => {}
        }
    }
    None
}

pub(super) fn contains_nested_key(value: &Value, needle: &str, _depth: usize) -> bool {
    let mut pending = vec![value];
    while let Some(candidate) = pending.pop() {
        match candidate {
            Value::Object(object) => {
                if object.contains_key(needle) {
                    return true;
                }
                pending.extend(object.values());
            }
            Value::Array(items) => pending.extend(items),
            _ => {}
        }
    }
    false
}

pub(super) fn rollout_session_id_from_filename(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let parts = stem.split('-').collect::<Vec<_>>();
    if parts.len() < 5 {
        return None;
    }
    for window in parts.windows(5) {
        let candidate = window.join("-");
        if looks_like_uuid(&candidate) {
            return Some(candidate);
        }
    }
    None
}

pub(super) fn looks_like_uuid(value: &str) -> bool {
    let parts = value.split('-').collect::<Vec<_>>();
    let lengths = [8usize, 4, 4, 4, 12];
    parts.len() == lengths.len()
        && parts.iter().zip(lengths).all(|(part, length)| {
            part.len() == length && part.chars().all(|ch| ch.is_ascii_hexdigit())
        })
}
