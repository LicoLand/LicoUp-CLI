//! The mobile entry's dependency-closure fixture.
//!
//! The acceptance this fixture exists for is a statement about *shape*: the
//! mobile entry must have only the required pairing/group/settings closure,
//! while the desktop client keeps its own independent executor. A passing
//! compile cannot show that, because a mobile build that happened to link the
//! whole desktop root would also compile.
//!
//! So this fixture walks the workspace manifests themselves. It resolves every
//! in-workspace dependency edge of every member (ordinary, target-specific and
//! build dependencies; a shipped closure never includes a dev-dependency),
//! computes what `licoup-mobile-core` can reach, and requires that set to be
//! *exactly* the allowed closure. Adding one desktop crate to the mobile core
//! fails here, and so does renaming a desktop crate out of the way without
//! answering the question.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// The complete set of workspace crates a mobile build may link.
///
/// Each entry is here because the mobile entry actually uses it:
///
/// * `licoup-mobile-core` — this crate;
/// * `licoup-endpoint-core` — the consumer-owned session, custody and
///   transport port types;
/// * `licoup-protocol-bindings` — the accepted protocol version and the relay
///   envelope identity durable delivery records;
/// * `licoup-conversation` — the Canonical Conversation authority, and the
///   only place a group lifecycle exists;
/// * `licoup-client-state` — the bounded resource policy and the privately
///   written client state root;
/// * `licoup-foundation` — the portable data root and private-file rules
///   underneath both of those;
/// * `licoup-platform-bridges` — the ABI identity this entry registers;
/// * `licoup-state-machine-codegen` — the build-time generator
///   `licoup-conversation` compiles its declarative state machines with.
const ALLOWED_CLOSURE: &[&str] = &[
    "licoup-mobile-core",
    "licoup-endpoint-core",
    "licoup-protocol-bindings",
    "licoup-conversation",
    "licoup-client-state",
    "licoup-foundation",
    "licoup-platform-bridges",
    "licoup-state-machine-codegen",
];

/// Desktop-only owners the mobile build must never reach. They are asserted by
/// name as well as by closure arithmetic, so a rename cannot make the fixture
/// pass by making the crate disappear.
const MUST_STAY_DESKTOP_ONLY: &[&str] = &[
    "licoup-native",
    "licoup-application",
    "licoup-agent-drivers",
    "licoup-agent-adapters",
    "licoup-agent-adapter-sdk",
    "licoup-agent-runtime",
    "licoup-agent-targets",
    "licoup-workflow",
    "licoup-private-gateway",
    "licoup-public-gateway",
    "licoup-vault",
    "licoup-migrate",
    "licoup-project",
    "licoup-extension-contracts",
    "licoup-model-catalog",
];

/// One workspace member: its package name and the directory holding its
/// manifest.
struct Member {
    directory: PathBuf,
    manifest: toml::Value,
}

impl Member {
    /// In-workspace dependency edges, as package names.
    ///
    /// `include_dev` exists so the fixture can show separately that the mobile
    /// core takes no workspace crate through a dev-dependency either: a
    /// dev-dependency of this crate is compiled into this crate's test build,
    /// so a workspace edge there would still contradict "only the required
    /// closure".
    fn internal_edges(
        &self,
        members: &BTreeMap<PathBuf, String>,
        include_dev: bool,
    ) -> BTreeSet<String> {
        let mut edges = BTreeSet::new();
        let Some(table) = self.manifest.as_table() else {
            return edges;
        };
        for key in ["dependencies", "build-dependencies"] {
            collect_edges(table.get(key), &self.directory, members, &mut edges);
        }
        if include_dev {
            collect_edges(
                table.get("dev-dependencies"),
                &self.directory,
                members,
                &mut edges,
            );
        }
        if let Some(targets) = table.get("target").and_then(toml::Value::as_table) {
            for target in targets.values() {
                let Some(target) = target.as_table() else {
                    continue;
                };
                for key in ["dependencies", "build-dependencies"] {
                    collect_edges(target.get(key), &self.directory, members, &mut edges);
                }
                if include_dev {
                    collect_edges(
                        target.get("dev-dependencies"),
                        &self.directory,
                        members,
                        &mut edges,
                    );
                }
            }
        }
        edges
    }
}

