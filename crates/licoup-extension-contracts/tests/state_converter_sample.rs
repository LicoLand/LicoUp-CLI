//! The sample under `samples/state-converter/` is checked against the contract.
//!
//! The sample is the smallest complete **converter package**: a manifest that
//! declares the persisted format it owns, the release declaration the index
//! republishes, and one native program inside the payload that performs the
//! move. These tests read the committed documents and hold them to the published
//! contract — the manifest is admitted, the declared pair is the one the client
//! requires, the entry is an entry inside the payload, the release declaration
//! agrees with the manifest, and the *published JSON Schema* accepts the
//! document the sample actually ships rather than a document restated here.
//!
//! They do not import or run the payload. Import is the host's own path and
//! execution is the caller's, and both are exercised where those owners live:
//! `crates/licoup-migrate/tests/state_converter_sample.rs` compiles the entry,
//! assembles the payload and drives it through `PackageStore` and the converter
//! protocol. A contract test reads declarations; it does not pretend to be the
//! host.

use licoup_extension_contracts::manifest::{
    ConverterKind, FrozenEndpoints, MAX_SOURCE_FORMATS, PackageManifest, Runtime, conversion_code,
    is_converter_entry, is_format_identity,
};
use licoup_extension_contracts::profile::{ExtensionProfile, ProfileDeclaration};
use licoup_extension_contracts::wire;
use regex::Regex;
use serde_json::Value;
use std::collections::BTreeSet;

const MANIFEST: &str = include_str!("../samples/state-converter/manifest.json");
const RELEASE: &str = include_str!("../samples/state-converter/package-release.json");
const CONVERTER_SOURCE: &str = include_str!("../samples/state-converter/converter/main.rs");
const SCHEMA: &str = include_str!("../../../schemas/extensions/manifest.schema.json");
/// The client's own embedded migration frontier: the required pair is read from
/// the document `licoup_native::domain::client_state_migration` embeds, not from
/// a pair restated here.
const FRONTIER: &str =
    include_str!("../../../crates/licoup-native/resources/client-state-migration-frontier.json");
const PRODUCT: &str = include_str!("../../../tools/client-version.json");

/// The declaration the sample ships, as the manifest writes it.
fn declaration() -> Value {
    serde_json::from_str::<Value>(MANIFEST).expect("manifest JSON")["conversion"].clone()
}

fn admitted() -> PackageManifest {
    PackageManifest::from_value(serde_json::from_str(MANIFEST).expect("manifest JSON"))
        .expect("the sample manifest is accepted")
}

fn schema() -> Value {
    serde_json::from_str(SCHEMA).expect("manifest schema")
}

/// The program the runtime declares the host starts.
fn admitted_entry() -> String {
    match &admitted().runtime {
        Runtime::Process { entry, .. } => entry.clone(),
        other => panic!("the sample is carried by a process, not {}", other.mode()),
    }
}

/// The pair the client requires, read from the frontier it embeds.
fn required() -> FrozenEndpoints {
    let frontier: Value = serde_json::from_str(FRONTIER).expect("migration frontier");
    FrozenEndpoints::new(
        frontier["sourceFrontierId"]
            .as_str()
            .expect("the frontier declares the format the client reads"),
        frontier["frontierId"]
            .as_str()
            .expect("the frontier declares the format the client produces"),
    )
}

#[test]
fn the_sample_manifest_is_a_conformant_converter_package() {
    let manifest = admitted();
    assert_eq!(manifest.schema, wire::MANIFEST);
    assert_eq!(manifest.id, "org.licoland.example.state-converter");
    assert_eq!(manifest.version, "1.0.0");

    let conversion = manifest
        .conversion
        .as_ref()
        .expect("the sample is a converter package, so it declares one");
    assert_eq!(conversion.kind, ConverterKind::NativeExecutable);
    assert_eq!(conversion.entry, "bin/licoup-sample-state-converter");

    // The program the host starts and the program the conversion runs are one
    // program at one path: a manifest that named two would let a package declare
    // a converter it does not carry.
    assert_eq!(manifest.runtime.mode(), "process");
    let Runtime::Process { entry, runtime_ref } = &manifest.runtime else {
        panic!("the sample is carried by a process");
    };
    assert_eq!(entry, &conversion.entry);
    // The sample carries its own program and borrows no interpreter. With no
    // runtime reference there is no `user:` runtime for the host to preserve and
    // no shared runtime for it to reference-count: what runs is the payload.
    assert!(runtime_ref.is_none());
    assert!(
        manifest.runtime.owns_its_runtime(),
        "a package with no runtime reference is the only owner of what it runs"
    );

    // The smallest converter asks for nothing and depends on nothing.
    assert!(manifest.permissions.is_empty());
    assert!(manifest.requires.is_empty());
    assert!(manifest.optional_requires.is_empty());
    assert!(manifest.resources.is_empty());
    assert!(!manifest.is_data_package());
}

