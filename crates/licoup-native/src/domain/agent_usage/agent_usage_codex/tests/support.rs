use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::super::constants::CACHE_DATABASE_PREFIX;

pub(super) fn install_v12_fixture_schema(connection: &rusqlite::Connection) {
    connection
        .execute_batch(
            "ALTER TABLE usage_files DROP COLUMN current_effort;
         ALTER TABLE usage_files DROP COLUMN current_fast;
         ALTER TABLE usage_files DROP COLUMN pending_context;
         ALTER TABLE usage_rows DROP COLUMN effort;
         ALTER TABLE usage_rows DROP COLUMN fast;
         ALTER TABLE usage_daily_models RENAME TO variant_models;
         CREATE TABLE usage_daily_models (
           root_key TEXT NOT NULL,day TEXT NOT NULL,model TEXT NOT NULL,
           prompt_tokens INTEGER NOT NULL,cached_input_tokens INTEGER NOT NULL,
           completion_tokens INTEGER NOT NULL,total_tokens INTEGER NOT NULL,
           PRIMARY KEY(root_key,day,model)
         );
         INSERT INTO usage_daily_models
           SELECT root_key,day,model,SUM(prompt_tokens),SUM(cached_input_tokens),
                  SUM(completion_tokens),SUM(total_tokens)
           FROM variant_models GROUP BY root_key,day,model;
         DROP TABLE variant_models;
         PRAGMA user_version=12;",
        )
        .unwrap();
}

pub(super) fn temp_dir(name: &str) -> PathBuf {
    // The system clock can repeat between two rapid calls on the same thread
    // pair; the process-local sequence keeps parallel tests on disjoint roots.
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let dir = std::env::temp_dir().join(format!(
        "lico-codex-usage-{name}-{}-{}-{}-{}",
        std::process::id(),
        now.as_secs(),
        now.subsec_nanos(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

pub(super) fn scan_params(history_root: &Path, state_root: &Path) -> Value {
    let _ = crate::domain::agent_usage::host::install();
    json!({
        "agent": "codex",
        "root": history_root.to_string_lossy(),
        "stateRoot": state_root.to_string_lossy(),
        "forceRefresh": true,
        "historyDays": 30,
        "now": "2026-07-10T12:00:00Z"
    })
}

pub(super) fn codex_database_path(state_root: &Path) -> PathBuf {
    let database_prefix = format!("{CACHE_DATABASE_PREFIX}-");
    fs::read_dir(state_root)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.extension().and_then(|value| value.to_str()) == Some("sqlite3")
                && path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.starts_with(&database_prefix))
        })
        .unwrap()
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
                    "output_tokens": total.2
                },
                "last_token_usage": {
                    "input_tokens": last.0,
                    "cached_input_tokens": last.1,
                    "output_tokens": last.2
                }
            }
        }
    })
    .to_string()
}
