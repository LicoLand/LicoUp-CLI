//! The committed extension sample is built, imported and driven by the real
//! owners, not described.
//!
//! `crates/licoup-extension-contracts/samples/state-converter/` is the developer
//! example: a manifest that declares one persisted-format conversion and one
//! native entry inside the payload. This suite is where the example stops being a
//! document:
//!
//! 1. **It builds.** The entry is compiled from the sample's own
//!    `converter/main.rs` with `rustc` — a real native executable, no
//!    interpreter, no workspace member and no dependency.
//! 2. **It is imported.** The compiled payload is handed to the coordinator as an
//!    offline payload, so it travels the host's own `ExpandedPackage::expand` and
//!    the store's own install before anything is selected. The store's
//!    import entry point is exercised directly as well.
//! 3. **It is selected by declaration.** The client's required pair comes from
//!    the frontier this binary embeds, and the sample is a candidate because its
//!    own manifest declares that pair — not because of its name or version.
//! 4. **Its stop and update contracts are the tool's.** The writers-stopped
//!    statement is required before anything opens, a stopped run is an unfinished
//!    run rather than a refusal, and the continued run continues the target it
//!    already had.
//!
//! Every root here is synthetic and disposable, and every run is offline.
//!
//! # Identity
//!
//! The sample declares the client line it supports (`>=0.3.0, <1.0.0`), and the
//! store refuses a package whose compatibility list does not cover the client it
//! is installed on. A development build reports the documented `0.0.1-alpha`
//! fallback, so this suite runs under the planned candidate identity the crate's
//! own runner constructs — `tools/scripts/migration-crate-tests.mjs` sets
//! `LICO_CLIENT_PRODUCT_VERSION` from the workspace product version:
//!
//! ```text
//! node tools/scripts/migration-crate-tests.mjs
//! LICO_CLIENT_PRODUCT_VERSION=<workspace version> cargo test -p licoup-migrate \
//!     --test state_converter_sample
//! ```
//!
//! That is not a courtesy to this suite: it is the identity the package's own
//! declaration is written for, and a suite that lowered the declaration to fit a
//! development build would stop testing the declaration that ships.

use licoup_extension_contracts::manifest::PackageManifest;
use licoup_migrate::converter::required_conversion;
use licoup_migrate::error::{PACKAGE_CONVERSION_UNFINISHED, ToolError, WRITERS_RUNNING};
use licoup_migrate::inventory::{InventoryRequest, inventory};
use licoup_migrate::package_conversion::{PackageConversionRequest, convert as package_convert};
use licoup_native::platform::extension_packages::{
    ArtifactLimits, ExpandedPackage, InstalledPackage, PackageStore, TrustRecord, content_digest,
    preflight,
};
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The sample's own directory, read from the checkout rather than restated.
fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../licoup-extension-contracts/samples/state-converter")
        .canonicalize()
        .expect("the committed sample directory")
}

/// The manifest the sample ships, as a document.
fn sample_manifest() -> Value {
    serde_json::from_str(
        &fs::read_to_string(sample().join("manifest.json")).expect("sample manifest"),
    )
    .expect("sample manifest JSON")
}

/// The entry the sample declares, relative to the package root.
fn declared_entry() -> String {
    PackageManifest::from_value(sample_manifest())
        .expect("the sample manifest is admitted")
        .conversion
        .expect("the sample declares a conversion")
        .entry
}

/// The sample's converter, compiled once for this test process.
///
/// The package carries a program, so the payload carries a compiled executable:
/// the entry cannot be a script, and the sample ships source instead of a binary
/// because a compiled artefact is not a committed file.
fn converter_binary() -> &'static Path {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY.get_or_init(|| {
        let directory = scratch_directory("entry");
        let source = sample().join("converter/main.rs");
        let binary = directory.join("licoup-sample-state-converter");
        let compiler = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
        let status = std::process::Command::new(compiler)
            .arg("--edition=2021")
            .arg("-C")
            .arg("debuginfo=0")
            .arg("-o")
            .arg(&binary)
            .arg(&source)
            .status()
            .expect("rustc runs");
        assert!(
            status.success(),
            "the sample converter compiles from {}",
            source.display()
        );
        binary
    })
}

