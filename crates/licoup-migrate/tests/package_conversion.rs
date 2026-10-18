//! The standalone tool coordinates package-owned converters across skipped releases.
//!
//! Every fixture here is synthetic and disposable: a data root built in the test's
//! own directory, a package store the store's own import path published, and a
//! converter compiled by `rustc` at test start — a real native executable, no
//! interpreter, no network and no installed client. The suites hold the tool to
//! the four properties the delivery names:
//!
//! 1. **Selection is by declaration.** A package that declares no conversion, or
//!    another pair, is not a candidate; the required pair is the client catalogue's.
//! 2. **One operation converts a root several releases behind**, and the data root
//!    is byte-identical afterwards.
//! 3. **A run is continued, never restarted.** An interrupted converter leaves the
//!    target it already produced, the next explicit resume continues it, and a
//!    plain convert refuses to start a second run over an unsettled one.
//! 4. **Completion is the converter's own checked report.** A converter that fails,
//!    is stopped, or reports an incomplete conversion leaves the run unfinished and
//!    the tool's exit status non-zero.

mod support;

use licoup_extension_contracts::manifest::FrozenEndpoints;
use licoup_migrate::converter::required_conversion;
use licoup_migrate::error::{
    CONVERTER_MISSING, CONVERTER_MODIFIED_SOURCE, CONVERTER_UNAVAILABLE,
    MAINTENANCE_ADMISSION_CLOSED, MAINTENANCE_WORK_UNFINISHED, PACKAGE_CONVERSION_UNFINISHED,
    PACKAGE_PAYLOAD_INVALID, ToolError,
};
use licoup_migrate::inventory::{InventoryRequest, inventory};
use licoup_migrate::package_conversion::{PackageConversionRequest, convert as package_convert};
use licoup_native::platform::extension_packages::{
    InstalledPackage, PackageStore, TrustRecord, canonical_unsigned_bytes, content_digest,
};
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use support::{TestRoot, write_file};

/// The synthetic converter, compiled once per test process.
///
/// It implements the documented protocol with nothing but `std`: it records every
/// invocation in the target, honours `--resume` by continuing rather than clearing,
/// and takes its behaviour from a control document in the source so one program
/// covers the conversions, the failures and the interruptions the suites need.
const CONVERTER_SOURCE: &str = r#"
use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::exit;

fn main() {
    let mut source = PathBuf::new();
    let mut target = PathBuf::new();
    let mut result = PathBuf::new();
    let mut source_format = String::new();
    let mut target_format = String::new();
    let mut resume = false;
    let args: Vec<String> = env::args().skip(1).collect();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--source" => { source = PathBuf::from(&args[index + 1]); index += 2; }
            "--target" => { target = PathBuf::from(&args[index + 1]); index += 2; }
            "--source-format" => { source_format = args[index + 1].clone(); index += 2; }
            "--target-format" => { target_format = args[index + 1].clone(); index += 2; }
            "--result" => { result = PathBuf::from(&args[index + 1]); index += 2; }
            "--resume" => { resume = true; index += 1; }
            other => { eprintln!("unsupported argument {other}"); exit(64); }
        }
    }
    let control = fs::read_to_string(source.join("control.txt")).unwrap_or_default();
    let log = target.join("attempts.log");
    let attempts = fs::read_to_string(&log)
        .map(|text| text.lines().count())
        .unwrap_or(0);
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .expect("attempt log");
    writeln!(file, "attempt={} resume={}", attempts + 1, resume).expect("attempt log line");
    drop(file);
    fs::write(target.join("started"), "started\n").expect("started marker");

    if control.contains("slow") && attempts == 0 {
        std::thread::sleep(std::time::Duration::from_secs(10));
    }
    if control.contains("write-source") {
        fs::write(source.join("intruder.txt"), "intruder").expect("intruder");
    }
    if control.contains("fail-once") && attempts == 0 {
        exit(3);
    }
    let payload = fs::read_to_string(source.join("payload.txt")).unwrap_or_default();
    let records = payload.lines().count() as u64;
    let complete = !control.contains("partial");
    if complete {
        fs::write(target.join("converted.txt"), format!("converted:{payload}")).expect("target");
    }
    let document = format!(
        "{{\"schema\":\"licoup.package-conversion-result.v1\",\"sourceFormat\":\"{}\",\
          \"targetFormat\":\"{}\",\"complete\":{},\"convertedRecords\":{}}}",
        source_format, target_format, complete, records
    );
    fs::write(&result, document).expect("result document");
}
"#;

