//! CODEX-PACKAGE's falsifying test: the kernel keeps no Codex, and the
//! installed package binary is what serves a turn.
//!
//! Two claims, and this file fails if either stops being true:
//!
//! 1. **The client keeps no Codex app-server and no plugin-repository route.**
//!    `crates/licoup-native/src` holds no `codex_app_server` module and no
//!    `LicoUp-Plugins` literal. The claim is asserted against the real tree
//!    rather than described, so re-introducing either one fails here.
//! 2. **An installed Codex package binary serves one turn.** The package
//!    archive is imported into a temporary managed root the way a user's
//!    install imports one, the production extension host resolves the entry the
//!    manifest declares, starts it on real pipes, and one `agent.execute` call
//!    settles from the terminal event the package emitted after running the
//!    app-server it launched.
//!
//! What is real: the production package store and install transaction, the
//! production [`ExtensionRuntime`] composition, the production isolation
//! carrier, the compiled package program, and a real app-server process. What
//! is synthetic: the package version's identity, the managed root, and the
//! app-server itself, which is this host's own committed fixture. No user file,
//! account, credential or service is touched.
//!
//! The second claim needs the Codex package program to exist, because the
//! extension host runs the program the manifest declares and never a stand-in.
//! `cargo test -p licoup-agent-codex` builds it; this test finds it beside the
//! target directory it was itself built into and fails loudly when it is
//! missing, so a missing program can never be mistaken for a passing claim.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command as TestCommand;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use licoup_extension_contracts::manifest::PermissionRequest;
use licoup_extension_contracts::wire;
use licoup_native::platform::extension_host::isolation::IsolationMode;
use licoup_native::platform::extension_host::{AgentExecutionCall, ExtensionRuntime};
use licoup_native::platform::extension_packages::{PackageStore, TrustRecord, content_digest};
use serde_json::{Value, json};

/// The package identity the synthetic archive declares.
const PACKAGE_ID: &str = "org.licoland.adapter.codex";
const PACKAGE_VERSION: &str = "0.14.0";

/// The capability the package's `agent-execution` profile serves.
const CAPABILITY: &str = "agent-execution.v1";

/// The prompt this host's own app-server fixture answers.
const FIXTURE_PROMPT: &str = "fake-child-private-prompt";

/// The repository root, from this crate's manifest directory.
fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("the repository root is reachable from the crate")
}

/// Every Rust source under one repository-relative root.
fn rust_sources(relative_root: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![repository_root().join(relative_root)];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

#[test]
fn the_client_keeps_no_codex_app_server_and_no_plugin_repository_route() {
    let kernel = repository_root().join("crates/licoup-native/src");

    // The module is gone as a module: no file, no directory, no declaration and
    // no path into one. A comment saying otherwise would not satisfy this.
    assert!(
        !kernel.join("platform/codex_app_server.rs").exists(),
        "the kernel must not keep a Codex app-server module"
    );
    assert!(
        !kernel.join("platform/codex_app_server").exists(),
        "the kernel must not keep a Codex app-server module directory"
    );
    let sources = rust_sources("crates/licoup-native/src");
    assert!(
        !sources.is_empty(),
        "the kernel sources must be readable for this claim to mean anything"
    );
    for path in &sources {
        let text = std::fs::read_to_string(path).expect("a Rust source is readable UTF-8");
        assert!(
            !text.contains("mod codex_app_server;"),
            "the kernel must not declare a Codex app-server module: {}",
            path.display()
        );
        assert!(
            !text.contains("codex_app_server::"),
            "the kernel must not reach a Codex app-server module: {}",
            path.display()
        );
        assert!(
            !text.contains("LicoUp-Plugins"),
            "the kernel must not keep a plugin-repository route: {}",
            path.display()
        );
    }

    // The package owns what the kernel no longer has: one driver module and the
    // program an extension host starts.
    let app_server = std::fs::read_to_string(
        repository_root().join("crates/licoup-agent-codex/src/app_server.rs"),
    )
    .expect("the package's app-server module is readable");
    assert!(app_server.contains("pub mod driver;"));
    let program = std::fs::read_to_string(
        repository_root().join("crates/licoup-agent-codex/src/bin/lico-agent-codex.rs"),
    )
    .expect("the package program is readable");
    for verb in [
        "extension.initialize",
        "extension.ready",
        "agent.describe",
        "agent.execute",
        "agent.cancel",
        "extension.shutdown",
    ] {
        assert!(
            program.contains(verb),
            "the package program must serve the extension host's {verb} verb"
        );
    }
}

/// The compiled Codex package program, beside the target directory this test
/// was built into.
fn package_program() -> PathBuf {
    let mut candidates = Vec::new();
    if let Ok(current) = std::env::current_exe()
        && let Some(directory) = current.parent().and_then(Path::parent)
    {
        candidates
            .push(directory.join(format!("lico-agent-codex{}", std::env::consts::EXE_SUFFIX)));
    }
    candidates.push(
        repository_root()
            .join("build/crates/licoup-native/target/debug")
            .join(format!("lico-agent-codex{}", std::env::consts::EXE_SUFFIX)),
    );
    if let Some(target) = std::env::var_os("CARGO_TARGET_DIR") {
        candidates.push(
            PathBuf::from(target)
                .join("debug")
                .join(format!("lico-agent-codex{}", std::env::consts::EXE_SUFFIX)),
        );
    }
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| {
            panic!(
                "the Codex package program is not built; \
                 run `cargo test -p licoup-agent-codex` first, because the \
                 extension host starts the program the manifest declares and \
                 never a stand-in"
            )
        })
}

