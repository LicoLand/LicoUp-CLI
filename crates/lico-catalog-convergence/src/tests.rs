use crate::{
    CATALOG_CONVERGENCE_SCHEMA, CatalogCacheStore, CatalogConvergenceEngine,
    CatalogFetchedSnapshot, CatalogSnapshot, CatalogToolEntry, CohortOutcome,
    InvalidationNotification, OFFICIAL_CLIENT_RECEIPT_SCHEMA, ReceiptContext, RefreshOutcomeKind,
    build_official_client_receipt, dispatch, is_hex_digest, is_opaque_partition_key,
    scan_privacy_text,
};
use std::sync::{Arc, Barrier, Condvar, Mutex};
use std::thread;
use std::time::Duration;

fn tool(name: impl Into<String>) -> CatalogToolEntry {
    CatalogToolEntry::named(name)
}

fn described_tool(name: &str, description: &str) -> CatalogToolEntry {
    serde_json::from_value(serde_json::json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "required": ["query"] },
        "annotations": { "readOnlyHint": true }
    }))
    .expect("tool descriptor")
}

fn synthetic_unix_absolute_path(root: &str, segments: &[&str]) -> String {
    let separator = '/';
    let suffix = segments.join(&separator.to_string());
    format!("{separator}{root}{separator}{suffix}")
}

fn synthetic_windows_absolute_path(drive: char, segments: &[&str]) -> String {
    let separator = char::from(92);
    let suffix = segments.join(&separator.to_string());
    format!("{drive}:{separator}{suffix}")
}

#[test]
fn schema_constants_match_checkpoint_contract() {
    assert_eq!(
        CATALOG_CONVERGENCE_SCHEMA,
        "v0.0.1:licoup:catalog-convergence-1"
    );
    assert_eq!(
        OFFICIAL_CLIENT_RECEIPT_SCHEMA,
        "v0.0.1:upstream-gateway:official-client-receipt-1"
    );
}

#[test]
fn rejects_stale_rollback_and_exposes_ui_revision() {
    let engine = CatalogConvergenceEngine::default();
    assert_eq!(
        engine
            .replace_partition(
                "p1",
                CatalogFetchedSnapshot {
                    source_revision: 2,
                    catalog_revision: "c2".to_string(),
                    audience_revision: 4,
                    tools: vec![tool("upstream.a".to_string())],
                }
            )
            .outcome,
        RefreshOutcomeKind::Replaced
    );
    assert_eq!(
        engine
            .replace_partition(
                "p1",
                CatalogFetchedSnapshot {
                    source_revision: 1,
                    catalog_revision: "c1".to_string(),
                    audience_revision: 3,
                    tools: vec![tool("upstream.stale".to_string())],
                }
            )
            .outcome,
        RefreshOutcomeKind::RejectedStale
    );
    assert_eq!(engine.list_tools("p1").tools[0].name, "upstream.a");
    assert_eq!(engine.ui_observed_revision(), -1);
    assert_eq!(engine.observe_ui_revision("p1"), Some(4));
    assert_eq!(engine.ui_observed_revision(), 4);
}

#[test]
fn invalidation_blocks_discovery_until_refresh_and_purge_clears_state() {
    let engine = CatalogConvergenceEngine::default();
    engine.replace_partition(
        "p1",
        CatalogFetchedSnapshot {
            source_revision: 2,
            catalog_revision: "c2".to_string(),
            audience_revision: 4,
            tools: vec![tool("upstream.a".to_string())],
        },
    );
    engine.apply_invalidation(InvalidationNotification {
        affected_partitions: vec!["p1".to_string()],
        partition_key: None,
        source_revision: 3,
        catalog_revision: "c3".to_string(),
        audience_revision: 5,
        reason_code: String::new(),
    });
    assert!(engine.discovery_blocked());
    assert_eq!(
        engine.list_tools("p1").reason_code,
        "catalog_reconciliation_required"
    );
    engine.replace_partition(
        "p1",
        CatalogFetchedSnapshot {
            source_revision: 3,
            catalog_revision: "c3".to_string(),
            audience_revision: 5,
            tools: vec![tool("upstream.b".to_string())],
        },
    );
    assert!(!engine.discovery_blocked());
    assert_eq!(engine.list_tools("p1").tools[0].name, "upstream.b");
    engine.purge_all();
    assert_eq!(
        engine.list_tools("p1").reason_code,
        "catalog_partition_missing"
    );
    assert_eq!(engine.ui_observed_revision(), -1);
}

