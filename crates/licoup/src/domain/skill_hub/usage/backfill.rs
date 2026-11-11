//! Incremental backfill of skill invocation counts from local transcripts.
//!
//! The scanner reuses the conversation domain's adapter catalog and bounded
//! history discovery to locate the same native history files the conversation
//! views already read; it adds no new file globbing. Only line-oriented
//! append formats (`.jsonl`/`.ndjson`/`.log`) are scanned: each complete line
//! is parsed as JSON and skill calls are projected with the same matching
//! semantics as the live runtime drivers (the shared history profile).
//! Records without a parseable timestamp are skipped because the ledger
//! aggregates per UTC day.
//!
//! Idempotency: per-source watermarks (file identity, size, mtime, parsed
//! byte offset, append guard, scanner revision) make unchanged files free to
//! skip and append-only growth cheap to resume. Every counted invocation is
//! also digested (agent + source + vendor call id or record byte offset +
//! skill); digests persist next to the ledger buckets, so a re-scan after a
//! rewrite, truncation, or forced refresh never double counts.
//!
//! Privacy posture matches the agent-usage token scanner: only aggregate
//! counts, sanitized skill ids, hashed source identities, and invocation
//! digests are stored. Paths, prompts, arguments, and tool output never
//! leave the transcript.

use super::ledger::{self, RecordSource};
use crate::domain::conversation::history_discovery::{
    HistoryDiscoveryOptions, discover_history_files,
};
use crate::domain::conversation::source_catalog::{
    HistoryAdapter, adapter_for_agent, history_roots,
};
use crate::domain::skill_hub::{
    ClientStateStore, Result, Value, bool_param, collection_items_mut, json, string_param,
};
use anyhow::anyhow;
use fs2::FileExt;
use licoup_agent_adapter_sdk::skill_invocation_projection::project_history_skill_invocations;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
use std::path::Path;
use std::time::UNIX_EPOCH;
use time::{OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

pub(super) const SCANNER_REVISION: &str = "skill-usage-backfill-v1";
const SCAN_SOURCE_KIND: &str = "skill-usage-scan-source";
const MAX_STORED_DIGESTS: usize = 8_192;
const HASH_BUFFER_BYTES: usize = 64 * 1024;

/// Agents whose local history the conversation domain can discover. Kept in
/// sync with `agent_usage::contract::SUPPORTED_AGENTS`.
const BACKFILL_AGENT_IDS: &[&str] = &[
    "antigravity",
    "claude-code",
    "codex",
    "copilot",
    "cursor",
    "hermes",
    "kilo-code",
    "kimi-code",
    "openclaw",
    "opencode",
    "pi",
];

#[derive(Default)]
struct AgentScanStats {
    files_discovered: u64,
    files_scanned: u64,
    files_unchanged: u64,
    files_appended: u64,
    files_replaced: u64,
    files_failed: u64,
    invocations_found: u64,
    invocations_added: u64,
    invocations_duplicate: u64,
    records_without_timestamp: u64,
}

impl AgentScanStats {
    fn merge(&mut self, other: &Self) {
        self.files_discovered = self.files_discovered.saturating_add(other.files_discovered);
        self.files_scanned = self.files_scanned.saturating_add(other.files_scanned);
        self.files_unchanged = self.files_unchanged.saturating_add(other.files_unchanged);
        self.files_appended = self.files_appended.saturating_add(other.files_appended);
        self.files_replaced = self.files_replaced.saturating_add(other.files_replaced);
        self.files_failed = self.files_failed.saturating_add(other.files_failed);
        self.invocations_found = self
            .invocations_found
            .saturating_add(other.invocations_found);
        self.invocations_added = self
            .invocations_added
            .saturating_add(other.invocations_added);
        self.invocations_duplicate = self
            .invocations_duplicate
            .saturating_add(other.invocations_duplicate);
        self.records_without_timestamp = self
            .records_without_timestamp
            .saturating_add(other.records_without_timestamp);
    }

    fn to_json(&self, agent_id: &str) -> Value {
        json!({
            "agentId": agent_id,
            "filesDiscovered": self.files_discovered,
            "filesScanned": self.files_scanned,
            "filesUnchanged": self.files_unchanged,
            "filesAppended": self.files_appended,
            "filesReplaced": self.files_replaced,
            "filesFailed": self.files_failed,
            "invocationsFound": self.invocations_found,
            "invocationsAdded": self.invocations_added,
            "invocationsDuplicate": self.invocations_duplicate,
            "recordsWithoutTimestamp": self.records_without_timestamp
        })
    }
}

struct SourceMetadata {
    modified_ns: u64,
    size: u64,
    file_id: Option<String>,
}

#[derive(Clone, Default)]
struct ScanSourceState {
    file_id: Option<String>,
    size: u64,
    modified_ns: u64,
    parsed_bytes: u64,
    append_guard: String,
    digests: BTreeSet<String>,
    revision: String,
}

impl ScanSourceState {
    fn from_json(item: &Value) -> Option<Self> {
        Some(Self {
            file_id: item
                .get("fileId")
                .and_then(Value::as_str)
                .map(str::to_owned),
            size: item.get("size")?.as_u64()?,
            modified_ns: item.get("modifiedNs")?.as_u64()?,
            parsed_bytes: item.get("parsedBytes")?.as_u64()?,
            append_guard: item
                .get("appendGuard")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            digests: item
                .get("digests")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|digest| digest.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            revision: item
                .get("scannerRevision")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        })
    }

    fn to_json(&self, source_key: &str, agent_id: &str) -> Value {
        json!({
            "kind": SCAN_SOURCE_KIND,
            "sourceKey": source_key,
            "agentId": agent_id,
            "fileId": self.file_id,
            "size": self.size,
            "modifiedNs": self.modified_ns,
            "parsedBytes": self.parsed_bytes,
            "appendGuard": self.append_guard,
            "digests": self.digests.iter().collect::<Vec<_>>(),
            "scannerRevision": self.revision,
            "scannedAt": OffsetDateTime::now_utc()
                .format(&Rfc3339)
                .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
        })
    }
}

struct Invocation {
    day: String,
    occurred_at: OffsetDateTime,
    skill_id: String,
    dedup_key: String,
}

struct ParsedFile {
    invocations: Vec<Invocation>,
    end_bytes: u64,
    records_without_timestamp: u64,
}

pub(super) fn scan(store: &ClientStateStore, params: &Value) -> Result<Value> {
    let agent_filter = string_param(params, &["agent", "agentId"], usize::MAX);
    let filter_adapter = agent_filter
        .as_deref()
        .map(|value| {
            adapter_for_agent(value).ok_or_else(|| {
                anyhow!("skill usage scan has no history adapter for agent '{value}'")
            })
        })
        .transpose()?;
    let force_refresh = bool_param(params, "forceRefresh") == Some(true);
    let mut agents = Vec::new();
    let mut totals = AgentScanStats::default();
    for agent_id in BACKFILL_AGENT_IDS {
        let Some(adapter) = adapter_for_agent(agent_id) else {
            continue;
        };
        if filter_adapter.is_some_and(|filter| filter != adapter) {
            continue;
        }
        let stats = scan_agent(store, adapter, params, force_refresh)?;
        totals.merge(&stats);
        agents.push(stats.to_json(adapter.id()));
    }
    Ok(json!({
        "ok": true,
        "mode": "local-skill-usage",
        "operation": "history-backfill-scan",
        "scannerRevision": SCANNER_REVISION,
        "generatedAt": OffsetDateTime::now_utc().format(&Rfc3339)?,
        "agents": agents,
        "filesScanned": totals.files_scanned,
        "filesUnchanged": totals.files_unchanged,
        "invocationsFound": totals.invocations_found,
        "invocationsAdded": totals.invocations_added,
        "invocationsDuplicate": totals.invocations_duplicate,
        "watermark": {
            "collection": ledger::COLLECTION,
            "sourceKind": SCAN_SOURCE_KIND,
            "sourcesTracked": count_scan_sources(store)?
        },
        "collectionSource": RecordSource::Backfill.label(),
        "privacy": "aggregate-only"
    }))
}

fn scan_agent(
    store: &ClientStateStore,
    adapter: HistoryAdapter,
    params: &Value,
    force_refresh: bool,
) -> Result<AgentScanStats> {
    let roots = history_roots(adapter, params);
    let discovery = discover_history_files(adapter, &roots, HistoryDiscoveryOptions::default());
    let mut stats = AgentScanStats {
        files_discovered: discovery.candidates.len() as u64,
        ..AgentScanStats::default()
    };
    let paths = discovery
        .candidates
        .into_iter()
        .map(|candidate| candidate.path)
        .collect::<BTreeSet<_>>();
    // Classification snapshot; every file is revalidated against fresh
    // collection state inside the usage lock before counts are applied.
    let states = load_scan_states(store, adapter.id())?;
    for path in paths {
        if !is_append_format(&path) {
            continue;
        }
        let Some(metadata) = source_metadata(&path) else {
            continue;
        };
        let prior = states.get(&source_key(adapter.id(), &path)).cloned();
        if scan_file(
            store,
            adapter,
            &path,
            &metadata,
            prior.as_ref(),
            force_refresh,
            &mut stats,
        )
        .is_err()
        {
            stats.files_failed = stats.files_failed.saturating_add(1);
        }
    }
    Ok(stats)
}

fn scan_file(
    store: &ClientStateStore,
    adapter: HistoryAdapter,
    path: &Path,
    metadata: &SourceMetadata,
    prior: Option<&ScanSourceState>,
    force_refresh: bool,
    stats: &mut AgentScanStats,
) -> Result<()> {
    let agent_id = adapter.id();
    let key = source_key(agent_id, path);
    let current_revision = prior.is_some_and(|state| state.revision == SCANNER_REVISION);
    let same_file =
        prior.is_some_and(|state| state.file_id.is_some() && state.file_id == metadata.file_id);
    if !force_refresh
        && current_revision
        && same_file
        && prior.is_some_and(|state| {
            state.size == metadata.size && state.modified_ns == metadata.modified_ns
        })
    {
        stats.files_unchanged = stats.files_unchanged.saturating_add(1);
        return Ok(());
    }
    let append = !force_refresh
        && current_revision
        && same_file
        && prior.is_some_and(|state| {
            metadata.size > state.size
                && state.parsed_bytes <= state.size
                && append_guard_matches(path, state.parsed_bytes, &state.append_guard)
        });
    let start_offset = if append {
        prior.map(|state| state.parsed_bytes).unwrap_or(0)
    } else {
        0
    };
    let parsed = parse_file_range(path, start_offset)?;
    stats.files_scanned = stats.files_scanned.saturating_add(1);
    stats.invocations_found = stats
        .invocations_found
        .saturating_add(parsed.invocations.len() as u64);
    stats.records_without_timestamp = stats
        .records_without_timestamp
        .saturating_add(parsed.records_without_timestamp);
    if append {
        stats.files_appended = stats.files_appended.saturating_add(1);
    } else {
        stats.files_replaced = stats.files_replaced.saturating_add(1);
    }

    let lock = ledger::usage_lock(store)?;
    lock.lock_exclusive()?;
    let recorded = record_file_scan(store, agent_id, &key, path, metadata, &parsed, append);
    let unlock_result = lock.unlock();
    let (added, duplicates) = recorded?;
    unlock_result?;
    stats.invocations_added = stats.invocations_added.saturating_add(added);
    stats.invocations_duplicate = stats.invocations_duplicate.saturating_add(duplicates);
    Ok(())
}

/// Apply one file's scan inside the usage lock: dedup against fresh state,
/// merge day buckets, and advance the watermark in the same document write.
fn record_file_scan(
    store: &ClientStateStore,
    agent_id: &str,
    source_key: &str,
    path: &Path,
    metadata: &SourceMetadata,
    parsed: &ParsedFile,
    append: bool,
) -> Result<(u64, u64)> {
    let mut document = store.read_collection(ledger::COLLECTION)?;
    let items = collection_items_mut(&mut document)?;
    let mut fresh = take_scan_source(items, source_key);
    let mut added = 0_u64;
    let mut duplicates = 0_u64;
    let mut by_day = BTreeMap::<String, (BTreeMap<String, u64>, OffsetDateTime)>::new();
    for invocation in &parsed.invocations {
        let digest = invocation_digest(
            agent_id,
            source_key,
            &invocation.dedup_key,
            &invocation.skill_id,
        );
        if !fresh.digests.insert(digest) {
            duplicates = duplicates.saturating_add(1);
            continue;
        }
        let entry = by_day
            .entry(invocation.day.clone())
            .or_insert_with(|| (BTreeMap::new(), invocation.occurred_at));
        *entry.0.entry(invocation.skill_id.clone()).or_default() += 1;
        if invocation.occurred_at > entry.1 {
            entry.1 = invocation.occurred_at;
        }
        added = added.saturating_add(1);
    }
    for (day, (counts, occurred_at)) in &by_day {
        ledger::upsert_day_buckets(items, agent_id, day, counts, *occurred_at)?;
    }
    if !append || parsed.end_bytes > fresh.parsed_bytes {
        fresh.parsed_bytes = parsed.end_bytes;
        fresh.size = metadata.size;
        fresh.modified_ns = metadata.modified_ns;
        fresh.file_id = metadata.file_id.clone();
        fresh.append_guard = append_guard(path, parsed.end_bytes)?;
    }
    fresh.revision = SCANNER_REVISION.to_owned();
    if fresh.digests.len() > MAX_STORED_DIGESTS {
        fresh.digests = fresh
            .digests
            .iter()
            .take(MAX_STORED_DIGESTS)
            .cloned()
            .collect();
    }
    items.push(fresh.to_json(source_key, agent_id));
    store.write_collection(ledger::COLLECTION, document)?;
    Ok((added, duplicates))
}

fn parse_file_range(path: &Path, start_offset: u64) -> Result<ParsedFile> {
    let mut reader = BufReader::new(fs::File::open(path)?);
    reader.seek(SeekFrom::Start(start_offset))?;
    let mut parsed_bytes = start_offset;
    let mut invocations = Vec::new();
    let mut records_without_timestamp = 0_u64;
    loop {
        let line_start = parsed_bytes;
        let mut bytes = Vec::new();
        let read = reader.read_until(b'\n', &mut bytes)?;
        if read == 0 {
            break;
        }
        if bytes.last() != Some(&b'\n') && serde_json::from_slice::<Value>(&bytes).is_err() {
            // A valid final JSON record does not require a trailing newline.
            // Only an actually incomplete tail is left for the next append.
            parsed_bytes = line_start;
            break;
        }
        parsed_bytes = parsed_bytes.saturating_add(read as u64);
        while matches!(bytes.last(), Some(b'\n' | b'\r')) {
            bytes.pop();
        }
        if bytes.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        let events = project_history_skill_invocations(&record);
        if events.is_empty() {
            continue;
        }
        let Some(occurred_at) = record_timestamp(&record) else {
            records_without_timestamp = records_without_timestamp.saturating_add(1);
            continue;
        };
        let day = utc_day(&occurred_at)?;
        for (index, event) in events.iter().enumerate() {
            let Some(skill_id) = event.get("skillId").and_then(Value::as_str) else {
                continue;
            };
            if !ledger::is_sanitized_skill_id(skill_id) {
                continue;
            }
            let dedup_key = event
                .get("invocationIdDigest")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("offset:{line_start}:{index}"));
            invocations.push(Invocation {
                day: day.clone(),
                occurred_at,
                skill_id: skill_id.to_owned(),
                dedup_key,
            });
        }
    }
    Ok(ParsedFile {
        invocations,
        end_bytes: parsed_bytes,
        records_without_timestamp,
    })
}

