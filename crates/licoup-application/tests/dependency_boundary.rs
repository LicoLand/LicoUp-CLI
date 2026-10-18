//! Dependency-boundary proof for the shared business crate.
//!
//! The crate's whole promise is that it is protocol-neutral: a CLI and an MCP
//! can both depend on it, and it depends on neither of them nor on any concrete
//! business implementation. Publishing that as a test means a future edit
//! cannot quietly reintroduce the coupling this crate exists to remove.
//!
//! The check reads the crate's own manifest rather than the resolved dependency
//! graph, so it fails on the edit that adds the dependency — not later, in a
//! build that happens to notice.

use std::path::{Path, PathBuf};

/// Crates this one must never depend on. `licoup-native` owns business
/// implementations and platform access; the others are concrete business crates
/// whose internals a protocol-neutral layer cannot know about.
const FORBIDDEN: &[&str] = &[
    "licoup-native",
    "licoup-client-state",
    "licoup-endpoint-core",
    "licoup-platform-bridges",
    "licoup-protocol-bindings",
    "lico-catalog-convergence",
];

/// The only dependencies allowed, by section. Serialization is the one thing a
/// command envelope genuinely needs.
const ALLOWED_DEPENDENCIES: &[&str] = &["serde", "serde_json"];

fn manifest_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")
}

/// Dependency names declared in one section of the manifest.
///
/// A hand-rolled reader keeps this test dependency-free: adding `toml` here to
/// parse the manifest would itself be a dependency change to police.
fn declared_dependencies(source: &str) -> Vec<(String, String)> {
    let mut section = String::new();
    let mut found = Vec::new();
    for line in source.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            section = line.trim_matches(['[', ']']).to_owned();
            continue;
        }
        if section != "dependencies" && section != "dev-dependencies" {
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, _)) = line.split_once('=') else {
            continue;
        };
        // A workspace dependency is written `serde.workspace = true`, so the
        // crate name is the part before the first dot.
        let name = name.trim().split('.').next().unwrap_or_default().trim();
        if name.is_empty() {
            continue;
        }
        found.push((section.clone(), name.to_owned()));
    }
    found
}

#[test]
fn the_shared_crate_never_depends_on_native_or_concrete_business_crates() {
    let source = std::fs::read_to_string(manifest_path()).expect("manifest is readable");
    let declared = declared_dependencies(&source);
    assert!(
        !declared.is_empty(),
        "the manifest reader found no dependencies at all, so it is not proving anything"
    );
    for (section, name) in &declared {
        assert!(
            !FORBIDDEN.contains(&name.as_str()),
            "{section} declares {name}, which the protocol-neutral layer must not know about"
        );
    }
}

#[test]
fn the_shared_crate_stays_on_serialization_only() {
    let source = std::fs::read_to_string(manifest_path()).expect("manifest is readable");
    for (section, name) in declared_dependencies(&source) {
        assert!(
            ALLOWED_DEPENDENCIES.contains(&name.as_str()),
            "{section} adds {name}; a new dependency here changes what both interfaces compile \
             against, so it needs a deliberate decision rather than an incidental edit"
        );
    }
}

#[test]
fn the_crate_is_registered_as_a_workspace_member() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("Cargo.toml");
    let source = std::fs::read_to_string(workspace).expect("workspace manifest is readable");
    assert!(
        source.contains("\"crates/licoup-application\""),
        "the crate must stay a workspace member or its boundary is not enforced by the build"
    );
}