#[test]
fn receipt_builder_rejects_privacy_unsafe_values() {
    let base = ReceiptContext {
        target: "macos".to_string(),
        platform: "desktop".to_string(),
        runtime: "native".to_string(),
        source_digest: "a".repeat(64),
        negotiated_capability: "upstream.catalog.list_changed".to_string(),
        opaque_partition_key: "b".repeat(64),
        source_revision: 5,
        catalog_revision: "catalog-r5".to_string(),
        audience_revision: 7,
        applied_revision: 7,
        cache_digest: "c".repeat(64),
        cohort_outcome: "applied".to_string(),
        ui_observed_revision: 7,
        restart_ok: true,
        restart_reason_code: "restart_recovered".to_string(),
        observed_at: Some("2026-07-16T00:00:00.000Z".to_string()),
    };
    assert!(build_official_client_receipt(base).is_ok());
    assert!(scan_privacy_text("Bearer secret-token").is_err());
    assert!(scan_privacy_text("-----BEGIN CERTIFICATE-----").is_err());
    for path in [
        synthetic_unix_absolute_path("Users", &["sample", "file"]),
        synthetic_unix_absolute_path("home", &["sample", "file"]),
        synthetic_unix_absolute_path("private", &["sample", "file"]),
        synthetic_windows_absolute_path('C', &["Users", "sample"]),
    ] {
        assert!(scan_privacy_text(&path).is_err());
    }
}

#[test]
fn privacy_scan_does_not_reject_relative_or_similarly_named_paths() {
    for path in [
        "Users/sample/file",
        "home/sample/file",
        "private/sample/file",
        "C:Users",
    ] {
        assert!(scan_privacy_text(path).is_ok());
    }
    assert!(scan_privacy_text("é").is_ok());
}

#[test]
fn digest_checks_require_hex_source_cache_and_opaque_partition_key() {
    assert!(is_hex_digest(&"a".repeat(64)));
    assert!(!is_hex_digest(&"A".repeat(64)));
    assert!(!is_hex_digest("short"));
    assert!(is_opaque_partition_key(&"b".repeat(64)));
    assert!(is_opaque_partition_key(&"b".repeat(43)));
    assert!(!is_opaque_partition_key("short"));
}

