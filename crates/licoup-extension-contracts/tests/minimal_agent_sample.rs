//! The samples under `samples/` are checked against the contract.
//!
//! `samples/echo-agent/` is the smallest complete extension: a program in a
//! language that is not Rust, implementing three Agent methods and the handshake,
//! asking for no permission, and installed from a directory the user already has.
//! `samples/converter-package/` is the smallest complete converter package: a
//! manifest that owns one published format conversion with a program it carries
//! itself. These tests read authored synthetic wire vectors and manifests and
//! assert their declared contract — that the required set is satisfied exactly,
//! that no optional method was needed, that nothing was resolved over the network,
//! that the frames it emits are valid envelopes, and that a conversion declaration
//! is refused exactly where the published rules say it is.
//!
//! They do not run the programs. A language-agnostic contract is checked against
//! frames and manifests, not against one interpreter being installed. The separate
//! Python subprocess suite verifies actual execution; these vectors are not runtime
//! logs.

use licoup_application::ContractRange;
use licoup_extension_contracts::agent::{AgentEvent, CancelOutcome};
use licoup_extension_contracts::deployment::{
    InstallClosure, LocalCatalogue, PackageEntry, PackageSource, install_closure,
};
use licoup_extension_contracts::manifest::{
    ConversionDeclaration, ConverterKind, FrozenEndpoints, PackageManifest, conversion_code,
};
use licoup_extension_contracts::profile::{
    DeclaredMethods, ExtensionProfile, PROFILE_MAJOR, ProfileStatus, all_contracts,
};
use licoup_extension_contracts::transport::{
    DEFAULT_MAX_FRAME_BYTES, Framing, JSONRPC_VERSION, MAX_MAX_FRAME_BYTES, MIN_MAX_FRAME_BYTES,
};
use licoup_extension_contracts::wire;
use serde_json::Value;
use std::collections::BTreeSet;

const MANIFEST: &str = include_str!("../samples/echo-agent/manifest.json");
const CONVERTER_MANIFEST: &str = include_str!("../samples/converter-package/manifest.json");
const FIXTURE: &str = include_str!("../samples/echo-agent/wire_fixture.json");
const TRANSCRIPT: &str = "requestLines";
const EVENTS: &str = "eventLines";
const AGENT_SOURCE: &str = include_str!("../samples/echo-agent/agent.py");

fn frames(section: &str) -> Vec<Value> {
    let fixture: Value = serde_json::from_str(FIXTURE).expect("synthetic wire fixture");
    assert_eq!(fixture["schema"], "licoup.extension-sample-vector.v1");
    assert_eq!(fixture["synthetic"], true);
    fixture[section]
        .as_array()
        .expect("declared frame vector")
        .iter()
        .map(|line| {
            serde_json::from_str(line.as_str().expect("one frame string")).expect("frame JSON")
        })
        .collect()
}

fn published_methods() -> BTreeSet<&'static str> {
    all_contracts()
        .iter()
        .flat_map(|contract| contract.methods())
        .collect()
}

fn agent_optional_methods() -> BTreeSet<&'static str> {
    ExtensionProfile::AgentExecution
        .contract_profile()
        .optional
        .iter()
        .copied()
        .collect()
}

/// The required method set exercised by the synthetic request/event vectors.
fn declared_methods() -> DeclaredMethods {
    let mut names: BTreeSet<String> = BTreeSet::new();
    for frame in frames(TRANSCRIPT).into_iter().chain(frames(EVENTS)) {
        names.insert(
            frame["method"]
                .as_str()
                .expect("every frame names a method")
                .to_owned(),
        );
    }
    DeclaredMethods::new(names)
}

#[test]
fn the_sample_manifest_is_a_conformant_local_package() {
    let manifest =
        PackageManifest::from_value(serde_json::from_str(MANIFEST).expect("manifest JSON"))
            .expect("the sample manifest is accepted");

    assert_eq!(manifest.schema, wire::MANIFEST);
    assert!(
        manifest.permissions.is_empty(),
        "the smallest Agent asks for nothing"
    );
    assert!(
        matches!(
            manifest.activation,
            licoup_application::ActivationMode::OnDemand
        ),
        "an extension with no business call is never started"
    );

    // The interpreter is the user's. Uninstalling the package must not remove it.
    assert!(!manifest.runtime.owns_its_runtime());
    assert_eq!(manifest.runtime.mode(), "process");
}

