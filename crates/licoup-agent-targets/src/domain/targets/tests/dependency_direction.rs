//! The inventory is a capability slice and must stay below the host that
//! composes it.
//!
//! `licoup-native` depends on this crate and answers its port; a dependency in
//! the other direction would be a cycle, and a dependency on any other host
//! crate would put an owner above the inventory. The manifests are the
//! contract, so they are asserted directly rather than inferred from a build
//! that happens to link.

const MANIFEST: &str = include_str!("../../../../Cargo.toml");
const HOST_MANIFEST: &str = include_str!("../../../../../licoup-native/Cargo.toml");

/// Every LicoUp crate this inventory is allowed to reach downward.
const DOWNWARD_DEPENDENCIES: &[&str] = &[
    "licoup-foundation",
    "licoup-client-state",
    "licoup-agent-deepseek",
];

fn path_dependency_names(manifest: &str) -> Vec<String> {
    manifest
        .lines()
        .filter_map(|line| {
            let name = line.split('=').next()?.trim();
            let value = line.split_once('=')?.1;
            if !value.contains("path =") || name.starts_with('#') {
                return None;
            }
            Some(name.trim_end_matches(".workspace").to_string())
        })
        .collect()
}

#[test]
fn the_inventory_depends_on_no_host_crate() {
    assert!(
        !MANIFEST.contains("licoup-native"),
        "the Agent inventory must not depend on the host that composes it"
    );
    for name in path_dependency_names(MANIFEST) {
        assert!(
            DOWNWARD_DEPENDENCIES.contains(&name.as_str()),
            "{name} is not a declared downward dependency of the Agent inventory"
        );
    }
}

#[test]
fn the_host_composes_the_inventory_and_answers_its_port() {
    assert!(
        HOST_MANIFEST.contains("licoup-agent-targets"),
        "the host must depend on the inventory it composes"
    );
    assert!(
        HOST_MANIFEST.contains(r#"features = ["test-support"]"#),
        "the host's test build states the packaged fixtures it expects"
    );
}
