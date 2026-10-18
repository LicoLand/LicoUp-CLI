//! Private bounded projection cache for browse-mode conversation rows.
//!
//! A browse row is derived state: the same source files produce the same
//! visible row until the source changes. Caching that projection beneath the
//! client-state root removes whole-file re-parsing for warm pages. The cache is
//! strictly bounded (256 entries, 8 MiB), keyed by adapter, canonical identity,
//! source size, and modification metadata, schema-versioned, written
//! atomically, and discarded as a whole on schema mismatch, corruption, or
//! ambiguity (fail-closed). Cached content is the same newest-page, redacted
//! browse projection the page already renders; the full transcript stays in the
//! single-session read path.

use crate::domain::conversation::parameters::text_param;
use crate::domain::conversation::paths::expand_home;
use fs2::FileExt;
use licoup_foundation::platform::paths::portable_data_dir;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const HISTORY_PROJECTION_CACHE_SCHEMA: &str = "licoup.history-projection-cache/v5";
pub(crate) const MAX_PROJECTION_CACHE_ENTRIES: usize = 256;
pub(crate) const MAX_PROJECTION_CACHE_BYTES: usize = 8 * 1024 * 1024;
const MAX_PROJECTION_CACHE_FILE_BYTES: usize = MAX_PROJECTION_CACHE_BYTES + 4096;
const HISTORY_PROJECTION_CACHE_FILE: &str = "history-projections.json";

/// Stat snapshot of one source file that shaped a cached projection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct SourceFingerprint {
    pub(crate) path: String,
    pub(crate) len: u64,
    pub(crate) modified_ns: u64,
}

impl SourceFingerprint {
    pub(crate) fn from_path(path: &Path) -> Option<Self> {
        let metadata = fs::metadata(path).ok()?;
        Some(Self {
            path: path.to_string_lossy().into_owned(),
            len: metadata.len(),
            modified_ns: modified_ns(&metadata),
        })
    }
}

pub(super) fn modified_ns(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0)
}

