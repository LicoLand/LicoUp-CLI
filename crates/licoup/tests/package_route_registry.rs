//! The package bridge contract's native routes, pinned to the real command
//! registry.
//!
//! Why this exists: the Desktop client addresses the package routes as argv,
//! so a renamed or dropped native route breaks the client without failing any
//! test in this repository. The bridge contract `schemas/client_bridge/package.json`
//! is the declaration of the package operations a client may address; this test
//! binds every one of them to the registry the client actually calls. The
//! Desktop repository checks the routes it invokes against the released
//! `contract.json` command catalog.
//!
//! What is real here: `admit_cli_command`, the admission entry `execute_cli`
//! dispatches through, which is built by the private `build_command_table()` and
//! reached through its public projection `cli_command_schemas()`. The two
//! mutating routes are additionally driven through their production handlers
//! over a synthetic data home.
//!
//! The one naming rule this file owns is the projection from a bridge operation
//! onto its CLI route: the leading `package` namespace is the route root and
//! the remaining dotted components are hyphen-joined, so `package.update.apply`
//! is `package update-apply`.
//!
//! What is synthetic, stated plainly: the values handed to admission and to the
//! two mutating routes are placeholders. No archive is read, nothing is
//! fetched, and no package is installed.

use licoup::ffi::commands::{
    CliCommandSchema, CliExecution, OptionArity, RequiredArgumentKind, admit_cli_command,
    cli_command_schemas, execute_cli,
};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const PACKAGE_BRIDGE_SCHEMA: &str = "schemas/client_bridge/package.json";

fn repository_file(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "the expectations this test derives live at {}: {error}",
            path.display()
        )
    })
}

/// The package route family the bridge contract declares, projected onto the
/// CLI routes that answer it.
fn bridge_routes() -> Vec<Vec<String>> {
    let schema: Value = serde_json::from_str(&repository_file(PACKAGE_BRIDGE_SCHEMA))
        .expect("the package bridge contract is valid JSON");
    let operations = schema["operations"]
        .as_array()
        .expect("the package bridge contract declares its operations");
    operations
        .iter()
        .map(|operation| {
            let operation = operation
                .as_str()
                .expect("every bridge operation is a string");
            let mut components = operation.split('.');
            let family = components.next().expect("an operation has a namespace");
            assert_eq!(
                family, "package",
                "the package route family stays under one root: {operation}"
            );
            let verb = components.collect::<Vec<_>>().join("-");
            assert!(
                !verb.is_empty(),
                "a package operation names its route verb: {operation}"
            );
            vec![family.to_owned(), verb]
        })
        .collect()
}

/// The invocation the table's own schema documents for one route: the route's
/// path, its required positionals, and its required options, with placeholder
/// values. Admission is pure parsing, so a placeholder may stand in for a real
/// data home or archive.
fn documented_invocation(schema: &CliCommandSchema, route: &[String]) -> Vec<String> {
    let mut args = route.to_vec();
    for positional in schema.required_positionals() {
        args.push(placeholder(positional.kind()));
    }
    for option in schema.options().iter().filter(|option| option.required()) {
        args.push(format!("--{}", option.name()));
        if option.arity() == OptionArity::Value {
            args.push(placeholder(option.value_kind()));
        }
    }
    args
}

fn placeholder(kind: RequiredArgumentKind) -> String {
    match kind {
        RequiredArgumentKind::Text => "synthetic".to_owned(),
        RequiredArgumentKind::Json => "{}".to_owned(),
    }
}

/// Every route in `routes` that the real command table does not resolve.
///
/// A route that is absent and a route that is present but refuses the arguments
/// its own schema documents are both reported; neither may be skipped.
fn unresolved(routes: &[Vec<String>]) -> Vec<String> {
    let schemas = cli_command_schemas();
    let mut unresolved = Vec::new();
    for route in routes {
        let segments = route.iter().map(String::as_str).collect::<Vec<_>>();
        let Some(schema) = schemas
            .iter()
            .find(|schema| schema.path() == segments.as_slice())
        else {
            unresolved.push(format!("`{}` is not registered", route.join(" ")));
            continue;
        };
        let args = documented_invocation(schema, route);
        if let Err(error) = admit_cli_command(args.clone()) {
            unresolved.push(format!(
                "`{}` is registered but not addressable by its documented arguments {args:?}: {error:?}",
                route.join(" ")
            ));
        }
    }
    unresolved
}

fn assert_resolved(routes: &[Vec<String>], reason: &str) {
    let missing = unresolved(routes);
    assert!(
        missing.is_empty(),
        "every route {reason} must resolve through the native command table; missing or renamed: {missing:#?}"
    );
}