/// One disposable directory under the system temporary root.
fn scratch_directory(label: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir()
        .canonicalize()
        .expect("canonical temporary directory")
        .join(format!(
            "licoup-migrate-sample-{}-{label}-{unique}",
            std::process::id()
        ));
    if path.exists() {
        fs::remove_dir_all(&path).expect("clear a stale directory");
    }
    fs::create_dir_all(&path).expect("create the directory");
    path
}

/// The payload the sample declares: the manifest at the archive root and the
/// compiled entry at the path the manifest names.
fn payload_bytes() -> Vec<u8> {
    use zip::write::SimpleFileOptions;
    let entry = declared_entry();
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        writer
            .start_file("manifest.json", options)
            .expect("manifest entry");
        writer
            .write_all(
                fs::read(sample().join("manifest.json"))
                    .expect("sample manifest bytes")
                    .as_slice(),
            )
            .expect("manifest bytes");
        writer
            .start_file(entry.clone(), options.unix_permissions(0o755))
            .expect("entry");
        writer
            .write_all(&fs::read(converter_binary()).expect("converter bytes"))
            .expect("converter bytes");
        writer.finish().expect("payload finished");
    }
    buffer.into_inner()
}

/// Publish the sample payload through the store's own import path.
fn install(store_root: &Path) -> InstalledPackage {
    let manifest = sample_manifest();
    let bytes = payload_bytes();
    let store = PackageStore::open(store_root).expect("package store");
    let trust =
        TrustRecord::local_approved(content_digest(&bytes), Vec::new()).expect("local approval");
    store
        .install_local_import(
            manifest["id"].as_str().expect("id"),
            manifest["version"].as_str().expect("version"),
            trust,
            &bytes,
        )
        .expect("the committed sample imports")
        .installed
}

/// One synthetic data root carrying the persisted documents the sample reads.
fn seed_data_root(root: &Path, hold_millis: Option<u64>) {
    licoup_foundation::platform::file_security::ensure_private_dir(&root.join("client-state"))
        .expect("client state");
    for (name, document) in [
        ("preferences.json", r#"{"theme":"dark"}"#),
        ("layout.json", r#"{"sidebar":"wide"}"#),
    ] {
        licoup_foundation::platform::file_security::atomic_write_private_text(
            &root.join("client-state").join(name),
            document,
        )
        .expect("state document");
    }
    if let Some(millis) = hold_millis {
        fs::write(
            root.join("client-state/sample-conversion-hold-millis.txt"),
            format!("{millis}\n"),
        )
        .expect("hold document");
    }
}

/// Every file under one root with its bytes, for byte-for-byte comparisons.
fn root_files(root: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    fn collect(
        root: &Path,
        directory: &Path,
        into: &mut std::collections::BTreeMap<String, Vec<u8>>,
    ) {
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect(root, &path, into);
            } else if let Ok(bytes) = fs::read(&path) {
                let name = path
                    .strip_prefix(root)
                    .expect("a path below the root")
                    .to_string_lossy()
                    .replace('\\', "/");
                into.insert(name, bytes);
            }
        }
    }
    let mut files = std::collections::BTreeMap::new();
    collect(root, root, &mut files);
    files
}

fn refused<T: std::fmt::Debug>(result: Result<T, ToolError>) -> ToolError {
    result.expect_err("this run must be refused")
}

