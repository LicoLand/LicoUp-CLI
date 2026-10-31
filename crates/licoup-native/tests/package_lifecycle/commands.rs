//! The package lifecycle driven through the native CLI routes over a synthetic
//! data home.
//!
//! What is real here: a real data home on disk, real package archives, the real
//! `PackageStore` at the layout owner's root, the real install and uninstall
//! transactions, the real recovery, and the real command registry reached through
//! `execute_cli`. No process is launched and nothing is fetched.
//!
//! What is synthetic, stated plainly: the data home and the archives are
//! generated fixtures. The bytes stand in for what a host fetcher or an operator
//! would have handed in, which is why the trust that goes with them is built from
//! their own digest.

use super::{
    archive, cleanup, content_digest_of, covering_client_versions, data_package_bytes,
    excluding_client_versions, manifest_json_declaring, root,
};
use licoup_extension_contracts::deployment::{LocalCatalogue, PackageEntry, PackageSource};
use licoup_extension_contracts::manifest::{PermissionRequest, ResourceKind};
use licoup_native::ffi::commands::{CliExecution, execute_cli};
use licoup_native::platform::extension_packages::registration::{
    RecordedRegistration, RegistrationOwner, RegistrationOwners, ReleasedRegistration,
};
use licoup_native::platform::extension_packages::{
    DependentsDecision, Drained, FaultPlan, InstallPhase, InstallRequest, InstanceIdentity,
    InstanceMachine, InstanceRegistry, MOUNT_PLAN_FORMAT, PackageStore, RemainingWork, TrustRecord,
    UninstallTransaction, content_digest, declared_default, mount_plan_path, preview,
    read_published_mount_plan, system_default_name,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// One synthetic data home that the CLI resolves the package store inside.
struct DataHome {
    path: PathBuf,
}

impl DataHome {
    fn new(tag: &str) -> Self {
        let path = root(&format!("commands-{tag}"));
        std::fs::create_dir_all(&path).expect("data home");
        Self { path }
    }

    fn text(&self) -> String {
        self.path.display().to_string()
    }

    /// The store root the layout owner names, which is what the CLI must use.
    fn store_root(&self) -> PathBuf {
        licoup_foundation::platform::paths::package_store_root(&self.path)
    }

    fn store(&self) -> PackageStore {
        PackageStore::open(&self.store_root()).expect("the CLI's own store root")
    }
}

impl Drop for DataHome {
    fn drop(&mut self) {
        cleanup(&self.path);
    }
}

/// Write one archive inside the data home and return its path.
fn archive_file(home: &DataHome, name: &str, bytes: &[u8]) -> PathBuf {
    let path = home.path.join(name);
    std::fs::write(&path, bytes).expect("archive on disk");
    path
}

/// One fixture archive declaring the client versions the operator chose.
fn fixture_bytes(id: &str, version: &str, client_versions: &[String]) -> Vec<u8> {
    archive(&[
        (
            "manifest.json",
            manifest_json_declaring(
                id,
                version,
                "agent.py",
                &[("example.fixture/net", "self")],
                None,
                &[],
                client_versions,
            )
            .into_bytes(),
        ),
        ("agent.py", b"print('fixture')\n".to_vec()),
    ])
}

fn permission() -> PermissionRequest {
    PermissionRequest::new("example.fixture/net", "self")
}

fn local_trust(bytes: &[u8]) -> TrustRecord {
    TrustRecord::local_approved(content_digest(bytes), [permission()]).expect("trust")
}

const PACKAGE_ID: &str = "example.fixture.echo";
const VERSION: &str = "1.0.0";

/// Run one package route through the real command registry.
///
/// The positionals each route declares are supplied here, in the order the route
/// declares them, so the fixture drives the CLI exactly as a caller would.
fn run(home: &DataHome, path: &str, options: &[(&str, String)]) -> Value {
    let mut args = path
        .split_ascii_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let operation = args.get(1).map(String::as_str).unwrap_or_default();
    match operation {
        "catalog" | "install-plan" | "install-confirm" | "install-apply" | "import" | "recover" => {
            args.push(home.text())
        }
        "enable" | "disable" | "uninstall-preview" | "uninstall-drain" | "uninstall-collect"
        | "activate" | "resource-select" => {
            args.push(home.text());
            args.push(PACKAGE_ID.to_owned());
            args.push(VERSION.to_owned());
        }
        "update-preview" | "update-apply" => {
            args.push(home.text());
            args.push(PACKAGE_ID.to_owned());
        }
        other => panic!("the fixture has no positionals for package {other}"),
    }
    for (name, value) in options {
        args.push(format!("--{name}"));
        args.push(value.clone());
    }
    let execution = execute_cli(args).expect("the route is admitted and shaped correctly");
    let CliExecution::Json(report) = execution else {
        panic!("a package route publishes a JSON report");
    };
    report
}

fn is_ok(report: &Value, operation: &str) {
    assert_eq!(
        report["isError"], false,
        "{operation} must succeed: {report}"
    );
    assert_eq!(report["operation"], operation);
}

fn catalog(home: &DataHome) -> Value {
    let report = run(home, "package catalog", &[]);
    is_ok(&report, "catalog");
    report
}

fn installed_count(home: &DataHome) -> usize {
    catalog(home)["packages"]
        .as_array()
        .expect("packages")
        .len()
}

// ---------------------------------------------------------------------------
// The whole lifecycle, one route at a time
// ---------------------------------------------------------------------------

#[test]
fn the_whole_lifecycle_runs_through_the_cli_routes_over_a_synthetic_data_home() {
    let home = DataHome::new("lifecycle");
    let bytes = fixture_bytes(PACKAGE_ID, VERSION, &covering_client_versions());
    let archive = archive_file(&home, "echo-1.0.0.zip", &bytes);
    let archive_text = archive.display().to_string();

    // Nothing exists yet: the layout owner resolves the root and the store
    // creates its own private directories below the data home.
    assert!(!home.store_root().exists());
    let first = catalog(&home);
    assert_eq!(first["recoveredBeforeRead"], true);
    assert_eq!(first["packages"].as_array().expect("packages").len(), 0);
    assert_eq!(
        first["storeRoot"],
        home.store_root()
            .canonicalize()
            .expect("the store root exists after the first read")
            .display()
            .to_string(),
        "the store root is the layout owner's directory inside the data home"
    );

    // Plan: read-only, and it names the digest of the bytes it planned against.
    let planned = run(
        &home,
        "package install-plan",
        &[("archive", archive_text.clone())],
    );
    is_ok(&planned, "install-plan");
    let plan = &planned["plan"];
    assert_eq!(plan["packageId"], PACKAGE_ID);
    assert_eq!(plan["version"], VERSION);
    assert_eq!(plan["compatibility"]["covers"], true);
    assert_eq!(plan["alreadyInstalled"], false);
    assert_eq!(plan["processesSpawned"], 0);
    assert_eq!(plan["installScriptsExecuted"], 0);
    assert_eq!(plan["digest"], content_digest_of(&bytes));
    assert_eq!(
        plan["permissions"],
        json!([{ "capability": "example.fixture/net", "scope": "self" }])
    );
    assert_eq!(
        home.store_root().join("records").join(PACKAGE_ID).exists(),
        false,
        "a plan installs nothing"
    );
    let plan_digest = planned["planDigest"]
        .as_str()
        .expect("plan digest")
        .to_owned();
    assert!(plan_digest.starts_with("sha256:"));

    // Confirm: the explicit second step, bound to the reviewed plan digest. A
    // digest the archive does not reproduce is refused.
    let stale = run(
        &home,
        "package install-confirm",
        &[
            ("archive", archive_text.clone()),
            ("plan", "sha256:not-the-plan".to_owned()),
        ],
    );
    assert_eq!(stale["isError"], true);
    assert_eq!(stale["reasonCode"], "package_install_plan_stale");

    let confirmed = run(
        &home,
        "package install-confirm",
        &[("archive", archive_text.clone()), ("plan", plan_digest)],
    );
    is_ok(&confirmed, "install-confirm");
    let confirmation = confirmed["confirmation"]
        .as_str()
        .expect("confirmation")
        .to_owned();
    assert!(confirmation.starts_with("licoup.package-install-confirmation.v1:sha256:"));

    // Apply: the wrong confirmation is refused, and the right one installs.
    let wrong = run(
        &home,
        "package install-apply",
        &[
            ("archive", archive_text.clone()),
            (
                "confirmation",
                "licoup.package-install-confirmation.v1:sha256:0".to_owned(),
            ),
        ],
    );
    assert_eq!(wrong["reasonCode"], "package_install_confirmation_stale");

    let applied = run(
        &home,
        "package install-apply",
        &[
            ("archive", archive_text.clone()),
            ("confirmation", confirmation.clone()),
        ],
    );
    is_ok(&applied, "install-apply");
    assert_eq!(applied["packageId"], PACKAGE_ID);
    assert_eq!(applied["version"], VERSION);
    assert_eq!(applied["state"], "installed");
    assert_eq!(applied["trustChannel"], "local-approved");
    // Installing decides availability, not activity.
    assert_eq!(applied["enabled"], false);
    assert_eq!(applied["processesSpawned"], 0);
    assert_eq!(applied["installScripts"], json!([]));

    // A second apply of the same version is refused: an install never replaces.
    let again = run(
        &home,
        "package install-apply",
        &[
            ("archive", archive_text.clone()),
            ("confirmation", confirmation.clone()),
        ],
    );
    assert_eq!(again["reasonCode"], "package_version_already_installed");

    // Catalogue: the installed version, with the user's stored preference.
    let listed = catalog(&home);
    let packages = listed["packages"].as_array().expect("packages");
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0]["packageId"], PACKAGE_ID);
    assert_eq!(packages[0]["version"], VERSION);
    assert_eq!(packages[0]["trustChannel"], "local-approved");
    assert_eq!(packages[0]["source"], "local-import");
    assert_eq!(packages[0]["enabled"], true);
    assert_eq!(packages[0]["registrations"], json!([]));

    // Disable then enable: the stored preference, and no activation either way.
    let disabled = run(&home, "package disable", &[]);
    is_ok(&disabled, "disable");
    assert_eq!(disabled["enabled"], false);
    assert_eq!(disabled["activated"], false);
    assert_eq!(disabled["processesSpawned"], 0);
    assert_eq!(catalog(&home)["packages"][0]["enabled"], false);
    let enabled = run(&home, "package enable", &[]);
    is_ok(&enabled, "enable");
    assert_eq!(enabled["enabled"], true);
    assert_eq!(enabled["activated"], false);

    // The user's own data, where the platform keeps user data for a package.
    let user_data = home.path.join("user-data").join(PACKAGE_ID);
    std::fs::create_dir_all(user_data.join("history")).expect("user data");
    std::fs::write(
        user_data.join("history/transcript.jsonl"),
        b"{\"turn\":1}\n",
    )
    .expect("history");

    // Uninstall preview: what would be touched, and what is preserved.
    let previewed = run(&home, "package uninstall-preview", &[]);
    is_ok(&previewed, "uninstall-preview");
    assert_eq!(previewed["plan"]["packageId"], PACKAGE_ID);
    assert!(previewed["plan"]["exclusiveBytes"].as_u64().expect("bytes") > 0);
    assert_eq!(previewed["plan"]["registrations"], json!([]));
    assert_eq!(previewed["needsUserChoice"], false);
    assert_eq!(previewed["preservesUserData"], true);
    assert!(
        home.store_root()
            .join("packages")
            .join(PACKAGE_ID)
            .join(VERSION)
            .exists(),
        "a preview removes nothing"
    );

    // Collect without a recorded drain is refused rather than reclaiming.
    let undrained_home = DataHome::new("no-drain");
    let undrained = run(&undrained_home, "package uninstall-collect", &[]);
    assert_eq!(undrained["reasonCode"], "package_uninstall_not_drained");

    // Drain then collect, as two routes: the drained decision is written down and
    // read back, and no managed byte moves until collect.
    let drained = run(&home, "package uninstall-drain", &[]);
    is_ok(&drained, "uninstall-drain");
    assert_eq!(drained["packageId"], PACKAGE_ID);
    assert_eq!(drained["version"], VERSION);
    assert_eq!(drained["drainedInstances"], json!([]));
    assert_eq!(drained["releasesRegistrationsOnCollect"], 0);
    assert_eq!(drained["preservesUserData"], true);
    assert!(
        home.store_root()
            .join("uninstall")
            .join(format!("{PACKAGE_ID}@{VERSION}.json"))
            .exists(),
        "the drained decision is durable"
    );
    assert!(
        home.store_root()
            .join("packages")
            .join(PACKAGE_ID)
            .join(VERSION)
            .exists(),
        "draining removes no bytes"
    );

    let collected = run(&home, "package uninstall-collect", &[]);
    is_ok(&collected, "uninstall-collect");
    assert_eq!(collected["packageId"], PACKAGE_ID);
    assert!(collected["reclaimedBytes"].as_u64().expect("bytes") > 0);
    assert_eq!(collected["releasedRegistrations"], json!([]));
    assert_eq!(collected["preservedUserData"]["history"], true);
    assert_eq!(collected["preservedUserData"]["credentials"], true);
    assert_eq!(collected["preservedUserData"]["protocolState"], true);
    assert!(
        !home
            .store_root()
            .join("packages")
            .join(PACKAGE_ID)
            .join(VERSION)
            .exists(),
        "collect reclaims the version's own bytes"
    );
    assert!(
        user_data.join("history/transcript.jsonl").exists(),
        "an ordinary uninstall preserves user data"
    );
    assert!(
        !home
            .store_root()
            .join("uninstall")
            .join(format!("{PACKAGE_ID}@{VERSION}.json"))
            .exists(),
        "the drained handoff is cleared once its collect succeeded"
    );

    // Recover on an empty store is clean and changes nothing.
    let recovered = run(&home, "package recover", &[]);
    is_ok(&recovered, "recover");
    assert_eq!(recovered["recovery"]["clean"], true);
    assert_eq!(recovered["installed"], json!([]));
}

