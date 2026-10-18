use super::*;
use crate::state_machines::{strategy_store_artifact, update_handoff};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::{collections::BTreeMap, collections::BTreeSet, fs, path::Path};

// The released source fixture is shared data frozen from tag v0.2.1; unit,
// recovery and standalone migration targets include the same file so the
// released layout is defined exactly once.
include!("../../../../../tests/fixtures/client_state_migration/released_source.rs");
include!("../../../../../tests/fixtures/client_state_migration/owner_layouts.rs");
mod structural;

/// Materialize the shared released source root with the native drivers.
fn seed_released_source_root(root: &Path) {
    seed_released_conversation_store(root);
    seed_released_strategy_store(&root.join(RELEASED_STRATEGY_DATABASE));
    let marker_root = root.join("client-state/migrations/domain-state");
    licoup_foundation::platform::file_security::ensure_private_dir(&marker_root).unwrap();
    for (relative, content) in released_root_files() {
        let path = root.join(&relative);
        if relative == RELEASED_CONVERSATION_COMPLETION {
            fs::write(&path, content).unwrap();
            continue;
        }
        let document: Value = serde_json::from_str(&content).unwrap();
        if relative == RELEASED_INVENTORY_FILE {
            assert_eq!(document["schemaVersion"], RELEASED_INVENTORY_SCHEMA);
        }
        write_json_atomic(&path, &document).unwrap();
    }
}

