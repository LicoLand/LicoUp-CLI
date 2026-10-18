//! The published schemas and this crate must say the same thing.
//!
//! A schema that drifts from the code is worse than no schema: a third party
//! validates against it, ships a package, and the host refuses it for a rule the
//! schema never published. These tests pin every value that appears in both
//! places — the wire identifiers, the namespaced-name pattern, the published
//! profile set and each closed enumeration — so an edit to one is an edit to the
//! other or a failing test.
//!
//! The schemas are read from the repository's public `schemas/extensions/`
//! directory, which is the copy a third party sees.

use licoup_application::is_namespaced;
use licoup_extension_contracts::agent::{AgentEventKind, CancelOutcome, UsageSupport};
use licoup_extension_contracts::deployment::{
    CapabilityAvailability, PackageLifecycle, PackageSource,
};
use licoup_extension_contracts::manifest::{
    ConverterKind, FrozenEndpoints, MAX_CONVERTER_ENTRY_BYTES, MAX_FONT_FAMILY_BYTES,
    MAX_FORMAT_BYTES, MAX_HOST_ACTIONS, MAX_LOCALE_TAG_BYTES, MAX_RANGE_BYTES,
    MAX_RESOURCE_DEFINITION_BYTES, MAX_RESOURCE_KEYS, MAX_SOURCE_FORMATS, PackageManifest,
    ResourceKind, is_converter_entry, is_font_family, is_format_identity, is_locale_tag,
    is_resource_definition,
};
use licoup_extension_contracts::profile::ExtensionProfile;
use licoup_extension_contracts::provider::COMPATIBLE_DIALECTS;
use licoup_extension_contracts::ui::{
    ContributionKind, FieldType, GRAPH_RESOURCE_V1, HOST_PRIMITIVES, MAX_CONTRIBUTION_ID_BYTES,
};
use licoup_extension_contracts::usage::{Quality, Temporality, UsageOperation};
use licoup_extension_contracts::wire;
use regex::Regex;
use serde_json::{Value, json};

const MANIFEST: &str = include_str!("../../../schemas/extensions/manifest.schema.json");
const PROVIDER: &str = include_str!("../../../schemas/extensions/provider.schema.json");
const USAGE: &str = include_str!("../../../schemas/extensions/usage.schema.json");
const UI: &str = include_str!("../../../schemas/extensions/ui.schema.json");
const GRAPH_RESOURCE: &str = include_str!("../../../schemas/extensions/graph-resource.schema.json");
const DEPLOYMENT: &str = include_str!("../../../schemas/extensions/deployment.schema.json");
/// The committed release fixture the release tool packages and signs.
const RELEASE_FIXTURE_MANIFEST: &str = include_str!(
    "../../../tests/fixtures/client_package_release/fixture-native-converter/manifest.json"
);
const RELEASE_FIXTURE_RELEASE: &str = include_str!(
    "../../../tests/fixtures/client_package_release/fixture-native-converter/package-release.json"
);

fn schemas() -> Vec<(&'static str, Value)> {
    [
        ("manifest", MANIFEST),
        ("provider", PROVIDER),
        ("usage", USAGE),
        ("ui", UI),
        ("deployment", DEPLOYMENT),
    ]
    .into_iter()
    .map(|(name, text)| (name, serde_json::from_str(text).expect("valid JSON")))
    .collect()
}

/// The wire string one value serializes to, so an enumeration is compared in the
/// same vocabulary the schema publishes rather than in Rust's own naming.
fn wire_of<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value).expect("serialize") {
        Value::String(text) => text,
        other => panic!("expected a string wire form, got {other}"),
    }
}

fn namespaced_pattern(schema: &Value) -> String {
    schema["$defs"]["namespacedName"]["pattern"]
        .as_str()
        .expect("namespaced pattern")
        .to_owned()
}

