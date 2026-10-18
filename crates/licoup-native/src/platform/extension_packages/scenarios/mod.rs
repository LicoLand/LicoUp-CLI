//! Component-integration scenarios for the optional package lifecycle.
//!
//! These tests compose the modules the way a host composes them — discovery
//! metadata, the install transaction, the package and instance machines, the
//! storage account, the GC pass and the uninstall transaction — against
//! synthetic packages and the real local filesystem. They are the
//! component-level evidence for A31, A37, A38 and A39; installed-product and
//! production-integration proof stays with the final verification nodes.
//!
//! Nothing here touches a real user installation: every root is a fresh
//! temporary directory built for the test.

mod external_fixtures;
mod failure;
mod lifecycle;
mod security;
mod surface;

use crate::platform::extension_packages::artifact::content_digest;
use crate::platform::extension_packages::install::{InstallOutcome, PackageStore};
use crate::platform::extension_packages::state::{InstanceIdentity, InstanceMachine, TrustRecord};
use crate::platform::extension_packages::unique_suffix;
use licoup_extension_contracts::deployment::InstanceLifecycle;
use licoup_extension_contracts::manifest::PermissionRequest;
use licoup_extension_contracts::wire;
use std::io::Write;
use std::path::{Path, PathBuf};

const ECHO: &str = "example.specialist.echo";
const SCRIPTED: &str = "example.specialist.scripted";
const NET: &str = "example.specialist/net";
const FS: &str = "example.specialist/fs";

/// The client versions a fixture declares it supports: the client this test
/// binary runs as, up to but not including the next major line. The value comes
/// from the product version owner rather than a literal here.
fn covering_client_versions() -> Vec<String> {
    let client = crate::platform::extension_packages::running_client_version()
        .expect("the binary declares a product version");
    let next_major = client
        .split('.')
        .next()
        .and_then(|major| major.parse::<u64>().ok())
        .map(|major| major + 1)
        .expect("a semantic major version");
    vec![format!(">={client}, <{next_major}")]
}

fn manifest_json(
    id: &str,
    version: &str,
    runtime_ref: Option<&str>,
    permissions: &[(&str, &str)],
) -> String {
    let mut runtime = serde_json::json!({ "mode": "process", "entry": "agent.py" });
    if let Some(reference) = runtime_ref {
        runtime["runtimeRef"] = serde_json::Value::String(reference.to_owned());
    }
    serde_json::json!({
        "schema": wire::MANIFEST,
        "id": id,
        "version": version,
        "displayName": "Echo specialist",
        "hostProtocol": { "major": 1, "minimumMinor": 0 },
        "compatibility": { "clientVersions": covering_client_versions() },
        "profiles": [{ "id": "agent-execution", "major": 1 }],
        "runtime": runtime,
        "activation": "on-demand",
        "requires": [],
        "optionalRequires": [],
        "permissions": permissions
            .iter()
            .map(|(capability, scope)| serde_json::json!({
                "capability": capability,
                "scope": scope,
            }))
            .collect::<Vec<_>>(),
        "contributions": [],
    })
    .to_string()
}

fn package_bytes(
    id: &str,
    version: &str,
    runtime_ref: Option<&str>,
    permissions: &[(&str, &str)],
    extra: &[(&str, &[u8])],
) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    writer.start_file("manifest.json", options).expect("entry");
    writer
        .write_all(manifest_json(id, version, runtime_ref, permissions).as_bytes())
        .expect("manifest");
    writer.start_file("agent.py", options).expect("entry");
    writer.write_all(b"print('echo')\n").expect("agent");
    for (name, bytes) in extra {
        writer.start_file(*name, options).expect("entry");
        writer.write_all(bytes).expect("extra");
    }
    writer.finish().expect("finish").into_inner()
}

fn net_permission() -> PermissionRequest {
    PermissionRequest::new(NET, "self")
}

fn trust_for(
    bytes: &[u8],
    permissions: impl IntoIterator<Item = PermissionRequest>,
) -> TrustRecord {
    TrustRecord::local_approved(content_digest(bytes), permissions).expect("trust")
}

fn sandbox(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("licoup-pkg-scenario-{tag}-{}", unique_suffix()))
}

fn store(tag: &str) -> (PathBuf, PackageStore) {
    let root = sandbox(tag);
    let store = PackageStore::open(&root).expect("store");
    (root, store)
}

fn install_local(
    store: &PackageStore,
    id: &str,
    version: &str,
    runtime_ref: Option<&str>,
) -> InstallOutcome {
    let bytes = package_bytes(id, version, runtime_ref, &[(NET, "self")], &[]);
    let trust = trust_for(&bytes, [net_permission()]);
    store
        .install_local_import(id, version, trust, &bytes)
        .expect("install")
}

fn active_instance(
    registry: &mut crate::platform::extension_packages::InstanceRegistry,
    package_id: &str,
    package_version: &str,
    generation: u64,
) -> String {
    let identity = InstanceIdentity::new(
        format!("instance-{generation}"),
        package_id,
        package_version,
        generation,
        7,
        Vec::<String>::new(),
    )
    .expect("identity");
    let mut machine = InstanceMachine::discovered(identity).expect("instance");
    machine
        .advance(InstanceLifecycle::Preparing)
        .expect("prepare");
    machine
        .advance(InstanceLifecycle::Active)
        .expect("activate");
    let instance_id = machine.identity().instance_id.clone();
    registry.insert(machine);
    instance_id
}

fn cleanup(root: &Path) {
    crate::platform::extension_packages::remove_managed_tree(root).expect("cleanup");
}