#[test]
fn the_sample_declares_a_profile_this_host_publishes_nothing_for() {
    let manifest: Value = serde_json::from_str(MANIFEST).expect("manifest JSON");
    let declarations: Vec<ProfileDeclaration> =
        serde_json::from_value(manifest["profiles"].clone()).expect("profile declarations");
    assert_eq!(declarations.len(), 1, "one package, one declaration");
    let declaration = &declarations[0];
    assert_eq!(declaration.id, "native-converter");
    assert_eq!(declaration.major, 1);

    // A converter serves no call profile: it is a maintenance program the host
    // runs over a data root. The id is preserved and refused for nothing, which
    // is what an unknown profile costs.
    assert_eq!(ExtensionProfile::from_id(&declaration.id), None);
    let status = declaration.status(
        licoup_application::ContractRange {
            major: 1,
            minimum_minor: 0,
        },
        &licoup_extension_contracts::profile::DeclaredMethods::default(),
    );
    assert_eq!(
        status,
        licoup_extension_contracts::profile::ProfileStatus::Unpublished
    );
    assert!(
        status
            .refusal(&declaration.id, ExtensionProfile::DeclarativeUi)
            .is_none(),
        "an unpublished profile refuses nothing"
    );
}

#[test]
fn the_sample_owns_the_conversion_pair_the_client_requires() {
    let manifest = admitted();
    let endpoints = required();

    let conversion = manifest
        .conversion_owner(&endpoints)
        .expect("the sample owns the conversion the client requires");
    assert!(
        conversion.converts_from(endpoints.source_format()),
        "the required source is one the sample declares"
    );
    assert_eq!(conversion.target_format, endpoints.target_format());

    // A caller that required another pair is refused, and the refusal names the
    // field it read rather than reporting an unreadable package.
    let other_source = manifest
        .conversion_owner(&FrozenEndpoints::new(
            "example.format.absent",
            endpoints.target_format(),
        ))
        .expect_err("a source the sample does not declare is refused");
    assert_eq!(other_source.code, conversion_code::ENDPOINT_MISMATCH);
    assert_eq!(
        other_source.field.as_deref(),
        Some("conversion.sourceFormats")
    );

    let other_target = manifest
        .conversion_owner(&FrozenEndpoints::new(
            endpoints.source_format(),
            "example.format.absent",
        ))
        .expect_err("a target the sample does not produce is refused");
    assert_eq!(other_target.code, conversion_code::ENDPOINT_MISMATCH);
    assert_eq!(
        other_target.field.as_deref(),
        Some("conversion.targetFormat")
    );

    // Falling back to a package with no declaration at all is the other refusal.
    let plain = PackageManifest::from_value(serde_json::json!({
        "schema": wire::MANIFEST,
        "id": "org.licoland.example.plain",
        "version": "1.0.0",
        "displayName": "Example package with no conversion",
        "hostProtocol": { "major": 1, "minimumMinor": 0 },
        "compatibility": { "clientVersions": [">=0.3.0, <1.0.0"] },
        "profiles": [{ "id": "native-converter", "major": 1 }],
        "runtime": { "mode": "process", "entry": "bin/plain" },
        "activation": "on-demand",
        "requires": [],
        "optionalRequires": [],
        "permissions": []
    }))
    .expect("a package that owns no format is still a package");
    let absent = plain
        .conversion_owner(&endpoints)
        .expect_err("a package asked for a conversion it does not declare is refused");
    assert_eq!(absent.code, conversion_code::MISSING);
    assert_eq!(absent.field.as_deref(), Some("conversion"));
}