#[test]
fn every_schema_publishes_the_identifier_this_crate_uses() {
    let expected = [
        ("manifest", wire::MANIFEST),
        ("provider", wire::PROVIDER),
        ("usage", wire::USAGE),
        ("ui", wire::UI),
        ("deployment", wire::DEPLOYMENT),
    ];
    for (name, identifier) in expected {
        let (_, schema) = schemas()
            .into_iter()
            .find(|(candidate, _)| *candidate == name)
            .expect("schema");
        assert_eq!(
            schema["properties"]["schema"]["const"], identifier,
            "{name} schema publishes a different identifier"
        );
        assert!(
            schema["required"]
                .as_array()
                .expect("required list")
                .iter()
                .any(|key| key == "schema"),
            "{name} must require its identifier"
        );
    }
}

#[test]
fn one_namespaced_pattern_is_published_everywhere() {
    let all = schemas();
    let pattern = namespaced_pattern(&all[0].1);
    for (name, schema) in &all {
        assert_eq!(
            namespaced_pattern(schema),
            pattern,
            "{name} publishes a different namespaced-name pattern"
        );
    }

    let regex = Regex::new(&pattern).expect("compiles");
    let corpus = [
        "org.licoland.core",
        "org.licoland.adapter.generic",
        "org.licoland.example/stream",
        "licoup.quota.tokens",
        "licoup.tokens.input",
        "example.specialist/purpose",
        "a.b",
        "bareword",
        "org.licoland.core/",
        "org.licoland..core",
        "Org.Licoland.Core",
    ];
    for name in corpus {
        assert_eq!(
            regex.is_match(name),
            is_namespaced(name),
            "the published pattern and is_namespaced disagree on {name:?}"
        );
    }
}

#[test]
fn the_published_profile_set_is_exactly_the_contract_set() {
    let expected: Vec<String> = ExtensionProfile::ALL
        .iter()
        .map(|profile| profile.id().to_owned())
        .collect();
    for (name, schema) in schemas() {
        let Some(enumeration) = schema["$defs"]["publishedProfile"].get("enum") else {
            continue;
        };
        let published: Vec<String> = enumeration
            .as_array()
            .expect("enum")
            .iter()
            .map(|value| value.as_str().expect("string").to_owned())
            .collect();
        assert_eq!(published, expected, "{name} publishes another profile set");
    }
    // The two schemas that name profiles must not silently drop the catalog.
    assert!(MANIFEST.contains("publishedProfile"));
    assert!(UI.contains("publishedProfile"));
}

#[test]
fn manifest_declares_exactly_the_profiles_this_crate_publishes() {
    let manifest: Value = serde_json::from_str(MANIFEST).expect("manifest");
    let referenced =
        &manifest["properties"]["profiles"]["items"]["properties"]["id"]["anyOf"][0]["$ref"];
    assert_eq!(referenced, "#/$defs/publishedProfile");
    assert_eq!(
        manifest["properties"]["profiles"]["items"]["properties"]["id"]["anyOf"]
            .as_array()
            .expect("anyOf")
            .len(),
        2,
        "an unpublished profile id must stay expressible"
    );
}

