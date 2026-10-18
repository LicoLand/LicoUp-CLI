use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde_json::{Map, Value};

use super::catalog::TargetCandidate;

/// The Codex quota-dependent reserve row. It is a live availability signal,
/// not a durable model, so the persisted cache never keeps it.
const CODEX_RESERVE_MODEL: &str = "gpt-reserve";
use super::model_catalog::{BUILTIN_FALLBACK_SOURCE, builtin_cold_start_catalog};
use crate::port::AgentTargetPort;
use licoup_client_state::{
    ClientStateStore, TARGET_DISCOVERY_CACHE_SCHEMA as CACHE_SCHEMA, TargetRouteRecord,
};

pub(super) fn persist_discovery_cache(
    store: &ClientStateStore,
    candidates: &[TargetCandidate],
) -> Result<()> {
    let archived = archived_model_catalogs(store);
    let cached_at = now_epoch_seconds();
    let records = candidates
        .iter()
        .filter_map(|candidate| {
            cache_record(
                candidate,
                cached_at,
                archived.get(candidate.target.as_str()),
            )
        })
        .collect::<Vec<_>>();
    store.write_target_routes(&records)
}

pub(super) fn upsert_discovery_cache(
    store: &ClientStateStore,
    candidate: &TargetCandidate,
) -> Result<()> {
    upsert_discovery_cache_many(store, &[candidate])
}

pub(super) fn upsert_discovery_cache_many(
    store: &ClientStateStore,
    candidates: &[&TargetCandidate],
) -> Result<()> {
    let selected = candidates
        .iter()
        .map(|candidate| candidate.target.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let cached_at = now_epoch_seconds();
    store.update_target_routes(|records| {
        let archived = records
            .iter()
            .filter_map(|record| {
                persisted_scan_catalog(record.extension.get("modelCatalog"))
                    .map(|catalog| (record.target.clone(), catalog))
            })
            .collect::<BTreeMap<_, _>>();
        records.retain(|record| !selected.contains(record.target.as_str()));
        for &candidate in candidates {
            if let Some(record) = cache_record(
                candidate,
                cached_at,
                archived.get(candidate.target.as_str()),
            ) {
                records.push(record);
            }
        }
        records.sort_by(|left, right| left.target.cmp(&right.target));
        Ok(())
    })
}

pub(super) fn cached_runtime_executable(store: &ClientStateStore, target: &str) -> Option<PathBuf> {
    let record = store.target_route(target).ok()??;
    if !record.runtime_ready {
        return None;
    }
    let path = PathBuf::from(record.binary_path?);
    if !path.is_absolute() || !path.is_file() {
        return None;
    }
    fs::canonicalize(path).ok()
}

pub(super) fn hydrate_model_catalogs(
    port: &AgentTargetPort,
    store: &ClientStateStore,
    candidates: &mut [TargetCandidate],
) {
    for candidate in candidates {
        hydrate_model_catalog(port, store, candidate);
    }
}

pub(super) fn hydrate_model_catalog(
    port: &AgentTargetPort,
    store: &ClientStateStore,
    candidate: &mut TargetCandidate,
) {
    if model_catalog_has_models(candidate.model_catalog.as_ref()) {
        return;
    }
    if let Ok(Some(record)) = store.target_route(&candidate.target)
        && let Some(archived) = persisted_scan_catalog(record.extension.get("modelCatalog"))
    {
        candidate.model_catalog = Some(archived);
        return;
    }
    if candidate_is_detected(candidate)
        && let Some(fallback) = builtin_cold_start_catalog(port, &candidate.target)
    {
        candidate.model_catalog = Some(fallback);
    }
}

fn cache_record(
    candidate: &TargetCandidate,
    cached_at: u64,
    archived_catalog: Option<&Value>,
) -> Option<TargetRouteRecord> {
    if candidate.location != "local" {
        return None;
    }
    let binary_path = candidate.binary_path.clone();
    let config_path = (candidate.configured || candidate.manual)
        .then(|| candidate.config_path.clone())
        .flatten();
    if binary_path.is_none() && config_path.is_none() {
        return None;
    }
    let mut extension = Map::new();
    if let Some(catalog) =
        persistable_model_catalog(candidate.model_catalog.as_ref(), archived_catalog)
    {
        extension.insert("modelCatalog".to_string(), catalog);
    }
    Some(TargetRouteRecord {
        schema_version: CACHE_SCHEMA.to_string(),
        target: candidate.target.clone(),
        binary_path,
        config_path,
        scan_source: candidate
            .scan_source
            .clone()
            .unwrap_or_else(|| "host-local-discovery".to_string()),
        runtime_ready: candidate
            .supported_actions
            .iter()
            .any(|action| action == "runtime.message.send"),
        cached_at_epoch_seconds: cached_at,
        extension,
    })
}

fn archived_model_catalogs(store: &ClientStateStore) -> BTreeMap<String, Value> {
    store
        .read_target_routes()
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|record| {
            persisted_scan_catalog(record.extension.get("modelCatalog"))
                .map(|catalog| (record.target, catalog))
        })
        .collect()
}

