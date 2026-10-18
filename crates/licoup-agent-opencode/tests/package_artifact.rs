//! The OpenCode adapter package's own artifact, read the way the host reads it.
//!
//! What is real here: the committed `crates/licoup-agent-opencode/package` release
//! source, the host's own manifest, permission and client-compatibility contract
//! (`licoup_extension_contracts`), the release declaration the packaging tool
//! packages beside it, and the adapter this package's library actually
//! registers.
//!
//! What this proves, in one place: the package the release tool stages is the
//! package the host's own ownership table names, it is admitted by the client
//! line it declares and refused outside it, its declared runtime entry is the
//! native program it ships, and the adapter it contributes is the one its
//! library registers — so the manifest cannot describe a different package from
//! the one that runs.
//!
//! Nothing is generated, nothing is executed, and nothing reaches the network.

use licoup_extension_contracts::deployment::{PackOwnership, capability_owner};
use licoup_extension_contracts::manifest::{PackageManifest, Runtime};
use licoup_extension_contracts::ui::Contribution;
use serde_json::Value;
use std::path::{Path, PathBuf};

const PACKAGE_ID: &str = "org.licoland.adapter.opencode";
const PACKAGE_VERSION: &str = "0.14.0";
const CAPABILITY: &str = "agent-execution.v1";
const ADAPTER_ID: &str = "opencode";
const NATIVE_ENTRY: &str = "bin/lico-agent-opencode";
const CONTRIBUTION_ID: &str = "org.licoland.adapter.opencode/adapter-status";
const RELEASE_SCHEMA: &str = "licoup.package-release.v1";
const COVERED_CLIENT: &str = "0.3.0";
const OLDER_CLIENT: &str = "0.2.9";
const NEWER_CLIENT: &str = "1.0.0";

/// The one directory the release tool packages, and the only files in it.
const DECLARED_FILES: [&str; 4] = [
    "bin/lico-agent-opencode",
    "contributions/adapter-status.json",
    "manifest.json",
    "package-release.json",
];

const INTERPRETER_EXTENSIONS: [&str; 24] = [
    ".py", ".pyw", ".rb", ".pl", ".php", ".lua", ".sh", ".bash", ".zsh", ".ksh", ".fish", ".ps1",
    ".psm1", ".bat", ".cmd", ".js", ".mjs", ".cjs", ".ts", ".mts", ".cts", ".jsx", ".tsx", ".wasm",
];

fn package_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("package")
}

fn read(relative: &str) -> String {
    std::fs::read_to_string(package_root().join(relative))
        .unwrap_or_else(|error| panic!("{relative} must be readable: {error}"))
}

fn read_json(relative: &str) -> Value {
    serde_json::from_str(&read(relative))
        .unwrap_or_else(|error| panic!("{relative} must be JSON: {error}"))
}

fn manifest() -> PackageManifest {
    PackageManifest::from_value(read_json("manifest.json"))
        .expect("the committed manifest is one the host reads")
}

/// Every file under the package source directory, as package-relative paths.
fn declared_files(directory: &Path, prefix: &str, found: &mut Vec<String>) {
    let mut entries: Vec<_> = std::fs::read_dir(directory)
        .expect("the package source directory exists")
        .map(|entry| entry.expect("readable directory entry"))
        .collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let relative = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let kind = entry.file_type().expect("readable file type");
        assert!(
            !kind.is_symlink(),
            "{relative} must be a regular file, not a link"
        );
        if kind.is_dir() {
            declared_files(&entry.path(), &relative, found);
            continue;
        }
        assert!(kind.is_file(), "{relative} must be a regular file");
        found.push(relative);
    }
}

