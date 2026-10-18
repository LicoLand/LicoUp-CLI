//! Declared artifact inputs through the published CLI surface.
//!
//! Every case runs the real `licoup-cli` process against a synthetic data root,
//! so what is proven is the composition a client runs: route admission, the
//! typed command, the native port, the authorization that port applies, and the
//! durable store inside the layout owner's client-state root. Removing a route,
//! the port arm, or the store call makes these cases fail rather than silently
//! pass, which is the wiring this file exists to hold in place.
//!
//! The four behaviours the declared-dependency model exists for are asserted
//! here, not in the crate that owns the rules: a blocked shared producer blocks
//! exactly its actual consumers while an unrelated branch progresses, a missing
//! artifact reference is reported explicitly, a cycle is refused with the
//! actionable path, and an unauthorized cross-project reference or a location
//! escaping the authorized root is refused.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// One synthetic user home and data root for the real CLI process.
struct CliFixture {
    base: PathBuf,
    home: PathBuf,
    root: PathBuf,
}

impl CliFixture {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "licoup-project-cli-{label}-{}-{nonce}",
            std::process::id()
        ));
        let home = base.join("home");
        let root = base.join("data");
        std::fs::create_dir_all(&home).expect("the synthetic home is creatable");
        std::fs::create_dir_all(&root).expect("the synthetic data root is creatable");
        Self { base, home, root }
    }

    /// A real authorized root of one synthetic project.
    fn authorized_root(&self, label: &str) -> PathBuf {
        let root = self.base.join("authorized").join(label);
        std::fs::create_dir_all(&root).expect("the synthetic authorized root is creatable");
        root
    }

    /// A sibling beside the authorized roots: the location a reference that
    /// escapes its root would reach if it were followed.
    fn outside_root(&self, label: &str) -> PathBuf {
        let root = self.base.join("outside").join(label);
        std::fs::create_dir_all(&root).expect("the synthetic outside root is creatable");
        root
    }

    fn write(&self, at: &Path, relative: &str, contents: &str) -> PathBuf {
        let location = at.join(relative);
        if let Some(parent) = location.parent() {
            std::fs::create_dir_all(parent).expect("the synthetic parent is creatable");
        }
        std::fs::write(&location, contents).expect("the synthetic artifact is writable");
        location
    }

    /// Run one real CLI invocation and return its complete output.
    fn output(&self, args: &[String]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_licoup-cli"))
            .args(args)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data-dir"))
            .env("APPDATA", self.home.join("appdata"))
            .env("LOCALAPPDATA", self.home.join("local-appdata"))
            .env("LICOUP_HOME", &self.root)
            .env("LICOUP_MCP_AUTOSTART", "0")
            .env("LICO_MOBILE_RELAY_NATIVE_SECRET_STORE", "disabled")
            .env_remove("LICOUP_CLIENT_PID")
            .env_remove("RUST_LOG")
            .env_remove("RUST_BACKTRACE")
            .output()
            .expect("the real licoup CLI must be runnable")
    }

    /// Run one real CLI invocation and return the single envelope it prints.
    fn run(&self, args: &[String]) -> Value {
        let output = self.output(args);
        let invoked = args.join(" ");
        assert!(
            output.status.success(),
            "licoup {invoked} exited with {}: stdout={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "licoup {invoked} must print one JSON envelope ({error}): {}",
                String::from_utf8_lossy(&output.stdout)
            )
        })
    }

    /// Run one invocation that must resolve, returning the outcome payload.
    fn resolved(&self, args: &[String]) -> Value {
        let envelope = self.run(args);
        assert_eq!(
            envelope["schema"], "licoup.project-identity/v1",
            "the project family publishes one envelope: {envelope}"
        );
        assert_eq!(
            envelope["status"], "ok",
            "expected a resolution: {envelope}"
        );
        envelope["outcome"]["payload"].clone()
    }

    /// Run one invocation that must be refused, returning the failure body.
    fn refused(&self, args: &[String]) -> Value {
        let envelope = self.run(args);
        assert_eq!(
            envelope["status"], "failed",
            "expected a refusal: {envelope}"
        );
        envelope["failure"].clone()
    }
}

impl Drop for CliFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn strings<'a>(values: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    values.into_iter().map(str::to_owned).collect()
}