// ---------------------------------------------------------------------------
// A crash in the middle of an install
// ---------------------------------------------------------------------------

#[test]
fn an_interrupted_install_is_reconciled_before_the_catalogue_is_read() {
    let home = DataHome::new("crash");

    // One version is installed and serving.
    let first = fixture_bytes(PACKAGE_ID, VERSION, &covering_client_versions());
    home.store()
        .install_local_import(PACKAGE_ID, VERSION, local_trust(&first), &first)
        .expect("the first version installs");

    // A second install dies after its rename and before its record: the version
    // is on disk and invisible to every reader.
    let second = fixture_bytes(PACKAGE_ID, "2.0.0", &covering_client_versions());
    let request = InstallRequest::new(
        PACKAGE_ID,
        "2.0.0",
        PackageSource::LocalImport,
        local_trust(&second),
    )
    .with_faults(FaultPlan::failing_at(InstallPhase::Activate));
    let failure = home
        .store()
        .install(&request, &second)
        .expect_err("the injected crash interrupts the install");
    assert_eq!(failure.code, "package_install_interrupted");
    assert!(
        home.store_root()
            .join("packages")
            .join(PACKAGE_ID)
            .join("2.0.0")
            .exists(),
        "the crash left a half-published version on disk"
    );
    assert_eq!(
        home.store_root()
            .join("records")
            .join(PACKAGE_ID)
            .join("2.0.0.json")
            .exists(),
        false,
        "the crash left the version unrecorded"
    );

    // The catalogue reconciles first, so the half state is never presented.
    let listed = catalog(&home);
    let packages = listed["packages"].as_array().expect("packages");
    assert_eq!(
        packages.len(),
        1,
        "only the recorded version is in the catalogue"
    );
    assert_eq!(packages[0]["version"], VERSION);
    assert_eq!(listed["recovery"]["clean"], false);
    let abandoned = listed["recovery"]["abandonedStages"]
        .as_array()
        .expect("abandoned");
    assert_eq!(abandoned.len(), 1);
    assert_eq!(abandoned[0]["packageId"], PACKAGE_ID);
    assert_eq!(abandoned[0]["version"], "2.0.0");
    assert!(abandoned[0]["reclaimedBytes"].as_u64().expect("bytes") > 0);
    assert_eq!(
        home.store_root()
            .join("packages")
            .join(PACKAGE_ID)
            .join("2.0.0")
            .exists(),
        false,
        "recovery reclaimed the half-published version"
    );

    // The explicit route reports the same, and is idempotent.
    let recovered = run(&home, "package recover", &[]);
    is_ok(&recovered, "recover");
    assert_eq!(recovered["recovery"]["clean"], true);
    assert_eq!(
        recovered["installed"],
        json!([format!("{PACKAGE_ID}@{VERSION}")])
    );
}

