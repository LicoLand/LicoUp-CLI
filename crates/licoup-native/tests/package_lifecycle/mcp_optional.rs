//! MCP-OPTIONAL-COMPOSITION: an absent package composes into nothing at all.
//!
//! The MCP service is not a bundled neighbour of the client. It arrives only as
//! the independently released `org.licoland.feature.mcp` package, so this module
//! drives the real CLI against a synthetic data home with no package installed
//! and asserts what the composition actually does:
//!
//! - the service process is never started, by the host or by an explicit verb;
//! - no discovery document is published, which is the only way any client learns
//!   the loopback endpoint — so there is no listener to reach;
//! - the ordinary conversation path still starts and still answers a read-only
//!   list, because nothing about it was made to depend on the optional service;
//! - the availability projection says `not-installed`: an unavailable capability
//!   with a real next step, not a malformed request and not a client failure.
//!
//! The CLI under test is copied into the synthetic root first. That keeps the
//! test hermetic: a program sitting next to the client must never be what makes
//! this pass, and the whole point is that no such program ships any more.
#![cfg(unix)]

use fs2::FileExt as _;
use licoup_extension_contracts::deployment::{
    self, CapabilityAvailability, PackOwnership, PackageFacts,
};
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const CLI: &str = env!("CARGO_BIN_EXE_licoup-cli");
const MCP_PACKAGE: &str = "org.licoland.feature.mcp";
const MCP_CAPABILITY: &str = "mcp-server.v1";

/// Codes that all mean the same thing: there is nothing installed to serve this
/// capability. A success would mean a payload came back into the client.
const NOTHING_INSTALLED: [&str; 4] = [
    "mcp_package_absent",
    "mcp_package_disabled",
    "mcp_binary_unavailable",
    "mcp_service_unavailable",
];

