//! The optional analytics package on the store's lifecycle: its own surface,
//! and everything an uninstall must leave alone.
//!
//! This is the component-integration evidence for the node's outcome —
//! "analytics uninstall removes only its own package resources and leaves base
//! usage, ongoing execution and the base facts intact" — and it is written so the
//! boundary is visible:
//!
//! - **The package installed is the package the release tool packages.** The
//!   archive is built from `components/analytics/package` — the committed
//!   manifest, release declaration, contribution resource and native entry — and
//!   installed through the production [`PackageStore`]. The one field the archive
//!   adapts is the client line, which must cover the client running the test.
//! - **The base facts are the kernel's and live outside the store.** A usage
//!   journal and a run record are written to a separate data home before the
//!   install; the uninstall is asserted to leave them byte for byte.
//! - **Ongoing execution is another package's.** The base execution instance
//!   keeps running, keeps its admission and keeps its in-flight work while
//!   analytics is drained and its bytes reclaimed.
//! - **The report claims only owned resources.** The released set is the
//!   package's own declaration exactly: its panel and its runtime entry. A
//!   resource of another package — or of the kernel — is refused, not reported.
//!
//! Everything is synthetic; no account, ledger, network or real user
//! installation is touched.

use super::*;
use crate::platform::extension_packages::install::InstallRequest;
use crate::platform::extension_packages::surface::{
    PackageSurface, RESOURCE_NOT_OWNED, SurfaceResource, uninstall_package,
};
use crate::platform::extension_packages::{InstanceRegistry, RemainingWork, Settlement};
use crate::platform::package_registration_release::PackageRegistrationOwners;
use licoup_extension_contracts::deployment::{LocalCatalogue, PackageEntry, PackageSource};
use licoup_extension_contracts::manifest::PackageManifest;

/// These scenarios record no external registration, so every owner is asked for
/// nothing and the reclaim proceeds. One shared value keeps a `'static`
/// reference available without each test naming or rebuilding the adapter.
static OWNERS: std::sync::LazyLock<PackageRegistrationOwners> =
    std::sync::LazyLock::new(PackageRegistrationOwners::default);

fn owners() -> &'static PackageRegistrationOwners {
    &OWNERS
}

const ANALYTICS: &str = "org.licoland.feature.analytics";
const ANALYTICS_VERSION: &str = "0.1.0";
const ANALYTICS_ENTRY: &str = "bin/licoup-analytics";
const ANALYTICS_PANEL: &str = "org.licoland.feature.analytics/usage-panel";
/// A capability owner that is not this package: its contribution is not ours.
const FOREIGN_RESOURCE: &str = "org.licoland.feature.mcp/service-status";
/// The running execution's package. It is optional too, and it is not analytics.
const ADAPTER: &str = "org.licoland.adapter.generic";
const ADAPTER_VERSION: &str = "1.0.0";

/// The committed payload source, exactly as the release tooling reads it.
fn shipped_package_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../components/analytics/package")
}

fn shipped_manifest() -> serde_json::Value {
    let text = std::fs::read_to_string(shipped_package_root().join("manifest.json"))
        .expect("the committed package manifest is readable");
    serde_json::from_str(&text).expect("the committed manifest is JSON")
}

/// The archive the release tooling produces from the committed source directory.
///
/// One field is adapted and named here: the compatibility list must cover the
/// client this test binary reports, exactly as the release declaration's range
/// covers a released client line. Every other byte is the committed package's.
fn shipped_payload() -> Vec<u8> {
    let root = shipped_package_root();
    let mut manifest = shipped_manifest();
    manifest["compatibility"]["clientVersions"] = serde_json::json!(covering_client_versions());
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let plain = zip::write::SimpleFileOptions::default();
    let executable = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
    for (name, options) in [
        ("bin/licoup-analytics", executable),
        ("contributions/usage-panel.json", plain),
        ("package-release.json", plain),
    ] {
        writer.start_file(name, options).expect("entry");
        writer
            .write_all(
                &std::fs::read(root.join(name))
                    .unwrap_or_else(|error| panic!("{name} is readable: {error}")),
            )
            .expect("content");
    }
    writer.start_file("manifest.json", plain).expect("entry");
    writer
        .write_all(manifest.to_string().as_bytes())
        .expect("manifest");
    writer.finish().expect("finish").into_inner()
}

