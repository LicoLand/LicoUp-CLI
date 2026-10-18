use super::super::catalog::target_def;
use super::super::manual::ManualTarget;
use super::super::processes::ScanContext;
use super::super::scan_merge::{
    model_catalog_params, scan_automatic_virtual_machine_target, scan_target_with_manual,
};
use super::super::support::display_path;
use super::super::virtual_machine_discovery::AutomaticVmTarget;
use super::test_support::{port, temp_test_dir};
use crate::platform::virtual_machine::SshRuntimeConnection;
use serde_json::json;

#[test]
fn scan_merge_preserves_manual_projection_and_local_model_fixture() {
    let root = temp_test_dir("scan-merge-manual");
    let config_path = root.join("manual-config.json");
    std::fs::write(&config_path, "{}").unwrap();
    let history_root = root.join("history");
    let manual = ManualTarget {
        target: "opencode".to_string(),
        label: "Manual OpenCode".to_string(),
        kind: "cli".to_string(),
        config_path: Some(config_path.clone()),
        binary_path: None,
        history_roots: vec![history_root.clone()],
        location: "local".to_string(),
        runtime_connection: None,
    };
    let params = json!({
        "homeDir": display_path(root),
        "runningProcessNames": [],
        "includeHistoryModelCatalog": false,
        "modelCatalogFixture": {
            "opencode": {
                "source": "fixture",
                "models": [{"id": "gpt-5.5", "name": "GPT-5.5"}]
            }
        }
    });
    let mut context = ScanContext::from_params(&params);

    let candidate = scan_target_with_manual(
        &port(),
        &target_def("opencode").unwrap(),
        Some(&manual),
        None,
        &mut context,
        &params,
    )
    .unwrap();

    assert!(candidate.manual);
    assert!(candidate.configured);
    assert_eq!(candidate.label, "Manual OpenCode");
    assert_eq!(
        candidate.config_path.as_deref(),
        Some(display_path(config_path).as_str())
    );
    assert_eq!(candidate.history_roots, vec![display_path(history_root)]);
    assert_eq!(
        candidate.model_catalog.as_ref().unwrap()["status"],
        "available"
    );
    assert_eq!(
        candidate.model_catalog.as_ref().unwrap()["models"][0]["name"],
        "GPT-5.5"
    );
}

#[test]
fn selected_catalog_reuses_every_manually_bound_cli() {
    let binary = std::path::Path::new("/synthetic/agent-cli");
    for (target, parameter) in [
        ("antigravity", "antigravityCliPath"),
        ("claude-code", "claudeCliPath"),
        ("codex", "codexCliPath"),
        ("cursor", "cursorCliPath"),
        ("deepseek-harness", "deepseekHarnessCliPath"),
        ("kilo-code", "kiloCliPath"),
        ("opencode", "opencodeCliPath"),
        ("pi", "piCliPath"),
    ] {
        let params = model_catalog_params(target, Some(binary), &json!({}));
        assert_eq!(params[parameter], json!(display_path(binary.to_path_buf())));
    }
}

#[test]
fn scan_merge_projects_automatic_vm_as_transient_send_ready_route() {
    let guest_home = format!("/{}", ["users", "agent"].join("/"));
    let guest_python = format!("{guest_home}/.hermes/venv/bin/python");
    let connection = SshRuntimeConnection::from_value(
        Some(&json!({
            "kind": "ssh",
            "host": "orb",
            "user": "test-machine",
            "remoteExecutable": guest_python,
            "workingDirectory": guest_home,
            "runtimeProtocol": "hermes-tui-gateway"
        })),
        "hermes",
    )
    .unwrap()
    .unwrap();
    let candidate = scan_automatic_virtual_machine_target(
        &port(),
        &target_def("hermes").unwrap(),
        &AutomaticVmTarget {
            label: "Hermes Agent - CLI · test-machine".to_string(),
            runtime_connection: connection,
        },
    );

    assert_eq!(candidate.location, "virtual-machine");
    assert_eq!(candidate.status, "detected");
    assert!(candidate.configured);
    assert!(!candidate.manual);
    assert_eq!(
        candidate.scan_source.as_deref(),
        Some("virtual-machine-orbstack")
    );
    assert!(
        candidate
            .supported_actions
            .iter()
            .any(|action| action == "runtime.message.send")
    );
    assert_eq!(
        candidate.adapter_capabilities.conversation_protocol,
        "hermes-tui-gateway-stdio-jsonrpc"
    );
    assert_eq!(
        candidate
            .adapter_capabilities
            .conversation_capability_matrix["laneFamily"],
        "rpc"
    );
    assert_eq!(
        candidate
            .adapter_capabilities
            .conversation_capability_matrix["cancel"],
        false
    );
    assert!(!candidate.detail.contains("test-machine"));
}

