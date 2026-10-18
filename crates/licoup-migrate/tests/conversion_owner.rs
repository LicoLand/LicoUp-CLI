//! The standalone tool identifies a conversion owner by reading the package.
//!
//! These suites hold the tool to the contract it consumes: the required pair comes
//! from the client's embedded frontier catalogue, the answer comes from the
//! package's own manifest, and a package that does not own the required pair is
//! refused by name. Nothing here converts anything and no client version is read:
//! identification is a read of two declarations.

use licoup_extension_contracts::manifest::{FrozenEndpoints, is_format_identity};
use licoup_migrate::converter::{
    ConversionOwner, identify_owner, identify_owner_in_root, required_conversion,
};
use licoup_migrate::error::{
    CONVERTER_ENDPOINT_MISMATCH, CONVERTER_ENTRY_MISSING, CONVERTER_ENTRY_OUTSIDE_PACKAGE,
    CONVERTER_INCOMPLETE, CONVERTER_MISSING, CONVERTER_NOT_NATIVE, ToolError,
};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// One disposable package root, removed when the test ends.
struct PackageRoot {
    path: PathBuf,
}

impl PackageRoot {
    fn new(label: &str) -> Self {
        let base = std::env::temp_dir()
            .canonicalize()
            .expect("canonical temporary directory");
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = base.join(format!(
            "licoup-migrate-converter-{}-{label}-{unique}",
            std::process::id()
        ));
        if path.exists() {
            fs::remove_dir_all(&path).expect("clear a stale package root");
        }
        fs::create_dir_all(&path).expect("create the package root");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    /// Seed the native converter entry and answer its relative path.
    fn seed_entry(&self, relative: &str) -> String {
        let entry = self.path.join(relative);
        fs::create_dir_all(entry.parent().expect("entry directory")).expect("entry directory");
        fs::write(&entry, b"#!/bin/sh\nexec true\n").expect("seed the converter entry");
        relative.to_owned()
    }

    /// Write a manifest document, with one conversion declaration or none.
    fn seed_manifest(&self, conversion: Option<Value>) -> PathBuf {
        let mut manifest = json!({
            "schema": "licoup.extension-package.v1",
            "id": "org.licoland.fixture.converter",
            "version": "1.4.0",
            "displayName": "Synthetic converter",
            "hostProtocol": { "major": 1, "minimumMinor": 0 },
            "compatibility": { "clientVersions": ["0"] },
            "profiles": [{ "id": "agent-execution", "major": 1 }],
            "runtime": { "mode": "process", "entry": "bin/converter" },
            "activation": "on-demand",
            "requires": [],
            "optionalRequires": [],
            "permissions": [],
            "contributions": []
        });
        if let Some(declaration) = conversion {
            manifest["conversion"] = declaration;
        }
        let path = self.path.join(licoup_migrate::converter::MANIFEST_NAME);
        fs::write(
            &path,
            serde_json::to_string_pretty(&manifest).expect("manifest"),
        )
        .expect("write the manifest");
        path
    }

    /// A complete declaration for one pair.
    fn declaration(source_formats: &[&str], target_format: &str) -> Value {
        json!({
            "kind": "native-executable",
            "entry": "bin/converter",
            "sourceFormats": source_formats,
            "targetFormat": target_format,
        })
    }
}

impl Drop for PackageRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn refused(result: Result<ConversionOwner, ToolError>) -> ToolError {
    result.expect_err("this package must not be identified as the conversion owner")
}

fn required() -> FrozenEndpoints {
    FrozenEndpoints::new("agent-session.v2", "licoup.conversation.v1")
}

#[test]
fn the_tool_identifies_the_package_whose_declaration_owns_the_required_pair() {
    let root = PackageRoot::new("owner");
    let entry = root.seed_entry("bin/converter");
    let manifest = root.seed_manifest(Some(PackageRoot::declaration(
        &["agent-session.v2", "agent-session.v1"],
        "licoup.conversation.v1",
    )));

    let owner = identify_owner(&manifest, &required()).expect("the package owns this conversion");
    assert_eq!(owner.package_id, "org.licoland.fixture.converter");
    assert_eq!(owner.package_version, "1.4.0");
    assert_eq!(owner.entry, entry);
    assert_eq!(owner.target_format, "licoup.conversation.v1");
    assert_eq!(
        owner.source_formats,
        vec!["agent-session.v2".to_owned(), "agent-session.v1".to_owned()],
        "the reported source formats are the package's own list"
    );

    // The same answer through the package root, which also reads the payload.
    assert_eq!(
        identify_owner_in_root(root.path(), &required()).expect("the entry is inside the package"),
        owner
    );
}

#[test]
fn the_declaration_is_read_rather_than_recognised() {
    // Two packages, identical except for the formats they declare. A tool that
    // carried a table of known converters would answer the same for both; this
    // one answers from the document it read.
    let owns = PackageRoot::new("owns");
    owns.seed_entry("bin/converter");
    owns.seed_manifest(Some(PackageRoot::declaration(
        &["agent-session.v2"],
        "licoup.conversation.v1",
    )));

    let other = PackageRoot::new("other");
    other.seed_entry("bin/converter");
    other.seed_manifest(Some(PackageRoot::declaration(
        &["kilo.agent-session.v9"],
        "licoup.conversation.v9",
    )));

    assert!(identify_owner_in_root(owns.path(), &required()).is_ok());
    assert_eq!(
        refused(identify_owner_in_root(other.path(), &required())),
        CONVERTER_ENDPOINT_MISMATCH,
        "a package that declares another pair is refused, not recognised by name"
    );
    assert_eq!(
        refused(identify_owner_in_root(
            owns.path(),
            &FrozenEndpoints::new("kilo.agent-session.v9", "licoup.conversation.v9")
        )),
        CONVERTER_ENDPOINT_MISMATCH,
        "and the first package is refused for the pair it does not declare"
    );
}

#[test]
fn a_package_that_declares_no_conversion_is_refused() {
    let root = PackageRoot::new("absent");
    root.seed_entry("bin/converter");
    // The field is absent from the document: this package owns no format.
    let manifest = root.seed_manifest(None);

    assert_eq!(
        refused(identify_owner(&manifest, &required())),
        CONVERTER_MISSING
    );
    assert_eq!(
        refused(identify_owner_in_root(root.path(), &required())),
        CONVERTER_MISSING
    );
}

#[test]
fn an_interpreter_converter_is_refused() {
    let root = PackageRoot::new("interpreter");
    root.seed_entry("bin/converter");
    let manifest = root.seed_manifest(Some(json!({
        "kind": "node-module",
        "entry": "bin/converter",
        "sourceFormats": ["agent-session.v2"],
        "targetFormat": "licoup.conversation.v1",
    })));

    assert_eq!(
        refused(identify_owner(&manifest, &required())),
        CONVERTER_NOT_NATIVE
    );
}

#[test]
fn an_entry_outside_the_package_is_refused() {
    // The declaration itself: a path that leaves the payload is refused before any
    // file is read.
    let escaping = PackageRoot::new("escaping");
    escaping.seed_entry("bin/converter");
    let manifest = escaping.seed_manifest(Some(json!({
        "kind": "native-executable",
        "entry": "../outside/converter",
        "sourceFormats": ["agent-session.v2"],
        "targetFormat": "licoup.conversation.v1",
    })));
    assert_eq!(
        refused(identify_owner(&manifest, &required())),
        CONVERTER_ENTRY_OUTSIDE_PACKAGE
    );

    // An entry the declaration promises but the payload does not carry.
    let absent = PackageRoot::new("entry-absent");
    absent.seed_manifest(Some(PackageRoot::declaration(
        &["agent-session.v2"],
        "licoup.conversation.v1",
    )));
    assert_eq!(
        refused(identify_owner_in_root(absent.path(), &required())),
        CONVERTER_ENTRY_MISSING
    );
}

/// An entry that names a file outside the package root is refused even when the
/// declaration itself is well formed: the payload decides where the entry is.
#[cfg(unix)]
#[test]
fn an_entry_that_resolves_outside_the_package_root_is_refused() {
    let linked = PackageRoot::new("linked");
    let outside = linked.path().parent().expect("parent").join(format!(
        "licoup-migrate-converter-outside-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&outside).expect("outside directory");
    fs::write(outside.join("converter"), b"#!/bin/sh\nexec true\n").expect("outside entry");
    let bin = linked.path().join("bin");
    fs::create_dir_all(&bin).expect("bin directory");
    std::os::unix::fs::symlink(outside.join("converter"), bin.join("converter"))
        .expect("seed an escaping link");
    linked.seed_manifest(Some(PackageRoot::declaration(
        &["agent-session.v2"],
        "licoup.conversation.v1",
    )));

    assert_eq!(
        refused(identify_owner_in_root(linked.path(), &required())),
        CONVERTER_ENTRY_OUTSIDE_PACKAGE
    );
    let _ = fs::remove_dir_all(&outside);
}

#[test]
fn an_incomplete_declaration_is_refused() {
    let root = PackageRoot::new("incomplete");
    root.seed_entry("bin/converter");
    let manifest = root.seed_manifest(Some(json!({
        "kind": "native-executable",
        "entry": "bin/converter",
        "sourceFormats": ["agent-session.v2"],
    })));
    assert_eq!(
        refused(identify_owner(&manifest, &required())),
        CONVERTER_INCOMPLETE,
        "a declaration with no target format produces nothing"
    );
}

#[test]
fn a_declaration_that_disagrees_with_the_required_endpoints_is_refused_by_endpoint() {
    let root = PackageRoot::new("endpoints");
    root.seed_entry("bin/converter");
    let manifest = root.seed_manifest(Some(PackageRoot::declaration(
        &["agent-session.v2"],
        "licoup.conversation.v1",
    )));

    let wrong_source = FrozenEndpoints::new("agent-session.v3", "licoup.conversation.v1");
    assert_eq!(
        refused(identify_owner(&manifest, &wrong_source)),
        CONVERTER_ENDPOINT_MISMATCH
    );
    let wrong_target = FrozenEndpoints::new("agent-session.v2", "licoup.conversation.v2");
    assert_eq!(
        refused(identify_owner(&manifest, &wrong_target)),
        CONVERTER_ENDPOINT_MISMATCH
    );
    // One source format in the list is enough: the list is what the package
    // supports, and a required source among them is a supported source.
    assert!(identify_owner(&manifest, &required()).is_ok());
}

#[test]
fn identification_does_not_consult_the_client_version() {
    // A package released for another client line still owns its formats: the
    // compatibility list decides whether a *host* loads the package, and the tool
    // asks only which formats it converts. A migration must not be refused because
    // the installed old client is a different version from this tool.
    let root = PackageRoot::new("compatibility");
    root.seed_entry("bin/converter");
    let mut document = json!({
        "schema": "licoup.extension-package.v1",
        "id": "org.licoland.fixture.converter",
        "version": "9.9.9",
        "displayName": "Synthetic converter",
        "hostProtocol": { "major": 1, "minimumMinor": 0 },
        "compatibility": { "clientVersions": [">=99.0.0"] },
        "profiles": [{ "id": "agent-execution", "major": 1 }],
        "runtime": { "mode": "process", "entry": "bin/converter" },
        "conversion": PackageRoot::declaration(&["agent-session.v2"], "licoup.conversation.v1"),
        "activation": "on-demand",
        "requires": [],
        "optionalRequires": [],
        "permissions": [],
        "contributions": [],
    });
    let manifest = root.path().join(licoup_migrate::converter::MANIFEST_NAME);
    fs::write(
        &manifest,
        serde_json::to_string_pretty(&document).expect("manifest"),
    )
    .expect("write the manifest");

    let owner = identify_owner(&manifest, &required())
        .expect("no client version takes part in which formats a package owns");
    assert_eq!(owner.package_version, "9.9.9");

    // The same document, with a compatibility list that covers nothing at all, is
    // still the owner; and the declaration is what changes the verdict.
    document["compatibility"] = json!({ "clientVersions": [] });
    fs::write(
        &manifest,
        serde_json::to_string_pretty(&document).expect("manifest"),
    )
    .expect("write the manifest");
    assert_eq!(
        refused(identify_owner(&manifest, &required())),
        licoup_migrate::error::CONVERTER_MANIFEST_INVALID,
        "an empty client list is a manifest failure, not a conversion verdict"
    );
    document["compatibility"] = json!({ "clientVersions": [">=99.0.0"] });
    document["conversion"]["sourceFormats"] = json!(["agent-session.v3"]);
    fs::write(
        &manifest,
        serde_json::to_string_pretty(&document).expect("manifest"),
    )
    .expect("write the manifest");
    assert_eq!(
        refused(identify_owner(&manifest, &required())),
        CONVERTER_ENDPOINT_MISMATCH,
        "only the declaration decides, and only about formats"
    );
}

#[test]
fn the_required_conversion_is_the_pair_the_client_catalogue_declares() {
    let required = required_conversion().expect("the embedded catalogue declares its endpoints");
    let declared = licoup_native::domain::client_state_migration::conversion_endpoints()
        .expect("the embedded catalogue declares its endpoints");
    assert_eq!(required.source_format(), declared.source_frontier_id);
    assert_eq!(required.target_format(), declared.target_frontier_id);
    assert_ne!(
        required.source_format(),
        required.target_format(),
        "one format is not both endpoints of the same conversion"
    );
    assert!(is_format_identity(required.source_format()));
    assert!(is_format_identity(required.target_format()));

    // The pair is the frozen state-format pair, not a product version and not an
    // agent session format: the committed release fixture declares the latter.
    assert!(required.source_format().starts_with("licoup-state-"));
    assert_ne!(required.source_format(), "fixture.agent-session.v1");
}

#[test]
fn the_committed_release_fixture_owns_the_conversion_its_release_metadata_publishes() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/client_package_release/fixture-native-converter");
    assert!(fixture.is_dir(), "the release package fixture is missing");
    let declaration: Value = serde_json::from_str(
        &fs::read_to_string(fixture.join("package-release.json")).expect("release declaration"),
    )
    .expect("release declaration json");
    let converter = &declaration["converter"];

    // The fixture's own pair is the one its release declaration publishes, so the
    // manifest contract and the authenticated artifact metadata agree.
    let published = FrozenEndpoints::new(
        converter["sourceFormat"].as_str().expect("source format"),
        converter["targetFormat"].as_str().expect("target format"),
    );
    let owner = identify_owner_in_root(&fixture, &published)
        .expect("the committed fixture owns the conversion its release metadata publishes");
    assert_eq!(owner.package_id, declaration["packageId"]);
    assert_eq!(owner.entry, converter["entry"]);

    // And the tool refuses to hand it the client-state conversion: that package
    // declares a different format pair, which is exactly what reading the
    // declaration means.
    assert_eq!(
        refused(identify_owner_in_root(&fixture, &required())),
        CONVERTER_ENDPOINT_MISMATCH
    );
}