fn install_analytics(store: &PackageStore) -> InstallOutcome {
    let bytes = shipped_payload();
    // The decision covers exactly the permissions the committed manifest
    // declares: a package that asked for more than this would be refused, which
    // is the rule the payload itself has to satisfy.
    let declared = PackageManifest::from_value(shipped_manifest()).expect("the committed manifest");
    let trust = TrustRecord::local_approved(content_digest(&bytes), declared.permissions.clone())
        .expect("trust");
    store
        .install(
            &InstallRequest::new(
                ANALYTICS,
                ANALYTICS_VERSION,
                PackageSource::LocalImport,
                trust,
            ),
            &bytes,
        )
        .expect("the store installs the committed package")
}

/// The kernel's own data home: usage facts and run records no package owns.
fn base_data_home(tag: &str) -> PathBuf {
    let home = sandbox(&format!("{tag}-home"));
    std::fs::create_dir_all(home.join("usage-journal")).expect("journal directory");
    std::fs::create_dir_all(home.join("runs")).expect("run directory");
    std::fs::write(
        home.join("usage-journal/settled.jsonl"),
        concat!(
            "{\"factId\":\"fact-1\",\"scopeRef\":\"run-1\",\"metric\":\"licoup.tokens.input\",\"value\":\"120\"}\n",
            "{\"factId\":\"fact-2\",\"scopeRef\":\"run-1\",\"metric\":\"licoup.tokens.output\",\"value\":\"80\"}\n",
        ),
    )
    .expect("settled facts");
    std::fs::write(
        home.join("runs/run-1.json"),
        "{\"runRef\":\"run-1\",\"state\":\"running\"}\n",
    )
    .expect("run record");
    home
}

fn read_home(home: &Path) -> (String, String) {
    (
        std::fs::read_to_string(home.join("usage-journal/settled.jsonl")).expect("usage journal"),
        std::fs::read_to_string(home.join("runs/run-1.json")).expect("run record"),
    )
}

fn working_instance(
    registry: &mut InstanceRegistry,
    package_id: &str,
    version: &str,
    generation: u64,
    work: u32,
) -> String {
    let instance_id = active_instance(registry, package_id, version, generation);
    let machine = registry
        .get_mut(&instance_id)
        .expect("the instance just inserted");
    for _ in 0..work {
        machine.begin_in_flight().expect("admitted work");
    }
    instance_id
}

#[test]
fn the_store_installs_the_payload_the_release_tooling_packages() {
    let (root, store) = store("analytics-shipped");
    let outcome = install_analytics(&store);
    assert_eq!(
        outcome.processes_spawned, 0,
        "installing runs nothing, including the package's own entry"
    );
    assert!(outcome.install_scripts.is_empty());

    // What the host read back is the committed declaration, field for field: the
    // only thing the test adapted is the client line it had to cover.
    let installed = store
        .installed_manifest(ANALYTICS, ANALYTICS_VERSION)
        .expect("the installed manifest");
    let committed = PackageManifest::from_value(shipped_manifest())
        .expect("the committed manifest is one the host reads");
    assert_eq!(installed.id, committed.id);
    assert_eq!(installed.id, ANALYTICS);
    assert_eq!(installed.version, committed.version);
    assert_eq!(installed.display_name, committed.display_name);
    assert_eq!(installed.host_protocol, committed.host_protocol);
    assert_eq!(installed.runtime, committed.runtime);
    assert_eq!(installed.profiles, committed.profiles);
    assert_eq!(installed.permissions, committed.permissions);
    assert_eq!(installed.contributions, committed.contributions);
    assert_eq!(
        installed.compatibility.client_versions,
        covering_client_versions(),
        "the one adapted field is the client line this binary reports"
    );

    // The installed content is the payload, and the client admits it.
    let installed_path = store.installed_path(ANALYTICS, ANALYTICS_VERSION);
    assert!(installed_path.join("manifest.json").is_file());
    assert!(installed_path.join(ANALYTICS_ENTRY).is_file());
    assert!(
        installed_path
            .join("contributions/usage-panel.json")
            .is_file()
    );
    store
        .admit_activation(ANALYTICS, ANALYTICS_VERSION)
        .expect("the declared client line admits this package");
    cleanup(&root);
}

