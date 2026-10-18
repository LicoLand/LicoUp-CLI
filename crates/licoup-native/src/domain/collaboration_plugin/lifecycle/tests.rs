use super::*;
use crate::domain::collaboration_plugin::manifest::{
    LOCAL_DEPLOYMENT_CAPABILITY, MANIFEST_SCHEMA, MCP_INSTALL_CAPABILITY, PLUGIN_KIND,
};
use crate::platform::client_state::ClientStateStore;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

fn fixture_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "lico-collaboration-lifecycle-{name}-{}",
        Uuid::new_v4()
    ))
}

fn store(name: &str) -> ClientStateStore {
    ClientStateStore::new(fixture_root(name)).unwrap()
}

fn plugin_fixture(name: &str) -> PathBuf {
    let root = fixture_root(name);
    fs::create_dir_all(root.join("workflows")).unwrap();
    fs::create_dir_all(root.join("payload/server-core")).unwrap();
    fs::create_dir_all(root.join("payload/mcp-selected")).unwrap();
    let manifest = json!({
        "schemaVersion": MANIFEST_SCHEMA,
        "kind": PLUGIN_KIND,
        "pluginId": "licomesh-collaboration",
        "displayName": "LicoMesh Collaboration",
        "version": "1.0.0",
        "capabilities": [LOCAL_DEPLOYMENT_CAPABILITY, MCP_INSTALL_CAPABILITY],
        "workflows": {
            "localDeployment": "workflows/local-deployment.json",
            "mcpInstall": "workflows/mcp-install.json"
        }
    });
    fs::write(
            root.join("workflows/local-deployment.json"),
            br#"{"schemaVersion":"licoup.collaboration.local-deployment.v1","manualOnly":true,"features":[{"id":"server-core","label":"Server Core","packagePath":"payload/server-core"}]}"#,
        )
        .unwrap();
    fs::write(
            root.join("workflows/mcp-install.json"),
            br#"{"schemaVersion":"licoup.collaboration.mcp-install.v2","manualOnly":true,"plugins":[{"id":"selected","label":"Selected MCP","packagePath":"payload/mcp-selected","endpoint":"https://example.invalid/mcp"}],"requiresPerFileApproval":true,"outboundPolicy":"direct-user-exact-scope-one-shot"}"#,
        )
        .unwrap();
    fs::write(root.join("payload/server-core/package.json"), b"{}").unwrap();
    fs::write(root.join("payload/mcp-selected/package.json"), b"{}").unwrap();
    crate::domain::collaboration_plugin::test_support::finalize_signed_test_manifest(
        &root, manifest,
    );
    root
}

fn plan_and_digest(store: &ClientStateStore, fixture: &Path) -> (String, String) {
    enable_in(
        store,
        &json!({"requestOrigin": "direct-user", "confirmed": true}),
    )
    .unwrap();
    crate::domain::collaboration_plugin::test_support::import_test_runner_trust(store);
    let plan = install_plan_from_directory_in(store, fixture).unwrap();
    (
        plan["planId"].as_str().unwrap().to_owned(),
        plan["packageDigestSha256"].as_str().unwrap().to_owned(),
    )
}

#[test]
fn default_state_is_absent_disabled_and_never_loaded() {
    let store = store("default");
    let status = status_in(&store).unwrap();
    assert_eq!(status["capabilityEnabled"], false);
    assert_eq!(status["pluginInstalled"], false);
    assert_eq!(status["pluginLoaded"], false);
    assert_eq!(status["loadPolicy"], "explicit-command-only");
}

#[test]
fn install_requires_manual_enable_confirmation_and_exact_digest() {
    let store = store("approval");
    let fixture = plugin_fixture("approval-package");
    assert!(install_plan_from_directory_in(&store, &fixture).is_err());
    assert!(enable_in(&store, &json!({})).is_err());
    let (plan_id, digest) = plan_and_digest(&store, &fixture);
    assert!(
        install_apply_in(
            &store,
            &json!({
                "requestOrigin": "direct-user",
                "planId": plan_id,
                "expectedDigestSha256": digest,
                "confirmed": false
            }),
        )
        .is_err()
    );
    let _ = fs::remove_dir_all(fixture);
}

