//! Synthetic owned-inventory fixtures.
//!
//! Every manifest here is built from literal JSON in the archive owner's own
//! layout. No fixture reads a real data root, a real archive, a protected key or
//! any provider value: the credential cases use a made-up domain name and a
//! made-up key string that only ever exists in this file, to show that a value
//! cannot reach a report.

use licoup_endpoint_collaboration_transfer::{
    ActivationReason, CredentialCustody, InventoryCompleteness, LimitationKind, ManagedDomain,
    TransferInventory, TransferOwnership,
};
use licoup_foundation::core::full_data_root_archive::{
    ArchiveManifest, InventoryKind, RecoveryCoverage,
};

/// A credential value that must never appear in any inventory report.
const SYNTHETIC_KEY_VALUE: &str = "sk-liveCANARYCREDENTIAL1234567890";

/// One file member in the archive-owner layout.
fn file(path: &str, size: u64) -> serde_json::Value {
    serde_json::json!({ "path": path, "kind": "file", "size": size })
}

/// One directory member in the archive-owner layout.
fn directory(path: &str) -> serde_json::Value {
    serde_json::json!({ "path": path, "kind": "directory", "size": 0 })
}

/// Build a manifest exactly as the archive owner serializes one.
fn manifest(coverage: &str, limitations: serde_json::Value, entries: Vec<serde_json::Value>) -> ArchiveManifest {
    let total_bytes: u64 = entries
        .iter()
        .map(|entry| entry["size"].as_u64().unwrap_or_default())
        .sum();
    let value = serde_json::json!({
        "layout": "licoup.full-data-root/v1",
        "container": "zip",
        "source_home": "/synthetic/state/root",
        "created_at_unix": 1_700_000_000_u64,
        "coverage": coverage,
        "limitations": limitations,
        "entries": entries,
        "total_bytes": total_bytes,
    });
    serde_json::from_value(value).expect("the synthetic manifest is in the archive owner's layout")
}

/// Every declared area of the data root is attributed, and every declared
/// limitation is classified: the inventory is complete as a description.
///
/// The areas match the ones the archive owner's own synthetic data root writes,
/// including the owner files that live at the data-root root rather than under
/// the client-state directory.
fn complete_manifest() -> ArchiveManifest {
    manifest(
        "limited",
        serde_json::json!([
            {
                "domain": "gateway-credential-custody",
                "reason": "llm-api-key-inventory.json travels as non-secret metadata; credential key material is not transported and its availability is determined by the credential owner",
            },
            {
                "domain": "provider-retained-history",
                "reason": "selected archives retained by the provider account are not part of the captured root",
            },
        ]),
        vec![
            directory("client-state"),
            file("client-state/preferences.json", 512),
            file("client-state/appearance-preferences.json", 128),
            directory("client-state/conversations"),
            file("client-state/conversations/conversations.sqlite3", 8192),
            file("client-state/conversations/migration-v5.complete", 0),
            file("client-state/migrations/ledger.json", 256),
            // The areas the migration owner records at the data-root root.
            file(".licoup-workspace.json", 192),
            file("adaptive-flywheel.toml", 320),
            directory("group-conversations"),
            file("group-conversations/group-1.json", 640),
            file("activity/activity.jsonl", 1024),
            file("snapshots/snapshot-1.json", 2048),
            file("archives/backup.zip", 4096),
            file("cache/target-discovery.json", 128),
            file("temp/scratch.tmp", 64),
            file("logs/client.log", 2048),
            // Non-secret metadata a person's key inventory lives beside; the
            // value itself is never written anywhere by this fixture.
            file("llm-api-key-inventory.json", 96),
        ],
    )
}

/// An archive whose capture missed an area, left an owner-unclaimed path, and
/// declared a limitation nothing classifies.
fn limited_manifest() -> ArchiveManifest {
    manifest(
        "limited",
        serde_json::json!([
            {
                "domain": "gateway-credential-custody",
                "reason": "credential key material is not transported",
            },
            {
                "domain": "an-owner-nobody-declared",
                "reason": "this domain is declared by a future owner",
            },
        ]),
        vec![
            directory("client-state"),
            file("client-state/preferences.json", 512),
            // No declaration claims this area.
            file("scratch-from-a-future-owner/blob.bin", 4096),
        ],
    )
}