#[test]
fn the_committed_package_serves_the_capability_the_host_leaves_optional() {
    // Attribution, not assertion by the package itself: the host's own ownership
    // table is what makes this capability optional rather than part of the
    // kernel, and what makes this package one of the adapters that may provide
    // it. The default distribution names a different provider, which is exactly
    // why the OpenCode adapter can be left out without the client losing the rest
    // of the capability table.
    let ownership =
        capability_owner(CAPABILITY).expect("the host publishes the Agent-execution capability");
    assert!(
        matches!(ownership, PackOwnership::Optional(_)),
        "the Agent-execution capability is one a user may leave out"
    );
    assert_ne!(
        ownership.package(),
        PACKAGE_ID,
        "this package is one provider of the capability, not the default owner: \
         the default distribution names {}",
        ownership.package()
    );

    let manifest = manifest();
    manifest.validate().expect("the manifest validates");
    assert_eq!(
        manifest.schema,
        licoup_extension_contracts::wire::MANIFEST,
        "the package declares the manifest format the host reads"
    );
    assert_eq!(manifest.id, PACKAGE_ID);
    assert_eq!(manifest.version, PACKAGE_VERSION);
    assert_eq!(manifest.display_name, "OpenCode adapter");
    assert_eq!(manifest.host_protocol.major, 1);
    assert_eq!(
        serde_json::to_value(&manifest.activation).expect("the activation mode serializes"),
        Value::String("on-demand".to_owned()),
        "the adapter starts when a conversation needs it"
    );

    // One published profile declaration carries the capability this package is
    // the owner of: the adapter serves Agent execution, and nothing else.
    assert_eq!(manifest.profiles.len(), 1);
    let profile = &manifest.profiles[0];
    assert_eq!(profile.id, "agent-execution");
    assert_eq!(profile.major, 1);
    assert_eq!(profile.capabilities, [CAPABILITY]);
    assert_eq!(
        profile.profile().map(|profile| profile.contract()),
        Some("C09"),
        "the declared profile is the published Agent-execution contract"
    );

    // The package installs alone: the client it drives is the installed host,
    // not a package dependency.
    assert!(manifest.requires.is_empty());
    assert!(manifest.optional_requires.is_empty());

    // The adapter the manifest contributes is the adapter the library registers,
    // so the documents and the program describe one package.
    assert_eq!(licoup_agent_opencode::registration::ADAPTER_ID, ADAPTER_ID);
    assert_eq!(licoup_agent_opencode::registration::FRAMING, "http-sse");
    assert_eq!(
        licoup_agent_opencode::registration::PROTOCOL_FORMAT,
        "opencode.serve-http.v1"
    );
}

#[test]
fn the_declared_runtime_is_a_native_process_with_no_interpreter() {
    let manifest = manifest();
    assert_eq!(
        manifest.runtime,
        Runtime::Process {
            entry: NATIVE_ENTRY.to_owned(),
            runtime_ref: None,
        },
        "the entry is a program the host starts, with no runtime reference"
    );
    assert_eq!(manifest.runtime.mode(), "process");

    let raw = read_json("manifest.json");
    let runtime = raw["runtime"].as_object().expect("a runtime object");
    assert_eq!(runtime.get("mode").and_then(Value::as_str), Some("process"));
    assert_eq!(
        runtime.get("entry").and_then(Value::as_str),
        Some(NATIVE_ENTRY)
    );
    assert!(!runtime.contains_key("runtimeRef"));
    for forbidden in ["installScript", "install", "postinstall"] {
        assert!(
            !raw.as_object().expect("an object").contains_key(forbidden),
            "the manifest declares no {forbidden}"
        );
    }

    let entry = package_root().join(NATIVE_ENTRY);
    let metadata = std::fs::metadata(&entry).expect("the declared entry exists");
    assert!(metadata.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_ne!(
            metadata.permissions().mode() & 0o111,
            0,
            "the declared entry is executable"
        );
    }
    let lower = NATIVE_ENTRY.to_ascii_lowercase();
    for extension in INTERPRETER_EXTENSIONS {
        assert!(
            !lower.ends_with(extension),
            "the declared entry may not be carried by an interpreter: {extension}"
        );
    }
    let bytes = std::fs::read(&entry).expect("readable entry");
    assert!(
        !bytes.starts_with(b"#!"),
        "the declared entry may not be a shebang script"
    );
    assert!(!bytes.is_empty());
}

#[test]
fn the_declared_compatibility_admits_its_client_line_and_refuses_others() {
    let manifest = manifest();
    let declaration = read_json("package-release.json");
    let declared_range = declaration["clientCompatibility"]["range"]
        .as_str()
        .expect("the release declaration carries a range");

    assert_eq!(manifest.compatibility.client_versions, [declared_range]);
    assert!(manifest.client_compatibility(COVERED_CLIENT).is_covered());
    assert!(manifest.client_compatibility("0.9.9").is_covered());
    assert!(!manifest.client_compatibility(OLDER_CLIENT).is_covered());
    assert!(!manifest.client_compatibility(NEWER_CLIENT).is_covered());

    manifest
        .admit_client(COVERED_CLIENT)
        .expect("the declared client line is admitted");
    for refused in [OLDER_CLIENT, NEWER_CLIENT] {
        let failure = manifest
            .admit_client(refused)
            .expect_err("a client outside the declared line is refused");
        assert_eq!(failure.code, "package_client_incompatible");
        assert_eq!(failure.field.as_deref(), Some("compatibility"));
    }
    assert!(!manifest.compatibility.client_versions.is_empty());
}