fn request<'a>(
    data_root: &'a Path,
    work_root: &'a Path,
    store: &'a Path,
    writers_stopped: bool,
    resume: bool,
    stop: Option<&'a dyn Fn() -> bool>,
) -> PackageConversionRequest<'a> {
    PackageConversionRequest {
        data_root,
        work_root,
        package_store: store,
        package: None,
        payload: None,
        index: None,
        index_public_keys: None,
        writers_stopped,
        resume,
        stop,
    }
}

#[test]
fn the_sample_payload_expands_and_imports_through_the_hosts_own_path() {
    let root = scratch_directory("import");
    let bytes = payload_bytes();
    let limits = ArtifactLimits::default();

    // The host's preflight and expansion are what an import reads first: the
    // archive's own bounds, then the manifest inside it, then the entry the
    // manifest declares must be a file in the payload that was just written.
    preflight(&bytes, &limits).expect("the payload is a bounded archive");
    let staging = root.join("staging");
    let expanded =
        ExpandedPackage::expand(&bytes, &staging, &limits).expect("the sample payload expands");
    let manifest = expanded.manifest().clone();
    assert_eq!(manifest.id, "org.licoland.example.state-converter");
    let conversion = manifest.conversion.as_ref().expect("declared conversion");
    assert_eq!(conversion.entry, declared_entry());
    assert!(
        expanded.content_dir().join(&conversion.entry).is_file(),
        "the declared entry is a file inside the payload"
    );
    assert_eq!(
        manifest
            .conversion_owner(&required_conversion().expect("the embedded catalogue"))
            .expect("the sample owns the required conversion")
            .entry,
        conversion.entry
    );

    // The store's own import path publishes it, and the record it writes names
    // the identity the caller asked for.
    let store_root = root.join("store");
    let installed = install(&store_root);
    assert_eq!(installed.package_id, "org.licoland.example.state-converter");
    assert_eq!(installed.version, "1.0.0");
    assert!(
        installed.runtime_ref.is_none(),
        "the sample carries its own program and borrows no interpreter"
    );
    let (_, catalogue) = PackageStore::open(&store_root)
        .expect("package store")
        .catalogue()
        .expect("the catalogue reads");
    assert_eq!(catalogue.len(), 1, "one version is installed");

    // Bytes that do not describe the package being installed are refused before
    // anything is published.
    let mislabelled = {
        let store = PackageStore::open(&root.join("mislabelled-store")).expect("package store");
        let trust = TrustRecord::local_approved(content_digest(&bytes), Vec::new())
            .expect("local approval");
        store.install_local_import("org.licoland.example.other", "1.0.0", trust, &bytes)
    };
    assert_eq!(
        mislabelled
            .expect_err("a manifest that is not the requested package")
            .code,
        "package_manifest_mismatch"
    );
}

#[test]
fn the_inventory_selects_the_sample_because_its_manifest_declares_the_required_pair() {
    let root = scratch_directory("inventory");
    let store_root = root.join("store");
    install(&store_root);
    let endpoints = required_conversion().expect("the embedded catalogue declares its endpoints");

    let (report, selected) = inventory(
        &InventoryRequest {
            package_store: &store_root,
            index: None,
            index_public_keys: None,
            requested_package: None,
        },
        &endpoints,
    )
    .expect("the installed converter inventory reads");

    assert_eq!(report.source_format, endpoints.source_format());
    assert_eq!(report.target_format, endpoints.target_format());
    assert_eq!(
        report.candidates,
        vec!["org.licoland.example.state-converter@1.0.0".to_owned()],
        "the sample is a candidate because of its own declaration"
    );
    let selected = selected.expect("one candidate is selected");
    assert_eq!(selected.entry, declared_entry());

    // A caller that names a package which does not declare the pair is refused
    // rather than silently given the one that does.
    assert_eq!(
        refused(inventory(
            &InventoryRequest {
                package_store: &store_root,
                index: None,
                index_public_keys: None,
                requested_package: Some("org.licoland.example.absent"),
            },
            &endpoints,
        ))
        .code(),
        "converter_unavailable"
    );
}

