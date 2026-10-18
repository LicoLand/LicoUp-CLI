//! Black-box scenario for `tests/integration/package_lifecycle/`.
//!
//! The packages this test installs are produced *outside* the Rust crate by an
//! independent fixture generator, so it proves the store accepts bytes it did not
//! author itself, on a real filesystem, and leaves a deterministic sandbox for the
//! generator to inspect after the process has exited.
//!
//! It is ignored by the ordinary suite because it needs the two environment
//! variables the driver sets:
//!
//! ```text
//! LICOUP_V71_PACKAGE_FIXTURES, LICOUP_V71_PACKAGE_SANDBOX
//! ```
//!
//! Run it through the driver:
//!
//! ```text
//! python3 tests/integration/package_lifecycle/verify_component_lifecycle.py
//! ```

use super::*;
use crate::platform::extension_packages::install::{FaultPlan, InstallPhase, InstallRequest};
use crate::platform::extension_packages::{
    DependentsDecision, InstanceRegistry, RemainingWork, UninstallTransaction, preview,
};
use crate::platform::package_registration_release::PackageRegistrationOwners;
use licoup_extension_contracts::deployment::{LocalCatalogue, PackageSource};

/// The production registration adapter with no caller-supplied release inputs.
///
/// This scenario records no external registration, so every owner is asked for
/// nothing and the reclaim proceeds. One shared value keeps a `'static` reference
/// available without the test naming or rebuilding the adapter.
static OWNERS: std::sync::LazyLock<PackageRegistrationOwners> =
    std::sync::LazyLock::new(PackageRegistrationOwners::default);

fn owners() -> &'static PackageRegistrationOwners {
    &OWNERS
}

fn fixture(directory: &std::path::Path, name: &str) -> Vec<u8> {
    std::fs::read(directory.join(name)).unwrap_or_else(|_| panic!("fixture {name}"))
}

#[test]
#[ignore = "driven by tests/integration/package_lifecycle/verify_component_lifecycle.py"]
fn externally_produced_packages_install_and_uninstall_on_the_real_filesystem() {
    let (Ok(fixtures), Ok(sandbox)) = (
        std::env::var("LICOUP_V71_PACKAGE_FIXTURES"),
        std::env::var("LICOUP_V71_PACKAGE_SANDBOX"),
    ) else {
        eprintln!("skipped: the lifecycle driver has not set the fixture environment");
        return;
    };
    let fixtures = std::path::Path::new(&fixtures);
    let sandbox = std::path::Path::new(&sandbox);
    let store = PackageStore::open(sandbox).expect("sandbox store");

    // Offline import of a package the store did not build.
    let echo_1 = fixture(fixtures, "echo-1.0.0.zip");
    let outcome = store
        .install_local_import(
            ECHO,
            "1.0.0",
            trust_for(&echo_1, [net_permission()]),
            &echo_1,
        )
        .expect("import echo 1.0.0");
    assert_eq!(outcome.processes_spawned, 0);

    // An interrupted update never replaces the installed version, and its stage
    // is reclaimed by recovery.
    let echo_2 = fixture(fixtures, "echo-1.1.0.zip");
    let request = InstallRequest::new(
        ECHO,
        "1.1.0",
        PackageSource::LocalImport,
        trust_for(&echo_2, [net_permission()]),
    )
    .with_faults(FaultPlan::failing_at(InstallPhase::Stage));
    assert_eq!(
        store
            .install(&request, &echo_2)
            .expect_err("interrupted")
            .code,
        "package_install_interrupted"
    );
    assert!(
        store
            .installed_path(ECHO, "1.0.0")
            .join("agent.py")
            .exists()
    );
    let report = store.recover().expect("recover");
    assert!(report.reclaimed_bytes > 0);
    assert!(store.staged_directories().expect("staged").is_empty());

    // A package that ships an install script: reported, never run.
    let scripted = fixture(fixtures, "scripted-1.0.0.zip");
    let outcome = store
        .install_local_import(
            SCRIPTED,
            "1.0.0",
            trust_for(&scripted, [net_permission()]),
            &scripted,
        )
        .expect("import scripted");
    assert_eq!(outcome.install_scripts, vec!["postinstall.sh".to_owned()]);
    assert!(!sandbox.join("ran-script.txt").exists());

    // An archive entry that leaves the package root is refused.
    let traversal = fixture(fixtures, "traversal-1.0.0.zip");
    let failure = store
        .install_local_import(
            "example.specialist.traversal",
            "1.0.0",
            trust_for(&traversal, [net_permission()]),
            &traversal,
        )
        .expect_err("traversal refused");
    assert!(failure.code.starts_with("package_artifact"));

    // A real uninstall: admission withdrawn, drained, bytes reclaimed.
    let installed = store.installed().expect("installed");
    let echo = installed
        .iter()
        .find(|package| package.package_id == ECHO)
        .expect("echo installed");
    let bytes = store.installed_bytes(ECHO, "1.0.0").expect("bytes");
    let record_bytes = std::fs::metadata(store.record_path(ECHO, "1.0.0"))
        .expect("record")
        .len();
    let mut registry = InstanceRegistry::new();
    let catalogue = LocalCatalogue::new();
    let plan = preview(&store, &catalogue, echo, &registry).expect("preview");
    let outcome =
        UninstallTransaction::begin(&mut registry, plan, DependentsDecision::SelectedOnly)
            .expect("begin")
            .drain(&mut registry, RemainingWork::Wait)
            .expect("drain")
            .collect(&store, &registry, owners())
            .expect("collect");
    assert_eq!(outcome.reclaimed_bytes, bytes + record_bytes);
    assert!(!store.installed_path(ECHO, "1.0.0").exists());
    assert_eq!(
        store
            .installed()
            .expect("installed")
            .iter()
            .filter(|package| package.package_id == ECHO)
            .count(),
        0
    );
    // The sandbox is left for the driver: scripted@1.0.0 installed, echo gone,
    // staging empty, nothing escaped.
    assert!(
        store
            .installed_path(SCRIPTED, "1.0.0")
            .join("agent.py")
            .exists()
    );
    assert!(store.staged_directories().expect("staged").is_empty());
    assert!(!sandbox.join("escape.txt").exists());
}
