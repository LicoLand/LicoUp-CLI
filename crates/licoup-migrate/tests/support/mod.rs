//! Shared support for the standalone migration suites.
//!
//! The released root is seeded from the frozen shared fixture (`v0.2.1`), never from a
//! hand-written approximation: the ledger, markers, released Conversation store and
//! released strategy store are the shapes the last published producer wrote. The suites
//! that convert it must run under the planned candidate identity, because the released
//! ledger records product high-water `0.2.1` and the client's own guard refuses an older
//! binary. That identity is constructed with the native build script's real
//! `LICO_CLIENT_PRODUCT_VERSION` input by `tools/scripts/migration-crate-tests.mjs`; no
//! fixture ledger or high-water is ever lowered for a development build.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use licoup_foundation::platform::file_security::{atomic_write_private_text, ensure_private_dir};
use rusqlite::Connection;

include!("../../../../tests/fixtures/client_state_migration/released_source.rs");

/// The planned release identity the delivered tool is built with.
pub const CANDIDATE_PRODUCT_VERSION: &str = "0.3.0";

/// Refuse to run a released-root conversion under a development identity.
pub fn assert_candidate_identity() {
    let running = licoup_native::domain::client_state_migration::running_product_version()
        .expect("the embedded product identity is valid");
    assert_eq!(
        running, CANDIDATE_PRODUCT_VERSION,
        "these suites convert the frozen v0.2.1 root and must run under the planned candidate \
         identity {CANDIDATE_PRODUCT_VERSION}, not {running}; run them through \
         tools/scripts/migration-crate-tests.mjs, which constructs that identity with the native \
         build script's own LICO_CLIENT_PRODUCT_VERSION input"
    );
}

/// One disposable fixture root, removed when the test ends.
pub struct TestRoot {
    path: PathBuf,
}

impl TestRoot {
    pub fn new(label: &str) -> Self {
        let base = std::env::temp_dir()
            .canonicalize()
            .expect("canonical temporary directory");
        let path = base.join(format!("licoup-migrate-{}-{label}", std::process::id()));
        if path.exists() {
            fs::remove_dir_all(&path).expect("clear a stale fixture root");
        }
        fs::create_dir_all(&path).expect("create the fixture root");
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn join(&self, relative: &str) -> PathBuf {
        self.path.join(relative)
    }

    /// The released-format source root this fixture seeds.
    pub fn released_source(&self) -> PathBuf {
        self.path.join("source")
    }

    /// The disposable work directory a rehearsal stages in.
    pub fn work(&self) -> PathBuf {
        self.path.join("work")
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub fn write_file(root: &Path, relative: &str, bytes: &[u8]) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("fixture parent")).expect("fixture directory");
    fs::write(&path, bytes).expect("fixture file");
}

fn write_json_atomic(path: &Path, value: &serde_json::Value) {
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent).expect("private document directory");
    }
    atomic_write_private_text(
        path,
        &serde_json::to_string(value).expect("encode document"),
    )
    .expect("private document write");
}

/// Materialize the frozen released root through its releasing producers' own layouts.
pub fn seed_released_root(root: &Path) {
    seed_released_conversation_store(root);
    seed_released_strategy_store(&root.join(RELEASED_STRATEGY_DATABASE));
    ensure_private_dir(&root.join("client-state/migrations/domain-state")).expect("marker dir");
    for (relative, content) in released_root_files() {
        let path = root.join(&relative);
        if relative == RELEASED_CONVERSATION_COMPLETION {
            fs::write(&path, content).expect("completion marker");
            continue;
        }
        let document: serde_json::Value =
            serde_json::from_str(&content).expect("released document");
        write_json_atomic(&path, &document);
    }
}

fn seed_released_conversation_store(root: &Path) {
    let database = root.join(RELEASED_CONVERSATION_DATABASE);
    fs::create_dir_all(database.parent().expect("database parent")).expect("database directory");
    let connection = Connection::open(&database).expect("open released conversation store");
    connection
        .execute_batch(RELEASED_CONVERSATION_SCHEMA)
        .expect("released conversation layout");
    connection
        .execute_batch(RELEASED_CONVERSATION_ROWS)
        .expect("released conversation rows");
}

pub fn seed_released_strategy_store(path: &Path) {
    fs::create_dir_all(path.parent().expect("database parent")).expect("database directory");
    let connection = Connection::open(path).expect("open released strategy store");
    connection
        .execute_batch(RELEASED_STRATEGY_SCHEMA)
        .expect("released strategy layout");
    connection
        .execute_batch(&released_strategy_rows())
        .expect("released strategy rows");
}