fn persistable_model_catalog(live: Option<&Value>, archived: Option<&Value>) -> Option<Value> {
    persisted_scan_catalog(live).or_else(|| archived.cloned())
}

fn persisted_scan_catalog(catalog: Option<&Value>) -> Option<Value> {
    let catalog = catalog?;
    if !model_catalog_has_models(Some(catalog)) || is_builtin_fallback(catalog) {
        return None;
    }
    let mut persisted = catalog.clone();
    let codex_catalog = catalog
        .get("sources")
        .and_then(Value::as_array)
        .is_some_and(|sources| {
            sources.iter().any(|source| {
                source
                    .as_str()
                    .is_some_and(|value| value.eq_ignore_ascii_case("codex-app-server"))
            })
        });
    let models = persisted.get_mut("models")?.as_array_mut()?;
    models.retain(|model| {
        model.get("ephemeral").and_then(Value::as_bool) != Some(true)
            && model
                .get("name")
                .and_then(Value::as_str)
                .is_none_or(|name| {
                    !codex_catalog || !name.eq_ignore_ascii_case(CODEX_RESERVE_MODEL)
                })
    });
    if models.is_empty() {
        return None;
    }
    Some(persisted)
}

fn model_catalog_has_models(catalog: Option<&Value>) -> bool {
    catalog
        .and_then(|value| value.get("models"))
        .and_then(Value::as_array)
        .is_some_and(|models| !models.is_empty())
}

fn is_builtin_fallback(catalog: &Value) -> bool {
    catalog
        .get("sources")
        .and_then(Value::as_array)
        .is_some_and(|sources| {
            !sources.is_empty()
                && sources
                    .iter()
                    .all(|source| source.as_str() == Some(BUILTIN_FALLBACK_SOURCE))
        })
}

fn candidate_is_detected(candidate: &TargetCandidate) -> bool {
    candidate.configured
        || candidate.manual
        || matches!(
            candidate.status.as_str(),
            "detected" | "configured" | "available"
        )
}

