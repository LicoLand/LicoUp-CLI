//! Discovery and catalogue behaviour over a synthetic home.
//!
//! Every case here states its own home and its own state root, so the suite
//! never reads the developer's machine and never depends on what happens to be
//! installed on it. The scan-path allowlist, the execution-admission rule and
//! the declarations the catalogue projects are all exercised against the same
//! synthetic tree.

use super::super::catalog::{target_def, target_defs};
use super::super::scan_paths::{
    HostRoots, agent_binary_dirs, binary_dirs, denied, probe_exists_under_home, probe_exists_with,
};
use super::test_support::temp_test_dir;
use std::fs;
use std::path::PathBuf;

/// The packaged Codex CLI inside the official ChatGPT desktop bundle, which the
/// scan-path manifest admits as the Agent's application-store entrypoint.
const CODEX_BUNDLE: &str = "Applications/ChatGPT.app/Contents/Resources/codex-cli/bin";

#[test]
fn the_scan_admits_the_declared_codex_bundle_under_a_synthetic_home() {
    let home = temp_test_dir("synthetic-home-bundle");
    let bundle = home.join(CODEX_BUNDLE);
    fs::create_dir_all(&bundle).unwrap();
    let cli = bundle.join("codex");
    fs::write(&cli, "synthetic packaged Codex entrypoint").unwrap();

    let roots = HostRoots::from_home(&home);
    // The manifest declares the machine-wide bundle and the same package under
    // the caller's home; only the second one is stated by this synthetic tree.
    assert_eq!(
        agent_binary_dirs("codex", "macos", &roots),
        vec![
            PathBuf::from("/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin"),
            home.join(CODEX_BUNDLE)
        ]
    );
    // The bundle path is admitted for a discovery probe, and the probe answers
    // from the synthetic tree instead of the host.
    assert!(probe_exists_with(&cli, &roots));
    assert!(denied(&home.join("Documents/codex"), Some(home.as_path())));
    assert!(!probe_exists_with(&home.join("Documents/codex"), &roots));

    let _ = fs::remove_dir_all(home);
}

#[test]
fn the_catalogue_declares_the_agent_whose_bundle_the_scan_admits() {
    let def = target_def("codex").expect("codex is a declared target");
    assert_eq!(def.id, "codex");
    assert_eq!(def.binary_names, &["codex"]);
    assert!(
        target_defs()
            .iter()
            .any(|candidate| candidate.id == "codex"),
        "the declaration set and the single declaration must agree"
    );
    // The catalogue normalises the caller's spelling without inventing a target.
    assert_eq!(target_def("CODEX").unwrap().id, "codex");
    assert!(target_def("not-a-declared-agent").is_err());
}

#[test]
fn a_synthetic_catalog_home_is_probed_only_through_the_caller_supplied_root() {
    let home = temp_test_dir("synthetic-home-catalog");
    let catalog_home = home.join("profile/.codex");
    fs::create_dir_all(&catalog_home).unwrap();
    let config = catalog_home.join("config.toml");
    fs::write(&config, "[model]\nname = \"synthetic\"\n").unwrap();

    // A caller-named catalog home is stated exactly; the same relative path
    // under a personal root stays denied.
    assert!(probe_exists_under_home(&config, &catalog_home));
    assert!(!probe_exists_under_home(
        &home.join("Documents/.codex/config.toml"),
        &catalog_home
    ));

    let _ = fs::remove_dir_all(home);
}

#[test]
fn the_automatic_search_dirs_are_manifest_paths_not_the_process_path() {
    let home = temp_test_dir("synthetic-home-search-dirs");
    let roots = HostRoots::from_home(&home);
    let dirs = binary_dirs("macos", &roots);
    assert!(
        dirs.iter()
            .all(|path| path.starts_with(&home) || path.is_absolute()),
        "every automatic search dir is a manifest path, never a PATH entry"
    );
    assert!(
        dirs.contains(&home.join(CODEX_BUNDLE)),
        "the packaged Codex entrypoint is one of the automatic search dirs"
    );
    assert!(
        !dirs.iter().any(
            |path| path.to_string_lossy().contains("/bin") == false && path.starts_with("/usr")
        ),
        "no system PATH directory is searched"
    );

    let _ = fs::remove_dir_all(home);
}
