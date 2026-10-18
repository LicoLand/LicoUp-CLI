use super::super::adapter::{
    CapabilityFact, CapabilityFactName, CapabilityFactState, NativeAbilityKind,
    NativeDriverStateKind, adapter_for_agent,
};
use super::super::live_status::LiveSnapshot;
use super::super::registry::{
    DRIVER_INVENTORY_JSON, READINESS_JSON, adapter_management_catalog,
    native_capabilities_for_agent, native_capability_catalog_entry, parse_runtime_driver_registry,
    runtime_driver_profile,
};
use super::super::{PACKAGED_RUNTIME_ADAPTER_IDS, RuntimeAdapter};
use licoup_agent_opencode::driver as opencode_driver;
use serde_json::{Value, json};

fn fact(facts: &[CapabilityFact], name: &str) -> CapabilityFact {
    facts
        .iter()
        .find(|fact| fact.name == name)
        .unwrap_or_else(|| panic!("the projection must report the fact {name}"))
        .clone()
}

/// The channel facts an Agent declares, followed by the ability facts every
/// participant is asked about.
fn expected_kinds(channels: &[&'static str]) -> Vec<&'static str> {
    channels
        .iter()
        .copied()
        .chain(["image-input", "real-interface"])
        .collect()
}

#[test]
fn participant_projection_reports_declared_unknown_and_not_declared() {
    super::compose();
    // One participant whose owner declares image input.
    let declared = native_capabilities_for_agent("codex");
    let image_input = fact(&declared, "image-input");
    assert_eq!(image_input.state, CapabilityFactState::Declared);
    assert_eq!(image_input.source, "driver-inventory");

    // One participant the readable owner does not list.
    let not_declared = native_capabilities_for_agent("claude-code");
    let image_input = fact(&not_declared, "image-input");
    assert_eq!(image_input.state, CapabilityFactState::NotDeclared);
    assert_eq!(image_input.source, "driver-inventory");

    // One participant whose owner cannot be read: not a packaged Agent, so no
    // owner answers for it. An unreadable owner is unknown, never not-declared.
    let unreadable = native_capabilities_for_agent("no-such-packaged-agent");
    assert!(
        !unreadable.is_empty(),
        "an unreadable owner must still produce facts with states"
    );
    let image_input = fact(&unreadable, "image-input");
    assert_eq!(image_input.state, CapabilityFactState::Unknown);
    assert_eq!(image_input.source, "driver-inventory");
    assert_ne!(image_input.state, CapabilityFactState::NotDeclared);

    // Every derived fact carries a state and the owner that produced it.
    for name in [
        "image-input",
        "real-interface",
        "conversationDriver:supported",
        "conversationDriver:ready",
    ] {
        for facts in [&declared, &not_declared, &unreadable] {
            let fact = fact(facts, name);
            assert!(!fact.source.is_empty(), "{name} must name its owner");
        }
    }
    // Channel facts keep their owner too.
    let channel = fact(&declared, "app-server");
    assert_eq!(channel.state, CapabilityFactState::Declared);
    assert_eq!(channel.source, "agent-cli-presence");
    let desktop = fact(&declared, "desktop");
    assert_eq!(desktop.source, "agent-desktop-presence");
}

#[test]
fn capability_fact_vocabulary_is_one_closed_set() {
    super::compose();
    assert_eq!(
        CapabilityFactName::parse("image-input"),
        Some(CapabilityFactName::Ability(NativeAbilityKind::ImageInput))
    );
    assert_eq!(
        CapabilityFactName::parse("real-interface"),
        Some(CapabilityFactName::Ability(
            NativeAbilityKind::RealInterface
        ))
    );
    assert_eq!(
        CapabilityFactName::parse("conversationDriver:supported"),
        Some(CapabilityFactName::DriverState(
            NativeDriverStateKind::ConversationDriverSupported
        ))
    );
    assert_eq!(
        CapabilityFactName::parse("app-server").map(CapabilityFactName::wire_name),
        Some("app-server")
    );
    assert_eq!(CapabilityFactName::parse("not-a-capability"), None);
}

#[test]
fn an_unknown_fact_reports_no_detection_value() {
    super::compose();
    let snapshot = LiveSnapshot::for_testing(Vec::new(), std::collections::BTreeMap::new());
    let adapter = RuntimeAdapter::Codex;
    let declared = native_capability_catalog_entry(
        adapter,
        &snapshot,
        &CapabilityFact {
            name: "image-input",
            state: CapabilityFactState::Declared,
            source: "driver-inventory",
        },
    )
    .unwrap();
    assert_eq!(declared["detected"], json!(true));
    assert_eq!(declared["state"], json!("declared"));

    let not_declared = native_capability_catalog_entry(
        adapter,
        &snapshot,
        &CapabilityFact {
            name: "real-interface",
            state: CapabilityFactState::NotDeclared,
            source: "agent-desktop-presence",
        },
    )
    .unwrap();
    assert_eq!(not_declared["detected"], json!(false));

    let unknown = native_capability_catalog_entry(
        adapter,
        &snapshot,
        &CapabilityFact {
            name: "image-input",
            state: CapabilityFactState::Unknown,
            source: "driver-inventory",
        },
    )
    .unwrap();
    assert!(
        unknown.get("detected").is_none(),
        "an unknown fact reports no detection value"
    );
    assert_eq!(unknown["state"], json!("unknown"));

    // A conversation-driver state is not a native capability entry.
    assert!(
        native_capability_catalog_entry(
            adapter,
            &snapshot,
            &CapabilityFact {
                name: "conversationDriver:supported",
                state: CapabilityFactState::Declared,
                source: "conversation-readiness",
            },
        )
        .is_none()
    );
}

#[test]
fn packaged_dispatch_ids_are_unique_and_complete() {
    super::compose();
    let unique = PACKAGED_RUNTIME_ADAPTER_IDS
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(unique.len(), PACKAGED_RUNTIME_ADAPTER_IDS.len());
    for id in PACKAGED_RUNTIME_ADAPTER_IDS {
        assert_eq!(adapter_for_agent(id).map(RuntimeAdapter::id), Some(*id));
        assert!(runtime_driver_profile(id).is_some());
    }
}

#[test]
fn management_catalog_distinguishes_bundled_native_and_installable_bridges() {
    super::compose();
    let catalog = adapter_management_catalog(false);
    let adapters = catalog["adapters"].as_array().unwrap();
    assert_eq!(adapters.len(), PACKAGED_RUNTIME_ADAPTER_IDS.len());

    let by_id = adapters
        .iter()
        .map(|entry| (entry["agentId"].as_str().unwrap(), entry))
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(by_id["kimi-code"]["managementKind"], "bundled-acp");
    assert_eq!(by_id["codex"]["managementKind"], "native");
    assert_eq!(by_id["antigravity"]["managementKind"], "managed-bridge");
    assert_eq!(by_id["antigravity"]["lifecycleActions"], json!(["install"]));

    let installed = adapter_management_catalog(true);
    let antigravity = installed["adapters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["agentId"] == "antigravity")
        .unwrap();
    assert_eq!(antigravity["installationState"], "installed");
    assert_eq!(antigravity["lifecycleActions"], json!(["uninstall"]));
}

#[test]
fn management_catalog_projects_native_capabilities_and_adapter_plugins() {
    super::compose();
    let catalog = adapter_management_catalog(false);
    let adapters = catalog["adapters"].as_array().unwrap();
    let by_id = adapters
        .iter()
        .map(|entry| (entry["agentId"].as_str().unwrap(), entry))
        .collect::<std::collections::BTreeMap<_, _>>();

    let capability_kinds = |agent: &str| {
        by_id[agent]["nativeCapabilities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|capability| {
                assert!(capability["detected"].is_boolean());
                assert!(capability["state"].is_string());
                capability["kind"].as_str().unwrap()
            })
            .collect::<Vec<_>>()
    };
    // The catalog publishes the declared channel and ability facts for every
    // packaged Agent from the one participant projection.
    assert_eq!(
        capability_kinds("codex"),
        expected_kinds(&["desktop", "cli", "app-server"])
    );
    assert_eq!(capability_kinds("claude-code"), expected_kinds(&["cli"]));
    assert_eq!(
        capability_kinds("antigravity"),
        expected_kinds(&["desktop", "cli"])
    );
    assert_eq!(
        capability_kinds("opencode"),
        expected_kinds(&["cli", "local-server"])
    );
    assert_eq!(
        capability_kinds("kilo-code"),
        expected_kinds(&["cli", "local-server"])
    );
    assert_eq!(
        capability_kinds("openclaw"),
        expected_kinds(&["cli", "acp", "gateway"])
    );
    assert_eq!(
        capability_kinds("hermes"),
        expected_kinds(&["cli", "acp", "tui-gateway"])
    );
    assert_eq!(
        capability_kinds("cursor"),
        expected_kinds(&["desktop", "cli"])
    );
    assert_eq!(capability_kinds("pi"), expected_kinds(&["cli", "rpc"]));
    assert_eq!(
        capability_kinds("deepseek-harness"),
        expected_kinds(&["cli", "rpc"])
    );
    assert_eq!(capability_kinds("copilot"), expected_kinds(&["cli", "acp"]));
    assert_eq!(
        capability_kinds("kimi-code"),
        expected_kinds(&["cli", "acp", "web-server"])
    );
    // The ready multimodal Agent declares image input; an Agent whose owner
    // was read and does not list it reports not-declared, not unknown.
    let image_input = |agent: &str| {
        by_id[agent]["nativeCapabilities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|capability| capability["kind"] == "image-input")
            .unwrap()
            .clone()
    };
    assert_eq!(image_input("codex")["state"], json!("declared"));
    assert_eq!(image_input("codex")["detected"], json!(true));
    assert_eq!(image_input("claude-code")["state"], json!("not-declared"));
    assert_eq!(image_input("claude-code")["detected"], json!(false));

    // Only agents with real managed plugins list adapter plugin entries.
    let plugin_ids = |agent: &str| {
        by_id[agent]["adapterPlugins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|plugin| plugin["id"].as_str().unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(plugin_ids("antigravity"), ["acp-bridge"]);
    assert!(
        plugin_ids("codex").is_empty(),
        "a Codex caller plugin belongs to Codex's own marketplace, so this client advertises none"
    );
    for agent in [
        "claude-code",
        "copilot",
        "cursor",
        "hermes",
        "kilo-code",
        "kimi-code",
        "openclaw",
        "opencode",
        "pi",
        "deepseek-harness",
    ] {
        assert!(
            plugin_ids(agent).is_empty(),
            "{agent} must not list plugins"
        );
    }

    // The bridge entry mirrors the managed-bridge lifecycle projection; the
    // LicoUp-managed ACP bridge declares install only from a confirmed
    // not-installed state and executes it through the digest-bound flow.
    let bridge = &by_id["antigravity"]["adapterPlugins"][0];
    assert_eq!(bridge["label"], "ACP Bridge");
    assert_eq!(bridge["installationState"], "not-installed");
    assert_eq!(bridge["lifecycleActions"], json!(["install"]));
}

#[test]
fn canonical_resources_are_the_only_runtime_profile_source() {
    super::compose();
    let registry = parse_runtime_driver_registry(DRIVER_INVENTORY_JSON, READINESS_JSON).unwrap();
    assert_eq!(registry.drivers.len(), PACKAGED_RUNTIME_ADAPTER_IDS.len());
    assert_eq!(registry.readiness.len(), PACKAGED_RUNTIME_ADAPTER_IDS.len());

    let opencode = registry.profile("opencode").unwrap();
    assert_eq!(opencode.driver_status, "implemented");
    assert_eq!(opencode.protocol, opencode_driver::RUNTIME_PROTOCOL);
    assert_eq!(opencode.blocker, None);
    assert_eq!(
        opencode
            .capability_matrix
            .as_ref()
            .and_then(|matrix| matrix.get("laneFamily"))
            .and_then(Value::as_str),
        Some("serve-http")
    );

    let antigravity = registry.profile("antigravity").unwrap();
    assert_eq!(antigravity.driver_status, "implemented");
    assert_eq!(antigravity.readiness, "unverified");
    assert_eq!(antigravity.blocker, None);
    assert_eq!(antigravity.evidence_age_class, "stale");
    assert!(
        antigravity
            .summary_codes
            .contains(&"evidence_stale_or_incomplete".to_string())
    );
    assert_eq!(
        antigravity
            .capability_matrix
            .as_ref()
            .and_then(|matrix| matrix.get("laneFamily"))
            .and_then(Value::as_str),
        Some("cli")
    );
    // The pty transport makes the lane stream progressively (antigravity was
    // previously final-stdout-only); processing evidence stays absent because
    // the driver never emits agent.turn.processing.
    assert_eq!(
        antigravity
            .capability_matrix
            .as_ref()
            .and_then(|matrix| matrix.get("streaming"))
            .and_then(Value::as_bool),
        Some(true)
    );
    let inventory = serde_json::from_str::<Value>(DRIVER_INVENTORY_JSON).unwrap();
    let antigravity_entry = inventory["drivers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["agentId"] == "antigravity")
        .unwrap();
    assert_eq!(
        antigravity_entry["lifecycleEvidence"]["processing"],
        json!(false)
    );

    let cursor = registry.profile("cursor").unwrap();
    assert_eq!(cursor.driver_status, "implemented");
    assert_eq!(cursor.blocker, None);

    let deepseek = registry.profile("deepseek-harness").unwrap();
    assert_eq!(deepseek.driver_status, "implemented");
    assert_eq!(deepseek.readiness, "unverified");
    assert_eq!(deepseek.protocol, "deepseek-harness-sdk-stdio-jsonrpc");
    assert_eq!(deepseek.blocker, None);
    assert_eq!(deepseek.summary_codes, vec!["evidence_missing"]);
    assert_eq!(
        deepseek
            .capability_matrix
            .as_ref()
            .and_then(|matrix| matrix.get("interruptSteer"))
            .and_then(Value::as_bool),
        Some(false)
    );
}

#[test]
fn malformed_or_set_drifted_resources_fail_closed() {
    super::compose();
    assert!(parse_runtime_driver_registry("{}", READINESS_JSON).is_err());
    assert!(parse_runtime_driver_registry(DRIVER_INVENTORY_JSON, "{}").is_err());

    let mut inventory = serde_json::from_str::<Value>(DRIVER_INVENTORY_JSON).unwrap();
    inventory["drivers"].as_array_mut().unwrap().pop();
    let drifted_inventory = serde_json::to_string(&inventory).unwrap();
    assert!(parse_runtime_driver_registry(&drifted_inventory, READINESS_JSON).is_err());

    let mut readiness = serde_json::from_str::<Value>(READINESS_JSON).unwrap();
    readiness["adapters"].as_array_mut().unwrap().pop();
    let drifted_readiness = serde_json::to_string(&readiness).unwrap();
    assert!(parse_runtime_driver_registry(DRIVER_INVENTORY_JSON, &drifted_readiness).is_err());
}

#[test]
fn protocol_drift_and_incomplete_ready_claims_fail_closed() {
    super::compose();
    let mut inventory = serde_json::from_str::<Value>(DRIVER_INVENTORY_JSON).unwrap();
    inventory["drivers"][0]["runtimeProtocol"] = json!("drifted-protocol");
    let drifted_inventory = serde_json::to_string(&inventory).unwrap();
    assert!(parse_runtime_driver_registry(&drifted_inventory, READINESS_JSON).is_err());

    let mut readiness = serde_json::from_str::<Value>(READINESS_JSON).unwrap();
    let claude = readiness["adapters"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|entry| entry["agentId"] == "claude-code")
        .unwrap();
    claude["status"] = json!("ready");
    claude["sendEnabled"] = json!(true);
    let invalid_ready = serde_json::to_string(&readiness).unwrap();
    let profile = parse_runtime_driver_registry(DRIVER_INVENTORY_JSON, &invalid_ready)
        .ok()
        .and_then(|registry| registry.profile("claude-code"));
    assert!(profile.is_none());
}
