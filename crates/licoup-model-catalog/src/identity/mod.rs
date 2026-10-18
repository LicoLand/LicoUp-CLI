//! Declared canonical model identities from public catalog facts, and the one
//! private cache that holds the current snapshot.
//!
//! Identity is a declaration, not entitlement: the registry says which model a
//! name denotes and which provider aliases reach it, and it never says that
//! this host can run it. Observed availability is a separate fact owned by
//! [`crate::availability`], and the two are joined only by an explicit query in
//! [`crate::selection`].
//!
//! The registry reads two things it does not own, both through
//! [`crate::port::ModelCatalogPort`]: the Agent declaration labels that name a
//! source rather than a model, and nothing else. The stale-catalog lock, the
//! atomic cache write and the bounded download are this module's own work.

mod index;
mod source;

use anyhow::{Result, anyhow};
// Display typography is shared vocabulary rather than registry behaviour: the
// Agent inventory, the usage projection and this registry format the same model
// names, so the formatter lives below all of them and its former path stays.
pub use index::RegistrySnapshot;
pub use licoup_foundation::core::model_naming::model_display_name;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalModel {
    pub id: String,
    pub display_name: String,
    pub lab_id: String,
    pub family: Option<String>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub(crate) struct CatalogDocument {
    pub models: BTreeMap<String, Value>,
    pub providers: BTreeMap<String, ProviderDocument>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub(crate) struct ProviderDocument {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub models: BTreeMap<String, Value>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistedCatalog {
    source: String,
    fetched_at: String,
    skipped_entries: usize,
    catalog: CatalogDocument,
}

struct RegistryState {
    snapshot: Arc<RegistrySnapshot>,
    signature: Option<(PathBuf, SystemTime, u64)>,
    source: String,
    fetched_at: Option<String>,
    skipped_entries: usize,
}

impl Default for RegistryState {
    fn default() -> Self {
        Self {
            snapshot: Arc::new(RegistrySnapshot::empty()),
            signature: None,
            source: String::new(),
            fetched_at: None,
            skipped_entries: 0,
        }
    }
}

#[derive(Default)]
struct RegistryRuntime {
    state: RwLock<RegistryState>,
    reload: Mutex<()>,
    refresh: Mutex<()>,
}

fn runtime() -> &'static RegistryRuntime {
    static REGISTRY: OnceLock<RegistryRuntime> = OnceLock::new();
    REGISTRY.get_or_init(RegistryRuntime::default)
}

/// No file or network work. Capture one Arc at each report boundary.
pub fn snapshot() -> Arc<RegistrySnapshot> {
    #[cfg(test)]
    {
        return test_snapshot();
    }
    #[cfg(not(test))]
    runtime().snapshot()
}

/// One metadata check per request observes refreshes performed by another
/// native process. Resolvers on the returned Arc perform no I/O.
pub fn refresh_cached_snapshot() -> Arc<RegistrySnapshot> {
    #[cfg(test)]
    {
        return test_snapshot();
    }
    #[cfg(not(test))]
    {
        if let Ok(root) = licoup_foundation::platform::paths::portable_data_dir_read_only() {
            let _ = runtime().reload_at(&root.join("model-registry/catalog.json"));
        }
        runtime().snapshot()
    }
}

/// An explicitly selected state store owns its catalog too. Keep this
/// request's snapshot separate so an empty store cannot inherit the host
/// catalog or another concurrently inspected store's identities.
pub fn refresh_cached_snapshot_for_state_root(state_root: Option<&Path>) -> Arc<RegistrySnapshot> {
    #[cfg(test)]
    {
        let _ = state_root;
        test_snapshot()
    }
    #[cfg(not(test))]
    {
        match state_root {
            Some(root) => {
                let isolated = RegistryRuntime::default();
                let _ = isolated.reload_at(&root.join("model-registry/catalog.json"));
                isolated.snapshot()
            }
            None => refresh_cached_snapshot(),
        }
    }
}

pub fn read() -> Value {
    let _ = refresh_cached_snapshot();
    runtime().summary(true, "ready", None)
}

/// Only explicit local refresh performs public network requests. A download,
/// parse, or cache-write failure leaves the previous valid snapshot intact.
pub fn refresh() -> Value {
    let _ = refresh_cached_snapshot();
    let result =
        licoup_foundation::platform::paths::portable_data_dir_read_only().and_then(|root| {
            runtime().refresh_at(&root.join("model-registry/catalog.json"), source::download)
        });
    match result {
        Ok(changed) => {
            runtime().summary(true, if changed { "refreshed" } else { "unchanged" }, None)
        }
        Err(_) => runtime().summary(false, "unavailable", Some("model_registry_refresh_failed")),
    }
}

impl RegistryRuntime {
    fn snapshot(&self) -> Arc<RegistrySnapshot> {
        Arc::clone(
            &self
                .state
                .read()
                .unwrap_or_else(|error| error.into_inner())
                .snapshot,
        )
    }

    fn reload_at(&self, path: &Path) -> Result<()> {
        let _reload = self
            .reload
            .lock()
            .map_err(|_| anyhow!("model_registry_reload_lock_failed"))?;
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(anyhow!("model_registry_cache_unavailable")),
        };
        let signature = (path.to_owned(), metadata.modified()?, metadata.len());
        if self
            .state
            .read()
            .map_err(|_| anyhow!("model_registry_lock_failed"))?
            .signature
            .as_ref()
            == Some(&signature)
        {
            return Ok(());
        }
        let persisted: PersistedCatalog = serde_json::from_slice(&fs::read(path)?)?;
        let snapshot = Arc::new(RegistrySnapshot::from_document(&persisted.catalog)?);
        let mut state = self
            .state
            .write()
            .map_err(|_| anyhow!("model_registry_lock_failed"))?;
        *state = RegistryState {
            snapshot,
            signature: Some(signature),
            source: persisted.source,
            fetched_at: Some(persisted.fetched_at),
            skipped_entries: persisted.skipped_entries,
        };
        Ok(())
    }

    fn refresh_at(
        &self,
        path: &Path,
        fetch: impl FnOnce() -> Result<source::DownloadedCatalog>,
    ) -> Result<bool> {
        let _refresh = self
            .refresh
            .lock()
            .map_err(|_| anyhow!("model_registry_refresh_lock_failed"))?;
        let downloaded = fetch()?;
        let next = RegistrySnapshot::from_document(&downloaded.catalog)?;
        let changed = next.revision() != self.snapshot().revision();
        let persisted = PersistedCatalog {
            source: downloaded.source,
            fetched_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)?
                .as_millis()
                .to_string(),
            skipped_entries: downloaded.skipped_entries,
            catalog: downloaded.catalog,
        };
        let parent = path
            .parent()
            .ok_or_else(|| anyhow!("model_registry_cache_path_invalid"))?;
        licoup_foundation::platform::file_security::ensure_private_dir(parent)?;
        licoup_foundation::platform::file_security::atomic_write_private_text(
            path,
            &serde_json::to_string(&persisted)?,
        )?;
        let _reload = self
            .reload
            .lock()
            .map_err(|_| anyhow!("model_registry_reload_lock_failed"))?;
        let mut state = self
            .state
            .write()
            .map_err(|_| anyhow!("model_registry_lock_failed"))?;
        *state = RegistryState {
            snapshot: Arc::new(next),
            // Another process can replace the file immediately after our
            // atomic write. Do not attach its metadata to our own snapshot;
            // the next request verifies the on-disk catalog once.
            signature: None,
            source: persisted.source,
            fetched_at: Some(persisted.fetched_at),
            skipped_entries: persisted.skipped_entries,
        };
        Ok(changed)
    }

    fn summary(&self, ok: bool, status: &str, error: Option<&str>) -> Value {
        let state = self
            .state
            .read()
            .unwrap_or_else(|poison| poison.into_inner());
        snapshot_report(
            &state.snapshot,
            ok,
            status,
            &SnapshotProvenance {
                source: state.source.clone(),
                fetched_at: state.fetched_at.clone(),
                skipped_entries: state.skipped_entries,
            },
            error,
        )
    }
}