#[test]
fn a_run_the_operator_did_not_stop_the_writers_for_is_refused_before_anything_is_opened() {
    let root = scratch_directory("writers");
    let data_root = root.join("data");
    let work_root = root.join("work");
    seed_data_root(&data_root, None);

    let error = refused(package_convert(&request(
        &data_root,
        &work_root,
        &root.join("store"),
        false,
        false,
        None,
    )));
    assert_eq!(error.code(), WRITERS_RUNNING.code());
    assert!(
        !work_root.exists(),
        "the refusal comes before a staging directory, journal or target exists"
    );
}

#[test]
fn the_sample_converts_through_the_coordinator_and_reports_the_published_receipt() {
    let root = scratch_directory("convert");
    let data_root = root.join("data");
    let work_root = root.join("work");
    let store_root = root.join("store");
    let payload = root.join("sample.zip");
    seed_data_root(&data_root, None);
    fs::write(&payload, payload_bytes()).expect("payload written");
    fs::create_dir_all(&store_root).expect("package store root");
    let before = root_files(&data_root);

    // The payload is named, so the run takes the offline import path the release
    // tooling publishes, and the manifest inside the archive is what gets
    // installed and selected.
    let mut convert_request = request(&data_root, &work_root, &store_root, true, false, None);
    convert_request.payload = Some(payload.as_path());
    let report = package_convert(&convert_request).expect("the sample converts the root");

    assert_eq!(report.status, "converted");
    assert!(report.is_complete());
    assert_eq!(report.package_id, "org.licoland.example.state-converter");
    assert_eq!(report.entry, declared_entry());
    assert_eq!(report.source_format, "licoup-state-0.1.1");
    assert_eq!(report.target_format, "licoup-state-0.3.0");
    assert!(report.source_untouched);
    assert_eq!(
        report.converted_records,
        Some(2),
        "the sample reports one converted document per persisted document"
    );

    // The receipt is the converter's own document, checked rather than trusted:
    // the process exiting zero is not a conversion.
    let converter = report.converter.as_ref().expect("the converter ran");
    assert_eq!(converter.exit_code, Some(0));
    assert!(converter.complete && converter.settled());
    let result = converter.result.as_ref().expect("the result document");
    assert_eq!(result.schema, "licoup.package-conversion-result.v1");
    assert_eq!(result.source_format, "licoup-state-0.1.1");
    assert_eq!(result.target_format, "licoup-state-0.3.0");
    assert!(result.complete);
    assert_eq!(result.converted_records, Some(2));

    // What the converter produced is in the target, and the data root it was
    // given a copy of is byte-identical afterwards.
    let target = work_root.join("target");
    for name in ["preferences.json", "layout.json"] {
        assert_eq!(
            fs::read(target.join("client-state").join(name)).expect("converted document"),
            fs::read(data_root.join("client-state").join(name)).expect("source document"),
            "the sample's move re-encodes each document it read"
        );
    }
    assert_eq!(root_files(&data_root), before);

    // A repeated operation answers from the record: the converter is not asked a
    // second time over a target that is already the result.
    let repeat = package_convert(&request(
        &data_root,
        &work_root,
        &store_root,
        true,
        false,
        None,
    ))
    .expect("the repeat answers");
    assert_eq!(repeat.status, "alreadyCurrent");
    assert_eq!(repeat.attempts, 1);
}

