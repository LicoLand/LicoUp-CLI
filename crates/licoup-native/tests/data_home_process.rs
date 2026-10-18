//! Synthetic process-level coverage for the dedicated data-home RPC owner.
#![cfg(unix)]

use fs2::FileExt as _;
use rusqlite::Connection;
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Read as _, Write as _},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const CLI: &str = env!("CARGO_BIN_EXE_licoup-cli");

#[test]
fn dedicated_process_copies_wal_and_commits_the_saved_root_without_removing_source() {
    let fixture = std::env::temp_dir().join(format!(
        "licoup-data-home-process-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let home = fixture.join("home");
    let source = home.join(".lico-up");
    let destination_parent = fixture.join("destination");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&destination_parent).unwrap();
    let destination = destination_parent.canonicalize().unwrap().join("LicoUp");
    save_fixture_locator(&home, &source);

    let database = source.join("conversations.sqlite3");
    let connection = Connection::open(&database).unwrap();
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .unwrap();
    connection
        .execute_batch("CREATE TABLE records (id INTEGER PRIMARY KEY, value TEXT NOT NULL);")
        .unwrap();
    connection
        .execute(
            "INSERT INTO records(value) VALUES (?1)",
            ["committed in wal"],
        )
        .unwrap();
    assert!(Path::new(&format!("{}-wal", database.display())).is_file());

    let result = invoke_data_home_rpc(
        &home,
        json!({
            "method": "data.home.relocate",
            "params": {
                "destinationParent": destination_parent,
                "confirmed": true
            }
        }),
    );

    assert_eq!(
        result["ok"], true,
        "unexpected relocation response: {result}"
    );
    assert_eq!(result["result"]["status"], "relocated");
    assert_eq!(
        result["result"]["dataHome"],
        destination.display().to_string()
    );
    assert_eq!(
        result["result"]["previousDataHome"],
        source.display().to_string()
    );
    assert!(source.is_dir());
    assert_eq!(
        fs::read(locator_path(&home)).unwrap(),
        format!("{}\n", destination.display()).as_bytes()
    );
    assert!(
        destination
            .join("client-state/data-home-previous-root")
            .is_file()
    );

    let copied = Connection::open(destination.join("conversations.sqlite3")).unwrap();
    let value: String = copied
        .query_row("SELECT value FROM records WHERE id = 1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(value, "committed in wal");

    drop(copied);
    drop(connection);
    fs::remove_dir_all(fixture).unwrap();
}

#[test]
fn copy_failure_preserves_the_source_locator_and_discards_the_destination() {
    use std::os::unix::ffi::OsStrExt as _;

    let fixture = std::env::temp_dir().join(format!(
        "licoup-data-home-copy-failure-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let home = fixture.join("home");
    let source = home.join(".lico-up");
    let destination_parent = fixture.join("destination");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&destination_parent).unwrap();
    fs::write(source.join("retained.json"), b"source remains intact").unwrap();
    save_fixture_locator(&home, &source);
    let fifo = source.join("unsupported-pipe");
    let path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);

    let response = invoke_data_home_rpc_error(
        &home,
        json!({
            "method": "data.home.relocate",
            "params": {
                "destinationParent": destination_parent,
                "confirmed": true
            }
        }),
    );

    assert_eq!(
        response.pointer("/error/code"),
        Some(&json!("data_home_copy_unsupported_entry"))
    );
    assert_eq!(
        fs::read(locator_path(&home)).unwrap(),
        format!("{}\n", source.display()).as_bytes()
    );
    assert_eq!(
        fs::read(source.join("retained.json")).unwrap(),
        b"source remains intact"
    );
    assert!(fifo.exists());
    assert!(!destination_parent.join("LicoUp").exists());
    assert_eq!(fs::read_dir(&destination_parent).unwrap().count(), 0);
    fs::remove_dir_all(fixture).unwrap();
}

#[test]
fn missing_saved_root_recovery_changes_only_the_locator() {
    let fixture = std::env::temp_dir().join(format!(
        "licoup-data-home-recovery-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let home = fixture.join("home");
    let missing_root = fixture.join("removed-volume/LicoUp");
    let replacement = fixture.join("recovered-root");
    fs::create_dir_all(replacement.join("client-state")).unwrap();
    save_fixture_locator(&home, &missing_root);
    fs::write(replacement.join("client-state/kept.json"), b"existing data").unwrap();
    assert!(!missing_root.exists());

    let result = invoke_data_home_rpc(
        &home,
        json!({
            "method": "data.home.recover",
            "params": {
                "dataHome": replacement,
                "confirmed": true
            }
        }),
    );

    assert_eq!(result["result"]["status"], "recovered");
    assert!(!missing_root.exists());
    assert_eq!(
        fs::read(locator_path(&home)).unwrap(),
        format!("{}\n", replacement.canonicalize().unwrap().display()).as_bytes()
    );
    assert_eq!(
        fs::read(replacement.join("client-state/kept.json")).unwrap(),
        b"existing data"
    );
    fs::remove_dir_all(fixture).unwrap();
}

#[test]
fn sequential_moves_retain_sources_and_record_only_the_immediate_previous_root() {
    let fixture = std::env::temp_dir().join(format!(
        "licoup-data-home-sequential-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let home = fixture.join("home");
    let source_a = home.join(".lico-up");
    let parent_b = fixture.join("destination-b");
    let parent_c = fixture.join("destination-c");
    fs::create_dir_all(source_a.join("client-state")).unwrap();
    fs::create_dir_all(&parent_b).unwrap();
    fs::create_dir_all(&parent_c).unwrap();
    fs::write(source_a.join("client-state/kept.json"), b"preserve me").unwrap();
    save_fixture_locator(&home, &source_a);
    let root_b = parent_b.canonicalize().unwrap().join("LicoUp");
    let root_c = parent_c.canonicalize().unwrap().join("LicoUp");

    let moved_to_b = invoke_data_home_rpc(
        &home,
        json!({
            "method": "data.home.relocate",
            "params": {"destinationParent": parent_b, "confirmed": true}
        }),
    );
    assert_eq!(
        moved_to_b["result"]["dataHome"],
        root_b.display().to_string()
    );
    let moved_to_c = invoke_data_home_rpc(
        &home,
        json!({
            "method": "data.home.relocate",
            "params": {"destinationParent": parent_c, "confirmed": true}
        }),
    );
    assert_eq!(
        moved_to_c["result"]["dataHome"],
        root_c.display().to_string()
    );

    assert_eq!(
        fs::read(source_a.join("client-state/kept.json")).unwrap(),
        b"preserve me"
    );
    assert_eq!(
        fs::read(root_b.join("client-state/kept.json")).unwrap(),
        b"preserve me"
    );
    assert_eq!(
        fs::read(root_c.join("client-state/kept.json")).unwrap(),
        b"preserve me"
    );
    assert_eq!(
        fs::read(root_b.join("client-state/data-home-previous-root")).unwrap(),
        format!("{}\n", source_a.canonicalize().unwrap().display()).as_bytes()
    );
    assert_eq!(
        fs::read(root_c.join("client-state/data-home-previous-root")).unwrap(),
        format!("{}\n", root_b.canonicalize().unwrap().display()).as_bytes()
    );
    assert_eq!(
        fs::read(locator_path(&home)).unwrap(),
        format!("{}\n", root_c.display()).as_bytes()
    );
    fs::remove_dir_all(fixture).unwrap();
}

#[test]
fn locator_commit_fault_preserves_both_roots_and_reports_uncertain_selection() {
    let fixture = std::env::temp_dir().join(format!(
        "licoup-data-home-locator-fault-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let home = fixture.join("home");
    let source = home.join(".lico-up");
    let destination_parent = fixture.join("destination");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&destination_parent).unwrap();
    fs::write(source.join("retained.json"), b"source bytes").unwrap();
    save_fixture_locator(&home, &source);

    let response = invoke_data_home_rpc_with_locator_commit_fault(
        &home,
        json!({
            "method": "data.home.relocate",
            "params": {"destinationParent": destination_parent, "confirmed": true}
        }),
    );

    let destination = destination_parent.canonicalize().unwrap().join("LicoUp");
    assert_eq!(
        response.pointer("/error/code").and_then(Value::as_str),
        Some("data_home_relocation_recovery_required")
    );
    assert!(source.is_dir());
    assert_eq!(
        fs::read(source.join("retained.json")).unwrap(),
        b"source bytes"
    );
    assert_eq!(
        fs::read(destination.join("retained.json")).unwrap(),
        b"source bytes"
    );
    assert!(fs::symlink_metadata(locator_path(&home)).unwrap().is_dir());
    fs::remove_dir_all(fixture).unwrap();
}

#[test]
fn dedicated_relocation_stops_an_existing_conversation_host_before_copy() {
    let fixture = std::env::temp_dir().join(format!(
        "licoup-data-home-host-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let home = fixture.join("home");
    let source = home.join(".lico-up");
    let destination_parent = fixture.join("destination");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&destination_parent).unwrap();
    let destination = destination_parent.canonicalize().unwrap().join("LicoUp");
    save_fixture_locator(&home, &source);

    let mut host = ChildGuard(
        Command::new(CLI)
            .args(["rpc", "conversation-host"])
            .env("HOME", &home)
            .env_remove("LICOUP_HOME")
            .env_remove("LICOUP_PORTABLE_DIR")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("LICOUP_CLIENT_PID")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_for_conversation_host(&source, &mut host.0);
    wait_for_conversation_store(&source, &mut host.0);
    verify_conversation_host_accepts_a_read_only_list(&home);
    assert!(
        source
            .join("client-state/conversations/conversations.sqlite3")
            .is_file()
    );

    let result = invoke_data_home_rpc(
        &home,
        json!({
            "method": "data.home.relocate",
            "params": {
                "destinationParent": destination_parent,
                "confirmed": true
            }
        }),
    );

    assert_eq!(
        result["ok"], true,
        "unexpected relocation response: {result}"
    );
    assert_eq!(
        result["result"]["dataHome"],
        destination.display().to_string()
    );
    assert!(source.is_dir());
    assert!(
        destination
            .join("client-state/conversations/conversations.sqlite3")
            .is_file()
    );
    assert!(host.0.wait().unwrap().success());
    fs::remove_dir_all(fixture).unwrap();
}

fn invoke_data_home_rpc(home: &Path, body: Value) -> Value {
    let (status, stdout, stderr) = run_data_home_rpc(home, body);
    assert!(
        status.success(),
        "dedicated data-home RPC process failed with bounded stderr: {}",
        stderr
    );
    let response: Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(response["ok"], true, "{response}; phases: {stderr}");
    response
}

fn invoke_data_home_rpc_error(home: &Path, body: Value) -> Value {
    let (status, stdout, stderr) = run_data_home_rpc(home, body);
    assert!(
        status.success(),
        "dedicated data-home RPC process failed with bounded stderr: {}",
        stderr
    );
    let response: Value = serde_json::from_slice(&stdout).unwrap();
    assert_eq!(response["ok"], false, "{response}; phases: {stderr}");
    response
}

fn run_data_home_rpc(home: &Path, body: Value) -> (std::process::ExitStatus, Vec<u8>, String) {
    let id = format!("data-home-{}", uuid::Uuid::new_v4().simple());
    let request = json!({
        "protocol": "licoup.stdio.v1",
        "id": id,
        "workflowId": id,
        "method": body["method"],
        "params": body["params"],
    });
    let mut child = Command::new(CLI)
        .args(["rpc", "data-home"])
        .env("HOME", home)
        .env_remove("LICOUP_HOME")
        .env_remove("LICOUP_PORTABLE_DIR")
        .env_remove("XDG_CONFIG_HOME")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(child.stdin.as_mut().unwrap(), "{request}").unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    (output.status, output.stdout, stderr.into_owned())
}

fn invoke_data_home_rpc_with_locator_commit_fault(home: &Path, body: Value) -> Value {
    let id = format!("data-home-{}", uuid::Uuid::new_v4().simple());
    let request = json!({
        "protocol": "licoup.stdio.v1",
        "id": id,
        "workflowId": id,
        "method": body["method"],
        "params": body["params"],
    });
    let mut child = Command::new(CLI)
        .args(["rpc", "data-home"])
        .env("HOME", home)
        .env_remove("LICOUP_HOME")
        .env_remove("LICOUP_PORTABLE_DIR")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("LICOUP_CLIENT_PID")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let locator = locator_path(home);
    let stderr = child.stderr.take().unwrap();
    let fault_injector = thread::spawn(move || {
        let mut output = String::new();
        let mut reader = BufReader::new(stderr);
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap() == 0 {
                break;
            }
            if line.trim_end() == "LICOUP_DATA_HOME_PHASE=switching-data-home" {
                fs::remove_file(&locator).unwrap();
                fs::create_dir(&locator).unwrap();
            }
            output.push_str(&line);
        }
        output
    });
    writeln!(child.stdin.as_mut().unwrap(), "{request}").unwrap();
    drop(child.stdin.take());
    let mut stdout = Vec::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut stdout)
        .unwrap();
    let status = child.wait().unwrap();
    let stderr = fault_injector.join().unwrap();
    assert!(
        status.success(),
        "unexpected process exit with stderr: {stderr}"
    );
    assert!(stderr.contains("LICOUP_DATA_HOME_PHASE=switching-data-home"));
    serde_json::from_slice(&stdout).unwrap()
}

fn save_fixture_locator(home: &Path, root: &Path) {
    let locator = locator_path(home);
    fs::create_dir_all(locator.parent().unwrap()).unwrap();
    fs::set_permissions(locator.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(&locator, format!("{}\n", root.display())).unwrap();
    fs::set_permissions(&locator, fs::Permissions::from_mode(0o600)).unwrap();
}

fn locator_path(home: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home.join("Library/Application Support/LicoUp/data-home")
    }
    #[cfg(not(target_os = "macos"))]
    {
        home.join(".config/licoup/data-home")
    }
}

fn wait_for_conversation_host(root: &Path, child: &mut Child) {
    let owner_path = root.join("client-state/conversation-runtime/host-owner.lock");
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Ok(file) = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&owner_path)
        {
            match file.try_lock_exclusive() {
                Ok(()) => {
                    let _ = file.unlock();
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return,
                Err(error) => panic!("cannot inspect synthetic host owner lock: {error}"),
            }
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("synthetic conversation host exited before ownership: {status}");
        }
        assert!(
            Instant::now() < deadline,
            "synthetic conversation host did not acquire its owner lock"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_conversation_store(root: &Path, child: &mut Child) {
    let database = root.join("client-state/conversations/conversations.sqlite3");
    let deadline = Instant::now() + Duration::from_secs(8);
    while !database.is_file() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("synthetic conversation host exited before opening its store: {status}");
        }
        assert!(
            Instant::now() < deadline,
            "synthetic conversation host did not open its store"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn verify_conversation_host_accepts_a_read_only_list(home: &Path) {
    // Resolve the endpoint from a CLI child so the endpoint generation matches
    // the host executable rather than this integration-test executable.
    let output = Command::new(CLI)
        .args([
            "conversation",
            "execute",
            "--require-running-host",
            "--stdin-json",
            r#"{"action":"conversation.list","includeArchived":false}"#,
        ])
        .env("HOME", home)
        .env_remove("LICOUP_HOME")
        .env_remove("LICOUP_PORTABLE_DIR")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("LICOUP_CLIENT_PID")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "synthetic host did not serve a read-only list: {}",
        stderr
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(response.is_object(), "unexpected host response: {response}");
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