/// Identity of one hydration unit: the adapter, the layout kind, every source
/// file that shaped the projection, and any store the parser consulted beyond
/// the unit files themselves (Codex delegated labels come from its thread
/// database, so that database is part of the key).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ProjectionCacheKey {
    pub(crate) adapter_id: String,
    pub(crate) source_kind: String,
    pub(crate) kind: String,
    pub(crate) sources: Vec<SourceFingerprint>,
    pub(crate) authority: Option<SourceFingerprint>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CacheEntry {
    key: ProjectionCacheKey,
    sessions: Vec<Value>,
    last_used_ns: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CacheFile {
    schema: String,
    entries: Vec<CacheEntry>,
}

pub(crate) struct HistoryProjectionCache {
    file_path: Option<PathBuf>,
    entries: BTreeMap<String, CacheEntry>,
    removed_keys: HashSet<String>,
    bytes: usize,
    dirty: bool,
    /// Fail-closed discards (schema mismatch, corruption, ambiguous keys).
    pub(crate) discard_count: usize,
}

impl HistoryProjectionCache {
    pub(crate) fn open(params: &Value) -> Self {
        let file_path = projection_cache_file_path(params);
        let mut cache = Self {
            file_path,
            entries: BTreeMap::new(),
            removed_keys: HashSet::new(),
            bytes: 0,
            dirty: false,
            discard_count: 0,
        };
        if let Some(path) = cache.file_path.clone() {
            cache.load(&path);
        }
        cache
    }

    pub(crate) fn get(&mut self, key: &ProjectionCacheKey) -> Option<Vec<Value>> {
        let key_string = key_string(key);
        if !self.entries.contains_key(&key_string) {
            return None;
        }
        if !key_is_current(key) {
            self.evict(&key_string);
            return None;
        }
        let entry = self.entries.get(&key_string)?;
        let sessions = entry.sessions.clone();
        if let Some(entry) = self.entries.get_mut(&key_string) {
            entry.last_used_ns = now_ns();
        }
        Some(sessions)
    }

    pub(crate) fn insert(&mut self, key: ProjectionCacheKey, sessions: Vec<Value>) {
        let key_string = key_string(&key);
        self.removed_keys.remove(&key_string);
        if let Some(previous) = self.entries.remove(&key_string) {
            self.bytes = self.bytes.saturating_sub(previous.serialized_bytes());
        }
        let entry = CacheEntry {
            key,
            sessions,
            last_used_ns: now_ns(),
        };
        let serialized_bytes = entry.serialized_bytes();
        self.entries.insert(key_string, entry);
        self.bytes = self.bytes.saturating_add(serialized_bytes);
        self.dirty = true;
        self.enforce_bounds();
    }

    pub(crate) fn save(&mut self) {
        if !self.dirty {
            return;
        }
        let Some(path) = self.file_path.clone() else {
            return;
        };
        if let Some(parent) = path.parent()
            && fs::create_dir_all(parent).is_err()
        {
            return;
        }
        let Ok(lock) = private_cache_file(&path.with_extension("lock"), false) else {
            return;
        };
        if lock.lock_exclusive().is_err() {
            return;
        }
        // Only publication is serialized. Independent readers hydrate in
        // parallel, then merge the latest on-disk entries before atomic rename.
        let mut latest = Self {
            file_path: None,
            entries: BTreeMap::new(),
            removed_keys: HashSet::new(),
            bytes: 0,
            dirty: false,
            discard_count: 0,
        };
        latest.load(&path);
        for (key, entry) in latest.entries {
            if self.removed_keys.contains(&key) {
                continue;
            }
            if self
                .entries
                .get(&key)
                .is_none_or(|current| current.last_used_ns < entry.last_used_ns)
            {
                self.entries.insert(key, entry);
            }
        }
        self.bytes = self
            .entries
            .values()
            .map(CacheEntry::serialized_bytes)
            .sum();
        self.enforce_bounds();
        let file = CacheFile {
            schema: HISTORY_PROJECTION_CACHE_SCHEMA.to_string(),
            entries: self.entries.values().cloned().collect(),
        };
        let Ok(serialized) = serde_json::to_vec(&file) else {
            return;
        };
        let temporary = path.with_extension("json.tmp");
        if private_cache_file(&temporary, true)
            .and_then(|mut file| file.write_all(&serialized))
            .is_ok()
            && fs::rename(&temporary, &path).is_ok()
        {
            self.dirty = false;
            self.removed_keys.clear();
        }
    }

    pub(crate) fn entry_count(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn byte_count(&self) -> usize {
        self.bytes
    }

    fn load(&mut self, path: &Path) {
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(_) => return,
        };
        let mut raw = Vec::new();
        if file
            .take((MAX_PROJECTION_CACHE_FILE_BYTES as u64).saturating_add(1))
            .read_to_end(&mut raw)
            .is_err()
        {
            return;
        }
        if raw.len() > MAX_PROJECTION_CACHE_FILE_BYTES {
            self.discard("oversized_cache_file");
            return;
        }
        let Ok(file) = serde_json::from_slice::<CacheFile>(&raw) else {
            self.discard("corrupt_cache_file");
            return;
        };
        if file.schema != HISTORY_PROJECTION_CACHE_SCHEMA {
            self.discard("cache_schema_mismatch");
            return;
        }
        for entry in file.entries {
            if entry.key.adapter_id.is_empty()
                || entry.key.kind.is_empty()
                || entry.key.sources.is_empty()
            {
                self.discard("ambiguous_cache_key");
                return;
            }
            let serialized_bytes = entry.serialized_bytes();
            self.bytes = self.bytes.saturating_add(serialized_bytes);
            if self.entries.insert(key_string(&entry.key), entry).is_some() {
                self.discard("duplicate_cache_key");
                return;
            }
        }
        self.enforce_bounds();
    }

    fn discard(&mut self, _reason: &str) {
        self.entries.clear();
        self.bytes = 0;
        self.discard_count += 1;
        // Rewrite the invalid file on the next save so the failure cannot
        // recur on every process start.
        self.dirty = true;
    }

    fn evict(&mut self, key: &str) {
        if let Some(entry) = self.entries.remove(key) {
            self.bytes = self.bytes.saturating_sub(entry.serialized_bytes());
            self.removed_keys.insert(key.to_string());
            self.dirty = true;
        }
    }

    fn enforce_bounds(&mut self) {
        while self.entries.len() > MAX_PROJECTION_CACHE_ENTRIES
            || self.bytes > MAX_PROJECTION_CACHE_BYTES
        {
            let Some(oldest) = self
                .entries
                .values()
                .min_by_key(|entry| entry.last_used_ns)
                .map(|entry| key_string(&entry.key))
            else {
                break;
            };
            self.evict(&oldest);
        }
    }
}

fn private_cache_file(path: &Path, truncate: bool) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options
        .create(true)
        .read(true)
        .write(true)
        .truncate(truncate);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

impl CacheEntry {
    fn serialized_bytes(&self) -> usize {
        serde_json::to_vec(self)
            .map(|bytes| bytes.len())
            .unwrap_or(0)
    }
}

fn key_string(key: &ProjectionCacheKey) -> String {
    serde_json::to_string(key).unwrap_or_default()
}

fn key_is_current(key: &ProjectionCacheKey) -> bool {
    let current = |source: &SourceFingerprint| {
        fs::metadata(&source.path).ok().is_some_and(|metadata| {
            metadata.len() == source.len && modified_ns(&metadata) == source.modified_ns
        })
    };
    key.sources.iter().all(current) && key.authority.as_ref().map(current).unwrap_or(true)
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0)
}

