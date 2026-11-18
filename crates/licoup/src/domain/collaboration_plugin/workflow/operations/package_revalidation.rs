use anyhow::{Result, ensure};

use crate::platform::client_state::ClientStateStore;

use super::super::super::lifecycle::{InstalledWorkflowPlugin, installed_workflow_plugin};
use super::super::super::package::{
    InspectedPackage, SelectedPayloadFile, inspect_package, local_deployment_choices,
    mcp_install_choices, selected_payload_files,
};
use super::super::model::{PlannedPayloadFile, WorkflowKind, WorkflowPlanRecord};
use super::destination_policy::relative_path_text;

pub(super) fn inspect_current_plugin(
    store: &ClientStateStore,
) -> Result<(InstalledWorkflowPlugin, InspectedPackage)> {
    let installed = installed_workflow_plugin(store)?;
    let package = inspect_package(&installed.package_root)?;
    ensure!(
        package.digest_sha256 == installed.digest_sha256,
        "collaboration_plugin_installed_digest_mismatch"
    );
    Ok((installed, package))
}

pub(super) fn revalidate_payload(
    store: &ClientStateStore,
    record: &WorkflowPlanRecord,
    namespace_by_selection: bool,
) -> Result<Vec<SelectedPayloadFile>> {
    let (installed, package) = inspect_current_plugin(store)?;
    ensure!(
        installed.plugin_id == record.plugin_id
            && installed.digest_sha256 == record.package_digest_sha256,
        "collaboration_workflow_installed_package_changed"
    );
    let choices = match record.workflow_kind {
        WorkflowKind::LocalDeployment => local_deployment_choices(&package)?,
        WorkflowKind::McpInstall => mcp_install_choices(&package)?,
    };
    let payload = selected_payload_files(
        &package,
        &choices,
        &record.selected_ids,
        namespace_by_selection,
    )?;
    ensure!(
        planned_payload(&payload)? == record.payload_files,
        "collaboration_workflow_payload_changed"
    );
    Ok(payload)
}

pub(super) fn planned_payload(files: &[SelectedPayloadFile]) -> Result<Vec<PlannedPayloadFile>> {
    files
        .iter()
        .map(|file| {
            Ok(PlannedPayloadFile {
                selection_id: file.selection_id.clone(),
                source_relative_path: relative_path_text(&file.source_relative_path)?,
                destination_relative_path: relative_path_text(&file.destination_relative_path)?,
                digest_sha256: file.digest_sha256.clone(),
                bytes: file.bytes.len(),
            })
        })
        .collect()
}