#[test]
fn the_package_bridge_routes_resolve_through_the_native_command_table() {
    let family = bridge_routes();
    assert!(
        family
            .iter()
            .any(|route| route == &["package".to_owned(), "catalog".to_owned()]),
        "the bridge contract declares the catalogue route, found {family:#?}"
    );
    let reason = format!("{PACKAGE_BRIDGE_SCHEMA} declares");
    assert_resolved(&family, &reason);
}

// ---------------------------------------------------------------------------
// The maintenance admission barrier on the two mutating routes
// ---------------------------------------------------------------------------

fn synthetic_root(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a monotonic clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "licoup-package-route-registry-{tag}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).expect("a synthetic data home");
    root
}

fn json_report(args: Vec<String>) -> Value {
    let execution =
        execute_cli(args.clone()).unwrap_or_else(|error| panic!("{args:?} is admitted: {error:?}"));
    let CliExecution::Json(report) = execution else {
        panic!("a package route publishes a JSON report: {args:?}");
    };
    report
}

fn admission_decision(root: &Path) -> licoup::domain::work_admission::AdmissionDecision {
    licoup::domain::work_admission::WorkAdmission::open(root)
        .admission()
        .expect("the maintenance admission is readable")
        .decision
}

/// The durable barrier record the domain's work admission writes, at the path
/// that owner names.
fn barrier_record(root: &Path) -> PathBuf {
    root.join("client-state").join("maintenance-admission.json")
}

/// The two mutating routes refuse through the maintenance admission barrier with
/// the documented code, and a refusal writes nothing: no package store appears,
/// the archive is never read, and the barrier another switch holds is left held.
#[test]
fn the_mutating_package_routes_refuse_through_the_maintenance_barrier() {
    let idle = synthetic_root("idle");
    let previewed = json_report(vec![
        "package".to_owned(),
        "update-preview".to_owned(),
        idle.display().to_string(),
        "synthetic-package".to_owned(),
    ]);
    assert_eq!(previewed["isError"], false, "{previewed}");
    assert_eq!(previewed["operation"], "update-preview");
    assert_eq!(previewed["mutated"], false);
    assert_eq!(previewed["apply"]["operation"], "update-apply");
    assert_eq!(previewed["apply"]["available"], true);
    assert_eq!(previewed["apply"]["guardPresent"], true);
    assert!(
        !barrier_record(&idle).exists(),
        "a read-only preview takes no close-admission barrier"
    );
    std::fs::remove_dir_all(&idle).expect("the synthetic data home is removed");

    let root = synthetic_root("barrier");
    licoup::domain::work_admission::hold_package_activation_admission(&root)
        .expect("the idle host takes the barrier");
    let archive = root.join("synthetic-package.zip");
    let held = admission_decision(&root);
    assert_eq!(
        held,
        licoup::domain::work_admission::AdmissionDecision::Closed,
        "the barrier is held before the mutating routes ask"
    );

    let refusal = json_report(vec![
        "package".to_owned(),
        "update-apply".to_owned(),
        root.display().to_string(),
        "synthetic-package".to_owned(),
        "--archive".to_owned(),
        archive.display().to_string(),
        "--confirmation".to_owned(),
        "licoup.package-install-confirmation.v1:sha256:synthetic".to_owned(),
    ]);
    assert_eq!(refusal["isError"], true, "{refusal}");
    assert_eq!(refusal["operation"], "update-apply");
    assert_eq!(
        refusal["reasonCode"],
        "package_maintenance_admission_closed"
    );
    assert_eq!(refusal["stage"], "extension/package-maintenance");
    assert_eq!(refusal["component"], "extension_packages_maintenance");
    assert_eq!(
        refusal["effect"], "not-attempted",
        "the refusal happens before the mutation path: {refusal}"
    );

    let activation = json_report(vec![
        "package".to_owned(),
        "activate".to_owned(),
        root.display().to_string(),
        "synthetic-package".to_owned(),
        "1.0.0".to_owned(),
    ]);
    assert_eq!(activation["isError"], true, "{activation}");
    assert_eq!(activation["operation"], "activate");
    assert_eq!(
        activation["reasonCode"],
        "package_maintenance_admission_closed"
    );
    assert_eq!(activation["effect"], "not-attempted");

    assert_eq!(
        admission_decision(&root),
        licoup::domain::work_admission::AdmissionDecision::Closed,
        "a refused route does not retire the barrier its holder owns"
    );
    assert!(
        !licoup_foundation::platform::paths::package_store_root(&root).exists(),
        "a refused route writes no package store"
    );
    assert!(
        !archive.exists(),
        "a refused route never touches the archive it was pointed at"
    );
    assert!(
        licoup::domain::work_admission::release_maintenance_admission(&root).is_ok(),
        "the test releases the barrier it took"
    );
    std::fs::remove_dir_all(&root).expect("the synthetic data home is removed");
}
