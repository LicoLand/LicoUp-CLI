//! The Gateway Runtime's real startup path.
//!
//! What is real here: the compiled `lico-gateway` program this package ships,
//! started the way the login item starts it — as the installed package's own
//! entry with nothing but `--port` — over a disposable data home, speaking its
//! loopback endpoint on a real socket.
//!
//! What this proves, in one place: the program the package declares is a program
//! that starts by itself. It resolves its own state, materializes the
//! configuration it owns when it has none, mints the client token its
//! authenticated endpoint needs, binds the loopback port it was given and
//! answers on it. A definition that named the client CLI with a gateway
//! subcommand would fail the negative case below, and a program that could not
//! resolve its own state would fail the positive one.
//!
//! Nothing here touches the developer's own data home: every path is under a
//! disposable root, and the child inherits no locator from this process.

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const GATEWAY: &str = env!("CARGO_BIN_EXE_lico-gateway");
const START_TIMEOUT: Duration = Duration::from_secs(30);

/// A disposable data home that is removed when the test ends, however it ends.
struct DisposableRoot(PathBuf);

impl DisposableRoot {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "licoup-gateway-entry-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("disposable root");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// The one directory this runtime keeps its state in.
    fn state(&self) -> PathBuf {
        self.0.join("llm-gateway")
    }
}

impl Drop for DisposableRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A port nothing is listening on, taken by binding and releasing it.
fn unused_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("a free loopback port");
    listener.local_addr().expect("the bound address").port()
}

/// One child that is killed and reaped when the test ends, however it ends.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The program as the login item starts it: the package entry, one port.
fn start_entry(root: &DisposableRoot, port: u16) -> Running {
    let log = fs::File::create(root.path().join("child.log")).expect("child log");
    let errors = log.try_clone().expect("child log clone");
    let child = Command::new(GATEWAY)
        .arg("--port")
        .arg(port.to_string())
        .env("LICOUP_HOME", root.path())
        .env("HOME", root.path())
        .env_remove("LICOUP_PORTABLE_DIR")
        .env_remove("TELEGRAM_API_ROOT")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(errors))
        .spawn()
        .expect("the package entry starts");
    Running(child)
}

/// One `GET /health` against the runtime's own loopback endpoint.
fn health(port: u16) -> Option<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    stream
        .write_all(
            format!("GET /health HTTP/1.1\r\nhost: 127.0.0.1:{port}\r\nconnection: close\r\n\r\n")
                .as_bytes(),
        )
        .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    Some(response)
}

/// The program this test starts is the entry the package declares, read from the
/// committed release source rather than from this file's own expectation.
fn declared_entry() -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the package directory")
        .join("package");
    let manifest = fs::read_to_string(root.join("manifest.json")).expect("the committed manifest");
    let manifest: serde_json::Value = serde_json::from_str(&manifest).expect("manifest json");
    manifest["runtime"]["entry"]
        .as_str()
        .expect("the package declares a process entry")
        .to_owned()
}

#[test]
fn the_started_program_is_the_entry_the_package_declares() {
    let declared = declared_entry();
    let shipped = Path::new(GATEWAY)
        .file_name()
        .expect("the built program has a name")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        Path::new(&declared)
            .file_name()
            .expect("the declared entry has a name")
            .to_string_lossy(),
        shipped,
        "the program this package builds is the one its manifest declares"
    );
    assert_eq!(declared, "bin/lico-gateway");
}

#[test]
fn the_package_entry_starts_from_the_arguments_the_login_item_writes() {
    let root = DisposableRoot::new("startup");
    let port = unused_port();
    let mut child = start_entry(&root, port);

    let deadline = Instant::now() + START_TIMEOUT;
    let mut response = None;
    while Instant::now() < deadline {
        if let Some(exit) = child.0.try_wait().expect("the child is observable") {
            panic!(
                "the package entry exited with {exit} instead of serving; log follows:\n{}",
                fs::read_to_string(root.path().join("child.log")).unwrap_or_default()
            );
        }
        if let Some(seen) = health(port) {
            response = Some(seen);
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let response = response.unwrap_or_else(|| {
        panic!(
            "the package entry never answered on 127.0.0.1:{port}; log follows:\n{}",
            fs::read_to_string(root.path().join("child.log")).unwrap_or_default()
        )
    });

    assert!(
        response.starts_with("HTTP/1.1 200"),
        "the runtime answers its own health endpoint: {response}"
    );
    assert!(
        response.contains(r#""ok":true"#) && response.contains("licoup-llm-gateway"),
        "the answer is the Gateway Runtime's own health document: {response}"
    );

    // A program started with nothing but a port owns the rest of its state: it
    // wrote the configuration it routes through and the token its authenticated
    // endpoint needs, in the layout the client's service manager also reads.
    let state = root.state();
    let config =
        fs::read_to_string(state.join("config.json")).expect("the runtime wrote its config");
    let parsed: serde_json::Value = serde_json::from_str(&config).expect("the config is json");
    assert!(
        parsed["providers"]
            .as_array()
            .is_some_and(|providers| !providers.is_empty()),
        "the configuration the runtime wrote carries its provider boundaries: {config}"
    );
    assert!(
        state.join("client-token").is_file(),
        "the runtime minted the client token for its authenticated endpoint"
    );
}

/// The package entry is not the client CLI: a definition that still passed the
/// CLI's gateway subcommand would fail here rather than start the client at
/// login.
#[test]
fn the_package_entry_refuses_the_client_cli_vocabulary() {
    let root = DisposableRoot::new("cli-vocabulary");
    let port = unused_port();
    let output = Command::new(GATEWAY)
        .arg("llm-gateway")
        .arg("service")
        .arg("start")
        .arg("--port")
        .arg(port.to_string())
        .env("LICOUP_HOME", root.path())
        .env("HOME", root.path())
        .stdin(Stdio::null())
        .output()
        .expect("the package entry runs");

    assert!(
        !output.status.success(),
        "the package entry has no CLI subcommands and refuses them instead of starting: {}",
        output.status
    );
    assert!(
        !TcpStream::connect(("127.0.0.1", port)).is_ok(),
        "a refused invocation starts no listener"
    );
}