#[test]
fn closed_enumerations_match_the_crate() {
    let manifest: Value = serde_json::from_str(MANIFEST).expect("manifest");
    let kinds: Vec<String> = ContributionKind::ALL.iter().map(wire_of).collect();
    assert_eq!(
        manifest["properties"]["contributions"]["items"]["properties"]["kind"]["enum"],
        json!(kinds)
    );
    assert_eq!(
        manifest["properties"]["activation"]["enum"],
        json!([
            wire_of(&licoup_application::ActivationMode::OnDemand),
            wire_of(&licoup_application::ActivationMode::Explicit)
        ]),
        "activation modes must match ActivationMode"
    );

    let ui: Value = serde_json::from_str(UI).expect("ui");
    let field_types: Vec<String> = [
        FieldType::Text,
        FieldType::Number,
        FieldType::Boolean,
        FieldType::Select,
        FieldType::SecretRef,
    ]
    .iter()
    .map(wire_of)
    .collect();
    assert_eq!(
        ui["properties"]["kind"]["enum"],
        json!(kinds),
        "the standalone contribution publishes the same kinds"
    );
    assert_eq!(
        ui["properties"]["fields"]["items"]["properties"]["type"]["enum"],
        json!(field_types)
    );
    assert_eq!(
        ui["$defs"]["namespacedName"]["maxLength"], MAX_CONTRIBUTION_ID_BYTES,
        "the published format bound is the native bound"
    );

    let graph: Value = serde_json::from_str(GRAPH_RESOURCE).expect("graph-resource");
    assert_eq!(
        graph["properties"]["schema"]["const"], GRAPH_RESOURCE_V1,
        "the graph document publishes the capability identifier this crate recognises"
    );
    assert_eq!(
        graph["properties"]["projects"]["maxItems"], 8,
        "the frozen multi-project bound"
    );
    assert_eq!(
        graph["$defs"]["project"]["properties"]["nodes"]["maxItems"], 1000,
        "the frozen node bound"
    );
    assert_eq!(
        graph["properties"]["edges"]["maxItems"], 2000,
        "the frozen edge bound"
    );
    assert_eq!(
        graph["$defs"]["node"]["properties"]["actions"]["items"]["$ref"], "#/$defs/opaqueRef",
        "actions stay opaque references, never executable payloads"
    );
    for forbidden in ["code", "script", "widget", "handler", "dart"] {
        assert!(
            graph["$defs"]["node"]["properties"]
                .get(forbidden)
                .is_none(),
            "a graph node has no place for {forbidden}"
        );
    }

    let usage: Value = serde_json::from_str(USAGE).expect("usage");
    assert_eq!(
        usage["properties"]["operation"]["enum"],
        json!([
            wire_of(&UsageOperation::Upsert),
            wire_of(&UsageOperation::Retract)
        ])
    );
    assert_eq!(
        usage["properties"]["metrics"]["additionalProperties"]["properties"]["quality"]["enum"],
        json!([
            wire_of(&Quality::Reported),
            wire_of(&Quality::Estimated),
            wire_of(&Quality::Unknown)
        ])
    );
    assert_eq!(
        usage["properties"]["metrics"]["additionalProperties"]["properties"]["temporality"]["enum"],
        json!([
            wire_of(&Temporality::Delta),
            wire_of(&Temporality::Cumulative),
            wire_of(&Temporality::Gauge),
            wire_of(&Temporality::Absolute)
        ])
    );

    let deployment: Value = serde_json::from_str(DEPLOYMENT).expect("deployment");
    assert_eq!(
        deployment["properties"]["profile"]["enum"],
        json!(["minimal-local", "standard", "gateway", "peer", "analytics"]),
        "the workflow and flywheel are kernel capabilities, so no profile selects them"
    );
    assert_eq!(
        deployment["properties"]["packages"]["items"]["properties"]["source"]["enum"],
        json!([
            PackageSource::LocalImport.id(),
            PackageSource::LocalDirectory.id(),
            PackageSource::OfficialDirectory.id(),
            PackageSource::ThirdPartyDirectory.id()
        ])
    );
    assert_eq!(
        deployment["properties"]["packages"]["items"]["properties"]["lifecycle"]["enum"],
        json!([
            wire_of(&PackageLifecycle::Available),
            wire_of(&PackageLifecycle::Downloaded),
            wire_of(&PackageLifecycle::Verified),
            wire_of(&PackageLifecycle::LocalApproved),
            wire_of(&PackageLifecycle::Staged),
            wire_of(&PackageLifecycle::Installed)
        ])
    );
    assert_eq!(
        deployment["properties"]["capabilities"]["items"]["properties"]["availability"]["enum"],
        json!([
            CapabilityAvailability::Served.describe(),
            CapabilityAvailability::InstalledNotEnabled.describe(),
            CapabilityAvailability::NotInstalled.describe(),
            CapabilityAvailability::NotInDistribution.describe()
        ])
    );

    let provider: Value = serde_json::from_str(PROVIDER).expect("provider");
    assert_eq!(
        provider["allOf"][0]["if"]["properties"]["apiDialect"]["not"]["enum"],
        json!(COMPATIBLE_DIALECTS),
        "the dialects that need no stream adapter must be the published ones"
    );
}

