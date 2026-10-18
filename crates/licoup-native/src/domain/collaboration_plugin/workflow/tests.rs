use super::operations::{
    local_deployment_apply, local_deployment_plan, mcp_install_apply, mcp_install_plan,
    workflow_cancel,
};
use super::store::rewrite_record_for_test;
use crate::domain::collaboration_plugin::lifecycle::{
    disable_in, enable_in, install_apply_in, install_plan_from_directory_in,
    installed_workflow_plugin, uninstall_in,
};
use crate::domain::collaboration_plugin::manifest::{
    LOCAL_DEPLOYMENT_CAPABILITY, MANIFEST_SCHEMA, MCP_INSTALL_CAPABILITY, PLUGIN_KIND,
};
use crate::platform::client_state::ClientStateStore;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

struct Fixture {
    store: ClientStateStore,
    source_root: PathBuf,
    output_root: PathBuf,
    package_digest: String,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "lico-collaboration-workflow-{name}-{}",
            Uuid::new_v4()
        ));
        let state_root = root.join("state");
        let source = root.join("source");
        let output_root = root.join("outputs");
        fs::create_dir_all(source.join("workflows")).unwrap();
        fs::create_dir_all(source.join("payload/server-core/config")).unwrap();
        fs::create_dir_all(source.join("payload/server-tools")).unwrap();
        fs::create_dir_all(source.join("payload/mcp-alpha/lib")).unwrap();
        fs::create_dir_all(source.join("payload/mcp-beta")).unwrap();
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
            source.join("workflows/local-deployment.json"),
            serde_json::to_vec(&json!({
                "schemaVersion": "licoup.collaboration.local-deployment.v1",
                "manualOnly": true,
                "features": [
                    {"id": "server-core", "label": "Server Core", "packagePath": "payload/server-core"},
                    {"id": "server-tools", "label": "Server Tools", "packagePath": "payload/server-tools"}
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            source.join("workflows/mcp-install.json"),
            serde_json::to_vec(&json!({
                "schemaVersion": "licoup.collaboration.mcp-install.v2",
                "manualOnly": true,
                "requiresPerFileApproval": true,
                "outboundPolicy": "direct-user-exact-scope-one-shot",
                "plugins": [
                    {"id": "mcp-alpha", "label": "MCP Alpha", "packagePath": "payload/mcp-alpha", "endpoint": "https://example.invalid/mcp-alpha"},
                    {"id": "mcp-beta", "label": "MCP Beta", "packagePath": "payload/mcp-beta", "endpoint": "https://example.invalid/mcp-beta"}
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(source.join("payload/server-core/config/main.json"), b"core").unwrap();
        fs::write(source.join("payload/server-tools/tool.json"), b"tools").unwrap();
        fs::write(source.join("payload/mcp-alpha/lib/plugin.json"), b"alpha").unwrap();
        fs::write(source.join("payload/mcp-beta/plugin.json"), b"beta").unwrap();
        crate::domain::collaboration_plugin::test_support::finalize_signed_test_manifest(
            &source, manifest,
        );
        let store = ClientStateStore::new(state_root).unwrap();
        licoup_foundation::platform::file_security::ensure_private_dir(&output_root).unwrap();
        enable_in(
            &store,
            &json!({"requestOrigin": "direct-user", "confirmed": true}),
        )
        .unwrap();
        crate::domain::collaboration_plugin::test_support::import_test_runner_trust(&store);
        let plan = install_plan_from_directory_in(&store, &source).unwrap();
        let package_digest = plan["packageDigestSha256"].as_str().unwrap().to_owned();
        install_apply_in(
            &store,
            &json!({
                "requestOrigin": "direct-user",
                "planId": plan["planId"],
                "expectedDigestSha256": package_digest,
                "confirmed": true
            }),
        )
        .unwrap();
        Self {
            store,
            source_root: source,
            output_root,
            package_digest,
        }
    }

    fn local_plan(&self, destination: &Path, selected: Value) -> Value {
        local_deployment_plan(&json!({
            "stateRoot": self.store.root(),
            "requestOrigin": "direct-user",
            "selectedFeatureIds": selected,
            "destination": destination,
            "destinationConfirmed": true
        }))
        .unwrap()
    }

    fn local_apply(
        &self,
        plan: &Value,
        destination: &Path,
        selected: Value,
    ) -> anyhow::Result<Value> {
        local_deployment_apply(&json!({
            "stateRoot": self.store.root(),
            "requestOrigin": "direct-user",
            "planId": plan["planId"],
            "expectedPlanDigestSha256": plan["planDigestSha256"],
            "expectedPackageDigestSha256": plan["packageDigestSha256"],
            "selectedFeatureIds": selected,
            "destination": destination,
            "destinationConfirmed": true,
            "confirmed": true
        }))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let root = self
            .store
            .root()
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.store.root().to_path_buf());
        let _ = fs::remove_dir_all(root);
    }
}

fn mcp_destinations(fixture: &Fixture) -> Value {
    json!([
        {
            "agentId": "cursor",
            "installDestination": fixture.output_root.join("cursor-mcp"),
            "confirmed": true
        },
        {
            "agentId": "hermes",
            "installDestination": fixture.output_root.join("hermes-mcp"),
            "confirmed": true
        }
    ])
}

#[test]
fn local_plan_previews_exact_files_applies_once_and_rejects_replay() {
    let fixture = Fixture::new("local-apply");
    let destination = fixture.output_root.join("local-server");
    let plan = fixture.local_plan(&destination, json!(["server-core", "server-tools"]));
    assert_eq!(plan["workflowKind"], "local-deployment");
    assert_eq!(plan["oneTime"], true);
    assert_eq!(plan["pluginExecuted"], false);
    assert_eq!(plan["fileChanges"].as_array().unwrap().len(), 3);
    assert_eq!(plan["assemblyPlan"]["preflightPassed"], true);
    assert_eq!(plan["assemblyPlan"]["pluginCodeWillExecute"], true);
    assert_eq!(
        plan["assemblyPlan"]["pluginCodeWillExecuteDuringAssembly"],
        false
    );
    assert!(
        plan["fileChanges"]
            .as_array()
            .unwrap()
            .iter()
            .all(|change| {
                change["digestSha256"].as_str().unwrap().len() == 64
                    && change["destinationRelativePath"].is_string()
            })
    );

    let applied = fixture
        .local_apply(&plan, &destination, json!(["server-core", "server-tools"]))
        .unwrap();
    assert_eq!(applied["status"], "assembled");
    assert_eq!(applied["planConsumed"], true);
    assert_eq!(applied["pluginExecuted"], false);
    assert_eq!(
        fs::read(destination.join("server-core/config/main.json")).unwrap(),
        b"core"
    );
    assert_eq!(
        fs::read(destination.join("server-tools/tool.json")).unwrap(),
        b"tools"
    );
    assert_eq!(
        applied["localServer"]["status"],
        "assembled-awaiting-deployment"
    );
    assert_eq!(
        applied["localServer"]["runtimeCapability"],
        "platform-loopback-isolated-runtime-v1"
    );
    assert_eq!(applied["localServer"]["pluginCodeExecuted"], false);
    assert!(
        fixture
            .local_apply(&plan, &destination, json!(["server-core", "server-tools"]))
            .is_err()
    );
}

#[test]
fn local_server_assembly_plan_can_be_cancelled_before_any_destination_is_written() {
    let fixture = Fixture::new("assembly-cancel");
    let destination = fixture.output_root.join("cancelled-server");
    let plan = fixture.local_plan(&destination, json!(["server-core"]));
    let cancelled = workflow_cancel(&json!({
        "stateRoot": fixture.store.root(),
        "requestOrigin": "direct-user",
        "planId": plan["planId"],
        "expectedPlanDigestSha256": plan["planDigestSha256"],
        "expectedPackageDigestSha256": plan["packageDigestSha256"],
        "confirmed": true
    }))
    .unwrap();
    assert_eq!(cancelled["status"], "cancelled");
    assert!(!destination.exists());
    let status = crate::domain::collaboration_plugin::local_server_status(&json!({
        "stateRoot": fixture.store.root()
    }))
    .unwrap();
    assert!(status["servers"].as_array().unwrap().is_empty());
}

#[test]
fn missing_expired_and_cancelled_plans_fail_closed() {
    let fixture = Fixture::new("plan-state");
    let destination = fixture.output_root.join("missing");
    assert!(
        local_deployment_apply(&json!({
            "stateRoot": fixture.store.root(),
            "requestOrigin": "direct-user",
            "planId": Uuid::new_v4().to_string(),
            "expectedPlanDigestSha256": "0".repeat(64),
            "expectedPackageDigestSha256": fixture.package_digest,
            "selectedFeatureIds": ["server-core"],
            "destination": destination,
            "destinationConfirmed": true,
            "confirmed": true
        }))
        .is_err()
    );

    let expired_destination = fixture.output_root.join("expired");
    let plan = fixture.local_plan(&expired_destination, json!(["server-core"]));
    let record =
        rewrite_record_for_test(&fixture.store, plan["planId"].as_str().unwrap(), |record| {
            record.expires_at_epoch_seconds = 0
        })
        .unwrap();
    let mut expired = plan.clone();
    expired["planDigestSha256"] = json!(record.plan_digest_sha256);
    assert!(
        fixture
            .local_apply(&expired, &expired_destination, json!(["server-core"]))
            .is_err()
    );

    let cancelled_destination = fixture.output_root.join("cancelled");
    let cancelled = fixture.local_plan(&cancelled_destination, json!(["server-core"]));
    let response = workflow_cancel(&json!({
        "stateRoot": fixture.store.root(),
        "requestOrigin": "direct-user",
        "planId": cancelled["planId"],
        "expectedPlanDigestSha256": cancelled["planDigestSha256"],
        "expectedPackageDigestSha256": cancelled["packageDigestSha256"],
        "confirmed": true
    }))
    .unwrap();
    assert_eq!(response["status"], "cancelled");
    assert_eq!(response["workflowKind"], "local-deployment");
    assert_eq!(response["planId"], cancelled["planId"]);
    assert_eq!(response["planDigestSha256"], cancelled["planDigestSha256"]);
    assert_eq!(
        response["packageDigestSha256"],
        cancelled["packageDigestSha256"]
    );
    assert_eq!(response["planConsumed"], true);
    assert!(
        fixture
            .local_apply(&cancelled, &cancelled_destination, json!(["server-core"]))
            .is_err()
    );
}

#[test]
fn package_mutation_wrong_destination_and_wrong_selection_are_consuming_failures() {
    let fixture = Fixture::new("binding");
    let planned_destination = fixture.output_root.join("planned");
    let wrong_destination = fixture.output_root.join("wrong");
    let plan = fixture.local_plan(&planned_destination, json!(["server-core"]));
    assert!(
        fixture
            .local_apply(&plan, &wrong_destination, json!(["server-core"]))
            .is_err()
    );
    assert!(
        fixture
            .local_apply(&plan, &planned_destination, json!(["server-core"]))
            .is_err()
    );

    let selection_destination = fixture.output_root.join("selection");
    let selection_plan = fixture.local_plan(&selection_destination, json!(["server-core"]));
    assert!(
        fixture
            .local_apply(
                &selection_plan,
                &selection_destination,
                json!(["server-tools"])
            )
            .is_err()
    );

    let mutation_destination = fixture.output_root.join("mutation");
    let mutation_plan = fixture.local_plan(&mutation_destination, json!(["server-core"]));
    let installed = installed_workflow_plugin(&fixture.store).unwrap();
    fs::write(
        installed
            .package_root
            .join("payload/server-core/config/main.json"),
        b"changed",
    )
    .unwrap();
    assert!(
        fixture
            .local_apply(
                &mutation_plan,
                &mutation_destination,
                json!(["server-core"])
            )
            .is_err()
    );
    assert!(!mutation_destination.exists());
}

#[test]
fn mcp_partial_commit_rolls_back_payload_and_private_agent_registration() {
    let fixture = Fixture::new("mcp-rollback");
    let agents = mcp_destinations(&fixture);
    let plan = mcp_install_plan(&json!({
        "stateRoot": fixture.store.root(),
        "requestOrigin": "direct-user",
        "selectedPluginIds": ["mcp-alpha", "mcp-beta"],
        "agentDestinations": agents
    }))
    .unwrap();
    assert_eq!(plan["requiresPerFileApproval"], true);
    assert_eq!(plan["externalFileTransferAuthorized"], false);
    assert!(
        plan["agentRegistrations"]
            .as_array()
            .unwrap()
            .iter()
            .all(|registration| {
                registration["registration"]["externalFileTransferAuthorized"] == false
                    && registration["registration"]["outboundPolicy"]
                        == "direct-user-exact-scope-one-shot"
            })
    );
    let result = mcp_install_apply(&json!({
        "stateRoot": fixture.store.root(),
        "requestOrigin": "direct-user",
        "planId": plan["planId"],
        "expectedPlanDigestSha256": plan["planDigestSha256"],
        "expectedPackageDigestSha256": plan["packageDigestSha256"],
        "selectedPluginIds": ["mcp-alpha", "mcp-beta"],
        "agentDestinations": agents,
        "confirmed": true,
        "failAfterCommits": 1
    }));
    assert!(result.is_err());
    for path in [
        fixture.output_root.join("cursor-mcp"),
        fixture.output_root.join("hermes-mcp"),
    ] {
        assert!(!path.exists());
    }
    assert!(
        mcp_install_apply(&json!({
            "stateRoot": fixture.store.root(),
            "requestOrigin": "direct-user",
            "planId": plan["planId"],
            "expectedPlanDigestSha256": plan["planDigestSha256"],
            "expectedPackageDigestSha256": plan["packageDigestSha256"],
            "selectedPluginIds": ["mcp-alpha", "mcp-beta"],
            "agentDestinations": agents,
            "confirmed": true
        }))
        .is_err()
    );
}

#[test]
fn mcp_uncommitted_authority_with_unavailable_recovery_is_durable_and_retryable() {
    let fixture = Fixture::new("mcp-authority-unavailable");
    let destination = fixture.output_root.join("cursor-mcp-authority-retry");
    let agents = json!([{
        "agentId": "cursor",
        "installDestination": destination,
        "confirmed": true
    }]);
    let plan = mcp_install_plan(&json!({
        "stateRoot": fixture.store.root(),
        "requestOrigin": "direct-user",
        "selectedPluginIds": ["mcp-alpha"],
        "agentDestinations": agents
    }))
    .unwrap();
    let base = || {
        json!({
            "stateRoot": fixture.store.root(),
            "requestOrigin": "direct-user",
            "planId": plan["planId"],
            "expectedPlanDigestSha256": plan["planDigestSha256"],
            "expectedPackageDigestSha256": plan["packageDigestSha256"],
            "selectedPluginIds": ["mcp-alpha"],
            "agentDestinations": agents,
            "confirmed": true
        })
    };
    let mut first = base();
    first["simulateMcpAuthorityFailureBeforeCommit"] = json!(true);
    first["simulateMcpAuthorityRecoveryUnavailable"] = json!(true);

    assert!(mcp_install_apply(&first).is_err());
    assert!(destination.exists());
    assert!(
        super::mcp_transaction::pending_for_plan(&fixture.store, plan["planId"].as_str().unwrap())
            .unwrap()
    );

    let recovered = mcp_install_apply(&base()).unwrap();
    assert_eq!(recovered["status"], "applied");
    assert!(
        !super::mcp_transaction::pending_for_plan(&fixture.store, plan["planId"].as_str().unwrap())
            .unwrap()
    );
    let registration_id = plan["agentRegistrations"][0]["registrationId"]
        .as_str()
        .unwrap();
    crate::domain::collaboration_plugin::registration::load_bridge_registration(
        &fixture.store,
        "cursor",
        registration_id,
    )
    .unwrap();
    assert_eq!(
        crate::domain::collaboration_plugin::serve_mcp_bridge(
            &fixture.store,
            "cursor",
            registration_id,
        )
        .unwrap_err()
        .to_string(),
        "collaboration_mcp_authenticated_authorization_broker_unavailable"
    );
    assert!(
        crate::domain::collaboration_plugin::registration::acp_servers_in(
            &fixture.store,
            "cursor",
        )
        .unwrap()
        .is_empty()
    );
}

#[test]
fn mcp_committed_authority_recovers_after_projection_settlement_failure() {
    let fixture = Fixture::new("mcp-authority-committed");
    let destination = fixture.output_root.join("cursor-mcp-committed-retry");
    let agents = json!([{
        "agentId": "cursor",
        "installDestination": destination,
        "confirmed": true
    }]);
    let plan = mcp_install_plan(&json!({
        "stateRoot": fixture.store.root(),
        "requestOrigin": "direct-user",
        "selectedPluginIds": ["mcp-alpha"],
        "agentDestinations": agents
    }))
    .unwrap();
    let base = || {
        json!({
            "stateRoot": fixture.store.root(),
            "requestOrigin": "direct-user",
            "planId": plan["planId"],
            "expectedPlanDigestSha256": plan["planDigestSha256"],
            "expectedPackageDigestSha256": plan["packageDigestSha256"],
            "selectedPluginIds": ["mcp-alpha"],
            "agentDestinations": agents,
            "confirmed": true
        })
    };
    let mut first = base();
    first["simulateMcpProjectionFailureAfterAuthorityCommit"] = json!(true);

    assert!(mcp_install_apply(&first).is_err());
    assert!(destination.exists());
    assert!(
        super::mcp_transaction::pending_for_plan(&fixture.store, plan["planId"].as_str().unwrap())
            .unwrap()
    );

    let recovered = mcp_install_apply(&base()).unwrap();
    assert_eq!(recovered["status"], "applied");
    assert!(
        !super::mcp_transaction::pending_for_plan(&fixture.store, plan["planId"].as_str().unwrap())
            .unwrap()
    );
}

#[test]
fn mcp_registration_is_authority_bound_but_inactive_and_retired_across_lifecycle() {
    let fixture = Fixture::new("mcp-acp-registration");
    let destination = fixture.output_root.join("cursor-mcp-first");
    let agents = json!([{
        "agentId": "cursor",
        "installDestination": destination,
        "confirmed": true
    }]);
    let plan = mcp_install_plan(&json!({
        "stateRoot": fixture.store.root(),
        "requestOrigin": "direct-user",
        "selectedPluginIds": ["mcp-alpha"],
        "agentDestinations": agents
    }))
    .unwrap();
    let applied = mcp_install_apply(&json!({
        "stateRoot": fixture.store.root(),
        "requestOrigin": "direct-user",
        "planId": plan["planId"],
        "expectedPlanDigestSha256": plan["planDigestSha256"],
        "expectedPackageDigestSha256": plan["packageDigestSha256"],
        "selectedPluginIds": ["mcp-alpha"],
        "agentDestinations": agents,
        "confirmed": true
    }))
    .unwrap();
    assert_eq!(applied["agentRegistrations"][0]["registered"], true);
    assert_eq!(
        fs::read(destination.join("mcp-alpha/lib/plugin.json")).unwrap(),
        b"alpha"
    );

    assert!(
        crate::domain::collaboration_plugin::registration::acp_servers_in(
            &fixture.store,
            "cursor",
        )
        .unwrap()
        .is_empty()
    );

    let registration_id = plan["agentRegistrations"][0]["registrationId"]
        .as_str()
        .unwrap();
    crate::domain::collaboration_plugin::registration::load_bridge_registration(
        &fixture.store,
        "cursor",
        registration_id,
    )
    .unwrap();
    let registration_path = PathBuf::from(
        plan["agentRegistrations"][0]["destination"]
            .as_str()
            .unwrap(),
    );
    let registration_bytes = fs::read(&registration_path).unwrap();
    fs::write(&registration_path, b"tampered").unwrap();
    assert!(
        crate::domain::collaboration_plugin::registration::load_bridge_registration(
            &fixture.store,
            "cursor",
            registration_id,
        )
        .is_err()
    );
    fs::write(&registration_path, registration_bytes).unwrap();
    let payload_path = destination.join("mcp-alpha/lib/plugin.json");
    fs::write(&payload_path, b"tampered").unwrap();
    assert!(
        crate::domain::collaboration_plugin::registration::load_bridge_registration(
            &fixture.store,
            "cursor",
            registration_id,
        )
        .is_err()
    );
    fs::write(&payload_path, b"alpha").unwrap();

    disable_in(
        &fixture.store,
        &json!({"requestOrigin": "direct-user", "confirmed": true}),
    )
    .unwrap();
    assert!(
        crate::domain::collaboration_plugin::registration::acp_servers_in(
            &fixture.store,
            "cursor",
        )
        .unwrap()
        .is_empty()
    );
    enable_in(
        &fixture.store,
        &json!({"requestOrigin": "direct-user", "confirmed": true}),
    )
    .unwrap();
    crate::domain::collaboration_plugin::registration::load_bridge_registration(
        &fixture.store,
        "cursor",
        registration_id,
    )
    .unwrap();
    assert!(
        crate::domain::collaboration_plugin::registration::acp_servers_in(
            &fixture.store,
            "cursor",
        )
        .unwrap()
        .is_empty()
    );

    uninstall_in(
        &fixture.store,
        &json!({
            "requestOrigin": "direct-user",
            "expectedDigestSha256": fixture.package_digest,
            "confirmed": true
        }),
    )
    .unwrap();
    assert!(
        crate::domain::collaboration_plugin::registration::load_bridge_registration(
            &fixture.store,
            "cursor",
            registration_id,
        )
        .is_err()
    );

    enable_in(
        &fixture.store,
        &json!({"requestOrigin": "direct-user", "confirmed": true}),
    )
    .unwrap();
    let reinstall = install_plan_from_directory_in(&fixture.store, &fixture.source_root).unwrap();
    install_apply_in(
        &fixture.store,
        &json!({
            "requestOrigin": "direct-user",
            "planId": reinstall["planId"],
            "expectedDigestSha256": reinstall["packageDigestSha256"],
            "confirmed": true
        }),
    )
    .unwrap();
    assert!(
        crate::domain::collaboration_plugin::registration::load_bridge_registration(
            &fixture.store,
            "cursor",
            registration_id,
        )
        .is_err()
    );
}

#[test]
fn destination_replacement_sentinel_is_preserved_and_unknown_agent_is_rejected() {
    let fixture = Fixture::new("destination-race");
    let destination = fixture.output_root.join("replaced-after-plan");
    let plan = fixture.local_plan(&destination, json!(["server-core"]));
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("sentinel"), b"preserve").unwrap();
    assert!(
        fixture
            .local_apply(&plan, &destination, json!(["server-core"]))
            .is_err()
    );
    assert_eq!(fs::read(destination.join("sentinel")).unwrap(), b"preserve");

    let commit_race_destination = fixture.output_root.join("replaced-during-commit");
    let commit_race_plan = fixture.local_plan(&commit_race_destination, json!(["server-core"]));
    assert!(
        local_deployment_apply(&json!({
            "stateRoot": fixture.store.root(),
            "requestOrigin": "direct-user",
            "planId": commit_race_plan["planId"],
            "expectedPlanDigestSha256": commit_race_plan["planDigestSha256"],
            "expectedPackageDigestSha256": commit_race_plan["packageDigestSha256"],
            "selectedFeatureIds": ["server-core"],
            "destination": commit_race_destination,
            "destinationConfirmed": true,
            "confirmed": true,
            "replaceDestinationBeforeCommitIndex": 0
        }))
        .is_err()
    );
    assert_eq!(
        fs::read(commit_race_destination.join("sentinel")).unwrap(),
        b"preserve"
    );

    assert!(
        mcp_install_plan(&json!({
            "stateRoot": fixture.store.root(),
            "requestOrigin": "direct-user",
            "selectedPluginIds": ["mcp-alpha"],
            "agentDestinations": [{
                "agentId": "unknown-agent",
                "installDestination": fixture.output_root.join("unknown"),
                "confirmed": true
            }]
        }))
        .is_err()
    );
}

#[cfg(unix)]
#[test]
fn symbolic_link_destination_ancestor_is_rejected() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new("destination-symlink");
    let actual = fixture.output_root.join("actual");
    fs::create_dir(&actual).unwrap();
    let link = fixture.output_root.join("linked");
    symlink(&actual, &link).unwrap();
    assert!(
        local_deployment_plan(&json!({
            "stateRoot": fixture.store.root(),
            "requestOrigin": "direct-user",
            "selectedFeatureIds": ["server-core"],
            "destination": link.join("target"),
            "destinationConfirmed": true
        }))
        .is_err()
    );
    assert!(fs::read_dir(actual).unwrap().next().is_none());
}