#[test]
fn a_complete_inventory_separates_managed_external_and_credential_domains() {
    let inventory =
        TransferInventory::compose(&complete_manifest(), &TransferOwnership::declared()).unwrap();

    assert_eq!(inventory.completeness(), InventoryCompleteness::Complete);
    // The archive owner's own coverage is preserved, not upgraded.
    assert_eq!(inventory.declared_coverage(), RecoveryCoverage::Limited);
    assert!(inventory.unattributed().is_empty());
    assert!(inventory.unexplained().is_empty());

    let domain_of = |path: &str| {
        inventory
            .managed()
            .iter()
            .find(|payload| payload.path == path)
            .map(|payload| payload.domain)
            .unwrap_or_else(|| panic!("{path} is attributed"))
    };
    assert_eq!(
        domain_of("client-state/conversations/conversations.sqlite3"),
        ManagedDomain::Conversation,
        "the specific area refines the general client-state declaration"
    );
    assert_eq!(
        domain_of("client-state/preferences.json"),
        ManagedDomain::ClientState
    );
    assert_eq!(
        domain_of("client-state/migrations/ledger.json"),
        ManagedDomain::MigrationJournal
    );
    assert_eq!(domain_of("activity/activity.jsonl"), ManagedDomain::Activity);
    assert_eq!(
        domain_of("snapshots/snapshot-1.json"),
        ManagedDomain::Snapshot
    );
    assert_eq!(domain_of("archives/backup.zip"), ManagedDomain::Archive);
    assert_eq!(domain_of("cache/target-discovery.json"), ManagedDomain::Cache);
    assert_eq!(domain_of("temp/scratch.tmp"), ManagedDomain::Temp);
    assert_eq!(domain_of("logs/client.log"), ManagedDomain::Logs);

    // The areas a real capture records at the data-root root are attributed too,
    // including the non-secret credential-inventory document.
    assert_eq!(
        domain_of(".licoup-workspace.json"),
        ManagedDomain::Workspace
    );
    assert_eq!(
        domain_of("adaptive-flywheel.toml"),
        ManagedDomain::ClientState
    );
    assert_eq!(
        domain_of("group-conversations/group-1.json"),
        ManagedDomain::Conversation
    );
    assert_eq!(
        domain_of("llm-api-key-inventory.json"),
        ManagedDomain::ClientState
    );

    // Provider-retained history is an external reference, not a managed payload.
    assert_eq!(inventory.external().len(), 1);
    assert_eq!(inventory.external()[0].domain, "provider-retained-history");

    // The credential domain is a requirement, and only a requirement.
    assert_eq!(inventory.credentials().len(), 1);
    assert_eq!(
        inventory.credentials()[0].custody,
        CredentialCustody::ProviderKeyReentry
    );

    // Caches, temporaries and logs travel in the archive but are not retained
    // content: their owners rebuild them, so they are not bytes a person must get
    // back for the transfer to be complete.
    for path in [
        "cache/target-discovery.json",
        "temp/scratch.tmp",
        "logs/client.log",
    ] {
        assert!(!domain_of(path).is_retained_content());
    }
    assert_eq!(
        inventory.retained_bytes(),
        512 + 128 + 8192 + 256 + 192 + 320 + 640 + 1024 + 2048 + 4096 + 96
    );
}

#[test]
fn a_complete_inventory_still_requires_fresh_identity_authority() {
    let inventory =
        TransferInventory::compose(&complete_manifest(), &TransferOwnership::declared()).unwrap();
    let requirement = inventory.activation_requirement();

    assert!(
        !requirement.permits_activation(),
        "an inventory, however complete, never activates an identity"
    );
    assert!(
        requirement
            .reasons()
            .contains(&ActivationReason::BackupPossessionIsNotAuthority)
    );
    assert!(
        requirement
            .reasons()
            .contains(&ActivationReason::ProviderKeyReentry)
    );
}