#[test]
fn the_agent_event_vocabulary_is_published_with_its_optional_outcomes() {
    let kinds = [
        AgentEventKind::Text,
        AgentEventKind::Artifact,
        AgentEventKind::Progress,
        AgentEventKind::State,
        AgentEventKind::Terminal,
    ]
    .map(|kind| kind.as_str());
    assert_eq!(
        kinds,
        ["text", "artifact", "progress", "state", "terminal"],
        "the event kinds are the C09 vocabulary"
    );
    assert_eq!(
        [
            CancelOutcome::Requested,
            CancelOutcome::Acknowledged,
            CancelOutcome::Unsupported,
            CancelOutcome::Unknown,
        ]
        .iter()
        .map(wire_of)
        .collect::<Vec<_>>(),
        ["requested", "acknowledged", "unsupported", "unknown"]
    );
    // An Agent that reports no usage is a complete Agent, so "unavailable" is
    // part of the vocabulary rather than a missing value.
    assert_eq!(wire_of(&UsageSupport::Unavailable), "unavailable");
}

#[test]
fn the_published_compatibility_list_is_the_one_this_crate_evaluates() {
    let manifest: Value = serde_json::from_str(MANIFEST).expect("manifest");
    assert!(
        manifest["required"]
            .as_array()
            .expect("required list")
            .iter()
            .any(|key| key == "compatibility"),
        "every package carries a self-described compatibility list"
    );
    let client_versions = &manifest["properties"]["compatibility"]["properties"]["clientVersions"];
    assert_eq!(
        client_versions["minItems"], 1,
        "a list that declares no client version is admitted by nothing"
    );
    assert_eq!(
        client_versions["items"]["maxLength"], MAX_RANGE_BYTES,
        "the published bound is the native bound"
    );
    assert_eq!(
        manifest["properties"]["compatibility"]["additionalProperties"], false,
        "the compatibility object carries the client list and nothing else"
    );

    // The refusal this crate raises is the stable reason a client reports, so the
    // schema's own description has to name the rule rather than imply it.
    let description = manifest["properties"]["compatibility"]["description"]
        .as_str()
        .expect("compatibility description");
    for token in ["hostProtocol", "activation"] {
        assert!(
            description.contains(token),
            "the published rule must name {token} as the separate fact it is"
        );
    }
}

/// The committed release fixture is the one package the release tool packages and
/// signs. Its host manifest and its own release declaration must say the same
/// thing, so the manifest contract cannot drift from the authenticated artifact
/// metadata that names the same converter.
#[test]
fn the_release_fixture_declares_the_converter_its_release_metadata_publishes() {
    let manifest: Value = serde_json::from_str(RELEASE_FIXTURE_MANIFEST).expect("manifest");
    let declaration: Value =
        serde_json::from_str(RELEASE_FIXTURE_RELEASE).expect("release declaration");
    let converter = &declaration["converter"];
    let entry = converter["entry"]
        .as_str()
        .expect("release converter entry");
    let source_format = converter["sourceFormat"]
        .as_str()
        .expect("release converter source format");
    let target_format = converter["targetFormat"]
        .as_str()
        .expect("release converter target format");

    assert_eq!(manifest["conversion"]["kind"], converter["kind"]);
    assert_eq!(manifest["conversion"]["entry"], converter["entry"]);
    assert_eq!(manifest["runtime"]["entry"], converter["entry"]);
    assert!(
        manifest["conversion"]["sourceFormats"]
            .as_array()
            .expect("source formats")
            .iter()
            .any(|declared| *declared == converter["sourceFormat"]),
        "the manifest lists every source format its release declaration names"
    );
    assert_eq!(
        manifest["conversion"]["targetFormat"],
        converter["targetFormat"]
    );
    assert!(
        is_converter_entry(entry),
        "the release converter entry is an entry inside the package"
    );

    // The published manifest contract reads the fixture as one package, and the
    // package owns the conversion its release metadata publishes.
    let package = PackageManifest::from_value(manifest).expect("the fixture manifest is valid");
    let owned = package
        .conversion_owner(&FrozenEndpoints::new(source_format, target_format))
        .expect("the fixture owns the conversion it publishes");
    assert_eq!(owned.entry, entry);
    assert!(owned.converts_from(source_format));
}

