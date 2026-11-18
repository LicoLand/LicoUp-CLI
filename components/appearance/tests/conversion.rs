//! The appearance converter, driven over synthetic roots.
//!
//! Every fixture here is synthetic: a hand-seeded data root at the declared source
//! format, not the frozen released-root fixture. That is the point of the suite — the
//! package's converter must convert a supported input, resume an interrupted one exactly
//! once, and refuse an unsupported one, through the client's own migration coordinator
//! rather than through a private copy of it.
//!
//! The oracle is the client's own record. The suite reads the converted document, the
//! client's ledger and the file metadata the conversion produced; it never re-derives a
//! veredict from this package's opinion of itself.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use licoup_foundation::platform::file_security::{atomic_write_private_text, ensure_private_dir};
use serde_json::{Value, json};

const DECLARATION: &str = env!("CARGO_MANIFEST_DIR");
const DOMAIN: &str = "appearance-presentation";
const DOMAIN_STEP: &str = "appearance-presentation.absent-to-1";
const APPEARANCE: &str = "client-state/appearance-preferences.json";
const LEDGER: &str = "client-state/migrations/ledger.json";
const MARKER: &str = "client-state/migrations/domain-state/appearance-presentation.json";
const LEDGER_SCHEMA: &str = "v0.0.1:client-state-migration-ledger-1";

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// One disposable synthetic root, removed when the test ends.
struct Root {
    path: PathBuf,
}

impl Root {
    fn new(label: &str) -> Self {
        let base = std::env::temp_dir()
            .canonicalize()
            .expect("canonical temporary directory");
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = base.join(format!(
            "licoup-appearance-{}-{label}-{unique}",
            std::process::id()
        ));
        if path.exists() {
            fs::remove_dir_all(&path).expect("clear a stale root");
        }
        fs::create_dir_all(&path).expect("create the root");
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

    fn read(&self, relative: &str) -> Value {
        serde_json::from_slice(&fs::read(self.join(relative)).expect("read the document"))
            .unwrap_or_else(|_| panic!("{relative} is JSON"))
    }

    /// Every file under the root with its bytes, for a preservation comparison.
    fn fingerprint(&self) -> BTreeMap<String, Vec<u8>> {
        let mut files = BTreeMap::new();
        collect(self.path(), self.path(), &mut files);
        files
    }

    fn appearance_metadata(&self) -> (Vec<u8>, std::time::SystemTime) {
        let path = self.join(APPEARANCE);
        let metadata = fs::metadata(&path).expect("the appearance document");
        (
            fs::read(&path).expect("read the appearance document"),
            metadata.modified().expect("modification time"),
        )
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn collect(root: &Path, directory: &Path, files: &mut BTreeMap<String, Vec<u8>>) {
    for entry in fs::read_dir(directory).expect("read the directory") {
        let entry = entry.expect("an entry");
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, files);
        } else {
            let relative = path
                .strip_prefix(root)
                .expect("a path under the root")
                .to_string_lossy()
                .into_owned();
            files.insert(relative, fs::read(&path).expect("read the file"));
        }
    }
}

/// One converter invocation and the report it printed.
struct Run {
    code: Option<i32>,
    report: Value,
}

impl Run {
    fn status(&self) -> &str {
        self.report["status"].as_str().expect("a status")
    }