/// Every regular file below one root, keyed by relative path.
pub fn root_files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut entries = BTreeMap::new();
    collect_files(root, root, &mut entries);
    entries
}

fn collect_files(root: &Path, directory: &Path, entries: &mut BTreeMap<String, Vec<u8>>) {
    let Ok(read) = fs::read_dir(directory) else {
        return;
    };
    let mut children: Vec<PathBuf> = read
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    children.sort();
    for child in children {
        let metadata = fs::symlink_metadata(&child).expect("fixture metadata");
        let relative = child
            .strip_prefix(root)
            .expect("fixture entry inside its root")
            .to_string_lossy()
            .replace('\\', "/");
        if metadata.is_dir() {
            collect_files(root, &child, entries);
        } else if metadata.is_file() {
            entries.insert(relative, fs::read(&child).expect("readable fixture file"));
        }
    }
}

/// The standalone tool binary under test.
pub fn tool_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_licoup-migrate"))
}

/// Run the standalone tool and parse its single JSON report.
pub fn run_tool(arguments: &[&str]) -> (i32, serde_json::Value) {
    static NEXT_HOME: AtomicUsize = AtomicUsize::new(0);
    let home = TestRoot::new(&format!(
        "tool-home-{}",
        NEXT_HOME.fetch_add(1, Ordering::Relaxed)
    ));
    run_tool_in_home(home.path(), arguments)
}

/// Use one explicitly owned home when testing coordination across processes.
pub fn run_tool_in_home(home: &Path, arguments: &[&str]) -> (i32, serde_json::Value) {
    let output = isolated_command(tool_binary(), home)
        .args(arguments)
        .output()
        .expect("run the tool");
    report_of(output)
}

/// Neither tool may inherit a developer's locator, custody or service environment.
pub fn isolated_command(binary: PathBuf, home: &Path) -> Command {
    fs::create_dir_all(home).expect("isolated process home");
    let mut command = Command::new(binary);
    command
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("APPDATA", home.join("AppData/Roaming"))
        .env("LOCALAPPDATA", home.join("AppData/Local"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env_remove("LICOUP_HOME")
        .env_remove("LICOUP_PORTABLE_DIR")
        .env_remove("RUST_LOG")
        .env_remove("RUST_BACKTRACE")
        .env("LICO_MOBILE_RELAY_NATIVE_SECRET_STORE", "disabled")
        .env("LICOUP_MCP_AUTO_START", "0");
    command
}

/// The client CLI binary that backs the cross-container oracle.
///
/// `cargo test` only exports `CARGO_BIN_EXE_*` for the package's own binaries, so the
/// real `licoup-cli` path is handed over by `tools/scripts/migration-crate-tests.mjs`,
/// which builds it in the same target directory under the same identity.
pub fn client_cli_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("LICOUP_MIGRATE_CLIENT_CLI") {
        return PathBuf::from(path);
    }
    let sibling = tool_binary()
        .parent()
        .expect("tool binary parent")
        .join("licoup-cli");
    assert!(
        sibling.is_file(),
        "the cross-container oracle needs the real licoup-cli binary; run it through \
         tools/scripts/migration-crate-tests.mjs (expected at {})",
        sibling.display()
    );
    sibling
}

/// Run the client CLI with an isolated home, as the CLI contract cases do.
pub fn run_client_cli(home: &Path, arguments: &[&str]) -> (i32, serde_json::Value) {
    let output = isolated_command(client_cli_binary(), home)
        .args(arguments)
        .output()
        .expect("run the client CLI");
    assert!(
        output.status.success(),
        "the client CLI must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    report_of(output)
}

fn report_of(output: Output) -> (i32, serde_json::Value) {
    let stdout = String::from_utf8(output.stdout).expect("utf-8 report");
    let report: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("one JSON report per invocation");
    (output.status.code().unwrap_or(-1), report)
}

/// Save the selected data-home locator for an isolated home, exactly as the CLI reads it.
pub fn save_data_home_locator(home: &Path, root: &Path) {
    let locator = licoup_foundation::platform::paths::data_home_locator_path_for(
        licoup_foundation::platform::paths::DataHomePlatform::current(),
        home,
        Some(home.join("AppData/Roaming").into_os_string()),
        Some(home.join(".config").into_os_string()),
    );
    fs::create_dir_all(locator.parent().expect("locator parent")).expect("locator directory");
    fs::write(&locator, format!("{}\n", root.display())).expect("locator document");
}
