use super::super::catalog::target_defs;
use super::super::scan_targets_with_params;
use super::super::support::display_path;
use super::test_support::{port, temp_test_dir};
use serde_json::json;
use std::collections::BTreeSet;
use std::fs;

#[test]
fn scan_includes_required_first_targets() {
    let dir = temp_test_dir("local-source-catalog");
    let scan = scan_targets_with_params(
        &port(),
        &json!({
            "portableDir": dir.to_string_lossy(),
            "runningProcessNames": []
        }),
    )
    .unwrap();
    assert_eq!(
        scan["scanScopes"],
        json!([
            "application-store",
            "package-manager",
            "executable-path",
            "local-configuration",
            "running-process"
        ])
    );
    let ids = scan["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["target"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![
            "openclaw",
            "claude-code",
            "codex",
            "code",
            "antigravity",
            "opencode",
            "copilot",
            "kilo-code",
            "cursor",
            "hermes",
            "kimi-code",
            "grok",
            "command-code",
            "pi",
            "deepseek-harness",
            "lico-agent",
            "workbuddy",
            "codebuddy",
            "trae-work",
            "trae-agent"
        ]
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn selected_target_batch_is_deduplicated_and_keeps_request_order() {
    let dir = temp_test_dir("selected-target-batch");
    let scan = scan_targets_with_params(
        &port(),
        &json!({
            "portableDir": dir.to_string_lossy(),
            "stateRoot": display_path(dir.join("client-state")),
            "targetIds": ["cursor", "codex", "cursor"],
            "runningProcessNames": [],
            "targetScanConcurrency": 2
        }),
    )
    .unwrap();
    assert!(scan.get("candidates").is_none());
    assert_eq!(
        scan["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|slot| slot["targetId"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["cursor", "codex"]
    );
    assert!(
        scan["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|slot| { slot["ok"] == true && slot["candidate"]["target"] == slot["targetId"] })
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn selected_target_batch_keeps_unknown_targets_as_fixed_failed_slots() {
    let dir = temp_test_dir("selected-target-partial");
    let scan = scan_targets_with_params(
        &port(),
        &json!({
            "portableDir": dir.to_string_lossy(),
            "stateRoot": display_path(dir.join("client-state")),
            "targetIds": ["vscode", "unknown-adapter", "code"],
            "modelCatalogTargetIds": ["vscode"],
            "runningProcessNames": []
        }),
    )
    .unwrap();
    let results = scan["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["targetId"], "code");
    assert_eq!(results[0]["ok"], true);
    assert_eq!(results[1]["targetId"], "unknown-adapter");
    assert_eq!(results[1]["ok"], false);
    assert_eq!(results[1]["error"]["code"], "target_scan_failed");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn target_ids_are_unique_and_the_projection_reports_the_supplied_port() {
    let definitions = target_defs();
    let unique = definitions
        .iter()
        .map(|definition| definition.id)
        .collect::<BTreeSet<_>>();
    assert_eq!(unique.len(), definitions.len());

    // The declarations are this crate's; which of them has a packaged driver is
    // the engines' answer and arrives through the port. The one-to-one
    // agreement between the declaration set and
    // `PACKAGED_RUNTIME_ADAPTER_IDS` needs both halves in view, so it is
    // asserted at composition:
    // `licoup-native/src/target_port.rs::tests::agreement_with_the_declarations`.
    let port = port();
    let projected = definitions
        .iter()
        .filter_map(|definition| {
            (port.runtime_driver_profile)(definition.id).map(|_| definition.id)
        })
        .collect::<BTreeSet<_>>();
    let supplied = crate::port::fixtures::declared_agent_ids()
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    assert_eq!(projected, supplied);
}

#[test]
fn scan_candidate_has_adapter_capabilities_and_supported_actions() {
    let dir = temp_test_dir("scan-caps");
    let state_root = dir.join("client-state");
    let scan = scan_targets_with_params(
        &port(),
        &json!({
            "stateRoot": display_path(state_root)
        }),
    )
    .unwrap();

    let opencode = scan["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["target"] == "opencode")
        .unwrap();
    assert_eq!(opencode["adapterStatus"], "implemented");
    assert_eq!(
        opencode["adapterCapabilities"]["configApply"],
        "unsupported"
    );
    assert_eq!(
        opencode["adapterCapabilities"]["conversationProtocol"],
        (port().runtime_driver_profile)("opencode")
            .unwrap()
            .protocol
    );
    assert_eq!(
        opencode["adapterCapabilities"]["conversationReadiness"],
        (port().runtime_driver_profile)("opencode")
            .unwrap()
            .readiness
    );
    assert_eq!(
        opencode["adapterCapabilities"]["conversationDriver"],
        "implemented"
    );
    let codex = scan["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["target"] == "codex")
        .unwrap();
    assert_eq!(codex["adapterStatus"], "implemented");
    assert_eq!(codex["adapterCapabilities"]["configApply"], "unsupported");
    // Parity evidence stays informational; a detected binary unlocks relay.
    assert_eq!(
        codex["adapterCapabilities"]["conversationReadiness"],
        (port().runtime_driver_profile)("codex").unwrap().readiness
    );
    assert_eq!(
        codex["supportedActions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a == "runtime.message.send"),
        codex["binaryPath"].as_str().is_some()
    );

    let copilot = scan["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["target"] == "copilot")
        .unwrap();
    assert_eq!(
        copilot["adapterCapabilities"]["conversationReadiness"],
        (port().runtime_driver_profile)("copilot")
            .unwrap()
            .readiness
    );
    assert_eq!(
        copilot["supportedActions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a == "runtime.message.send"),
        copilot["binaryPath"].as_str().is_some()
    );

    let cursor = scan["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["target"] == "cursor")
        .unwrap();
    assert_eq!(
        cursor["supportedActions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a == "runtime.message.send"),
        cursor["binaryPath"].as_str().is_some()
    );
}
