use anyhow::{Context, Result, anyhow, ensure};
use collection_store::CollectionStore;
use fs2::FileExt;
use licoup_foundation::platform::file_security::{
    ensure_private_dir, open_private_lock_file, read_existing_private_text_bounded, sync_directory,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

use super::{paths, policy};

pub const TARGET_DISCOVERY_CACHE_COLLECTION: &str = "target-discovery-cache";
pub const TARGET_DISCOVERY_CACHE_SCHEMA: &str = "licoup.target-discovery-cache.v1";
const TARGET_DISCOVERY_CACHE_LOCK: &str = ".target-discovery-cache.lock";

/// Typed internal projection of one target-discovery-cache record. Only
/// identity and resolution fields are decoded; unknown JSON extension fields
/// round-trip through the persisted document without a second authority.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetRouteRecord {
    pub schema_version: String,
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub binary_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_path: Option<String>,
    pub scan_source: String,
    pub runtime_ready: bool,
    pub cached_at_epoch_seconds: u64,
    #[serde(flatten)]
    pub extension: Map<String, Value>,
}

#[derive(Clone, Debug)]
pub struct ClientStateStore {
    root: PathBuf,
    target_index: Arc<TargetCollectionIndex>,
}

/// The shared client-state owner. `CollectionStore` resolves every collection
/// document; the target projection and its cross-process transaction lock stay
/// here because they are typed LicoUp payloads, not generic storage.
#[derive(Debug, Default)]
struct TargetCollectionIndex {
    collections: RwLock<Option<CollectionStore>>,
    state: RwLock<Option<IndexedTargetCollection>>,
}

/// Cross-process transaction boundary for every target-route mutation.
/// Atomic file replacement protects readers; this guard additionally keeps a
/// read-modify-write update indivisible across independent store instances.
struct TargetRouteTransactionGuard {
    lock: File,
}

impl TargetRouteTransactionGuard {
    fn acquire(root: &Path) -> Result<Self> {
        let lock = open_private_lock_file(&root.join(TARGET_DISCOVERY_CACHE_LOCK))?;
        lock.lock_exclusive()
            .map_err(|_| anyhow!("target_route_transaction_lock_unavailable"))?;
        Ok(Self { lock })
    }
}

impl Drop for TargetRouteTransactionGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.lock);
    }
}

#[derive(Debug)]
struct IndexedTargetCollection {
    generation: Option<FileGeneration>,
    parses: u64,
    invalidations: u64,
    records: Vec<TargetRouteRecord>,
    by_target: HashMap<String, usize>,
    document: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileGeneration {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    len: u64,
    modified: SystemTime,
}

impl ClientStateStore {
    pub fn portable() -> Result<Self> {
        Self::new(paths::portable_state_root()?)
    }

    pub fn portable_read_only() -> Result<Self> {
        Ok(Self::open_read_only(paths::portable_state_root_read_only()?))
    }

    /// Open a read-only projection without creating directories, collection
    /// files, snapshots, or activity state. Missing collections read empty.
    pub fn open_read_only(root: PathBuf) -> Self {
        Self {
            root,
            target_index: Arc::new(TargetCollectionIndex::default()),
        }
    }

