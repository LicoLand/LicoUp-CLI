use std::path::PathBuf;

use serde_json::{Value, json};

use super::super::super::assembly::{
    LocalAssemblyRecord, plan_projection as assembly_plan_projection,
    record_projection as assembly_record_projection,
};
use super::super::model::{WorkflowKind, WorkflowPlanRecord};

pub(super) fn plan_projection(record: &WorkflowPlanRecord) -> Value {
    json!({
        "ok": true,
        "status": "planned",
        "workflowKind": record.workflow_kind.as_str(),
        "planId": record.plan_id,
        "planDigestSha256": record.plan_digest_sha256,
        "packageDigestSha256": record.package_digest_sha256,
        "pluginId": record.plugin_id,
        "selectedFeatureIds": if record.workflow_kind == WorkflowKind::LocalDeployment { json!(record.selected_ids) } else { Value::Null },
        "selectedPluginIds": if record.workflow_kind == WorkflowKind::McpInstall { json!(record.selected_ids) } else { Value::Null },
        "destination": record.local_destination,
        "agents": record.agent_destinations,
        "fileChanges": expanded_file_changes(record),
        "agentRegistrations": record.agent_registrations.iter().map(|registration| json!({
            "agentId": registration.agent_id,
            "registrationId": registration.registration_id,
            "destination": registration.destination,
            "digestSha256": registration.digest_sha256,
            "registration": serde_json::from_str::<Value>(&registration.content).unwrap_or(Value::Null)
        })).collect::<Vec<_>>(),
        "assemblyPlan": expanded_assembly_plan(record),
        "expiresAtEpochSeconds": record.expires_at_epoch_seconds,
        "oneTime": true,
        "cancellable": true,
        "requiresDirectConfirmation": true,
        "pluginExecuted": false,
        "pluginCodeWillExecuteDuringPlanning": false,
        "selectedServerCodeWillExecuteOnDirectStart": record.workflow_kind == WorkflowKind::LocalDeployment,
        "assemblyAdapterWillExecute": record.workflow_kind == WorkflowKind::LocalDeployment,
        "vendorConfigurationModified": false,
        "agentRegistrationModified": false,
        "externalFileTransferAuthorized": false,
        "requiresPerFileApproval": record.workflow_kind == WorkflowKind::McpInstall,
        "outboundPolicy": if record.workflow_kind == WorkflowKind::McpInstall { Value::String("direct-user-exact-scope-one-shot".to_owned()) } else { Value::Null }
    })
}

fn expanded_assembly_plan(record: &WorkflowPlanRecord) -> Option<Value> {
    record.local_assembly.as_ref().map(|assembly| {
        let mut projection = assembly_plan_projection(assembly);
        projection["pluginId"] = json!(record.plugin_id);
        projection["packageDigestSha256"] = json!(record.package_digest_sha256);
        projection["selectedComponentIds"] = json!(record.selected_ids);
        projection["destination"] = json!(record.local_destination);
        projection
    })
}

pub(super) fn apply_projection(
    record: &WorkflowPlanRecord,
    cleanup_pending: bool,
    local_server: Option<&LocalAssemblyRecord>,
) -> Value {
    json!({
        "ok": true,
        "status": if record.workflow_kind == WorkflowKind::LocalDeployment { "assembled" } else { "applied" },
        "workflowKind": record.workflow_kind.as_str(),
        "planId": record.plan_id,
        "planConsumed": true,
        "packageDigestSha256": record.package_digest_sha256,
        "pluginId": record.plugin_id,
        "selectedFeatureIds": if record.workflow_kind == WorkflowKind::LocalDeployment { json!(record.selected_ids) } else { Value::Null },
        "selectedPluginIds": if record.workflow_kind == WorkflowKind::McpInstall { json!(record.selected_ids) } else { Value::Null },
        "destination": record.local_destination,
        "agents": record.agent_destinations,
        "fileChanges": expanded_file_changes(record),
        "agentRegistrations": record.agent_registrations.iter().map(|registration| json!({
            "agentId": registration.agent_id,
            "registrationId": registration.registration_id,
            "destination": registration.destination,
            "digestSha256": registration.digest_sha256,
            "registered": true
        })).collect::<Vec<_>>(),
        "localServer": local_server.map(assembly_record_projection),
        "pluginExecuted": false,
        "pluginCodeExecutedDuringApply": false,
        "selectedServerCodeWillExecuteOnDirectStart": record.workflow_kind == WorkflowKind::LocalDeployment,
        "assemblyAdapterExecuted": record.workflow_kind == WorkflowKind::LocalDeployment,
        "vendorConfigurationModified": false,
        "agentRegistrationModified": record.workflow_kind == WorkflowKind::McpInstall,
        "externalFileTransferAuthorized": false,
        "requiresPerFileApproval": record.workflow_kind == WorkflowKind::McpInstall,
        "outboundPolicy": if record.workflow_kind == WorkflowKind::McpInstall { Value::String("direct-user-exact-scope-one-shot".to_owned()) } else { Value::Null },
        "cleanupPending": cleanup_pending
    })
}

pub(super) fn expanded_file_changes(record: &WorkflowPlanRecord) -> Vec<Value> {
    match record.workflow_kind {
        WorkflowKind::LocalDeployment => {
            let destination = record.local_destination.as_deref().unwrap_or_default();
            let mut changes = record
                .payload_files
                .iter()
                .map(|file| {
                    json!({
                        "selectionId": file.selection_id,
                        "sourceRelativePath": file.source_relative_path,
                        "destination": PathBuf::from(destination).join(&file.destination_relative_path),
                        "destinationRelativePath": file.destination_relative_path,
                        "digestSha256": file.digest_sha256,
                        "bytes": file.bytes
                    })
                })
                .collect::<Vec<_>>();
            if let Some(assembly) = record.local_assembly.as_ref() {
                changes.push(json!({
                    "selectionId": "licoup-assembly-manifest",
                    "sourceRelativePath": "licoup-generated/assembly-manifest",
                    "destination": PathBuf::from(destination).join("licoup-assembly.json"),
                    "destinationRelativePath": "licoup-assembly.json",
                    "digestSha256": assembly.manifest_digest_sha256,
                    "bytes": assembly.manifest_bytes,
                    "generatedBy": assembly.assembly_adapter_id
                }));
            }
            changes
        }
        WorkflowKind::McpInstall => record
            .agent_destinations
            .iter()
            .flat_map(|agent| {
                record.payload_files.iter().map(move |file| {
                    json!({
                        "agentId": agent.agent_id,
                        "selectionId": file.selection_id,
                        "sourceRelativePath": file.source_relative_path,
                        "destination": PathBuf::from(&agent.install_destination).join(&file.destination_relative_path),
                        "destinationRelativePath": file.destination_relative_path,
                        "digestSha256": file.digest_sha256,
                        "bytes": file.bytes
                    })
                })
            })
            .collect(),
    }
}