/// The converter binary, compiled once for this test process.
fn converter_binary() -> &'static Path {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();
    BINARY.get_or_init(|| {
        let directory = std::env::temp_dir().join(format!(
            "licoup-migrate-package-converter-{}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).expect("converter directory");
        let source = directory.join("converter.rs");
        fs::write(&source, CONVERTER_SOURCE).expect("converter source");
        let binary = directory.join("converter");
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
        assert!(status.success(), "the fixture converter compiles");
        binary
    })
}

/// The required pair, from the client's own catalogue.
fn required() -> FrozenEndpoints {
    required_conversion().expect("the embedded catalogue declares its endpoints")
}

/// One package manifest that declares a conversion.
fn manifest(package_id: &str, version: &str, entry: &str, conversion: Option<Value>) -> Value {
    let mut document = json!({
        "schema": "licoup.extension-package.v1",
        "id": package_id,
        "version": version,
        "displayName": "Synthetic converter",
        "hostProtocol": { "major": 1, "minimumMinor": 0 },
        "compatibility": { "clientVersions": [">=0.0.1-alpha, <99.0.0"] },
        "profiles": [{ "id": "agent-execution", "major": 1 }],
        "runtime": { "mode": "process", "entry": entry },
        "activation": "on-demand",
        "requires": [],
        "optionalRequires": [],
        "permissions": [],
        "contributions": []
    });
    if let Some(conversion) = conversion {
        document["conversion"] = conversion;
    }
    document
}

/// The pair a fixture package declares, for the endpoints the catalogue requires.
fn declared(sources: &[&str], target: &str) -> Value {
    json!({
        "kind": "native-executable",
        "entry": "bin/converter",
        "sourceFormats": sources,
        "targetFormat": target,
    })
}

/// A conversion declaration for the required pair, with earlier releases listed.
fn skipped_release_declaration() -> Value {
    let endpoints = required();
    let earlier = ["licoup-state-0.0.9", "licoup-state-0.1.0"];
    let mut sources: Vec<&str> = earlier.to_vec();
    sources.push(endpoints.source_format());
    declared(&sources, endpoints.target_format())
}

/// One package payload carrying the fixture converter.
fn payload_bytes(document: &Value) -> Vec<u8> {
    use zip::write::SimpleFileOptions;
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        writer
            .start_file("manifest.json", options)
            .expect("manifest entry");
        writer
            .write_all(document.to_string().as_bytes())
            .expect("manifest bytes");
        writer
            .start_file("bin/converter", options.unix_permissions(0o755))
            .expect("converter entry");
        writer
            .write_all(&fs::read(converter_binary()).expect("converter binary"))
            .expect("converter bytes");
        writer.finish().expect("payload finished");
    }
    buffer.into_inner()
}

/// Publish one synthetic package through the store's own import path.
fn install_package(store_root: &Path, document: &Value) -> InstalledPackage {
    let package_id = document["id"].as_str().expect("package id");
    let version = document["version"].as_str().expect("package version");
    let bytes = payload_bytes(document);
    let store = PackageStore::open(store_root).expect("package store");
    let trust =
        TrustRecord::local_approved(content_digest(&bytes), Vec::new()).expect("local approval");
    store
        .install_local_import(package_id, version, trust, &bytes)
        .expect("the fixture package installs")
        .installed
}

/// One disposable data root with a control document and a payload.
fn seed_data_root(root: &Path, control: &str) {
    licoup_foundation::platform::file_security::ensure_private_dir(&root.join("client-state"))
        .expect("client state");
    write_file(root, "payload.txt", b"one\ntwo\nthree\n");
    licoup_foundation::platform::file_security::atomic_write_private_text(
        &root.join("client-state/preferences.json"),
        r#"{"theme":"dark"}"#,
    )
    .expect("preferences document");
    write_file(root, "control.txt", control.as_bytes());
}

fn refused<T: std::fmt::Debug>(result: Result<T, ToolError>) -> ToolError {
    result.expect_err("this run must be refused")
}