// ---------------------------------------------------------------------------
// Compatibility
// ---------------------------------------------------------------------------

#[test]
fn a_package_that_does_not_cover_this_client_is_refused_at_install() {
    let home = DataHome::new("compatibility");
    let bytes = fixture_bytes(PACKAGE_ID, VERSION, &excluding_client_versions());
    let archive = archive_file(&home, "echo-incompatible.zip", &bytes);
    let archive_text = archive.display().to_string();

    let planned = run(
        &home,
        "package install-plan",
        &[("archive", archive_text.clone())],
    );
    is_ok(&planned, "install-plan");
    assert_eq!(
        planned["plan"]["compatibility"]["covers"], false,
        "the plan states the incompatibility rather than hiding it"
    );
    let plan_digest = planned["planDigest"]
        .as_str()
        .expect("plan digest")
        .to_owned();

    let confirmed = run(
        &home,
        "package install-confirm",
        &[("archive", archive_text.clone()), ("plan", plan_digest)],
    );
    is_ok(&confirmed, "install-confirm");
    let confirmation = confirmed["confirmation"]
        .as_str()
        .expect("confirmation")
        .to_owned();

    let applied = run(
        &home,
        "package install-apply",
        &[("archive", archive_text), ("confirmation", confirmation)],
    );
    assert_eq!(applied["isError"], true);
    assert_eq!(applied["reasonCode"], "package_client_incompatible");
    assert_eq!(installed_count(&home), 0, "nothing was published");
}

// ---------------------------------------------------------------------------
// The maintenance-admission seam
// ---------------------------------------------------------------------------

