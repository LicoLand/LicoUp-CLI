//! A31 and A37 at component level: recommendation is not installation, an
//! offline import uninstalls for real, and GC frees only unreferenced, store-owned
//! bytes after the generation that used them has drained.

use super::*;
use crate::platform::extension_packages::{
    CatalogEntry, CatalogIndex, DependentsDecision, Detector, DiscoveryEnvironment, DiscoveryRule,
    InFlightPins, InstanceRegistry, OffFrameLane, PreservedFacts, RecommendationLog, RemainingWork,
    RetainReason, StorageEntry, StorageKind, UninstallTransaction, account_store, plan_gc, preview,
    reclaim,
};
use crate::platform::package_registration_release::PackageRegistrationOwners;
use licoup_extension_contracts::deployment::{LocalCatalogue, PackageSource};
use std::collections::BTreeMap;

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
fn a_directory_recommendation_never_installs_or_runs_anything() {
    let (root, store) = store("recommend");
    let index = CatalogIndex::from_entries([
        CatalogEntry::new(
            ECHO,
            "1.0.0",
            "Echo specialist",
            PackageSource::ThirdPartyDirectory,
        )
        .with_capabilities([NET])
        .with_rules([DiscoveryRule::new(
            ECHO,
            Detector::PathEntry {
                name: "echo-agent".to_owned(),
            },
        )]),
        CatalogEntry::new(
            "example.specialist.other",
            "2.0.0",
            "Other specialist",
            PackageSource::OfficialDirectory,
        )
        .with_rules([DiscoveryRule::new(
            "example.specialist.other",
            Detector::PathEntry {
                name: "other-agent".to_owned(),
            },
        )]),
    ]);
    let environment = DiscoveryEnvironment::new([
        "echo-agent".to_owned(),
        "other-agent".to_owned(),
        "unrelated-tool".to_owned(),
    ]);

    let lane = OffFrameLane::new_on_frame_thread();
    let scanned = lane.scan_off_frame(index, environment).expect("scan");
    assert_eq!(
        scanned.package_ids(),
        vec![ECHO, "example.specialist.other"]
    );
    assert_eq!(scanned.processes_spawned, 0, "matching runs nothing");
    assert!(!scanned.code_loaded, "matching loads nothing");

    let mut log = RecommendationLog::new();
    let pending = scanned.recommendations[0].accept(&mut log);
    assert!(pending.requires_user_confirmation);
    assert_eq!(pending.source, PackageSource::ThirdPartyDirectory);
    scanned.recommendations[1].decline(&mut log, "not wanted here");
    assert!(log.was_declined("example.specialist.other"));

    assert!(
        store.installed().expect("installed").is_empty(),
        "a recommendation is not an install"
    );
    assert!(store.staged_directories().expect("staged").is_empty());
    assert!(
        !store.installed_path(ECHO, "1.0.0").exists(),
        "nothing was downloaded either"
    );
    cleanup(&root);
}

#[test]
fn an_offline_import_uninstalls_for_real_and_keeps_the_users_history() {
    let (root, store) = store("uninstall");
    let user_data_root = sandbox("uninstall-userdata");
    let history = user_data_root.join(ECHO).join("history");
    std::fs::create_dir_all(&history).expect("user data");
    std::fs::write(history.join("turns.jsonl"), b"{\"turn\":1}\n").expect("history");

    // The catalogue is a separate, possibly unavailable thing. An empty local
    // catalogue (source disabled, no network) still previews and uninstalls an
    // imported package.
    let catalogue = LocalCatalogue::new();

    install_local(&store, ECHO, "1.0.0", None);
    let installed = store.installed().expect("installed");
    assert_eq!(installed.len(), 1);
    let bytes_before = store.installed_bytes(ECHO, "1.0.0").expect("bytes");
    let record_bytes = std::fs::metadata(store.record_path(ECHO, "1.0.0"))
        .expect("record")
        .len();
    assert!(bytes_before > 0);
    assert!(
        store
            .installed_path(ECHO, "1.0.0")
            .join("agent.py")
            .exists()
    );

    let mut registry = InstanceRegistry::new();
    let plan = preview(&store, &catalogue, &installed[0], &registry).expect("preview");
    assert_eq!(plan.exclusive_bytes, bytes_before);
    assert_eq!(plan.preserved, PreservedFacts::all_kept());

    let drained =
        UninstallTransaction::begin(&mut registry, plan, DependentsDecision::SelectedOnly)
            .expect("begin")
            .drain(&mut registry, RemainingWork::Wait)
            .expect("drain");
    let outcome = drained
        .collect(&store, &registry, owners())
        .expect("collect");
    assert_eq!(outcome.reclaimed_bytes, bytes_before + record_bytes);
    assert!(
        !store.installed_path(ECHO, "1.0.0").exists(),
        "the managed bytes are actually gone, not just hidden"
    );
    assert!(store.installed().expect("installed").is_empty());
    assert_eq!(store.installed_bytes(ECHO, "1.0.0").expect("bytes"), 0);
    assert!(
        history.join("turns.jsonl").exists(),
        "history is the user's, not the package's"
    );
    assert!(store.staged_directories().expect("staged").is_empty());

    // Reinstalling the same version still writes where the history lives.
    install_local(&store, ECHO, "1.0.0", None);
    assert_eq!(store.installed().expect("installed").len(), 1);
    assert!(history.join("turns.jsonl").exists());

    cleanup(&user_data_root);
    cleanup(&root);
}