    pub fn new(root: PathBuf) -> Result<Self> {
        // The client owns the data-home layout and ancestor hardening; the
        // published store owns the collection documents beneath this root.
        ensure_private_dir(&root)?;
        ensure_private_dir(&paths::snapshot_root(&root))?;
        ensure_private_dir(&paths::activity_root(&root))?;
        let collections = CollectionStore::new(root)?;
        let store = Self {
            root: collections.root().to_path_buf(),
            target_index: Arc::new(TargetCollectionIndex::default()),
        };
        store.cache_collection_store(collections);
        store.ensure_collections()?;
        Ok(store)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn collection_path(&self, collection: &str) -> Result<PathBuf> {
        validate_collection(collection)?;
        Ok(paths::collection_path(&self.root, collection))
    }

    pub fn read_collection(&self, collection: &str) -> Result<Value> {
        let path = self.collection_path(collection)?;
        read_collection_document(collection, &path)
    }

    pub fn read_collection_read_only(&self, collection: &str) -> Result<Value> {
        self.read_collection(collection)
    }

    pub fn write_collection(&self, collection: &str, value: Value) -> Result<Value> {
        self.collection_path(collection)?;
        let _target_transaction = self.target_transaction(collection)?;
        let store = self.writable_collection_store()?;
        write_collection_durably(&store, &self.root, collection, value)
    }

    /// Persist complete newest items from an oldest-first sequence, bounded
    /// both by count and the actual encoded collection size.
    pub fn write_collection_retaining_latest_items(
        &self,
        collection: &str,
        value: Value,
        max_items: usize,
    ) -> Result<Value> {
        self.collection_path(collection)?;
        let _target_transaction = self.target_transaction(collection)?;
        let store = self.writable_collection_store()?;
        let document =
            store.write_collection_retaining_latest_items(collection, value, max_items)?;
        sync_collection_parent(&self.root)?;
        Ok(document)
    }

    fn cached_collection_store(&self) -> Option<CollectionStore> {
        read_collections(&self.target_index).clone()
    }

    fn cache_collection_store(&self, store: CollectionStore) {
        *write_collections(&self.target_index) = Some(store);
    }

    /// The store used for a write. A root that disappeared is recreated, as
    /// the previous direct writer did.
    fn writable_collection_store(&self) -> Result<CollectionStore> {
        if let Some(store) = self.cached_collection_store() {
            return Ok(store);
        }
        let store = CollectionStore::new(self.root.clone())?;
        self.cache_collection_store(store.clone());
        Ok(store)
    }

    fn target_transaction(&self, collection: &str) -> Result<Option<TargetRouteTransactionGuard>> {
        if collection == TARGET_DISCOVERY_CACHE_COLLECTION {
            Ok(Some(TargetRouteTransactionGuard::acquire(&self.root)?))
        } else {
            Ok(None)
        }
    }

    /// Typed target-route read in document order. The projection is refreshed
    /// only when the persisted file generation changes, so repeated reads
    /// decode the collection exactly once per generation.
    pub fn read_target_routes(&self) -> Result<Vec<TargetRouteRecord>> {
        let mut guard = lock_target_index(&self.target_index);
        self.refresh_target_index_locked(&mut guard)?;
        let cached = guard
            .as_ref()
            .ok_or_else(|| anyhow!("target index projection is unavailable"))?;
        Ok(cached.records.clone())
    }

    /// Keyed target-route lookup over the store-owned projection. A changed
    /// file identity, size, modification time, or generation forces one
    /// reparse before the lookup; no linear scan or per-call reconstruction
    /// remains.
    pub fn target_route(&self, target: &str) -> Result<Option<TargetRouteRecord>> {
        let path = self.collection_path(TARGET_DISCOVERY_CACHE_COLLECTION)?;
        let generation = file_generation(&path);
        {
            let guard = read_target_index(&self.target_index);
            if let Some(cached) = guard.as_ref()
                && cached.generation == generation
            {
                return Ok(cached
                    .by_target
                    .get(target)
                    .map(|&position| cached.records[position].clone()));
            }
        }
        let mut guard = lock_target_index(&self.target_index);
        self.refresh_target_index_locked(&mut guard)?;
        let cached = guard
            .as_ref()
            .ok_or_else(|| anyhow!("target index projection is unavailable"))?;
        Ok(cached
            .by_target
            .get(target)
            .map(|&position| cached.records[position].clone()))
    }

    /// Transactional replacement of all typed target routes. The document
    /// keeps every unknown top-level and record extension field, is persisted
    /// with the private atomic writer, and only then swaps the projection.
    pub fn write_target_routes(&self, records: &[TargetRouteRecord]) -> Result<()> {
        let _transaction = TargetRouteTransactionGuard::acquire(&self.root)?;
        let mut guard = lock_target_index(&self.target_index);
        self.refresh_target_index_locked(&mut guard)?;
        self.write_target_routes_locked(&mut guard, records)
    }

    /// Transactional read-modify-write over the latest persisted route set.
    /// The callback runs while the cross-process lock is held, so no selected
    /// scan can overwrite routes committed by another concurrent scan.
    pub fn update_target_routes<F>(&self, update: F) -> Result<()>
    where
        F: FnOnce(&mut Vec<TargetRouteRecord>) -> Result<()>,
    {
        let _transaction = TargetRouteTransactionGuard::acquire(&self.root)?;
        let mut guard = lock_target_index(&self.target_index);
        self.refresh_target_index_locked(&mut guard)?;
        let mut records = guard
            .as_ref()
            .ok_or_else(|| anyhow!("target index projection is unavailable"))?
            .records
            .clone();
        update(&mut records)?;
        self.write_target_routes_locked(&mut guard, &records)
    }

    pub fn target_index_parse_count(&self) -> u64 {
        let guard = read_target_index(&self.target_index);
        guard.as_ref().map_or(0, |cached| cached.parses)
    }

    pub fn target_index_invalidation_count(&self) -> u64 {
        let guard = read_target_index(&self.target_index);
        guard.as_ref().map_or(0, |cached| cached.invalidations)
    }

    fn ensure_collections(&self) -> Result<()> {
        let store = self.writable_collection_store()?;
        for collection in policy::COLLECTIONS {
            let path = self.collection_path(collection)?;
            let _transaction = self.target_transaction(collection)?;
            if path.try_exists()? {
                read_collection_document(collection, &path)?;
            } else {
                store.write_collection(collection, json!({ "items": [] }))?;
                sync_collection_parent(&self.root)?;
            }
        }
        Ok(())
    }

    fn write_target_routes_locked(
        &self,
        index: &mut Option<IndexedTargetCollection>,
        records: &[TargetRouteRecord],
    ) -> Result<()> {
        let (mut document, parses, invalidations) = {
            let cached = index
                .as_ref()
                .ok_or_else(|| anyhow!("target index projection is unavailable"))?;
            (cached.document.clone(), cached.parses, cached.invalidations)
        };
        document["items"] = serde_json::to_value(records)?;
        let path = self.collection_path(TARGET_DISCOVERY_CACHE_COLLECTION)?;
        let store = self.writable_collection_store()?;
        let document = write_collection_durably(
            &store,
            &self.root,
            TARGET_DISCOVERY_CACHE_COLLECTION,
            document,
        )?;
        *index = Some(IndexedTargetCollection {
            generation: file_generation(&path),
            parses,
            invalidations,
            records: records.to_vec(),
            by_target: index_target_routes(records),
            document,
        });
        Ok(())
    }

    fn refresh_target_index_locked(
        &self,
        index: &mut Option<IndexedTargetCollection>,
    ) -> Result<()> {
        let path = self.collection_path(TARGET_DISCOVERY_CACHE_COLLECTION)?;
        let generation = file_generation(&path);
        if let Some(cached) = index.as_ref()
            && cached.generation == generation
        {
            return Ok(());
        }
        let document = self.read_collection(TARGET_DISCOVERY_CACHE_COLLECTION)?;
        let routes = decode_target_routes(&document)?;
        let prior = index.take();
        let parses = prior.as_ref().map_or(0, |cached| cached.parses) + 1;
        let invalidations =
            prior.as_ref().map_or(0, |cached| cached.invalidations) + u64::from(prior.is_some());
        let by_target = index_target_routes(&routes);
        *index = Some(IndexedTargetCollection {
            generation,
            parses,
            invalidations,
            records: routes,
            by_target,
            document,
        });
        Ok(())
    }
}

fn read_target_index(
    index: &TargetCollectionIndex,
) -> std::sync::RwLockReadGuard<'_, Option<IndexedTargetCollection>> {
    index
        .state
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn lock_target_index(
    index: &TargetCollectionIndex,
) -> std::sync::RwLockWriteGuard<'_, Option<IndexedTargetCollection>> {
    index
        .state
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn read_collections(
    index: &TargetCollectionIndex,
) -> std::sync::RwLockReadGuard<'_, Option<CollectionStore>> {
    index
        .collections
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write_collections(
    index: &TargetCollectionIndex,
) -> std::sync::RwLockWriteGuard<'_, Option<CollectionStore>> {
    index
        .collections
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn index_target_routes(records: &[TargetRouteRecord]) -> HashMap<String, usize> {
    let mut by_target = HashMap::with_capacity(records.len());
    for (position, record) in records.iter().enumerate() {
        // Preserve the legacy document-order lookup if a malformed external
        // writer supplies duplicate identities. The typed index accelerates
        // lookup without silently changing which record wins.
        by_target.entry(record.target.clone()).or_insert(position);
    }
    by_target
}

fn file_generation(path: &Path) -> Option<FileGeneration> {
    let metadata = fs::metadata(path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(FileGeneration {
            device: metadata.dev(),
            inode: metadata.ino(),
            len: metadata.len(),
            modified: metadata.modified().ok()?,
        })
    }
    #[cfg(not(unix))]
    {
        Some(FileGeneration {
            len: metadata.len(),
            modified: metadata.modified().ok()?,
        })
    }
}

fn decode_target_routes(document: &Value) -> Result<Vec<TargetRouteRecord>> {
    let items = document
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    items
        .into_iter()
        .map(|item| {
            let record: TargetRouteRecord = serde_json::from_value(item)?;
            ensure!(
                record.schema_version == TARGET_DISCOVERY_CACHE_SCHEMA,
                "target discovery cache schema is invalid"
            );
            Ok(record)
        })
        .collect()
}

pub fn validate_collection(collection: &str) -> Result<()> {
    if policy::COLLECTIONS.contains(&collection) {
        Ok(())
    } else {
        Err(anyhow!("unsupported client state collection"))
    }
}

fn empty_collection(collection: &str) -> Value {
    json!({
        "schemaVersion": policy::STATE_SCHEMA_VERSION,
        "collection": collection,
        "items": []
    })
}

/// Read one collection through the client's owned hardening boundary.
///
/// The published store wrote the document, so it owns its write and
/// normalization rules; it also read it, but its reader validates a path and
/// then reads that path again, and it validates the root rather than the
/// document. A symlink swapped between the two calls could redirect the read,
/// and a document whose owner or mode changed after the root check would still
/// be parsed. This function reads the bytes with the foundation's
/// no-follow/owner-and-mode-validated/bounded/identity-checked reader and then
/// applies the same document admission the store applies, so a caller sees
/// exactly the documents the published writer produces — or a refusal.
fn read_collection_document(collection: &str, path: &Path) -> Result<Value> {
    let Some(raw) =
        read_existing_private_text_bounded(path, policy::MAX_COLLECTION_DOCUMENT_BYTES)?
    else {
        return Ok(empty_collection(collection));
    };
    // The store refuses a zero-length existing document but treats a
    // whitespace-only document as the empty default; preserve both.
    ensure!(!raw.is_empty(), "client state collection is empty");
    if raw.trim().is_empty() {
        return Ok(empty_collection(collection));
    }
    let document: Value =
        serde_json::from_str(&raw).context("client state collection JSON is invalid")?;
    validate_collection_document(collection, &document)?;
    Ok(document)
}

/// The document admission the published store applies on read: an object, the
/// current schema marker, and the collection name that owns the file. Keeping
/// it beside the hardened reader means a migration probe validates the bytes it
/// actually read rather than re-reading the path through a second opener.
pub(crate) fn validate_collection_document(collection: &str, document: &Value) -> Result<()> {
    ensure!(
        document.is_object(),
        "client state collection is not an object"
    );
    ensure!(
        document.get("schemaVersion").and_then(Value::as_str) == Some(policy::STATE_SCHEMA_VERSION),
        "client state collection requires startup migration"
    );
    ensure!(
        document.get("collection").and_then(Value::as_str) == Some(collection),
        "client state collection owner mismatch"
    );
    Ok(())
}

/// Write one normalized document and confirm the rename that publishes it.
///
/// `collection_store` fsyncs the staged file and renames it over the document,
/// but it does not fsync the containing directory. A successful write is
/// reported as durable only after that parent sync, which is what makes the
/// rename itself survive a crash. On a platform that refuses a directory
/// handle (Windows without the privilege), the foundation helper treats the
/// sync as unsupported and returns success; the platform's replace semantics
/// are then the documented durability boundary.
fn write_collection_durably(
    store: &CollectionStore,
    root: &Path,
    collection: &str,
    value: Value,
) -> Result<Value> {
    let document = store.write_collection(collection, value)?;
    sync_collection_parent(root)?;
    Ok(document)
}

fn sync_collection_parent(root: &Path) -> Result<()> {
    #[cfg(test)]
    record_parent_sync();
    sync_directory(root)
}

#[cfg(test)]
thread_local! {
    static PARENT_SYNCS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn record_parent_sync() {
    PARENT_SYNCS.with(|count| count.set(count.get() + 1));
}

/// Test observation point: reset and read the parent-sync count for the
/// current thread. Production builds compile this out.
#[cfg(test)]
pub(crate) fn take_parent_sync_count() -> usize {
    PARENT_SYNCS.with(|count| count.replace(0))
}