fn now_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::targets::catalog::{AdapterCapabilities, TargetCandidate};
    use licoup_client_state::TARGET_DISCOVERY_CACHE_COLLECTION as COLLECTION;
    use serde_json::{Value, json};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Arc, Barrier};

    fn temp_test_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lico-targets-{}-{}", name, uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn route(target: &str, binary_path: Option<&str>) -> TargetRouteRecord {
        TargetRouteRecord {
            schema_version: CACHE_SCHEMA.to_string(),
            target: target.to_string(),
            binary_path: binary_path.map(str::to_string),
            config_path: None,
            scan_source: "executable-path".to_string(),
            runtime_ready: true,
            cached_at_epoch_seconds: 1,
            extension: Map::new(),
        }
    }

    fn candidate(binary_path: Option<String>, config_path: Option<String>) -> TargetCandidate {
        TargetCandidate {
            id: Some("codex".to_string()),
            target: "codex".to_string(),
            label: "Codex".to_string(),
            kind: "cli".to_string(),
            status: "configured".to_string(),
            configured: config_path.is_some(),
            confidence: 1.0,
            detail: "must-not-be-cached".to_string(),
            config_path,
            binary_path,
            history_roots: vec!["must-not-be-cached".to_string()],
            location: "local".to_string(),
            runtime_connection: None,
            manual: false,
            adapter_status: "implemented".to_string(),
            adapter_capabilities: AdapterCapabilities {
                detection: "implemented".to_string(),
                config_read: "unsupported".to_string(),
                config_plan: "unsupported".to_string(),
                config_apply: "unsupported".to_string(),
                rollback: "unsupported".to_string(),
                official_cli: "unknown".to_string(),
                conversation_driver: "implemented".to_string(),
                conversation_protocol: "acp".to_string(),
                conversation_readiness: "ready".to_string(),
                conversation_blocker: None,
                conversation_probe: json!({}),
                conversation_capability_matrix: Value::Null,
                conversation_summary_codes: Vec::new(),
                conversation_consecutive_passes: 0,
                conversation_evidence_age: String::new(),
            },
            supported_actions: vec!["runtime.message.send".to_string()],
            scan_source: Some("executable-path".to_string()),
            model_catalog: Some(json!({
                "schemaVersion": 1,
                "status": "available",
                "sources": ["antigravity-cli"],
                "models": [{ "name": "gemini-3" }],
                "diagnostics": []
            })),
        }
    }

    #[test]
    fn cache_drops_quota_dependent_reserve_rows() {
        let root = temp_test_dir("drop-reserve-row");
        let store = ClientStateStore::new(root.join("client-state")).unwrap();
        let mut item = candidate(
            Some(root.join("codex").to_string_lossy().into_owned()),
            Some(root.join("config.toml").to_string_lossy().into_owned()),
        );
        item.model_catalog = Some(json!({
            "schemaVersion": 1,
            "status": "available",
            "sources": ["codex-app-server"],
            "models": [
                {"name": "gpt-6-luna"},
                {"name": "gpt-reserve"},
                {"name": "gpt-reserve", "ephemeral": true}
            ],
            "diagnostics": []
        }));
        persist_discovery_cache(&store, &[item]).unwrap();
        let document = store.read_collection(COLLECTION).unwrap();
        let models = document["items"][0]["modelCatalog"]["models"]
            .as_array()
            .unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0]["name"], "gpt-6-luna");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cache_persists_scanned_model_catalog_and_drops_ephemeral_fields() {
        let root = std::env::temp_dir().join(format!("lico-target-cache-{}", uuid::Uuid::new_v4()));
        let store = ClientStateStore::new(root.clone()).unwrap();
        persist_discovery_cache(
            &store,
            &[candidate(
                Some(root.join("codex").to_string_lossy().into_owned()),
                Some(root.join("config.toml").to_string_lossy().into_owned()),
            )],
        )
        .unwrap();
        let document = store.read_collection(COLLECTION).unwrap();
        let item = &document["items"][0];
        assert_eq!(item["schemaVersion"], CACHE_SCHEMA);
        assert!(item.get("detail").is_none());
        assert!(item.get("historyRoots").is_none());
        assert_eq!(item["modelCatalog"]["models"][0]["name"], "gemini-3");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn empty_upsert_preserves_archived_model_catalog() {
        let dir = temp_test_dir("preserve-archive");
        let store = ClientStateStore::new(dir.join("client-state")).unwrap();
        let first = candidate(Some(dir.join("codex").to_string_lossy().into_owned()), None);
        persist_discovery_cache(&store, &[first]).unwrap();

        let mut empty = candidate(Some(dir.join("codex").to_string_lossy().into_owned()), None);
        empty.model_catalog = Some(json!({
            "schemaVersion": 1,
            "status": "empty",
            "sources": ["antigravity-cli"],
            "models": [],
            "diagnostics": []
        }));
        upsert_discovery_cache(&store, &empty).unwrap();

        let route = store.target_route("codex").unwrap().unwrap();
        assert_eq!(
            route.extension.get("modelCatalog").unwrap()["models"][0]["name"],
            "gemini-3"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn hydrate_restores_archive_and_skips_builtin_when_archive_exists() {
        let dir = temp_test_dir("hydrate-archive");
        let store = ClientStateStore::new(dir.join("client-state")).unwrap();
        let mut archived = candidate(
            Some(dir.join("antigravity").to_string_lossy().into_owned()),
            None,
        );
        archived.target = "antigravity".to_string();
        archived.id = Some("antigravity".to_string());
        persist_discovery_cache(&store, &[archived]).unwrap();

        let mut live = candidate(
            Some(dir.join("antigravity").to_string_lossy().into_owned()),
            None,
        );
        live.target = "antigravity".to_string();
        live.id = Some("antigravity".to_string());
        live.status = "detected".to_string();
        live.model_catalog = Some(json!({
            "schemaVersion": 1,
            "status": "empty",
            "sources": ["antigravity-cli"],
            "models": [],
            "diagnostics": []
        }));
        hydrate_model_catalog(&crate::port::fixtures::packaged(), &store, &mut live);
        let catalog = live.model_catalog.as_ref().unwrap();
        assert_eq!(catalog["models"][0]["name"], "gemini-3");
        assert_ne!(
            catalog["sources"]
                .as_array()
                .and_then(|sources| sources.first())
                .and_then(Value::as_str),
            Some(BUILTIN_FALLBACK_SOURCE)
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn hydrate_uses_builtin_only_when_detected_and_no_archive() {
        let dir = temp_test_dir("hydrate-builtin");
        let store = ClientStateStore::new(dir.join("client-state")).unwrap();
        let mut live = candidate(Some(dir.join("codex").to_string_lossy().into_owned()), None);
        live.status = "detected".to_string();
        live.model_catalog = Some(json!({
            "schemaVersion": 1,
            "status": "empty",
            "sources": ["config"],
            "models": [],
            "diagnostics": []
        }));
        hydrate_model_catalog(&crate::port::fixtures::packaged(), &store, &mut live);
        let catalog = live.model_catalog.as_ref().unwrap();
        assert!(
            catalog["models"]
                .as_array()
                .is_some_and(|models| !models.is_empty())
        );
        assert_eq!(catalog["sources"], json!([BUILTIN_FALLBACK_SOURCE]));
        persist_discovery_cache(&store, std::slice::from_ref(&live)).unwrap();
        let route = store.target_route("codex").unwrap().unwrap();
        assert!(route.extension.get("modelCatalog").is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn cache_drops_undetected_targets_without_local_routes() {
        let root =
            std::env::temp_dir().join(format!("lico-target-cache-empty-{}", uuid::Uuid::new_v4()));
        let store = ClientStateStore::new(root.clone()).unwrap();
        let mut undetected = candidate(None, None);
        undetected.configured = false;
        undetected.status = "not-detected".to_string();
        persist_discovery_cache(&store, &[undetected]).unwrap();
        assert_eq!(
            store.read_collection(COLLECTION).unwrap()["items"],
            json!([])
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cached_runtime_executable_resolves_through_the_store_owner() {
        let dir = temp_test_dir("binding-keyed");
        let executable = dir.join("codex");
        fs::write(&executable, "").unwrap();
        let store = ClientStateStore::new(dir.join("client-state")).unwrap();
        store
            .write_target_routes(&[route("codex", Some(executable.to_string_lossy().as_ref()))])
            .unwrap();

        assert_eq!(
            cached_runtime_executable(&store, "codex"),
            Some(fs::canonicalize(&executable).unwrap())
        );
        assert_eq!(
            cached_runtime_executable(&store, "codex"),
            Some(fs::canonicalize(&executable).unwrap())
        );
        assert!(cached_runtime_executable(&store, "missing").is_none());
        // Write-through installs the projection; local writes never reparse.
        assert_eq!(store.target_index_parse_count(), 1);
        store
            .write_target_routes(&[route("codex", Some(executable.to_string_lossy().as_ref()))])
            .unwrap();
        assert_eq!(store.target_index_parse_count(), 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn cached_runtime_executable_reparses_once_per_file_generation() {
        let dir = temp_test_dir("binding-reparse");
        let executable = dir.join("codex");
        fs::write(&executable, "").unwrap();
        let writer = ClientStateStore::new(dir.join("client-state")).unwrap();
        writer
            .write_target_routes(&[route("codex", Some(executable.to_string_lossy().as_ref()))])
            .unwrap();

        let reader = ClientStateStore::new(dir.join("client-state")).unwrap();
        assert!(cached_runtime_executable(&reader, "codex").is_some());
        assert!(cached_runtime_executable(&reader, "codex").is_some());
        assert_eq!(reader.target_index_parse_count(), 1);
        assert_eq!(reader.target_index_invalidation_count(), 0);

        writer.write_target_routes(&[]).unwrap();
        assert!(cached_runtime_executable(&reader, "codex").is_none());
        assert_eq!(reader.target_index_parse_count(), 2);
        assert_eq!(reader.target_index_invalidation_count(), 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn cached_runtime_executable_rejects_unready_or_unresolvable_routes() {
        let dir = temp_test_dir("binding-reject");
        let executable = dir.join("codex");
        fs::write(&executable, "").unwrap();
        let store = ClientStateStore::new(dir.join("client-state")).unwrap();

        let mut unready = route("codex", Some(executable.to_string_lossy().as_ref()));
        unready.runtime_ready = false;
        store.write_target_routes(&[unready]).unwrap();
        assert!(cached_runtime_executable(&store, "codex").is_none());

        store
            .write_target_routes(&[route("codex", Some(executable.to_string_lossy().as_ref()))])
            .unwrap();
        assert!(cached_runtime_executable(&store, "codex").is_some());

        store
            .write_target_routes(&[route("codex", Some("relative/codex"))])
            .unwrap();
        assert!(cached_runtime_executable(&store, "codex").is_none());

        store
            .write_target_routes(&[route(
                "codex",
                Some(dir.join("absent").to_string_lossy().as_ref()),
            )])
            .unwrap();
        assert!(cached_runtime_executable(&store, "codex").is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn upsert_replaces_and_sorts_through_the_typed_projection() {
        let dir = temp_test_dir("upsert-typed");
        let store = ClientStateStore::new(dir.join("client-state")).unwrap();
        let first = candidate(Some(dir.join("codex").to_string_lossy().into_owned()), None);
        let mut second = candidate(Some(dir.join("alpha").to_string_lossy().into_owned()), None);
        second.target = "alpha".to_string();
        second.id = Some("alpha".to_string());
        persist_discovery_cache(&store, &[first, second]).unwrap();

        let mut replacement =
            candidate(Some(dir.join("codex").to_string_lossy().into_owned()), None);
        replacement.scan_source = Some("manual".to_string());
        upsert_discovery_cache(&store, &replacement).unwrap();

        let document = store.read_collection(COLLECTION).unwrap();
        let items = document["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["target"], "alpha");
        assert_eq!(items[1]["target"], "codex");
        assert_eq!(items[1]["scanSource"], "manual");

        let routes = store.read_target_routes().unwrap();
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].target, "alpha");
        assert_eq!(routes[1].target, "codex");
        assert_eq!(routes[1].scan_source, "manual");
        assert_eq!(store.target_index_parse_count(), 1);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn concurrent_selected_scans_preserve_every_cached_agent_route() {
        const TARGET_COUNT: usize = 16;
        let dir = temp_test_dir("concurrent-upserts");
        let state_root = dir.join("client-state");
        let stores = (0..TARGET_COUNT)
            .map(|_| ClientStateStore::new(state_root.clone()).unwrap())
            .collect::<Vec<_>>();
        let start = Arc::new(Barrier::new(TARGET_COUNT));
        let handles = stores
            .into_iter()
            .enumerate()
            .map(|(index, store)| {
                let start = Arc::clone(&start);
                let mut target = candidate(
                    Some(
                        dir.join(format!("agent-{index}"))
                            .to_string_lossy()
                            .into_owned(),
                    ),
                    None,
                );
                target.id = Some(format!("agent-{index}"));
                target.target = format!("agent-{index}");
                std::thread::spawn(move || {
                    start.wait();
                    upsert_discovery_cache(&store, &target).unwrap();
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            handle.join().unwrap();
        }
        let routes = ClientStateStore::new(state_root)
            .unwrap()
            .read_target_routes()
            .unwrap();
        assert_eq!(routes.len(), TARGET_COUNT);
        assert_eq!(
            routes
                .iter()
                .map(|route| route.target.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            TARGET_COUNT
        );
        let _ = fs::remove_dir_all(dir);
    }
}
