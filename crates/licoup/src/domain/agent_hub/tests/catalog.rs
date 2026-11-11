use super::super::*;
use super::support::portable_params;
use crate::domain::agent_catalog;
use crate::domain::agent_hub::contract::{
    ADAPTATION_PARTIAL, HOST_SCOPE, InstallOwnership, LIFECYCLE_AVAILABLE, OWNERSHIP_OWNED,
};
use crate::domain::agent_hub::ownership;
use crate::platform::client_state::ClientStateStore;

#[test]
fn catalog_projects_channel_classes_and_toolchain_admission() {
    let mut params = portable_params("catalog-channel-classes").1;
    params["agentId"] = serde_json::json!("codex");
    params["discoveryCandidates"] = serde_json::json!([]);
    let catalog = catalog(&params).unwrap();
    let card = &catalog["cards"][0];
    assert_eq!(card["toolchainOnly"], false);
    assert_eq!(card["firstLaunchEligible"], true);
    let channels = card["installChannels"].as_array().unwrap();
    let first = &channels[0];
    assert_eq!(first["class"], "binary");
    assert_eq!(first["requiresDeveloperToolchain"], false);
    let npm = channels
        .iter()
        .find(|channel| channel["kind"] == "npm")
        .unwrap();
    assert_eq!(npm["class"], "vendor-script");
    assert_eq!(npm["requiresDeveloperToolchain"], true);
}