#[cfg(unix)]
#[test]
fn cursor_selected_catalog_uses_discovered_binary() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let root = temp_test_dir("scan-merge-cursor-cli");
    let executable = root.join("cursor-agent");
    fs::write(
        &executable,
        r#"#!/bin/sh
printf 'Available models\n\nauto - Auto (default)\ncomposer-2.5 - Composer 2.5\n'
"#,
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let manual = ManualTarget {
        target: "cursor".to_string(),
        label: "Cursor".to_string(),
        kind: "cli".to_string(),
        config_path: None,
        binary_path: Some(executable.clone()),
        history_roots: Vec::new(),
        location: "local".to_string(),
        runtime_connection: None,
    };
    let params = json!({
        "homeDir": display_path(root.clone()),
        "runningProcessNames": [],
        "includeHistoryModelCatalog": false,
        "enableAgentCliModelLookup": true,
    });
    let mut context = ScanContext::from_params(&params);
    let candidate = scan_target_with_manual(
        &port(),
        &target_def("cursor").unwrap(),
        Some(&manual),
        None,
        &mut context,
        &params,
    )
    .unwrap();

    let sources = candidate.model_catalog.as_ref().unwrap()["sources"]
        .as_array()
        .unwrap();
    assert!(sources.contains(&json!("cursor-cli")));
    let names = candidate.model_catalog.as_ref().unwrap()["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|model| model["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(names.contains(&"auto"));
    assert!(names.contains(&"composer-2.5"));
}

#[cfg(unix)]
#[test]
fn cursor_unused_scan_does_not_run_cli_models() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let root = temp_test_dir("scan-merge-cursor-unused");
    let executable = root.join("cursor-agent");
    fs::write(
        &executable,
        r#"#!/bin/sh
printf 'Available models\n\nauto - Auto (default)\n'
"#,
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let manual = ManualTarget {
        target: "cursor".to_string(),
        label: "Cursor".to_string(),
        kind: "cli".to_string(),
        config_path: None,
        binary_path: Some(executable.clone()),
        history_roots: Vec::new(),
        location: "local".to_string(),
        runtime_connection: None,
    };
    let params = json!({
        "homeDir": display_path(root.clone()),
        "runningProcessNames": [],
        "includeHistoryModelCatalog": false,
    });
    let mut context = ScanContext::from_params(&params);
    let candidate = scan_target_with_manual(
        &port(),
        &target_def("cursor").unwrap(),
        Some(&manual),
        None,
        &mut context,
        &params,
    )
    .unwrap();

    let sources = candidate.model_catalog.as_ref().unwrap()["sources"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(!sources.contains(&json!("cursor-cli")));
}

#[cfg(unix)]
#[test]
fn kilo_selected_catalog_uses_bound_extension_or_manual_binary() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let root = temp_test_dir("scan-merge-kilo-cli");
    let executable = root.join("kilo");
    fs::write(
        &executable,
        r#"#!/bin/sh
printf 'kilo/kilo-auto/free\nanthropic/claude-opus-4-6\n'
"#,
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let manual = ManualTarget {
        target: "kilo-code".to_string(),
        label: "Kilo Code".to_string(),
        kind: "cli".to_string(),
        config_path: None,
        binary_path: Some(executable),
        history_roots: Vec::new(),
        location: "local".to_string(),
        runtime_connection: None,
    };
    let params = json!({
        "homeDir": display_path(root),
        "runningProcessNames": [],
        "includeHistoryModelCatalog": false,
        "enableAgentCliModelLookup": true,
    });
    let mut context = ScanContext::from_params(&params);
    let candidate = scan_target_with_manual(
        &port(),
        &target_def("kilo-code").unwrap(),
        Some(&manual),
        None,
        &mut context,
        &params,
    )
    .unwrap();

    let catalog = candidate.model_catalog.as_ref().unwrap();
    assert_eq!(catalog["sources"], json!(["kilo-cli"]));
    let names = catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|model| model["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(names.contains(&"kilo/kilo-auto/free"));
    assert!(names.contains(&"anthropic/claude-opus-4-6"));
}