#[test]
fn the_published_conversion_declaration_is_the_one_this_crate_validates() {
    let manifest: Value = serde_json::from_str(MANIFEST).expect("manifest");

    // The decision this pin states: the declaration is optional, because a
    // package that owns no persisted format carries none, and required only of
    // the package a caller asks for one conversion.
    assert!(
        !manifest["required"]
            .as_array()
            .expect("required list")
            .iter()
            .any(|key| key == "conversion"),
        "a package that owns no format must stay expressible"
    );
    let conversion = &manifest["properties"]["conversion"];
    assert_eq!(
        conversion["additionalProperties"], false,
        "the declaration carries the converter and nothing else"
    );
    assert_eq!(
        conversion["required"],
        json!(["kind", "entry", "sourceFormats", "targetFormat"])
    );
    assert_eq!(
        conversion["properties"]["kind"]["const"],
        ConverterKind::NativeExecutable.as_str(),
        "the schema publishes exactly the converter kinds this crate accepts"
    );
    assert_eq!(
        conversion["properties"]["sourceFormats"]["minItems"], 1,
        "a converter that reads nothing converts nothing"
    );
    assert_eq!(
        conversion["properties"]["sourceFormats"]["maxItems"], MAX_SOURCE_FORMATS,
        "the published bound is the native bound"
    );
    assert_eq!(
        conversion["properties"]["sourceFormats"]["uniqueItems"],
        true
    );
    assert_eq!(
        conversion["properties"]["targetFormat"]["$ref"],
        "#/$defs/formatIdentity"
    );
    assert_eq!(
        conversion["properties"]["entry"]["$ref"], "#/$defs/converterEntry",
        "an entry outside the package payload must not be expressible"
    );

    let entry_definition = &manifest["$defs"]["converterEntry"];
    assert_eq!(entry_definition["maxLength"], MAX_CONVERTER_ENTRY_BYTES);
    let format_definition = &manifest["$defs"]["formatIdentity"];
    assert_eq!(format_definition["maxLength"], MAX_FORMAT_BYTES);

    // The published patterns and this crate's predicates accept one corpus: a
    // schema a third party validates against must refuse what the host refuses.
    let entry_pattern = Regex::new(
        entry_definition["pattern"]
            .as_str()
            .expect("converter entry pattern"),
    )
    .expect("compiles");
    for value in [
        "bin/converter",
        "bin/licoup-fixture-converter",
        "bin/native/convert.v2",
        "converter",
        "/bin/converter",
        "bin/",
        "bin//converter",
        "../bin/converter",
        "bin/../converter",
        "bin/converter.exe",
        "bin\\converter",
        "bin/con verter",
        "",
    ] {
        assert_eq!(
            entry_pattern.is_match(value),
            is_converter_entry(value),
            "the published entry pattern and is_converter_entry disagree on {value:?}"
        );
    }

    let format_pattern = Regex::new(
        format_definition["pattern"]
            .as_str()
            .expect("format identity pattern"),
    )
    .expect("compiles");
    for value in [
        "licoup.conversation.v1",
        "licoup-state-0.1.1",
        "fixture.agent-session.v1",
        "agent-session.v2",
        "v1",
        "Licoup.conversation.v1",
        "licoup..v1",
        ".licoup.v1",
        "licoup.v1.",
        "licoup_conversation_v1",
        "",
    ] {
        assert_eq!(
            format_pattern.is_match(value),
            is_format_identity(value),
            "the published format pattern and is_format_identity disagree on {value:?}"
        );
    }
}