/// One registration of an explicitly declared project, as the CLI publishes it.
fn register(project_id: &str, authorized_root: &Path) -> Vec<String> {
    let payload = json!({
        "projectId": project_id,
        "displayName": format!("Synthetic project {project_id}"),
        "authorizedRoot": authorized_root.to_string_lossy(),
        "authorityKind": "membership",
        "authorityReference": "membership:owner",
        "workspaceId": "workspace:shared",
        "planId": format!("plan:{project_id}"),
    })
    .to_string();
    strings(["project", "register", "--stdin-json", &payload])
}

/// One accepted dependency declaration, driven through the `--stdin-json` route.
fn declare(declaration: Value) -> Vec<String> {
    let payload = declaration.to_string();
    strings(["project", "dependency", "declare", "--stdin-json", &payload])
}

fn dependencies(project_id: &str) -> Vec<String> {
    strings(["project", "dependency", "list", project_id])
}

fn unresolved(project_id: &str) -> Vec<String> {
    strings(["project", "dependency", "unresolved", project_id])
}

fn blocked(project_id: &str, work_item_id: &str) -> Vec<String> {
    strings(["project", "dependency", "blocked", project_id, work_item_id])
}

/// A local declaration: the consumer takes one location inside its own root.
fn local(project_id: &str, consumer: &str, producer: &str, path: &str) -> Value {
    json!({
        "projectId": project_id,
        "workItemId": consumer,
        "artifact": {
            "kind": "local",
            "producerWorkItemId": producer,
            "path": path,
        },
    })
}

/// A cross-project declaration: the consumer references a shared result.
fn cross_project(
    project_id: &str,
    consumer: &str,
    producer_project: &str,
    producer: &str,
) -> Value {
    json!({
        "projectId": project_id,
        "workItemId": consumer,
        "artifact": {
            "kind": "cross-project",
            "projectId": producer_project,
            "workItemId": producer,
        },
    })
}

/// One work-item reference listing as comparable `project/work-item` strings.
fn work_refs(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("a work-item listing must be an array: {value}"))
        .iter()
        .map(|work| {
            format!(
                "{}/{}",
                work["projectId"].as_str().unwrap_or_default(),
                work["workItemId"].as_str().unwrap_or_default()
            )
        })
        .collect()
}

fn sorted(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values
}

#[test]
fn a_blocked_shared_producer_blocks_exactly_its_consumers_through_the_cli() {
    let cli = CliFixture::new("blocked");
    let alpha = cli.authorized_root("alpha");
    let bravo = cli.authorized_root("bravo");
    cli.resolved(&register("alpha-project", &alpha));
    cli.resolved(&register("bravo-project", &bravo));
    cli.write(&alpha, "dist/out.bin", "synthetic build output");

    // alpha: build -> test -> package, with an independent lint branch.
    cli.resolved(&declare(local(
        "alpha-project",
        "test",
        "build",
        "dist/out.bin",
    )));
    cli.resolved(&declare(local(
        "alpha-project",
        "package",
        "test",
        "dist/package.tar",
    )));
    cli.resolved(&declare(local(
        "alpha-project",
        "lint",
        "build",
        "dist/lint.log",
    )));
    // bravo references the shared result across projects, and keeps an
    // unrelated branch with its own producer.
    let shared = cli.resolved(&declare(cross_project(
        "bravo-project",
        "integrate",
        "alpha-project",
        "build",
    )));
    assert_eq!(
        shared["artifactState"], "materialized",
        "a shared result is referenced, not produced twice: {shared}"
    );
    cli.resolved(&declare(local(
        "bravo-project",
        "release",
        "integrate",
        "dist/release.tar",
    )));
    cli.resolved(&declare(local(
        "bravo-project",
        "docs",
        "docs-source",
        "docs/site.html",
    )));

    let consumers = cli.resolved(&blocked("alpha-project", "build"));
    assert_eq!(consumers["producer"]["projectId"], "alpha-project");
    assert_eq!(consumers["producer"]["workItemId"], "build");
    assert_eq!(
        sorted(work_refs(&consumers["blockedConsumers"])),
        sorted(strings([
            "alpha-project/lint",
            "alpha-project/test",
            "bravo-project/integrate",
            "alpha-project/package",
            "bravo-project/release",
        ])),
        "a shared producer blocks exactly its declared consumers, transitively: {consumers}"
    );
    assert!(
        !work_refs(&consumers["blockedConsumers"]).contains(&"bravo-project/docs".to_owned()),
        "an unrelated branch does not wait on a blocked producer"
    );

    // The unrelated branch keeps its own consumer set, and a leaf blocks none.
    let unrelated = cli.resolved(&blocked("bravo-project", "docs-source"));
    assert_eq!(
        work_refs(&unrelated["blockedConsumers"]),
        strings(["bravo-project/docs"])
    );
    let leaf = cli.resolved(&blocked("bravo-project", "release"));
    assert_eq!(leaf["blockedConsumers"], json!([]));
    let undeclared = cli.resolved(&blocked("alpha-project", "missing"));
    assert_eq!(undeclared["blockedConsumers"], json!([]));
}

