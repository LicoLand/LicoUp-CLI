use super::models::SourceMetadata;
use crate::domain::conversation::source_catalog::{HistoryAdapter, HistoryRoot};
use anyhow::{Context, Result};
use licoup_statistics::window::UsageWindow;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Take};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const HASH_BUFFER_BYTES: usize = 64 * 1024;

pub(super) fn usage_roots(adapter: HistoryAdapter, roots: Vec<HistoryRoot>) -> Vec<HistoryRoot> {
    let mut seen = BTreeSet::<PathBuf>::new();
    roots
        .into_iter()
        .filter_map(|mut root| {
            if adapter == HistoryAdapter::Antigravity {
                if matches!(
                    root.source_kind.as_str(),
                    "antigravity-bridge" | "antigravity-cli"
                ) && root.path.file_name().and_then(|value| value.to_str()) != Some("brain")
                {
                    root.path = root.path.join("brain");
                }
            }
            seen.insert(root.path.clone()).then_some(root)
        })
        .collect()
}

pub(super) fn is_usage_source(adapter: HistoryAdapter, path: &Path, source_kind: &str) -> bool {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match adapter {
        HistoryAdapter::DeepSeekHarness => {
            crate::domain::conversation::source_catalog::deepseek_generation(path).is_some()
        }
        HistoryAdapter::Antigravity
            if matches!(source_kind, "antigravity-bridge" | "antigravity-cli") =>
        {
            (is_append_format(path) && path_component_pair(path, ".system_generated", "logs"))
                || is_snapshot_format(path)
        }
        HistoryAdapter::Antigravity if source_kind == "antigravity-ide-state" => {
            matches!(file_name.as_str(), "state.vscdb" | "store.db")
        }
        HistoryAdapter::Cursor
            if matches!(source_kind, "cursor-cli-chats" | "cursor-cli-projects") =>
        {
            is_append_format(path) || is_snapshot_format(path)
        }
        HistoryAdapter::Cursor if source_kind.starts_with("cursor-") => {
            matches!(file_name.as_str(), "state.vscdb" | "store.db")
        }
        HistoryAdapter::Copilot if source_kind == "copilot-cli-session-store" => {
            is_append_format(path)
        }
        HistoryAdapter::Copilot if source_kind.starts_with("vscode-copilot-") => {
            matches!(file_name.as_str(), "state.vscdb" | "store.db")
        }
        HistoryAdapter::Hermes => file_name == "state.db",
        HistoryAdapter::Pi if source_kind == "pi-session-store" => is_append_format(path),
        HistoryAdapter::LicoAgent if source_kind == "lico-agent-session-store" => {
            is_append_format(path)
        }
        _ => is_append_format(path) || is_snapshot_format(path),
    }
}

pub(super) fn source_metadata(path: &Path) -> Option<SourceMetadata> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return None;
    }
    let mut modified_ns = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos()
        .min(u64::MAX as u128) as u64;
    let mut size = metadata.len();
    if is_snapshot_database(path)
        && let Ok(wal_metadata) = fs::symlink_metadata(sqlite_sidecar(path, "-wal"))
        && wal_metadata.file_type().is_file()
        && !wal_metadata.file_type().is_symlink()
    {
        size = size.saturating_add(wal_metadata.len());
        if let Ok(wal_modified) = wal_metadata.modified()
            && let Ok(wal_elapsed) = wal_modified.duration_since(UNIX_EPOCH)
        {
            modified_ns = modified_ns.max(wal_elapsed.as_nanos().min(u64::MAX as u128) as u64);
        }
    }
    #[cfg(unix)]
    let file_id = Some(format!("{}:{}", metadata.dev(), metadata.ino()));
    #[cfg(windows)]
    let file_id =
        (metadata.creation_time() > 0).then(|| format!("windows:{}", metadata.creation_time()));
    #[cfg(not(any(unix, windows)))]
    let file_id = None;
    Some(SourceMetadata {
        modified_ns,
        size,
        file_id,
    })
}

fn is_snapshot_database(path: &Path) -> bool {
    matches!(
        extension(path).as_str(),
        "sqlite" | "sqlite3" | "db" | "vscdb"
    )
}

fn sqlite_sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(suffix);
    PathBuf::from(sidecar)
}

pub(super) fn roots_fingerprint(agent_id: &str, roots: &[PathBuf], timezone_key: &str) -> String {
    let mut normalized = roots
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();
    hash_text(&format!(
        "native-usage-v1\nagent={agent_id}\ntz={timezone_key}\n{}",
        normalized.join("\n")
    ))
}

pub(super) fn source_key(scope_key: &str, path: &Path) -> String {
    hash_text(&format!("{scope_key}\n{}", path.to_string_lossy()))
}

pub(super) fn append_guard(path: &Path, guarded_bytes: u64) -> Result<String> {
    let file = fs::File::open(path).context("native usage append guard open failed")?;
    if file.metadata()?.len() < guarded_bytes {
        anyhow::bail!("native usage append guard exceeds source length");
    }
    let mut limited = file.take(guarded_bytes);
    digest_reader(&mut limited)
}