#[test]
fn the_sample_declares_the_source_formats_it_reads_without_duplicating_one() {
    let conversion = declaration();
    let sources = conversion["sourceFormats"]
        .as_array()
        .expect("declared source formats");
    assert!(
        !sources.is_empty(),
        "a converter that reads nothing converts nothing"
    );
    assert!(sources.len() <= MAX_SOURCE_FORMATS);

    let mut unique: BTreeSet<&str> = BTreeSet::new();
    for source in sources {
        let source = source.as_str().expect("a format identity is a string");
        assert!(
            is_format_identity(source),
            "{source} is not a format identity"
        );
        assert!(unique.insert(source), "{source} is declared twice");
    }
    let target = conversion["targetFormat"]
        .as_str()
        .expect("a declared target format");
    assert!(is_format_identity(target));
    assert!(
        !unique.contains(target),
        "one format cannot be both endpoints of the same conversion"
    );
    assert!(
        unique.contains(required().source_format()),
        "the sample must read the format the client requires"
    );
}

#[test]
fn the_release_declaration_agrees_with_the_manifest_it_republishes() {
    let manifest = admitted();
    let release: Value = serde_json::from_str(RELEASE).expect("release declaration");
    assert_eq!(release["schemaVersion"], "licoup.package-release.v1");
    assert_eq!(release["packageId"], manifest.id);
    assert_eq!(release["packageVersion"], manifest.version);

    let converter = &release["converter"];
    let conversion = manifest.conversion.as_ref().expect("declared conversion");
    assert_eq!(converter["kind"], conversion.kind.as_str());
    assert_eq!(converter["entry"], conversion.entry);
    assert_eq!(converter["targetFormat"], conversion.target_format);
    // The index publishes one source format per entry; it must be one the
    // manifest's own list declares, or the two documents describe two packages.
    let source_format = converter["sourceFormat"]
        .as_str()
        .expect("the release declaration names one source format");
    assert!(conversion.converts_from(source_format));

    // The compatibility range is one fact with two spellings, and they must agree.
    assert_eq!(
        release["clientCompatibility"]["kind"], "range",
        "the published compatibility kinds are the release index's"
    );
    assert_eq!(
        release["clientCompatibility"]["range"], manifest.compatibility.client_versions[0],
        "the release declaration and the manifest declare one compatibility list"
    );
}

#[test]
fn the_sample_covers_the_client_it_ships_against() {
    let manifest = admitted();
    let product: Value = serde_json::from_str(PRODUCT).expect("client version manifest");
    let product_version = product["productVersion"]
        .as_str()
        .expect("the client version manifest declares a product version");

    assert!(
        manifest.client_compatibility(product_version).is_covered(),
        "the sample must cover the client it ships against ({product_version})"
    );
    assert_eq!(
        manifest
            .client_compatibility(product_version)
            .refusal(&manifest.id, product_version),
        None
    );
}