/// One temporary managed root holding one imported package archive.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let root = std::env::temp_dir().join(format!(
            "licoup-codex-package-turn-{tag}-{}-{nanos:x}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("managed root");
        Self { root }
    }

    fn root(&self) -> &Path {
        &self.root
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn archive(files: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    for (name, content) in files {
        writer.start_file(*name, options).expect("start file");
        writer.write_all(content).expect("write file");
    }
    writer.finish().expect("finish archive").into_inner()
}

/// The manifest of the package an install would stage into the managed root.
///
/// It is the shape the committed package declares: one `agent-execution`
/// profile, a process runtime whose entry is the package's own program, and the
/// capability that profile serves.
fn manifest_json() -> Vec<u8> {
    json!({
        "schema": wire::MANIFEST,
        "id": PACKAGE_ID,
        "version": PACKAGE_VERSION,
        "displayName": "Codex adapter",
        "hostProtocol": { "major": 1, "minimumMinor": 0 },
        "profiles": [
            { "id": "agent-execution", "major": 1, "capabilities": [CAPABILITY] }
        ],
        "runtime": { "mode": "process", "entry": "bin/lico-agent-codex" },
        "activation": "on-demand",
        "requires": [],
        "optionalRequires": [],
        "permissions": [{ "capability": CAPABILITY, "scope": "self" }],
        "contributions": [],
    })
    .to_string()
    .into_bytes()
}

/// Import the compiled package program as an installed package archive.
fn install_package(fixture: &Fixture) -> PathBuf {
    let program = package_program();
    let bytes = archive(&[
        ("manifest.json", manifest_json()),
        (
            "bin/lico-agent-codex",
            std::fs::read(&program).expect("the compiled package program is readable"),
        ),
    ]);
    let store = PackageStore::open(fixture.root()).expect("package store");
    store
        .install_local_import(
            PACKAGE_ID,
            PACKAGE_VERSION,
            TrustRecord::local_approved(
                content_digest(&bytes),
                [PermissionRequest::new(CAPABILITY, "self")],
            )
            .expect("trust"),
            &bytes,
        )
        .expect("local import");
    let entry = store
        .installed_path(PACKAGE_ID, PACKAGE_VERSION)
        .join("bin/lico-agent-codex");
    // A release stages an executable; an archive carries no mode, so this test
    // states the one the package declares rather than weakening the host's
    // "only a real program is a program" rule.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&entry, std::fs::Permissions::from_mode(0o755))
            .expect("the installed entry is executable");
    }
    entry
}

/// Compile this host's committed app-server fixture with the active toolchain.
///
/// One fixture, shared with the host's own end-to-end Codex suites: this test
/// drives a real app-server process rather than a second stand-in.
fn compile_fake_app_server() -> PathBuf {
    let fixture =
        repository_root().join("crates/licoup-native/tests/fixtures/fake_codex_app_server.rs");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp_dir = std::env::temp_dir().join(format!(
        "licoup-codex-package-app-server-{}-{nanos:x}",
        std::process::id()
    ));
    std::fs::create_dir_all(&temp_dir).expect("fixture directory");
    let executable = temp_dir.join(format!("fake-codex{}", std::env::consts::EXE_SUFFIX));
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_string());
    let compile = TestCommand::new(rustc)
        .arg("--edition=2024")
        .arg(&fixture)
        .arg("-o")
        .arg(&executable)
        .status()
        .expect("the app-server fixture should compile with the active Rust toolchain");
    assert!(
        compile.success(),
        "the app-server fixture failed to compile"
    );
    executable
}