fn collect_edges(
    table: Option<&toml::Value>,
    directory: &Path,
    members: &BTreeMap<PathBuf, String>,
    edges: &mut BTreeSet<String>,
) {
    let Some(table) = table.and_then(toml::Value::as_table) else {
        return;
    };
    for (name, spec) in table {
        let Some(spec) = spec.as_table() else {
            continue;
        };
        let Some(path) = spec.get("path").and_then(toml::Value::as_str) else {
            continue;
        };
        let resolved = normalize(&directory.join(path));
        match members.get(&resolved) {
            Some(package) => {
                edges.insert(package.clone());
            }
            None => panic!(
                "manifest at {} declares a path dependency on {name} at {}, which is not a \
                 workspace member; the closure fixture cannot reason about it",
                directory.display(),
                resolved.display()
            ),
        }
    }
}

/// Canonical absolute form, with `.` and `..` resolved and symlinks followed,
/// so two spellings of one directory compare equal.
fn normalize(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|error| {
        panic!(
            "the closure fixture could not resolve {}: {error}",
            path.display()
        )
    })
}

fn workspace_root() -> PathBuf {
    normalize(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
}

fn members(root: &Path) -> BTreeMap<PathBuf, String> {
    let root_manifest: toml::Value = fs::read_to_string(root.join("Cargo.toml"))
        .expect("the workspace manifest is readable")
        .parse()
        .expect("the workspace manifest is valid TOML");
    let listed = root_manifest
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(toml::Value::as_array)
        .expect("the workspace manifest declares members");
    let mut members = BTreeMap::new();
    for entry in listed {
        let relative = entry
            .as_str()
            .expect("every workspace member is a path string");
        let directory = normalize(&root.join(relative));
        let manifest: toml::Value = fs::read_to_string(directory.join("Cargo.toml"))
            .unwrap_or_else(|error| {
                panic!("the manifest for member {relative} is readable: {error}")
            })
            .parse()
            .unwrap_or_else(|error| {
                panic!("the manifest for member {relative} is valid TOML: {error}")
            });
        let name = manifest
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(toml::Value::as_str)
            .unwrap_or_else(|| panic!("member {relative} declares a package name"))
            .to_owned();
        members.insert(directory, name);
    }
    members
}

fn reachable_closure(
    members: &BTreeMap<PathBuf, String>,
    manifests: &BTreeMap<String, toml::Value>,
    directories: &BTreeMap<String, PathBuf>,
) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    let mut frontier = vec!["licoup-mobile-core".to_owned()];
    while let Some(package) = frontier.pop() {
        if !seen.insert(package.clone()) {
            continue;
        }
        let member = Member {
            directory: directories
                .get(&package)
                .unwrap_or_else(|| panic!("{package} is a workspace member"))
                .clone(),
            manifest: manifests
                .get(&package)
                .unwrap_or_else(|| panic!("{package} has a manifest"))
                .clone(),
        };
        for edge in member.internal_edges(members, false) {
            if !seen.contains(&edge) {
                frontier.push(edge);
            }
        }
    }
    seen
}