#[test]
fn mutating_update_and_activation_pass_the_native_idle_guard_and_cycle_its_barrier() {
    let home = DataHome::new("maintenance");
    let bytes = fixture_bytes(PACKAGE_ID, VERSION, &covering_client_versions());
    let archive = archive_file(&home, "echo.zip", &bytes);
    let archive_text = archive.display().to_string();

    // An idle host: the guard renders a verdict, so the seam is composed and the
    // read-only preview reports that apply would be admitted. Asking is not
    // holding — the preview left no barrier behind.
    let previewed = run(
        &home,
        "package update-preview",
        &[("archive", archive_text.clone())],
    );
    is_ok(&previewed, "update-preview");
    assert_eq!(previewed["mutated"], false);
    assert_eq!(previewed["apply"]["available"], true);
    assert_eq!(previewed["apply"]["operation"], "update-apply");
    assert_eq!(previewed["apply"]["guardPresent"], true);
    assert_eq!(previewed["apply"]["guardOwner"], "UPDATE-IDLE-ADMISSION");
    assert_eq!(previewed["candidate"]["plan"]["packageId"], PACKAGE_ID);
    assert!(
        !barrier_record(&home).exists(),
        "a read-only preview takes no close-admission barrier"
    );

    // Mutating update: admitted by the guard, so the route takes the durable
    // close-admission barrier, and retires it again because the replacement is
    // not wired yet. Nothing is replaced and the host stays usable.
    let updated = run(
        &home,
        "package update-apply",
        &[
            ("archive", archive_text),
            (
                "confirmation",
                "licoup.package-install-confirmation.v1:sha256:any".to_owned(),
            ),
        ],
    );
    is_ok(&updated, "update-apply");
    assert_eq!(updated["admitted"], "update-apply");
    assert_eq!(updated["admissionHeld"], true);
    assert_eq!(updated["admissionReleased"], true);
    assert_eq!(updated["replaced"], false);
    assert_eq!(updated["reasonCode"], "package_update_apply_not_wired");
    assert_eq!(updated["mutated"], false);
    assert!(
        !barrier_record(&home).exists(),
        "an admitted operation that published nothing leaves admission open"
    );

    // Activation: the same guard and the same barrier pair, over a route that
    // now serves what it is pointed at. A package this store does not hold is
    // refused, and the barrier it took is retired, so a refused activation
    // leaves the host usable rather than closed.
    let activated = run(&home, "package activate", &[]);
    assert_eq!(activated["isError"], true, "{activated}");
    assert_eq!(activated["operation"], "activate");
    assert_eq!(activated["reasonCode"], "package_not_installed");
    assert_eq!(activated["stage"], "extension/package-activation");
    assert!(!barrier_record(&home).exists());

    // Neither route installed anything, and the host is still idle.
    assert_eq!(installed_count(&home), 0);
    assert_eq!(
        licoup_native::domain::work_admission::WorkAdmission::open(&home.path)
            .admission()
            .expect("admission")
            .decision,
        licoup_native::domain::work_admission::AdmissionDecision::Idle
    );
}

/// The mount plan the client renders from has a producer on a real action.
///
/// The document is produced by the route the client addresses, over the real
/// store, from the release this host installed, and read back through the
/// publication's own reader at the path the client resolves. Nothing here writes
/// the document first: the plan is the action's output, not the test's input.
#[test]
fn serving_an_installed_release_publishes_the_mount_plan_the_client_reads() {
    let home = DataHome::new("mount-plan-publication");
    // A data package: the release carries typed resources, which is what a
    // published plan is decided from.
    let bytes = data_package_bytes(PACKAGE_ID, VERSION, &covering_client_versions());
    let archive = archive_file(&home, "appearance.zip", &bytes);
    let archive_text = archive.display().to_string();

    // Nothing is published before a package action produces one.
    assert!(
        read_published_mount_plan(&home.path)
            .expect("readable")
            .is_none(),
        "no plan exists before an action publishes one"
    );

    // Install the release through the real routes.
    let planned = run(
        &home,
        "package install-plan",
        &[("archive", archive_text.clone())],
    );
    is_ok(&planned, "install-plan");
    let plan_digest = planned["planDigest"]
        .as_str()
        .expect("plan digest")
        .to_owned();
    let confirmed = run(
        &home,
        "package install-confirm",
        &[("archive", archive_text.clone()), ("plan", plan_digest)],
    );
    is_ok(&confirmed, "install-confirm");
    let confirmation = confirmed["confirmation"]
        .as_str()
        .expect("confirmation")
        .to_owned();
    let applied = run(
        &home,
        "package install-apply",
        &[("archive", archive_text), ("confirmation", confirmation)],
    );
    is_ok(&applied, "install-apply");
    assert!(
        read_published_mount_plan(&home.path)
            .expect("readable")
            .is_none(),
        "installing decides availability, not what this host serves"
    );

    // The real action: serve the installed release.
    let declared = home
        .store()
        .installed_resources(PACKAGE_ID, VERSION)
        .expect("the installed release declares typed resources");
    assert!(
        !declared.is_empty(),
        "the fixture release is a data package with resources"
    );
    let activated = run(&home, "package activate", &[]);
    is_ok(&activated, "activate");
    assert_eq!(activated["packageId"], PACKAGE_ID);
    assert_eq!(
        activated["servedGeneration"], 1,
        "the first committed release of this version is what serves"
    );
    assert_eq!(activated["replacedGeneration"], Value::Null);
    assert_eq!(activated["planPublished"], true);
    assert_eq!(
        activated["publishedResources"],
        json!(declared.len()),
        "the plan is decided from the release's own typed resources"
    );

    // The client's reader: the document at the path the client resolves, in the
    // format the client owns.
    assert_eq!(
        mount_plan_path(&home.path),
        home.path.join("client-state/presentation/mount-plan.json")
    );
    let document = read_published_mount_plan(&home.path)
        .expect("readable")
        .expect("the action published it");
    assert_eq!(document["format"], MOUNT_PLAN_FORMAT);
    assert_eq!(document["version"], 1);
    assert_eq!(document["revision"], activated["planRevision"]);
    assert_eq!(document["hostActions"], json!([]));

    // ...carrying the real facts. Every kind resolves, and with no resource of
    // the release requested yet each one resolves to the client's own declared
    // system fact rather than to a resource nobody asked for.
    let bindings = document["bindings"].as_array().expect("bindings");
    assert_eq!(bindings.len(), ResourceKind::ALL.len());
    for kind in ResourceKind::ALL {
        let binding = bindings
            .iter()
            .find(|binding| binding["kind"] == kind.as_str())
            .unwrap_or_else(|| panic!("{kind:?} is published"));
        assert_eq!(
            binding["system"],
            system_default_name(declared_default(kind)),
            "{kind:?} serves the declared default"
        );
        assert!(binding.get("resourceId").is_none());
    }
    assert_eq!(document["fallbacks"], json!([]));
    assert_eq!(
        document["contributions"],
        json!([]),
        "nothing is contributed while the user has requested no resource"
    );

    // The publication replaces the document in one step, so no temporary file
    // is left where a reader could find it.
    let presentation = home.path.join("client-state/presentation");
    let mut entries: Vec<String> = std::fs::read_dir(&presentation)
        .expect("the client's presentation directory")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    entries.sort();
    assert_eq!(
        entries,
        vec!["mount-plan.json".to_owned()],
        "a publication leaves exactly the document"
    );

    // A document that is not one whole plan is refused rather than half-read: a
    // renderer never acts on a truncated or foreign plan.
    let path = mount_plan_path(&home.path);
    let published = std::fs::read_to_string(&path).expect("the published document");
    std::fs::write(&path, &published[..published.len() / 2]).expect("truncated document");
    let truncated = read_published_mount_plan(&home.path).expect_err("half a plan is refused");
    assert_eq!(truncated.code, "mount_plan_invalid");
    assert_eq!(truncated.stage, "extension/mount-plan");
    std::fs::write(
        &path,
        json!({ "format": "licoup.client.mount-plan.v2", "version": 2 }).to_string(),
    )
    .expect("foreign document");
    assert_eq!(
        read_published_mount_plan(&home.path)
            .expect_err("another format is refused")
            .code,
        "mount_plan_invalid"
    );

    // Serving the release again republishes a whole document, so the host that
    // owns the plan is also the one that repairs it.
    let again = run(&home, "package activate", &[]);
    is_ok(&again, "activate");
    assert_eq!(again["planPublished"], true);
    assert_eq!(
        read_published_mount_plan(&home.path)
            .expect("readable")
            .expect("republished")["format"],
        MOUNT_PLAN_FORMAT
    );
}

