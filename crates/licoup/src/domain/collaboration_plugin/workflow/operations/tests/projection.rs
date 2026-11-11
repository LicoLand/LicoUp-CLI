use super::super::super::super::registration::AgentDestination;
use super::super::super::model::{PlannedPayloadFile, WorkflowKind};
use super::super::projection::{apply_projection, expanded_file_changes, plan_projection};
use super::sample_record;

#[test]
fn local_projection_exposes_exact_preview_without_authorizing_transfer() {
    let mut record = sample_record(WorkflowKind::LocalDeployment);
    record.local_destination = Some("local-output".to_owned());
    record.payload_files.push(PlannedPayloadFile {
        selection_id: "server-core".to_owned(),
        source_relative_path: "payload/main.json".to_owned(),
        destination_relative_path: "config/main.json".to_owned(),
        digest_sha256: "c".repeat(64),
        bytes: 4,
    });
    let projection = plan_projection(&record);
    assert_eq!(projection["status"], "planned");
    assert_eq!(projection["selectedFeatureIds"][0], "server-core");
    assert_eq!(projection["externalFileTransferAuthorized"], false);
    assert_eq!(
        projection["fileChanges"][0]["destinationRelativePath"],
        "config/main.json"
    );

    let applied = apply_projection(&record, true, None);
    assert_eq!(applied["planConsumed"], true);
    assert_eq!(applied["cleanupPending"], true);
}

#[test]
fn mcp_file_changes_expand_once_per_agent_destination() {
    let mut record = sample_record(WorkflowKind::McpInstall);
    record.payload_files.push(PlannedPayloadFile {
        selection_id: "mcp-alpha".to_owned(),
        source_relative_path: "payload/plugin.json".to_owned(),
        destination_relative_path: "mcp-alpha/plugin.json".to_owned(),
        digest_sha256: "c".repeat(64),
        bytes: 5,
    });
    for agent_id in ["cursor", "hermes"] {
        record.agent_destinations.push(AgentDestination {
            agent_id: agent_id.to_owned(),
            install_destination: format!("output/{agent_id}"),
        });
    }
    assert_eq!(expanded_file_changes(&record).len(), 2);
}