pub(super) fn append_guard_matches(path: &Path, guarded_bytes: u64, expected: &str) -> bool {
    !expected.is_empty()
        && append_guard(path, guarded_bytes)
            .map(|observed| observed == expected)
            .unwrap_or(false)
}

pub(super) fn is_append_format(path: &Path) -> bool {
    matches!(extension(path).as_str(), "jsonl" | "ndjson" | "log")
}

pub(super) fn is_snapshot_format(path: &Path) -> bool {
    matches!(
        extension(path).as_str(),
        "json" | "sqlite" | "sqlite3" | "db" | "vscdb"
    )
}

pub(super) fn source_is_closed(metadata: &SourceMetadata, calendar: &UsageWindow) -> bool {
    let seconds = (metadata.modified_ns / 1_000_000_000).to_string();
    calendar
        .date_key(&seconds)
        .is_some_and(|day| day < calendar.end)
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn path_component_pair(path: &Path, first: &str, second: &str) -> bool {
    let components = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>();
    components
        .windows(2)
        .any(|window| window == [first, second])
}

fn digest_reader(reader: &mut Take<fs::File>) -> Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(b"lico-native-usage-source-v1\0");
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn hash_text(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use std::time::SystemTime;

    #[test]
    fn append_guard_accepts_only_an_unchanged_prefix() {
        let path = std::env::temp_dir().join(format!(
            "lico-native-usage-guard-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, b"first\n").unwrap();
        let guard = append_guard(&path, 6).unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"second\n")
            .unwrap();
        assert!(append_guard_matches(&path, 6, &guard));
        fs::write(&path, b"changed\nsecond\n").unwrap();
        assert!(!append_guard_matches(&path, 6, &guard));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn closed_source_uses_the_report_calendar() {
        let window = UsageWindow::from_params(&json!({
            "now": "2026-07-15T12:00:00Z"
        }));
        let timestamp = time::OffsetDateTime::parse(
            "2026-07-14T10:00:00Z",
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap();
        let metadata = SourceMetadata {
            modified_ns: timestamp.unix_timestamp_nanos() as u64,
            size: 1,
            file_id: None,
        };
        assert!(source_is_closed(&metadata, &window));
    }

    #[test]
    fn snapshot_metadata_tracks_uncheckpointed_sqlite_wal_bytes() {
        let root = std::env::temp_dir().join(format!(
            "lico-native-usage-wal-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let database = root.join("state.db");
        fs::write(&database, b"db").unwrap();
        let before = source_metadata(&database).unwrap();

        fs::write(sqlite_sidecar(&database, "-wal"), b"gateway-wal").unwrap();
        let after = source_metadata(&database).unwrap();
        assert_eq!(after.size, before.size + 11);

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn usage_sources_keep_native_metadata_and_bounded_estimation_sources() {
        assert!(is_usage_source(
            HistoryAdapter::Antigravity,
            Path::new("brain/id/.system_generated/logs/transcript.jsonl"),
            "antigravity-cli"
        ));
        assert!(is_usage_source(
            HistoryAdapter::Antigravity,
            Path::new("brain/id/conversation.json"),
            "antigravity-cli"
        ));
        assert!(is_usage_source(
            HistoryAdapter::Cursor,
            Path::new("workspace/state.vscdb"),
            "cursor-workspace-storage"
        ));
        assert!(!is_usage_source(
            HistoryAdapter::Cursor,
            Path::new("workspace/large.json"),
            "cursor-workspace-storage"
        ));
        assert!(is_usage_source(
            HistoryAdapter::Copilot,
            Path::new("session-state/id/events.jsonl"),
            "copilot-cli-session-store"
        ));
        assert!(is_usage_source(
            HistoryAdapter::Hermes,
            Path::new("profile/state.db"),
            "hermes-home"
        ));
        assert!(!is_usage_source(
            HistoryAdapter::Hermes,
            Path::new("profile/session.json"),
            "hermes-home"
        ));
    }

    #[test]
    fn usage_roots_keep_vendor_conversations_for_cached_fallback_estimation() {
        let roots = vec![
            HistoryRoot {
                path: PathBuf::from("copilot/session-state"),
                source_kind: "copilot-cli-session-store".to_owned(),
                explicitly_selected: false,
            },
            HistoryRoot {
                path: PathBuf::from("code/workspaceStorage"),
                source_kind: "vscode-copilot-workspace-storage".to_owned(),
                explicitly_selected: false,
            },
        ];
        let copilot = usage_roots(HistoryAdapter::Copilot, roots);
        assert_eq!(copilot.len(), 2);
        assert!(
            copilot
                .iter()
                .any(|root| root.source_kind == "vscode-copilot-workspace-storage")
        );

        let cursor = usage_roots(
            HistoryAdapter::Cursor,
            vec![
                HistoryRoot {
                    path: PathBuf::from("cursor/workspaceStorage"),
                    source_kind: "cursor-workspace-storage".to_owned(),
                    explicitly_selected: false,
                },
                HistoryRoot {
                    path: PathBuf::from("cursor/chats"),
                    source_kind: "cursor-cli-chats".to_owned(),
                    explicitly_selected: false,
                },
            ],
        );
        assert_eq!(cursor.len(), 2);
        assert!(
            cursor
                .iter()
                .any(|root| root.source_kind == "cursor-cli-chats")
        );
    }
}