    fn code_of(&self, field: &str) -> &str {
        self.report[field].as_str().unwrap_or("")
    }
}

fn run(root: &Path) -> Run {
    let output = Command::new(env!("CARGO_BIN_EXE_licoup-appearance-convert"))
        .arg("--data-root")
        .arg(root)
        .output()
        .expect("run the converter entry");
    assert!(
        output.stderr.is_empty(),
        "a conversion prints no diagnostics: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "one JSON report is printed: {}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    Run {
        code: output.status.code(),
        report,
    }
}

/// The published pair, read from the client's own catalogue.
fn endpoints() -> (String, String) {
    let declared = licoup_appearance::declaration::endpoints().expect("the catalogue declares");
    (
        declared.source_format().to_owned(),
        declared.target_format().to_owned(),
    )
}

/// A synthetic supported source: the appearance store as the last published release
/// wrote it — an object with no `schemaVersion` — carrying user preferences.
fn seed_supported(root: &Root) -> Value {
    let source = json!({
        "appearancePresetId": "preset-canary",
        "fontPreferenceId": "system-default",
        "localePreference": "zh-CN"
    });
    root.seed(APPEARANCE, &source);
    source
}

#[test]
fn a_supported_synthetic_source_converts_and_preserves_its_user_content() {
    let root = Root::new("convert");
    let source = seed_supported(&root);

    let run = run(root.path());

    assert_eq!(
        (run.status(), run.code),
        ("converted", Some(0)),
        "the appearance domain reaches its target: {:?}",
        run.report
    );
    assert_eq!(run.code_of("domainId"), DOMAIN);
    let (declared_source, declared_target) = endpoints();
    assert_eq!(run.code_of("sourceFormat"), declared_source);
    assert_eq!(run.code_of("targetFormat"), declared_target);

    // The client's own store carries the target shape, and every seeded value is still
    // there under the identity it was seeded with.
    let converted = root.read(APPEARANCE);
    assert_eq!(converted["schemaVersion"], json!(1));
    for field in ["appearancePresetId", "fontPreferenceId", "localePreference"] {
        assert_eq!(converted[field], source[field], "{field} survives");
    }

    // The client's own ledger recorded the one step, once.
    let ledger = root.read(LEDGER);
    assert_eq!(ledger["schemaVersion"], json!(LEDGER_SCHEMA));
    assert_eq!(
        ledger["domains"][DOMAIN]["completedStepIds"],
        json!([DOMAIN_STEP])
    );
    assert_eq!(ledger["domains"][DOMAIN]["schemaVersion"], json!(1));
}

#[test]
fn an_interrupted_conversion_resumes_to_exactly_one_result() {
    let root = Root::new("resume");
    seed_supported(&root);
    assert_eq!(run(root.path()).status(), "converted");
    let (committed, committed_at) = root.appearance_metadata();

    // The interruption this reproduces is the window the coordinator's recovery rule is
    // written for: the store committed its target shape and the process stopped before
    // the domain marker and the ledger entry were written.
    fs::remove_file(root.join(MARKER)).expect("drop the marker the crash never wrote");
    root.seed(
        LEDGER,
        &json!({
            "schemaVersion": LEDGER_SCHEMA,
            "highestAdmittedProductVersion": "0.0.0",
            "frontierId": endpoints().1,
            "domains": { DOMAIN: { "schemaVersion": 0, "completedStepIds": [] } }
        }),
    );

    let resumed = run(root.path());

    assert_eq!(
        (resumed.status(), resumed.code),
        ("already-current", Some(0)),
        "a committed store is authoritative over the stale bookkeeping: {:?}",
        resumed.report
    );
    // Exactly once: the move is not applied a second time, so the document the first run
    // produced is the document the resume leaves — same bytes, same modification time.
    let (after, after_at) = root.appearance_metadata();
    assert_eq!(after, committed, "the resume does not rewrite the store");
    assert_eq!(
        after_at, committed_at,
        "the resume does not touch the store"
    );

    let ledger = root.read(LEDGER);
    assert_eq!(
        ledger["domains"][DOMAIN]["completedStepIds"],
        json!([DOMAIN_STEP]),
        "the resume reconciles the completed step exactly once"
    );
    assert_eq!(ledger["domains"][DOMAIN]["schemaVersion"], json!(1));
}

#[test]
fn an_unsupported_source_is_refused_with_a_stable_code_before_any_write() {
    let root = Root::new("unsupported");
    // A shape no release published: the client's own probe refuses it rather than
    // guessing what a future schema meant.
    root.seed(
        APPEARANCE,
        &json!({ "schemaVersion": 9, "appearancePresetId": "canary" }),
    );
    let before = root.fingerprint();

    let refused = run(root.path());

    assert_eq!(
        (refused.status(), refused.code),
        ("refused", Some(3)),
        "an unsupported source is never reported as converted: {:?}",
        refused.report
    );
    assert_eq!(refused.code_of("code"), "state_newer_than_binary");
    assert_eq!(
        refused.code_of("stage"),
        "probe",
        "the refusal happened before the owner was asked to move anything"
    );
    assert_eq!(
        root.fingerprint(),
        before,
        "a refused conversion opens nothing: the whole root is byte-identical"
    );
}

#[test]
fn a_root_whose_declared_format_is_not_a_published_endpoint_is_refused() {
    let root = Root::new("undeclared");
    seed_supported(&root);
    // A ledger that names a format between the two published endpoints. The client's own
    // admission refuses it before the high-water advances or any domain moves.
    root.seed(
        LEDGER,
        &json!({
            "schemaVersion": LEDGER_SCHEMA,
            "highestAdmittedProductVersion": "0.0.0",
            "frontierId": "licoup-state-0.0.9",
            "domains": {}
        }),
    );
    let source_before = fs::read(root.join(APPEARANCE)).expect("read the source");
    let ledger_before = fs::read(root.join(LEDGER)).expect("read the ledger");

    let refused = run(root.path());

    assert_eq!(
        (refused.status(), refused.code),
        ("refused", Some(3)),
        "{:?}",
        refused.report
    );
    assert_eq!(refused.code_of("code"), "unsupported_state_shape");
    assert_eq!(refused.code_of("stage"), "owner");
    assert_eq!(
        fs::read(root.join(APPEARANCE)).expect("read the source"),
        source_before,
        "the source document is preserved when the root is refused"
    );
    assert_eq!(
        fs::read(root.join(LEDGER)).expect("read the ledger"),
        ledger_before,
        "the refused run leaves the ledger it refused exactly as it was"
    );
}

#[test]
fn the_committed_manifest_is_the_document_this_package_publishes() {
    let committed =
        fs::read_to_string(Path::new(DECLARATION).join(licoup_appearance::MANIFEST_FILE))
            .expect("the committed manifest");
    assert_eq!(
        committed,
        licoup_appearance::declaration::manifest_json().expect("the published manifest"),
        "the committed manifest is stale; regenerate it with \
         `cargo run --manifest-path components/appearance/Cargo.toml -- --manifest`"
    );

    // The document is the contract's own declaration, and the pair it owns is the pair
    // the client's catalogue freezes.
    let document: Value = serde_json::from_str(&committed).expect("the manifest is JSON");
    let manifest =
        licoup_appearance::declaration::validate(&document).expect("the contract reads it");
    let (source, target) = endpoints();
    let owner = manifest
        .conversion_owner(&licoup_appearance::declaration::endpoints().expect("the catalogue"))
        .expect("this package owns the required conversion");
    assert_eq!(owner.source_formats, vec![source]);
    assert_eq!(owner.target_format, target);
    assert_eq!(owner.entry, licoup_appearance::ENTRY);
    assert_eq!(manifest.id, licoup_appearance::PACKAGE_ID);
}

#[test]
fn the_manifest_verb_prints_the_same_document_without_converting_anything() {
    let output = Command::new(env!("CARGO_BIN_EXE_licoup-appearance-convert"))
        .arg("--manifest")
        .output()
        .expect("run the manifest verb");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).expect("utf-8"),
        licoup_appearance::declaration::manifest_json().expect("the published manifest")
    );
}