#[test]
fn the_inventory_selects_by_declaration_and_a_package_without_one_is_not_a_candidate() {
    let root = TestRoot::new("inventory");
    let store_root = root.path().join("store");
    let endpoints = required();

    install_package(
        &store_root,
        &manifest(
            "org.licoland.fixture.converter",
            "1.4.0",
            "bin/converter",
            Some(skipped_release_declaration()),
        ),
    );
    install_package(
        &store_root,
        &manifest(
            "org.licoland.fixture.other-pair",
            "9.9.9",
            "bin/converter",
            Some(declared(
                &["fixture.agent-session.v1"],
                "licoup.conversation.v1",
            )),
        ),
    );
    install_package(
        &store_root,
        &manifest("org.licoland.fixture.plain", "2.0.0", "bin/converter", None),
    );

    let (report, selected) = inventory(
        &InventoryRequest {
            package_store: &store_root,
            index: None,
            index_public_keys: None,
            requested_package: None,
        },
        &endpoints,
    )
    .expect("the inventory reads");

    assert_eq!(report.source_format, endpoints.source_format());
    assert_eq!(report.target_format, endpoints.target_format());
    assert_eq!(report.packages.len(), 3);

    // Only the declared pair is a candidate, and a greater version of another
    // declaration does not outrank it.
    assert_eq!(
        report.candidates,
        vec!["org.licoland.fixture.converter@1.4.0".to_string()]
    );
    let selected = selected.expect("one candidate is selected");
    assert_eq!(selected.package_id, "org.licoland.fixture.converter");
    assert_eq!(selected.target_format, endpoints.target_format());
    assert!(
        selected
            .source_formats
            .iter()
            .any(|format| format == endpoints.source_format()),
        "the required source is one the package declares"
    );

    let plain = report
        .packages
        .iter()
        .find(|package| package.package_id == "org.licoland.fixture.plain")
        .expect("the plain package is reported");
    assert!(!plain.candidate);
    assert_eq!(plain.reason.as_deref(), Some(CONVERTER_MISSING.code()));

    let other = report
        .packages
        .iter()
        .find(|package| package.package_id == "org.licoland.fixture.other-pair")
        .expect("the other pair is reported");
    assert!(!other.candidate);
    assert_eq!(
        other.reason.as_deref(),
        Some(licoup_migrate::error::CONVERTER_ENDPOINT_MISMATCH.code())
    );

    // A caller that names a package without the declaration is refused, not
    // silently given the one that has it.
    assert_eq!(
        refused(inventory(
            &InventoryRequest {
                package_store: &store_root,
                index: None,
                index_public_keys: None,
                requested_package: Some("org.licoland.fixture.other-pair"),
            },
            &endpoints,
        ))
        .code(),
        CONVERTER_UNAVAILABLE.code()
    );
}

#[test]
fn one_operation_converts_a_root_several_releases_behind_and_leaves_the_source_untouched() {
    let root = TestRoot::new("convert");
    let store_root = root.path().join("store");
    let data_root = root.path().join("data");
    let work_root = root.path().join("work");
    seed_data_root(&data_root, "");
    install_package(
        &store_root,
        &manifest(
            "org.licoland.fixture.converter",
            "1.4.0",
            "bin/converter",
            Some(skipped_release_declaration()),
        ),
    );

    let before = support::root_files(&data_root);
    let report = package_convert(&PackageConversionRequest {
        data_root: &data_root,
        work_root: &work_root,
        package_store: &store_root,
        package: None,
        payload: None,
        index: None,
        index_public_keys: None,
        writers_stopped: true,
        resume: false,
        stop: None,
    })
    .expect("the run converts");

    assert_eq!(report.status, "converted");
    assert!(report.is_complete());
    assert_eq!(report.attempts, 1);
    assert!(!report.resumed);
    assert!(report.source_untouched);
    assert_eq!(report.converted_records, Some(3));
    assert_eq!(
        report.package_id, "org.licoland.fixture.converter",
        "the report names the package that declared the pair"
    );
    assert!(report.steps.iter().all(|step| step.status == "committed"));

    // The converted target is the staged copy's product, and the source root is the
    // same root it was before the run: byte for byte, file for file.
    assert_eq!(
        fs::read_to_string(work_root.join("target/converted.txt")).expect("target"),
        "converted:one\ntwo\nthree\n"
    );
    assert_eq!(
        support::root_files(&data_root),
        before,
        "a conversion stages a copy and never writes the data root"
    );
    assert!(
        !data_root.join("intruder.txt").exists(),
        "nothing the converter does reaches the source root"
    );

    // A second operation over the settled run answers from the record instead of
    // running the converter again.
    let repeat = package_convert(&PackageConversionRequest {
        data_root: &data_root,
        work_root: &work_root,
        package_store: &store_root,
        package: None,
        payload: None,
        index: None,
        index_public_keys: None,
        writers_stopped: true,
        resume: false,
        stop: None,
    })
    .expect("the repeat answers");
    assert_eq!(repeat.status, "alreadyCurrent");
    assert!(repeat.is_complete());
    assert_eq!(
        repeat.attempts, 1,
        "the converter is not asked a second time"
    );
}

