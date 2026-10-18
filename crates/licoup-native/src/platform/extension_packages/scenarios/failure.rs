//! A38 at component level: crash windows around the atomic rename, and the facts
//! that survive them. Recovery never rolls back a recorded installation and never
//! leaves an unrecorded one behind as a version nothing can see.

use super::*;
use crate::platform::extension_packages::artifact::content_digest;
use crate::platform::extension_packages::install::{FaultPlan, InstallPhase, InstallRequest};
use crate::platform::extension_packages::{
    DependentsDecision, InstalledPackage, InstanceRegistry, RemainingWork, UninstallTransaction,
    now_unix_ms, preview,
};
use crate::platform::package_registration_release::PackageRegistrationOwners;
use licoup_extension_contracts::deployment::{LocalCatalogue, PackageLifecycle, PackageSource};

/// The production registration adapter with no caller-supplied release inputs.
///
/// These scenarios record no external registration, so every owner is asked for
/// nothing and the reclaim proceeds. One shared value keeps a `'static` reference
/// available without each test naming or rebuilding the adapter.
static OWNERS: std::sync::LazyLock<PackageRegistrationOwners> =
    std::sync::LazyLock::new(PackageRegistrationOwners::default);

fn owners() -> &'static PackageRegistrationOwners {
    &OWNERS
}

#[test]
fn an_interrupted_publication_is_reclaimed_and_can_be_installed_again() {
    let (root, store) = store("activate-crash");
    let outside = sandbox("activate-crash-state");
    std::fs::create_dir_all(outside.join(ECHO)).expect("state root");
    std::fs::write(outside.join(ECHO).join("state.bin"), b"protocol state").expect("state");

    install_local(&store, ECHO, "1.0.0", None);
    let bytes = package_bytes(ECHO, "1.1.0", None, &[(NET, "self")], &[]);
    let request = InstallRequest::new(
        ECHO,
        "1.1.0",
        PackageSource::LocalImport,
        trust_for(&bytes, [net_permission()]),
    )
    .with_faults(FaultPlan::failing_at(InstallPhase::Activate));
    let failure = store
        .install(&request, &bytes)
        .expect_err("interrupted at the publish rename");
    assert_eq!(failure.code, "package_install_interrupted");
    assert!(
        store.installed_path(ECHO, "1.1.0").exists(),
        "the rename itself landed"
    );
    assert!(
        !store.record_path(ECHO, "1.1.0").exists(),
        "the host record never landed"
    );
    assert_eq!(
        store.installed().expect("installed").len(),
        1,
        "an unrecorded version is not installed"
    );

    let report = store.recover().expect("recover");
    assert!(
        report
            .abandoned
            .iter()
            .any(|stage| stage.package_id == ECHO && stage.version == "1.1.0"),
        "the unrecorded publication is named: {report:?}"
    );
    assert!(report.reclaimed_bytes > 0);
    assert!(report.installed_untouched.is_empty());
    assert!(!store.installed_path(ECHO, "1.1.0").exists());
    assert!(
        store
            .installed_path(ECHO, "1.0.0")
            .join("agent.py")
            .exists(),
        "the previous version keeps serving"
    );
    assert!(store.staged_directories().expect("staged").is_empty());
    assert!(
        outside.join(ECHO).join("state.bin").exists(),
        "recovery does not roll back state outside the store"
    );

    // The interrupted install left no zombie: the same version installs cleanly.
    install_local(&store, ECHO, "1.1.0", None);
    assert_eq!(store.installed().expect("installed").len(), 2);
    assert!(
        store.journal().committed(ECHO, "1.1.0").expect("journal"),
        "the clean retry commits the version"
    );

    cleanup(&outside);
    cleanup(&root);
}

#[test]
fn a_record_that_outlived_its_journal_commit_is_reconciled_as_installed() {
    let (root, store) = store("record-crash");
    let bytes = package_bytes(ECHO, "2.0.0", None, &[(NET, "self")], &[]);
    let digest = content_digest(&bytes);

    // Reproduce the crash window by hand: the rename and the record landed, the
    // process died before the journal commit line and the stage cleanup.
    let content = store.installed_path(ECHO, "2.0.0");
    crate::platform::extension_packages::ensure_private_directory(&content).expect("content");
    std::fs::write(
        content.join("manifest.json"),
        manifest_json(ECHO, "2.0.0", None, &[(NET, "self")]),
    )
    .expect("manifest");
    std::fs::write(content.join("agent.py"), b"print('echo')\n").expect("agent");
    let record = InstalledPackage {
        package_id: ECHO.to_owned(),
        version: "2.0.0".to_owned(),
        digest: digest.clone(),
        source: PackageSource::LocalImport,
        trust_channel: PackageLifecycle::LocalApproved,
        installed_at_unix_ms: now_unix_ms(),
        compressed_bytes: bytes.len() as u64,
        expanded_bytes: 32,
        entry_count: 2,
        install_scripts: Vec::new(),
        runtime_ref: None,
        permissions: vec![net_permission()],
        registrations: Vec::new(),
    };
    crate::platform::extension_packages::replace_file_atomically(
        &store.record_path(ECHO, "2.0.0"),
        &serde_json::to_string(&record).expect("record"),
    )
    .expect("record file");
    let stage = store.staging_path().join(format!(
        "2.0.0-{}",
        crate::platform::extension_packages::unique_suffix()
    ));
    crate::platform::extension_packages::ensure_private_directory(&stage).expect("stage");
    crate::platform::extension_packages::replace_file_atomically(
        &stage.join("stage.json"),
        &serde_json::json!({
            "packageId": ECHO,
            "version": "2.0.0",
            "digest": digest,
            "stagedAtUnixMs": now_unix_ms(),
        })
        .to_string(),
    )
    .expect("marker");
    assert!(
        !store.journal().committed(ECHO, "2.0.0").expect("journal"),
        "the journal never saw the commit"
    );

    let report = store.recover().expect("recover");
    assert_eq!(
        report.installed_untouched,
        vec![format!("{ECHO}@2.0.0")],
        "a recorded installation is not rolled back"
    );
    assert!(report.abandoned.is_empty());
    assert!(
        store.journal().committed(ECHO, "2.0.0").expect("journal"),
        "the journal commit is restored"
    );
    assert!(
        store
            .installed_path(ECHO, "2.0.0")
            .join("agent.py")
            .exists()
    );
    assert_eq!(store.installed().expect("installed").len(), 1);
    assert!(store.staged_directories().expect("staged").is_empty());
    cleanup(&root);
}

