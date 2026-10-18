pub(super) use licoup_native::domain::agent_usage;
pub(super) use licoup_native::domain::conversations;
pub(super) use licoup_native::platform::client_state::ClientStateStore;
pub(super) use rusqlite::Connection as SqliteConnection;
pub(super) use serde_json::{Value, json};
pub(super) use std::fs;
pub(super) use std::io::Write;
pub(super) use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn find_explicit_usage(value: &Value) -> Option<&Value> {
    match value {
        Value::Object(object) => {
            if object.contains_key("promptTokens") && object.contains_key("totalTokens") {
                return Some(value);
            }
            object.values().find_map(find_explicit_usage)
        }
        Value::Array(values) => values.iter().find_map(find_explicit_usage),
        _ => None,
    }
}

pub(super) fn explicit_usages(value: &Value) -> Vec<&Value> {
    let mut usages = Vec::new();
    collect_explicit_usages(value, &mut usages);
    usages
}

fn collect_explicit_usages<'a>(value: &'a Value, usages: &mut Vec<&'a Value>) {
    match value {
        Value::Object(object) => {
            if object.contains_key("promptTokens") && object.contains_key("totalTokens") {
                usages.push(value);
                return;
            }
            for child in object.values() {
                collect_explicit_usages(child, usages);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_explicit_usages(child, usages);
            }
        }
        _ => {}
    }
}

pub(super) fn scan_params(history_root: &PathBuf, state_root: &PathBuf) -> Value {
    json!({
        "agent": "codex",
        "root": history_root.to_string_lossy(),
        "stateRoot": state_root.to_string_lossy(),
        "forceRefresh": true,
        "historyDays": 30,
        "now": "2026-07-10T12:00:00Z"
    })
}

pub(super) fn token_event(
    timestamp: &str,
    total: (u64, u64, u64),
    last: (u64, u64, u64),
) -> String {
    json!({
        "timestamp": timestamp,
        "type": "event_msg",
        "payload": {
            "type": "token_count",
            "info": {
                "total_token_usage": {
                    "input_tokens": total.0,
                    "cached_input_tokens": total.1,
                    "output_tokens": total.2,
                    "reasoning_output_tokens": total.2 / 2,
                    "total_tokens": total.0 + total.2
                },
                "last_token_usage": {
                    "input_tokens": last.0,
                    "cached_input_tokens": last.1,
                    "output_tokens": last.2,
                    "reasoning_output_tokens": last.2 / 2,
                    "total_tokens": last.0 + last.2
                }
            }
        }
    })
    .to_string()
}

pub(super) fn temp_dir(prefix: &str) -> PathBuf {
    // The facade's scan/report are the statistics engine's entries; every case
    // below composes the product host the way `install_environment_ports` does
    // rather than inventing an empty catalog. A repeat install is refused, so
    // the first case in the process installs and the rest reuse it.
    let _ = agent_usage::host::install();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("{prefix}-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    path
}