#[test]
fn an_interrupted_converter_resumes_the_same_target_and_a_plain_convert_never_restarts_it() {
    let root = TestRoot::new("resume");
    let store_root = root.path().join("store");
    let data_root = root.path().join("data");
    let work_root = root.path().join("work");
    // The converter records its first attempt and then waits, so the run can be
    // stopped inside it exactly as a user's cancellation would.
    seed_data_root(&data_root, "slow");
    install_package(
        &store_root,
        &manifest(
            "org.licoland.fixture.converter",
            "1.4.0",
            "bin/converter",
            Some(skipped_release_declaration()),
        ),
    );

    let armed = AtomicBool::new(true);
    let started = work_root.join("target/started");
    let stop = || {
        if !armed.load(Ordering::SeqCst) {
            return false;
        }
        started.exists()
    };
    let interrupted = package_convert(&PackageConversionRequest {
        data_root: &data_root,
        work_root: &work_root,
        package_store: &store_root,
        package: None,
        payload: None,
        index: None,
        index_public_keys: None,
        writers_stopped: true,
        resume: false,
        stop: Some(&stop),
    })
    .expect("a stopped run is an outcome, not a refusal");

    armed.store(false, Ordering::SeqCst);
    assert_eq!(interrupted.status, "partial");
    assert!(!interrupted.is_complete());
    assert_eq!(interrupted.attempts, 1);
    let converter = interrupted.converter.as_ref().expect("the converter ran");
    assert!(converter.stopped, "the run reports that it was stopped");
    let target_after_stop = support::root_files(&work_root.join("target"));
    assert!(
        target_after_stop.contains_key("started"),
        "the interrupted attempt's target is preserved: {target_after_stop:?}"
    );

    // The unfinished run is not restarted: a plain operation refuses it and names the
    // resume that continues it.
    assert_eq!(
        refused(package_convert(&PackageConversionRequest {
            data_root: &data_root,
            work_root: &work_root,
            package_store: &store_root,
            package: None,
            payload: None,
            index: None,
            index_public_keys: None,
            writers_stopped: true,
            resume: false,
            stop: None,
        }))
        .code(),
        PACKAGE_CONVERSION_UNFINISHED.code()
    );

    // The source is the same root across both attempts — that is what makes the
    // resume the same migration — so nothing between the attempts edits it.
    let before_resume = support::root_files(&data_root);
    let resumed = package_convert(&PackageConversionRequest {
        data_root: &data_root,
        work_root: &work_root,
        package_store: &store_root,
        package: None,
        payload: None,
        index: None,
        index_public_keys: None,
        writers_stopped: true,
        resume: true,
        stop: None,
    })
    .expect("the resume continues");

    assert_eq!(resumed.status, "converted");
    assert!(resumed.resumed);
    assert_eq!(resumed.attempts, 2);
    assert!(
        resumed.staged_source_reused,
        "an intact staged copy is reused"
    );
    assert_eq!(resumed.converted_records, Some(3));
    assert_eq!(support::root_files(&data_root), before_resume);

    // The converter was continued, not started over: its own record of the two
    // attempts is appended to, and the marker the interrupted attempt wrote is still
    // the one on disk.
    let log = fs::read_to_string(work_root.join("target/attempts.log")).expect("attempt log");
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 2, "one line per invocation: {log:?}");
    assert!(lines[0].ends_with("resume=false"), "{log:?}");
    assert!(
        lines[1].ends_with("resume=true"),
        "the resume is declared to the converter: {log:?}"
    );
    assert_eq!(
        fs::read_to_string(work_root.join("target/started")).expect("first attempt marker"),
        "started\n",
        "the first attempt's target was never cleared"
    );
}