#[test]
fn a_limited_inventory_names_every_unattributed_and_unexplained_domain() {
    let inventory =
        TransferInventory::compose(&limited_manifest(), &TransferOwnership::declared()).unwrap();

    assert_eq!(inventory.completeness(), InventoryCompleteness::Limited);
    assert_eq!(inventory.unattributed().len(), 1);
    assert_eq!(
        inventory.unattributed()[0].path,
        "scratch-from-a-future-owner/blob.bin"
    );
    assert_eq!(inventory.unexplained().len(), 1);
    assert_eq!(inventory.unexplained()[0].domain, "an-owner-nobody-declared");

    let limitations = inventory.limitations();
    let kinds = limitations
        .iter()
        .map(|limitation| limitation.kind)
        .collect::<Vec<_>>();
    assert!(kinds.contains(&LimitationKind::Credential(
        CredentialCustody::ProviderKeyReentry
    )));
    assert!(kinds.contains(&LimitationKind::Unattributed));
    assert!(kinds.contains(&LimitationKind::Unexplained));

    // The person is told what is missing, in fixed text, for every limitation.
    for limitation in &limitations {
        assert!(!limitation.requirement().is_empty());
    }
    assert!(
        !inventory.activation_requirement().permits_activation(),
        "an incomplete inventory is even less able to activate an identity"
    );
}

#[test]
fn no_report_field_can_carry_a_credential_value() {
    let inventory =
        TransferInventory::compose(&complete_manifest(), &TransferOwnership::declared()).unwrap();

    // The only text this component can produce for a credential domain is the
    // fixed classification constant. A value has nowhere to be written, which is
    // why this assertion is about the whole rendered report rather than a filter.
    let mut report = Vec::new();
    for limitation in inventory.limitations() {
        report.push(format!("{}: {}", limitation.domain, limitation.requirement()));
    }
    for reason in inventory.activation_requirement().reasons() {
        report.push(reason.requirement().to_string());
    }
    let report = report.join("\n");
    assert!(!report.contains(SYNTHETIC_KEY_VALUE));
    assert!(
        report.contains("re-enter them on this device"),
        "the person is told what to do about the key they must re-enter"
    );

    // The metadata-only member is attributed as client state, so the archive is
    // described honestly: the file travels, the key material does not.
    assert!(
        inventory
            .managed()
            .iter()
            .any(|payload| payload.path == "llm-api-key-inventory.json"
                && payload.domain == ManagedDomain::ClientState)
    );
    assert_eq!(inventory.credentials().len(), 1);
}

#[test]
fn a_manifest_from_another_layout_is_refused() {
    let mut value = serde_json::to_value(complete_manifest()).unwrap();
    value["layout"] = serde_json::json!("licoup.full-data-root/other");
    let manifest: ArchiveManifest = serde_json::from_value(value).unwrap();

    let error = TransferInventory::compose(&manifest, &TransferOwnership::declared())
        .expect_err("another layout is not a transfer inventory");
    assert_eq!(error.to_string(), "transfer_inventory_layout_unsupported");
}

#[test]
fn a_declared_prefix_that_is_not_a_portable_path_is_refused() {
    for prefix in ["/absolute", "trailing/", "a//b", "../escape", "c:\\windows", ""] {
        let result = TransferOwnership::new(
            [(prefix.to_string(), ManagedDomain::Cache)],
            Vec::new(),
            Vec::new(),
        );
        assert!(result.is_err(), "prefix {prefix:?} must be refused");
    }
    assert!(
        TransferOwnership::new(
            [("cache".to_string(), ManagedDomain::Cache)],
            Vec::new(),
            Vec::new(),
        )
        .is_ok()
    );
}

#[test]
fn the_declared_table_attributes_only_whole_path_components() {
    let ownership = TransferOwnership::new(
        [("cache".to_string(), ManagedDomain::Cache)],
        Vec::new(),
        Vec::new(),
    )
    .unwrap();

    let (managed, unattributed) = licoup_endpoint_collaboration_transfer::classify_entries(
        &[
            licoup_foundation::core::full_data_root_archive::InventoryEntry {
                path: "cache".to_string(),
                kind: InventoryKind::Directory,
                size: 0,
            },
            licoup_foundation::core::full_data_root_archive::InventoryEntry {
                path: "cache/blob".to_string(),
                kind: InventoryKind::File,
                size: 4,
            },
            licoup_foundation::core::full_data_root_archive::InventoryEntry {
                path: "cache-other/blob".to_string(),
                kind: InventoryKind::File,
                size: 4,
            },
        ],
        &ownership,
    );

    assert_eq!(managed.len(), 2, "a prefix owns itself and its descendants");
    assert_eq!(unattributed.len(), 1);
    assert_eq!(unattributed[0].path, "cache-other/blob");
}