#[test]
fn gc_executes_only_its_own_plan_and_only_on_its_own_bytes() {
    let (root, store) = store("gc");
    install_local(&store, ECHO, "1.0.0", Some("runtime.node-22"));
    install_local(&store, ECHO, "1.1.0", Some("runtime.node-22"));
    let bytes_1_0 = store.installed_bytes(ECHO, "1.0.0").expect("bytes");
    let bytes_1_1 = store.installed_bytes(ECHO, "1.1.0").expect("bytes");
    let record_1_0 = std::fs::metadata(store.record_path(ECHO, "1.0.0"))
        .expect("record")
        .len();
    assert!(bytes_1_0 > 0 && bytes_1_1 > 0);

    let cache = store.root().join("cache");
    std::fs::write(cache.join("blob"), vec![7u8; 64]).expect("cache");

    let mut pins = InFlightPins::new();
    pins.pin(format!("{ECHO}@1.1.0#1"), bytes_1_1);

    let mut entries = vec![
        StorageEntry::new("core", StorageKind::Core, 4_000_000),
        // A version on disk that nothing references: the host's plan may reclaim
        // it.
        StorageEntry::new(
            format!("{ECHO}@1.0.0"),
            StorageKind::OptionalCode,
            bytes_1_0,
        ),
        StorageEntry::new(
            format!("{ECHO}@1.1.0"),
            StorageKind::OptionalCode,
            bytes_1_1,
        )
        .pinned(true),
        StorageEntry::new(
            "runtime:runtime.node-22",
            StorageKind::SharedRuntime,
            90_000,
        )
        .with_references(2),
        StorageEntry::new("runtime:user:python3", StorageKind::SharedRuntime, 60_000)
            .managed(false)
            .user_installed(true),
        StorageEntry::new("cache", StorageKind::Cache, 64),
        StorageEntry::new("user-data", StorageKind::UserData, 128).managed(false),
    ];
    entries.extend(pins.entries());

    let plan = plan_gc(&entries);
    assert_eq!(
        plan.removed,
        vec!["cache".to_owned(), format!("{ECHO}@1.0.0")]
    );
    let reasons: BTreeMap<&str, RetainReason> = plan
        .retained
        .iter()
        .map(|(id, reason)| (id.as_str(), *reason))
        .collect();
    assert_eq!(reasons["core"], RetainReason::Core);
    assert_eq!(reasons["user-data"], RetainReason::UserData);
    assert_eq!(
        reasons[format!("{ECHO}@1.1.0").as_str()],
        RetainReason::PinnedInFlight
    );
    assert_eq!(
        reasons["runtime:runtime.node-22"],
        RetainReason::SharedRuntimeInUse
    );
    assert_eq!(reasons["runtime:user:python3"], RetainReason::UserInstalled);

    let outcome = reclaim(&store, &entries).expect("reclaim");
    assert_eq!(
        outcome.removed,
        vec!["cache".to_owned(), format!("{ECHO}@1.0.0")]
    );
    assert_eq!(outcome.reclaimed_bytes, 64 + bytes_1_0 + record_1_0);
    assert!(!store.installed_path(ECHO, "1.0.0").exists());
    assert!(
        store
            .installed_path(ECHO, "1.1.0")
            .join("agent.py")
            .exists(),
        "the pinned version keeps its bytes"
    );
    assert!(!cache.exists(), "the managed cache is reclaimed");
    assert!(
        outcome.not_reclaimed.is_empty(),
        "everything the plan removed is the store's to remove: {outcome:?}"
    );
    assert!(
        store
            .journal()
            .entries()
            .expect("journal")
            .iter()
            .any(|entry| {
                entry.operation == crate::platform::extension_packages::JournalOperation::Gc
                    && entry.package_id == ECHO
                    && entry.version == "1.0.0"
            }),
        "a GC removal is recorded as GC, not as a user uninstall"
    );

    // A plan that marks a shared runtime reclaimable is not the store's bytes to
    // remove: it is reported instead of guessed at.
    let foreign = [StorageEntry::new(
        "runtime:runtime.node-22",
        StorageKind::SharedRuntime,
        90_000,
    )];
    let outcome = reclaim(&store, &foreign).expect("reclaim");
    assert!(outcome.removed.is_empty());
    assert_eq!(
        outcome.not_reclaimed,
        vec!["runtime:runtime.node-22".to_owned()]
    );
    cleanup(&root);
}