#[test]
fn a_converter_that_reports_an_incomplete_conversion_leaves_the_run_unfinished() {
    let root = TestRoot::new("partial");
    let store_root = root.path().join("store");
    let data_root = root.path().join("data");
    let work_root = root.path().join("work");
    seed_data_root(&data_root, "partial");
    install_package(
        &store_root,
        &manifest(
            "org.licoland.fixture.converter",
            "1.4.0",
            "bin/converter",
            Some(skipped_release_declaration()),
        ),
    );

    let report = package_convert(&PackageConversionRequest {
        data_root: &data_root,
        work_root: &work_root,
        package_store: &store_root,
        package: None,
        payload: None,
        index: None,
        index_public_keys: None,
        writers_stopped: true,
        resume: false,
        stop: None,
    })
    .expect("an incomplete conversion is an outcome");

    assert_eq!(report.status, "partial");
    assert!(!report.is_complete());
    let converter = report.converter.as_ref().expect("the converter ran");
    assert!(!converter.complete);
    assert_eq!(
        converter.reason.as_deref(),
        Some(licoup_migrate::error::CONVERTER_RESULT_INCOMPLETE.code())
    );
    let step = report
        .steps
        .iter()
        .find(|step| step.step == "runConverter")
        .expect("the converter step");
    assert_eq!(step.status, "pending", "an incomplete run stays owed");
}

#[test]
fn a_converter_that_fails_once_is_continued_by_the_next_resume() {
    let root = TestRoot::new("fail-once");
    let store_root = root.path().join("store");
    let data_root = root.path().join("data");
    let work_root = root.path().join("work");
    seed_data_root(&data_root, "fail-once");
    install_package(
        &store_root,
        &manifest(
            "org.licoland.fixture.converter",
            "1.4.0",
            "bin/converter",
            Some(skipped_release_declaration()),
        ),
    );

    let failed = package_convert(&PackageConversionRequest {
        data_root: &data_root,
        work_root: &work_root,
        package_store: &store_root,
        package: None,
        payload: None,
        index: None,
        index_public_keys: None,
        writers_stopped: true,
        resume: false,
        stop: None,
    })
    .expect("a failing converter is an outcome");
    assert_eq!(failed.status, "partial");
    assert_eq!(
        failed
            .converter
            .as_ref()
            .and_then(|converter| converter.exit_code),
        Some(3)
    );

    let resumed = package_convert(&PackageConversionRequest {
        data_root: &data_root,
        work_root: &work_root,
        package_store: &store_root,
        package: None,
        payload: None,
        index: None,
        index_public_keys: None,
        writers_stopped: true,
        resume: true,
        stop: None,
    })
    .expect("the resume continues");
    assert_eq!(resumed.status, "converted");
    assert_eq!(resumed.attempts, 2);
}

#[test]
fn a_converter_that_writes_inside_the_source_copy_is_refused() {
    let root = TestRoot::new("write-source");
    let store_root = root.path().join("store");
    let data_root = root.path().join("data");
    let work_root = root.path().join("work");
    seed_data_root(&data_root, "write-source");
    install_package(
        &store_root,
        &manifest(
            "org.licoland.fixture.converter",
            "1.4.0",
            "bin/converter",
            Some(skipped_release_declaration()),
        ),
    );

    let error = refused(package_convert(&PackageConversionRequest {
        data_root: &data_root,
        work_root: &work_root,
        package_store: &store_root,
        package: None,
        payload: None,
        index: None,
        index_public_keys: None,
        writers_stopped: true,
        resume: false,
        stop: None,
    }));
    assert_eq!(error.code(), CONVERTER_MODIFIED_SOURCE.code());
    assert!(
        !data_root.join("intruder.txt").exists(),
        "the real source root is never reachable from the staged copy"
    );
}