#[test]
fn a_stopped_run_is_unfinished_and_the_resume_continues_the_target_it_already_had() {
    let root = scratch_directory("stop");
    let data_root = root.join("data");
    let work_root = root.join("work");
    let store_root = root.join("store");
    let payload = root.join("sample.zip");
    // The sample holds before writing its result for as long as this document
    // states, so the stop window is observed instead of raced.
    seed_data_root(&data_root, Some(30_000));
    fs::write(&payload, payload_bytes()).expect("payload written");
    fs::create_dir_all(&store_root).expect("package store root");

    // The caller stops the run as soon as the converter has said it started: the
    // sample holds for 30 seconds after that marker, so the window is entered
    // rather than raced.
    let started = work_root.join("target/started");
    let stop = || started.exists();
    let mut convert_request = request(
        &data_root,
        &work_root,
        &store_root,
        true,
        false,
        Some(&stop),
    );
    convert_request.payload = Some(payload.as_path());
    let interrupted =
        package_convert(&convert_request).expect("a stopped run is an outcome, not a refusal");

    assert_eq!(interrupted.status, "partial");
    assert!(!interrupted.is_complete());
    let converter = interrupted.converter.as_ref().expect("the converter ran");
    assert!(converter.stopped, "the run reports that it was stopped");
    assert!(!converter.complete);
    assert!(!converter.settled());
    assert_eq!(
        converter.reason.as_deref(),
        Some("package_conversion_stopped"),
        "a stop is reported in the tool's own vocabulary"
    );
    assert!(
        !work_root.join("result.json").exists(),
        "a stopped converter reports nothing, and no report is not a completion"
    );
    // What the interrupted attempt produced is preserved: the resume depends on it.
    assert!(started.exists(), "the interrupted target is kept");

    // The unfinished run is not restarted by a plain operation; the caller has to
    // say it is continuing one.
    assert_eq!(
        refused(package_convert(&request(
            &data_root,
            &work_root,
            &store_root,
            true,
            false,
            None,
        )))
        .code(),
        PACKAGE_CONVERSION_UNFINISHED.code()
    );

    // The continuation runs under the same declared pair and the same staged
    // copy, and it continues the target the stopped attempt left behind.
    let resumed = package_convert(&request(
        &data_root,
        &work_root,
        &store_root,
        true,
        true,
        None,
    ))
    .expect("the resume continues");
    assert_eq!(resumed.status, "converted");
    assert!(resumed.resumed);
    assert!(resumed.is_complete());
    assert_eq!(resumed.attempts, 2);
    assert_eq!(resumed.converted_records, Some(2));

    // The converter was continued, not started over: its own record of the two
    // attempts is appended to, and the marker the interrupted attempt wrote is
    // still the one on disk.
    let log = fs::read_to_string(work_root.join("target/attempts.log")).expect("attempt log");
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 2, "one line per invocation: {log:?}");
    assert!(lines[0].ends_with("resume=false"), "{log:?}");
    assert!(
        lines[1].ends_with("resume=true"),
        "the continuation is declared to the converter: {log:?}"
    );
    assert_eq!(
        fs::read_to_string(&started).expect("first attempt marker"),
        "started\n",
        "the first attempt's target was never cleared"
    );
}

#[test]
fn the_sample_manifest_is_the_document_the_payload_carries() {
    // The committed manifest is the one the payload is built from, so the schema
    // check on the committed file is a check on what an import reads.
    let manifest: Value =
        serde_json::from_slice(&fs::read(sample().join("manifest.json")).expect("sample manifest"))
            .expect("manifest JSON");
    let release: Value = serde_json::from_slice(
        &fs::read(sample().join("package-release.json")).expect("release declaration"),
    )
    .expect("release declaration JSON");
    assert_eq!(release["packageId"], manifest["id"]);
    assert_eq!(release["packageVersion"], manifest["version"]);
    assert_eq!(
        release["converter"]["entry"], manifest["conversion"]["entry"],
        "the release index republishes the entry the manifest declares"
    );
    assert_eq!(release["converter"]["sourceFormat"], "licoup-state-0.1.1");
    // The manifest declares more than the release entry publishes: one entry
    // names the pair a signed index binds, the manifest names every source the
    // program reads.
    assert!(
        manifest["conversion"]["sourceFormats"]
            .as_array()
            .expect("declared sources")
            .iter()
            .any(|source| source == &json!("licoup-state-0.1.1"))
    );
}