#[test]
fn uninstalling_analytics_releases_its_own_surface_and_leaves_the_base_alone() {
    let home = base_data_home("analytics-preserved");
    let (root, store) = store("analytics-preserved");
    install_analytics(&store);
    let base_before = read_home(&home);

    let mut registry = InstanceRegistry::new();
    let analytics_instance = working_instance(&mut registry, ANALYTICS, ANALYTICS_VERSION, 1, 1);
    let execution = working_instance(&mut registry, ADAPTER, ADAPTER_VERSION, 2, 1);
    let catalogue = LocalCatalogue::new();

    // The registered surface is the package's own declaration: its panel and the
    // runtime entry it is carried as. Nothing else is this package's to release.
    let installed = store
        .installed_version(ANALYTICS, ANALYTICS_VERSION)
        .expect("readable record")
        .expect("installed");
    let mut surface = PackageSurface::register(&store, &installed).expect("registered");
    let declared = surface.declared().to_vec();
    assert_eq!(
        declared
            .iter()
            .map(SurfaceResource::identity)
            .collect::<Vec<_>>(),
        [ANALYTICS_PANEL, ANALYTICS_ENTRY]
    );
    for foreign in [FOREIGN_RESOURCE, "licoup.tokens.input", "bin/other"] {
        let failure = surface
            .release(foreign)
            .expect_err("a resource this package does not own");
        assert_eq!(failure.code, RESOURCE_NOT_OWNED);
    }
    assert_eq!(
        surface.held(),
        surface.declared(),
        "a refused claim releases nothing"
    );

    // The package's own work is still in flight, so draining waits rather than
    // cutting it off; and a refusal changes nothing at all.
    let failure = uninstall_package(
        &store,
        &mut registry,
        &catalogue,
        &mut surface,
        RemainingWork::Wait,
        owners(),
    )
    .expect_err("the package's own work is unsettled");
    assert_eq!(failure.code, "package_uninstall_in_flight");
    assert_eq!(
        read_home(&home),
        base_before,
        "the base facts are untouched"
    );
    assert!(
        store.installed_path(ANALYTICS, ANALYTICS_VERSION).exists(),
        "the package is still installed"
    );
    assert_eq!(
        surface.held(),
        surface.declared(),
        "a refused transaction releases nothing"
    );
    let base_execution = registry.get(&execution).expect("the base execution");
    assert_eq!(
        base_execution.admission(),
        crate::platform::extension_packages::Admission::Open,
        "the base execution keeps taking work"
    );
    assert_eq!(base_execution.in_flight(), 1);

    // The package's work settles, and the same call completes.
    registry
        .get_mut(&analytics_instance)
        .expect("analytics instance")
        .settle(Settlement::Completed)
        .expect("settled");
    let done = uninstall_package(
        &store,
        &mut registry,
        &catalogue,
        &mut surface,
        RemainingWork::Wait,
        owners(),
    )
    .expect("the drained package is reclaimed");

    // What was released is exactly the declared surface, and nothing else.
    assert_eq!(done.released, declared);
    assert!(done.released_exactly(&surface));
    assert!(surface.held().is_empty());
    assert_eq!(done.outcome.package_id, ANALYTICS);
    assert_eq!(done.outcome.version, ANALYTICS_VERSION);
    assert!(done.outcome.reclaimed_bytes > 0);
    assert_eq!(
        done.outcome.removed_together,
        Vec::<String>::new(),
        "an uninstall removes the package it was asked for, not others"
    );
    assert_eq!(done.outcome.shared_runtime_retained, None);
    assert!(!done.outcome.user_runtime_kept);
    assert_eq!(
        done.preserved(),
        crate::platform::extension_packages::PreservedFacts::all_kept()
    );

    // The package's own bytes are gone.
    assert!(!store.installed_path(ANALYTICS, ANALYTICS_VERSION).exists());
    assert!(!store.record_path(ANALYTICS, ANALYTICS_VERSION).exists());
    assert_eq!(
        registry
            .get(&analytics_instance)
            .expect("analytics instance")
            .state(),
        InstanceLifecycle::Stopped
    );

    // Base usage, the base run record and the base execution all survive: the
    // ongoing execution never stopped, never lost its admission and still holds
    // its own in-flight work.
    assert_eq!(read_home(&home), base_before);
    let base_execution = registry.get(&execution).expect("the base execution");
    assert_eq!(base_execution.state(), InstanceLifecycle::Active);
    assert_eq!(
        base_execution.admission(),
        crate::platform::extension_packages::Admission::Open
    );
    assert_eq!(base_execution.in_flight(), 1);
    assert_eq!(base_execution.settled(), 0);

    // The journal keeps its history: the install is still recorded and the
    // uninstall is appended to it rather than replacing it.
    let entries = store.journal().entries().expect("journal entries");
    let analytics_entries: Vec<_> = entries
        .iter()
        .filter(|entry| entry.package_id == ANALYTICS)
        .collect();
    assert!(
        analytics_entries.iter().any(|entry| entry.operation
            == crate::platform::extension_packages::JournalOperation::Commit),
        "the install that happened is still recorded"
    );
    assert!(
        analytics_entries.iter().any(|entry| entry.operation
            == crate::platform::extension_packages::JournalOperation::Uninstall),
        "the uninstall is appended, so the history is complete"
    );

    // And the base still works: reinstalling analytics finds the same facts and
    // nothing to clean up.
    install_analytics(&store);
    assert_eq!(read_home(&home), base_before);

    cleanup(&root);
    cleanup(&home);
}