#[test]
fn explicit_catalog_load_disable_and_uninstall_are_separate_actions() {
    let store = store("lifecycle");
    let fixture = plugin_fixture("lifecycle-package");
    let (plan_id, digest) = plan_and_digest(&store, &fixture);
    let installed = install_apply_in(
        &store,
        &json!({
            "requestOrigin": "direct-user",
            "planId": plan_id,
            "expectedDigestSha256": digest,
            "confirmed": true
        }),
    )
    .unwrap();
    assert_eq!(installed["pluginLoaded"], false);
    let catalog = workflow_catalog_in(&store).unwrap();
    assert_eq!(catalog["pluginLoaded"], true);
    assert_eq!(
        catalog["externalTransferPolicy"],
        "direct-exact-operation-approval-required"
    );
    disable_in(
        &store,
        &json!({"requestOrigin": "direct-user", "confirmed": true}),
    )
    .unwrap();
    assert!(workflow_catalog_in(&store).is_err());
    let removed = uninstall_in(
        &store,
        &json!({
            "requestOrigin": "direct-user",
            "expectedDigestSha256": digest,
            "confirmed": true
        }),
    )
    .unwrap();
    assert_eq!(removed["pluginInstalled"], false);
    let _ = fs::remove_dir_all(fixture);
}

#[test]
fn staged_package_mutation_invalidates_the_install_plan() {
    let store = store("mutation");
    let fixture = plugin_fixture("mutation-package");
    let (plan_id, digest) = plan_and_digest(&store, &fixture);
    fs::write(
            plan_root(&store, &plan_id)
                .unwrap()
                .join("package/workflows/mcp-install.json"),
            br#"{"schemaVersion":"licoup.collaboration.mcp-install.v2","manualOnly":true,"plugins":[{"id":"changed","label":"Changed MCP","packagePath":"payload/mcp-selected","endpoint":"https://example.invalid/changed"}],"requiresPerFileApproval":true,"outboundPolicy":"direct-user-exact-scope-one-shot"}"#,
        )
        .unwrap();
    assert!(
        install_apply_in(
            &store,
            &json!({
                "requestOrigin": "direct-user",
                "planId": plan_id,
                "expectedDigestSha256": digest,
                "confirmed": true
            }),
        )
        .is_err()
    );
    let _ = fs::remove_dir_all(fixture);
}

#[test]
fn install_cancel_is_digest_bound_and_idempotent() {
    let store = store("cancel");
    let fixture = plugin_fixture("cancel-package");
    let (plan_id, digest) = plan_and_digest(&store, &fixture);
    assert!(
        install_cancel_in(
            &store,
            &json!({
                "requestOrigin": "direct-user",
                "planId": plan_id,
                "expectedDigestSha256": "0".repeat(64),
                "confirmed": true
            }),
        )
        .is_err()
    );
    let cancelled = install_cancel_in(
        &store,
        &json!({
            "requestOrigin": "direct-user",
            "planId": plan_id,
            "expectedDigestSha256": digest,
            "confirmed": true
        }),
    )
    .unwrap();
    assert_eq!(cancelled["planConsumed"], true);
    assert_eq!(cancelled["idempotentReplay"], false);
    let replay = install_cancel_in(
        &store,
        &json!({
            "requestOrigin": "direct-user",
            "planId": plan_id,
            "expectedDigestSha256": digest,
            "confirmed": true
        }),
    )
    .unwrap();
    assert_eq!(replay["idempotentReplay"], true);
    assert!(
        install_apply_in(
            &store,
            &json!({
                "requestOrigin": "direct-user",
                "planId": plan_id,
                "expectedDigestSha256": digest,
                "confirmed": true
            }),
        )
        .is_err()
    );
    let _ = fs::remove_dir_all(fixture);
}

#[test]
fn committed_install_and_uninstall_report_bounded_cleanup_pending_without_false_failure() {
    let store = store("cleanup-pending");
    let fixture = plugin_fixture("cleanup-pending-package");
    let (plan_id, digest) = plan_and_digest(&store, &fixture);
    let installed = install_apply_in(
        &store,
        &json!({
            "requestOrigin": "direct-user",
            "planId": plan_id,
            "expectedDigestSha256": digest,
            "confirmed": true,
            "simulateCleanupFailure": true
        }),
    )
    .unwrap();
    assert_eq!(installed["status"], "installed");
    assert_eq!(installed["cleanupPending"], true);
    assert_eq!(status_in(&store).unwrap()["pluginInstalled"], true);
    let cleaned = cleanup_in(
        &store,
        &json!({"requestOrigin": "direct-user", "confirmed": true}),
    )
    .unwrap();
    assert_eq!(cleaned["cleanupPending"], false);

    let removed = uninstall_in(
        &store,
        &json!({
            "requestOrigin": "direct-user",
            "expectedDigestSha256": digest,
            "confirmed": true,
            "simulateCleanupFailure": true
        }),
    )
    .unwrap();
    assert_eq!(removed["status"], "uninstalled");
    assert_eq!(removed["cleanupPending"], true);
    assert_eq!(status_in(&store).unwrap()["pluginInstalled"], false);
    let cleaned = cleanup_in(
        &store,
        &json!({"requestOrigin": "direct-user", "confirmed": true}),
    )
    .unwrap();
    assert_eq!(cleaned["cleanupPending"], false);
    let _ = fs::remove_dir_all(fixture);
}
