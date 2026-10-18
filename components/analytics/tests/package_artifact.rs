//! The optional analytics package's committed payload, read the way the host
//! and the release tooling read it.
//!
//! What is real here: `components/analytics/package`, the release source the
//! packaging tool turns into one payload, the host's own manifest, profile,
//! permission, compatibility and interface-contribution contracts
//! (`licoup_extension_contracts`), and this component's own metric catalog, panel
//! registry and C11 input mapping.
//!
//! What this proves, in one place: the package the release tool stages is the
//! package the host's capability table names as optional, the client line it
//! declares is the client line the release declaration publishes, the runtime it
//! declares is a native process with no interpreter, the panel it contributes is
//! a panel this component actually mounts, and every series that panel draws is a
//! metric the C11 catalog defines.
//!
//! Nothing is generated, nothing is executed, and nothing reaches the network.

use licoup_analytics::metrics::MetricCatalog;
use licoup_analytics::panels::{PanelRegistry, PreparedPanelValue};
use licoup_extension_contracts::deployment::{PackOwnership, capability_owner};
use licoup_extension_contracts::manifest::{PackageManifest, Runtime};
use licoup_extension_contracts::profile::ExtensionProfile;
use licoup_extension_contracts::ui::{Contribution, MountRequirement};
use licoup_extension_contracts::usage::Quality;
use licoup_usage_source_sdk::binding::SourceBinding;
use licoup_usage_source_sdk::normalize::OtlpSumNormalizer;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const PACKAGE_ID: &str = "org.licoland.feature.analytics";
const CAPABILITY: &str = "analytics.v1";
const NATIVE_ENTRY: &str = "bin/licoup-analytics";
const CONTRIBUTION_ID: &str = "org.licoland.feature.analytics/usage-panel";
const RELEASE_SCHEMA: &str = "licoup.package-release.v1";
const SOURCE_FORMAT: &str = "otlp.metrics.v1";
const COVERED_CLIENT: &str = "0.3.0";
const OLDER_CLIENT: &str = "0.2.9";
const NEWER_CLIENT: &str = "1.0.0";

/// The one directory the release tool packages, and the only files in it.
const DECLARED_FILES: [&str; 4] = [
    "bin/licoup-analytics",
    "contributions/usage-panel.json",
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
    // table is what makes this package the analytics capability, and what makes
    // it optional rather than part of the kernel.
    assert_eq!(
        capability_owner(CAPABILITY),
        Some(PackOwnership::Optional(PACKAGE_ID)),
        "the analytics capability's owner is this package"
    );
    assert!(
        !capability_owner(CAPABILITY).expect("owned").is_core(),
        "analytics is a capability a user may leave out"
    );

    let manifest = manifest();
    manifest.validate().expect("the manifest validates");
    assert_eq!(
        manifest.schema,
        licoup_extension_contracts::wire::MANIFEST,
        "the package declares the manifest format the host reads"
    );
    assert_eq!(manifest.id, PACKAGE_ID);
    assert_eq!(
        manifest.version,
        env!("CARGO_PKG_VERSION"),
        "the package version is the crate version that carries the code"
    );
    assert_eq!(manifest.display_name, "LicoUp analytics and usage sources");
    assert_eq!(manifest.host_protocol.major, 1);
    assert_eq!(
        serde_json::to_value(manifest.activation).expect("the activation mode serializes"),
        Value::String("on-demand".to_owned()),
        "the package starts when a call or a panel needs it"
    );

    // One profile declaration carries the capability this package is the owner
    // of, and it is the published C11 profile rather than an id of its own.
    assert_eq!(manifest.profiles.len(), 1);
    let profile = &manifest.profiles[0];
    assert_eq!(profile.id, "usage-metric");
    assert_eq!(profile.major, 1);
    assert_eq!(profile.capabilities, [CAPABILITY]);
    assert_eq!(
        profile.profile(),
        Some(ExtensionProfile::UsageMetric),
        "the declared profile is the published usage and metric profile"
    );

    // The package installs alone: the ledger it reads is the kernel's port, not
    // a package dependency, so nothing here can force the kernel to load it or
    // pull a second ledger in with it.
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
    assert!(
        manifest.runtime.owns_its_runtime(),
        "a package with no runtime reference carries its own program"
    );

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

    // The inbound format is the metrics record shape this package's own C11
    // input mapping reads at the boundary: it is proven below by normalizing a
    // document of exactly that shape into observations, rather than asserted
    // here. The outbound format is the published usage-observation contract,
    // which is what the C11 profile publishes.
    assert_eq!(
        converter.get("sourceFormat").and_then(Value::as_str),
        Some(SOURCE_FORMAT)
    );
    assert_eq!(
        converter.get("targetFormat").and_then(Value::as_str),
        Some(licoup_extension_contracts::wire::USAGE),
        "the package produces the published usage observation"
    );
    assert_ne!(
        converter.get("sourceFormat"),
        converter.get("targetFormat"),
        "one format is never both endpoints of the same conversion"
    );

    // The declared source format is not a name invented for the release
    // document: it is the record shape the SDK's boundary normalizer reads, and
    // one such record becomes observations rather than a refusal.
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/integration/usage_sources/fixtures/otlp-cumulative.json"
    ))
    .expect("the OTLP-shaped fixture parses");
    let normalizer =
        OtlpSumNormalizer::new("example.vendor", Quality::Reported).expect("normalizer");
    let binding = SourceBinding::new(
        "source:vendor#1",
        PACKAGE_ID,
        "instance-1",
        1,
        "epoch-1",
        ["scope-1"],
    )
    .expect("binding");
    let observations = normalizer
        .normalize(&fixture, &binding)
        .expect("a document in the declared source format normalizes");
    assert!(
        !observations.is_empty(),
        "the declared inbound format produces observations"
    );
}