#[test]
fn the_published_schema_admits_the_manifest_the_sample_ships() {
    let schema = schema();
    let manifest: Value = serde_json::from_str(MANIFEST).expect("manifest JSON");
    let object = manifest.as_object().expect("object");

    // Every field the schema requires is present, and the sample carries no field
    // the schema does not publish: `additionalProperties` is false, and a sample
    // that drifted from the published shape must fail here rather than in a
    // reader that silently drops what it does not know.
    for required in schema["required"].as_array().expect("required") {
        let key = required.as_str().expect("string");
        assert!(object.contains_key(key), "the sample omits {key}");
    }
    let properties: BTreeSet<&str> = schema["properties"]
        .as_object()
        .expect("properties")
        .keys()
        .map(String::as_str)
        .collect();
    for key in object.keys() {
        assert!(
            properties.contains(key.as_str()),
            "the sample carries {key}, which the schema does not publish"
        );
    }

    // The conversion block is checked against the schema's own definition of it,
    // including the two published patterns. The patterns are compiled from the
    // schema document, so a hand-edited schema and a hand-edited sample cannot
    // agree with each other while disagreeing with the contract.
    let conversion = schema["properties"]["conversion"].clone();
    let conversion_properties = conversion["properties"]
        .as_object()
        .expect("the schema publishes the conversion fields");
    let sample = declaration();
    let declared: BTreeSet<&str> = sample
        .as_object()
        .expect("declared conversion")
        .keys()
        .map(String::as_str)
        .collect();
    let published: BTreeSet<&str> = conversion_properties.keys().map(String::as_str).collect();
    assert_eq!(
        declared, published,
        "the sample declares exactly the conversion fields the schema publishes"
    );
    let required_fields: BTreeSet<&str> = conversion["required"]
        .as_array()
        .expect("the conversion block has a required list")
        .iter()
        .map(|field| field.as_str().expect("string"))
        .collect();
    assert_eq!(
        required_fields, published,
        "every published field is required"
    );

    let entry_pattern = schema["$defs"]["converterEntry"]["pattern"]
        .as_str()
        .expect("the schema publishes the converter-entry pattern");
    let format_pattern = schema["$defs"]["formatIdentity"]["pattern"]
        .as_str()
        .expect("the schema publishes the format-identity pattern");
    let entry = Regex::new(entry_pattern).expect("the published entry pattern compiles");
    let format = Regex::new(format_pattern).expect("the published format pattern compiles");

    let declared = declaration();
    let declared_entry = declared["entry"].as_str().expect("declared entry");
    assert!(
        entry.is_match(declared_entry) && is_converter_entry(declared_entry),
        "the declared entry and the published entry rule disagree on {declared_entry:?}"
    );
    assert_eq!(
        declared_entry,
        admitted_entry(),
        "the host starts the program the conversion declares"
    );
    assert_eq!(
        schema["properties"]["conversion"]["properties"]["entry"]["$ref"], "#/$defs/converterEntry",
        "the conversion entry is the published converter-entry definition"
    );

    // The published bounds are read from the schema rather than restated.
    let sources = declared["sourceFormats"]
        .as_array()
        .expect("source formats");
    let min_items = conversion_properties["sourceFormats"]["minItems"]
        .as_u64()
        .expect("a published lower bound");
    let max_items = conversion_properties["sourceFormats"]["maxItems"]
        .as_u64()
        .expect("a published upper bound");
    assert!(
        (min_items..=max_items).contains(&(sources.len() as u64)),
        "the sample declares {} source formats, outside the published {min_items}..={max_items}",
        sources.len()
    );
    for source in sources.iter().chain([&declared["targetFormat"]]) {
        let value = source.as_str().expect("a format identity is a string");
        assert!(
            format.is_match(value),
            "the published format pattern refuses {value:?}"
        );
    }
    assert_eq!(conversion_properties["kind"]["const"], "native-executable");
    assert_eq!(
        conversion_properties["kind"]["const"].as_str(),
        Some(ConverterKind::NativeExecutable.as_str())
    );
}

#[test]
fn the_sample_converter_source_implements_the_invocation_it_documents() {
    // The invocation is the contract's, and the sample's program is the half a
    // package author writes. Asserting each documented argument is present is
    // what keeps the source and the documented protocol from drifting apart:
    // the real invocation is exercised by `licoup-migrate`'s suite.
    for argument in [
        "--source",
        "--target",
        "--source-format",
        "--target-format",
        "--result",
    ] {
        assert!(
            CONVERTER_SOURCE.contains(argument),
            "the sample converter does not read {argument}"
        );
    }
    assert!(
        CONVERTER_SOURCE.contains("\"--resume\""),
        "a converter that ignores --resume restarts the run it was asked to continue"
    );
    assert!(
        CONVERTER_SOURCE.contains("licoup.package-conversion-result.v1"),
        "the sample converter does not write the published result document"
    );
    assert!(
        CONVERTER_SOURCE.contains("client-state"),
        "the sample converter does not read the persisted state directory"
    );
    // A program that needs no dependency is the reason this sample needs no
    // build system in its payload; the entry is compiled from this one file.
    assert!(
        !CONVERTER_SOURCE.contains("extern crate") && !CONVERTER_SOURCE.contains("use serde"),
        "the sample converter must stay standard-library only"
    );
}
