use anyhow::{Result, anyhow};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

pub use lico_catalog_convergence::*;

static RUNTIME: OnceLock<Mutex<Option<CatalogRuntime>>> = OnceLock::new();

struct CatalogRuntime {
    root: PathBuf,
    engine: lico_catalog_convergence::CatalogConvergenceEngine,
    store: lico_catalog_convergence::CatalogCacheStore,
}

impl CatalogRuntime {
    fn open(root: PathBuf) -> Result<Self> {
        let store = lico_catalog_convergence::CatalogCacheStore::open(root.join("catalog-cache"))?;
        let engine = lico_catalog_convergence::CatalogConvergenceEngine::default();
        for snapshot in store.load_partitions()? {
            let result = engine.restore_partition(snapshot);
            if result.snapshot.is_none() {
                return Err(anyhow!("catalog_cache_restore_failed"));
            }
        }
        engine.begin_reconnect();
        Ok(Self {
            root,
            engine,
            store,
        })
    }

    fn dispatch(&self, args: &[String], params: &Value) -> Result<Value> {
        let operation = args.get(1).map(String::as_str).unwrap_or("status");
        if operation == "purge" {
            if let Some(partition_key) = params
                .get("partitionKey")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                self.store.remove_partition(partition_key)?;
            } else {
                self.store.purge_all()?;
            }
        }
        let result = lico_catalog_convergence::dispatch_with_engine(&self.engine, args, params)?;
        match operation {
            "refresh" if result.get("outcome").and_then(Value::as_str) == Some("replaced") => {
                let snapshot: lico_catalog_convergence::CatalogSnapshot = serde_json::from_value(
                    result
                        .get("snapshot")
                        .cloned()
                        .ok_or_else(|| anyhow!("catalog_refresh_snapshot_missing"))?,
                )?;
                if self.store.persist_partition(&snapshot).is_err() {
                    self.engine.mark_fenced(
                        &snapshot.partition_key,
                        snapshot.audience_revision,
                        &snapshot.catalog_revision,
                        snapshot.source_revision,
                    );
                    return Err(anyhow!("catalog_cache_persist_failed"));
                }
            }
            _ => {}
        }
        Ok(result)
    }
}

pub fn dispatch(args: &[String], params: &Value) -> Result<Value> {
    let root = licoup_foundation::platform::paths::portable_data_dir()?;
    let runtime = RUNTIME.get_or_init(|| Mutex::new(None));
    let mut guard = runtime
        .lock()
        .map_err(|_| anyhow!("catalog_runtime_lock_failed"))?;
    if guard.as_ref().is_none_or(|current| current.root != root) {
        *guard = Some(CatalogRuntime::open(root)?);
    }
    guard
        .as_ref()
        .ok_or_else(|| anyhow!("catalog_runtime_unavailable"))?
        .dispatch(args, params)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn runtime_restores_persisted_catalog_fenced_and_purge_cannot_resurrect_it() {
        let base =
            std::env::temp_dir().join(format!("lico-catalog-runtime-{}", std::process::id()));
        let first = base.join("first");
        let second = base.join("second");
        let previous =
            licoup_foundation::platform::paths::set_portable_data_dir_override(Some(first.clone()));
        let refresh = dispatch(
            &["catalog".into(), "refresh".into()],
            &json!({
                "partitionKey": "opaque-a",
                "sourceRevision": 2,
                "catalogRevision": "catalog-2",
                "audienceRevision": 3,
                "tools": [{"name": "upstream.synthetic"}]
            }),
        )
        .unwrap();
        assert_eq!(refresh["outcome"], "replaced");

        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(second));
        dispatch(&["catalog".into(), "status".into()], &json!({})).unwrap();
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(first.clone()));
        let restored = dispatch(&["catalog".into(), "status".into()], &json!({})).unwrap();
        assert_eq!(restored["partitionCount"], 1);
        assert_eq!(restored["reconnectFence"], true);
        let blocked = dispatch(
            &["catalog".into(), "list".into()],
            &json!({"partitionKey": "opaque-a"}),
        )
        .unwrap();
        assert_eq!(blocked["ok"], false);

        dispatch(&["catalog".into(), "purge".into()], &json!({})).unwrap();
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(
            base.join("third"),
        ));
        dispatch(&["catalog".into(), "status".into()], &json!({})).unwrap();
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(first));
        let purged = dispatch(&["catalog".into(), "status".into()], &json!({})).unwrap();
        assert_eq!(purged["partitionCount"], 0);

        licoup_foundation::platform::paths::set_portable_data_dir_override(previous);
        let _ = std::fs::remove_dir_all(base);
    }
}