#[test]
fn the_release_declaration_and_the_manifest_describe_one_package() {
    let manifest = manifest();
    let declaration = read_json("package-release.json");
    assert_eq!(
        declaration["schemaVersion"].as_str(),
        Some(RELEASE_SCHEMA),
        "the packaging tool reads one release declaration format"
    );
    assert_eq!(declaration["packageId"].as_str(), Some(PACKAGE_ID));
    assert_eq!(
        declaration["packageVersion"].as_str(),
        Some(manifest.version.as_str())
    );

    let converter = declaration["converter"].as_object().expect("a converter");
    assert_eq!(
        converter.get("kind").and_then(Value::as_str),
        Some("native-executable")
    );
    assert_eq!(
        converter.get("entry").and_then(Value::as_str),
        Some(NATIVE_ENTRY),
        "the declared entry is the runtime entry the manifest names"
    );
    // The inbound format is the exact wire the package's entry speaks, named by
    // the crate rather than retyped in the release document, and the outbound
    // format is the canonical conversation every interface shares.
    assert_eq!(
        converter.get("sourceFormat").and_then(Value::as_str),
        Some(licoup_agent_opencode::registration::PROTOCOL_FORMAT),
        "the package declares the protocol its entry speaks"
    );
    assert_eq!(
        converter.get("targetFormat").and_then(Value::as_str),
        Some("licoup.conversation.v1")
    );
}

#[test]
fn the_package_owns_no_persisted_data_and_says_so() {
    let manifest = manifest();
    assert!(
        manifest.conversion.is_none(),
        "this package owns no persisted package data, so it declares no conversion"
    );
    // The absence is explicit rather than silent: the package states the reason
    // in its own namespaced attributes, and a reader never has to infer it.
    assert_eq!(
        manifest
            .extensions
            .get("org.licoland.adapter.opencode/persistentData")
            .and_then(Value::as_str),
        Some("none")
    );
    assert!(
        manifest
            .extensions
            .contains_key("org.licoland.adapter.opencode/persistentDataReason"),
        "the package states why it declares no conversion"
    );
}

#[test]
fn the_declared_data_footprint_and_contribution_are_the_files_the_payload_ships() {
    let manifest = manifest();
    let mut found = Vec::new();
    declared_files(&package_root(), "", &mut found);
    assert_eq!(
        found, DECLARED_FILES,
        "the payload ships the package's own documents, its entry and its resources, and nothing else"
    );
    for relative in &found {
        let name = relative.to_ascii_lowercase();
        for extension in INTERPRETER_EXTENSIONS {
            assert!(
                !name.ends_with(extension),
                "no package asset may be carried by an interpreter: {relative}"
            );
        }
        let bytes = std::fs::read(package_root().join(relative)).expect("readable asset");
        assert!(
            !bytes.starts_with(b"#!"),
            "no package asset may be a shebang script: {relative}"
        );
    }

    assert_eq!(manifest.permissions.len(), 3);
    let mut capabilities: Vec<&str> = manifest
        .permissions
        .iter()
        .map(|permission| permission.capability.as_str())
        .collect();
    capabilities.sort_unstable();
    assert_eq!(
        capabilities,
        [
            "org.licoland.adapter.opencode/agent-process",
            "org.licoland.adapter.opencode/agent-sessions",
            "org.licoland.adapter.opencode/client-data-home",
        ],
        "the package asks for exactly the data it uses"
    );
    for permission in &manifest.permissions {
        assert!(
            permission.capability.starts_with(&format!("{PACKAGE_ID}/")),
            "{} is requested under the package's own namespace",
            permission.capability
        );
        assert!(!permission.scope.is_empty());
    }

    assert_eq!(manifest.contributions.len(), 1);
    let declaration = &manifest.contributions[0];
    assert_eq!(declaration.id, CONTRIBUTION_ID);
    let contribution: Contribution =
        serde_json::from_str(&read(&declaration.definition)).expect("the resource parses");
    contribution
        .validate()
        .expect("the contribution resource validates");
    assert_eq!(contribution.id, CONTRIBUTION_ID);
    assert_eq!(contribution.kind, declaration.kind);
    assert_eq!(
        contribution.requirement(),
        licoup_extension_contracts::ui::MountRequirement::None,
        "the contribution needs no published profile to mount"
    );
    assert_eq!(
        contribution.action_ref.as_deref(),
        Some("org.licoland.adapter.opencode/adapter/status"),
        "the contribution names the status verb this package's namespace owns"
    );
}