#[test]
fn a_missing_artifact_reference_is_reported_explicitly_through_the_cli() {
    let cli = CliFixture::new("missing");
    let alpha = cli.authorized_root("alpha");
    let bravo = cli.authorized_root("bravo");
    cli.resolved(&register("alpha-project", &alpha));
    cli.resolved(&register("bravo-project", &bravo));

    let absent = cli.resolved(&declare(local(
        "alpha-project",
        "test",
        "build",
        "dist/absent.bin",
    )));
    assert_eq!(
        absent["artifactState"], "missing",
        "an absent location is reported, never silently startable: {absent}"
    );
    // An authorized cross-project reference to a result the referenced project
    // does not declare is still an explicit answer.
    cli.resolved(&declare(cross_project(
        "bravo-project",
        "integrate",
        "alpha-project",
        "ghost",
    )));

    let report = cli.resolved(&unresolved("alpha-project"));
    assert_eq!(report["projectId"], "alpha-project");
    let entries = report["unresolvedArtifacts"]
        .as_array()
        .expect("the report carries entries");
    assert_eq!(entries.len(), 1, "one unresolved reference: {report}");
    assert_eq!(entries[0]["artifactState"], "missing");
    assert_eq!(entries[0]["consumer"]["projectId"], "alpha-project");
    assert_eq!(entries[0]["consumer"]["workItemId"], "test");
    assert_eq!(
        entries[0]["artifact"]["path"], "dist/absent.bin",
        "the report names the declared reference itself"
    );

    let cross = cli.resolved(&unresolved("bravo-project"));
    let cross_entries = cross["unresolvedArtifacts"]
        .as_array()
        .expect("the report carries entries");
    assert_eq!(cross_entries.len(), 1, "one unresolved reference: {cross}");
    assert_eq!(cross_entries[0]["artifact"]["kind"], "cross-project");
    assert_eq!(cross_entries[0]["artifact"]["projectId"], "alpha-project");
    assert_eq!(cross_entries[0]["artifact"]["workItemId"], "ghost");

    // The state describes the result, not a remembered answer: once the
    // producer's output exists, the same declaration reports it materialized.
    cli.write(&alpha, "dist/absent.bin", "synthetic later output");
    let after = cli.resolved(&unresolved("alpha-project"));
    assert_eq!(after["unresolvedArtifacts"], json!([]));
    let listed = cli.resolved(&dependencies("alpha-project"));
    assert_eq!(listed["dependencies"][0]["artifactState"], "materialized");
}

#[test]
fn a_cycle_is_refused_through_the_cli_with_the_actionable_path() {
    let cli = CliFixture::new("cycle");
    let alpha = cli.authorized_root("alpha");
    let bravo = cli.authorized_root("bravo");
    cli.resolved(&register("alpha-project", &alpha));
    cli.resolved(&register("bravo-project", &bravo));

    cli.resolved(&declare(local(
        "alpha-project",
        "lint",
        "build",
        "build/lint.log",
    )));
    cli.resolved(&declare(local(
        "alpha-project",
        "package",
        "lint",
        "dist/package.tar",
    )));

    let refusal = cli.refused(&declare(local(
        "alpha-project",
        "build",
        "package",
        "dist/rebuilt.tar",
    )));
    assert_eq!(refusal["code"], "project_dependency_cycle");
    assert_eq!(refusal["stage"], "project/dependency");
    assert_eq!(refusal["retryable"], false);
    assert_eq!(
        refusal["presentationArgs"]["dependencyPath"],
        "alpha-project/build -> alpha-project/package -> alpha-project/lint -> alpha-project/build",
        "the refusal carries the path the caller has to break: {refusal}"
    );

    // The same rule holds across projects.
    cli.resolved(&declare(cross_project(
        "bravo-project",
        "integrate",
        "alpha-project",
        "package",
    )));
    let cross = cli.refused(&declare(cross_project(
        "alpha-project",
        "package",
        "bravo-project",
        "integrate",
    )));
    assert_eq!(cross["code"], "project_dependency_cycle");
    assert_eq!(
        cross["presentationArgs"]["dependencyPath"],
        "alpha-project/package -> bravo-project/integrate -> alpha-project/package"
    );

    let listed = cli.resolved(&dependencies("alpha-project"));
    assert_eq!(
        listed["dependencies"]
            .as_array()
            .expect("the listing carries entries")
            .len(),
        2,
        "a refused edge leaves no row behind: {listed}"
    );
}