fn record_timestamp(value: &Value) -> Option<OffsetDateTime> {
    for candidate in [
        value.get("timestamp"),
        value.get("time"),
        value.get("createdAt"),
        value.get("created_at"),
        value.get("date"),
        value.pointer("/data/timestamp"),
        value.pointer("/data/time"),
        value.pointer("/message/timestamp"),
        value.pointer("/message/time"),
        value.pointer("/payload/timestamp"),
        value.pointer("/payload/time"),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(text) = candidate
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
        {
            if let Ok(timestamp) = OffsetDateTime::parse(text, &Rfc3339) {
                return Some(timestamp.to_offset(UtcOffset::UTC));
            }
            if let Ok(epoch) = text.parse::<i64>() {
                return epoch_timestamp(epoch);
            }
        }
        if let Some(number) = candidate.as_i64() {
            return epoch_timestamp(number);
        }
        if let Some(number) = candidate
            .as_u64()
            .and_then(|value| i64::try_from(value).ok())
        {
            return epoch_timestamp(number);
        }
    }
    None
}

fn epoch_timestamp(value: i64) -> Option<OffsetDateTime> {
    if value <= 0 {
        return None;
    }
    let absolute = (value as i128).abs();
    let seconds = if absolute >= 100_000_000_000_000_000 {
        value / 1_000_000_000
    } else if absolute >= 100_000_000_000_000 {
        value / 1_000_000
    } else if absolute >= 100_000_000_000 {
        value / 1_000
    } else {
        value
    };
    OffsetDateTime::from_unix_timestamp(seconds).ok()
}