#[test]
fn catalog_joins_one_discovery_snapshot_onto_supported_cards() {
    let mut params = portable_params("catalog").1;
    params["discoveryCandidates"] = serde_json::json!([
        {
            "target": "codex",
            "status": "detected",
            "present": true,
            "location": "local",
            "scanSource": "package-manager",
            "binaryPath": "private-binary-canary"
        },
        {
            "target": "openclaw",
            "status": "detected",
            "present": true,
            "location": "virtual-machine",
            "scanSource": "virtual-machine-orbstack"
        }
    ]);
    let catalog = catalog(&params).unwrap();
    assert_eq!(catalog["scanGeneration"], 7);
    assert_eq!(catalog["pluginManagementBoundary"], "adapter-plugins-only");
    let cards = catalog["cards"].as_array().unwrap();
    assert_catalog_membership(cards);
    let ids = cards
        .iter()
        .map(|card| card["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(ids.contains(&"codex"));
    assert!(ids.contains(&"kimi-code"));
    assert!(ids.contains(&"grok"));
    assert!(ids.contains(&"command-code"));
    let antigravity = cards
        .iter()
        .find(|card| card["id"] == "antigravity")
        .unwrap();
    assert_eq!(antigravity["adaptation"], ADAPTATION_PARTIAL);
    let deepseek = cards
        .iter()
        .find(|card| card["id"] == "deepseek-harness")
        .unwrap();
    assert_eq!(deepseek["adaptation"], "pending-evaluation");
    let codex = cards.iter().find(|card| card["id"] == "codex").unwrap();
    assert_eq!(codex["ownership"], "external");
    assert_eq!(codex["installable"], false);
    assert_eq!(codex["primaryAction"], "open");
    let cursor = cards.iter().find(|card| card["id"] == "cursor").unwrap();
    assert_eq!(cursor["adaptation"], "deep");
    assert_eq!(cursor["installable"], false);
    assert_eq!(cursor["primaryAction"], "install");
    assert_eq!(cursor["channelKind"], "");
    assert!(cursor["installChannels"].as_array().unwrap().is_empty());
    let openclaw = cards.iter().find(|card| card["id"] == "openclaw").unwrap();
    assert_eq!(openclaw["location"], "virtual-machine");
    assert!(
        openclaw["connectionModes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|mode| mode == "virtual-machine")
    );
    for card in cards {
        assert!(card.get("binaryPath").is_none());
        assert!(card.get("configPath").is_none());
        let label = card["label"].as_str().unwrap();
        assert!(!label.contains(" - "));
        assert!(!label.contains('–'));
        let summary = card["summary"].as_str().unwrap();
        assert!(!summary.is_empty());
        assert!(!summary.to_lowercase().contains("rank"));
        assert_eq!(card["channelKind"], "");
        assert!(card["installChannels"].as_array().unwrap().is_empty());
        let homepage = card["homepage"].as_str().unwrap();
        if homepage.is_empty() {
            continue;
        }
        assert!(homepage.starts_with("https://"));
    }
    assert_eq!(codex["homepage"], "https://developers.openai.com/codex");
    assert_eq!(codex["version"], "");
    assert_eq!(codex["installedVersion"], "");
    assert_eq!(codex["latestVersion"], "");
    assert_eq!(codex["updateAvailable"], false);
}

#[test]
fn catalog_with_agent_id_loads_that_agent_toml() {
    let mut params = portable_params("agent-toml").1;
    params["agentId"] = serde_json::json!("cursor");
    params["discoveryCandidates"] = serde_json::json!([]);
    let catalog = catalog(&params).unwrap();
    let cards = catalog["cards"].as_array().unwrap();
    assert_eq!(cards.len(), 1);
    let cursor = &cards[0];
    assert_eq!(cursor["id"], "cursor");
    assert_eq!(cursor["installable"], true);
    assert_eq!(cursor["channelKind"], "homebrew");
    let channels = cursor["installChannels"].as_array().unwrap();
    assert!(channels.iter().any(|channel| channel["id"] == "homebrew"));
    assert!(channels.iter().all(|channel| channel["kind"] != "npm"));
    assert!(
        channels
            .iter()
            .all(|channel| channel.get("installArgv").is_none())
    );
    let homebrew = channels
        .iter()
        .find(|channel| channel["id"] == "homebrew")
        .unwrap();
    assert_eq!(homebrew["officialSource"], "https://downloads.cursor.com");
    assert_eq!(homebrew["commandPreview"], "brew install --cask cursor-cli");
}

#[test]
fn catalog_does_not_emit_install_actions_for_missing_channels() {
    let mut params = portable_params("no-channel").1;
    params["platformCapabilities"] = serde_json::json!({
        "os": "windows",
        "architecture": "x86_64",
        "managers": ["npm"],
        "scanGeneration": 2
    });
    params["discoveryCandidates"] = serde_json::json!([]);
    params["agentId"] = serde_json::json!("hermes");
    let catalog = catalog(&params).unwrap();
    let hermes = catalog["cards"]
        .as_array()
        .unwrap()
        .iter()
        .find(|card| card["id"] == "hermes")
        .unwrap();
    assert_eq!(hermes["installable"], false);
    assert_eq!(hermes["primaryAction"], "unsupported");
    assert_eq!(hermes["channelKind"], "official-artifact");
    assert!(hermes["selectedChannelKind"].is_null());
    assert_eq!(hermes["version"], "");
    assert_eq!(hermes["updateAvailable"], false);
    assert!(hermes["installChannels"].as_array().unwrap().is_empty());
}

fn present_codex(params: &mut serde_json::Value) {
    params["discoveryCandidates"] = serde_json::json!([
        {
            "target": "codex",
            "status": "detected",
            "present": true,
            "location": "local"
        }
    ]);
}

fn card<'a>(catalog: &'a serde_json::Value, id: &str) -> &'a serde_json::Value {
    catalog["cards"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == id)
        .unwrap()
}

#[test]
fn catalog_marks_update_available_when_latest_is_strictly_newer() {
    let mut params = portable_params("newer").1;
    present_codex(&mut params);
    params["packageMetadata"] = serde_json::json!({
        "codex": {
            "installedVersion": "0.42.1",
            "latestVersion": "0.43.0"
        }
    });
    let catalog = catalog(&params).unwrap();
    let codex = card(&catalog, "codex");
    assert_eq!(codex["installedVersion"], "0.42.1");
    assert_eq!(codex["latestVersion"], "0.43.0");
    assert_eq!(codex["version"], "0.42.1");
    assert_eq!(codex["updateAvailable"], true);
}

#[test]
fn catalog_does_not_mark_update_when_versions_are_equal() {
    let mut params = portable_params("equal").1;
    present_codex(&mut params);
    params["packageMetadata"] = serde_json::json!({
        "codex": {
            "installedVersion": "0.42.1",
            "latestVersion": "0.42.1"
        }
    });
    let catalog = catalog(&params).unwrap();
    let codex = card(&catalog, "codex");
    assert_eq!(codex["installedVersion"], "0.42.1");
    assert_eq!(codex["latestVersion"], "0.42.1");
    assert_eq!(codex["updateAvailable"], false);
}

#[test]
fn catalog_does_not_mark_update_when_versions_are_missing() {
    let mut params = portable_params("missing").1;
    present_codex(&mut params);
    let catalog = catalog(&params).unwrap();
    let codex = card(&catalog, "codex");
    assert_eq!(codex["installedVersion"], "");
    assert_eq!(codex["latestVersion"], "");
    assert_eq!(codex["updateAvailable"], false);
}

#[test]
fn catalog_does_not_mark_update_when_versions_are_unparseable() {
    let mut params = portable_params("unparseable").1;
    present_codex(&mut params);
    params["packageMetadata"] = serde_json::json!({
        "codex": {
            "installedVersion": "latest",
            "latestVersion": "vendor-latest"
        }
    });
    let catalog = catalog(&params).unwrap();
    let codex = card(&catalog, "codex");
    assert_eq!(codex["installedVersion"], "");
    assert_eq!(codex["latestVersion"], "");
    assert_eq!(codex["updateAvailable"], false);
}

#[test]
fn catalog_prefers_owned_installed_version_over_the_word_latest() {
    let (dir, mut params) = portable_params("owned-version");
    present_codex(&mut params);
    let store = ClientStateStore::new(dir.join("client-state")).unwrap();
    ownership::save(
        &store,
        InstallOwnership {
            agent_id: "codex".to_string(),
            channel_id: "homebrew".to_string(),
            channel_kind: "homebrew".to_string(),
            package_coordinate: "codex".to_string(),
            installed_version: "0.41.0".to_string(),
            ownership: OWNERSHIP_OWNED.to_string(),
            lifecycle: LIFECYCLE_AVAILABLE.to_string(),
        },
    )
    .unwrap();
    params["packageMetadata"] = serde_json::json!({
        "codex": { "latestVersion": "0.42.1" }
    });
    let catalog = catalog(&params).unwrap();
    let codex = card(&catalog, "codex");
    assert_eq!(codex["installedVersion"], "0.41.0");
    assert_eq!(codex["latestVersion"], "0.42.1");
    assert_eq!(codex["version"], "0.41.0");
    assert_eq!(codex["updateAvailable"], true);
}

#[test]
fn catalog_uses_dedicated_version_probes_and_keeps_absent_cards_blank() {
    let mut params = portable_params("probes").1;
    params["discoveryCandidates"] = serde_json::json!([
        { "target": "codex", "present": true, "location": "local" },
        { "target": "cursor", "present": true, "location": "local" }
    ]);
    params["versionProbes"] = serde_json::json!({
        "codex": "codex-cli 0.147.0",
        "cursor": "cursor-agent 1.4.2",
        "opencode": "1.0.0"
    });
    params["packageMetadata"] = serde_json::json!({
        "opencode": { "installedVersion": "9.9.9" }
    });
    let catalog = catalog(&params).unwrap();
    assert_eq!(card(&catalog, "codex")["installedVersion"], "0.147.0");
    assert_eq!(card(&catalog, "cursor")["installedVersion"], "1.4.2");
    assert_eq!(card(&catalog, "opencode")["installedVersion"], "");
    assert_eq!(card(&catalog, "claude-code")["installedVersion"], "");
    assert_eq!(card(&catalog, "opencode")["version"], "");
    for item in catalog["cards"].as_array().unwrap() {
        let version = item["installedVersion"].as_str().unwrap();
        assert_ne!(version, "unknown");
        assert_ne!(version, "未知");
    }
}

#[test]
fn contradictory_cursor_presence_admits_strong_discovery_and_version_probe() {
    let mut params = portable_params("cursor-contradictory-presence").1;
    params["discoveryCandidates"] = serde_json::json!([{
        "target": "cursor",
        "status": "detected",
        "present": false,
        "location": "local",
        "binaryPath": "private-binary-canary"
    }]);
    params["versionProbes"] = serde_json::json!({
        "cursor": "cursor-agent 2026.08.25-3e8eec8"
    });

    let catalog = catalog(&params).unwrap();
    let cursor = card(&catalog, "cursor");
    assert_eq!(cursor["present"], true);
    assert_eq!(cursor["installedVersion"], "2026.08.25-3e8eec8");
    assert!(cursor.get("binaryPath").is_none());
}

#[test]
fn catalog_without_live_lookup_is_a_static_card_template() {
    let mut params = portable_params("static-template").1;
    params
        .as_object_mut()
        .unwrap()
        .remove("discoveryCandidates");
    let catalog = catalog(&params).unwrap();
    let cards = catalog["cards"].as_array().unwrap();
    assert_catalog_membership(cards);
    for item in cards {
        assert_eq!(item["installedVersion"], "");
        assert_eq!(item["latestVersion"], "");
        assert_eq!(item["updateAvailable"], false);
        assert_eq!(item["present"], false);
        assert!(!item["label"].as_str().unwrap().is_empty());
        assert!(!item["summary"].as_str().unwrap().is_empty());
    }
}

#[test]
fn catalog_with_agent_id_projects_one_injected_card() {
    let mut params = portable_params("one-card").1;
    present_codex(&mut params);
    params["agentId"] = serde_json::json!("codex");
    params["versionProbes"] = serde_json::json!({
        "codex": "codex-cli 0.147.0"
    });
    let catalog = catalog(&params).unwrap();
    let cards = catalog["cards"].as_array().unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0]["id"], "codex");
    assert_eq!(cards[0]["present"], true);
    assert_eq!(cards[0]["installedVersion"], "0.147.0");
}