/// Cache file beneath the client-state root. `historyProjectionCacheRoot` is a
/// test-only override that keeps unit tests off the process-global portable
/// data directory; production uses the standard client-state root. Under the
/// test harness the cache stays in memory unless the override names a root, so
/// tests never create or mutate the real portable data directory.
fn projection_cache_file_path(params: &Value) -> Option<PathBuf> {
    let root = text_param(params, &["historyProjectionCacheRoot"])
        .filter(|value| !value.trim().is_empty())
        .map(|value| expand_home(&value));
    match root {
        Some(root) => Some(root.join(HISTORY_PROJECTION_CACHE_FILE)),
        None if cfg!(test) => None,
        None => portable_data_dir().ok().map(|data_dir| {
            data_dir
                .join("client-state")
                .join(HISTORY_PROJECTION_CACHE_FILE)
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::Duration;

    fn temp_dir(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("lico-projection-cache-{label}-{nonce}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn key_for(paths: &[&Path]) -> ProjectionCacheKey {
        ProjectionCacheKey {
            adapter_id: "codex".to_string(),
            source_kind: "codex-session-store".to_string(),
            kind: "file".to_string(),
            sources: paths
                .iter()
                .map(|path| SourceFingerprint::from_path(path).unwrap())
                .collect(),
            authority: None,
        }
    }

    fn open_in(root: &Path) -> HistoryProjectionCache {
        HistoryProjectionCache::open(&json!({
            "historyProjectionCacheRoot": root.to_string_lossy()
        }))
    }

    #[test]
    fn cached_projection_round_trips_through_the_cache_file() {
        let root = temp_dir("roundtrip");
        let source = root.join("rollout.jsonl");
        fs::write(&source, "{}\n").unwrap();
        let key = key_for(&[&source]);
        let mut cache = open_in(&root);
        assert_eq!(cache.get(&key), None);
        cache.insert(
            key.clone(),
            vec![json!({"nativeSessionId": "s1", "messages": []})],
        );
        cache.save();
        drop(cache);

        let mut reopened = open_in(&root);
        assert_eq!(reopened.discard_count, 0);
        let cached = reopened.get(&key).expect("warm read hits the cache");
        assert_eq!(cached[0]["nativeSessionId"], "s1");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn concurrent_readers_merge_independent_projections_at_publication() {
        let root = temp_dir("concurrent-publish");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let mut workers = Vec::new();
        let mut keys = Vec::new();
        for index in 0..4 {
            let source = root.join(format!("source-{index}.jsonl"));
            fs::write(&source, "{}\n").unwrap();
            let key = key_for(&[&source]);
            keys.push(key.clone());
            let root = root.clone();
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                let mut cache = open_in(&root);
                cache.insert(
                    key,
                    vec![json!({"nativeSessionId": format!("session-{index}"), "messages": []})],
                );
                // All readers opened before any publisher acquires its lock.
                barrier.wait();
                cache.save();
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }
        let mut cache = open_in(&root);
        assert_eq!(cache.entry_count(), 4);
        for key in keys {
            assert!(cache.get(&key).is_some());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(root.join(HISTORY_PROJECTION_CACHE_FILE))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn changed_source_stat_invalidates_the_entry() {
        let root = temp_dir("invalidate");
        let source = root.join("rollout.jsonl");
        fs::write(&source, "one\n").unwrap();
        let mut cache = open_in(&root);
        let key = key_for(&[&source]);
        cache.insert(key.clone(), vec![json!({"v": 1})]);
        cache.save();
        drop(cache);

        let mut reopened = open_in(&root);
        assert_eq!(
            reopened.get(&key).map(|s| s[0]["v"].as_i64()),
            Some(Some(1))
        );
        fs::write(&source, "one\ntwo\n").unwrap();
        let modified = SystemTime::now() + Duration::from_secs(5);
        let file = fs::File::open(&source).unwrap();
        file.set_modified(modified).unwrap();
        drop(file);
        assert_eq!(
            reopened.get(&key),
            None,
            "a size or modification change must miss and evict"
        );
        assert_eq!(reopened.entry_count(), 0);
        reopened.save();
        assert_eq!(open_in(&root).entry_count(), 0);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn deleted_source_invalidates_the_entry() {
        let root = temp_dir("deleted-source");
        let source = root.join("rollout.jsonl");
        fs::write(&source, "one\n").unwrap();
        let mut cache = open_in(&root);
        let key = key_for(&[&source]);
        cache.insert(key.clone(), vec![json!({"v": 1})]);
        cache.save();
        drop(cache);

        let mut reopened = open_in(&root);
        fs::remove_file(&source).unwrap();
        assert_eq!(reopened.get(&key), None);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn schema_mismatch_discards_the_whole_cache() {
        let root = temp_dir("schema-mismatch");
        fs::write(
            root.join(HISTORY_PROJECTION_CACHE_FILE),
            json!({
                "schema": "licoup.history-projection-cache/v0",
                "entries": []
            })
            .to_string(),
        )
        .unwrap();
        let cache = open_in(&root);
        assert_eq!(cache.entry_count(), 0);
        assert_eq!(cache.discard_count, 1);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn corrupt_cache_file_discards_the_whole_cache() {
        let root = temp_dir("corrupt");
        fs::write(root.join(HISTORY_PROJECTION_CACHE_FILE), b"not json at all").unwrap();
        let cache = open_in(&root);
        assert_eq!(cache.entry_count(), 0);
        assert_eq!(cache.discard_count, 1);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn oversized_cache_file_is_read_with_a_hard_limit_and_discarded() {
        let root = temp_dir("oversized");
        fs::write(
            root.join(HISTORY_PROJECTION_CACHE_FILE),
            vec![b'x'; MAX_PROJECTION_CACHE_FILE_BYTES + 1],
        )
        .unwrap();
        let cache = open_in(&root);
        assert_eq!(cache.entry_count(), 0);
        assert_eq!(cache.discard_count, 1);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn ambiguous_cache_key_discards_the_whole_cache() {
        let root = temp_dir("ambiguous");
        fs::write(
            root.join(HISTORY_PROJECTION_CACHE_FILE),
            json!({
                "schema": HISTORY_PROJECTION_CACHE_SCHEMA,
                "entries": [{
                    "key": {
                        "adapterId": "codex",
                        "sourceKind": "codex-session-store",
                        "kind": "file",
                        "sources": [],
                        "authority": null
                    },
                    "sessions": [{"nativeSessionId": "s1"}],
                    "lastUsedNs": 1
                }]
            })
            .to_string(),
        )
        .unwrap();
        let cache = open_in(&root);
        assert_eq!(cache.entry_count(), 0);
        assert_eq!(cache.discard_count, 1);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn entry_and_byte_bounds_evict_least_recently_used() {
        let root = temp_dir("bounds");
        let mut cache = open_in(&root);
        let mut paths = Vec::new();
        for index in 0..(MAX_PROJECTION_CACHE_ENTRIES + 16) {
            let source = root.join(format!("rollout-{index}.jsonl"));
            fs::write(&source, format!("record {index}\n")).unwrap();
            paths.push(source);
        }
        for index in 0..paths.len() {
            let key = key_for(&[&paths[index]]);
            cache.insert(key, vec![json!({"nativeSessionId": format!("s{index}")})]);
        }
        assert!(
            cache.entry_count() <= MAX_PROJECTION_CACHE_ENTRIES,
            "entry bound holds"
        );
        assert!(
            cache.byte_count() <= MAX_PROJECTION_CACHE_BYTES,
            "byte bound holds"
        );
        // The oldest entries are gone; the newest survive.
        assert_eq!(cache.get(&key_for(&[&paths[0]])), None);
        assert!(cache.get(&key_for(&[&paths[paths.len() - 1]])).is_some());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn byte_bound_evicts_when_single_payloads_exceed_the_budget() {
        let root = temp_dir("byte-budget");
        let mut cache = open_in(&root);
        let mut paths = Vec::new();
        for index in 0..64 {
            let source = root.join(format!("blob-{index}.jsonl"));
            fs::write(&source, format!("record {index}\n")).unwrap();
            paths.push(source);
        }
        let blob = "x".repeat(256 * 1024);
        for (index, path) in paths.iter().enumerate() {
            cache.insert(
                key_for(&[path]),
                vec![json!({"nativeSessionId": format!("s{index}"), "blob": blob})],
            );
        }
        assert!(
            cache.byte_count() <= MAX_PROJECTION_CACHE_BYTES,
            "byte bound holds under heavy payloads"
        );
        fs::remove_dir_all(&root).unwrap();
    }
}
