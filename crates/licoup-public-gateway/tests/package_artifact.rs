//! The optional MCP service package's own artifact, read the way the host reads
//! it.
//!
//! What is real here: the committed `crates/licoup-public-gateway/package` release source,
//! the host's own manifest, permission and client-compatibility contract
//! (`licoup_extension_contracts`), and the release declaration the packaging
//! tool packages beside it. Nothing is generated, nothing is executed, and
//! nothing reaches the network.
//!
//! What this proves, in one place: the package the release tool stages is the
//! package the host's capability table names, it is admitted by the client line
//! it declares and refused outside it, and its declared runtime, data footprint
//! and interface contribution are the files it actually ships.
//!
//! The declaration is tied to the optional service by one fact: the protocol
//! revision this package's entry negotiates is the one the `service` payload
//! publishes, so this target needs that feature.
#![cfg(feature = "service")]

use licoup_extension_contracts::deployment::{PackOwnership, capability_owner};
use licoup_extension_contracts::manifest::{PackageManifest, Runtime};
use licoup_extension_contracts::ui::Contribution;
use serde_json::Value;
use std::path::{Path, PathBuf};

const PACKAGE_ID: &str = "org.licoland.feature.mcp";
const PACKAGE_VERSION: &str = "0.14.0";
const CAPABILITY: &str = "mcp-server.v1";
const NATIVE_ENTRY: &str = "bin/lico-subagent-mcp";
const CONTRIBUTION_ID: &str = "org.licoland.feature.mcp/service-status";
const RELEASE_SCHEMA: &str = "licoup.package-release.v1";
const COVERED_CLIENT: &str = "0.3.0";
const OLDER_CLIENT: &str = "0.2.9";
const NEWER_CLIENT: &str = "1.0.0";

/// The one directory the release tool packages, and the only files in it.
const DECLARED_FILES: [&str; 4] = [
    "bin/lico-subagent-mcp",
    "contributions/service-status.json",
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
fn the_committed_package_is_the_capability_the_host_leaves_optional() {
    // Attribution, not assertion by the package itself: the host's own ownership
    // table is what makes this package the MCP service, and what makes it
    // optional rather than part of the kernel.
    assert_eq!(
        capability_owner(CAPABILITY),
        Some(PackOwnership::Optional(PACKAGE_ID)),
        "the MCP capability's owner is this package"
    );
    assert!(
        !capability_owner(CAPABILITY).expect("owned").is_core(),
        "the MCP service is a capability a user may leave out"
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
    assert_eq!(manifest.display_name, "LicoUp Subagent MCP service");
    assert_eq!(manifest.host_protocol.major, 1);
    assert_eq!(
        serde_json::to_value(&manifest.activation).expect("the activation mode serializes"),
        Value::String("on-demand".to_owned()),
        "the service starts when a call needs it"
    );

    // One profile declaration carries the capability this package is the owner
    // of. It is not a published extension profile: the raw binary serves the MCP
    // wire and the service verbs, not the `extension.initialize` handshake.
    assert_eq!(manifest.profiles.len(), 1);
    let profile = &manifest.profiles[0];
    assert_eq!(profile.id, "subagent-mcp");
    assert_eq!(profile.major, 1);
    assert_eq!(profile.capabilities, [CAPABILITY]);
    assert_eq!(
        profile.profile(),
        None,
        "an unpublished profile id is preserved, refuses nothing and grants nothing"
    );

    // The package installs alone: the client it drives is the installed host,
    // not a package dependency.
    assert!(manifest.requires.is_empty());
    assert!(manifest.optional_requires.is_empty());
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

    // The declaration is not decorative: the raw document carries no interpreter
    // reference and no install script, because an official package is a native
    // executable rather than something a host has to interpret or run a script
    // for.
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

    // The entry the payload ships is executable and is not a script under any of
    // the structural rules the release tooling applies.
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

    // The two formats state one claim: the manifest's required list admits
    // exactly the client line the release declaration publishes.
    assert_eq!(manifest.compatibility.client_versions, [declared_range]);
    assert!(manifest.client_compatibility(COVERED_CLIENT).is_covered());
    assert!(manifest.client_compatibility("0.3.1").is_covered());
    assert!(manifest.client_compatibility("0.9.9").is_covered());
    assert!(!manifest.client_compatibility(OLDER_CLIENT).is_covered());
    assert!(!manifest.client_compatibility(NEWER_CLIENT).is_covered());

    // Admission is the host's decision, with the host's own refusal: a client
    // outside the declared line is refused by name, and nothing else about the
    // package is a reason to refuse it.
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

    // An empty list would be admitted by nothing, so the list is required and is
    // not empty here.
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
    // The inbound format is the exact protocol revision this binary negotiates,
    // and the outbound format is the canonical conversation every interface
    // shares. Neither is invented for the release document.
    let negotiated = format!(
        "mcp.{}",
        licoup_public_gateway::application::PROTOCOL_REVISION
    );
    assert_eq!(
        converter.get("sourceFormat").and_then(Value::as_str),
        Some(negotiated.as_str()),
        "the package declares the protocol revision its entry negotiates"
    );
    assert_eq!(
        converter.get("targetFormat").and_then(Value::as_str),
        Some("licoup.conversation.v1")
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

    // The declared data footprint belongs to this package's own namespace, and
    // each request is bounded to a scope.
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
            "org.licoland.feature.mcp/agent-sessions",
            "org.licoland.feature.mcp/client-data-home",
            "org.licoland.feature.mcp/local-endpoint",
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

    // Every declared contribution names a resource the payload ships, and that
    // resource is the declarative contribution the host's own contract reads.
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
        Some("org.licoland.feature.mcp/service/status"),
        "the contribution names the service status verb the binary already accepts"
    );
}