#[test]
fn the_declared_footprint_and_contribution_are_the_files_the_payload_ships() {
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
            "org.licoland.feature.analytics/panel-surface",
            "org.licoland.feature.analytics/usage-facts",
            "org.licoland.feature.analytics/usage-sources",
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
        MountRequirement::Served(ExtensionProfile::UsageMetric),
        "the panel mounts exactly when the profile its package serves is served"
    );

    // Every series the panel draws is a metric the C11 catalog defines, and the
    // unit the panel prints is the unit that definition declares. A panel that
    // named a metric nobody defined would be drawing a chart of nothing.
    let catalog = MetricCatalog::with_general();
    assert!(!contribution.series.is_empty());
    for series in &contribution.series {
        let definition = catalog
            .definition(&series.metric)
            .unwrap_or_else(|| panic!("{} is a catalog metric", series.metric));
        assert_eq!(
            series.unit, definition.unit,
            "{} is drawn in the unit its definition declares",
            series.metric
        );
    }

    // And the panel really mounts: this component's registry accepts the shipped
    // contribution, and preparing it with no readings hands back unknown values
    // rather than zeroes.
    let mut registry = PanelRegistry::new(1);
    let report = registry.mount(
        std::slice::from_ref(&contribution),
        &[ExtensionProfile::UsageMetric],
    );
    assert_eq!(report.mounted, vec![CONTRIBUTION_ID.to_owned()]);
    assert!(report.blocked.is_empty());
    let prepared = PreparedPanelValue::from_readings(&contribution, 1, &BTreeMap::new());
    assert_eq!(prepared.points.len(), contribution.series.len());
    for point in &prepared.points {
        assert_eq!(
            point.value, None,
            "{} is unreported, not zero",
            point.metric
        );
        assert_eq!(point.quality, Quality::Unknown);
    }

    // Releasing the surface releases the contribution and nothing else.
    let withdrawn = registry.withdraw();
    assert_eq!(withdrawn.contributions, vec![CONTRIBUTION_ID.to_owned()]);
    assert!(registry.mounted_ids().is_empty());
    assert!(registry.prepared(CONTRIBUTION_ID).is_none());
}