#[test]
fn unfinished_local_work_and_a_held_admission_barrier_each_refuse_a_conversion() {
    let root = TestRoot::new("admission");
    let store_root = root.path().join("store");
    let data_root = root.path().join("data");
    let work_root = root.path().join("work");
    seed_data_root(&data_root, "");
    install_package(
        &store_root,
        &manifest(
            "org.licoland.fixture.converter",
            "1.4.0",
            "bin/converter",
            Some(skipped_release_declaration()),
        ),
    );
    let request = || PackageConversionRequest {
        data_root: &data_root,
        work_root: &work_root,
        package_store: &store_root,
        package: None,
        payload: None,
        index: None,
        index_public_keys: None,
        writers_stopped: true,
        resume: false,
        stop: None,
    };

    // Unfinished locally owned work: the host's own decision owner reports it from
    // the canonical store, and the conversion is refused before it stages anything.
    let conversation =
        licoup_conversation::ConversationStore::open(&data_root).expect("conversation store");
    let scope = conversation
        .prepare_runtime_dispatch(
            "synthetic",
            "synthetic-session",
            "synthetic request",
            None,
            None,
            None,
            None,
        )
        .expect("dispatch");
    conversation
        .finalize_event(&scope.event_id)
        .expect("finalize");
    // The fixture's own dispatch is settled first, so the only unfinished work left
    // is the queued dispatch that blocks maintenance.
    conversation
        .update_dispatch(
            &scope.dispatch_id,
            licoup_conversation::DispatchState::Running,
            None,
            None,
        )
        .expect("running");
    conversation
        .update_dispatch(
            &scope.dispatch_id,
            licoup_conversation::DispatchState::Completed,
            None,
            None,
        )
        .expect("settled");
    let queued = conversation
        .create_dispatch(
            &scope.conversation_id,
            &scope.membership_id,
            "send",
            licoup_conversation::DispatchSessionMode::New,
        )
        .expect("queued dispatch");

    assert_eq!(
        refused(package_convert(&request())).code(),
        MAINTENANCE_WORK_UNFINISHED.code()
    );
    assert!(
        !work_root.exists(),
        "a refused conversion stages nothing at all"
    );

    // Settling the work reopens the decision, and the conversion runs.
    conversation
        .update_dispatch(
            &queued.id,
            licoup_conversation::DispatchState::Cancelled,
            None,
            None,
        )
        .expect("settle");
    drop(conversation);
    let converted = package_convert(&request()).expect("the conversion runs when the host is idle");
    assert!(converted.is_complete());

    // The other maintenance owner's barrier closes admission for everyone else.
    let barrier = licoup_native::domain::work_admission::WorkAdmission::open(&data_root)
        .begin_maintenance(
            licoup_native::domain::work_admission::MaintenanceOperation::ClientReplacement,
        )
        .expect("begin maintenance");
    assert!(barrier.barrier().is_some());
    let second_work_root = root.path().join("work-2");
    let closed = refused(package_convert(&PackageConversionRequest {
        work_root: &second_work_root,
        ..request()
    }));
    assert_eq!(closed.code(), MAINTENANCE_ADMISSION_CLOSED.code());
    assert!(!second_work_root.exists());
}

#[test]
fn the_documented_protocol_reaches_a_real_native_converter_without_an_interpreter() {
    let root = TestRoot::new("protocol");
    let store_root = root.path().join("store");
    let data_root = root.path().join("data");
    let work_root = root.path().join("work");
    seed_data_root(&data_root, "");
    install_package(
        &store_root,
        &manifest(
            "org.licoland.fixture.converter",
            "1.4.0",
            "bin/converter",
            Some(skipped_release_declaration()),
        ),
    );

    let report = package_convert(&PackageConversionRequest {
        data_root: &data_root,
        work_root: &work_root,
        package_store: &store_root,
        package: None,
        payload: None,
        index: None,
        index_public_keys: None,
        writers_stopped: true,
        resume: false,
        stop: None,
    })
    .expect("the run converts");

    assert!(report.is_complete());
    let result: Value = serde_json::from_str(
        &fs::read_to_string(work_root.join("result.json")).expect("result document"),
    )
    .expect("result json");
    assert_eq!(result["schema"], "licoup.package-conversion-result.v1");
    assert_eq!(result["sourceFormat"], required().source_format());
    assert_eq!(result["targetFormat"], required().target_format());
    assert_eq!(result["complete"], true);
    assert_eq!(result["convertedRecords"], 3);
}

