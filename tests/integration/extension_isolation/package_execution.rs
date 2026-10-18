//! The composed runtime starts an installed package's own program end to end.
//!
//! What is real here: the production [`PackageStore`] (an archive imported and
//! recorded the way a user's package is), the production [`ExtensionRuntime`]
//! composition, the production isolation carrier, and a real program started on
//! real pipes. What is synthetic: the package, its manifest and its entry point,
//! all created under a temporary root. No user file, account, credential or
//! service is touched.
//!
//! The four cases exist because they are the four ways a program source can say
//! no: the package is not installed, its manifest asks for something that is not
//! a process, it does not declare the profile the call is admitted for, or it is
//! installed and enabled and really runs.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use licoup_extension_contracts::manifest::PermissionRequest;
use licoup_extension_contracts::wire;
use licoup_native::platform::extension_host::isolation::IsolationMode;
use licoup_native::platform::extension_host::{AgentExecutionCall, ExtensionRuntime};
use licoup_native::platform::extension_packages::{PackageStore, TrustRecord, content_digest};
use serde_json::{Value, json};

/// The capability the synthetic package's `agent-execution` profile serves.
const CAPABILITY: &str = "dev.example.agent/run";

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
            "licoup-package-execution-{tag}-{}-{nanos:x}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("managed root");
        Self { root }
    }

    fn root(&self) -> &Path {
        &self.root
    }

    /// Import one archive and return the store that now records it.
    fn store(&self) -> PackageStore {
        PackageStore::open(&self.root).expect("package store")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// One manifest for a package that may or may not be a process program.
fn manifest_json(id: &str, version: &str, runtime: Value, profiles: Value) -> Vec<u8> {
    json!({
        "schema": wire::MANIFEST,
        "id": id,
        "version": version,
        "displayName": format!("Fixture {id}"),
        "hostProtocol": { "major": 1, "minimumMinor": 0 },
        "profiles": profiles,
        "runtime": runtime,
        "activation": "on-demand",
        "requires": [],
        "optionalRequires": [],
        "permissions": [{ "capability": "dev.example.agent/run", "scope": "self" }],
        "contributions": [],
    })
    .to_string()
    .into_bytes()
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

/// The published `agent-execution` profile declaration for one capability.
fn agent_profile() -> Value {
    json!([{ "id": "agent-execution", "major": 1, "capabilities": [CAPABILITY] }])
}

/// The fixture entry point, written by the test so the marker is explicit.
///
/// It speaks the published C09 line protocol with shell builtins only, so it
/// needs no second binary and no runtime of its own.
const ENTRY_SCRIPT: &str = r#"#!/bin/sh
send() { printf '%s\n' "$1"; }
take_id() {
  rest="${1#*\"id\":}"
  REQ_ID="${rest%%[!0-9]*}"
}
while IFS= read -r line; do
  case "$line" in
    *'"method":"extension.initialize"'*)
      take_id "$line"
      send '{"jsonrpc":"2.0","method":"extension.ready","params":{"profiles":["agent-execution"]}}'
      send "{\"jsonrpc\":\"2.0\",\"id\":$REQ_ID,\"result\":{\"protocol\":{\"major\":1,\"minimumMinor\":0},\"maxFrameBytes\":65536,\"profiles\":[\"agent-execution\"]}}"
      ;;
    *'"method":"agent.describe"'*)
      take_id "$line"
      send "{\"jsonrpc\":\"2.0\",\"id\":$REQ_ID,\"result\":{\"id\":\"dev.example.agent.fixture\",\"instanceKind\":\"executable\",\"inputKinds\":[\"text\"],\"capabilities\":[\"dev.example.agent/run\"],\"interfaceVersion\":\"1.0.0\",\"usage\":\"unavailable\",\"cancel\":\"unsupported\",\"resume\":\"unsupported\"}}"
      ;;
    *'"method":"agent.execute"'*)
      take_id "$line"
      rest="${line#*\"invocationRef\":\"}"
      REF="${rest%%\"*}"
      send "{\"jsonrpc\":\"2.0\",\"id\":$REQ_ID,\"result\":{\"invocationRef\":\"$REF\",\"outcome\":\"accepted\"}}"
      send "{\"jsonrpc\":\"2.0\",\"method\":\"agent.event\",\"params\":{\"invocationRef\":\"$REF\",\"sequence\":1,\"kind\":\"terminal\",\"body\":{\"outcome\":\"succeeded\",\"marker\":\"package-program\"}}}"
      ;;
    *'"method":"extension.shutdown"'*)
      take_id "$line"
      send "{\"jsonrpc\":\"2.0\",\"id\":$REQ_ID,\"result\":{\"outcome\":\"stopped\"}}"
      exit 0
      ;;
    *) : ;;
  esac
done
"#;

/// Import one process package whose entry is the fixture script.
fn import_process_package(fixture: &Fixture, id: &str, version: &str) -> Vec<u8> {
    let bytes = archive(&[
        (
            "manifest.json",
            manifest_json(
                id,
                version,
                json!({ "mode": "process", "entry": "agent.sh", "runtimeRef": "user:sh" }),
                agent_profile(),
            ),
        ),
        ("agent.sh", ENTRY_SCRIPT.as_bytes().to_vec()),
    ]);
    let store = fixture.store();
    store
        .install_local_import(
            id,
            version,
            TrustRecord::local_approved(
                content_digest(&bytes),
                [PermissionRequest::new(CAPABILITY, "self")],
            )
            .expect("trust"),
            &bytes,
        )
        .expect("local import");
    bytes
}