#[test]
fn the_sample_declares_the_client_versions_it_supports() {
    let manifest =
        PackageManifest::from_value(serde_json::from_str(MANIFEST).expect("manifest JSON"))
            .expect("the sample manifest is accepted");

    // The published product identity, read from the same manifest the build
    // injects into the client, rather than a version restated here.
    let product: Value = serde_json::from_str(include_str!("../../../tools/client-version.json"))
        .expect("client version manifest");
    let product_version = product["productVersion"]
        .as_str()
        .expect("the client version manifest declares a product version");

    assert!(
        !manifest.compatibility.client_versions.is_empty(),
        "every package carries a compatibility list; one that declares none is admitted by nothing"
    );
    assert!(
        manifest.client_compatibility(product_version).is_covered(),
        "the sample must cover the client it ships against ({product_version})"
    );
    assert_eq!(
        manifest
            .client_compatibility(product_version)
            .refusal(&manifest.id, product_version),
        None,
        "a covering list refuses nothing"
    );
}

#[test]
fn the_sample_manifest_carries_every_field_its_schema_requires() {
    let schema: Value = serde_json::from_str(include_str!(
        "../../../schemas/extensions/manifest.schema.json"
    ))
    .expect("manifest schema");
    let manifest: Value = serde_json::from_str(MANIFEST).expect("manifest JSON");
    let object = manifest.as_object().expect("object");
    for required in schema["required"].as_array().expect("required") {
        let key = required.as_str().expect("string");
        assert!(object.contains_key(key), "the sample omits {key}");
    }
    assert_eq!(manifest["schema"], wire::MANIFEST);
    let properties = schema["properties"]
        .as_object()
        .expect("properties")
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    for key in object.keys() {
        assert!(
            properties.contains(key),
            "the sample carries {key}, which the schema does not publish"
        );
    }
}

#[test]
fn the_sample_implements_the_required_set_exactly() {
    let declared = declared_methods();
    let methods: BTreeSet<&str> = declared.names().collect();
    let published = published_methods();

    for method in &methods {
        assert!(published.contains(method), "{method} is not published");
    }
    for method in &methods {
        assert!(
            !agent_optional_methods().contains(method),
            "the minimal sample must not need the optional {method}"
        );
    }

    let contract = ExtensionProfile::AgentExecution.contract_profile();
    let required: BTreeSet<&str> = contract.required.iter().copied().collect();
    assert_eq!(
        methods, required,
        "the sample implements exactly what agent-execution requires"
    );

    let status = declaration().status(host_range(), &declared);
    assert_eq!(status, ProfileStatus::Available);
    assert!(
        status
            .refusal(
                "org.licoland.example.echo",
                ExtensionProfile::AgentExecution
            )
            .is_none()
    );
}

/// The sample package's declared profile, as its manifest writes it.
fn declaration() -> licoup_extension_contracts::profile::ProfileDeclaration {
    let manifest: Value = serde_json::from_str(MANIFEST).expect("manifest JSON");
    serde_json::from_value(manifest["profiles"][0].clone()).expect("profile declaration")
}

fn host_range() -> ContractRange {
    ContractRange {
        major: 1,
        minimum_minor: 0,
    }
}

#[test]
fn the_sample_is_installed_from_a_local_directory_with_no_directory_service() {
    let manifest: Value = serde_json::from_str(MANIFEST).expect("manifest JSON");
    let requires = manifest["requires"].as_array().expect("requires");
    assert_eq!(requires.len(), 1);
    assert_eq!(requires[0]["packageId"], "org.licoland.core");
    assert!(
        manifest["optionalRequires"]
            .as_array()
            .expect("optional")
            .is_empty()
    );

    let mut catalogue = LocalCatalogue::new();
    catalogue.insert(PackageEntry::new(
        "org.licoland.core",
        "0.3.0",
        PackageSource::LocalImport,
    ));
    let mut sample = PackageEntry::new(
        manifest["id"].as_str().expect("id"),
        manifest["version"].as_str().expect("version"),
        PackageSource::LocalImport,
    );
    sample
        .requires
        .push(serde_json::from_value(requires[0].clone()).expect("dependency"));
    catalogue.insert(sample);

    let closure: InstallClosure = install_closure(&catalogue, &["org.licoland.example.echo"])
        .expect("a local import resolves offline");
    assert_eq!(closure.len(), 2);
    assert!(closure.contains("org.licoland.core"));
    assert!(closure.declined_optional().next().is_none());
}