#[test]
fn an_installed_codex_package_binary_serves_one_turn_through_the_extension_host() {
    let fixture = Fixture::new("turn");
    install_package(&fixture);
    let app_server = compile_fake_app_server();
    let workspace = fixture.root().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace");

    let runtime = ExtensionRuntime::open(fixture.root(), IsolationMode::TrustedLocal)
        .expect("the composed runtime opens over the package store root");

    // The entry the host resolves is the package's own program, inside the
    // installed bytes — not a path this test supplied.
    let program = runtime
        .programs()
        .program_for(
            PACKAGE_ID,
            PACKAGE_VERSION,
            "instance-1",
            &["agent-execution".to_owned()],
        )
        .expect("the installed package resolves to a program");
    assert_eq!(
        program
            .executable
            .file_name()
            .and_then(|name| name.to_str()),
        Some("lico-agent-codex"),
        "the extension host starts the package's declared entry"
    );

    let request = json!({
        "executable": app_server.to_string_lossy(),
        "prompt": FIXTURE_PROMPT,
        "params": {"model": "gpt-5.6-luna", "reasoningEffort": "high"},
        "cwd": workspace.to_string_lossy(),
        "timeoutMs": 20_000,
        "maxStdout": 1024 * 1024,
        "maxStderr": 1024 * 1024,
    });
    let report = runtime
        .serve_agent_execution(
            PACKAGE_ID,
            PACKAGE_VERSION,
            &AgentExecutionCall::new(request).with_wait(Duration::from_secs(60)),
        )
        .expect("the composed runtime serves one agent-execution call");

    assert_eq!(report["capability"], CAPABILITY);
    assert_eq!(report["profile"], "agent-execution");
    assert_eq!(
        report["outcome"]["state"], "completed",
        "the package's own terminal event settles the call: {report}"
    );
    let payload = &report["outcome"]["payload"];
    assert_eq!(payload["outcome"], "succeeded", "{report}");
    assert_eq!(payload["text"], "fake child final answer", "{report}");
    assert_eq!(payload["threadId"], "fake-thread", "{report}");
    assert_eq!(payload["turnId"], "fake-turn", "{report}");

    // The package program really is a process the carrier accounts for, not an
    // in-process stand-in: the instance has a pid and its own writable root.
    let facts = runtime
        .carrier()
        .facts(report["instanceId"].as_str().expect("instance id"))
        .expect("live facts for the committed instance");
    assert!(facts.pid > 0);
    assert_eq!(facts.fault(), None, "the package's wire stayed healthy");
    assert!(runtime.programs().instance_root("instance-1").is_dir());
}

/// The refusal an uninstalled package produces, so an absent install is a fact
/// about the store rather than a claim about the program above.
#[test]
fn a_codex_package_that_is_not_installed_produces_no_turn() {
    let fixture = Fixture::new("absent");
    let runtime = ExtensionRuntime::open(fixture.root(), IsolationMode::TrustedLocal)
        .expect("the composed runtime opens over an empty root");
    let failure = runtime
        .serve_agent_execution(
            PACKAGE_ID,
            PACKAGE_VERSION,
            &AgentExecutionCall::new(json!({"prompt": FIXTURE_PROMPT})),
        )
        .expect_err("an uninstalled package has no program");
    assert_eq!(failure.code, "extension_program_unavailable");
    assert!(runtime.catalog().is_empty());
}

/// The package program itself, asked for its description without a host.
#[test]
fn the_package_program_describes_itself_without_a_host() {
    let output = TestCommand::new(package_program())
        .arg("--describe")
        .output()
        .expect("the package program runs");
    assert!(output.status.success());
    let described: Value =
        serde_json::from_slice(&output.stdout).expect("the description is one JSON document");
    assert_eq!(described["packageId"], PACKAGE_ID);
    assert_eq!(described["adapterId"], "codex");
    assert_eq!(described["format"], "codex.app-server.v1");
}