#[test]
fn an_interrupted_removal_is_completed_by_recovery() {
    let (root, store) = store("removal-crash");
    install_local(&store, ECHO, "1.0.0", None);
    // A crash after the removal intent was recorded, with the bytes already
    // partially gone.
    std::fs::remove_file(store.installed_path(ECHO, "1.0.0").join("agent.py"))
        .expect("partial removal");
    store
        .journal()
        .append(
            crate::platform::extension_packages::JournalOperation::Uninstall,
            ECHO,
            "1.0.0",
            PackageLifecycle::Available,
            Some("interrupted"),
        )
        .expect("intent");

    let report = store.recover().expect("recover");
    assert_eq!(report.finished_removals, vec![format!("{ECHO}@1.0.0")]);
    assert!(report.reclaimed_bytes > 0);
    assert!(
        !store.installed_path(ECHO, "1.0.0").exists(),
        "a half-removed version is not left visible"
    );
    assert!(store.installed().expect("installed").is_empty());
    cleanup(&root);
}

#[test]
fn a_reinstall_after_a_removal_intent_is_not_removed_by_recovery() {
    let (root, store) = store("removal-reinstall");
    install_local(&store, ECHO, "1.0.0", None);
    store.remove_installed(ECHO, "1.0.0").expect("removed");
    install_local(&store, ECHO, "1.0.0", None);

    let report = store.recover().expect("recover");
    assert!(
        report.finished_removals.is_empty(),
        "the later commit wins over the older removal intent"
    );
    assert!(report.installed_untouched.is_empty());
    assert!(
        store
            .installed_path(ECHO, "1.0.0")
            .join("agent.py")
            .exists()
    );
    assert_eq!(store.installed().expect("installed").len(), 1);
    cleanup(&root);
}

#[test]
fn a_restart_that_lost_an_instance_records_unknown_before_its_package_is_removed() {
    let (root, store) = store("restart");
    install_local(&store, ECHO, "1.0.0", None);
    let installed = store.installed().expect("installed");
    let bytes = store.installed_bytes(ECHO, "1.0.0").expect("bytes");
    let record_bytes = std::fs::metadata(store.record_path(ECHO, "1.0.0"))
        .expect("record")
        .len();

    let mut registry = InstanceRegistry::new();
    let instance_id = active_instance(&mut registry, ECHO, "1.0.0", 1);
    registry
        .get_mut(&instance_id)
        .expect("instance")
        .begin_in_flight()
        .expect("admitted work");

    // The host restarts and finds no instance running.
    let missing = registry.reconcile_after_restart(&[]);
    assert_eq!(missing, vec![instance_id.clone()]);
    let machine = registry.get(&instance_id).expect("instance");
    assert_eq!(machine.state(), InstanceLifecycle::Stopped);
    assert_eq!(
        machine.unknown(),
        1,
        "work the vanished process held is Unknown, never re-dispatched"
    );
    assert_eq!(machine.in_flight(), 0);

    // Only now can the uninstall proceed; the unknown outcome survives it.
    let catalogue = LocalCatalogue::new();
    let plan = preview(&store, &catalogue, &installed[0], &registry).expect("preview");
    assert_eq!(plan.in_flight, 0);
    let outcome =
        UninstallTransaction::begin(&mut registry, plan, DependentsDecision::SelectedOnly)
            .expect("begin")
            .drain(&mut registry, RemainingWork::Wait)
            .expect("drain")
            .collect(&store, &registry, owners())
            .expect("collect");
    assert_eq!(outcome.reclaimed_bytes, bytes + record_bytes);
    assert_eq!(
        registry.get(&instance_id).expect("instance").unknown(),
        1,
        "the unknown fact outlives the package's bytes"
    );
    cleanup(&root);
}