fn utc_day(value: &OffsetDateTime) -> Result<String> {
    let format = time::format_description::parse_borrowed::<2>("[year]-[month]-[day]")?;
    Ok(value.date().format(&format)?)
}

fn load_scan_states(
    store: &ClientStateStore,
    agent_id: &str,
) -> Result<BTreeMap<String, ScanSourceState>> {
    let document = store.read_collection(ledger::COLLECTION)?;
    Ok(document
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|item| {
            item.get("kind").and_then(Value::as_str) == Some(SCAN_SOURCE_KIND)
                && item.get("agentId").and_then(Value::as_str) == Some(agent_id)
        })
        .filter_map(|item| {
            let key = item.get("sourceKey")?.as_str()?.to_string();
            Some((key, ScanSourceState::from_json(item)?))
        })
        .collect())
}

fn take_scan_source(items: &mut Vec<Value>, source_key: &str) -> ScanSourceState {
    items
        .iter()
        .position(|item| {
            item.get("kind").and_then(Value::as_str) == Some(SCAN_SOURCE_KIND)
                && item.get("sourceKey").and_then(Value::as_str) == Some(source_key)
        })
        .map(|index| items.remove(index))
        .and_then(|item| ScanSourceState::from_json(&item))
        .unwrap_or_default()
}