#[test]
fn catalog_rejects_unknown_agent_id() {
    let mut params = portable_params("unknown-id").1;
    params["agentId"] = serde_json::json!("not-an-agent");
    let error = catalog(&params).unwrap_err();
    assert!(error.to_string().contains("agent_not_found"));
}

#[test]
fn catalog_accepts_kimi_code_without_an_install_recipe() {
    let mut params = portable_params("kimi-code-card").1;
    params["agentId"] = serde_json::json!("kimi-code");
    params["discoveryCandidates"] = serde_json::json!([{
        "target": "kimi-code",
        "status": "detected",
        "present": true,
        "location": "local"
    }]);
    let catalog = catalog(&params).unwrap();
    let cards = catalog["cards"].as_array().unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0]["id"], "kimi-code");
    assert_eq!(cards[0]["label"], "Kimi Code CLI");
    assert!(!cards[0]["label"].as_str().unwrap().contains('-'));
    assert_eq!(cards[0]["present"], true);
    assert_eq!(cards[0]["primaryAction"], "open");
    assert_eq!(cards[0]["installable"], false);
}

#[test]
fn catalog_rejects_scan_only_agents_without_a_runtime_lane() {
    let mut params = portable_params("unsupported-id").1;
    params["agentId"] = serde_json::json!("workbuddy");
    let error = catalog(&params).unwrap_err();
    assert!(error.to_string().contains("agent_not_found"));
}

