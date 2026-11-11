use super::super::availability::{self, AvailabilityFact, EXPIRED, FRESH, SUPERSEDED, UNREADABLE};
use super::super::version_support::{self, VersionStatus};
use super::super::{catalog, recipes};
use super::support::{portable_params, test_store};
use crate::platform::client_state::ClientStateStore;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

/// The cached record's path inside one client state root.
const CACHE_RECORD: &str = "agent-hub/availability-cache.json";

/// One live catalog read of an Agent: no supplied snapshot, so this Host really
/// inspects the machine the way a per-card refresh does.
fn live_params(name: &str) -> (PathBuf, Value) {
    let (dir, mut params) = portable_params(name);
    params
        .as_object_mut()
        .expect("params must be an object")
        .remove("discoveryCandidates");
    params["agentId"] = json!("codex");
    params["liveLookup"] = json!(true);
    (dir, params)
}

fn store_of(dir: &Path) -> ClientStateStore {
    ClientStateStore::new(dir.join("client-state")).unwrap()
}

fn cache_state(params: &Value) -> String {
    catalog(params).unwrap()["cards"][0]["availabilityCache"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn installed(card: &Value) -> String {
    card["installedVersion"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// A live read observes once and the next read answers from the record instead
/// of inspecting the machine again.
#[test]
fn a_live_read_records_one_observation_and_the_next_read_reuses_it() {
    let (dir, params) = live_params("availability-reuse");
    assert_eq!(cache_state(&params), "absent", "nothing was observed yet");
    assert_eq!(
        cache_state(&params),
        FRESH,
        "the record answers the next read"
    );

    let observed = availability::lookup(
        &store_of(&dir),
        "codex",
        "homebrew",
        7,
        availability::now_unix_ms(),
    );
    assert_eq!(observed.state, FRESH);
    let fact = observed.fact.expect("the read recorded a fact");
    assert_eq!(fact.agent_id, "codex");
    assert_eq!(fact.scan_generation, 7);
}

/// A new platform scan generation retires the record, so a rescan observes
/// again.
#[test]
fn a_new_scan_generation_retires_the_cached_observation() {
    let (_dir, params) = live_params("availability-generation");
    assert_eq!(cache_state(&params), "absent");

    let mut rescanned = params;
    rescanned["platformCapabilities"] = json!({
        "os": "macos",
        "architecture": "aarch64",
        "managers": ["homebrew", "npm"],
        "scanGeneration": 8
    });
    assert_eq!(cache_state(&rescanned), SUPERSEDED);
}

/// The operation that changed the machine retires the fact by name.
#[test]
fn an_explicit_invalidation_retires_the_observation() {
    let (dir, params) = live_params("availability-invalidate");
    assert_eq!(cache_state(&params), "absent");
    assert_eq!(cache_state(&params), FRESH);
    let store = store_of(&dir);

    availability::invalidate(&store, Some("codex")).unwrap();

    let observed =
        availability::lookup(&store, "codex", "homebrew", 7, availability::now_unix_ms());
    assert_eq!(observed.state, "absent");
    assert!(observed.fact.is_none());
    assert_eq!(
        cache_state(&params),
        "absent",
        "the next read observes again"
    );
}

/// A fact older than the freshness window is not used, and what answers instead
/// is an observation this read made.
#[test]
fn a_fact_older_than_the_freshness_window_is_not_used() {
    let (dir, params) = live_params("availability-expired");
    let store = store_of(&dir);
    availability::record(
        &store,
        AvailabilityFact {
            agent_id: "codex".to_owned(),
            channel_id: "homebrew".to_owned(),
            present: true,
            installed_version: "0.0.1-stale".to_owned(),
            latest_version: String::new(),
            probed_version: "0.0.1-stale".to_owned(),
            version_status: VersionStatus::Supported.as_str().to_owned(),
            version_reason: String::new(),
            version_range: String::new(),
            observed_at_unix_ms: availability::now_unix_ms()
                - availability::FRESHNESS_WINDOW_MS
                - 1,
            scan_generation: 7,
        },
    )
    .unwrap();

    let card = catalog(&params).unwrap()["cards"][0].clone();
    assert_eq!(card["availabilityCache"], EXPIRED);
    assert_ne!(
        installed(&card),
        "0.0.1-stale",
        "the expired observation does not answer"
    );
    let refreshed =
        availability::lookup(&store, "codex", "homebrew", 7, availability::now_unix_ms());
    assert_eq!(refreshed.state, FRESH, "the read recorded a current fact");
    assert_ne!(refreshed.fact.unwrap().probed_version, "0.0.1-stale");
}

/// An unreadable record is refused instead of trusted, and the read answers from
/// an observation it makes instead.
#[test]
fn an_unreadable_record_is_not_trusted() {
    let (dir, params) = live_params("availability-unreadable");
    let store = store_of(&dir);
    let path = dir.join("client-state").join(CACHE_RECORD);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"{ not an availability cache }\n").unwrap();

    let observed =
        availability::lookup(&store, "codex", "homebrew", 7, availability::now_unix_ms());
    assert_eq!(observed.state, UNREADABLE);
    assert!(observed.fact.is_none());

    assert_eq!(cache_state(&params), UNREADABLE);
    assert_eq!(
        availability::lookup(&store, "codex", "homebrew", 7, availability::now_unix_ms()).state,
        FRESH,
        "the observation replaced the record it could not read"
    );
}

/// A read that was given a discovery snapshot observes nothing and records
/// nothing: the snapshot already carries the facts.
#[test]
fn a_read_from_a_supplied_snapshot_does_not_touch_the_cache() {
    let (dir, mut params) = portable_params("availability-not-requested");
    params["agentId"] = json!("codex");
    params["discoveryCandidates"] = json!([]);

    assert_eq!(cache_state(&params), "not-requested");
    assert!(
        !dir.join("client-state").join(CACHE_RECORD).exists(),
        "a snapshot read writes no observation"
    );
}

/// The shipped cache publishes the conditions that retire it, so a reader does
/// not have to infer them from the implementation.
#[test]
fn the_cache_publishes_its_invalidation_conditions() {
    let conditions = availability::invalidation_conditions();
    assert_eq!(
        conditions["freshnessWindowMs"],
        availability::FRESHNESS_WINDOW_MS
    );
    let listed = conditions["conditions"].as_array().unwrap();
    for required in [
        "freshness-window",
        "platform-scan-generation",
        "explicit-invalidation",
    ] {
        assert!(
            listed.iter().any(|value| value == required),
            "{required} must be a declared invalidation condition"
        );
    }
}

/// The chain the interval facts come from: the probe observes a version, the
/// declared table classifies it, the observation keeps the verdict it reached,
/// and a refused version publishes the upgrade path the client already renders.
#[test]
fn an_observed_unsupported_version_is_classified_and_offers_the_upgrade_path() {
    let params = json!({
        "versionRanges": [{
            "agentId": "codex",
            "schemes": ["semver"],
            "supported": [">=0.20.0"],
            "unsupported": [
                { "range": "<0.20.0", "reason": "codex_app_server_required" }
            ]
        }]
    });
    let registry = recipes::registry().unwrap();
    let agent = recipes::agent_recipe(registry, "codex").unwrap();
    let channel = agent
        .channels
        .iter()
        .find(|channel| channel.id == "homebrew")
        .unwrap();

    // What the probe reports is what decides.
    let observed = version_support::support_for(&params, "codex", "0.19.0");
    assert_eq!(observed.status, VersionStatus::Unsupported);
    assert_eq!(observed.reason_code, "codex_app_server_required");
    let prompt = version_support::upgrade_prompt(Some(channel), &observed)
        .expect("a refused version has a way out");
    assert_eq!(prompt["command"], "brew upgrade --cask codex");
    assert_eq!(prompt["operation"], "update");

    // The observation keeps that verdict, and the read path reads it back.
    let store = test_store("availability-chain");
    availability::record(
        &store,
        AvailabilityFact {
            agent_id: "codex".to_owned(),
            channel_id: "homebrew".to_owned(),
            present: true,
            installed_version: String::new(),
            latest_version: String::new(),
            probed_version: observed.version.clone(),
            version_status: observed.status.as_str().to_owned(),
            version_reason: observed.reason_code.clone(),
            version_range: observed.matched_range.clone(),
            observed_at_unix_ms: availability::now_unix_ms(),
            scan_generation: 7,
        },
    )
    .unwrap();
    let cached = availability::lookup(&store, "codex", "homebrew", 7, availability::now_unix_ms());
    let fact = cached.fact.expect("the recorded observation is fresh");
    let recorded = version_support::recorded(
        &fact.probed_version,
        &["semver".to_owned()],
        &fact.version_status,
        &fact.version_reason,
        &fact.version_range,
    );
    assert_eq!(recorded.status, VersionStatus::Unsupported);
    assert_eq!(recorded.reason_code, "codex_app_server_required");
    assert_eq!(
        version_support::upgrade_prompt(Some(channel), &recorded).unwrap()["command"],
        "brew upgrade --cask codex"
    );
}