#[test]
fn the_cli_reports_the_inventory_and_the_conversion_under_the_host_identity() {
    let root = TestRoot::new("cli");
    let home = root.path().join("home");
    let store_root = root.path().join("store");
    let data_root = root.path().join("data");
    let work_root = root.path().join("work");
    seed_data_root(&data_root, "");
    install_package(
        &store_root,
        &manifest(
            "org.licoland.fixture.converter",
            "1.4.0",
            "bin/converter",
            Some(skipped_release_declaration()),
        ),
    );

    let store = store_root.to_string_lossy().to_string();
    let data = data_root.to_string_lossy().to_string();
    let work = work_root.to_string_lossy().to_string();

    // The read-only inventory reports the candidate and the host's own decision.
    let (status, report) = support::run_tool_in_home(
        &home,
        &[
            "converters",
            "--package-store",
            &store,
            "--data-root",
            &data,
        ],
    );
    assert_eq!(status, 0, "{report}");
    assert_eq!(report["status"], "inventory");
    assert_eq!(
        report["selected"]["packageId"],
        "org.licoland.fixture.converter"
    );
    assert_eq!(report["admission"]["decision"], "idle");

    // The conversion itself runs through the shipped binary, needing no client.
    let (status, report) = support::run_tool_in_home(
        &home,
        &[
            "package-convert",
            "--data-root",
            &data,
            "--work-root",
            &work,
            "--package-store",
            &store,
            "--writers-stopped",
        ],
    );
    assert_eq!(status, 0, "{report}");
    assert_eq!(report["status"], "converted");
    assert_eq!(report["sourceUntouched"], true);
    assert_eq!(report["attempts"], 1);

    // Without the operator's statement the run never starts.
    let (status, report) = support::run_tool_in_home(
        &home,
        &[
            "package-convert",
            "--data-root",
            &data,
            "--work-root",
            &work,
            "--package-store",
            &store,
        ],
    );
    assert_eq!(status, 2, "{report}");
    assert_eq!(report["error"], "maintenance_confirmation_required");

    // And a store with no declared converter is refused explicitly.
    let empty_store = root.path().join("empty-store");
    fs::create_dir_all(&empty_store).expect("empty store");
    let empty = empty_store.to_string_lossy().to_string();
    let empty_work = root.path().join("work-empty");
    let empty_work_text = empty_work.to_string_lossy().to_string();
    let (status, report) = support::run_tool_in_home(
        &home,
        &[
            "package-convert",
            "--data-root",
            &data,
            "--work-root",
            &empty_work_text,
            "--package-store",
            &empty,
            "--writers-stopped",
        ],
    );
    assert_eq!(status, 1, "{report}");
    assert_eq!(report["error"], CONVERTER_UNAVAILABLE.code());
    assert_eq!(report["admission"]["decision"], "idle");
}

#[test]
fn an_offline_payload_is_verified_through_the_store_and_the_signed_release_index() {
    let root = TestRoot::new("payload");
    let store_root = root.path().join("store");
    let data_root = root.path().join("data");
    let work_root = root.path().join("work");
    seed_data_root(&data_root, "");
    fs::create_dir_all(&store_root).expect("store root");
    let document = manifest(
        "org.licoland.fixture.converter",
        "1.4.0",
        "bin/converter",
        Some(skipped_release_declaration()),
    );
    let bytes = payload_bytes(&document);
    let payload_path = root.path().join("fixture.licopkg");
    fs::write(&payload_path, &bytes).expect("payload");
    let (index_path, keys_path) = signed_index(
        &root,
        "org.licoland.fixture.converter",
        "1.4.0",
        &bytes,
        required().source_format(),
        required().target_format(),
    );

    let report = package_convert(&PackageConversionRequest {
        data_root: &data_root,
        work_root: &work_root,
        package_store: &store_root,
        package: None,
        payload: Some(&payload_path),
        index: Some(&index_path),
        index_public_keys: Some(&keys_path),
        writers_stopped: true,
        resume: false,
        stop: None,
    })
    .expect("the offline payload is imported and converted");

    assert!(report.is_complete());
    assert!(report.index_verified);
    let store = PackageStore::open(&store_root).expect("store");
    let installed = store
        .installed_version("org.licoland.fixture.converter", "1.4.0")
        .expect("read")
        .expect("installed");
    assert_eq!(installed.digest, content_digest(&bytes));

    // Payload bytes that are not the bytes the signed index describes are refused
    // before anything is imported.
    let tampered = root.path().join("tampered.licopkg");
    let mut modified = bytes.clone();
    let last = modified.len() - 1;
    modified[last] ^= 0xff;
    fs::write(&tampered, &modified).expect("tampered payload");
    let fresh_store = root.path().join("store-tampered");
    fs::create_dir_all(&fresh_store).expect("fresh store");
    let fresh_work = root.path().join("work-tampered");
    let error = refused(package_convert(&PackageConversionRequest {
        data_root: &data_root,
        work_root: &fresh_work,
        package_store: &fresh_store,
        package: None,
        payload: Some(&tampered),
        index: Some(&index_path),
        index_public_keys: Some(&keys_path),
        writers_stopped: true,
        resume: false,
        stop: None,
    }));
    assert_eq!(error.code(), PACKAGE_PAYLOAD_INVALID.code());
    assert!(
        PackageStore::open(&fresh_store)
            .expect("store")
            .installed()
            .expect("installed")
            .is_empty(),
        "a refused payload publishes nothing"
    );
}