#[test]
fn catalog_projects_discovered_grok_and_command_code() {
    let mut params = portable_params("extra-discovery").1;
    params["discoveryCandidates"] = serde_json::json!([
        {
            "target": "grok",
            "status": "detected",
            "present": true,
            "location": "local"
        },
        {
            "target": "command-code",
            "status": "detected",
            "present": true,
            "location": "local"
        },
        {
            "target": "custom-local-agent",
            "status": "detected",
            "present": true,
            "location": "local"
        }
    ]);
    let catalog = catalog(&params).unwrap();
    let cards = catalog["cards"].as_array().unwrap();
    assert_catalog_membership(cards);
    let grok = card(&catalog, "grok");
    assert_eq!(grok["present"], true);
    assert_eq!(grok["primaryAction"], "open");
    let command_code = card(&catalog, "command-code");
    assert_eq!(command_code["present"], true);
    assert_eq!(command_code["primaryAction"], "open");
    assert!(
        catalog["cards"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["id"] != "custom-local-agent")
    );
}

/// A full refresh takes the whole catalog in one command, so the batched pass
/// must return exactly the cards its per-card requests returned — same set, same
/// membership order — or the client silently shows a different catalog.
///
/// The fixture seeds an *empty* `discoveryCandidates`, and any supplied snapshot
/// suppresses live inspection. That key is removed here on purpose: without it
/// the only remaining fact source is the batched member inspection this test
/// exists to cover, and an injected process name must then surface on the card.
#[test]
fn catalog_live_lookup_resolves_every_member_in_one_pass() {
    let (_dir, params) = portable_params("catalog-live-batch");
    let mut batched_params = params.clone();
    batched_params["liveLookup"] = serde_json::json!(true);
    batched_params["syntheticDiscoveryIsolation"] = serde_json::json!(true);
    batched_params["runningProcessNames"] = serde_json::json!(["codex"]);
    batched_params
        .as_object_mut()
        .unwrap()
        .remove("discoveryCandidates");

    let batched = catalog(&batched_params).unwrap();
    let batched_cards = batched["cards"].as_array().unwrap().clone();
    assert_catalog_membership(&batched_cards);

    let expected = agent_catalog::supported_membership(std::iter::empty::<&str>())
        .into_iter()
        .map(|entry| entry.id)
        .collect::<Vec<_>>();
    let ids = batched_cards
        .iter()
        .map(|card| card["id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(ids, expected, "batched cards follow membership order");
    assert_eq!(batched["scanGeneration"], 7);
    assert_eq!(batched["hostScope"], HOST_SCOPE);

    // Proof the real batched inspection branch ran: the fixture isolates every
    // automatic host source, then supplies only the process snapshot. Codex is
    // therefore present while an absent member stays absent on every host.
    let card = |id: &str| {
        batched_cards
            .iter()
            .find(|card| card["id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(
        card("codex")["present"],
        true,
        "a batched refresh must carry inspected presence onto the card"
    );
    assert_eq!(card("openclaw")["present"], false);
}

/// A snapshot the caller already holds is authoritative: the batched request
/// must not inspect on top of it, or the client pays for live work it did not
/// ask for and its own facts lose to machine state.
#[test]
fn catalog_live_lookup_defers_to_a_supplied_snapshot() {
    let (_dir, params) = portable_params("catalog-live-supplied");
    let mut injected = params.clone();
    injected["liveLookup"] = serde_json::json!(true);
    injected["runningProcessNames"] = serde_json::json!(["codex"]);
    injected["discoveryCandidates"] = serde_json::json!([
        {"target": "openclaw", "present": true, "status": "detected",
         "location": "virtual-machine", "scanSource": "virtual-machine-orbstack"}
    ]);
    let cards = catalog(&injected).unwrap()["cards"]
        .as_array()
        .unwrap()
        .clone();
    let card = |id: &str| cards.iter().find(|card| card["id"] == id).unwrap().clone();
    // The supplied fact wins even though codex is the running process.
    assert_eq!(card("openclaw")["present"], true);
    assert_eq!(card("openclaw")["location"], "virtual-machine");
    assert_eq!(card("codex")["present"], false);
}

#[test]
fn catalog_live_lookup_keeps_membership_order_and_one_card_per_member() {
    let (_dir, params) = portable_params("catalog-live-order");
    let mut batched_params = params.clone();
    batched_params["liveLookup"] = serde_json::json!(true);
    batched_params
        .as_object_mut()
        .unwrap()
        .remove("discoveryCandidates");
    let cards = catalog(&batched_params).unwrap()["cards"]
        .as_array()
        .unwrap()
        .clone();
    let mut ids = cards
        .iter()
        .map(|card| card["id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    let total = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), total, "one card per member, no duplicates");
}

fn assert_catalog_membership(cards: &[serde_json::Value]) {
    let ids = cards
        .iter()
        .map(|card| card["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    for entry in agent_catalog::entries() {
        if entry.has_adapter {
            assert!(
                ids.contains(&entry.id.as_str()),
                "hub catalog missing supported {}",
                entry.id
            );
        } else {
            assert!(
                !ids.contains(&entry.id.as_str()),
                "hub catalog leaked unsupported {}",
                entry.id
            );
        }
    }
    assert!(!ids.contains(&"code"));
    assert!(!ids.contains(&"workbuddy"));
    assert!(!ids.contains(&"codebuddy"));
    assert!(!ids.contains(&"trae-work"));
    assert!(!ids.contains(&"trae-agent"));
    assert!(!ids.contains(&"custom-local-agent"));
}
