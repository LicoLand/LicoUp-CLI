use anyhow::{Result, ensure};
use serde_json::Value;

use super::super::super::assembly::apply_local_assembly;
use super::super::super::lifecycle::{client_state_store, require_direct_confirmation};
use super::super::model::WorkflowKind;
use super::super::store::claim_plan;
use super::destination_policy::{absolute_path_param, path_text, validate_new_destination};
use super::package_revalidation::revalidate_payload;
use super::projection::apply_projection;
use super::staging::{settle_apply_claim, simulate_plan_cleanup_failure};
use super::validation::{
    ApplyRequest, require_bool, require_direct_origin, selected_ids, validate_apply_binding,
};

pub(in crate::domain::collaboration_plugin::workflow) fn local_deployment_apply(
    params: &Value,
) -> Result<Value> {
    require_direct_origin(params)?;
    require_direct_confirmation(params, "collaboration_workflow_apply_confirmation_required")?;
    require_bool(
        params,
        "destinationConfirmed",
        "collaboration_workflow_destination_confirmation_required",
    )?;
    let request = ApplyRequest::from_params(params)?;
    let selected_ids = selected_ids(params, "selectedFeatureIds")?;
    let destination = absolute_path_param(params, "destination")?;
    let store = client_state_store(params)?;
    let _transaction =
        super::super::super::transaction::CollaborationTransactionGuard::acquire(&store)?;
    let claim = claim_plan(&store, &request.plan_id)?;
    let outcome = (|| -> Result<Value> {
        validate_apply_binding(&claim.record, WorkflowKind::LocalDeployment, &request)?;
        ensure!(
            claim.record.selected_ids == selected_ids
                && claim.record.local_destination.as_deref()
                    == Some(path_text(&destination)?.as_str())
                && claim.record.agent_destinations.is_empty(),
            "collaboration_workflow_apply_selection_or_destination_mismatch"
        );
        validate_new_destination(&destination)?;
        let payload = revalidate_payload(&store, &claim.record, true)?;
        let (installed, _) = super::package_revalidation::inspect_current_plugin(&store)?;
        let assembly_plan = claim
            .record
            .local_assembly
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("collaboration_local_server_build_plan_missing"))?;
        let server = apply_local_assembly(
            &store,
            &installed,
            &selected_ids,
            &destination,
            &payload,
            assembly_plan,
            params,
        )?;
        Ok(apply_projection(&claim.record, false, Some(&server)))
    })();
    settle_apply_claim(
        &store,
        &claim,
        outcome,
        simulate_plan_cleanup_failure(params),
    )
}