/// Where a reported snapshot was captured from. Provenance is stated by the
/// caller, never inferred from the snapshot's contents.
#[derive(Clone, Debug, Default)]
pub struct SnapshotProvenance {
    pub source: String,
    pub fetched_at: Option<String>,
    pub skipped_entries: usize,
}

/// The report one captured snapshot produces, with no file or network work.
///
/// The caller states the snapshot and its provenance, so a build that holds a
/// synthetic snapshot reports that one instead of reaching for the host's
/// catalog. It is the same report the cache produces for its current snapshot,
/// which is what keeps the two from drifting.
pub fn snapshot_report(
    snapshot: &RegistrySnapshot,
    ok: bool,
    status: &str,
    provenance: &SnapshotProvenance,
    error: Option<&str>,
) -> Value {
    json!({
        "ok": ok,
        "status": if status == "ready" && snapshot.models.is_empty() { "empty" } else { status },
        "revision": snapshot.revision(),
        "modelCount": snapshot.models.len(),
        "providerCount": snapshot.provider_count,
        "source": provenance.source,
        "fetchedAt": provenance.fetched_at,
        "skippedEntries": provenance.skipped_entries,
        "errorCode": error,
    })
}

#[cfg(test)]
thread_local! {
    static TEST_SNAPSHOT: std::cell::RefCell<Arc<RegistrySnapshot>> =
        std::cell::RefCell::new(Arc::new(RegistrySnapshot::empty()));
}

#[cfg(test)]
fn test_snapshot() -> Arc<RegistrySnapshot> {
    TEST_SNAPSHOT.with(|snapshot| Arc::clone(&snapshot.borrow()))
}

#[cfg(test)]
pub fn with_test_snapshot<T>(snapshot: RegistrySnapshot, action: impl FnOnce() -> T) -> T {
    struct Restore(Option<Arc<RegistrySnapshot>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(previous) = self.0.take() {
                TEST_SNAPSHOT.with(|snapshot| {
                    snapshot.replace(previous);
                });
            }
        }
    }
    let previous = TEST_SNAPSHOT.with(|current| current.replace(Arc::new(snapshot)));
    let _restore = Restore(Some(previous));
    action()
}

#[cfg(test)]
mod tests;