#[test]
fn a_cancelled_uninstall_settles_its_own_work_and_never_the_base_execution() {
    let (root, store) = store("analytics-cancel");
    install_analytics(&store);
    let mut registry = InstanceRegistry::new();
    let analytics_instance = working_instance(&mut registry, ANALYTICS, ANALYTICS_VERSION, 1, 2);
    let execution = working_instance(&mut registry, ADAPTER, ADAPTER_VERSION, 2, 1);
    let catalogue = LocalCatalogue::new();
    let installed = store
        .installed_version(ANALYTICS, ANALYTICS_VERSION)
        .expect("readable record")
        .expect("installed");
    let mut surface = PackageSurface::register(&store, &installed).expect("registered");

    let done = uninstall_package(
        &store,
        &mut registry,
        &catalogue,
        &mut surface,
        RemainingWork::Cancel,
        owners(),
    )
    .expect("cancelling is a recorded outcome, not a failure");

    assert_eq!(
        done.outcome.canceled_work, 2,
        "only the package's own in-flight work is cancelled"
    );
    assert_eq!(done.outcome.unknown_work, 2);
    let cancelled = registry
        .get(&analytics_instance)
        .expect("analytics instance");
    assert_eq!(cancelled.state(), InstanceLifecycle::Stopped);
    assert_eq!(cancelled.in_flight(), 0);
    assert_eq!(
        cancelled.unknown(),
        2,
        "the outcome is recorded as unknown, never as success"
    );

    let base_execution = registry.get(&execution).expect("the base execution");
    assert_eq!(base_execution.state(), InstanceLifecycle::Active);
    assert_eq!(
        base_execution.in_flight(),
        1,
        "the base work is not cancelled"
    );
    assert_eq!(base_execution.unknown(), 0);
    assert_eq!(
        base_execution.admission(),
        crate::platform::extension_packages::Admission::Open
    );
    cleanup(&root);
}

#[test]
fn a_package_another_installed_package_requires_is_refused_with_its_dependents() {
    let (root, store) = store("analytics-dependent");
    install_analytics(&store);
    let mut registry = InstanceRegistry::new();
    let installed = store
        .installed_version(ANALYTICS, ANALYTICS_VERSION)
        .expect("readable record")
        .expect("installed");
    let mut surface = PackageSurface::register(&store, &installed).expect("registered");

    let mut catalogue = LocalCatalogue::new();
    catalogue.insert(
        PackageEntry::new(
            "example.optional.dashboard",
            "1.0.0",
            PackageSource::LocalImport,
        )
        .requiring([licoup_extension_contracts::manifest::Dependency::new(
            ANALYTICS, "^0.1",
        )]),
    );
    let failure = uninstall_package(
        &store,
        &mut registry,
        &catalogue,
        &mut surface,
        RemainingWork::Wait,
        owners(),
    )
    .expect_err("a dependent package is the user's decision");
    assert_eq!(failure.code, "package_uninstall_has_dependents");
    assert_eq!(
        surface.held(),
        surface.declared(),
        "a refused removal releases nothing"
    );
    assert!(store.installed_path(ANALYTICS, ANALYTICS_VERSION).exists());
    cleanup(&root);
}