/// A request that crosses the bridge decides the plan.
///
/// The client asks for one kind and one namespaced identity; the native package
/// owner serves exactly that resource and publishes a document whose binding is
/// the requested resource and whose contributions are the components of the
/// generation that declares it. Without the request the same document serves the
/// declared default, so the request is what the assertions are about.
#[test]
fn a_requested_resource_becomes_the_binding_the_published_plan_serves() {
    let home = DataHome::new("resource-select");
    let bytes = data_package_bytes(PACKAGE_ID, VERSION, &covering_client_versions());
    let archive = archive_file(&home, "appearance-select.zip", &bytes);
    let archive_text = archive.display().to_string();

    let planned = run(
        &home,
        "package install-plan",
        &[("archive", archive_text.clone())],
    );
    is_ok(&planned, "install-plan");
    let plan_digest = planned["planDigest"]
        .as_str()
        .expect("plan digest")
        .to_owned();
    let confirmed = run(
        &home,
        "package install-confirm",
        &[("archive", archive_text.clone()), ("plan", plan_digest)],
    );
    is_ok(&confirmed, "install-confirm");
    let confirmation = confirmed["confirmation"]
        .as_str()
        .expect("confirmation")
        .to_owned();
    let applied = run(
        &home,
        "package install-apply",
        &[("archive", archive_text), ("confirmation", confirmation)],
    );
    is_ok(&applied, "install-apply");

    let declared = home
        .store()
        .installed_resources(PACKAGE_ID, VERSION)
        .expect("the installed release declares typed resources");
    let theme = declared
        .iter()
        .find(|resource| resource.kind() == ResourceKind::Theme)
        .expect("the release declares a theme")
        .id()
        .to_owned();
    let composition = declared
        .iter()
        .find(|resource| resource.kind() == ResourceKind::Composition)
        .expect("the release declares a composition");
    let mounted_by_id = |document: &Value, id: &str| {
        document["contributions"]
            .as_array()
            .expect("contributions")
            .iter()
            .any(|contribution| contribution["id"] == id)
    };

    // Serving the release without a request: the kind resolves to the client's
    // own declared fact, and the generation contributes nothing.
    let activated = run(&home, "package activate", &[]);
    is_ok(&activated, "activate");
    let unrequested = read_published_mount_plan(&home.path)
        .expect("readable")
        .expect("published");
    let theme_binding = unrequested["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .find(|binding| binding["kind"] == "theme")
        .expect("the theme kind is published")
        .clone();
    assert_eq!(theme_binding["system"], "appearance");
    assert!(theme_binding.get("resourceId").is_none());
    assert_eq!(unrequested["contributions"], json!([]));

    // The request crosses the bridge as a kind and a namespaced identity.
    let selected = run(
        &home,
        "package resource-select",
        &[("kind", "theme".to_owned()), ("resource", theme.clone())],
    );
    is_ok(&selected, "resource-select");
    assert_eq!(selected["kind"], "theme");
    assert_eq!(selected["resourceId"], theme.as_str());
    assert_eq!(selected["servedGeneration"], 1);
    assert_eq!(selected["planPublished"], true);

    // The client's reader now sees the requested resource serving, and the
    // generation that declares it contributing.
    let document = read_published_mount_plan(&home.path)
        .expect("readable")
        .expect("republished by the selection");
    let theme_binding = document["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .find(|binding| binding["kind"] == "theme")
        .expect("the theme kind is published")
        .clone();
    assert_eq!(theme_binding["resourceId"], theme.as_str());
    assert_eq!(theme_binding["packageId"], PACKAGE_ID);
    assert_eq!(theme_binding["packageGeneration"], 1);
    assert!(theme_binding.get("system").is_none());
    let contributions = document["contributions"].as_array().expect("contributions");
    assert!(
        !contributions.is_empty(),
        "the served generation contributes the components it declares: {document}"
    );
    for component in composition.components() {
        let published = mounted_by_id(&document, &component.component);
        // A component that binds an action no shell registered is refused by
        // the host, and a refused component has no member in the document.
        assert_eq!(
            published,
            component.action_ref.is_none(),
            "{} is published exactly when the host may mount it",
            component.component
        );
    }

    // A kind this client does not serve, an identity that is not namespaced, and
    // a resource the release does not declare are refused with their own codes:
    // none of them is answered with the declared default as though it had served.
    let unknown_kind = run(
        &home,
        "package resource-select",
        &[("kind", "colour".to_owned()), ("resource", theme.clone())],
    );
    assert_eq!(unknown_kind["isError"], true);
    assert_eq!(unknown_kind["reasonCode"], "package_resource_kind_unknown");
    let loose_identity = run(
        &home,
        "package resource-select",
        &[
            ("kind", "theme".to_owned()),
            ("resource", "theme".to_owned()),
        ],
    );
    assert_eq!(
        loose_identity["reasonCode"],
        "package_resource_identity_invalid"
    );
    let undeclared = run(
        &home,
        "package resource-select",
        &[
            ("kind", "theme".to_owned()),
            ("resource", "org.licoland.theme.absent".to_owned()),
        ],
    );
    assert_eq!(undeclared["reasonCode"], "package_resource_not_installed");

    // A refused request changed nothing: the document still serves exactly the
    // resource the accepted request asked for.
    let intact = read_published_mount_plan(&home.path)
        .expect("readable")
        .expect("published");
    assert_eq!(
        intact["bindings"]
            .as_array()
            .expect("bindings")
            .iter()
            .find(|binding| binding["kind"] == "theme")
            .expect("the theme kind is published")["resourceId"],
        theme.as_str()
    );
}

/// A barrier another switch already holds: the guard refuses through the seam,
/// with its own stable code and without opening anything.
#[test]
fn a_closed_barrier_refuses_the_mutating_routes_through_the_guard() {
    let home = DataHome::new("maintenance-closed");
    let bytes = fixture_bytes(PACKAGE_ID, VERSION, &covering_client_versions());
    let archive = archive_file(&home, "echo-closed.zip", &bytes);
    let archive_text = archive.display().to_string();
    licoup_native::domain::work_admission::hold_package_activation_admission(&home.path)
        .expect("the idle host takes the barrier");

    let previewed = run(
        &home,
        "package update-preview",
        &[("archive", archive_text.clone())],
    );
    is_ok(&previewed, "update-preview");
    assert_eq!(previewed["apply"]["available"], false);
    assert_eq!(
        previewed["apply"]["reasonCode"],
        "package_maintenance_admission_closed"
    );
    assert_eq!(previewed["apply"]["guardPresent"], true);

    let updated = run(
        &home,
        "package update-apply",
        &[
            ("archive", archive_text),
            (
                "confirmation",
                "licoup.package-install-confirmation.v1:sha256:any".to_owned(),
            ),
        ],
    );
    assert_eq!(updated["isError"], true);
    assert_eq!(
        updated["reasonCode"],
        "package_maintenance_admission_closed"
    );
    assert_eq!(updated["stage"], "extension/package-maintenance");
    assert_eq!(updated["component"], "extension_packages_maintenance");
    assert_eq!(updated["retryable"], true);
    // The CLI vocabulary has one retry value; the MCP one distinguishes them.
    assert_eq!(updated["recovery"], "retry_or_review_request");

    let activated = run(&home, "package activate", &[]);
    assert_eq!(activated["isError"], true);
    assert_eq!(
        activated["reasonCode"],
        "package_maintenance_admission_closed"
    );

    // A refusal closed nothing: the holding switch still holds the barrier.
    assert!(barrier_record(&home).exists());
    assert_eq!(installed_count(&home), 0);
    licoup_native::domain::work_admission::release_maintenance_admission(&home.path)
        .expect("released");
}

/// Unfinished local work: the guard refuses because the host is busy, and no
/// barrier record is written for the operation it refused.
#[test]
fn unfinished_local_work_refuses_the_mutating_routes_through_the_guard() {
    let home = DataHome::new("maintenance-busy");
    let bytes = fixture_bytes(PACKAGE_ID, VERSION, &covering_client_versions());
    let archive = archive_file(&home, "echo-busy.zip", &bytes);
    let store = licoup_conversation::ConversationStore::open(&home.path)
        .expect("the canonical conversation store");
    store
        .prepare_runtime_dispatch(
            "synthetic",
            "synthetic-session",
            "synthetic request",
            None,
            None,
            None,
            None,
        )
        .expect("an unfinished local dispatch");
    drop(store);

    let previewed = run(
        &home,
        "package update-preview",
        &[("archive", archive.display().to_string())],
    );
    is_ok(&previewed, "update-preview");
    assert_eq!(previewed["apply"]["available"], false);
    assert_eq!(
        previewed["apply"]["reasonCode"],
        "package_maintenance_work_in_flight"
    );
    assert_eq!(previewed["apply"]["guardPresent"], true);

    let updated = run(
        &home,
        "package update-apply",
        &[
            ("archive", archive.display().to_string()),
            (
                "confirmation",
                "licoup.package-install-confirmation.v1:sha256:any".to_owned(),
            ),
        ],
    );
    assert_eq!(updated["isError"], true);
    assert_eq!(updated["reasonCode"], "package_maintenance_work_in_flight");

    let activated = run(&home, "package activate", &[]);
    assert_eq!(activated["isError"], true);
    assert_eq!(
        activated["reasonCode"],
        "package_maintenance_work_in_flight"
    );

    assert!(
        !barrier_record(&home).exists(),
        "a refused switch writes no barrier record"
    );
    assert_eq!(installed_count(&home), 0);
}

/// A decision that cannot be read is not an idle host: the guard refuses, and so
/// does every mutating route that asks it.
#[test]
fn an_unreadable_maintenance_decision_refuses_the_mutating_routes() {
    let home = DataHome::new("maintenance-unreadable");
    let bytes = fixture_bytes(PACKAGE_ID, VERSION, &covering_client_versions());
    let archive = archive_file(&home, "echo-unreadable.zip", &bytes);
    let record = barrier_record(&home);
    licoup_foundation::platform::file_security::ensure_private_dir(
        record.parent().expect("client-state"),
    )
    .expect("client-state");
    licoup_foundation::platform::file_security::atomic_write_private_text(
        &record,
        "{ not a maintenance record",
    )
    .expect("invalid record");

    let previewed = run(
        &home,
        "package update-preview",
        &[("archive", archive.display().to_string())],
    );
    is_ok(&previewed, "update-preview");
    assert_eq!(previewed["apply"]["available"], false);
    assert_eq!(
        previewed["apply"]["reasonCode"],
        "package_maintenance_decision_unreadable"
    );
    assert_eq!(
        previewed["apply"]["guardPresent"], false,
        "a decision nobody can read is not a composed guard"
    );

    let updated = run(
        &home,
        "package update-apply",
        &[
            ("archive", archive.display().to_string()),
            (
                "confirmation",
                "licoup.package-install-confirmation.v1:sha256:any".to_owned(),
            ),
        ],
    );
    assert_eq!(updated["isError"], true);
    assert_eq!(
        updated["reasonCode"],
        "package_maintenance_decision_unreadable"
    );

    let activated = run(&home, "package activate", &[]);
    assert_eq!(activated["isError"], true);
    assert_eq!(
        activated["reasonCode"],
        "package_maintenance_decision_unreadable"
    );

    assert_eq!(installed_count(&home), 0);
}

/// The close-admission barrier the guard holds for one data root.
fn barrier_record(home: &DataHome) -> PathBuf {
    home.path
        .join("client-state")
        .join("maintenance-admission.json")
}

// ---------------------------------------------------------------------------
// Registration release through the owners
// ---------------------------------------------------------------------------

/// An owner double that records which releases arrived, in order.
struct RecordingOwners {
    released: std::cell::RefCell<Vec<String>>,
    refuse: Option<&'static str>,
}

impl RegistrationOwners for RecordingOwners {
    fn release(
        &self,
        registration: &RecordedRegistration,
    ) -> Result<ReleasedRegistration, licoup_application::ApplicationFailure> {
        if let Some(code) = self.refuse {
            return Err(licoup_application::ApplicationFailure::permanent(
                code,
                "extension/package-registration",
            ));
        }
        self.released.borrow_mut().push(format!(
            "{}:{}",
            registration.owner.wire_name(),
            registration.key
        ));
        Ok(ReleasedRegistration::removed(registration))
    }
}

/// An installed version that recorded one registration in another module's
/// surface.
fn installed_with_registration(home: &DataHome, version: &str) {
    let bytes = fixture_bytes(PACKAGE_ID, version, &covering_client_versions());
    let store = home.store();
    let request = InstallRequest::new(
        PACKAGE_ID,
        version,
        PackageSource::LocalImport,
        local_trust(&bytes),
    );
    store.install(&request, &bytes).expect("install");
    store
        .record_registration(
            PACKAGE_ID,
            version,
            RecordedRegistration::new(RegistrationOwner::CursorMcp, "land.lico.fixture"),
        )
        .expect("the owner records what it registered");
}

/// Drain one version through the real transaction, so `collect` is reached the
/// only way it can be reached.
fn drained_for(home: &DataHome, version: &str) -> Drained {
    let store = home.store();
    let installed = store
        .installed_version(PACKAGE_ID, version)
        .expect("read")
        .expect("installed");
    let mut catalogue = LocalCatalogue::default();
    catalogue.insert(PackageEntry::new(
        PACKAGE_ID,
        version,
        PackageSource::LocalImport,
    ));
    let plan = preview(&store, &catalogue, &installed, &InstanceRegistry::new()).expect("plan");
    assert_eq!(
        plan.registrations.len(),
        1,
        "the plan names what was recorded"
    );
    let mut registry = InstanceRegistry::new();
    UninstallTransaction::begin(&mut registry, plan, DependentsDecision::SelectedOnly)
        .expect("admission is withdrawn first")
        .drain(&mut registry, RemainingWork::Wait)
        .expect("nothing is in flight")
}

#[test]
fn collect_releases_every_recorded_registration_through_its_owner_before_the_bytes_move() {
    let home = DataHome::new("release");
    installed_with_registration(&home, VERSION);
    let owners = RecordingOwners {
        released: std::cell::RefCell::new(Vec::new()),
        refuse: None,
    };
    let store = home.store();
    let outcome = drained_for(&home, VERSION)
        .collect(&store, &InstanceRegistry::new(), &owners)
        .expect("collect");

    assert_eq!(
        owners.released.borrow().as_slice(),
        ["cursor-mcp:land.lico.fixture"],
        "the owner that wrote the entry is the owner asked to remove it"
    );
    assert_eq!(outcome.released_registrations.len(), 1);
    assert!(outcome.released_registrations[0].removed);
    assert!(outcome.reclaimed_bytes > 0);
    assert!(outcome.preserved.history && outcome.preserved.credentials);
    assert!(
        !store.installed_path(PACKAGE_ID, VERSION).exists(),
        "the bytes are reclaimed after the registrations are released"
    );
}

#[test]
fn a_refused_registration_release_stops_the_uninstall_with_the_bytes_still_in_place() {
    let home = DataHome::new("release-refused");
    installed_with_registration(&home, VERSION);
    let owners = RecordingOwners {
        released: std::cell::RefCell::new(Vec::new()),
        refuse: Some("package_registration_release_inputs_missing"),
    };
    let store = home.store();
    let failure = drained_for(&home, VERSION)
        .collect(&store, &InstanceRegistry::new(), &owners)
        .expect_err("the owner refused");
    assert_eq!(failure.code, "package_registration_release_inputs_missing");
    assert!(owners.released.borrow().is_empty());
    assert!(
        store
            .installed_path(PACKAGE_ID, VERSION)
            .join("agent.py")
            .exists(),
        "a refused release leaves the package installed rather than half removed"
    );
    assert!(
        store
            .installed_version(PACKAGE_ID, VERSION)
            .expect("read")
            .is_some(),
        "the record survives with the bytes"
    );
}

#[test]
fn a_running_instance_blocks_collect_and_the_owner_is_never_asked() {
    let home = DataHome::new("release-running");
    installed_with_registration(&home, VERSION);
    let owners = RecordingOwners {
        released: std::cell::RefCell::new(Vec::new()),
        refuse: None,
    };
    let store = home.store();
    let admission = store
        .admit_activation(PACKAGE_ID, VERSION)
        .expect("the version covers this client");
    let identity = InstanceIdentity::new(
        "instance-live",
        PACKAGE_ID,
        VERSION,
        1,
        7,
        Vec::<String>::new(),
    )
    .expect("identity");
    let mut machine = InstanceMachine::prepare(admission, identity).expect("preparing");
    machine.activate().expect("active");
    let mut registry = InstanceRegistry::new();
    registry.insert(machine);

    let failure = drained_for(&home, VERSION)
        .collect(&store, &registry, &owners)
        .expect_err("an active instance blocks the reclaim");
    assert_eq!(failure.code, "package_instance_still_active");
    assert!(
        owners.released.borrow().is_empty(),
        "nothing is released while the package is still serving"
    );
    assert!(
        store.installed_path(PACKAGE_ID, VERSION).exists(),
        "nothing is reclaimed either"
    );
}

/// A caller may report instances it observes, and that report can only make a
/// drain refuse — never let it past its own check.
#[test]
fn a_reported_running_instance_blocks_the_drain_route() {
    let home = DataHome::new("reported-instance");
    let bytes = fixture_bytes(PACKAGE_ID, VERSION, &covering_client_versions());
    let archive = archive_file(&home, "echo.zip", &bytes);
    let archive_text = archive.display().to_string();
    let planned = run(
        &home,
        "package install-plan",
        &[("archive", archive_text.clone())],
    );
    let plan_digest = planned["planDigest"]
        .as_str()
        .expect("plan digest")
        .to_owned();
    let confirmed = run(
        &home,
        "package install-confirm",
        &[("archive", archive_text.clone()), ("plan", plan_digest)],
    );
    let confirmation = confirmed["confirmation"]
        .as_str()
        .expect("confirmation")
        .to_owned();
    is_ok(
        &run(
            &home,
            "package install-apply",
            &[("archive", archive_text), ("confirmation", confirmation)],
        ),
        "install-apply",
    );

    let observed = json!([{
        "instanceId": "instance-observed",
        "packageId": PACKAGE_ID,
        "packageVersion": VERSION,
        "generation": 1,
        "registryEpoch": 3,
        "lifecycle": "active",
        "inFlight": 1,
    }])
    .to_string();
    let refused = run(
        &home,
        "package uninstall-drain",
        &[
            ("instances", observed.clone()),
            ("remaining", "wait".to_owned()),
        ],
    );
    assert_eq!(refused["isError"], true);
    assert_eq!(refused["reasonCode"], "package_uninstall_in_flight");
    assert!(
        !home
            .store_root()
            .join("uninstall")
            .join(format!("{PACKAGE_ID}@{VERSION}.json"))
            .exists(),
        "a refused drain writes no handoff"
    );

    // Cancelling the reported work is the user's own decision, and then the drain
    // proceeds and records that the outcome is Unknown rather than completed.
    let drained = run(
        &home,
        "package uninstall-drain",
        &[("instances", observed), ("remaining", "cancel".to_owned())],
    );
    is_ok(&drained, "uninstall-drain");
    assert_eq!(
        drained["withdrawnInstances"]
            .as_array()
            .expect("withdrawn")
            .len(),
        1
    );
    assert_eq!(drained["drainedInstances"], json!(["instance-observed"]));
    assert_eq!(drained["canceledWork"], 1);
    assert_eq!(drained["unknownWork"], 1);

    let collected = run(&home, "package uninstall-collect", &[]);
    is_ok(&collected, "uninstall-collect");
    assert_eq!(collected["canceledWork"], 1);
    assert_eq!(collected["unknownWork"], 1);
}

// ---------------------------------------------------------------------------
// The bridge family the client reaches these routes through
// ---------------------------------------------------------------------------

#[test]
fn the_package_bridge_family_names_every_route_and_every_refusal() {
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(repository_file("schemas/client_bridge/manifest.json"))
            .expect("manifest"),
    )
    .expect("manifest JSON");
    let families = manifest["families"].as_array().expect("families");
    assert_eq!(families.len(), 8, "the bridge has eight ordered families");
    let package = families
        .iter()
        .find(|family| family["id"] == "package")
        .expect("the package family is registered");
    assert_eq!(package["status"], "active");
    assert_eq!(package["schema"], "schemas/client_bridge/package.json");

    let schema: Value = serde_json::from_str(
        &std::fs::read_to_string(repository_file("schemas/client_bridge/package.json"))
            .expect("schema"),
    )
    .expect("schema JSON");
    assert_eq!(schema["title"], "PackageBridge");

    // The schema's operations and the native routes are one set, both ways.
    let mut operations = schema["operations"]
        .as_array()
        .expect("operations")
        .iter()
        .map(|operation| operation.as_str().expect("operation").to_owned())
        .collect::<Vec<_>>();
    let mut routes = licoup_native::ffi::commands::cli_command_schemas()
        .iter()
        .filter(|command| command.path().first() == Some(&"package"))
        .map(|command| {
            format!(
                "package.{}",
                command.path()[1..].join(".").replace('-', ".")
            )
        })
        .collect::<Vec<_>>();
    operations.sort();
    routes.sort();
    assert_eq!(
        operations, routes,
        "the bridge family and the native routes are exactly one set"
    );

    // Every refusal the family publishes is one this surface can actually report.
    let codes = schema["failureCodes"].as_array().expect("failure codes");
    for code in [
        "package_maintenance_decision_unreadable",
        "package_maintenance_work_in_flight",
        "package_maintenance_admission_closed",
        "package_client_incompatible",
        "package_version_already_installed",
        "package_uninstall_not_drained",
        "package_registration_release_inputs_missing",
    ] {
        assert!(
            codes.iter().any(|candidate| candidate == code),
            "the family publishes {code}"
        );
    }

    // Both generated outputs carry the family's constants and operations.
    for (path, needle) in [
        (
            "crates/licoup-native/src/ffi/generated/package.rs",
            "PACKAGE_BRIDGE_SCHEMA_VERSION",
        ),
        (
            "apps/desktop/lib/src/contracts/generated/package.g.dart",
            "packageBridgeSchemaVersion",
        ),
    ] {
        let source = std::fs::read_to_string(repository_file(path)).expect(path);
        assert!(source.contains(needle), "{path} carries {needle}");
        assert!(
            source.contains("package.uninstall.collect"),
            "{path} names the collect operation"
        );
    }

    // The generated Rust constants agree with the platform's own bounds.
    assert_eq!(
        licoup_native::ffi::generated::package::PACKAGE_BRIDGE_MAX_REGISTRATIONS,
        32
    );
    assert_eq!(
        licoup_native::ffi::generated::package::PACKAGE_BRIDGE_MAX_INSTANCES,
        64
    );
}

/// A path inside this checkout, resolved from the test binary's manifest dir.
fn repository_file(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}