#[test]
fn the_mobile_entry_reaches_only_its_required_closure() {
    let root = workspace_root();
    let member_directories = members(&root);
    let manifests: BTreeMap<String, toml::Value> = member_directories
        .iter()
        .map(|(directory, name)| {
            let manifest: toml::Value = fs::read_to_string(directory.join("Cargo.toml"))
                .expect("a member manifest is readable")
                .parse()
                .expect("a member manifest is valid TOML");
            (name.clone(), manifest)
        })
        .collect();
    let directories: BTreeMap<String, PathBuf> = member_directories
        .iter()
        .map(|(directory, name)| (name.clone(), directory.clone()))
        .collect();

    for crate_name in MUST_STAY_DESKTOP_ONLY {
        assert!(
            directories.contains_key(*crate_name),
            "{crate_name} is named as a desktop-only owner but is no longer a workspace member; \
             update this fixture rather than letting the guarantee lapse"
        );
    }

    let closure = reachable_closure(&member_directories, &manifests, &directories);
    let allowed: BTreeSet<String> = ALLOWED_CLOSURE
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    assert_eq!(
        closure, allowed,
        "the mobile entry's workspace closure changed; every crate here is linked into the \
         mobile build"
    );
}

/// The mobile core's direct dependencies, workspace and registry alike.
///
/// The closure test above is about workspace crates. This list is about the
/// other half of the same question: a desktop-only implementation crate that
/// arrives as a registry dependency — a process supervisor, a messaging
/// layer, a SQL stack for a local Agent — would link into the mobile build
/// without ever appearing as a workspace edge.
const ALLOWED_DIRECT_DEPENDENCIES: &[&str] = &[
    "licoup-endpoint-core",
    "licoup-protocol-bindings",
    "licoup-conversation",
    "licoup-client-state",
    "licoup-foundation",
    "licoup-platform-bridges",
    "anyhow",
    "serde",
    "serde_json",
];

/// The only dev-dependency this crate may take. It reads manifests; it links
/// nothing.
const ALLOWED_DEV_DEPENDENCIES: &[&str] = &["toml"];

#[test]
fn the_mobile_core_takes_no_workspace_crate_through_a_dev_dependency() {
    let manifest = own_manifest();
    let dev_only: Vec<String> = declared_dependencies(&manifest, "dev-dependencies")
        .difference(&declared_dependencies(&manifest, "dependencies"))
        .cloned()
        .collect();
    let allowed: BTreeSet<String> = ALLOWED_DEV_DEPENDENCIES
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    assert!(
        dev_only.iter().all(|name| allowed.contains(name)),
        "the mobile core declares a dev-dependency outside the allowed set: {dev_only:?}"
    );
}

#[test]
fn the_mobile_core_declares_exactly_its_allowed_dependencies() {
    let manifest = own_manifest();
    let declared = declared_dependencies(&manifest, "dependencies");
    let allowed: BTreeSet<String> = ALLOWED_DIRECT_DEPENDENCIES
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    assert_eq!(
        declared, allowed,
        "the mobile core's direct dependency set changed; every entry links into the mobile build"
    );
    assert!(
        manifest
            .get("target")
            .and_then(toml::Value::as_table)
            .is_none_or(|targets| targets.is_empty()),
        "the mobile core declares a target-specific dependency outside the audited set"
    );
    assert!(
        manifest
            .get("build-dependencies")
            .and_then(toml::Value::as_table)
            .is_none_or(|build| build.is_empty()),
        "the mobile core declares a build dependency outside the audited set"
    );
}

fn own_manifest() -> toml::Value {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .expect("this crate's manifest is readable")
        .parse()
        .expect("this crate's manifest is valid TOML")
}

fn declared_dependencies(manifest: &toml::Value, table: &str) -> BTreeSet<String> {
    manifest
        .get(table)
        .and_then(toml::Value::as_table)
        .map(|dependencies| dependencies.keys().cloned().collect())
        .unwrap_or_default()
}

fn source_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = fs::read_dir(&directory)
            .unwrap_or_else(|error| panic!("{} is readable: {error}", directory.display()));
        for entry in entries {
            let path = entry.expect("a directory entry is readable").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }
    files
}

/// Every `.rs` file under the crate, so a source-level audit has one reader.
#[test]
fn the_mobile_core_source_tree_is_present() {
    assert!(
        !source_files(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src")).is_empty(),
        "the fixture found no source to check"
    );
}