#[test]
fn an_unauthorized_reference_or_an_escaping_location_is_refused_through_the_cli() {
    let cli = CliFixture::new("unauthorized");
    let alpha = cli.authorized_root("alpha");
    cli.resolved(&register("alpha-project", &alpha));

    let ghost = cli.refused(&declare(cross_project(
        "alpha-project",
        "test",
        "ghost-project",
        "build",
    )));
    assert_eq!(ghost["code"], "project_artifact_reference_unauthorized");
    assert_eq!(ghost["stage"], "project/dependency");
    assert_eq!(ghost["retryable"], false);

    // Two real locations a following implementation would have reached: a
    // sibling beside the authorized root, and an absolute location.
    let sibling = cli.outside_root("alpha-secret");
    let leak = cli.write(&sibling, "leak.txt", "synthetic sibling content");
    let absolute = cli.write(&cli.base, "absolute.txt", "synthetic absolute content");
    for escaped in [
        "../outside/alpha-secret/leak.txt".to_owned(),
        "dist/../../outside/alpha-secret/leak.txt".to_owned(),
        absolute.to_string_lossy().into_owned(),
    ] {
        let refusal = cli.refused(&declare(local("alpha-project", "test", "build", &escaped)));
        assert_eq!(
            refusal["code"],
            "project_artifact_reference_escapes_authorized_root"
        );
        assert_eq!(refusal["stage"], "project/dependency");
    }

    let listed = cli.resolved(&dependencies("alpha-project"));
    assert_eq!(
        listed["dependencies"],
        json!([]),
        "a refused reference leaves no row behind: {listed}"
    );
    assert_eq!(
        std::fs::read_to_string(&leak).expect("the sibling location is untouched"),
        "synthetic sibling content"
    );
}

#[test]
fn the_declared_commands_are_reachable_only_through_their_published_routes() {
    // Admission is the wiring a client depends on: a route that lost its
    // registration is unknown rather than silently served by another family,
    // and a registered route refuses its own missing arguments by name before
    // any port runs.
    let cli = CliFixture::new("routes");
    // The typed admission error carries the stage the code belongs to; the
    // process publishes the code on stderr, so both are asserted.
    let refusal =
        licoup_native::ffi::commands::execute_cli(strings(["project", "dependency", "declare"]))
            .expect_err("the declaration route refuses a missing payload");
    let refusal = refusal
        .downcast_ref::<licoup_native::ffi::commands::CliCommandError>()
        .expect("an admission refusal is typed");
    assert_eq!(refusal.code(), "cli_required_option_missing");
    assert_eq!(refusal.stage(), "cli/admission");

    for (args, expected) in [
        (
            strings(["project", "dependency", "declare"]),
            "cli_required_option_missing",
        ),
        (
            strings(["project", "dependency", "list"]),
            "cli_required_argument_missing",
        ),
        (
            strings(["project", "dependency", "unresolved"]),
            "cli_required_argument_missing",
        ),
        (
            strings(["project", "dependency", "blocked", "alpha-project"]),
            "cli_required_argument_missing",
        ),
    ] {
        let output = cli.output(&args);
        assert!(
            !output.status.success(),
            "{} must be admitted and then refused: stdout={}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout)
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(expected),
            "{} must refuse through admission with {expected}: {stderr}",
            args.join(" ")
        );
    }
    let unknown = cli.output(&strings(["project", "dependency", "unknown"]));
    assert!(
        String::from_utf8_lossy(&unknown.stderr).contains("cli_operation_unsupported"),
        "an unregistered dependency verb is refused rather than served: {}",
        String::from_utf8_lossy(&unknown.stderr)
    );
}
