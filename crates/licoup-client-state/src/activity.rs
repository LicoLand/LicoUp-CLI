use anyhow::{Result, ensure};
use licoup_foundation::platform::file_security::{
    ensure_private_dir, open_private_text_bounded, validate_private_file_unchanged,
};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};

use super::{paths, policy, redaction, serialization};

#[derive(Clone, Debug)]
pub struct ActivityLog {
    path: PathBuf,
}

impl ActivityLog {
    pub fn portable() -> Result<Self> {
        let root = paths::portable_state_root()?;
        ensure_private_dir(&root)?;
        ensure_private_dir(&paths::activity_root(&root))?;
        Ok(Self::from_state_root(&root))
    }

    pub fn from_state_root(root: &Path) -> Self {
        Self {
            path: paths::activity_path(root),
        }
    }

    /// The private activity log under one explicit portable data root.
    ///
    /// A durable owner that already holds its exact root (for example the
    /// strategy store's run drive) records diagnostics there instead of
    /// consulting the ambient process data root.
    pub fn in_data_root(data_root: &Path) -> Result<Self> {
        let root = paths::state_root_for_data_home(data_root);
        ensure_private_dir(&root)?;
        ensure_private_dir(&paths::activity_root(&root))?;
        Ok(Self::from_state_root(&root))
    }

    pub fn append(&self, event_type: &str, payload: Value) -> Result<Value> {
        validate_event_type(event_type)?;
        let payload = redaction::redact_activity_payload(payload)?;
        let target = safe_target(&payload).to_string();
        let created_at = serialization::timestamp();
        let event = json!({
            "schemaVersion": policy::STATE_SCHEMA_VERSION,
            "eventId": format!("activity-{created_at}"),
            "type": event_type,
            "target": target,
            "createdAt": created_at,
            "payload": payload,
        });
        let encoded = serde_json::to_string(&event)?;
        ensure!(
            encoded.len() <= policy::MAX_ACTIVITY_EVENT_BYTES,
            "activity event exceeds its bounded size"
        );
        licoup_foundation::platform::file_security::append_private_line(&self.path, &encoded)?;
        Ok(event)
    }

    pub fn list(&self, filter: &Value) -> Result<Value> {
        let limit = filter
            .get("limit")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(policy::MAX_ACTIVITY_EVENTS)
            .min(policy::MAX_ACTIVITY_EVENTS);
        let mut events = VecDeque::<Value>::with_capacity(limit.min(256));
        #[cfg(test)]
        let mut validated_lines = 0usize;
        #[cfg(test)]
        let mut peak_retained = 0usize;
        #[cfg(test)]
        let mut peak_line_buffer_bytes = 0usize;
        let Some(mut reader) =
            open_private_text_bounded(&self.path, policy::MAX_ACTIVITY_FILE_BYTES)?
        else {
            return Ok(list_result(&self.path, events));
        };
        let opened = reader.get_ref().metadata()?;
        let mut buffer = String::new();
        loop {
            buffer.clear();
            // Include room for CRLF while bounding a hostile unterminated
            // record before `read_line` can grow the buffer to the file size.
            let read = Read::by_ref(&mut reader)
                .take((policy::MAX_ACTIVITY_EVENT_BYTES as u64).saturating_add(2))
                .read_line(&mut buffer)?;
            if read == 0 {
                break;
            }
            #[cfg(test)]
            {
                peak_line_buffer_bytes = peak_line_buffer_bytes.max(buffer.len());
            }
            let line = match buffer.strip_suffix('\n') {
                Some(without_newline) => without_newline
                    .strip_suffix('\r')
                    .unwrap_or(without_newline),
                None => buffer.as_str(),
            };
            if line.trim().is_empty() {
                continue;
            }
            ensure!(
                line.len() <= policy::MAX_ACTIVITY_EVENT_BYTES,
                "activity event exceeds its bounded size"
            );
            #[cfg(test)]
            {
                validated_lines += 1;
            }
            let event: Value = serde_json::from_str(line)?;
            if matches_activity_filter(&event, filter) && limit > 0 {
                if events.len() == limit {
                    events.pop_front();
                }
                events.push_back(event);
                #[cfg(test)]
                {
                    peak_retained = peak_retained.max(events.len());
                }
            }
        }
        validate_private_file_unchanged(&self.path, &opened)?;
        #[cfg(test)]
        retention_probe::observe(validated_lines, peak_retained, peak_line_buffer_bytes);
        Ok(list_result(&self.path, events))
    }
}

fn list_result(path: &Path, events: VecDeque<Value>) -> Value {
    json!({
        "ok": true,
        "schemaVersion": policy::STATE_SCHEMA_VERSION,
        "path": paths::internal_state_reference(policy::ACTIVITY_DIR, path),
        "events": events.into_iter().collect::<Vec<_>>()
    })
}

#[cfg(test)]
pub(super) mod retention_probe {
    use std::cell::Cell;

    #[derive(Clone, Copy, Debug, Default)]
    pub struct Snapshot {
        pub validated_lines: usize,
        pub peak_retained: usize,
        pub peak_line_buffer_bytes: usize,
    }

    thread_local! {
        static SNAPSHOT: Cell<Snapshot> = Cell::new(Snapshot::default());
    }

    pub fn reset() {
        SNAPSHOT.set(Snapshot::default());
    }

    pub fn observe(validated_lines: usize, peak_retained: usize, peak_line_buffer_bytes: usize) {
        SNAPSHOT.set(Snapshot {
            validated_lines,
            peak_retained,
            peak_line_buffer_bytes,
        });
    }

    pub fn snapshot() -> Snapshot {
        SNAPSHOT.with(|snapshot| snapshot.get())
    }
}

fn validate_event_type(event_type: &str) -> Result<()> {
    ensure!(
        !event_type.is_empty()
            && event_type.len() <= policy::MAX_ACTIVITY_TYPE_BYTES
            && event_type.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-' || byte == b'_'
            }),
        "activity event type is invalid"
    );
    Ok(())
}

fn safe_target(payload: &Value) -> &str {
    payload
        .get("target")
        .and_then(Value::as_str)
        .filter(|target| {
            !target.is_empty()
                && target.len() <= 128
                && target.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric()
                        || byte == b'.'
                        || byte == b'-'
                        || byte == b'_'
                        || byte == b':'
                })
        })
        .unwrap_or("")
}

fn matches_activity_filter(event: &Value, filter: &Value) -> bool {
    let type_matches = filter
        .get("type")
        .and_then(Value::as_str)
        .map(|expected| event.get("type").and_then(Value::as_str) == Some(expected))
        .unwrap_or(true);
    let target_matches = filter
        .get("target")
        .and_then(Value::as_str)
        .map(|expected| event.get("target").and_then(Value::as_str) == Some(expected))
        .unwrap_or(true);
    type_matches && target_matches
}