#[test]
fn cache_store_atomic_write_survives_restart_restore() {
    let root = std::env::temp_dir().join(format!("lico-catalog-cache-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let store = CatalogCacheStore::open(root.clone()).expect("open store");
    let snapshot = CatalogSnapshot::from_fetched(
        "p1",
        &CatalogFetchedSnapshot {
            source_revision: 2,
            catalog_revision: "c2".to_string(),
            audience_revision: 4,
            tools: vec![tool("upstream.a".to_string())],
        },
        "2026-07-16T00:00:00Z",
    );
    store.persist_partition(&snapshot).expect("persist");
    drop(store);
    let restored = CatalogCacheStore::open(root.clone()).expect("reopen");
    let partitions = restored.load_partitions().expect("load");
    assert_eq!(partitions.len(), 1);
    assert_eq!(partitions[0].partition_key, "p1");
    let engine = CatalogConvergenceEngine::default();
    assert_eq!(
        engine.restore_partition(partitions[0].clone()).outcome,
        RefreshOutcomeKind::Replaced
    );
    assert_eq!(engine.list_tools("p1").tools[0].name, "upstream.a");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn cache_preserves_complete_tool_descriptors_and_digest_covers_them() {
    let first = CatalogSnapshot::from_fetched(
        "p1",
        &CatalogFetchedSnapshot {
            source_revision: 1,
            catalog_revision: "c1".to_string(),
            audience_revision: 1,
            tools: vec![described_tool("upstream.search", "Search one")],
        },
        "2026-07-16T00:00:00Z",
    );
    let second = CatalogSnapshot::from_fetched(
        "p1",
        &CatalogFetchedSnapshot {
            source_revision: 2,
            catalog_revision: "c2".to_string(),
            audience_revision: 2,
            tools: vec![described_tool("upstream.search", "Search two")],
        },
        "2026-07-16T00:00:01Z",
    );
    assert_ne!(first.digest, second.digest);

    let engine = CatalogConvergenceEngine::default();
    assert_eq!(
        engine.restore_partition(second).outcome,
        RefreshOutcomeKind::Replaced
    );
    let listed = engine.list_tools("p1");
    assert_eq!(listed.tools[0].descriptor["description"], "Search two");
    assert_eq!(
        listed.tools[0].descriptor["annotations"]["readOnlyHint"],
        true
    );
}

#[test]
fn cache_store_rejects_tampered_snapshot_metadata() {
    let root = std::env::temp_dir().join(format!(
        "lico-catalog-cache-tamper-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let store = CatalogCacheStore::open(root.clone()).expect("open store");
    let snapshot = CatalogSnapshot::from_fetched(
        "p1",
        &CatalogFetchedSnapshot {
            source_revision: 1,
            catalog_revision: "c1".to_string(),
            audience_revision: 1,
            tools: vec![tool("upstream.a")],
        },
        "2026-07-16T00:00:00Z",
    );
    store.persist_partition(&snapshot).expect("persist");
    let path = root.join("partitions.json");
    let mut envelope: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read store")).expect("parse store");
    envelope["partitions"][0]["toolCount"] = serde_json::json!(99);
    std::fs::write(
        &path,
        serde_json::to_vec(&envelope).expect("serialize store"),
    )
    .expect("tamper store");
    assert!(store.load_partitions().is_err());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn dispatch_status_reports_engine_state() {
    let value = dispatch(
        &["catalog".to_string(), "status".to_string()],
        &serde_json::json!({}),
    )
    .expect("dispatch status");
    assert_eq!(
        value.get("schemaVersion").and_then(|v| v.as_str()),
        Some(CATALOG_CONVERGENCE_SCHEMA)
    );
}

#[test]
fn reconnect_fence_and_cohort_markers_are_observable() {
    let engine = CatalogConvergenceEngine::default();
    engine.replace_partition(
        "p1",
        CatalogFetchedSnapshot {
            source_revision: 1,
            catalog_revision: "c1".to_string(),
            audience_revision: 1,
            tools: vec![tool("upstream.a".to_string())],
        },
    );
    engine.begin_reconnect();
    assert!(engine.discovery_blocked());
    engine.mark_fenced("p1", 2, "c2", 2);
    assert_eq!(engine.cohort_snapshot()[0].1.outcome, CohortOutcome::Fenced);
    engine.mark_disconnected("p2", 2);
    assert!(
        engine
            .cohort_snapshot()
            .iter()
            .any(|(_, e)| e.outcome == CohortOutcome::Disconnected)
    );
}

#[test]
fn duplicate_invalidation_is_ignored_when_older() {
    let engine = CatalogConvergenceEngine::default();
    engine.replace_partition(
        "p1",
        CatalogFetchedSnapshot {
            source_revision: 2,
            catalog_revision: "c2".to_string(),
            audience_revision: 4,
            tools: vec![tool("upstream.a".to_string())],
        },
    );
    engine.apply_invalidation(InvalidationNotification {
        affected_partitions: vec!["p1".to_string()],
        partition_key: None,
        source_revision: 3,
        catalog_revision: "c3".to_string(),
        audience_revision: 5,
        reason_code: String::new(),
    });
    let second = engine.apply_invalidation(InvalidationNotification {
        affected_partitions: vec!["p1".to_string()],
        partition_key: None,
        source_revision: 2,
        catalog_revision: "c2".to_string(),
        audience_revision: 4,
        reason_code: String::new(),
    });
    assert!(second.accepted_partition_keys.is_empty());
}

#[test]
fn pending_and_reconnect_fences_are_partition_scoped() {
    let engine = CatalogConvergenceEngine::default();
    for key in ["p1", "p2"] {
        engine.replace_partition(
            key,
            CatalogFetchedSnapshot {
                source_revision: 1,
                catalog_revision: "c1".to_string(),
                audience_revision: 1,
                tools: vec![tool(format!("upstream.{key}"))],
            },
        );
    }
    engine.apply_invalidation(InvalidationNotification {
        affected_partitions: vec!["p1".to_string()],
        partition_key: None,
        source_revision: 2,
        catalog_revision: "c2".to_string(),
        audience_revision: 2,
        reason_code: String::new(),
    });
    assert!(!engine.list_tools("p1").ok);
    assert!(engine.list_tools("p2").ok);

    engine.replace_partition(
        "p1",
        CatalogFetchedSnapshot {
            source_revision: 2,
            catalog_revision: "c2".to_string(),
            audience_revision: 2,
            tools: vec![tool("upstream.p1-new".to_string())],
        },
    );
    engine.begin_reconnect();
    engine.replace_partition(
        "p1",
        CatalogFetchedSnapshot {
            source_revision: 3,
            catalog_revision: "c3".to_string(),
            audience_revision: 3,
            tools: vec![tool("upstream.p1-newer".to_string())],
        },
    );
    assert!(engine.list_tools("p1").ok);
    assert!(!engine.list_tools("p2").ok);
}

#[test]
fn rejects_same_revision_conflicts_and_capacity_overflow_without_eviction() {
    let engine = CatalogConvergenceEngine::new(1);
    assert_eq!(
        engine
            .replace_partition(
                "p1",
                CatalogFetchedSnapshot {
                    source_revision: 1,
                    catalog_revision: "c1".to_string(),
                    audience_revision: 1,
                    tools: vec![tool("upstream.a".to_string())],
                }
            )
            .outcome,
        RefreshOutcomeKind::Replaced
    );
    assert_eq!(
        engine
            .replace_partition(
                "p1",
                CatalogFetchedSnapshot {
                    source_revision: 1,
                    catalog_revision: "conflict".to_string(),
                    audience_revision: 1,
                    tools: vec![tool("upstream.b".to_string())],
                }
            )
            .outcome,
        RefreshOutcomeKind::RejectedConflict
    );
    assert_eq!(
        engine
            .replace_partition(
                "p2",
                CatalogFetchedSnapshot {
                    source_revision: 2,
                    catalog_revision: "c2".to_string(),
                    audience_revision: 2,
                    tools: vec![tool("upstream.b".to_string())],
                }
            )
            .outcome,
        RefreshOutcomeKind::RejectedCapacity
    );
    assert_eq!(engine.list_tools("p1").tools[0].name, "upstream.a");
}

#[test]
fn refresh_cannot_clear_a_newer_invalidation_that_arrives_during_pull() {
    let engine = CatalogConvergenceEngine::default();
    engine.replace_partition(
        "p1",
        CatalogFetchedSnapshot {
            source_revision: 1,
            catalog_revision: "c1".to_string(),
            audience_revision: 1,
            tools: vec![tool("upstream.a".to_string())],
        },
    );
    engine.apply_invalidation(InvalidationNotification {
        affected_partitions: vec!["p1".to_string()],
        partition_key: None,
        source_revision: 2,
        catalog_revision: "c2".to_string(),
        audience_revision: 2,
        reason_code: String::new(),
    });
    let result = engine.refresh_partition("p1", |_| {
        engine.apply_invalidation(InvalidationNotification {
            affected_partitions: vec!["p1".to_string()],
            partition_key: None,
            source_revision: 3,
            catalog_revision: "c3".to_string(),
            audience_revision: 3,
            reason_code: String::new(),
        });
        CatalogFetchedSnapshot {
            source_revision: 2,
            catalog_revision: "c2".to_string(),
            audience_revision: 2,
            tools: vec![tool("upstream.stale".to_string())],
        }
    });
    assert_eq!(result.outcome, RefreshOutcomeKind::RejectedStale);
    assert!(!engine.list_tools("p1").ok);
    assert_eq!(engine.state()["pendingInvalidationCount"], 1);
}

#[test]
fn ui_revision_is_monotonic_across_partitions_and_recomputed_on_remove() {
    let engine = CatalogConvergenceEngine::default();
    engine.replace_partition(
        "high",
        CatalogFetchedSnapshot {
            source_revision: 8,
            catalog_revision: "c8".to_string(),
            audience_revision: 8,
            tools: vec![tool("upstream.high")],
        },
    );
    engine.replace_partition(
        "low",
        CatalogFetchedSnapshot {
            source_revision: 2,
            catalog_revision: "c2".to_string(),
            audience_revision: 2,
            tools: vec![tool("upstream.low")],
        },
    );
    assert_eq!(engine.ui_observed_revision(), -1);
    assert_eq!(engine.observe_ui_revision("high"), Some(8));
    assert_eq!(engine.observe_ui_revision("low"), Some(2));
    assert_eq!(engine.ui_observed_revision(), 8);
    assert!(engine.remove_partition("high"));
    assert_eq!(engine.ui_observed_revision(), 2);
}

#[test]
fn fetch_panic_completes_all_coalesced_waiters_and_clears_inflight() {
    let engine = Arc::new(CatalogConvergenceEngine::default());
    engine.replace_partition(
        "p1",
        CatalogFetchedSnapshot {
            source_revision: 1,
            catalog_revision: "c1".to_string(),
            audience_revision: 1,
            tools: vec![tool("upstream.a")],
        },
    );
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let first_engine = Arc::clone(&engine);
    let first_entered = Arc::clone(&entered);
    let first_release = Arc::clone(&release);
    let first = thread::spawn(move || {
        first_engine.refresh_partition("p1", |_| {
            first_entered.wait();
            let (lock, cv) = &*first_release;
            let mut allowed = lock.lock().expect("release");
            while !*allowed {
                allowed = cv.wait(allowed).expect("release wait");
            }
            panic!("synthetic fetch failure")
        })
    });
    entered.wait();
    let second_engine = Arc::clone(&engine);
    let second = thread::spawn(move || {
        second_engine.refresh_partition("p1", |_| panic!("coalesced fetcher must not run"))
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while engine.refresh_waiter_count("p1") < 1 {
        assert!(
            std::time::Instant::now() < deadline,
            "waiter did not coalesce"
        );
        thread::sleep(Duration::from_millis(5));
    }
    {
        let (lock, cv) = &*release;
        *lock.lock().expect("release") = true;
        cv.notify_all();
    }
    assert_eq!(
        first.join().expect("first join").outcome,
        RefreshOutcomeKind::FetchFailed
    );
    assert_eq!(
        second.join().expect("second join").outcome,
        RefreshOutcomeKind::FetchFailed
    );
    assert_eq!(engine.state()["inFlightCount"], 0);
    assert_eq!(engine.list_tools("p1").tools[0].name, "upstream.a");
}

#[test]
fn refresh_coalesces_one_inflight_per_partition_race_safe() {
    let engine = Arc::new(CatalogConvergenceEngine::default());
    engine.replace_partition(
        "p1",
        CatalogFetchedSnapshot {
            source_revision: 1,
            catalog_revision: "c1".to_string(),
            audience_revision: 1,
            tools: vec![tool("upstream.a".to_string())],
        },
    );
    engine.apply_invalidation(InvalidationNotification {
        affected_partitions: vec!["p1".to_string()],
        partition_key: None,
        source_revision: 2,
        catalog_revision: "c2".to_string(),
        audience_revision: 2,
        reason_code: String::new(),
    });

    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let fetch_count = Arc::new(Mutex::new(0usize));
    let barrier = Arc::new(Barrier::new(2));

    let engine_first = Arc::clone(&engine);
    let gate_first = Arc::clone(&gate);
    let fetch_count_first = Arc::clone(&fetch_count);
    let barrier_first = Arc::clone(&barrier);
    let first = thread::spawn(move || {
        engine_first.refresh_partition("p1", |_| {
            {
                let mut count = fetch_count_first.lock().expect("fetch");
                *count += 1;
            }
            barrier_first.wait();
            let (lock, cv) = &*gate_first;
            let mut allowed = lock.lock().expect("gate");
            while !*allowed {
                allowed = cv.wait(allowed).expect("wait");
            }
            CatalogFetchedSnapshot {
                source_revision: 2,
                catalog_revision: "c2".to_string(),
                audience_revision: 2,
                tools: vec![tool("upstream.b".to_string())],
            }
        })
    });

    barrier.wait();

    let engine_second = Arc::clone(&engine);
    let fetch_count_second = Arc::clone(&fetch_count);
    let second = thread::spawn(move || {
        engine_second.refresh_partition("p1", |_| {
            let mut count = fetch_count_second.lock().expect("fetch");
            *count += 1;
            panic!("second fetcher must not run");
        })
    });

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while engine.refresh_waiter_count("p1") < 1 {
        assert!(
            std::time::Instant::now() < deadline,
            "second refresh did not enter coalesce wait"
        );
        thread::sleep(Duration::from_millis(5));
    }

    {
        let (lock, cv) = &*gate;
        let mut allowed = lock.lock().expect("gate");
        *allowed = true;
        cv.notify_all();
    }

    let first_result = first.join().expect("first join");
    let second_result = second.join().expect("second join");
    assert_eq!(first_result.outcome, RefreshOutcomeKind::Replaced);
    assert_eq!(second_result.outcome, RefreshOutcomeKind::Replaced);
    assert_eq!(*fetch_count.lock().expect("fetch"), 1);
    assert_eq!(engine.list_tools("p1").tools[0].name, "upstream.b");
}