#[test]
fn the_sample_emits_events_that_are_valid_envelopes() {
    let events = frames(EVENTS);
    assert_eq!(
        events[0]["method"], "extension.ready",
        "the handshake publishes readiness before any work"
    );
    assert!(
        events[0].get("id").is_none(),
        "readiness is a notification, not an answer to a request"
    );

    let mut sequences = Vec::new();
    let mut terminals = 0;
    for frame in &events[1..] {
        assert_eq!(frame["method"], "agent.event");
        let event: AgentEvent =
            serde_json::from_value(frame["params"].clone()).expect("a valid event envelope");
        assert_eq!(event.invocation_ref, "sample-invocation-1");
        if event.is_terminal() {
            terminals += 1;
        }
        sequences.push(event.sequence);
    }
    assert_eq!(sequences, vec![1, 2], "sequences are monotone from one");
    assert_eq!(terminals, 1);
    assert!(
        events.last().expect("last frame")["params"]["kind"] == "terminal",
        "the end of the work is reported last"
    );
    assert_eq!(
        events[1]["params"]["body"], "hello from the sample transcript",
        "the body is carried verbatim"
    );
}

#[test]
fn the_sample_session_stays_inside_the_published_carrier() {
    let transcript = frames(TRANSCRIPT);
    let initialize = &transcript[0];
    assert_eq!(initialize["jsonrpc"], JSONRPC_VERSION);
    assert_eq!(
        initialize["params"]["protocol"]["major"], PROFILE_MAJOR,
        "the sample negotiates the published profile major"
    );

    let max_frame_bytes = initialize["params"]["maxFrameBytes"]
        .as_u64()
        .expect("a negotiated bound");
    assert!(
        (MIN_MAX_FRAME_BYTES as u64..=MAX_MAX_FRAME_BYTES as u64).contains(&max_frame_bytes),
        "the sample negotiates outside the accepted range"
    );
    let negotiated = Framing::new(DEFAULT_MAX_FRAME_BYTES, max_frame_bytes as usize);
    assert_eq!(
        negotiated.negotiated_max_frame_bytes(),
        max_frame_bytes as usize
    );

    let published = published_methods();
    for frame in &transcript {
        assert_eq!(frame["jsonrpc"], JSONRPC_VERSION);
        let method = frame["method"].as_str().expect("method");
        assert!(published.contains(method), "{method} is not published");
        assert!(
            frame.get("id").is_some(),
            "{method} is a request, so it carries an id"
        );
    }

    // The program is a sample of the carrier, not of a language binding: it is
    // not Rust, and the contract does not care.
    assert!(AGENT_SOURCE.starts_with("#!/usr/bin/env python3"));
}

/// The converter sample's manifest, as the store would read it.
fn converter_sample() -> PackageManifest {
    PackageManifest::from_value(
        serde_json::from_str(CONVERTER_MANIFEST).expect("converter sample manifest JSON"),
    )
    .expect("the converter sample manifest is accepted")
}

