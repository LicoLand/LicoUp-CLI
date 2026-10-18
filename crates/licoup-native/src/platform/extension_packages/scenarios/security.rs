//! A39 at component level: package-provided content never decides its own path,
//! its own permissions or its own digest, and store operations refuse an identity
//! that could walk out of the managed root.

use super::*;
use crate::platform::extension_packages::install::InstallRequest;
use crate::platform::extension_packages::{UserDataPurgeRequest, purge_user_data};
use licoup_extension_contracts::deployment::PackageSource;
use licoup_extension_contracts::manifest::PermissionRequest;

#[test]
fn an_archive_that_escapes_its_root_is_refused_and_writes_nothing_outside() {
    let (root, store) = store("traversal");
    let bytes = package_bytes(
        ECHO,
        "1.0.0",
        None,
        &[(NET, "self")],
        &[("../escape.txt", b"escaped".as_slice())],
    );
    let failure = store
        .install_local_import(ECHO, "1.0.0", trust_for(&bytes, [net_permission()]), &bytes)
        .expect_err("an archive entry may not leave the staging directory");
    assert!(
        failure.code.starts_with("package_artifact"),
        "refused as an artifact problem: {}",
        failure.code
    );
    assert!(!store.installed_path(ECHO, "1.0.0").exists());
    assert!(store.installed().expect("installed").is_empty());
    assert!(
        !root.join("escape.txt").exists() && !root.join("staging/escape.txt").exists(),
        "nothing was written outside the staging content directory"
    );
    assert!(store.staged_directories().expect("staged").is_empty());

    // The identity inside the archive is the archive's claim, not the request's.
    let other = package_bytes(
        "example.specialist.other",
        "1.0.0",
        None,
        &[(NET, "self")],
        &[],
    );
    let failure = store
        .install_local_import(ECHO, "1.0.0", trust_for(&other, [net_permission()]), &other)
        .expect_err("a manifest that describes another package is refused");
    assert_eq!(failure.code, "package_manifest_mismatch");
    assert!(store.installed().expect("installed").is_empty());
    cleanup(&root);
}

#[test]
fn store_operations_refuse_an_identity_that_could_escape_the_root() {
    let (root, store) = store("identity");
    std::fs::write(root.join("sentinel"), b"keep").expect("sentinel");
    let user_data_root = sandbox("identity-userdata");
    std::fs::create_dir_all(user_data_root.join(ECHO).join("history")).expect("user data");
    std::fs::write(
        user_data_root.join(ECHO).join("history").join("turns"),
        b"1",
    )
    .expect("history");

    for failure in [
        store
            .remove_installed("..", "1.0.0")
            .expect_err("parent id"),
        store
            .remove_installed(ECHO, "../../x")
            .expect_err("parent version"),
        store.installed_bytes("..", "1.0.0").expect_err("parent id"),
        store
            .installed_version("..", "1.0.0")
            .expect_err("parent id"),
        purge_user_data(
            &user_data_root,
            &UserDataPurgeRequest {
                package_id: "..".to_owned(),
                history: true,
                credentials: true,
                protocol_state: true,
            },
            false,
        )
        .expect_err("parent id"),
    ] {
        assert_eq!(failure.code, "package_identity_invalid");
    }
    assert!(
        root.join("sentinel").exists(),
        "nothing outside was touched"
    );
    assert!(root.exists());
    assert!(
        user_data_root
            .join(ECHO)
            .join("history")
            .join("turns")
            .exists()
    );

    cleanup(&user_data_root);
    cleanup(&root);
}

#[test]
fn a_grown_permission_scope_needs_a_new_decision() {
    let (root, store) = store("scope");
    install_local(&store, ECHO, "1.0.0", None);

    let bytes = package_bytes(ECHO, "1.1.0", None, &[(NET, "self"), (FS, "/tmp")], &[]);
    // The old decision covered the network permission only; the new manifest
    // asks for more, and matching content does not widen a decision.
    let narrow = trust_for(&bytes, [net_permission()]);
    let request = InstallRequest::new(ECHO, "1.1.0", PackageSource::LocalImport, narrow);
    let failure = store
        .install(&request, &bytes)
        .expect_err("a grown scope needs a new decision");
    assert_eq!(failure.code, "package_permission_scope_expanded");
    assert!(store.installed_path(ECHO, "1.0.0").exists());
    assert!(!store.installed_path(ECHO, "1.1.0").exists());
    assert!(store.staged_directories().expect("staged").is_empty());

    let widened = trust_for(
        &bytes,
        [net_permission(), PermissionRequest::new(FS, "/tmp")],
    );
    let request = InstallRequest::new(ECHO, "1.1.0", PackageSource::LocalImport, widened);
    store.install(&request, &bytes).expect("approved scope");
    assert_eq!(store.installed().expect("installed").len(), 2);
    cleanup(&root);
}

#[test]
fn install_scripts_are_recorded_and_never_executed() {
    let (root, store) = store("scripts");
    let sentinel = root.join("ran-script.txt");
    let script = format!("#!/bin/sh\necho ran > {}\n", sentinel.display());
    let bytes = package_bytes(
        SCRIPTED,
        "1.0.0",
        None,
        &[(NET, "self")],
        &[("postinstall.sh", script.as_bytes())],
    );
    let outcome = store
        .install_local_import(
            SCRIPTED,
            "1.0.0",
            trust_for(&bytes, [net_permission()]),
            &bytes,
        )
        .expect("install");
    assert_eq!(outcome.processes_spawned, 0, "nothing is executed");
    assert_eq!(outcome.install_scripts, vec!["postinstall.sh".to_owned()]);
    assert!(
        !sentinel.exists(),
        "the install script was reported, not run"
    );
    let record = store
        .installed_version(SCRIPTED, "1.0.0")
        .expect("record")
        .expect("installed");
    assert_eq!(record.install_scripts, vec!["postinstall.sh".to_owned()]);
    cleanup(&root);
}