#[test]
fn an_uninstall_never_touches_a_runtime_the_user_installed() {
    let (root, store) = store("user-runtime");
    let user_root = sandbox("user-runtime-home");
    let interpreter = user_root.join("bin").join("python3");
    std::fs::create_dir_all(interpreter.parent().expect("parent")).expect("runtime");
    std::fs::write(&interpreter, b"the user's own interpreter").expect("interpreter");

    install_local(&store, ECHO, "1.0.0", Some("user:python3"));
    let installed = store.installed().expect("installed");
    let catalogue = LocalCatalogue::new();
    let mut registry = InstanceRegistry::new();
    let plan = preview(&store, &catalogue, &installed[0], &registry).expect("preview");
    assert_eq!(
        plan.shared_runtime_ref, None,
        "a user runtime is the user's to release, not the host's"
    );

    let outcome =
        UninstallTransaction::begin(&mut registry, plan, DependentsDecision::SelectedOnly)
            .expect("begin")
            .drain(&mut registry, RemainingWork::Wait)
            .expect("drain")
            .collect(&store, &registry, owners())
            .expect("collect");
    assert!(outcome.user_runtime_kept);
    assert!(
        interpreter.exists(),
        "the package manager only removes its own managed bytes"
    );
    cleanup(&user_root);
    cleanup(&root);
}

#[test]
fn an_in_flight_generation_is_drained_before_its_bytes_are_reclaimed() {
    let (root, store) = store("drain-gc");
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

    let mut pins = InFlightPins::new();
    pins.pin(format!("{ECHO}@1.0.0#1"), bytes);
    let account =
        account_store(&store, &installed, 4_000, 0, &BTreeMap::new(), &pins).expect("account");
    assert_eq!(account.retained_bytes(), bytes, "the pin holds the bytes");

    // While the work is in flight, the plan retains the version and the
    // uninstall refuses to drain.
    let entries = [
        StorageEntry::new(format!("{ECHO}@1.0.0"), StorageKind::OptionalCode, bytes).pinned(true),
    ];
    let plan = plan_gc(&entries);
    assert!(plan.removed.is_empty());
    assert!(plan.retained.iter().any(|(id, reason)| {
        id == &format!("{ECHO}@1.0.0") && *reason == RetainReason::PinnedInFlight
    }));

    let catalogue = LocalCatalogue::new();
    let preview_plan = preview(&store, &catalogue, &installed[0], &registry).expect("preview");
    assert_eq!(preview_plan.in_flight, 1);
    let waiting = UninstallTransaction::begin(
        &mut registry,
        preview_plan.clone(),
        DependentsDecision::SelectedOnly,
    )
    .expect("begin");
    let failure = waiting
        .drain(&mut registry, RemainingWork::Wait)
        .expect_err("wait refuses while work is unsettled");
    assert_eq!(failure.code, "package_uninstall_in_flight");
    assert!(store.installed_path(ECHO, "1.0.0").exists());

    // The user cancels: the outcome is Unknown, the instance stops, and only
    // then are the bytes actually reclaimed.
    let canceling = UninstallTransaction::begin(
        &mut registry,
        preview_plan,
        DependentsDecision::SelectedOnly,
    )
    .expect("begin again");
    let outcome = canceling
        .drain(&mut registry, RemainingWork::Cancel)
        .expect("cancel")
        .collect(&store, &registry, owners())
        .expect("collect");
    assert_eq!(outcome.canceled_work, 1);
    assert_eq!(outcome.unknown_work, 1, "cancelled work is Unknown");
    assert_eq!(outcome.reclaimed_bytes, bytes + record_bytes);
    assert!(!store.installed_path(ECHO, "1.0.0").exists());
    assert_eq!(
        registry.get(&instance_id).expect("instance").unknown(),
        1,
        "the unknown outcome stays visible after the bytes are gone"
    );
    cleanup(&root);
}