#[test]
fn a_payload_that_prereleases_an_unusable_converter_is_refused_by_the_store() {
    let root = TestRoot::new("payload-refused");
    let store_root = root.path().join("store");
    let data_root = root.path().join("data");
    let work_root = root.path().join("work");
    seed_data_root(&data_root, "");
    fs::create_dir_all(&store_root).expect("store root");

    // The payload's own manifest declares a converter kind the contract does not
    // publish, so the store's import refuses it.
    let mut document = manifest(
        "org.licoland.fixture.converter",
        "1.4.0",
        "bin/converter",
        None,
    );
    document["conversion"] = json!({
        "kind": "node-module",
        "entry": "bin/converter",
        "sourceFormats": ["licoup-state-0.1.1"],
        "targetFormat": "licoup-state-0.3.0",
    });
    let bytes = payload_bytes(&document);
    let payload_path = root.path().join("interpreter.licopkg");
    fs::write(&payload_path, &bytes).expect("payload");

    let error = refused(package_convert(&PackageConversionRequest {
        data_root: &data_root,
        work_root: &work_root,
        package_store: &store_root,
        package: None,
        payload: Some(&payload_path),
        index: None,
        index_public_keys: None,
        writers_stopped: true,
        resume: false,
        stop: None,
    }));
    assert_eq!(
        error.code(),
        licoup_migrate::error::CONVERTER_NOT_NATIVE.code(),
        "the payload's own declaration is what refuses it"
    );
}

/// Build a signed release index and its public key catalogue for one payload.
fn signed_index(
    root: &TestRoot,
    package_id: &str,
    version: &str,
    bytes: &[u8],
    source_format: &str,
    target_format: &str,
) -> (PathBuf, PathBuf) {
    use base64::Engine as _;
    use ed25519_dalek::Signer;

    let mut secret = [0_u8; 32];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut secret);
    let offline = ed25519_dalek::SigningKey::from_bytes(&secret);
    let mut online_secret = [0_u8; 32];
    rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut online_secret);
    let online = ed25519_dalek::SigningKey::from_bytes(&online_secret);

    let mut document = json!({
        "schemaVersion": licoup_native::platform::extension_packages::PACKAGE_INDEX_SCHEMA,
        "releaseTrack": "stable",
        "packages": [{
            "packageId": package_id,
            "displayName": "Synthetic converter",
            "packageVersion": version,
            "hostProtocol": { "major": 1, "minimumMinor": 0 },
            "clientCompatibility": { "kind": "range", "range": ">=0.0.1-alpha, <99.0.0" },
            "converter": {
                "kind": "native-executable",
                "entry": "bin/converter",
                "sourceFormat": source_format,
                "targetFormat": target_format,
            },
            "payload": {
                "fileName": "fixture.licopkg",
                "byteSize": bytes.len(),
                "sha256": content_digest(bytes),
            },
        }],
        "signaturePolicy": {
            "offlineRootKeyId": "fixture-offline-root",
            "onlineSigningKeyId": "fixture-online-signing",
        },
    });
    let payload = canonical_unsigned_bytes(&document);
    let encode = |key: &ed25519_dalek::SigningKey| {
        base64::engine::general_purpose::STANDARD.encode(key.sign(&payload).to_bytes())
    };
    document["signatures"] = json!([
        { "keyId": "fixture-offline-root", "algorithm": "Ed25519", "signature": encode(&offline) },
        { "keyId": "fixture-online-signing", "algorithm": "Ed25519", "signature": encode(&online) },
    ]);
    let index_path = root.join("index.json");
    fs::write(
        &index_path,
        serde_json::to_string_pretty(&document).expect("index json"),
    )
    .expect("index document");
    let keys_path = root.join("keys.json");
    let key_text = |key: &ed25519_dalek::SigningKey| {
        base64::engine::general_purpose::STANDARD.encode(key.verifying_key().to_bytes())
    };
    fs::write(
        &keys_path,
        json!({
            "keys": {
                "fixture-offline-root": { "publicKey": key_text(&offline) },
                "fixture-online-signing": { "publicKey": key_text(&online) },
            }
        })
        .to_string(),
    )
    .expect("key catalogue");
    (index_path, keys_path)
}
