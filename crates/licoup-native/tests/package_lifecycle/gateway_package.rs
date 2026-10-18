#![cfg(unix)]
//! GATEWAY-PACKAGE-LIFECYCLE: the Gateway login item and the package that owns
//! it, driven through the real store and the real registration owner.
//!
//! What is real here: the committed `crates/licoup-private-gateway/package` release
//! source (its manifest, its release declaration and the native entry it
//! declares), the production `PackageStore` with a real managed root, the
//! production login-item owner (`llm_gateway_autostart`), and a real gateway
//! state directory holding the configuration and usage files the sidecar owns.
//!
//! What is synthetic, stated plainly: the login item is written over a
//! disposable home directory through `LoginItemHost::synthetic`, so nothing
//! touches the developer's own login items and no `launchctl`/`systemctl`
//! registration runs. The packaged entry is the staged release source rather
//! than a compiled binary; the lifecycle measures it and never executes it.
//!
//! What this does not cover: starting and stopping the sidecar process. That
//! belongs to the service owner (`llm_gateway_service`), whose start/stop
//! behaviour this change does not alter, and this harness spawns no gateway
//! process, so nothing here claims the sidecar was run.
//!
//! What this proves, in one place: the login item exists only while the package
//! is installed, activation measures the entry the package's own manifest
//! declares before it registers anything, and uninstall removes the
//! registration while the configuration and usage data stay.

use std::path::{Path, PathBuf};

use licoup_extension_contracts::manifest::PermissionRequest;
use licoup_native::platform::extension_packages::{PackageStore, TrustRecord};
use licoup_native::platform::llm_gateway_autostart::{
    GATEWAY_PACKAGE_ID, GatewayPackageBinding, LoginItemHost, autostart_status_at,
};

use super::{archive, cleanup, content_digest_of, covering_client_versions, root};

/// The committed release source directory the packaging tool packages.
const PACKAGE_SOURCE: &str = "crates/licoup-private-gateway/package";

fn gateway_source_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(PACKAGE_SOURCE)
}

/// Every file under the package source directory, as package-relative paths.
fn source_files(directory: &Path, prefix: &str, found: &mut Vec<String>) {
    let mut entries: Vec<_> = std::fs::read_dir(directory)
        .expect("the gateway package source exists")
        .map(|entry| entry.expect("readable directory entry"))
        .collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let relative = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let kind = entry.file_type().expect("readable file type");
        if kind.is_dir() {
            source_files(&entry.path(), &relative, found);
            continue;
        }
        assert!(kind.is_file(), "{relative} must be a regular file");
        found.push(relative);
    }
}

/// The committed package, archived and admitted the way a local import is.
///
/// The manifest's compatibility list is replaced with the running client's own
/// line before the archive is built, exactly as the neighbouring release-fixture
/// test does: the committed list names the released product version, and a
/// development binary is not that version. The committed file on disk is not
/// touched, and the packaged bytes are otherwise the release source verbatim.
fn install_gateway_package(store: &PackageStore, manifest: &serde_json::Value) -> String {
    let source = gateway_source_root();
    let mut files = Vec::new();
    source_files(&source, "", &mut files);
    let contents: Vec<(String, Vec<u8>)> = files
        .into_iter()
        .map(|relative| {
            let bytes = if relative == "manifest.json" {
                let mut admitted = manifest.clone();
                admitted["compatibility"]["clientVersions"] =
                    serde_json::json!(covering_client_versions());
                serde_json::to_vec_pretty(&admitted).expect("admitted manifest text")
            } else {
                std::fs::read(source.join(&relative)).expect("readable package asset")
            };
            (relative, bytes)
        })
        .collect();
    let borrowed: Vec<(&str, Vec<u8>)> = contents
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.clone()))
        .collect();
    let bytes = archive(&borrowed);

    let package_id = manifest["id"].as_str().expect("package id").to_owned();
    let version = manifest["version"].as_str().expect("version").to_owned();
    let permissions: Vec<PermissionRequest> = manifest["permissions"]
        .as_array()
        .expect("permissions")
        .iter()
        .map(|permission| {
            PermissionRequest::new(
                permission["capability"].as_str().expect("capability"),
                permission["scope"].as_str().expect("scope"),
            )
        })
        .collect();
    store
        .install_local_import(
            &package_id,
            &version,
            TrustRecord::local_approved(content_digest_of(&bytes), permissions).expect("trust"),
            &bytes,
        )
        .expect("offline import of the committed gateway package");
    version
}

/// The manifest the committed release source declares.
fn committed_manifest() -> serde_json::Value {
    let text = std::fs::read_to_string(gateway_source_root().join("manifest.json"))
        .expect("the committed gateway manifest");
    serde_json::from_str(&text).expect("manifest json")
}

