use anyhow::Result;
use serde_json::Value;

use super::super::super::lifecycle::client_state_store;
use super::super::super::package::{mcp_install_choices, selected_payload_files};
use super::super::super::registration::build_registrations;
use super::super::model::WorkflowKind;
use super::super::store::{persist_plan, prepare_new_plan};
use super::destination_policy::{
    agent_destinations, validate_agent_destinations, validate_registration_destinations,
};
use super::package_revalidation::{inspect_current_plugin, planned_payload};
use super::projection::plan_projection;
use super::validation::{new_plan_record, require_direct_origin, selected_ids};

pub(in crate::domain::collaboration_plugin::workflow) fn mcp_install_plan(
    params: &Value,
) -> Result<Value> {
    require_direct_origin(params)?;
    let selected_ids = selected_ids(params, "selectedPluginIds")?;
    let agent_destinations = agent_destinations(params)?;
    validate_agent_destinations(&agent_destinations)?;

    let store = client_state_store(params)?;
    let _transaction =
        super::super::super::transaction::CollaborationTransactionGuard::acquire(&store)?;
    prepare_new_plan(&store)?;
    let (installed, package) = inspect_current_plugin(&store)?;
    let choices = mcp_install_choices(&package)?;
    let payload = selected_payload_files(&package, &choices, &selected_ids, true)?;
    let registrations = build_registrations(
        &store,
        &installed,
        &choices,
        &selected_ids,
        &agent_destinations,
        &payload,
    )?;
    validate_registration_destinations(&agent_destinations, &registrations)?;
    let mut record = new_plan_record(
        WorkflowKind::McpInstall,
        &installed,
        selected_ids,
        None,
        agent_destinations,
        planned_payload(&payload)?,
        registrations,
    )?;
    record.seal()?;
    persist_plan(&store, &record)?;
    Ok(plan_projection(&record))
}