#[test]
fn the_data_package_category_is_published_with_its_typed_resources() {
    let manifest: Value = serde_json::from_str(MANIFEST).expect("manifest");

    // The category is the runtime mode, and the data mode carries the mode and
    // nothing else: an entry or a runtime reference cannot be read as data.
    let variants = manifest["properties"]["runtime"]["oneOf"]
        .as_array()
        .expect("runtime oneOf");
    let modes: Vec<&str> = variants
        .iter()
        .map(|variant| {
            variant["properties"]["mode"]["const"]
                .as_str()
                .expect("mode const")
        })
        .collect();
    assert_eq!(modes, ["process", "declarative", "service", "data"]);
    let data = &variants[3];
    assert_eq!(data["additionalProperties"], false);
    assert_eq!(
        data["properties"].as_object().expect("properties").len(),
        1,
        "a data runtime publishes the mode and no executable field"
    );
    assert_eq!(data["required"], json!(["mode"]));

    // Every published kind has exactly one shape, and that shape publishes the
    // kind, the shape identifier this crate pins and the coverage field its
    // variant carries — with no room for a free-form field.
    let shapes = manifest["$defs"]["dataResource"]["oneOf"]
        .as_array()
        .expect("resource oneOf");
    assert_eq!(
        shapes.len(),
        ResourceKind::ALL.len(),
        "one published shape per kind"
    );
    let published: Vec<&str> = ResourceKind::ALL.iter().map(|kind| kind.as_str()).collect();
    assert_eq!(
        published,
        [
            "theme",
            "layout",
            "style",
            "font",
            "language",
            "composition"
        ]
    );
    for (kind, shape) in ResourceKind::ALL.iter().zip(shapes) {
        let name = shape["$ref"]
            .as_str()
            .expect("shape ref")
            .rsplit('/')
            .next()
            .expect("shape name");
        let definition = &manifest["$defs"][name];
        assert_eq!(definition["properties"]["kind"]["const"], kind.as_str());
        assert_eq!(
            definition["properties"]["format"]["const"],
            kind.format(),
            "{name} publishes another shape identifier"
        );
        assert_eq!(definition["additionalProperties"], false);
        let required: Vec<&str> = definition["required"]
            .as_array()
            .expect("required")
            .iter()
            .map(|value| value.as_str().expect("string"))
            .collect();
        assert!(required.contains(&"kind"), "{name}");
        assert!(required.contains(&"id"), "{name}");
        assert!(required.contains(&"definition"), "{name}");
        assert!(required.contains(&"format"), "{name}");
        assert!(
            required.contains(&kind.coverage_field()),
            "{name} must require what the kind covers"
        );
        assert!(
            definition["properties"]
                .get(kind.coverage_field())
                .is_some(),
            "{name} must publish its coverage field"
        );
    }

    // The declared requirement set is published with the compiled primitive
    // vocabulary, and a composition component may bind only those.
    let primitives: Vec<&str> = HOST_PRIMITIVES
        .iter()
        .map(|primitive| primitive.as_str())
        .collect();
    assert_eq!(
        manifest["$defs"]["hostPrimitive"]["enum"],
        json!(primitives),
        "the manifest publishes the primitives this client compiles"
    );
    assert_eq!(
        manifest["$defs"]["compositionComponent"]["properties"]["primitive"]["$ref"],
        "#/$defs/hostPrimitive"
    );

    // The published bounds and shapes are the ones this crate evaluates.
    assert_eq!(
        manifest["$defs"]["resourceDefinition"]["maxLength"],
        MAX_RESOURCE_DEFINITION_BYTES
    );
    assert_eq!(
        manifest["$defs"]["localeTag"]["maxLength"],
        MAX_LOCALE_TAG_BYTES
    );
    assert_eq!(
        manifest["$defs"]["fontFamily"]["maxLength"],
        MAX_FONT_FAMILY_BYTES
    );
    assert_eq!(
        manifest["$defs"]["compositionResource"]["properties"]["components"]["maxItems"],
        MAX_RESOURCE_KEYS
    );
    assert_eq!(
        manifest["properties"]["hostActions"]["maxItems"],
        MAX_HOST_ACTIONS
    );
    assert_eq!(
        manifest["properties"]["hostPrimitives"]["uniqueItems"], true,
        "the requirement set is a set"
    );
    assert_eq!(manifest["properties"]["hostActions"]["uniqueItems"], true);
    for kind in ResourceKind::ALL {
        let name = shape_name(&manifest, kind);
        if kind == ResourceKind::Composition {
            continue;
        }
        assert_eq!(
            manifest["$defs"][name]["properties"][kind.coverage_field()]["maxItems"],
            MAX_RESOURCE_KEYS,
            "{name}"
        );
        assert_eq!(
            manifest["$defs"][name]["properties"][kind.coverage_field()]["uniqueItems"],
            true,
            "{name} covers each key once"
        );
    }

    // The two category rules are published as conditions, so a schema reader
    // learns the same rule the host enforces.
    let conditions = manifest["allOf"].as_array().expect("allOf");
    assert_eq!(conditions.len(), 2);
    assert_eq!(
        conditions[0]["if"]["properties"]["runtime"]["properties"]["mode"]["const"],
        "data"
    );
    assert_eq!(
        conditions[0]["then"]["properties"]["profiles"]["maxItems"], 0,
        "a data package serves no profile"
    );
    assert_eq!(
        conditions[0]["else"]["properties"]["profiles"]["minItems"], 1,
        "every other category declares the profiles it serves"
    );
    assert_eq!(
        conditions[1]["then"]["properties"]["runtime"]["properties"]["mode"]["const"], "data",
        "typed resources are carried by data and nothing else"
    );
    assert_eq!(
        conditions[1]["if"]["properties"]["resources"]["minItems"],
        1
    );

    // The refusals this crate raises name the rules the schema publishes.
    let resources = manifest["properties"]["resources"]["description"]
        .as_str()
        .expect("resources description");
    for token in ["data", "hostPrimitives", "hostActions", "definition"] {
        assert!(
            resources.contains(token),
            "the published resource rule must name {token}"
        );
    }
    let runtime = manifest["properties"]["runtime"]["description"]
        .as_str()
        .expect("runtime description");
    for token in ["data", "entry", "runtimeRef", "refused"] {
        assert!(
            runtime.contains(token),
            "the published category rule must name {token}"
        );
    }
}