#[test]
fn the_converter_sample_declares_the_conversion_it_owns() {
    let manifest = converter_sample();
    let conversion = manifest
        .conversion
        .as_ref()
        .expect("the sample owns one conversion");

    assert_eq!(conversion.kind, ConverterKind::NativeExecutable);
    assert_eq!(conversion.kind.as_str(), "native-executable");
    assert_eq!(conversion.entry, "bin/example-converter");
    assert_eq!(conversion.source_formats, vec!["licoup-state-0.1.1"]);
    assert_eq!(conversion.target_format, "licoup-state-0.3.0");
    conversion
        .validate()
        .expect("the published declaration is structurally valid");

    // The declaration answers one catalogue question: does this package own the
    // pair a caller requires?
    assert!(conversion.converts_from("licoup-state-0.1.1"));
    assert!(!conversion.converts_from("licoup-state-0.3.0"));
    let required = FrozenEndpoints::new("licoup-state-0.1.1", "licoup-state-0.3.0");
    assert_eq!(
        manifest
            .conversion_owner(&required)
            .expect("the sample owns the required conversion"),
        conversion
    );
    let elsewhere = FrozenEndpoints::new("licoup-state-0.1.1", "licoup-state-0.9.0");
    assert_eq!(
        manifest
            .conversion_owner(&elsewhere)
            .expect_err("the sample does not own a pair it never declared")
            .code,
        conversion_code::ENDPOINT_MISMATCH
    );

    // The converter runs as a program the package carries, so uninstalling the
    // package cannot remove a runtime it borrowed from somewhere else.
    assert_eq!(manifest.runtime.mode(), "process");
    let licoup_extension_contracts::manifest::Runtime::Process { entry, .. } = &manifest.runtime
    else {
        panic!("the sample is carried by a program");
    };
    assert_eq!(
        entry, &conversion.entry,
        "the program that converts is the program the package declares"
    );
    // No `user:` reference is declared: nothing outside the payload is borrowed,
    // so the host removes nothing on this package's behalf. The echo sample
    // declares the other case — an interpreter the user installed, which the host
    // reuses and never removes.
    assert!(manifest.runtime.owns_its_runtime());

    // The published schema and the sample agree in both directions.
    let schema: Value = serde_json::from_str(include_str!(
        "../../../schemas/extensions/manifest.schema.json"
    ))
    .expect("manifest schema");
    let raw: Value = serde_json::from_str(CONVERTER_MANIFEST).expect("manifest JSON");
    let object = raw.as_object().expect("object");
    for required in schema["required"].as_array().expect("required") {
        let key = required.as_str().expect("string");
        assert!(object.contains_key(key), "the sample omits {key}");
    }
    let properties = schema["properties"]
        .as_object()
        .expect("properties")
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    for key in object.keys() {
        assert!(
            properties.contains(key),
            "the sample carries {key}, which the schema does not publish"
        );
    }
}

#[test]
fn the_converter_sample_is_refused_exactly_where_the_contract_says_it_is() {
    /// The refusal one mutation of the sample's declaration produces.
    fn mutate(
        declaration: &ConversionDeclaration,
        change: impl FnOnce(&mut ConversionDeclaration),
    ) -> licoup_application::ApplicationFailure {
        let mut candidate = declaration.clone();
        change(&mut candidate);
        candidate
            .validate()
            .expect_err("the mutation is not a valid declaration")
    }

    let base = converter_sample();
    let declaration = base.conversion.clone().expect("a conversion");

    // A converter that needed an interpreter would borrow a runtime the package
    // does not carry, so no other kind is expressible.
    let not_native = mutate(&declaration, |candidate| {
        candidate.kind = ConverterKind::Unsupported;
    });
    assert_eq!(not_native.code, conversion_code::NOT_NATIVE);
    assert_eq!(not_native.field.as_deref(), Some("conversion.kind"));

    // The entry is a path inside the payload: a bare name, a parent directory and
    // a Windows-style path are all someone else's program.
    for entry in [
        "example-converter",
        "../example-converter",
        "bin\\converter",
    ] {
        let outside = mutate(&declaration, |candidate| {
            candidate.entry = entry.to_owned();
        });
        assert_eq!(
            outside.code,
            conversion_code::ENTRY_OUTSIDE_PACKAGE,
            "{entry} was not refused as an outside-payload entry"
        );
        assert_eq!(outside.field.as_deref(), Some("conversion.entry"));
    }

    // A declaration that names no source, or a target that is not a format
    // identity, is incomplete rather than invalid: the field is missing a value.
    let no_source = mutate(&declaration, |candidate| candidate.source_formats.clear());
    assert_eq!(no_source.code, conversion_code::INCOMPLETE);
    assert_eq!(no_source.field.as_deref(), Some("conversion.sourceFormats"));

    let no_target = mutate(&declaration, |candidate| {
        candidate.target_format = "LicoUp State".to_owned();
    });
    assert_eq!(no_target.code, conversion_code::INCOMPLETE);
    assert_eq!(no_target.field.as_deref(), Some("conversion.targetFormat"));

    // A duplicated source and a format that is both endpoints are malformed
    // declarations: nothing about them can be repaired by filling in a field.
    let duplicate = mutate(&declaration, |candidate| {
        candidate
            .source_formats
            .push(candidate.source_formats[0].clone());
    });
    assert_eq!(duplicate.code, conversion_code::INVALID);
    assert_eq!(duplicate.field.as_deref(), Some("conversion.sourceFormats"));

    let circular = mutate(&declaration, |candidate| {
        candidate.target_format = candidate.source_formats[0].clone();
    });
    assert_eq!(circular.code, conversion_code::INVALID);
    assert_eq!(circular.field.as_deref(), Some("conversion.targetFormat"));
}

