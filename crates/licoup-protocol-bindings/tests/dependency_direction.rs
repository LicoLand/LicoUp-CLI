//! A03 — the endpoint lane's dependency direction, checked against the real
//! Cargo graph and a real compilation context.
//!
//! The lane's direction is `platform adapter → protocol bindings → fixed SDK`,
//! with the consumer-owned ports owned by `licoup-endpoint-core`. Two things
//! could break it: someone adds an edge that reverses it, or someone writes a
//! reference that only compiles because such an edge exists. Reachability is
//! read from Cargo's own resolved graph. A forbidden *reference* is checked by
//! compiling it in a probe crate whose declared dependencies mirror the subject
//! crate's own declared dependencies, read from `cargo metadata`.
//!
//! The mirror is the point. Under the 2018+ extern prelude a name resolves only
//! through a dependency the probe itself declares, so a probe that merely
//! depends on the subject crate cannot name what the subject crate reaches: the
//! forbidden reference would fail even while the illegal edge exists, which
//! would be an empty oracle. Mirroring the declared dependency list makes the
//! probe name exactly what the subject crate can name. Compiling the reference
//! therefore means the subject declares that edge, and failing with the target
//! unresolved (`E0432`/`E0433`) means it does not. Positive controls compile the
//! same reference with the edge added, so a refusal is never just the probe's
//! own shape.
//!
//! | crate | may reach |
//! | --- | --- |
//! | `licoup-endpoint-core` | nothing at all: a consumer-owned port cannot name the SDK it exists to keep out |
//! | `licoup-protocol-bindings` | the fixed SDK and its transitive crates, never the platform adapter |
//! | platform adapter (`licoup-native`, M12) | both of the above |
//!
//! `view → RPC` is the same class of edge on the Dart side and is deliberately
//! *not* re-proved here: `npm run client:verify:architecture` already owns the
//! Flutter presentation boundary (`flutter.presentation-boundary`), and a second
//! Rust-side copy would be another gate platform rather than a stronger check.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

const CORE: &str = "licoup-endpoint-core";
const BINDINGS: &str = "licoup-protocol-bindings";
const SDK: &str = "licoarc-rust";
const ADAPTER: &str = "licoup-native";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate lives at <workspace>/crates/<name>")
        .to_path_buf()
}