/// The `$defs` name one kind's published shape.
fn shape_name(manifest: &Value, kind: ResourceKind) -> &str {
    let shapes = manifest["$defs"]["dataResource"]["oneOf"]
        .as_array()
        .expect("resource oneOf");
    let index = ResourceKind::ALL
        .iter()
        .position(|candidate| *candidate == kind)
        .expect("published kind");
    shapes[index]["$ref"]
        .as_str()
        .expect("shape ref")
        .rsplit('/')
        .next()
        .expect("shape name")
}

#[test]
fn the_published_resource_shapes_are_the_ones_this_crate_checks() {
    let manifest: Value = serde_json::from_str(MANIFEST).expect("manifest");

    let locale_pattern = manifest["$defs"]["localeTag"]["pattern"]
        .as_str()
        .expect("locale pattern");
    let locale = Regex::new(locale_pattern).expect("compiles");
    let families = manifest["$defs"]["fontFamily"]["pattern"]
        .as_str()
        .expect("font pattern");
    let family = Regex::new(families).expect("compiles");
    let definition = manifest["$defs"]["resourceDefinition"]["pattern"]
        .as_str()
        .expect("definition pattern");
    let definition_pattern = Regex::new(definition).expect("compiles");
    let escaped = manifest["$defs"]["resourceDefinition"]["not"]["pattern"]
        .as_str()
        .expect("definition escape pattern");
    let escaped_pattern = Regex::new(escaped).expect("compiles");

    for tag in [
        "en",
        "zh",
        "zh-CN",
        "pt-BR",
        "sr-Latn-RS",
        "",
        "e",
        "zh_CN",
        "english",
    ] {
        assert_eq!(
            locale.is_match(tag),
            is_locale_tag(tag),
            "the published locale shape and is_locale_tag disagree on {tag:?}"
        );
    }
    for name in [
        "Inter",
        "Noto Sans SC",
        "",
        " Inter",
        "Inter ",
        "Inter\nMono",
    ] {
        assert_eq!(
            family.is_match(name),
            is_font_family(name),
            "the published font shape and is_font_family disagree on {name:?}"
        );
    }
    for path in [
        "themes/midnight.json",
        "a/b/c.json",
        "",
        "/etc/passwd",
        "../outside.json",
        "a/../b.json",
        "..",
        "themes\\midnight.json",
        "./themes/midnight.json",
    ] {
        let published = definition_pattern.is_match(path) && !escaped_pattern.is_match(path);
        assert_eq!(
            published,
            is_resource_definition(path),
            "the published definition shape and is_resource_definition disagree on {path:?}"
        );
    }
}

#[test]
fn no_schema_publishes_a_place_for_key_material_or_a_self_hash() {
    for (name, schema) in schemas() {
        let text = serde_json::to_string(&schema).expect("serialize");
        for forbidden in ["apiKey", "api_key", "password", "artifactDigest", "granted"] {
            assert!(!text.contains(forbidden), "{name} publishes {forbidden}");
        }
    }
}
