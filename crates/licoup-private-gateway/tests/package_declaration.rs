//! What the shipped Gateway package declares about itself, read from the
//! committed release source through the host's own manifest contract.
//!
//! The manifest is a claim, and the claims that matter here are checked against
//! each other: the entry the package says it carries, the data footprint it
//! declares in its own namespaced attributes, and the client line that admits
//! it. The reader is `licoup_extension_contracts::manifest::PackageManifest` —
//! the same reader the installer admits an installed generation through — so a
//! declaration this file accepts is one the host accepts, and a package that
//! stopped declaring its footprint fails here rather than going unnoticed.
//!
//! Nothing in this file runs a package or reaches the network.

use std::fs;
use std::path::{Path, PathBuf};

use licoup_extension_contracts::manifest::{PackageManifest, Runtime};
use serde_json::Value;

const PACKAGE_ID: &str = "org.licoland.feature.gateway";
const RUNTIME_ENTRY: &str = "bin/lico-gateway";
/// The client line this package declares, and one outside it in each direction.
const COVERED_CLIENT: &str = "0.3.0";
const OLDER_CLIENT: &str = "0.2.9";
const NEWER_CLIENT: &str = "1.0.0";

fn package_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("package")
}

fn package_document(name: &str) -> Value {
    let path = package_root().join(name);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{name} is json: {error}"))
}

/// The committed manifest as the host's own contract reads it.
fn manifest() -> PackageManifest {
    PackageManifest::from_value(package_document("manifest.json"))
        .expect("the committed manifest is one the host reads")
}

/// The declaration this package makes in its own namespace.
fn declared(manifest: &PackageManifest, attribute: &str) -> Option<String> {
    manifest
        .extensions
        .get(&format!("{PACKAGE_ID}/{attribute}"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

#[test]
fn the_package_declares_the_runtime_entry_it_carries() {
    let manifest = manifest();
    // The host reads the entry through its typed declaration, not through a
    // string this test happens to look for.
    let Runtime::Process { entry, .. } = &manifest.runtime else {
        panic!(
            "the Gateway is a process the host starts: {:?}",
            manifest.runtime
        );
    };
    assert_eq!(entry, RUNTIME_ENTRY);
    assert_eq!(manifest.id, PACKAGE_ID);
    // The profile the package offers is the capability the gateway answers, and
    // an unpublished id is still a readable declaration the host keeps.
    let profile = manifest
        .profiles
        .iter()
        .find(|declaration| declaration.id == "model-gateway")
        .expect("the package offers the profile it implements");
    assert!(
        profile
            .capabilities
            .iter()
            .any(|value| value == "model-gateway.v1")
    );
    let entry = package_root().join(RUNTIME_ENTRY);
    assert!(
        entry.is_file(),
        "the declared entry is a file of the payload: {}",
        entry.display()
    );
    // The authenticated release declaration names the same entry, which is the
    // fact the packaging tool compares before it builds a payload.
    assert_eq!(
        package_document("package-release.json")["converter"]["entry"].as_str(),
        Some(RUNTIME_ENTRY)
    );
    // The declaration is admitted by the host for the client line it names, and
    // refused outside it: a manifest the host would reject cannot be shipped.
    manifest
        .admit_client(COVERED_CLIENT)
        .expect("the declared line covers a client this package supports");
    assert!(manifest.admit_client(OLDER_CLIENT).is_err());
    assert!(manifest.admit_client(NEWER_CLIENT).is_err());
}

#[test]
fn the_package_declares_the_persisted_data_it_owns_and_the_reason() {
    let manifest = manifest();
    // The host validates every namespaced attribute of an admitted manifest, so
    // parsing it above is what makes these keys readable declarations rather
    // than free-form JSON.
    manifest
        .validate()
        .expect("the committed manifest is valid for the host");
    // The Gateway owns a persisted format of its own: the usage journal it
    // appends to and the configuration it is started with are its own bytes,
    // held in the client's data home rather than inside the installed package.
    // It converts no *other* format, so it declares no conversion.
    assert!(
        manifest.conversion.is_none(),
        "this package converts no format it does not own, so it declares no conversion"
    );
    assert_eq!(
        declared(&manifest, "persistentData").as_deref(),
        Some("self-owned"),
        "the package states that the persisted data is its own"
    );
    let reason = declared(&manifest, "persistentDataReason")
        .expect("the package states why it owns that data");
    assert!(
        reason.contains("data home") && reason.contains("usage journal"),
        "the stated reason names where the Gateway's own bytes live: {reason}"
    );
}