/// Every package reachable from `root`, as Cargo itself resolves it.
///
/// The reading comes from `cargo tree`, so the answer is the resolved graph and
/// not a scan of manifest text or directory names.
fn reachable(root: &str) -> BTreeSet<String> {
    let mut command = Command::new(env!("CARGO"));
    command
        .args([
            "tree",
            "-p",
            root,
            "-e",
            "all",
            "--prefix",
            "none",
            "--no-dedupe",
            "--format",
            "{p}",
            "--offline",
        ])
        .current_dir(workspace_root());
    // Reuse the already-built artifacts when the caller pinned a target
    // directory, so this costs a resolution rather than a rebuild.
    if let Ok(target) = std::env::var("CARGO_TARGET_DIR") {
        command.env("CARGO_TARGET_DIR", target);
    }
    let output = command.output().expect("cargo tree runs");
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("cargo tree writes UTF-8")
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

/// Cargo's own metadata for this workspace, read once.
fn cargo_metadata() -> serde_json::Value {
    static METADATA: OnceLock<serde_json::Value> = OnceLock::new();
    METADATA
        .get_or_init(|| {
            let mut command = Command::new(env!("CARGO"));
            command.args([
                "metadata",
                "--offline",
                "--locked",
                "--format-version",
                "1",
                "--no-deps",
            ]);
            command.current_dir(workspace_root());
            let output = command.output().expect("cargo metadata runs");
            assert!(
                output.status.success(),
                "cargo metadata failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            serde_json::from_slice(&output.stdout).expect("cargo metadata writes JSON")
        })
        .clone()
}

/// The workspace's resolved target directory, so probe builds reuse the same
/// already-built dependency artifacts as the rest of the suite.
fn target_directory() -> PathBuf {
    PathBuf::from(
        cargo_metadata()["target_directory"]
            .as_str()
            .expect("metadata names the target directory"),
    )
}

/// One dependency a subject crate declares: the name the subject's own source
/// refers to it by, and the manifest entry that reproduces the same source.
struct DeclaredDependency {
    extern_name: String,
    manifest_entry: String,
}

/// The subject crate's real declared dependency context, from `cargo metadata`.
///
/// Optional dependencies are kept: an enabled optional dependency is nameable,
/// and a probe that is too permissive can only produce a loud false alarm, never
/// a silent pass. Dev and build dependencies are left out because the library
/// source under test cannot name them.
fn declared_dependencies(subject: &str) -> Vec<DeclaredDependency> {
    let metadata = cargo_metadata();
    let package = metadata["packages"]
        .as_array()
        .expect("metadata lists packages")
        .iter()
        .find(|package| package["name"] == subject)
        .unwrap_or_else(|| panic!("{subject} is a workspace member"));
    package["dependencies"]
        .as_array()
        .expect("a package has a dependency list")
        .iter()
        .filter(|dependency| dependency["kind"].is_null())
        .map(manifest_entry)
        .collect()
}

/// A direct path dependency on one workspace crate.
fn path_dependency(crate_name: &str) -> DeclaredDependency {
    DeclaredDependency {
        extern_name: crate_name.replace('-', "_"),
        manifest_entry: format!(
            "{crate_name} = {{ path = \"{}\" }}",
            workspace_root().join("crates").join(crate_name).display()
        ),
    }
}

/// Converts one `cargo metadata` dependency into the manifest entry that
/// reproduces it.
fn manifest_entry(dependency: &serde_json::Value) -> DeclaredDependency {
    let package = dependency["name"]
        .as_str()
        .expect("a dependency has a package name");
    let extern_name = dependency["rename"].as_str().unwrap_or(package).to_owned();
    let mut fields = Vec::new();
    if extern_name != package {
        fields.push(format!("package = \"{package}\""));
    }
    if let Some(path) = dependency["path"].as_str() {
        fields.push(format!("path = \"{path}\""));
    } else if let Some(source) = dependency["source"].as_str() {
        if let Some(git) = source.strip_prefix("git+") {
            let (url, rev) = git_source(git);
            fields.push(format!("git = \"{url}\""));
            if let Some(rev) = rev {
                fields.push(format!("rev = \"{rev}\""));
            }
        } else if source.starts_with("registry+") {
            let requirement = dependency["req"]
                .as_str()
                .expect("a registry dependency has a version requirement");
            fields.push(format!("version = \"{requirement}\""));
        } else {
            panic!("unsupported dependency source for {package}: {source}");
        }
    } else {
        panic!("dependency {package} names no source");
    }
    if dependency["uses_default_features"] == false {
        fields.push("default-features = false".to_owned());
    }
    let features = dependency["features"]
        .as_array()
        .expect("a dependency has a feature list");
    if !features.is_empty() {
        let names = features
            .iter()
            .map(|feature| format!("\"{}\"", feature.as_str().expect("features are strings")))
            .collect::<Vec<_>>()
            .join(", ");
        fields.push(format!("features = [{names}]"));
    }
    DeclaredDependency {
        manifest_entry: format!("{extern_name} = {{ {} }}", fields.join(", ")),
        extern_name,
    }
}

/// Splits one Cargo git source into its URL and its pinned revision.
///
/// Cargo writes the requirement as a query (`…?rev=<rev>`) and may add the
/// resolved commit as a fragment (`…#<commit>`).
fn git_source(source: &str) -> (&str, Option<&str>) {
    let (without_fragment, fragment) = match source.split_once('#') {
        Some((head, fragment)) => (head, Some(fragment)),
        None => (source, None),
    };
    if let Some((url, query)) = without_fragment.split_once('?') {
        let rev = query
            .split('&')
            .find_map(|part| part.strip_prefix("rev="))
            .or(fragment);
        return (url, rev);
    }
    (without_fragment, fragment)
}

/// Compiles one probe crate with exactly the given dependencies, so the only
/// names that resolve are the ones the probe itself declares.
fn compile_probe(label: &str, dependencies: &[DeclaredDependency], source: &str) -> (bool, String) {
    let root = workspace_root();
    let probe = root.join("build/trybuild-local").join(label);
    let _ = std::fs::remove_dir_all(&probe);
    std::fs::create_dir_all(probe.join("src")).expect("probe source dir");

    let declared = dependencies
        .iter()
        .map(|dependency| format!("{}\n", dependency.manifest_entry))
        .collect::<String>();
    let manifest = format!(
        "[package]\nname = \"{label}\"\nversion = \"0.0.0\"\nedition = \"2024\"\npublish = false\n\n\
         [dependencies]\n{declared}\n[workspace]\n"
    );
    std::fs::write(probe.join("Cargo.toml"), manifest).expect("probe manifest");
    std::fs::write(probe.join("src/lib.rs"), source).expect("probe source");

    let mut command = Command::new(env!("CARGO"));
    command
        .args(["build", "--offline", "--quiet"])
        .current_dir(&probe);
    // Reuse the already-built artifacts when the caller pinned a target
    // directory; otherwise reuse the workspace's own resolved one, so this
    // costs a resolution rather than a rebuild.
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(target_directory, PathBuf::from);
    command.env("CARGO_TARGET_DIR", target);
    let output = command.output().expect("probe compiles");
    let _ = std::fs::remove_dir_all(&probe);
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn the_ports_owner_stays_dependency_free() {
    let graph = reachable(CORE);
    for forbidden in [SDK, BINDINGS, ADAPTER] {
        assert!(
            !graph.contains(forbidden),
            "{CORE} must not reach {forbidden}; the consumer owns these ports \
             precisely so the protocol side stays on the other side of them"
        );
    }
    assert_eq!(
        graph,
        BTreeSet::from([CORE.to_owned()]),
        "{CORE} declared a dependency: every edge kind (normal, build, dev) is \
         part of this direction, and the core owns none of them"
    );
}

#[test]
fn the_sdk_boundary_reaches_the_fixed_sdk_and_not_the_platform_adapter() {
    let graph = reachable(BINDINGS);
    assert!(
        graph.contains(SDK),
        "{BINDINGS} must depend on the fixed SDK; the boundary adapts it rather \
         than re-implementing the Protocol Line"
    );
    assert!(
        !graph.contains(ADAPTER),
        "{BINDINGS} must not reach {ADAPTER}; the platform adapter composes the \
         bindings, so the bindings cannot name it"
    );
    // The boundary owns no LicoUp dependency at all. An edge from this crate to
    // another LicoUp crate would let a later node build a second protocol path
    // behind the adapter instead of composing this one.
    for package in &graph {
        assert!(
            package.as_str() == BINDINGS
                || (!package.starts_with("licoup-") && !package.starts_with("lico-")),
            "{BINDINGS} must not reach {package}; every LicoUp layer sits above the \
             adapter that composes this crate"
        );
    }
}

/// Every forbidden reference the lane's direction names: the subject crate that
/// must not name it, the extern name it tries to reach, and real source that
/// would compile if — and only if — that dependency were declared.
const FORBIDDEN_REFERENCES: [(&str, &str, &str, &str); 3] = [
    (
        "core_cannot_use_the_sdk",
        CORE,
        "licoarc",
        "use licoarc::artifact::VerifiedProtocolLine;\n\
         #[allow(dead_code)]\n\
         fn probe(_line: &VerifiedProtocolLine) {}\n",
    ),
    (
        "core_cannot_use_the_bindings",
        CORE,
        "licoup_protocol_bindings",
        "use licoup_protocol_bindings::EndpointConsumer;\n\
         #[allow(dead_code)]\n\
         fn probe() -> EndpointConsumer { EndpointConsumer::new() }\n",
    ),
    (
        "bindings_cannot_use_the_platform_adapter",
        BINDINGS,
        "licoup_native",
        "use licoup_native::domain::protocol_input_admission::AuthorityInput;\n\
         #[allow(dead_code)]\n\
         fn probe<'a>(bytes: &'a [u8]) -> AuthorityInput<'a> { AuthorityInput::new(bytes) }\n",
    ),
];

/// The oracle's answer for every entry: an empty list means each reference was
/// refused for the right reason.
///
/// A reference that compiles is reported as a declared edge, and a failure that
/// is not the target's own `E0432`/`E0433` is reported as inconclusive, so a
/// probe that fails for any other reason (a typo, an unavailable offline
/// dependency, a toolchain problem) proves nothing.
fn forbidden_reference_failures(entries: &[(&str, &str, &str, &str)]) -> Vec<String> {
    let mut failures = Vec::new();
    for &(label, subject, target, source) in entries {
        let (succeeded, stderr) = compile_probe(label, &declared_dependencies(subject), source);
        if succeeded {
            failures.push(format!(
                "{label}: {subject} compiled a reference to {target} it must not be \
                 able to make; the edge is declared"
            ));
            continue;
        }
        if !(stderr.contains("E0432") || stderr.contains("E0433")) || !stderr.contains(target) {
            failures.push(format!(
                "{label}: the reference to {target} failed for a reason other than the \
                 missing dependency: {stderr}"
            ));
        }
    }
    failures
}

#[test]
fn each_forbidden_reference_fails_to_compile_in_its_subject_context() {
    let failures = forbidden_reference_failures(&FORBIDDEN_REFERENCES);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The mirror has to be the subject's real declared context, or the refusals
/// above would be produced by an empty probe rather than by the missing edge.
#[test]
fn the_probe_context_is_the_subjects_own_declared_context() {
    assert!(
        declared_dependencies(CORE).is_empty(),
        "the core declares no dependency, so its probe context must be empty"
    );
    let names: Vec<String> = declared_dependencies(BINDINGS)
        .into_iter()
        .map(|dependency| dependency.extern_name)
        .collect();
    assert!(
        names.iter().any(|name| name == "licoarc"),
        "the mirror must carry the renamed SDK dependency: {names:?}"
    );
    assert!(
        names.iter().any(|name| name == "serde_json"),
        "the mirror must carry every declared dependency: {names:?}"
    );
}

/// Non-vacuity control, end to end: the same oracle must *report* a reference
/// the subject crate can really make. `licoarc` is the SDK the bindings
/// legitimately declare, so a green run of the refusals above cannot come from a
/// mirror that silently lost the declared dependency context.
#[test]
fn the_oracle_reports_a_reference_the_subject_can_make() {
    let control = [(
        "bindings_can_use_the_sdk_control",
        BINDINGS,
        "licoarc",
        "use licoarc::artifact::VerifiedProtocolLine;\n\
         #[allow(dead_code)]\n\
         fn probe(_line: &VerifiedProtocolLine) {}\n",
    )];
    let failures = forbidden_reference_failures(&control);
    assert_eq!(
        failures.len(),
        1,
        "the control must be reported, not silently accepted: {failures:?}"
    );
    assert!(
        failures[0].contains("compiled a reference"),
        "the control must be reported as a declared edge: {failures:?}"
    );
}

/// The same forbidden source with the missing edge added to the probe's own
/// manifest. Each control must compile, so a refusal above is caused by the
/// missing dependency and never by the probe's own shape.
///
/// The adapter control is deliberately absent: adding `licoup-native` would make
/// this contract test build an unrelated crate that is still under active
/// change, and the controls below already prove the mechanism.
#[test]
fn the_same_reference_compiles_once_the_edge_exists() {
    let sdk = declared_dependencies(BINDINGS)
        .into_iter()
        .find(|dependency| dependency.extern_name == "licoarc")
        .expect("the bindings declare the fixed SDK");

    let mut controls = Vec::new();
    let mut core_with_sdk = declared_dependencies(CORE);
    core_with_sdk.push(sdk);
    controls.push((
        "core_with_the_sdk_edge",
        core_with_sdk,
        FORBIDDEN_REFERENCES[0].3,
    ));
    let mut core_with_bindings = declared_dependencies(CORE);
    core_with_bindings.push(path_dependency(BINDINGS));
    controls.push((
        "core_with_the_bindings_edge",
        core_with_bindings,
        FORBIDDEN_REFERENCES[1].3,
    ));

    let mut failures = Vec::new();
    for (label, dependencies, source) in controls {
        let (succeeded, stderr) = compile_probe(label, &dependencies, source);
        if !succeeded {
            failures.push(format!(
                "{label}: the same reference must compile once the edge exists: {stderr}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A legal reference compiled through the same mechanism. The bindings declare
/// the fixed SDK, so a probe with their declared context names it and compiles;
/// the core declares nothing, so a probe with its context still compiles when it
/// names only itself.
#[test]
fn the_legal_direction_compiles_in_the_same_context() {
    let (succeeded, stderr) = compile_probe(
        "bindings_can_use_the_sdk",
        &declared_dependencies(BINDINGS),
        "use licoarc::artifact::VerifiedProtocolLine;\n\
         #[allow(dead_code)]\n\
         fn probe(_line: &VerifiedProtocolLine) {}\n",
    );
    assert!(
        succeeded,
        "the SDK edge {BINDINGS} declares must compile: {stderr}"
    );

    let (succeeded, stderr) = compile_probe(
        "core_context_compiles",
        &declared_dependencies(CORE),
        "#[allow(dead_code)]\nfn probe() -> u8 { 0 }\n",
    );
    assert!(
        succeeded,
        "the core's declared context must still compile a plain probe: {stderr}"
    );
}

/// The legal direction has to actually work, or the ports are decoration.
const LEGAL_IMPLEMENTATION: &str = "\
use licoup_endpoint_core::{CustodyHandle, KeyCustody, PortFailure};\n\
\n\
/// One caller-owned custody backend, written outside the crate that declares\n\
/// the trait: this is what a platform adapter must be able to do.\n\
struct NoKeyMaterial;\n\
\n\
impl KeyCustody for NoKeyMaterial {\n\
    fn ed25519_public(&self, _handle: CustodyHandle) -> Result<[u8; 32], PortFailure> {\n\
        Err(PortFailure::Unavailable)\n\
    }\n\
    fn ed25519_sign(&self, _h: CustodyHandle, _m: &[u8]) -> Result<[u8; 64], PortFailure> {\n\
        Err(PortFailure::Unavailable)\n\
    }\n\
    fn ml_dsa_65_public(&self, _h: CustodyHandle) -> Result<Vec<u8>, PortFailure> {\n\
        Err(PortFailure::Unavailable)\n\
    }\n\
    fn ml_dsa_65_sign(&self, _h: CustodyHandle, _m: &[u8]) -> Result<Vec<u8>, PortFailure> {\n\
        Err(PortFailure::Unavailable)\n\
    }\n\
    fn x25519_public(&self, _h: CustodyHandle) -> Result<[u8; 32], PortFailure> {\n\
        Err(PortFailure::Unavailable)\n\
    }\n\
    fn x25519(&self, _h: CustodyHandle, _p: &[u8; 32]) -> Result<[u8; 32], PortFailure> {\n\
        Err(PortFailure::Unavailable)\n\
    }\n\
    fn ml_kem_768_public(&self, _h: CustodyHandle) -> Result<Vec<u8>, PortFailure> {\n\
        Err(PortFailure::Unavailable)\n\
    }\n\
    fn ml_kem_768_encapsulate(\n\
        &self,\n\
        _p: &[u8],\n\
        _e: CustodyHandle,\n\
    ) -> Result<(Vec<u8>, [u8; 32]), PortFailure> {\n\
        Err(PortFailure::Unavailable)\n\
    }\n\
    fn ml_kem_768_decapsulate(\n\
        &self,\n\
        _h: CustodyHandle,\n\
        _c: &[u8],\n\
    ) -> Result<[u8; 32], PortFailure> {\n\
        Err(PortFailure::Unavailable)\n\
    }\n\
    fn abort_x25519(&mut self, _staged: CustodyHandle) {}\n\
    fn abort_ml_kem_768(&mut self, _staged: CustodyHandle) {}\n\
}\n\
\n\
#[allow(dead_code)]\n\
fn probe<K: KeyCustody>(custody: &K) {\n\
    let _ = custody;\n\
}\n\
\n\
#[allow(dead_code)]\n\
fn installed() {\n\
    probe(&NoKeyMaterial);\n\
}\n";

#[test]
fn a_consumer_owned_port_is_implementable_from_outside_the_core() {
    let (succeeded, stderr) = compile_probe(
        "port_implementor",
        &[path_dependency(CORE)],
        LEGAL_IMPLEMENTATION,
    );
    assert!(
        succeeded,
        "a caller outside {CORE} must be able to implement its port; stderr: {stderr}"
    );
}
