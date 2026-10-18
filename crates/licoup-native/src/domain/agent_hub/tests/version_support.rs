use super::super::version_support::{
    self, NOT_COMPARABLE, OUTSIDE_SUPPORTED, SCHEME_DATE_HASH, SCHEME_SEMVER, UNDECLARED,
    VersionStatus,
};
use super::super::{catalog, recipes};
use super::support::portable_params;
use serde_json::json;

/// Every Agent whose version this Host probes has one row, so no Agent is left
/// undecided by omission.
#[test]
fn the_shipped_table_covers_every_probed_agent() {
    let params = json!({});
    for agent_id in [
        "codex",
        "cursor",
        "opencode",
        "claude-code",
        "pi",
        "openclaw",
        "hermes",
        "antigravity",
        "deepseek-harness",
    ] {
        let row = version_support::ranges_for(&params, agent_id)
            .unwrap()
            .unwrap_or_else(|| panic!("{agent_id} must have a declared version row"));
        assert!(!row.schemes.is_empty(), "{agent_id} declares no domain");
    }
    assert!(
        version_support::ranges_for(&params, "kimi-code")
            .unwrap()
            .is_none(),
        "an Agent this Host does not probe has no version row"
    );
}

/// Support is read from the table: the same version that is supported with the
/// declared row is refused once the row is gone, so nothing is admitted by
/// default.
#[test]
fn support_is_read_from_the_declared_table() {
    let params = json!({});
    let supported = version_support::support_for(&params, "codex", "0.48.0");
    assert_eq!(supported.status, VersionStatus::Supported);
    assert!(supported.allows_execution());
    assert!(supported.reason_code.is_empty());
    assert_eq!(supported.schemes, vec![SCHEME_SEMVER.to_string()]);

    let undeclared =
        version_support::support_for(&json!({ "versionRanges": [] }), "codex", "0.48.0");
    assert_eq!(undeclared.status, VersionStatus::Undeclared);
    assert_eq!(undeclared.reason_code, UNDECLARED);
    assert!(!undeclared.allows_execution());
    assert!(undeclared.needs_upgrade());
}

/// A value the declared domains cannot compare is refused rather than driven,
/// and each domain decides for itself: a build stamp is comparable for the Agent
/// that reports build stamps and not for one that reports semantic versions.
#[test]
fn a_version_outside_every_declared_domain_is_refused() {
    let params = json!({});
    for version in ["latest", "latest-stable", "vendor-latest", "nightly-build"] {
        let support = version_support::support_for(&params, "codex", version);
        assert_eq!(
            support.status,
            VersionStatus::Unsupported,
            "{version} must not be driven"
        );
        assert_eq!(support.reason_code, NOT_COMPARABLE);
    }
    let absent = version_support::support_for(&params, "codex", "   ");
    assert_eq!(absent.status, VersionStatus::Absent);
    assert!(absent.reason_code.is_empty());

    let stamp = version_support::support_for(&params, "cursor", "2025.09.17-1a2b3c4");
    assert_eq!(stamp.status, VersionStatus::Supported);
    assert_eq!(
        stamp.schemes,
        vec![SCHEME_DATE_HASH.to_string(), SCHEME_SEMVER.to_string()]
    );
    let stamp_on_a_semver_agent =
        version_support::support_for(&params, "codex", "2025.09.17-1a2b3c4");
    assert_eq!(stamp_on_a_semver_agent.status, VersionStatus::Unsupported);
    assert_eq!(stamp_on_a_semver_agent.reason_code, NOT_COMPARABLE);
}

/// A declared range decides, in both directions, and reports the range that did.
#[test]
fn a_declared_range_decides_and_names_itself() {
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
    let old = version_support::support_for(&params, "codex", "0.19.0");
    assert_eq!(old.status, VersionStatus::Unsupported);
    assert_eq!(old.reason_code, "codex_app_server_required");
    assert_eq!(old.matched_range, "<0.20.0");

    let current = version_support::support_for(&params, "codex", "0.21.4");
    assert_eq!(current.status, VersionStatus::Supported);
    assert_eq!(current.matched_range, ">=0.20.0");

    let above = version_support::support_for(
        &json!({
            "versionRanges": [{
                "agentId": "codex",
                "schemes": ["semver"],
                "supported": ["<1.0.0"]
            }]
        }),
        "codex",
        "2.0.0",
    );
    assert_eq!(above.status, VersionStatus::Unsupported);
    assert_eq!(above.reason_code, OUTSIDE_SUPPORTED);
}

/// A refused version publishes the operation that fixes it, taken from the
/// recipe's own update argv, and a supported version publishes no prompt.
#[test]
fn a_refused_version_publishes_the_recipe_own_upgrade_path() {
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

    let refused = version_support::support_for(&params, "codex", "0.19.0");
    let prompt = version_support::upgrade_prompt(Some(channel), &refused)
        .expect("a refused version has a way out");
    assert_eq!(prompt["reasonCode"], "codex_app_server_required");
    assert_eq!(prompt["installedVersion"], "0.19.0");
    assert_eq!(prompt["channelId"], "homebrew");
    assert_eq!(prompt["command"], "brew upgrade --cask codex");
    assert_eq!(prompt["operation"], "update");

    let admitted = version_support::support_for(&params, "codex", "0.21.4");
    assert!(version_support::upgrade_prompt(Some(channel), &admitted).is_none());
}

/// The production card route publishes the verdict and the prompt, so a client
/// reads a refused version and its way out from one answer.
#[test]
fn the_catalog_card_publishes_the_version_verdict_and_the_upgrade_prompt() {
    let mut params = portable_params("version-support-card").1;
    params["agentId"] = json!("codex");
    params["versionRanges"] = json!([{
        "agentId": "codex",
        "schemes": ["semver"],
        "supported": [">=0.20.0"],
        "unsupported": [
            { "range": "<0.20.0", "reason": "codex_app_server_required" }
        ]
    }]);
    params["discoveryCandidates"] = json!([{
        "target": "codex",
        "present": true,
        "installedVersion": "0.19.0"
    }]);

    let card = catalog(&params).unwrap()["cards"][0].clone();
    assert_eq!(card["installedVersion"], "0.19.0");
    assert_eq!(card["versionSupport"]["status"], "unsupported");
    assert_eq!(
        card["versionSupport"]["reasonCode"],
        "codex_app_server_required"
    );
    assert_eq!(card["versionSupport"]["allowsExecution"], false);
    assert_eq!(
        card["upgradePrompt"]["command"],
        "brew upgrade --cask codex"
    );
    assert_eq!(
        card["updateAvailable"], true,
        "a refused version offers the Agent update the client already renders"
    );

    let mut admitted = params;
    admitted["discoveryCandidates"] = json!([{
        "target": "codex",
        "present": true,
        "installedVersion": "0.21.4"
    }]);
    let card = catalog(&admitted).unwrap()["cards"][0].clone();
    assert_eq!(card["versionSupport"]["status"], "supported");
    assert_eq!(card["versionSupport"]["allowsExecution"], true);
    assert_eq!(card["upgradePrompt"], json!(null));
    assert_eq!(card["updateAvailable"], false);
}