#[test]
fn an_installed_package_program_is_started_and_settled_by_the_host() {
    let fixture = Fixture::new("installed");
    import_process_package(&fixture, "dev.example.agent.fixture", "1.0.0");
    let runtime = ExtensionRuntime::open(fixture.root(), IsolationMode::TrustedLocal)
        .expect("the composed runtime opens over the package store root");

    // The program source reads the installed manifest, so the refusal is a fact
    // about the store's record rather than about a caller-supplied path.
    let program = runtime
        .programs()
        .program_for(
            "dev.example.agent.fixture",
            "1.0.0",
            "instance-1",
            &["agent-execution".to_owned()],
        )
        .expect("the installed package resolves to a program");
    assert_eq!(
        program
            .executable
            .file_name()
            .and_then(|name| name.to_str()),
        Some("sh"),
        "a user: runtime reference resolves the user's own interpreter"
    );
    let entry = program
        .args
        .first()
        .expect("the entry is the interpreter's argument");
    assert!(
        entry.ends_with("agent.sh"),
        "the interpreter runs the package's own entry: {entry}"
    );

    let report = runtime
        .serve_agent_execution(
            "dev.example.agent.fixture",
            "1.0.0",
            &AgentExecutionCall::new(json!({"input": "one turn"}))
                .with_wait(Duration::from_secs(20)),
        )
        .expect("the composed runtime serves one agent-execution call");
    assert_eq!(report["capability"], CAPABILITY);
    assert_eq!(report["outcome"]["state"], "completed");
    assert_eq!(
        report["outcome"]["payload"],
        json!({"outcome": "succeeded", "marker": "package-program"}),
        "the program's own terminal body is the settled payload"
    );
    assert_eq!(report["profile"], "agent-execution");
    assert!(
        runtime.catalog().serves(CAPABILITY),
        "the committed catalog routes the capability the profile serves"
    );

    // The package program really is a process the carrier accounts for, not an
    // in-process stand-in: the instance has a pid and its own writable root.
    let facts = runtime
        .carrier()
        .facts(report["instanceId"].as_str().expect("instance id"))
        .expect("live facts for the committed instance");
    assert!(facts.pid > 0);
    assert!(runtime.programs().instance_root("instance-1").is_dir());
}

#[test]
fn a_package_that_is_not_installed_is_refused_before_anything_starts() {
    let fixture = Fixture::new("absent");
    let runtime = ExtensionRuntime::open(fixture.root(), IsolationMode::TrustedLocal)
        .expect("the composed runtime opens over an empty root");
    let failure = runtime
        .serve_agent_execution(
            "dev.example.agent.absent",
            "1.0.0",
            &AgentExecutionCall::new(json!({"input": "one turn"})),
        )
        .expect_err("an uninstalled package has no program");
    assert_eq!(failure.code, "extension_program_unavailable");
    assert_eq!(failure.component.as_ref(), "extension_host");
    assert!(runtime.catalog().is_empty());
}

#[test]
fn a_declarative_package_is_not_a_process_program() {
    let fixture = Fixture::new("declarative");
    let bytes = archive(&[
        (
            "manifest.json",
            manifest_json(
                "dev.example.agent.declarative",
                "1.0.0",
                json!({ "mode": "declarative", "descriptor": "ui.json" }),
                agent_profile(),
            ),
        ),
        ("ui.json", b"{\"kind\":\"panel\"}".to_vec()),
    ]);
    let store = fixture.store();
    store
        .install_local_import(
            "dev.example.agent.declarative",
            "1.0.0",
            TrustRecord::local_approved(
                content_digest(&bytes),
                [PermissionRequest::new(CAPABILITY, "self")],
            )
            .expect("trust"),
            &bytes,
        )
        .expect("local import");
    let runtime =
        ExtensionRuntime::open(fixture.root(), IsolationMode::TrustedLocal).expect("runtime");
    let failure = runtime
        .serve_agent_execution(
            "dev.example.agent.declarative",
            "1.0.0",
            &AgentExecutionCall::new(json!({"input": "one turn"})),
        )
        .expect_err("a descriptor is not a program");
    assert_eq!(failure.code, "extension_program_not_process");
    assert_eq!(failure.presentation_args.get("mode"), Some("declarative"));
}

#[test]
fn a_package_that_does_not_declare_agent_execution_is_not_enabled() {
    let fixture = Fixture::new("not-enabled");
    let bytes = archive(&[
        (
            "manifest.json",
            manifest_json(
                "dev.example.agent.other",
                "1.0.0",
                json!({ "mode": "process", "entry": "agent.sh", "runtimeRef": "user:sh" }),
                json!([]),
            ),
        ),
        ("agent.sh", ENTRY_SCRIPT.as_bytes().to_vec()),
    ]);
    let store = fixture.store();
    store
        .install_local_import(
            "dev.example.agent.other",
            "1.0.0",
            TrustRecord::local_approved(
                content_digest(&bytes),
                [PermissionRequest::new(CAPABILITY, "self")],
            )
            .expect("trust"),
            &bytes,
        )
        .expect("local import");
    let runtime =
        ExtensionRuntime::open(fixture.root(), IsolationMode::TrustedLocal).expect("runtime");
    let failure = runtime
        .serve_agent_execution(
            "dev.example.agent.other",
            "1.0.0",
            &AgentExecutionCall::new(json!({"input": "one turn"})),
        )
        .expect_err("a package that declares no agent-execution profile is not enabled for it");
    assert_eq!(failure.code, "extension_agent_execution_not_declared");
    assert!(runtime.catalog().is_empty());
}