fn seed_released_conversation_store(root: &Path) {
    let database = root.join(RELEASED_CONVERSATION_DATABASE);
    fs::create_dir_all(database.parent().unwrap()).unwrap();
    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch(RELEASED_CONVERSATION_SCHEMA)
        .unwrap();
    connection
        .execute_batch(RELEASED_CONVERSATION_ROWS)
        .unwrap();
    let version: String = connection
        .query_row(
            "SELECT value FROM schema_meta WHERE key='version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, RELEASED_CONVERSATION_SCHEMA_VERSION);
}

fn seed_released_strategy_store(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let connection = Connection::open(path).unwrap();
    connection.execute_batch(RELEASED_STRATEGY_SCHEMA).unwrap();
    connection.execute_batch(&released_strategy_rows()).unwrap();
    let version: String = connection
        .query_row(
            "SELECT value FROM strategy_meta WHERE key='version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, RELEASED_STRATEGY_META_VERSION);
}

#[cfg(target_os = "macos")]
#[test]
fn protected_custody_upgrade_defers_until_success_and_retries_without_reset() {
    let root =
        std::env::temp_dir().join(format!("licoup-custody-migration-{}", uuid::Uuid::new_v4()));
    admit(&root).unwrap();
    assert!(gateway_credential_migration_pending(&root).unwrap());
    // An installed older frontier has no custody completion receipt. The root
    // names the declared source, which is the one older format this binary admits.
    let ledger_path = root.join("client-state/migrations/ledger.json");
    let frontier = embedded_frontier().unwrap();
    let mut ledger = load_ledger(&ledger_path, &frontier).unwrap();
    ledger.domains.remove(GATEWAY_CUSTODY_DOMAIN);
    ledger.frontier_id = frontier.source_frontier_id.clone();
    write_json_atomic(&ledger_path, &ledger).unwrap();

    let startup = admit(&root).unwrap();
    assert_eq!(
        startup.pending_authorization_domain_ids,
        [GATEWAY_CUSTODY_DOMAIN]
    );
    assert_eq!(startup.status, "ready");
    assert!(gateway_credential_migration_pending(&root).unwrap());
    let failure =
        migrate_gateway_credentials_with(&root, || Err(anyhow!("synthetic_native_cancellation")));
    assert_eq!(
        failure.unwrap_err().to_string(),
        "synthetic_native_cancellation"
    );
    assert!(gateway_credential_migration_pending(&root).unwrap());
    let inventory = migrate_gateway_credentials_with(&root, || {
        // Native approval can take arbitrarily long. Its dedicated lock
        // must not block unrelated startup admission while waiting.
        assert_eq!(admit(&root)?.status, "ready");
        licoup_vault::api_key::LlmApiKeyInventory::new(
            licoup_vault::api_key::GatewayCredentialLeaseDays::default(),
            Vec::new(),
        )
    })
    .unwrap();
    assert!(inventory.entries.is_empty());
    assert!(!gateway_credential_migration_pending(&root).unwrap());
    assert!(
        admit(&root)
            .unwrap()
            .pending_authorization_domain_ids
            .is_empty()
    );
    migrate_gateway_credentials_with(&root, || {
        panic!("completed migration must not authenticate again")
    })
    .unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unavailable_optional_documents_preserve_bytes_and_do_not_refuse_core_startup() {
    let cases = [
        ("agent-tab-order", "client-state/agent-tab-order.json"),
        (
            "agent-tool-allowlist",
            "client-state/agent-tool-allowlists.json",
        ),
        (
            "appearance-presentation",
            "client-state/appearance-preferences.json",
        ),
        ("current-view", "client-state/current-client-view.json"),
        ("mobile-home-layout", "client-state/mobile-home-layout.json"),
        ("mobile-relay", "client-state/mobile-relay/config.json"),
        (
            "skill-hub-preferences",
            "client-state/skill-hub-preferences.json",
        ),
    ];
    for raw in [
        b"{broken".as_slice(),
        br#"{"schemaVersion":999,"retained":"synthetic"}"#.as_slice(),
    ] {
        let root =
            std::env::temp_dir().join(format!("licoup-optional-startup-{}", uuid::Uuid::new_v4()));
        admit(&root).unwrap();
        let before = fs::read(root.join("client-state/migrations/ledger.json")).unwrap();
        for (_, relative) in cases {
            let file = root.join(relative);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            licoup_foundation::platform::file_security::atomic_write_private_text(
                &file,
                std::str::from_utf8(raw).unwrap(),
            )
            .unwrap();
        }
        let result = admit(&root).unwrap();
        assert_eq!(result.status, "ready");
        assert_eq!(result.unavailable_feature_domain_ids.len(), cases.len());
        for (domain, relative) in cases {
            assert!(
                result
                    .unavailable_feature_domain_ids
                    .iter()
                    .any(|id| id == domain)
            );
            assert_eq!(fs::read(root.join(relative)).unwrap(), raw);
        }
        assert_eq!(
            fs::read(root.join("client-state/migrations/ledger.json")).unwrap(),
            before
        );
        // A broken core owner's data is still fatal, independently of optional state.
        fs::write(root.join(".licoup-workspace.json"), b"{broken").unwrap();
        assert!(admit(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn conversation_marker_reconstruction_preserves_unowned_relations_on_admission() {
    for mutation in [
        "DROP TABLE schema_meta;",
        "DELETE FROM schema_meta WHERE key='version';",
    ] {
        let root =
            std::env::temp_dir().join(format!("licoup-marker-startup-{}", uuid::Uuid::new_v4()));
        admit(&root).unwrap();
        let path = root.join("client-state/conversations/conversations.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(include_str!(
                "../../../../../tests/fixtures/client_state_migration/retained_related_tables.sql"
            ))
            .unwrap();
        connection.execute_batch("INSERT INTO conversations(id,title,created_at,updated_at) VALUES ('kept','Retained',1,1); INSERT INTO peer_bindings VALUES (x'01',x'02','kept','member','provider',1);").unwrap();
        connection.execute_batch(mutation).unwrap();
        drop(connection);
        admit(&root).unwrap();
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM peer_bindings", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT value FROM schema_meta WHERE key='version'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            licoup_conversation::store::CURRENT_SCHEMA_VERSION
        );
        drop(connection);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn admission_is_incremental_and_rerun_is_a_noop() {
    let root = std::env::temp_dir().join(format!("licoup-migration-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let first = admit(&root).unwrap();
    assert!(!first.applied_domain_ids.is_empty());
    let second = admit(&root).unwrap();
    assert!(second.applied_domain_ids.is_empty());
    assert_eq!(
        second.skipped_domain_ids.len() + second.pending_authorization_domain_ids.len(),
        embedded_frontier().unwrap().domains.len()
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn admission_lock_preserves_existing_contents() {
    let root = std::env::temp_dir().join(format!("licoup-migration-{}", uuid::Uuid::new_v4()));
    let migration_root = root.join("client-state/migrations");
    licoup_foundation::platform::file_security::ensure_private_dir(&migration_root).unwrap();
    let lock_path = migration_root.join("admission.lock");
    let canary = b"existing-lock-content";
    fs::write(&lock_path, canary).unwrap();

    admit(&root).unwrap();

    assert_eq!(fs::read(&lock_path).unwrap(), canary);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn ahead_domain_fails_without_advancing_other_domains() {
    let root = std::env::temp_dir().join(format!("licoup-migration-{}", uuid::Uuid::new_v4()));
    let marker_root = root.join("client-state/migrations/domain-state");
    licoup_foundation::platform::file_security::ensure_private_dir(&marker_root).unwrap();
    let domain = &embedded_frontier().unwrap().domains[0];
    write_json_atomic(
        &marker_path(&marker_root, &domain.domain_id),
        &DomainMarker {
            schema_version: DOMAIN_MARKER_SCHEMA.to_owned(),
            domain_id: domain.domain_id.clone(),
            authoritative_schema_version: domain.target_schema_version + 1,
        },
    )
    .unwrap();
    assert_eq!(
        admit(&root).unwrap_err().to_string(),
        "state_newer_than_binary"
    );
    assert!(!root.join("client-state/migrations/ledger.json").exists());
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn invalid_ledger_entry_is_not_replaced_as_missing_state() {
    use std::os::unix::fs::symlink;

    let root = std::env::temp_dir().join(format!("licoup-migration-{}", uuid::Uuid::new_v4()));
    let migration_root = root.join("client-state/migrations");
    fs::create_dir_all(&migration_root).unwrap();
    let ledger = migration_root.join("ledger.json");
    symlink(root.join("missing-ledger-target"), &ledger).unwrap();

    assert_eq!(
        admit(&root).unwrap_err().to_string(),
        "migration_ledger_invalid"
    );
    assert!(
        fs::symlink_metadata(&ledger)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn invalid_domain_marker_is_not_replaced_as_missing_state() {
    use std::os::unix::fs::symlink;

    let root = std::env::temp_dir().join(format!("licoup-migration-{}", uuid::Uuid::new_v4()));
    admit(&root).unwrap();
    let marker = root.join("client-state/migrations/domain-state/adaptive-flywheel.json");
    fs::remove_file(&marker).unwrap();
    symlink(root.join("missing-marker-target"), &marker).unwrap();

    assert_eq!(
        admit(&root).unwrap_err().to_string(),
        "unsupported_state_shape"
    );
    assert!(
        fs::symlink_metadata(&marker)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn current_marker_cannot_hide_a_reintroduced_legacy_store() {
    let root = std::env::temp_dir().join(format!("licoup-migration-{}", uuid::Uuid::new_v4()));
    admit(&root).unwrap();
    let config = root.join("client-state/mobile-relay/config.json");
    write_json_atomic(
        &config,
        &json!({
            "schemaVersion": 1,
            "pcClientId": "preserved-canary"
        }),
    )
    .unwrap();
    let before = fs::read(&config).unwrap();

    let admitted = admit(&root).unwrap();
    assert_eq!(admitted.unavailable_feature_domain_ids, ["mobile-relay"]);
    assert!(
        !admitted
            .skipped_domain_ids
            .iter()
            .any(|id| id == "mobile-relay")
    );
    assert_eq!(fs::read(&config).unwrap(), before);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn frontier_has_a_direct_unique_edge_registry() {
    let frontier = embedded_frontier().unwrap();
    assert!(!frontier.domains.is_empty());
    assert!(frontier.domains.iter().all(|domain| {
        domain
            .steps
            .first()
            .is_some_and(|step| step.from_schema_version == 0)
    }));
}

#[test]
fn unregistered_frontier_edge_fails_closed() {
    let root = std::env::temp_dir().join(format!(
        "licoup-migration-registry-{}",
        uuid::Uuid::new_v4()
    ));
    let edge = MigrationEdge {
        step_id: "unregistered.absent-to-1".to_owned(),
        from_schema_version: 0,
        to_schema_version: 1,
    };
    assert_eq!(
        apply_authoritative_store(&root, "unregistered", &edge)
            .unwrap_err()
            .to_string(),
        "migration_frontier_incomplete"
    );
}

#[test]
fn json_domain_migrations_preserve_durable_canaries_and_secret_custody() {
    let root = std::env::temp_dir().join(format!("licoup-migration-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(root.join("client-state/mobile-relay")).unwrap();
    write_json_atomic(
        &root.join("client-state/appearance-preferences.json"),
        &json!({
            "appearancePresetId": "canary-preset",
            "localePreference": "canary-locale"
        }),
    )
    .unwrap();
    write_json_atomic(
        &root.join("client-state/mobile-relay/config.json"),
        &json!({
            "schemaVersion": 1,
            "pcClientId": "synthetic-canary",
            "secretCustodyCanary": "must-survive"
        }),
    )
    .unwrap();

    admit(&root).unwrap();

    let appearance: serde_json::Value = serde_json::from_slice(
        &fs::read(root.join("client-state/appearance-preferences.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(appearance["schemaVersion"], json!(1));
    assert_eq!(appearance["appearancePresetId"], json!("canary-preset"));
    let relay: serde_json::Value = serde_json::from_slice(
        &fs::read(root.join("client-state/mobile-relay/config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(relay["schemaVersion"], json!(2));
    assert_eq!(relay["secretCustodyCanary"], json!("must-survive"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn client_state_collection_adoption_preserves_items_and_adds_current_authority() {
    let root = std::env::temp_dir().join(format!("licoup-migration-{}", uuid::Uuid::new_v4()));
    let path = root.join("client-state/settings.json");
    licoup_foundation::platform::file_security::ensure_private_dir(&root).unwrap();
    licoup_foundation::platform::file_security::ensure_private_dir(path.parent().unwrap()).unwrap();
    let canary = json!({
        "collection": "settings",
        "items": [{"id": "preserved-canary", "value": 42}]
    });
    write_json_atomic(&path, &canary).unwrap();
    licoup_foundation::platform::file_security::harden_private_path(&path).unwrap();

    admit(&root).unwrap();

    let migrated: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        migrated["schemaVersion"],
        json!("v0.0.1:schema:definition-1")
    );
    assert_eq!(migrated["collection"], json!("settings"));
    assert_eq!(migrated["items"], canary["items"]);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn incompatible_mobile_relay_protocol_disables_feature_without_mutating_the_store() {
    let root = std::env::temp_dir().join(format!("licoup-migration-{}", uuid::Uuid::new_v4()));
    let path = root.join("client-state/mobile-relay/config.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let original = json!({
        "schemaVersion": 1,
        "pcClientId": "synthetic-canary",
        "mobileRelayE2ee": {"protocolVersion": "future-protocol"},
        "secretCustodyCanary": "must-survive"
    });
    write_json_atomic(&path, &original).unwrap();

    let admitted = admit(&root).unwrap();
    assert_eq!(admitted.unavailable_feature_domain_ids, ["mobile-relay"]);
    let after: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(after, original);
    assert!(
        !root
            .join("client-state/migrations/domain-state/mobile-relay.json")
            .exists()
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn crash_after_store_commit_reconciles_without_reapplying_user_data() {
    let root = std::env::temp_dir().join(format!("licoup-migration-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(root.join("client-state")).unwrap();
    write_json_atomic(
        &root.join("client-state/appearance-preferences.json"),
        &json!({"appearancePresetId": "preserved", "localePreference": "en"}),
    )
    .unwrap();
    {
        let _guard = MigrationFailpointGuard::set("after-store");
        assert_eq!(
            admit(&root).unwrap_err().to_string(),
            "migration_step_failed"
        );
    }
    let recovered = admit(&root).unwrap();
    assert_eq!(recovered.status, "ready");
    let value: serde_json::Value = serde_json::from_slice(
        &fs::read(root.join("client-state/appearance-preferences.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(value["appearancePresetId"], json!("preserved"));
    let ledger: Ledger = serde_json::from_slice(
        &fs::read(root.join("client-state/migrations/ledger.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        ledger.domains.len() + recovered.pending_authorization_domain_ids.len(),
        embedded_frontier().unwrap().domains.len()
    );
    for pending in &recovered.pending_authorization_domain_ids {
        assert!(!ledger.domains.contains_key(pending));
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn crashes_before_store_and_after_ledger_resume_forward() {
    for failpoint in ["before-store", "after-ledger"] {
        let root = std::env::temp_dir().join(format!("licoup-migration-{}", uuid::Uuid::new_v4()));
        {
            let _guard = MigrationFailpointGuard::set(failpoint);
            assert_eq!(
                admit(&root).unwrap_err().to_string(),
                "migration_step_failed"
            );
        }
        assert_eq!(admit(&root).unwrap().status, "ready");
        assert!(admit(&root).unwrap().applied_domain_ids.is_empty());
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn mismatched_claim_blocks_before_ledger_or_domain_changes() {
    let root = std::env::temp_dir().join(format!("licoup-migration-{}", uuid::Uuid::new_v4()));
    let handoff = root.join("client-state/migrations/update-handoff.json");
    let receipt_id = format!("sha256:{}", "a".repeat(64));
    let target = root.join("Applications/LicoUp.app");
    let backup = pre_claim_backup_path(&target, &receipt_id).unwrap();
    write_json_atomic(
        &handoff,
        &UpdateHandoff {
            schema_version: UPDATE_HANDOFF_SCHEMA.to_owned(),
            state: update_handoff::State::Pending,
            version: "999.0.0".to_owned(),
            target_release_track: ReleaseTrack::running().unwrap().as_str().to_owned(),
            migration_frontier: frontier_projection().unwrap(),
            receipt_id,
            target_path: target.to_string_lossy().into_owned(),
            backup_path: backup.to_string_lossy().into_owned(),
        },
    )
    .unwrap();
    assert_eq!(
        admit(&root).unwrap_err().to_string(),
        "update_handoff_mismatch"
    );
    assert!(!root.join("client-state/migrations/ledger.json").exists());
    assert!(update_handoff_rejection_path(&handoff).unwrap().exists());
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn post_claim_cleanup_failure_never_authorizes_rollback() {
    use std::os::unix::fs::symlink;

    let root = std::env::temp_dir().join(format!(
        "licoup-migration-post-claim-{}",
        uuid::Uuid::new_v4()
    ));
    let handoff = root.join("client-state/migrations/update-handoff.json");
    let receipt_id = format!("sha256:{}", "e".repeat(64));
    let target = root.join("Applications/LicoUp.app");
    let backup = pre_claim_backup_path(&target, &receipt_id).unwrap();
    fs::create_dir_all(backup.parent().unwrap()).unwrap();
    symlink(&target, &backup).unwrap();
    write_json_atomic(
        &handoff,
        &UpdateHandoff {
            schema_version: UPDATE_HANDOFF_SCHEMA.to_owned(),
            state: update_handoff::State::Pending,
            version: running_product_version().unwrap().to_owned(),
            target_release_track: ReleaseTrack::running().unwrap().as_str().to_owned(),
            migration_frontier: frontier_projection().unwrap(),
            receipt_id,
            target_path: target.to_string_lossy().into_owned(),
            backup_path: backup.to_string_lossy().into_owned(),
        },
    )
    .unwrap();

    assert_eq!(
        admit(&root).unwrap_err().to_string(),
        "update_handoff_mismatch"
    );
    let claimed: UpdateHandoff = serde_json::from_slice(&fs::read(&handoff).unwrap()).unwrap();
    assert_eq!(claimed.state, update_handoff::State::Claimed);
    assert!(!update_handoff_rejection_path(&handoff).unwrap().exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn valid_claim_is_consumed_only_after_successful_admission() {
    let root = std::env::temp_dir().join(format!("licoup-migration-{}", uuid::Uuid::new_v4()));
    let handoff = root.join("client-state/migrations/update-handoff.json");
    let receipt_id = format!("sha256:{}", "b".repeat(64));
    let target = root.join("Applications/LicoUp.app");
    let backup = pre_claim_backup_path(&target, &receipt_id).unwrap();
    fs::create_dir_all(&backup).unwrap();
    fs::write(backup.join("preserved"), b"old-app").unwrap();
    write_json_atomic(
        &handoff,
        &UpdateHandoff {
            schema_version: UPDATE_HANDOFF_SCHEMA.to_owned(),
            state: update_handoff::State::Pending,
            version: running_product_version().unwrap().to_owned(),
            target_release_track: ReleaseTrack::running().unwrap().as_str().to_owned(),
            migration_frontier: frontier_projection().unwrap(),
            receipt_id,
            target_path: target.to_string_lossy().into_owned(),
            backup_path: backup.to_string_lossy().into_owned(),
        },
    )
    .unwrap();
    admit(&root).unwrap();
    assert!(!handoff.exists());
    assert!(!update_handoff_rejection_path(&handoff).unwrap().exists());
    assert!(!backup.exists());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn update_handoff_stays_pending_until_the_candidate_admits_state() {
    let root = std::env::temp_dir().join(format!("licoup-migration-{}", uuid::Uuid::new_v4()));
    let target = root.join("Applications/LicoUp.app");
    let receipt = json!({
        "version": running_product_version().unwrap(),
        "targetReleaseTrack": ReleaseTrack::running().unwrap().as_str(),
        "migrationFrontier": frontier_projection().unwrap(),
        "receiptId": format!("sha256:{}", "c".repeat(64)),
    });
    let prepared = prepare_update_handoff(&root, &receipt, &target).unwrap();
    let pending: UpdateHandoff =
        serde_json::from_slice(&fs::read(&prepared.handoff_path).unwrap()).unwrap();
    assert_eq!(pending.state, update_handoff::State::Pending);
    assert!(prepare_update_handoff(&root, &receipt, &target).is_err());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn update_handoff_carries_a_strictly_extended_candidate_frontier() {
    let root = std::env::temp_dir().join(format!("licoup-migration-{}", uuid::Uuid::new_v4()));
    let target = root.join("Applications/LicoUp.app");
    let mut candidate = frontier_projection().unwrap();
    candidate["frontierId"] = json!("licoup-state-next");
    candidate["domains"].as_array_mut().unwrap().push(json!({
        "domainId": "future-domain",
        "targetSchemaVersion": 1,
        "requiredStepIds": ["future-domain.absent-to-1"]
    }));
    let receipt = json!({
        "version": "999.0.0",
        "targetReleaseTrack": "nightly",
        "migrationFrontier": candidate,
        "receiptId": format!("sha256:{}", "d".repeat(64)),
    });

    let prepared = prepare_update_handoff(&root, &receipt, &target).unwrap();
    let pending: UpdateHandoff =
        serde_json::from_slice(&fs::read(&prepared.handoff_path).unwrap()).unwrap();
    assert_eq!(pending.migration_frontier, receipt["migrationFrontier"]);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn current_sqlite_domains_preserve_canary_rows() {
    let root = std::env::temp_dir().join(format!("licoup-migration-{}", uuid::Uuid::new_v4()));
    // Create both current SQLite layouts through their owners. Admission must
    // preserve complete current stores, not repair a partial layout that merely
    // carries the current version number.
    let conversations = crate::domain::client_conversation::ConversationStore::open(&root).unwrap();
    conversations.checkpoint().unwrap();
    drop(conversations);
    let strategy = crate::domain::workflow_store::StrategyStore::open(&root).unwrap();
    drop(strategy);
    for relative in [
        "client-state/conversations/conversations.sqlite3",
        "client-state/adaptive-flywheel/strategies.sqlite3",
    ] {
        let connection = Connection::open(root.join(relative)).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE preservation_canary(value TEXT NOT NULL);\
                 INSERT INTO preservation_canary(value) VALUES ('must-survive');",
            )
            .unwrap();
    }
    admit(&root).unwrap();
    for relative in [
        "client-state/conversations/conversations.sqlite3",
        "client-state/adaptive-flywheel/strategies.sqlite3",
    ] {
        let connection = Connection::open(root.join(relative)).unwrap();
        let canary: String = connection
            .query_row("SELECT value FROM preservation_canary", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(canary, "must-survive");
    }
    let _ = fs::remove_dir_all(root);
}

#[test]
fn generated_native_machines_own_handoff_and_strategy_transitions() {
    use update_handoff::{Event as HandoffEvent, State as HandoffState};

    assert_eq!(update_handoff::MACHINE_ID, "native.update-handoff");
    assert_eq!(
        update_handoff::transition(HandoffState::Pending, HandoffEvent::Claim),
        Some(HandoffState::Claimed)
    );
    assert_eq!(
        update_handoff::transition(HandoffState::Claimed, HandoffEvent::Claim),
        None
    );

    use strategy_store_artifact::{Event as StoreEvent, State as StoreState};

    assert_eq!(
        strategy_store_artifact::MACHINE_ID,
        "native.strategy-store-artifact"
    );
    assert_eq!(
        strategy_store_artifact::transition(StoreState::Pending, StoreEvent::Begin),
        Some(StoreState::Pending)
    );
    assert_eq!(
        strategy_store_artifact::transition(StoreState::Pending, StoreEvent::Complete),
        Some(StoreState::Applied)
    );
    assert_eq!(
        strategy_store_artifact::transition(StoreState::Applied, StoreEvent::Begin),
        Some(StoreState::Pending)
    );
    assert_eq!(
        strategy_store_artifact::transition(StoreState::Applied, StoreEvent::Complete),
        Some(StoreState::Applied)
    );
}

#[test]
fn adaptive_flywheel_probe_recognizes_only_the_released_and_current_layouts() {
    let root = std::env::temp_dir().join(format!("licoup-adaptive-probe-{}", uuid::Uuid::new_v4()));
    let database = root.join(strategy_store::STRATEGY_STORE_DATABASE);

    // Absence is version 0 with no authority, so the immutable 0→1 step is
    // reconciled in the ledger without a store conversion.
    let probe = strategy_store::probe_adaptive_flywheel(&root).unwrap();
    assert!(!probe.present);
    assert_eq!(probe.version, 0);

    // The released tag's layout is the source layout, at domain version 1.
    seed_released_strategy_store(&database);
    let probe = strategy_store::probe_adaptive_flywheel(&root).unwrap();
    assert!(probe.present);
    assert_eq!(probe.version, 1);
    assert_eq!(
        strategy_store::read_strategy_store_format(&database)
            .unwrap()
            .format_id,
        "strategy-store-2"
    );
    let _ = fs::remove_dir_all(root);

    // The current layout, created by its owner, is domain version 2.
    let root = std::env::temp_dir().join(format!("licoup-adaptive-probe-{}", uuid::Uuid::new_v4()));
    let database = root.join(strategy_store::STRATEGY_STORE_DATABASE);
    let strategy = crate::domain::workflow_store::StrategyStore::open(&root).unwrap();
    drop(strategy);
    let probe = strategy_store::probe_adaptive_flywheel(&root).unwrap();
    assert!(probe.present);
    assert_eq!(probe.version, 2);
    assert_eq!(
        strategy_store::read_strategy_store_format(&database)
            .unwrap()
            .format_id,
        "strategy-store-3"
    );
    let _ = fs::remove_dir_all(root);

    // A version row on a database without the layout is not a layout. The
    // released writer always creates all seven tables, and both known layouts
    // plus the development-era stamps must be told apart by what is there.
    for schema in ["0", "1", "2", "3"] {
        let root =
            std::env::temp_dir().join(format!("licoup-adaptive-stamp-{}", uuid::Uuid::new_v4()));
        let database = root.join(strategy_store::STRATEGY_STORE_DATABASE);
        fs::create_dir_all(database.parent().unwrap()).unwrap();
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(&format!(
                "CREATE TABLE strategy_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
                     INSERT INTO strategy_meta(key,value) VALUES ('version','{schema}');"
            ))
            .unwrap();
        drop(connection);
        assert_eq!(
            strategy_store::probe_adaptive_flywheel(&root)
                .unwrap_err()
                .to_string(),
            "unsupported_state_shape",
            "a version stamp alone is not the store layout for {schema}"
        );
        let _ = fs::remove_dir_all(root);
    }
}

#[test]
fn unsupported_adaptive_flywheel_schema_refuses_without_writing_database() {
    for (schema, error) in [
        ("4", "state_newer_than_binary"),
        ("1", "unsupported_state_shape"),
        ("malformed", "unsupported_state_shape"),
    ] {
        let root =
            std::env::temp_dir().join(format!("licoup-adaptive-refusal-{}", uuid::Uuid::new_v4()));
        let database = root.join(strategy_store::STRATEGY_STORE_DATABASE);
        seed_released_strategy_store(&database);
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(&format!(
                "UPDATE strategy_meta SET value='{schema}' WHERE key='version';"
            ))
            .unwrap();
        drop(connection);
        let before = fs::read(&database).unwrap();

        assert_eq!(admit(&root).unwrap_err().to_string(), error);
        assert_eq!(fs::read(&database).unwrap(), before);
        assert!(!root.join("client-state/migrations/ledger.json").exists());
        let connection = Connection::open(&database).unwrap();
        let binding: String = connection
            .query_row(
                "SELECT value_id FROM strategy_bindings
                  WHERE revision_digest=?1 AND slot_id='actor' AND ordinal=0",
                rusqlite::params![RELEASED_DEFINITION_REVISION],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(binding, "lico-basic");
        let _ = fs::remove_dir_all(root);
    }
}

/// The released source root is admitted to this binary's own frontier: the
/// released strategy layout becomes the current one, the released Conversation
/// schema is upgraded in place by its owner, the released ledger advances, and
/// the released credential inventory stays metadata rather than custody proof.
#[test]
fn a_released_source_root_is_admitted_to_the_current_frontier() {
    let root =
        std::env::temp_dir().join(format!("licoup-released-source-{}", uuid::Uuid::new_v4()));
    seed_released_source_root(&root);
    let ledger_path = root.join("client-state/migrations/ledger.json");
    let inventory_path = root.join(RELEASED_INVENTORY_FILE);
    let inventory_before = fs::read(&inventory_path).unwrap();

    // The released root records product high-water 0.2.1. The candidate runs
    // under the identity it actually is, 0.3.0; the source stamp is not lowered
    // to the development fallback to make this pass.
    let result = admit_as_version(&root, "0.3.0").unwrap();

    assert_eq!(result.status, "ready");
    assert_eq!(result.running_product_version, "0.3.0");
    assert_eq!(
        result.frontier_id,
        embedded_frontier().unwrap().frontier_id,
        "a released source root is admitted to this binary's own target"
    );
    assert!(
        result
            .applied_domain_ids
            .iter()
            .any(|domain| domain == "adaptive-flywheel"),
        "the released strategy layout has to be converted"
    );
    assert!(
        !result
            .applied_domain_ids
            .iter()
            .any(|domain| domain == "canonical-conversation"),
        "a domain already at its target is not reported as converted"
    );

    // The released strategy layout reached the current one. The owning store
    // reads the released definition, its ordinal binding and its authorization
    // back, and the raw run/event rows survive with their values.
    let database = root.join(strategy_store::STRATEGY_STORE_DATABASE);
    let connection = Connection::open(&database).unwrap();
    let version: String = connection
        .query_row(
            "SELECT value FROM strategy_meta WHERE key='version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, "3");
    drop(connection);
    let strategy = crate::domain::workflow_store::StrategyStore::open(&root).unwrap();
    let definition = strategy
        .definition_by_revision(RELEASED_DEFINITION_REVISION)
        .unwrap();
    assert_eq!(definition.summary.name, "Temporary");
    assert_eq!(definition.workflow.metadata.id, "assistant-temporary");
    assert_eq!(definition.bindings.len(), 1);
    assert_eq!(definition.bindings[0].slot_id, "actor");
    assert!(definition.authorization.unwrap().active);
    // The published legacy workflow is normalized by its owner: the first actor
    // slot becomes the entry, while the metadata, states and transitions it was
    // seeded with survive.
    assert!(definition.workflow.actor_slots[0].entry);
    assert_eq!(definition.workflow.states.len(), 3);
    assert_eq!(definition.workflow.transitions.len(), 2);
    let stored_workflow: String = Connection::open(&database)
        .unwrap()
        .query_row(
            "SELECT workflow_json FROM strategy_definitions WHERE revision_digest=?1",
            rusqlite::params![RELEASED_DEFINITION_REVISION],
            |row| row.get(0),
        )
        .unwrap();
    assert_ne!(
        stored_workflow, RELEASED_WORKFLOW_JSON,
        "the legacy definition must have been rewritten"
    );
    assert!(stored_workflow.contains("\"entry\":true"));
    // The published run snapshot is readable through the owning store, and the
    // published run event is a valid serialized reducer event.
    let snapshot = strategy.run(RELEASED_RUN_ID).unwrap();
    assert_eq!(
        snapshot.status,
        licoup_workflow::ir::StrategyRunStatus::Completed
    );
    assert_eq!(
        snapshot.conversation_id.as_deref(),
        Some(RELEASED_CONVERSATION_ID)
    );
    let connection = Connection::open(&database).unwrap();
    let idempotency: String = connection
        .query_row(
            "SELECT idempotency_key FROM strategy_runs WHERE run_id=?1",
            rusqlite::params![RELEASED_RUN_ID],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(idempotency, "released-idempotency");
    let event: (String, String) = connection
        .query_row(
            "SELECT event_type, event_json FROM strategy_run_events
              WHERE run_id=?1 AND sequence=1",
            rusqlite::params![RELEASED_RUN_ID],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(event.0, "start");
    let reducer_event: licoup_workflow::ReducerEvent = serde_json::from_str(&event.1).unwrap();
    assert!(matches!(
        reducer_event,
        licoup_workflow::ReducerEvent::Start { .. }
    ));
    drop(connection);

    // The released Conversation store is upgraded in place by its owner; the
    // owning store reads the released conversation and its event back.
    let database = root.join(RELEASED_CONVERSATION_DATABASE);
    let connection = Connection::open(&database).unwrap();
    let inner: String = connection
        .query_row(
            "SELECT value FROM schema_meta WHERE key='version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(inner, licoup_conversation::store::CURRENT_SCHEMA_VERSION);
    drop(connection);
    let conversations = crate::domain::client_conversation::ConversationStore::open(&root).unwrap();
    let conversation = conversations.get(RELEASED_CONVERSATION_ID).unwrap();
    assert_eq!(conversation.title, "Synthetic released conversation");
    assert_eq!(conversation.memberships.len(), 1);
    let page = conversations
        .page_events(RELEASED_CONVERSATION_ID, None, 10)
        .unwrap();
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.events[0].id, RELEASED_EVENT_ID);
    // The retained event sequence uniqueness still holds after conversion.
    let duplicate = Connection::open(&database).unwrap().execute(
        "INSERT INTO events(id, conversation_id, sequence, kind, created_at, finalized)
         VALUES ('released-event-duplicate', ?1, 1, 'message', 1, 1)",
        rusqlite::params![RELEASED_CONVERSATION_ID],
    );
    assert!(
        duplicate.is_err(),
        "a duplicate conversation sequence must be rejected"
    );

    // The ledger now names this binary's target and keeps the released step.
    let ledger: Ledger = serde_json::from_slice(&fs::read(&ledger_path).unwrap()).unwrap();
    assert_eq!(ledger.frontier_id, embedded_frontier().unwrap().frontier_id);
    assert_eq!(ledger.highest_admitted_product_version, "0.3.0");
    let adaptive = &ledger.domains["adaptive-flywheel"];
    assert_eq!(adaptive.schema_version, 2);
    assert_eq!(
        adaptive.completed_step_ids,
        vec![
            "adaptive-flywheel.absent-to-1".to_owned(),
            "adaptive-flywheel.workflow-routing-to-2".to_owned(),
        ]
    );
    assert_eq!(
        ledger.domains["workspace-manifest"].schema_version, 1,
        "a released domain already at its target keeps its entry"
    );
    assert_eq!(
        ledger.domains.len() + result.pending_authorization_domain_ids.len(),
        embedded_frontier().unwrap().domains.len()
    );

    // The platform owner alone declares whether this root awaits protected custody.
    assert_eq!(fs::read(&inventory_path).unwrap(), inventory_before);
    let requires_authorization = PlatformLlmApiKeyVault::legacy_credential_migration_disposition()
        .unwrap()
        == LegacyCredentialMigrationDisposition::RequiresAuthorization;
    let custody_marker =
        root.join("client-state/migrations/domain-state/gateway-credential-custody.json");
    assert_eq!(custody_marker.exists(), !requires_authorization);
    assert_eq!(
        gateway_credential_migration_pending(&root).unwrap(),
        requires_authorization
    );
    assert_eq!(
        result
            .pending_authorization_domain_ids
            .contains(&GATEWAY_CUSTODY_DOMAIN.to_owned()),
        requires_authorization
    );
    if !requires_authorization {
        let marker: DomainMarker =
            serde_json::from_slice(&fs::read(custody_marker).unwrap()).unwrap();
        assert_eq!(marker.domain_id, GATEWAY_CUSTODY_DOMAIN);
        assert_eq!(
            marker.authoritative_schema_version,
            ledger.domains[GATEWAY_CUSTODY_DOMAIN].schema_version
        );
    }
    let _ = fs::remove_dir_all(root);
}

/// A running identity below the released product high-water refuses the root
/// untouched — exactly what the development fallback does. The fixture keeps
/// the true 0.2.1 stamp; the test does not lower it to pass.
#[test]
fn a_development_identity_refuses_the_released_source_without_writing() {
    let root =
        std::env::temp_dir().join(format!("licoup-released-source-{}", uuid::Uuid::new_v4()));
    seed_released_source_root(&root);
    let files = [
        root.join("client-state/migrations/ledger.json"),
        root.join(strategy_store::STRATEGY_STORE_DATABASE),
        root.join(RELEASED_CONVERSATION_DATABASE),
    ];
    let before = files
        .iter()
        .map(|path| fs::read(path).unwrap())
        .collect::<Vec<_>>();

    for running in ["0.2.0", "0.0.1-alpha"] {
        assert_eq!(
            admit_as_version(&root, running).unwrap_err().to_string(),
            "state_newer_than_binary",
            "running {running} is below the released high-water"
        );
        for (path, bytes) in files.iter().zip(&before) {
            assert_eq!(&fs::read(path).unwrap(), bytes, "{path:?} is untouched");
        }
        assert!(
            !root.join("client-state/migrations/artifacts").exists(),
            "a refused root gains no conversion record"
        );
    }
    let _ = fs::remove_dir_all(root);
}

/// A stop after the owner committed the new store layout but before the ledger
/// bookkeeping resumes on the next run: the store is already converted, so only
/// the ledger prefix is reconciled.
#[test]
fn an_interrupted_conversion_reconciles_the_committed_store_before_bookkeeping() {
    let root = std::env::temp_dir().join(format!(
        "licoup-adaptive-prefix-recovery-{}",
        uuid::Uuid::new_v4()
    ));
    seed_released_source_root(&root);

    {
        let _guard = MigrationFailpointGuard::set("after-store");
        assert_eq!(
            admit_as_version(&root, "0.3.0").unwrap_err().to_string(),
            "migration_step_failed"
        );
    }

    // The physical store and its marker committed; the ledger still holds the
    // released prefix because the crash happened before its write.
    assert_eq!(
        strategy_store::probe_adaptive_flywheel(&root)
            .unwrap()
            .version,
        2
    );
    let ledger_path = root.join("client-state/migrations/ledger.json");
    let ledger: Ledger = serde_json::from_slice(&fs::read(&ledger_path).unwrap()).unwrap();
    assert_eq!(ledger.domains["adaptive-flywheel"].schema_version, 1);

    let recovered = admit_as_version(&root, "0.3.0").unwrap();
    assert!(
        recovered
            .skipped_domain_ids
            .iter()
            .any(|domain| domain == "adaptive-flywheel"),
        "the store is already converted; only bookkeeping resumes"
    );
    assert!(
        !recovered
            .applied_domain_ids
            .iter()
            .any(|domain| domain == "adaptive-flywheel")
    );
    let ledger: Ledger = serde_json::from_slice(&fs::read(&ledger_path).unwrap()).unwrap();
    assert_eq!(ledger.frontier_id, embedded_frontier().unwrap().frontier_id);
    let adaptive = &ledger.domains["adaptive-flywheel"];
    assert_eq!(adaptive.schema_version, 2);
    assert_eq!(
        adaptive.completed_step_ids,
        vec![
            "adaptive-flywheel.absent-to-1".to_owned(),
            "adaptive-flywheel.workflow-routing-to-2".to_owned(),
        ]
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn conversation_legacy_import_finishes_during_admission() {
    let root = std::env::temp_dir().join(format!(
        "licoup-conversation-admission-{}",
        uuid::Uuid::new_v4()
    ));
    let legacy = root.join("client-state/agent-conversation-projections.json");
    fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    fs::write(
            &legacy,
            r#"{"schemaVersion":1,"sessionsByAgent":{"agent-one":[{"id":"session-1","title":"Preserved","messages":[{"role":"user","content":"canary"}]}]}}"#,
        )
        .unwrap();

    admit(&root).unwrap();

    assert!(!legacy.exists());
    assert!(
        root.join("client-state/conversations/migration-v5.complete")
            .is_file()
    );
    let store = crate::domain::client_conversation::ConversationStore::open(&root).unwrap();
    let conversations = store.list(false).unwrap();
    assert_eq!(conversations.len(), 1);
    assert_eq!(conversations[0].title, "Preserved");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn admitted_v11_conversation_store_upgrades_without_resetting_the_domain() {
    let root = std::env::temp_dir().join(format!(
        "licoup-conversation-v11-admit-{}",
        uuid::Uuid::new_v4()
    ));
    admit(&root).unwrap();
    let database = root.join("client-state/conversations/conversations.sqlite3");
    let connection = Connection::open(&database).unwrap();
    connection
        .execute("UPDATE schema_meta SET value='11' WHERE key='version'", [])
        .unwrap();
    drop(connection);
    assert_eq!(probe_canonical_conversation(&root).unwrap().version, 1);

    admit(&root).unwrap();

    let version: String = Connection::open(&database)
        .unwrap()
        .query_row(
            "SELECT value FROM schema_meta WHERE key='version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, licoup_conversation::store::CURRENT_SCHEMA_VERSION);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn the_strategy_store_graph_is_connected_and_the_frontier_agrees_with_it() {
    let current = strategy_store::current_strategy_format();
    assert_eq!(current.format_id, "strategy-store-3");
    assert_eq!(
        strategy_store::STRATEGY_STORE_FORMATS.len(),
        2,
        "exactly the released layout and this binary's current one are known"
    );
    assert_eq!(
        strategy_store::STRATEGY_STORE_FORMATS[0].format_id,
        "strategy-store-2"
    );
    assert_eq!(
        strategy_store::STRATEGY_STORE_FORMATS[0].meta_versions,
        &["2"],
        "the released tag stamped strategy_meta version 2"
    );
    assert_eq!(
        strategy_store::STRATEGY_STORE_FORMATS[0].domain_schema_version,
        1
    );
    // Every known layout reaches the current one along the graph, and every
    // edge leaves a layout exactly once: a layout with two successors would
    // make a conversion ambiguous, which is what `strategy_store_path` refuses
    // rather than guesses.
    for format in strategy_store::STRATEGY_STORE_FORMATS {
        let path =
            strategy_store::strategy_store_path(format.format_id, current.format_id).unwrap();
        assert_eq!(
            path.last().map(|edge| edge.to),
            (format.format_id != current.format_id).then_some(current.format_id)
        );
        assert!(
            strategy_store::STRATEGY_STORE_EDGES
                .iter()
                .filter(|edge| edge.from == format.format_id)
                .count()
                <= 1
        );
    }
    // A layout id that is not in the graph has no path, so a typo or a
    // development-era stamp fails closed instead of converting to "the newest
    // thing".
    assert!(strategy_store::strategy_store_path("strategy-store-9", current.format_id).is_err());
    assert!(strategy_store::strategy_store_path("strategy-store-1", current.format_id).is_err());

    // The frontier's domain versions and the known layouts are one numbering:
    // every domain version the frontier names must have a layout, and the
    // current layout must be the frontier's target. There is no layout for a
    // domain version below the released one, so that state is refused.
    let frontier = embedded_frontier().unwrap();
    let domain = frontier
        .domains
        .iter()
        .find(|domain| domain.domain_id == strategy_store::STRATEGY_STORE_DOMAIN)
        .unwrap();
    assert_eq!(domain.target_schema_version, current.domain_schema_version);
    for edge in &domain.steps {
        assert!(strategy_store::strategy_format_for_domain_version(edge.to_schema_version).is_ok());
    }
    assert!(strategy_store::strategy_format_for_domain_version(0).is_err());
}

#[test]
fn an_interrupted_released_store_conversion_resumes_from_its_artifact() {
    let root = std::env::temp_dir().join(format!(
        "licoup-store-artifact-resume-{}",
        uuid::Uuid::new_v4()
    ));
    seed_released_source_root(&root);
    let database = root.join(strategy_store::STRATEGY_STORE_DATABASE);

    {
        let _guard = MigrationFailpointGuard::set("before-strategy-store-edge");
        assert_eq!(
            admit_as_version(&root, "0.3.0").unwrap_err().to_string(),
            "migration_step_failed"
        );
    }
    // The crash left the recovery record and nothing else: the store is still
    // the released layout, so the next run converts rather than believing a
    // conversion that never happened.
    let artifact: strategy_store::StrategyStoreArtifact = serde_json::from_slice(
        &fs::read(strategy_store::strategy_store_artifact_path(&root)).unwrap(),
    )
    .unwrap();
    assert_eq!(artifact.status, "pending");
    assert!(artifact.applied_step_ids.is_empty());
    assert_eq!(artifact.from_format, "strategy-store-2");
    assert_eq!(artifact.target_format, "strategy-store-3");
    assert_eq!(
        strategy_store::read_strategy_store_format(&database)
            .unwrap()
            .format_id,
        "strategy-store-2"
    );

    let result = admit_as_version(&root, "0.3.0").unwrap();
    assert_eq!(result.frontier_id, embedded_frontier().unwrap().frontier_id);
    assert!(
        result
            .applied_domain_ids
            .iter()
            .any(|domain| domain == "adaptive-flywheel"),
        "the store advanced, so the domain must not be reported as untouched"
    );
    assert_eq!(
        strategy_store::read_strategy_store_format(&database)
            .unwrap()
            .format_id,
        "strategy-store-3"
    );
    let artifact: strategy_store::StrategyStoreArtifact = serde_json::from_slice(
        &fs::read(strategy_store::strategy_store_artifact_path(&root)).unwrap(),
    )
    .unwrap();
    assert_eq!(artifact.status, "applied");
    assert_eq!(
        artifact.applied_step_ids,
        vec!["adaptive-flywheel.strategy-store-workflow-routing".to_owned()]
    );
    let connection = Connection::open(&database).unwrap();
    let binding: String = connection
        .query_row(
            "SELECT value_id FROM strategy_bindings
              WHERE revision_digest=?1 AND slot_id='actor' AND ordinal=0",
            rusqlite::params![RELEASED_DEFINITION_REVISION],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(binding, "lico-basic");
    let frontier = embedded_frontier().unwrap();
    let ledger = load_ledger(&root.join("client-state/migrations/ledger.json"), &frontier).unwrap();
    assert_eq!(
        ledger.frontier_id, frontier.frontier_id,
        "a converted root records this binary's own target"
    );
    assert_eq!(ledger.domains["adaptive-flywheel"].schema_version, 2);
    let _ = fs::remove_dir_all(root);
}

/// Completed history never selects a runtime format or certifies store health.
#[test]
fn completed_conversion_history_does_not_override_the_current_store() {
    let root = std::env::temp_dir().join(format!(
        "licoup-completed-conversion-history-{}",
        uuid::Uuid::new_v4()
    ));
    seed_released_source_root(&root);
    admit_as_version(&root, "0.3.0").unwrap();
    let database = root.join(strategy_store::STRATEGY_STORE_DATABASE);
    let store_before = fs::read(&database).unwrap();
    let artifact_path = strategy_store::strategy_store_artifact_path(&root);
    let mut artifact: strategy_store::StrategyStoreArtifact =
        serde_json::from_slice(&fs::read(&artifact_path).unwrap()).unwrap();
    // An unpublished conversion can leave a completed historical receipt after
    // its implementation is corrected. It is not a supported physical format.
    artifact.target_format = "retired-strategy-layout".to_owned();
    artifact.applied_step_ids = vec!["retired-strategy-conversion".to_owned()];
    write_json_atomic(&artifact_path, &artifact).unwrap();
    let history_before = fs::read(&artifact_path).unwrap();

    assert_eq!(admit_as_version(&root, "0.3.0").unwrap().status, "ready");
    assert_eq!(fs::read(&database).unwrap(), store_before);
    assert_eq!(fs::read(&artifact_path).unwrap(), history_before);

    // An unfinished record still needs a known conversion path; completion is
    // not inferred from a malformed or unknown pending request.
    artifact.status = "pending".to_owned();
    write_json_atomic(&artifact_path, &artifact).unwrap();
    assert_eq!(
        admit_as_version(&root, "0.3.0").unwrap_err().to_string(),
        "migration_frontier_incomplete"
    );
    assert_eq!(fs::read(&database).unwrap(), store_before);

    // A historical success never certifies the current physical schema.
    artifact.status = "applied".to_owned();
    write_json_atomic(&artifact_path, &artifact).unwrap();
    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch("DROP TABLE strategy_bindings;")
        .unwrap();
    drop(connection);
    assert_eq!(
        admit_as_version(&root, "0.3.0").unwrap_err().to_string(),
        "unsupported_state_shape"
    );
    assert_eq!(fs::read(&artifact_path).unwrap(), history_before);
    fs::remove_dir_all(root).unwrap();
}

/// A stop between the owner's committed store and this module's record: the
/// store is current, the journal is pending, and only the record may be
/// rewritten when the next run reconciles it.
#[test]
fn a_journal_pending_after_the_store_commit_reconciles_without_rewriting_the_store() {
    let root = std::env::temp_dir().join(format!(
        "licoup-store-journal-reconcile-{}",
        uuid::Uuid::new_v4()
    ));
    seed_released_source_root(&root);
    let database = root.join(strategy_store::STRATEGY_STORE_DATABASE);

    {
        let _guard = MigrationFailpointGuard::set("after-strategy-store-edge");
        assert_eq!(
            admit_as_version(&root, "0.3.0").unwrap_err().to_string(),
            "migration_step_failed"
        );
    }
    // The owner committed the current layout before the record was updated,
    // which is exactly the state that must not be lost.
    assert_eq!(
        strategy_store::read_strategy_store_format(&database)
            .unwrap()
            .format_id,
        "strategy-store-3"
    );
    let artifact: strategy_store::StrategyStoreArtifact = serde_json::from_slice(
        &fs::read(strategy_store::strategy_store_artifact_path(&root)).unwrap(),
    )
    .unwrap();
    assert_eq!(artifact.status, "pending");
    assert!(artifact.applied_step_ids.is_empty());
    let store_before = fs::read(&database).unwrap();

    let recovered = admit_as_version(&root, "0.3.0").unwrap();
    assert!(
        recovered
            .skipped_domain_ids
            .iter()
            .any(|domain| domain == "adaptive-flywheel"),
        "the store is already at its target; only the record is owed"
    );
    let artifact: strategy_store::StrategyStoreArtifact = serde_json::from_slice(
        &fs::read(strategy_store::strategy_store_artifact_path(&root)).unwrap(),
    )
    .unwrap();
    assert_eq!(artifact.status, "applied");
    assert_eq!(
        artifact.applied_step_ids,
        vec!["adaptive-flywheel.strategy-store-workflow-routing".to_owned()]
    );
    assert_eq!(
        fs::read(&database).unwrap(),
        store_before,
        "reconciling a record must not rewrite a store that is already current"
    );

    // From here the root is an ordinary current-shape root.
    let again = admit_as_version(&root, "0.3.0").unwrap();
    assert!(
        !again
            .applied_domain_ids
            .iter()
            .any(|domain| domain == "adaptive-flywheel")
    );
    assert_eq!(fs::read(&database).unwrap(), store_before);
    let _ = fs::remove_dir_all(root);
}

/// Every durable file's bytes under `root`, keyed by relative path. SQLite's
/// transient `-wal`/`-shm` companions are excluded: a read-only probe may
/// create one without admission writing a durable byte.
fn durable_file_bytes(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            if metadata.is_dir() {
                visit(root, &path, files);
                continue;
            }
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            if name.ends_with("-wal") || name.ends_with("-shm") {
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            files.insert(relative, fs::read(&path).unwrap());
        }
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

/// Replace the conversation store with a two-table store stamped as an older
/// published schema: the identity tables every published generation carried are
/// missing, so the preflight refuses before the owner could fail later.
fn truncate_older_conversation_store(root: &Path) {
    let database = root.join(RELEASED_CONVERSATION_DATABASE);
    fs::remove_file(&database).unwrap();
    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE schema_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO schema_meta(key, value) VALUES ('version', '11');
             CREATE TABLE conversations(id TEXT PRIMARY KEY, title TEXT NOT NULL);",
        )
        .unwrap();
}

/// Keep the released layout but weaken the membership identity index to a
/// non-unique object with the same name: an index name alone is not the
/// constraint.
fn weaken_membership_identity_index(root: &Path) {
    let connection = Connection::open(root.join(RELEASED_CONVERSATION_DATABASE)).unwrap();
    connection
        .execute_batch(
            "DROP INDEX memberships_principal_unique;
             CREATE INDEX memberships_principal_unique
               ON memberships(conversation_id, principal_id);",
        )
        .unwrap();
}

/// Admit the current store, then remove the conversation title the migration
/// admission relies on.
fn remove_conversation_title(root: &Path) {
    admit_as_version(root, "0.3.0").unwrap();
    let connection = Connection::open(root.join(RELEASED_CONVERSATION_DATABASE)).unwrap();
    connection
        .execute_batch("ALTER TABLE conversations DROP COLUMN title;")
        .unwrap();
}

/// Admit the current store, then remove a principal business field every
/// reader selects.
fn remove_current_principal_display_name(root: &Path) {
    admit_as_version(root, "0.3.0").unwrap();
    let connection = Connection::open(root.join(RELEASED_CONVERSATION_DATABASE)).unwrap();
    connection
        .execute_batch("ALTER TABLE principals DROP COLUMN display_name;")
        .unwrap();
}

/// Remove a principal business field from the released layout itself.
fn remove_released_principal_display_name(root: &Path) {
    let connection = Connection::open(root.join(RELEASED_CONVERSATION_DATABASE)).unwrap();
    connection
        .execute_batch("ALTER TABLE principals DROP COLUMN display_name;")
        .unwrap();
}

/// Rebuild the released events table without its
/// `UNIQUE(conversation_id, sequence)` constraint, exactly the shape a
/// weakened store presents after conversion.
fn replace_event_sequence_uniqueness(root: &Path) {
    let connection = Connection::open(root.join(RELEASED_CONVERSATION_DATABASE)).unwrap();
    connection
        .execute_batch(
            "PRAGMA foreign_keys=OFF;
             CREATE TABLE events_rebuilt (
               id TEXT PRIMARY KEY,
               conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
               sequence INTEGER NOT NULL,
               author_membership_id TEXT REFERENCES memberships(id),
               kind TEXT NOT NULL, causation_id TEXT, correlation_id TEXT,
               created_at INTEGER NOT NULL,
               finalized INTEGER NOT NULL DEFAULT 0 CHECK(finalized IN (0,1)),
               CHECK(1)
             );
             INSERT INTO events_rebuilt
               SELECT id, conversation_id, sequence, author_membership_id, kind,
                      causation_id, correlation_id, created_at, finalized
                 FROM events;
             DROP TABLE events;
             ALTER TABLE events_rebuilt RENAME TO events;",
        )
        .unwrap();
}

/// A refusal leaves an existing ledger, high-water and every store byte for
/// byte as they were.
#[test]
fn admission_preserves_existing_ledger_and_stores_when_it_refuses() {
    let cases: [(&str, fn(&Path), &str); 6] = [
        (
            "a truncated older published store",
            truncate_older_conversation_store,
            SOURCE_FRONTIER_ID,
        ),
        (
            "a non-unique membership identity index",
            weaken_membership_identity_index,
            SOURCE_FRONTIER_ID,
        ),
        (
            // This case first admits the root, so its existing ledger already
            // names this binary's target; the refusal must not move it.
            "a current store missing the conversation title",
            remove_conversation_title,
            "licoup-state-0.3.0",
        ),
        (
            "a current store missing a principal business field",
            remove_current_principal_display_name,
            "licoup-state-0.3.0",
        ),
        (
            "a released store missing a principal business field",
            remove_released_principal_display_name,
            SOURCE_FRONTIER_ID,
        ),
        (
            "a released store without event sequence uniqueness",
            replace_event_sequence_uniqueness,
            SOURCE_FRONTIER_ID,
        ),
    ];
    for (label, mutate, expected_frontier) in cases {
        let root =
            std::env::temp_dir().join(format!("licoup-refusal-preserves-{}", uuid::Uuid::new_v4()));
        seed_released_source_root(&root);
        mutate(&root);
        let ledger_path = root.join("client-state/migrations/ledger.json");
        let conversation_path = root.join(RELEASED_CONVERSATION_DATABASE);
        let strategy_path = root.join(strategy_store::STRATEGY_STORE_DATABASE);
        let before = [
            fs::read(&ledger_path).unwrap(),
            fs::read(&conversation_path).unwrap(),
            fs::read(&strategy_path).unwrap(),
        ];
        let artifact_path = strategy_store::strategy_store_artifact_path(&root);
        let artifact_before = artifact_path
            .exists()
            .then(|| fs::read(&artifact_path).unwrap());

        assert_eq!(
            admit_as_version(&root, "0.3.0").unwrap_err().to_string(),
            "unsupported_state_shape",
            "{label}"
        );
        assert_eq!(
            fs::read(&ledger_path).unwrap(),
            before[0],
            "{label}: ledger"
        );
        assert_eq!(
            fs::read(&conversation_path).unwrap(),
            before[1],
            "{label}: conversation store"
        );
        assert_eq!(
            fs::read(&strategy_path).unwrap(),
            before[2],
            "{label}: strategy store"
        );
        let ledger: Ledger = serde_json::from_slice(&before[0]).unwrap();
        assert_eq!(ledger.frontier_id, expected_frontier, "{label}");
        assert!(
            !ledger.highest_admitted_product_version.is_empty(),
            "{label}: the high-water survived"
        );
        if artifact_path.exists() {
            assert_eq!(
                fs::read(&artifact_path).unwrap(),
                artifact_before.clone().unwrap(),
                "{label}: an existing conversion record is preserved"
            );
        } else {
            assert!(artifact_before.is_none(), "{label}: no conversion record");
        }
        let _ = fs::remove_dir_all(root);
    }
}

/// The published producer upgraded older stores in place, adding
/// `strategy_runs.terminal` through `ensure_column`, which leaves the column
/// nullable. That variant is a valid source within the same product and
/// frontier, and its business rows must convert and read back through the
/// owner.
#[test]
fn a_producer_upgraded_strategy_store_is_admitted_and_read_back() {
    let root =
        std::env::temp_dir().join(format!("licoup-producer-upgraded-{}", uuid::Uuid::new_v4()));
    seed_released_source_root(&root);
    let path = root.join(strategy_store::STRATEGY_STORE_DATABASE);
    fs::remove_file(&path).unwrap();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(&released_strategy_schema_producer_upgraded())
        .unwrap();
    connection.execute_batch(&released_strategy_rows()).unwrap();
    assert_eq!(
        strategy_store::read_strategy_store_format(&path)
            .unwrap()
            .format_id,
        "strategy-store-2"
    );
    drop(connection);

    let result = admit_as_version(&root, "0.3.0").unwrap();
    assert!(
        result
            .applied_domain_ids
            .iter()
            .any(|domain| domain == "adaptive-flywheel")
    );
    let strategy = crate::domain::workflow_store::StrategyStore::open(&root).unwrap();
    let snapshot = strategy.run(RELEASED_RUN_ID).unwrap();
    assert_eq!(
        snapshot.status,
        licoup_workflow::ir::StrategyRunStatus::Completed
    );
    assert_eq!(
        snapshot.conversation_id.as_deref(),
        Some(RELEASED_CONVERSATION_ID)
    );
    let definition = strategy
        .definition_by_revision(RELEASED_DEFINITION_REVISION)
        .unwrap();
    assert!(definition.workflow.actor_slots[0].entry);
    assert_eq!(definition.bindings[0].slot_id, "actor");
    assert!(definition.authorization.unwrap().active);
    let _ = fs::remove_dir_all(root);
}

/// `CREATE INDEX IF NOT EXISTS` never repairs an existing index, and a
/// predicate that merely contains `active=1` is a different uniqueness
/// constraint, so the partial predicate is compared exactly.
#[test]
fn a_weakened_authorization_predicate_is_refused() {
    let root = std::env::temp_dir().join(format!("licoup-weak-predicate-{}", uuid::Uuid::new_v4()));
    seed_released_source_root(&root);
    let path = root.join(strategy_store::STRATEGY_STORE_DATABASE);
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "DROP INDEX strategy_authorization_active_idx;
             CREATE UNIQUE INDEX strategy_authorization_active_idx
               ON strategy_authorizations(revision_digest) WHERE active=1 AND 0;",
        )
        .unwrap();
    drop(connection);
    let ledger_path = root.join("client-state/migrations/ledger.json");
    let before = [fs::read(&path).unwrap(), fs::read(&ledger_path).unwrap()];

    assert_eq!(
        admit_as_version(&root, "0.3.0").unwrap_err().to_string(),
        "unsupported_state_shape"
    );
    assert_eq!(fs::read(&path).unwrap(), before[0]);
    assert_eq!(fs::read(&ledger_path).unwrap(), before[1]);
    let _ = fs::remove_dir_all(root);
}

/// The released layout stamped as the adjacent legacy version is still a
/// legitimate owner-supported source: the in-memory upgrade proof accepts it,
/// the real upgrade runs, and the owner reads the business rows back.
#[test]
fn a_released_layout_stamped_as_legacy_is_upgraded_and_read_back() {
    let root = std::env::temp_dir().join(format!(
        "licoup-legacy-stamped-released-{}",
        uuid::Uuid::new_v4()
    ));
    seed_released_source_root(&root);
    let database = root.join(RELEASED_CONVERSATION_DATABASE);
    Connection::open(&database)
        .unwrap()
        .execute("UPDATE schema_meta SET value='11' WHERE key='version'", [])
        .unwrap();
    let result = admit_as_version(&root, "0.3.0").unwrap();
    assert!(
        result
            .skipped_domain_ids
            .iter()
            .any(|domain| domain == "canonical-conversation"),
        "the existing store keeps its domain version while its inner schema is upgraded"
    );
    let inner: String = Connection::open(&database)
        .unwrap()
        .query_row(
            "SELECT value FROM schema_meta WHERE key='version'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(inner, licoup_conversation::store::CURRENT_SCHEMA_VERSION);
    let conversations = crate::domain::client_conversation::ConversationStore::open(&root).unwrap();
    let conversation = conversations.get(RELEASED_CONVERSATION_ID).unwrap();
    assert_eq!(conversation.title, "Synthetic released conversation");
    let page = conversations
        .page_events(RELEASED_CONVERSATION_ID, None, 10)
        .unwrap();
    assert_eq!(page.events.len(), 1);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_current_shape_root_is_admitted_without_rewriting_its_files() {
    let root = std::env::temp_dir().join(format!(
        "licoup-current-shape-admission-{}",
        uuid::Uuid::new_v4()
    ));
    admit(&root).unwrap();
    let before = durable_file_bytes(&root);

    let result = admit(&root).unwrap();

    assert!(
        result.applied_domain_ids.is_empty(),
        "a current-shape root moves no domain"
    );
    assert_eq!(result.frontier_id, embedded_frontier().unwrap().frontier_id);
    assert_eq!(
        durable_file_bytes(&root),
        before,
        "admission of a current-shape root must not rewrite a durable file"
    );
    let _ = fs::remove_dir_all(root);
}

/// A database that only declares the current schema version is not a
/// conversation store: the admission reads the completion marker, so a
/// fabricated file would otherwise become the canonical owner's state.
#[test]
fn a_fabricated_conversation_store_is_refused_rather_than_admitted() {
    let root = std::env::temp_dir().join(format!(
        "licoup-conversation-fabricated-{}",
        uuid::Uuid::new_v4()
    ));
    let database = root.join("client-state/conversations/conversations.sqlite3");
    fs::create_dir_all(database.parent().unwrap()).unwrap();
    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE schema_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 INSERT INTO schema_meta(key,value) VALUES ('version','18');
                 CREATE TABLE conversations(
                   conversation_id TEXT PRIMARY KEY, title TEXT NOT NULL,
                   created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
                 );
                 CREATE TABLE conversation_messages(
                   message_id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL,
                   role TEXT NOT NULL, content TEXT NOT NULL, created_at INTEGER NOT NULL
                 );",
        )
        .unwrap();
    drop(connection);
    fs::write(
        root.join("client-state/conversations/migration-v5.complete"),
        "schema=v5\nstatus=complete\n",
    )
    .unwrap();

    assert_eq!(
        admit(&root).unwrap_err().to_string(),
        "unsupported_state_shape"
    );
    // The refusal happens before any ledger or domain marker is written, so
    // the root is left exactly as it was found.
    assert!(!root.join("client-state/migrations/ledger.json").exists());
    assert!(
        !root
            .join("client-state/migrations/domain-state/canonical-conversation.json")
            .exists()
    );
    let _ = fs::remove_dir_all(root);
}

/// A stamped version on anything less than a real Conversation layout is
/// refused before the root's ledger, high-water or markers advance.
///
/// The removed partial check accepted `schema_meta` plus two
/// `conversations` columns; the owner preflight requires the owner's own
/// current table set and columns, and rejects development snapshots outright.
#[test]
fn a_stamped_conversation_fake_is_refused_before_any_advance() {
    let cases = [
        (
            "a current stamp on two tables",
            "CREATE TABLE schema_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO schema_meta(key,value) VALUES ('version','18');
             CREATE TABLE conversations(id TEXT PRIMARY KEY, title TEXT NOT NULL);",
        ),
        (
            "a development snapshot",
            "CREATE TABLE schema_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO schema_meta(key,value) VALUES ('version','15');",
        ),
    ];
    for (label, statements) in cases {
        let root =
            std::env::temp_dir().join(format!("licoup-conversation-fake-{}", uuid::Uuid::new_v4()));
        let database = root.join("client-state/conversations/conversations.sqlite3");
        fs::create_dir_all(database.parent().unwrap()).unwrap();
        let connection = Connection::open(&database).unwrap();
        connection.execute_batch(statements).unwrap();
        drop(connection);
        fs::write(
            root.join("client-state/conversations/migration-v5.complete"),
            "schema=v5\nstatus=complete\n",
        )
        .unwrap();
        let before = fs::read(&database).unwrap();

        assert_eq!(
            admit(&root).unwrap_err().to_string(),
            "unsupported_state_shape",
            "{label}"
        );
        assert_eq!(
            fs::read(&database).unwrap(),
            before,
            "{label}: the refused store keeps its bytes"
        );
        assert!(
            !root.join("client-state/migrations/ledger.json").exists(),
            "{label}: the refusal must not advance the ledger"
        );
        assert!(
            !root.join("client-state/migrations/domain-state").exists(),
            "{label}: the refusal must not write a domain marker"
        );
        let _ = fs::remove_dir_all(root);
    }
}

/// A domain converted without a store file reports the version it is at.
///
/// Most domains own no store file: admission writes their durable marker and
/// nothing else. The projection resolves a version the way the admission does —
/// a store that reports a version above zero is authoritative, otherwise the
/// domain's own marker is — so a converted domain never reads as version zero
/// to a consumer of the raw projection. The version comes from the marker only
/// because no store exists, and reading it must not create one.
#[test]
fn a_converted_domain_without_a_store_file_reports_its_marker_version() {
    let root = std::env::temp_dir().join(format!(
        "licoup-migration-projection-{}",
        uuid::Uuid::new_v4()
    ));
    // The domains below are converted by their marker alone; their store files
    // are never written, which is what the projection must report without
    // inventing one.
    let storeless = [
        root.join("client-state/agent-tab-order.json"),
        root.join("client-state/current-client-view.json"),
        root.join("client-state/skill-hub-preferences.json"),
    ];
    let admission = admit(&root).unwrap();
    assert!(storeless.iter().all(|path| !path.exists()));

    let states = domain_state_projection(&root).unwrap();
    assert_eq!(
        states.len(),
        embedded_frontier().unwrap().domains.len(),
        "every declared domain is projected"
    );
    let mut converted = 0;
    for state in &states {
        // Platform credential custody is deliberately deferred on macOS: a data
        // root alone cannot prove the account holds no legacy Keychain items, so
        // its version stays at zero until the protected operation completes.
        if admission
            .pending_authorization_domain_ids
            .contains(&state.domain_id)
        {
            continue;
        }
        assert_eq!(
            state.marker_schema_version,
            Some(state.target_schema_version),
            "{} was admitted, so its marker records the target",
            state.domain_id
        );
        assert_eq!(
            state.authority,
            DomainAuthority::Known {
                version: state.target_schema_version
            },
            "{} is converted without a store file, so its authority is the \
             marker's version instead of zero",
            state.domain_id
        );
        converted += 1;
    }
    assert!(converted > 0, "a fresh root converts every domain it can");
    assert!(
        storeless.iter().all(|path| !path.exists()),
        "projecting a domain must not fabricate the store file it has none of"
    );
    let _ = fs::remove_dir_all(root);
}

/// A store the owner cannot read never hides the other domains' versions.
///
/// The projection answers per domain. A store document whose version marker is not
/// a version is refused by the domain's own probe, and that one domain reads as
/// version 0 without failing the read for the root's other domains; a converted
/// neighbour keeps reporting the version its durable marker records.
#[test]
fn an_unreadable_store_never_hides_the_other_domains_versions() {
    let root = std::env::temp_dir().join(format!(
        "licoup-migration-unreadable-store-{}",
        uuid::Uuid::new_v4()
    ));
    admit(&root).unwrap();
    fs::write(
        root.join("client-state/agent-tool-allowlists.json"),
        b"{\"schemaVersion\":\"one\"}",
    )
    .unwrap();

    let states = domain_state_projection(&root).unwrap();
    assert_eq!(
        states.len(),
        embedded_frontier().unwrap().domains.len(),
        "an unreadable store must not shorten the projection"
    );
    let unreadable = states
        .iter()
        .find(|state| state.domain_id == "agent-tool-allowlist")
        .unwrap();
    assert_eq!(
        unreadable.authority,
        DomainAuthority::Refused {
            code: "unsupported_state_shape".to_owned()
        },
        "a refused store is reported as a refusal, never flattened to version zero"
    );
    let readable = states
        .iter()
        .find(|state| state.domain_id == "current-view")
        .unwrap();
    assert_eq!(
        readable.authority,
        DomainAuthority::Known {
            version: readable.target_schema_version
        },
        "a converted neighbour still reports the version its marker records"
    );
    let _ = fs::remove_dir_all(root);
}

/// A marker ahead of this binary is a typed refusal, not a version and not a
/// failure of the whole projection.
///
/// The tolerant resolution of an unreadable store must not swallow the marker's
/// own refusal: a domain whose marker claims more than the embedded frontier
/// supports is a state the client refuses, and the projection says so per
/// domain so every other domain stays readable.
#[test]
fn a_marker_ahead_of_the_binary_is_reported_as_a_refusal() {
    let root = std::env::temp_dir().join(format!(
        "licoup-migration-ahead-marker-{}",
        uuid::Uuid::new_v4()
    ));
    let marker_root = root.join("client-state/migrations/domain-state");
    licoup_foundation::platform::file_security::ensure_private_dir(&marker_root).unwrap();
    let domain = embedded_frontier().unwrap().domains[0].clone();
    write_json_atomic(
        &marker_path(&marker_root, &domain.domain_id),
        &DomainMarker {
            schema_version: DOMAIN_MARKER_SCHEMA.to_owned(),
            domain_id: domain.domain_id.clone(),
            authoritative_schema_version: domain.target_schema_version + 1,
        },
    )
    .unwrap();
    let states = domain_state_projection(&root).unwrap();
    assert_eq!(
        states.len(),
        embedded_frontier().unwrap().domains.len(),
        "one refused domain must not shorten the projection"
    );
    let ahead = states
        .iter()
        .find(|state| state.domain_id == domain.domain_id)
        .unwrap();
    assert_eq!(
        ahead.authority,
        DomainAuthority::Refused {
            code: "state_newer_than_binary".to_owned()
        }
    );
    let other = states
        .iter()
        .find(|state| state.domain_id == "workspace-manifest")
        .unwrap();
    assert_eq!(
        other.authority,
        DomainAuthority::Absent,
        "an untouched root's other domains stay absent, not refused"
    );
    let _ = fs::remove_dir_all(root);
}

/// An absent authority is not a refused one.
#[test]
fn projection_distinguishes_an_absent_domain_from_a_refused_one() {
    let root = std::env::temp_dir().join(format!(
        "licoup-migration-absent-refused-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&root).unwrap();
    let states = domain_state_projection(&root).unwrap();
    let absent = states
        .iter()
        .find(|state| state.domain_id == "workspace-manifest")
        .unwrap();
    assert_eq!(absent.authority, DomainAuthority::Absent);
    assert_eq!(absent.marker_schema_version, None);

    // A corrupt store document is a refusal for that one domain.
    fs::write(root.join(".licoup-workspace.json"), b"{not json").unwrap();
    let states = domain_state_projection(&root).unwrap();
    let corrupt = states
        .iter()
        .find(|state| state.domain_id == "workspace-manifest")
        .unwrap();
    assert_eq!(
        corrupt.authority,
        DomainAuthority::Refused {
            code: "unsupported_state_shape".to_owned()
        }
    );
    let absent = states
        .iter()
        .find(|state| state.domain_id == "current-view")
        .unwrap();
    assert_eq!(absent.authority, DomainAuthority::Absent);
    let _ = fs::remove_dir_all(root);
}

/// After the owner committed the store layout but before the bookkeeping, the
/// projection reports the committed authority even though the durable marker
/// still carries the released version.
#[test]
fn projection_reports_committed_authority_after_an_interrupted_store_commit() {
    let root = std::env::temp_dir().join(format!(
        "licoup-migration-committed-authority-{}",
        uuid::Uuid::new_v4()
    ));
    seed_released_source_root(&root);
    {
        let _guard = MigrationFailpointGuard::set("after-strategy-store-edge");
        assert_eq!(
            admit_as_version(&root, "0.3.0").unwrap_err().to_string(),
            "migration_step_failed"
        );
    }
    // The physical store committed the current layout; the marker was not
    // written yet, and the projection must not lower the committed authority.
    let states = domain_state_projection(&root).unwrap();
    let adaptive = states
        .iter()
        .find(|state| state.domain_id == "adaptive-flywheel")
        .unwrap();
    assert_eq!(adaptive.marker_schema_version, Some(1));
    assert_eq!(
        adaptive.authority,
        DomainAuthority::Known { version: 2 },
        "the committed store is the authority, not the stale marker"
    );

    admit_as_version(&root, "0.3.0").unwrap();
    let states = domain_state_projection(&root).unwrap();
    let adaptive = states
        .iter()
        .find(|state| state.domain_id == "adaptive-flywheel")
        .unwrap();
    assert_eq!(adaptive.marker_schema_version, Some(2));
    assert_eq!(adaptive.authority, DomainAuthority::Known { version: 2 });
    let _ = fs::remove_dir_all(root);
}

/// The catalog declares exactly the two conversion endpoints, and a reader
/// enumerates the pair instead of restating a format name.
///
/// The source is the format the last published client left, the target is this
/// binary's own frontier, and the two are distinct: one format is never both
/// halves of the same conversion.
#[test]
fn the_declared_conversion_endpoints_are_the_published_source_and_the_binary_target() {
    let frontier = embedded_frontier().unwrap();
    let endpoints = conversion_endpoints().unwrap();
    assert_eq!(
        endpoints,
        ConversionEndpoints {
            source_frontier_id: frontier.source_frontier_id.clone(),
            target_frontier_id: frontier.frontier_id.clone(),
        },
        "the enumerated endpoints are the catalog's own declaration"
    );
    assert_ne!(
        endpoints.source_frontier_id, endpoints.target_frontier_id,
        "one format cannot be both endpoints"
    );
    assert!(
        is_frontier_identity(&endpoints.source_frontier_id)
            && is_frontier_identity(&endpoints.target_frontier_id),
        "both endpoints are frontier identities"
    );
    assert!(
        frontier
            .require_declared_format(&endpoints.source_frontier_id)
            .is_ok()
            && frontier
                .require_declared_format(&endpoints.target_frontier_id)
                .is_ok(),
        "both declared endpoints are admitted formats"
    );
}

/// Only the declared source is converted, and only this binary's own target is a
/// rerun. Any other name is refused as an unsupported source before the ledger's
/// high-water advances or a domain moves, so an older published format stays
/// release history instead of becoming a second supported source.
#[test]
fn admission_refuses_a_root_that_names_a_format_outside_the_declared_pair() {
    let frontier = embedded_frontier().unwrap();
    let root = std::env::temp_dir().join(format!(
        "licoup-migration-declared-source-{}",
        uuid::Uuid::new_v4()
    ));
    let migration_root = root.join("client-state/migrations");
    licoup_foundation::platform::file_security::ensure_private_dir(&migration_root).unwrap();
    let ledger_path = migration_root.join("ledger.json");
    let mut ledger = load_ledger(&ledger_path, &frontier).unwrap();

    // `licoup-state-0.2.1` is an unpublished spelling and `licoup-state-0.2.2`
    // an unpublished correction; the declaration offers neither as a conversion
    // endpoint, and the released source `licoup-state-0.1.1` is the only older
    // name the admission converts.
    for unsupported in ["licoup-state-0.2.1", "licoup-state-0.2.2"] {
        ledger.frontier_id = unsupported.to_owned();
        write_json_atomic(&ledger_path, &ledger).unwrap();
        let before = fs::read(&ledger_path).unwrap();
        assert_eq!(
            admit(&root).unwrap_err().to_string(),
            "unsupported_state_shape",
            "{unsupported} has no conversion path"
        );
        assert_eq!(
            fs::read(&ledger_path).unwrap(),
            before,
            "the refusal writes no ledger"
        );
        assert!(
            !migration_root.join("domain-state").exists(),
            "the refusal converts no domain"
        );
    }

    // The declared source is the one older format the admission converts, and the
    // ledger then names this binary's own target.
    ledger.frontier_id = frontier.source_frontier_id.clone();
    write_json_atomic(&ledger_path, &ledger).unwrap();
    assert_eq!(admit(&root).unwrap().frontier_id, frontier.frontier_id);
    assert_eq!(
        load_ledger(&ledger_path, &frontier).unwrap().frontier_id,
        frontier.frontier_id,
        "a converted root records this binary's own target"
    );
    let _ = fs::remove_dir_all(root);
}

/// The signed update frontier that leaves this client is exactly the released
/// wire shape: `{frontierId, domains}` with `{domainId, targetSchemaVersion,
/// requiredStepIds}` per domain, and every released domain keeps the released
/// step ids as a prefix. `sourceFrontierId` is internal catalog state and must
/// never appear on the signed wire or in a handoff.
#[test]
fn the_signed_update_wire_is_closed_and_keeps_the_released_prefixes() {
    let projection = frontier_projection().unwrap();
    let object = projection.as_object().unwrap();
    let keys = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    assert_eq!(
        keys,
        BTreeSet::from(["frontierId", "domains"]),
        "the signed frontier contract is closed"
    );
    let domains = object["domains"].as_array().unwrap();
    let by_id = domains
        .iter()
        .map(|domain| (domain["domainId"].as_str().unwrap(), domain))
        .collect::<BTreeMap<_, _>>();
    for domain in domains {
        let keys = domain
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            keys,
            BTreeSet::from(["domainId", "targetSchemaVersion", "requiredStepIds"]),
            "every signed frontier domain contract is closed"
        );
    }
    // The released validator requires all of its domains to survive, with a
    // nonregressing version and the released requiredStepIds as a prefix.
    for domain_id in RELEASED_DOMAINS {
        let domain = by_id[domain_id];
        assert!(
            domain["targetSchemaVersion"].as_u64().unwrap() >= 1,
            "{domain_id} regressed"
        );
        let steps = domain["requiredStepIds"].as_array().unwrap();
        assert_eq!(
            steps[0].as_str().unwrap(),
            released_step_id(domain_id),
            "{domain_id} rewrites released migration history"
        );
    }
    // The candidate adds its own domain without rewriting the released list:
    // the projection carries this binary's own catalog step ids.
    let embedded = embedded_frontier().unwrap();
    let gateway = embedded
        .domains
        .iter()
        .find(|domain| domain.domain_id == "gateway-credential-custody")
        .unwrap();
    assert_eq!(
        gateway.steps[0].step_id,
        "gateway-credential-custody.classic-to-data-protection"
    );
    assert_eq!(
        by_id["gateway-credential-custody"]["requiredStepIds"][0]
            .as_str()
            .unwrap(),
        gateway.steps[0].step_id
    );
    // The internal source endpoint is not part of the signed wire.
    assert!(!projection.to_string().contains("sourceFrontierId"));
    assert!(!projection.to_string().contains(SOURCE_FRONTIER_ID));
}

#[test]
fn custody_disposition_is_consumed_independently_of_the_test_host() {
    for disposition in [
        LegacyCredentialMigrationDisposition::RequiresAuthorization,
        LegacyCredentialMigrationDisposition::NotApplicable,
    ] {
        let root = std::env::temp_dir().join(format!(
            "licoup-custody-disposition-{}",
            uuid::Uuid::new_v4()
        ));
        seed_released_source_root(&root);
        let inventory_path = root.join(RELEASED_INVENTORY_FILE);
        let before = fs::read(&inventory_path).unwrap();
        let result =
            admit_with_credential_migration_disposition(&root, "0.3.0", disposition).unwrap();
        let pending = disposition == LegacyCredentialMigrationDisposition::RequiresAuthorization;
        assert_eq!(
            result
                .pending_authorization_domain_ids
                .contains(&GATEWAY_CUSTODY_DOMAIN.to_owned()),
            pending
        );
        assert_eq!(
            root.join("client-state/migrations/domain-state/gateway-credential-custody.json")
                .exists(),
            !pending
        );
        assert_eq!(fs::read(inventory_path).unwrap(), before);
        fs::remove_dir_all(root).unwrap();
    }
}
