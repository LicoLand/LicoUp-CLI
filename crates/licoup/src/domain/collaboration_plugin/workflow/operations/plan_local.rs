use anyhow::Result;
use serde_json::Value;

use super::super::super::assembly::plan_local_assembly;
use super::super::super::lifecycle::client_state_store;
use super::super::super::package::{local_deployment_choices, selected_payload_files};
use super::super::model::WorkflowKind;
use super::super::store::{persist_plan, prepare_new_plan};
use super::destination_policy::{absolute_path_param, path_text, validate_new_destination};
use super::package_revalidation::{inspect_current_plugin, planned_payload};
use super::projection::plan_projection;
use super::validation::{new_plan_record, require_bool, require_direct_origin, selected_ids};

pub(in crate::domain::collaboration_plugin::workflow) fn local_deployment_plan(
    params: &Value,
) -> Result<Value> {
    require_direct_origin(params)?;
    require_bool(
        params,
        "destinationConfirmed",
        "collaboration_workflow_destination_confirmation_required",
    )?;
    let selected_ids = selected_ids(params, "selectedFeatureIds")?;
    let destination = absolute_path_param(params, "destination")?;
    validate_new_destination(&destination)?;

    let store = client_state_store(params)?;
    let _transaction =
        super::super::super::transaction::CollaborationTransactionGuard::acquire(&store)?;
    prepare_new_plan(&store)?;
    let (installed, package) = inspect_current_plugin(&store)?;
    let choices = local_deployment_choices(&package)?;
    let payload = selected_payload_files(&package, &choices, &selected_ids, true)?;
    let assembly = plan_local_assembly(&store, &installed, &selected_ids, &payload, params)?;
    let mut record = new_plan_record(
        WorkflowKind::LocalDeployment,
        &installed,
        selected_ids,
        Some(path_text(&destination)?),
        Vec::new(),
        planned_payload(&payload)?,
        Vec::new(),
    )?;
    record.local_assembly = Some(assembly);
    record.seal()?;
    persist_plan(&store, &record)?;
    Ok(plan_projection(&record))
}