struct Fixture {
    root: PathBuf,
    home: PathBuf,
    data_root: PathBuf,
    bin: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "licoup-mcp-optional-{tag}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let home = root.join("home");
        let data_root = root.join("data");
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&data_root).unwrap();
        // The hermetic client: the only programs beside it are its own.
        let cli = bin.join("licoup-cli");
        fs::copy(CLI, &cli).unwrap();
        fs::set_permissions(&cli, fs::Permissions::from_mode(0o755)).unwrap();
        let locator = locator_path(&home);
        fs::create_dir_all(locator.parent().unwrap()).unwrap();
        fs::set_permissions(locator.parent().unwrap(), fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(&locator, format!("{}\n", data_root.display())).unwrap();
        fs::set_permissions(&locator, fs::Permissions::from_mode(0o600)).unwrap();
        Self {
            root,
            home,
            data_root,
            bin,
        }
    }

    fn cli(&self) -> Command {
        let mut command = Command::new(self.bin.join("licoup-cli"));
        command
            .env("HOME", &self.home)
            .env_remove("LICOUP_HOME")
            .env_remove("LICOUP_PORTABLE_DIR")
            .env_remove("XDG_CONFIG_HOME")
            .env_remove("LICOUP_CLIENT_PID")
            .env("LICO_MOBILE_RELAY_NATIVE_SECRET_STORE", "disabled")
            .env_remove("RUST_LOG")
            .env_remove("RUST_BACKTRACE");
        command
    }

    /// The published loopback endpoint. Its absence is the absence of a
    /// listener a client could reach.
    fn discovery(&self) -> PathBuf {
        self.service_state().join("discovery.json")
    }

    /// The writer lease the serving process holds while it runs.
    fn service_lock(&self) -> PathBuf {
        self.service_state().join("service.lock")
    }

    fn service_state(&self) -> PathBuf {
        self.data_root.join("client-state/subagent-mcp")
    }

    fn conversation_store(&self) -> PathBuf {
        self.data_root
            .join("client-state/conversations/conversations.sqlite3")
    }

    fn start_host(&self) -> ChildGuard {
        ChildGuard(
            self.cli()
                .args(["rpc", "conversation-host"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
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

fn wait_for_host(fixture: &Fixture, child: &mut Child) {
    let owner = fixture
        .data_root
        .join("client-state/conversation-runtime/host-owner.lock");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(file) = fs::OpenOptions::new().read(true).write(true).open(&owner) {
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
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_conversation_store(fixture: &Fixture, child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !fixture.conversation_store().is_file() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("synthetic conversation host exited before opening its store: {status}");
        }
        assert!(
            Instant::now() < deadline,
            "synthetic conversation host did not open its conversation store"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

/// One ordinary read-only conversation operation, served by the running host.
fn conversation_list_succeeds(fixture: &Fixture) {
    let output = fixture
        .cli()
        .args([
            "conversation",
            "execute",
            "--require-running-host",
            "--stdin-json",
            r#"{"action":"conversation.list","includeArchived":false}"#,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "an ordinary conversation no longer works: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(response.is_object(), "unexpected host response: {response}");
}

/// The refusal's own code, from either shape the CLI publishes it in: the
/// structured error envelope, or the plain `Error: <code>` line. Both name the
/// same fact, and this asserts on the fact rather than on the rendering.
fn error_code(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    if let Ok(value) = serde_json::from_str::<Value>(&stderr) {
        if let Some(code) = value.get("code").and_then(Value::as_str) {
            return code.to_owned();
        }
    }
    stderr
        .trim()
        .rsplit(": ")
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned()
}

/// With no installed package the service is simply absent: nothing is started,
/// nothing is published, and the conversation is untouched.
#[test]
fn an_absent_package_starts_no_service_and_leaves_the_conversation_alone() {
    let fixture = Fixture::new("absent");
    let mut host = fixture.start_host();
    wait_for_host(&fixture, &mut host.0);
    wait_for_conversation_store(&fixture, &mut host.0);

    // The host's own startup is the case a bundled payload used to hide in.
    assert!(
        !fixture.discovery().exists(),
        "the conversation host published an MCP endpoint nobody installed"
    );
    assert!(
        !fixture.service_lock().exists(),
        "the conversation host held an MCP service lease nobody installed"
    );

    let started = fixture.cli().args(["mcp", "start"]).output().unwrap();
    assert!(
        !started.status.success(),
        "an absent package started a service: {}",
        String::from_utf8_lossy(&started.stdout)
    );
    let code = error_code(&started);
    assert!(
        NOTHING_INSTALLED.contains(&code.as_str()),
        "an absent package must answer with an unavailable-service fact, not {code}"
    );
    assert!(
        !fixture.discovery().exists() && !fixture.service_lock().exists(),
        "a refused start still published an endpoint or a lease"
    );
    assert!(
        !fixture
            .service_state()
            .join("service-generation.json")
            .exists(),
        "a refused start still wrote a generation lease"
    );

    let status = fixture.cli().args(["mcp", "status"]).output().unwrap();
    assert!(
        !status.status.success(),
        "a status for an uninstalled service reported success: {}",
        String::from_utf8_lossy(&status.stdout)
    );
    assert!(
        NOTHING_INSTALLED.contains(&error_code(&status).as_str()),
        "status answered {code} instead of an unavailable-service fact"
    );

    // The optional service is not a prerequisite of the product: the same host
    // that refused to start it still serves the conversation.
    conversation_list_succeeds(&fixture);
    assert!(
        host.0.try_wait().unwrap().is_none(),
        "the conversation host exited while the MCP service was absent"
    );
}

/// The projection a user or a caller is shown is a state, not an error, and it
/// names the package that would change it.
#[test]
fn the_availability_projection_says_not_installed() {
    let absent = PackageFacts::default();
    assert_eq!(
        deployment::capability_owner(MCP_CAPABILITY),
        Some(PackOwnership::Optional(MCP_PACKAGE)),
        "the MCP capability belongs to the optional package"
    );
    assert_eq!(
        deployment::availability(MCP_CAPABILITY, absent),
        CapabilityAvailability::NotInstalled
    );
    assert_eq!(
        deployment::availability(MCP_CAPABILITY, absent).describe(),
        "not-installed"
    );
    assert!(
        deployment::optional_capabilities().any(|capability| capability == MCP_CAPABILITY),
        "an optional capability must be listed as one a user may leave out"
    );
    assert!(
        !deployment::core_capabilities().any(|capability| capability == MCP_CAPABILITY),
        "the MCP capability is not part of the kernel"
    );

    // Declining or switching off stays usable: the answer is a capability a user
    // can install or enable, with the state it is actually in.
    let declined = PackageFacts::default();
    let refusal = deployment::availability(MCP_CAPABILITY, declined)
        .refusal(MCP_CAPABILITY)
        .expect("an unserved capability has a refusal");
    assert_eq!(refusal.code, "capability_unavailable");
    assert_eq!(
        refusal.presentation_args.get("state"),
        Some("not-installed")
    );
    assert_eq!(
        refusal.presentation_args.get("capability"),
        Some(MCP_CAPABILITY)
    );
    assert_eq!(
        deployment::availability(MCP_CAPABILITY, PackageFacts::local_import(true)).describe(),
        "served"
    );

    // The facts are the owning package's facts, so the kernel is asked about
    // with the kernel installed: it is complete without the optional package,
    // and an absent MCP package cannot make the Assistant or the workflow
    // unavailable.
    let kernel = PackageFacts::local_import(true);
    for capability in [
        deployment::ASSISTANT_CAPABILITY,
        deployment::WORKFLOW_CAPABILITY,
    ] {
        assert_eq!(
            deployment::capability_owner(capability).map(PackOwnership::package),
            Some(deployment::CORE_PACKAGE),
            "{capability} is the kernel's"
        );
        assert_eq!(
            deployment::availability(capability, kernel).describe(),
            "served",
            "{capability} is served by the installed kernel with no optional package"
        );
    }
    // An MCP package that is installed but switched off is reported as exactly
    // that, so a user who declined it is never told the client is broken.
    assert_eq!(
        deployment::availability(MCP_CAPABILITY, PackageFacts::local_import(false)).describe(),
        "installed-not-enabled"
    );
}