fn count_scan_sources(store: &ClientStateStore) -> Result<u64> {
    let document = store.read_collection(ledger::COLLECTION)?;
    Ok(document
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|item| item.get("kind").and_then(Value::as_str) == Some(SCAN_SOURCE_KIND))
        .count() as u64)
}

fn source_metadata(path: &Path) -> Option<SourceMetadata> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return None;
    }
    let modified_ns = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos()
        .min(u64::MAX as u128) as u64;
    #[cfg(unix)]
    let file_id = Some(format!("{}:{}", metadata.dev(), metadata.ino()));
    #[cfg(windows)]
    let file_id =
        (metadata.creation_time() > 0).then(|| format!("windows:{}", metadata.creation_time()));
    #[cfg(not(any(unix, windows)))]
    let file_id = None;
    Some(SourceMetadata {
        modified_ns,
        size: metadata.len(),
        file_id,
    })
}

fn append_guard(path: &Path, guarded_bytes: u64) -> Result<String> {
    let file = fs::File::open(path)?;
    if file.metadata()?.len() < guarded_bytes {
        return Err(anyhow!(
            "skill usage scan append guard exceeds source length"
        ));
    }
    let mut limited = file.take(guarded_bytes);
    let mut hasher = Sha256::new();
    hasher.update(b"lico.skill-usage-scan-source-v1\0");
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES];
    loop {
        let read = limited.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn append_guard_matches(path: &Path, guarded_bytes: u64, expected: &str) -> bool {
    !expected.is_empty()
        && append_guard(path, guarded_bytes)
            .map(|observed| observed == expected)
            .unwrap_or(false)
}

fn source_key(agent_id: &str, path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"lico.skill-usage-scan-source-v1\0");
    hasher.update(agent_id.as_bytes());
    hasher.update(b"\n");
    hasher.update(path.to_string_lossy().as_bytes());
    format!("{:x}", hasher.finalize())
}

fn invocation_digest(agent_id: &str, source_key: &str, dedup_key: &str, skill_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"lico.skill-usage-backfill.v1\0");
    for part in [agent_id, source_key, dedup_key, skill_id] {
        hasher.update(part.as_bytes());
        hasher.update([0x1f_u8]);
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn is_append_format(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "jsonl" | "ndjson" | "log"
    )
}