#[test]
fn the_gateway_login_item_exists_only_while_its_package_is_installed() {
    let data_home = root("gateway-package-lifecycle");
    std::fs::create_dir_all(&data_home).expect("data home");
    let store_root = data_home.join("extension-packages");
    let store = PackageStore::open(&store_root).expect("store");
    let manifest = committed_manifest();

    // A synthetic login item over a disposable home: the production owner, no
    // registration with the platform, and nothing written outside this root.
    let login_home = data_home.join("login-home");
    let state_directory = data_home.join("llm-gateway");
    let binding = GatewayPackageBinding::over(
        LoginItemHost::synthetic(
            login_home.clone(),
            state_directory.clone(),
            gateway_source_root().join("bin/lico-gateway"),
        ),
        store_root.clone(),
    );

    // 1. Without the package there is nothing to register, and no login item.
    assert_eq!(binding.installed().expect("installed"), None);
    assert!(
        binding.activate(15_722).is_err(),
        "a package the client does not have registers no login item"
    );
    let before = binding.status().expect("status");
    assert_eq!(before["enabled"], serde_json::json!(false));
    assert_eq!(before["installed"], serde_json::json!(false));
    assert!(
        !binding
            .host()
            .definition_path()
            .expect("definition path")
            .exists(),
        "no definition file exists before the package is installed"
    );

    // 2. Install the committed package; activation registers the login item.
    //    The payload's declared entry is measured first, so a package that does
    //    not actually carry it cannot register anything.
    let version = install_gateway_package(&store, &manifest);
    assert_eq!(
        binding.installed().expect("installed"),
        Some(version.clone())
    );
    let program = binding.program().expect("the package declares a program");
    assert_eq!(
        program,
        store
            .installed_path(GATEWAY_PACKAGE_ID, &version)
            .join("bin/lico-gateway"),
        "the measured entry is the one the installed package's manifest declares"
    );

    let activated = binding.activate(15_722).expect("activation registers");
    assert_eq!(activated["enabled"], serde_json::json!(true));
    assert_eq!(activated["installed"], serde_json::json!(true));
    assert_eq!(activated["port"], serde_json::json!(15_722));
    assert_eq!(
        activated["program"],
        serde_json::json!(binding.host().program().to_string_lossy())
    );
    let definition = binding.host().definition_path().expect("definition path");
    assert!(definition.is_file(), "activation writes the definition");
    let written = std::fs::read_to_string(&definition).expect("readable definition");
    assert!(
        written.contains(binding.host().program().to_string_lossy().as_ref())
            && written.contains("--port")
            && written.contains("15722"),
        "the definition is the one launchable login item: the CLI entry point and its port"
    );

    // 3. The Gateway's state lives with the user, not with the package: the
    //    configuration it generated and the usage journal it wrote are here.
    let config = state_directory.join("config.json");
    let usage = state_directory.join("usage.json");
    std::fs::write(&config, serde_json::json!({"providers": []}).to_string())
        .expect("write configuration");
    std::fs::write(&usage, serde_json::json!({"turns": 3}).to_string()).expect("write usage");
    let config_bytes = std::fs::read(&config).expect("configuration bytes");
    let usage_bytes = std::fs::read(&usage).expect("usage bytes");

    // 4. Retire before the bytes go away: the registration is gone and the
    //    capability is reported off, exactly as an uninstall needs it.
    let retired = binding.retire().expect("retire removes the login item");
    assert_eq!(retired["enabled"], serde_json::json!(false));
    assert_eq!(retired["installed"], serde_json::json!(false));
    assert!(
        !definition.exists(),
        "uninstall removes the login item, not merely its enablement"
    );
    assert!(
        !binding
            .host()
            .state_directory()
            .join("autostart.json")
            .exists(),
        "the enablement marker goes with the definition"
    );

    // 5. The package bytes go away; the user's data does not. This is the whole
    //    reason the configuration and usage live outside the package root, and
    //    the removal is the store's own operation rather than a deleted folder.
    let removed = store
        .remove_installed(GATEWAY_PACKAGE_ID, &version)
        .expect("the installed payload is reclaimed");
    assert!(removed.reclaimed_bytes() > 0);
    assert_eq!(binding.installed().expect("installed"), None);
    assert!(
        binding.program().is_err(),
        "without the package there is no entry to register"
    );
    assert_eq!(
        std::fs::read(&config).expect("configuration survives"),
        config_bytes
    );
    assert_eq!(std::fs::read(&usage).expect("usage survives"), usage_bytes);

    // 6. Reinstalling brings the same registration back over the same data.
    let reinstalled = install_gateway_package(&store, &manifest);
    assert_eq!(reinstalled, version);
    assert_eq!(binding.program().expect("program"), program);
    let reactivated = binding.activate(15_722).expect("reinstall reactivates");
    assert_eq!(reactivated["enabled"], serde_json::json!(true));
    assert!(definition.is_file(), "reinstall restores the login item");
    assert_eq!(
        std::fs::read(&config).expect("configuration survives reinstall"),
        config_bytes
    );
    assert_eq!(
        std::fs::read(&usage).expect("usage survives reinstall"),
        usage_bytes
    );

    // 7. A retirement with no registration is a truthful report, not an error:
    //    a second uninstall cannot fail on state the first one removed.
    binding.retire().expect("retire is idempotent");
    let after = autostart_status_at(binding.host()).expect("status");
    assert_eq!(after["enabled"], serde_json::json!(false));

    cleanup(&data_home);
}