#[test]
fn the_converter_sample_carries_the_facts_a_replacement_reads() {
    let manifest = converter_sample();
    let conversion = manifest.conversion.as_ref().expect("a conversion");

    // Which client builds may load this package is `compatibility`, and a package
    // that declares none is admitted by nothing — including as a replacement.
    let product: Value = serde_json::from_str(include_str!("../../../tools/client-version.json"))
        .expect("client version manifest");
    let product_version = product["productVersion"]
        .as_str()
        .expect("the client version manifest declares a product version");
    assert!(
        manifest.client_compatibility(product_version).is_covered(),
        "the converter sample must cover the client it ships against ({product_version})"
    );

    // The conversion spans two distinct published formats: replacing an installed
    // version moves one format to another, and a converter that produced what it
    // reads would have nothing to move.
    assert_ne!(conversion.source_formats[0], conversion.target_format);
    assert!(conversion.source_formats.iter().all(|source| {
        licoup_extension_contracts::manifest::is_format_identity(source)
            && *source != conversion.target_format
    }));

    // The sample asks for no permission and starts no work by itself, so nothing
    // it declares can widen what a replacement is allowed to do. The idle verdict
    // that admits the replacement belongs to the host's own owner
    // (`platform/extension_packages/maintenance.rs`, proven by its unit tests and
    // `tests/extension_contract/a30_generation.rs`); a package is never asked
    // whether its own replacement is safe, and this sample declares no such claim.
    assert!(manifest.permissions.is_empty());
    assert!(matches!(
        manifest.activation,
        licoup_application::ActivationMode::OnDemand
    ));
}

#[test]
fn the_sample_states_the_stop_contract_it_must_keep() {
    // The four cancel outcomes are four different facts, and only one of them
    // says the work stopped. These are the facts a sample author is told to keep
    // truthful; the host never upgrades a package's answer into a stronger one.
    let outcomes = [
        CancelOutcome::Requested,
        CancelOutcome::Acknowledged,
        CancelOutcome::Unsupported,
        CancelOutcome::Unknown,
    ];
    let stopped = outcomes
        .iter()
        .filter(|outcome| outcome.is_stopped())
        .collect::<Vec<_>>();
    assert_eq!(
        stopped,
        vec![&CancelOutcome::Acknowledged],
        "only a confirmation means the work demonstrably stopped"
    );
    assert!(
        outcomes
            .iter()
            .all(|outcome| !outcome.settles_external_effect()),
        "a cancellation is a request and never settles an external effect"
    );

    // Cancel is an optional Agent method, so the smallest complete Agent is
    // complete without it and the host then reports `unsupported` rather than
    // pretending. The sample that implements the required set exactly is the
    // proof: it implements no cancel and declares no unsupported capability.
    let contract = ExtensionProfile::AgentExecution.contract_profile();
    assert!(contract.optional.contains(&"agent.cancel"));
    assert!(!contract.required.contains(&"agent.cancel"));
    assert!(!declared_methods().implements("agent.cancel"));
    assert!(
        !converter_sample().profiles.iter().any(|profile| profile
            .capabilities
            .iter()
            .any(|capability| capability.contains("cancel"))),
        "a converter package declares no cancel capability"
    );
}
