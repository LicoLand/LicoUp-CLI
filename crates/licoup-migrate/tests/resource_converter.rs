//! The appearance converter package, read through the contract and driven through the
//! client's own coordinator.
//!
//! This suite holds the standalone tool to the package-owning contract on synthetic
//! input. Two claims are checked, and neither is inferred:
//!
//! - **Identification is a read.** The committed appearance package declares the pair the
//!   client's embedded catalogue freezes, and the tool answers with that package for that
//!   pair and refuses it for a pair it does not declare. No package name or format alias
//!   takes part.
//! - **A synthetic source converts through the real owner.** The roots here are
//!   hand-seeded, not the frozen released-root fixture. The conversion runs through
//!   `licoup_migrate::convert`, which asks the client's own migration owner to move the
//!   domain, and an unsupported source is refused with a stable code while the source is
//!   preserved.

use licoup_extension_contracts::manifest::FrozenEndpoints;
use licoup_foundation::platform::file_security::{atomic_write_private_text, ensure_private_dir};
use licoup_migrate::convert::{DomainOutcome, convert};
use licoup_migrate::converter::{ConversionOwner, identify_owner_in_root, required_conversion};
use licoup_migrate::error::{CONVERTER_ENDPOINT_MISMATCH, OWNER_REFUSED, ToolError};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

const DOMAIN: &str = "appearance-presentation";
const DOMAIN_STEP: &str = "appearance-presentation.absent-to-1";
const APPEARANCE: &str = "client-state/appearance-preferences.json";
const LEDGER: &str = "client-state/migrations/ledger.json";
const LEDGER_SCHEMA: &str = "v0.0.1:client-state-migration-ledger-1";
const PACKAGE_ID: &str = "org.licoland.converter.appearance";
const PACKAGE_ENTRY: &str = "bin/licoup-appearance-convert";

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// One disposable directory, removed when the test ends.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(label: &str) -> Self {
        let base = std::env::temp_dir()
            .canonicalize()
            .expect("canonical temporary directory");
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = base.join(format!(
            "licoup-migrate-resource-{}-{label}-{unique}",
            std::process::id()
        ));
        if path.exists() {
            fs::remove_dir_all(&path).expect("clear a stale directory");
        }
        fs::create_dir_all(&path).expect("create the directory");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn join(&self, relative: &str) -> PathBuf {
        self.path.join(relative)
    }

    /// Write one private state document, the way the client's own owners write one.
    fn seed(&self, relative: &str, document: &Value) {
        let path = self.join(relative);
        ensure_private_dir(path.parent().expect("state directory")).expect("private directory");
        atomic_write_private_text(
            &path,
            &format!("{}\n", serde_json::to_string(document).unwrap()),
        )
        .expect("seed the document");
    }

    fn read(&self, relative: &str) -> Vec<u8> {
        fs::read(self.join(relative)).expect("read the document")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// The committed manifest of the appearance converter package.
fn appearance_manifest() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../components/appearance/package/manifest.json");
    serde_json::from_slice(&fs::read(&path).expect("the committed component manifest"))
        .expect("the manifest is JSON")
}

/// A package root carrying one manifest and the entry it promises.
fn package_root(label: &str, document: &Value) -> Scratch {
    let root = Scratch::new(label);
    fs::write(
        root.join(licoup_migrate::converter::MANIFEST_NAME),
        serde_json::to_string_pretty(document).expect("manifest"),
    )
    .expect("write the manifest");
    let entry = root.join(PACKAGE_ENTRY);
    fs::create_dir_all(entry.parent().expect("entry directory")).expect("entry directory");
    fs::write(&entry, b"#!/bin/sh\nexec true\n").expect("seed the converter entry");
    root
}

/// A synthetic supported source: the appearance store as the last published release
/// wrote it, plus the user's preferences.
fn seed_supported(root: &Scratch) -> Value {
    let source = json!({
        "appearancePresetId": "preset-canary",
        "fontPreferenceId": "system-default",
        "localePreference": "zh-CN"
    });
    root.seed(APPEARANCE, &source);
    source
}

fn refused(result: Result<ConversionOwner, ToolError>) -> ToolError {
    result.expect_err("this package must not be identified as the owner of that pair")
}

#[test]
fn the_appearance_package_owns_the_pair_the_client_catalogue_freezes() {
    let manifest = appearance_manifest();
    let root = package_root("appearance", &manifest);
    let required = required_conversion().expect("the catalogue declares its endpoints");

    let owner = identify_owner_in_root(root.path(), &required)
        .expect("the committed package owns the required conversion");
    assert_eq!(owner.package_id, PACKAGE_ID);
    assert_eq!(owner.package_version, manifest["version"]);
    assert_eq!(owner.entry, PACKAGE_ENTRY);
    assert_eq!(
        owner.source_formats,
        vec![required.source_format().to_owned()],
        "the declared source list is the catalogue's own source format"
    );
    assert_eq!(owner.target_format, required.target_format());

    // The same package is refused for the pair it does not declare: the answer comes from
    // the document, not from recognising a package that carries a native converter.
    assert_eq!(
        refused(identify_owner_in_root(
            root.path(),
            &FrozenEndpoints::new("fixture.agent-session.v1", "licoup.conversation.v1")
        )),
        CONVERTER_ENDPOINT_MISMATCH
    );
}

#[test]
fn a_synthetic_supported_source_converts_through_the_client_owner() {
    let root = Scratch::new("convert");
    let source = seed_supported(&root);
    let owed = vec![DOMAIN.to_owned()];

    let report = convert(root.path(), &owed, true).expect("the client's owner converts the root");

    assert_eq!(report.status, "converted");
    assert_eq!(report.domains.len(), 1);
    assert_eq!(report.domains[0].domain_id, DOMAIN);
    assert_eq!(report.domains[0].outcome, DomainOutcome::Converted);
    assert!(report.is_complete(), "{report:?}");

    // The client's own store carries the target shape with every seeded value preserved.
    let converted: Value = serde_json::from_slice(&root.read(APPEARANCE)).expect("JSON");
    assert_eq!(converted["schemaVersion"], json!(1));
    for field in ["appearancePresetId", "fontPreferenceId", "localePreference"] {
        assert_eq!(converted[field], source[field], "{field} survives");
    }

    // And the client's ledger recorded the one step, once.
    let ledger: Value = serde_json::from_slice(&root.read(LEDGER)).expect("JSON");
    assert_eq!(ledger["schemaVersion"], json!(LEDGER_SCHEMA));
    assert_eq!(
        ledger["domains"][DOMAIN]["completedStepIds"],
        json!([DOMAIN_STEP])
    );
}

#[test]
fn an_unsupported_store_shape_is_refused_with_a_stable_code_and_preserves_the_source() {
    let root = Scratch::new("unsupported");
    // A shape no release published. The client's own probe refuses it and the tool
    // reports that refusal instead of converting the domain.
    root.seed(
        APPEARANCE,
        &json!({ "schemaVersion": 9, "appearancePresetId": "canary" }),
    );
    let before = root.read(APPEARANCE);

    let report = convert(root.path(), &[DOMAIN.to_owned()], true).expect("the tool reports");

    assert!(!report.is_complete(), "{report:?}");
    assert_eq!(report.refused.len(), 1, "{report:?}");
    assert_eq!(report.refused[0].domain_id, DOMAIN);
    assert_eq!(report.refused[0].code, "state_newer_than_binary");
    assert_eq!(report.still_owed, vec![DOMAIN.to_owned()]);
    assert_eq!(
        report.domains[0].outcome,
        DomainOutcome::StillOwed,
        "a refused domain is never reported as converted"
    );
    assert_eq!(
        root.read(APPEARANCE),
        before,
        "the refused store is preserved exactly as it was"
    );
}

#[test]
fn a_root_whose_declared_format_is_not_a_published_endpoint_is_refused_by_the_owner() {
    let root = Scratch::new("undeclared");
    seed_supported(&root);
    root.seed(
        LEDGER,
        &json!({
            "schemaVersion": LEDGER_SCHEMA,
            "highestAdmittedProductVersion": "0.0.0",
            "frontierId": "licoup-state-0.0.9",
            "domains": {}
        }),
    );
    let source_before = root.read(APPEARANCE);

    let error = convert(root.path(), &[DOMAIN.to_owned()], true)
        .expect_err("the client's owner refuses a format no release published");

    // The owner's refusal is reported as the tool's own stable code, and the source the
    // owner refused to move is exactly the source that was seeded.
    assert_eq!(error, OWNER_REFUSED);
    assert_eq!(root.read(APPEARANCE), source_before);
}
