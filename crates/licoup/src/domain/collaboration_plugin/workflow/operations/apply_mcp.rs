use anyhow::{Result, anyhow, ensure};
use serde_json::Value;

use super::super::super::lifecycle::{client_state_store, require_direct_confirmation};
use super::super::super::package::mcp_install_choices;
use super::super::super::registration::{authority_bindings, revalidate_registrations};
use super::super::mcp_transaction::{self, Phase, Recovery};
use super::super::model::WorkflowKind;
use super::super::store::claim_plan;
use super::destination_policy::{
    agent_destinations, validate_agent_destinations, validate_registration_destinations,
};
use super::package_revalidation::{inspect_current_plugin, revalidate_payload};
use super::projection::apply_projection;
use super::staging::{
    commit_staged_units, discard_staged_units, settle_mcp_apply_claim,
    simulate_plan_cleanup_failure, stage_mcp_units,
};
use super::validation::{
    ApplyRequest, require_direct_origin, selected_ids, validate_apply_binding,
};

pub(in crate::domain::collaboration_plugin::workflow) fn mcp_install_apply(
    params: &Value,
) -> Result<Value> {
    require_direct_origin(params)?;
    require_direct_confirmation(params, "collaboration_workflow_apply_confirmation_required")?;
    let request = ApplyRequest::from_params(params)?;
    let selected_ids = selected_ids(params, "selectedPluginIds")?;
    let agent_destinations = agent_destinations(params)?;
    let store = client_state_store(params)?;
    let _transaction =
        super::super::super::transaction::CollaborationTransactionGuard::acquire(&store)?;
    let claim = claim_plan(&store, &request.plan_id)?;
    let outcome = (|| -> Result<Value> {
        validate_apply_binding(&claim.record, WorkflowKind::McpInstall, &request)?;
        ensure!(
            claim.record.selected_ids == selected_ids
                && claim.record.local_destination.is_none()
                && claim.record.agent_destinations == agent_destinations,
            "collaboration_workflow_apply_selection_or_destination_mismatch"
        );
        let payload = revalidate_payload(&store, &claim.record, true)?;
        let (installed, package) = inspect_current_plugin(&store)?;
        let choices = mcp_install_choices(&package)?;
        revalidate_registrations(
            &store,
            &installed,
            &choices,
            &selected_ids,
            &agent_destinations,
            &payload,
            &claim.record.agent_registrations,
        )?;
        match mcp_transaction::recover(&store, &claim.record, &installed)? {
            Recovery::Committed => {
                return Ok(apply_projection(&claim.record, false, None));
            }
            Recovery::None | Recovery::RolledBack => {}
        }
        validate_agent_destinations(&agent_destinations)?;
        validate_registration_destinations(&agent_destinations, &claim.record.agent_registrations)?;
        let registrations = authority_bindings(
            &store,
            &agent_destinations,
            &claim.record.agent_registrations,
        )?;
        let (authority_state, authority) = super::super::super::lifecycle::verified_authority(
            &store,
            "Verify the protected authority before exact MCP registration",
        )?;
        let mut replacement = authority.authority.clone();
        replacement.add_registrations(&registrations)?;
        let units = stage_mcp_units(
            &payload,
            &claim.record.agent_registrations,
            &agent_destinations,
        )?;
        if let Err(error) = mcp_transaction::begin(&store, &claim.record, &registrations, &units) {
            discard_staged_units(&units);
            return Err(error);
        }
        let cleanup_pending = match commit_staged_units(&units, params) {
            Ok(cleanup_pending) => cleanup_pending,
            Err(error) => {
                let _ = mcp_transaction::clear(&store);
                return Err(error);
            }
        };
        mcp_transaction::advance(&store, Phase::FilesCommitted)?;
        let authority_result = if simulate_authority_failure_before_commit(params) {
            Err(anyhow!("collaboration_mcp_test_authority_failure"))
        } else {
            super::super::super::lifecycle::replace_authority(
                &store,
                authority_state,
                &authority,
                replacement,
                "Authorize the exact MCP payload and private agent registrations",
            )
        };
        if let Err(error) = authority_result {
            if simulate_authority_recovery_unavailable(params) {
                return Err(error);
            }
            return match mcp_transaction::recover(&store, &claim.record, &installed) {
                Ok(Recovery::Committed) => {
                    Ok(apply_projection(&claim.record, cleanup_pending, None))
                }
                Ok(Recovery::RolledBack | Recovery::None) | Err(_) => Err(error),
            };
        }
        if simulate_projection_failure_after_authority_commit(params) {
            return Err(anyhow!("collaboration_mcp_test_projection_failure"));
        }
        mcp_transaction::advance(&store, Phase::AuthorityCommitted)?;
        mcp_transaction::clear(&store)?;
        Ok(apply_projection(&claim.record, cleanup_pending, None))
    })();
    settle_mcp_apply_claim(
        &store,
        &claim,
        outcome,
        simulate_plan_cleanup_failure(params),
    )
}

#[cfg(test)]
fn simulate_authority_failure_before_commit(params: &Value) -> bool {
    params
        .get("simulateMcpAuthorityFailureBeforeCommit")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

#[cfg(not(test))]
fn simulate_authority_failure_before_commit(_params: &Value) -> bool {
    false
}

#[cfg(test)]
fn simulate_authority_recovery_unavailable(params: &Value) -> bool {
    params
        .get("simulateMcpAuthorityRecoveryUnavailable")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

#[cfg(not(test))]
fn simulate_authority_recovery_unavailable(_params: &Value) -> bool {
    false
}

#[cfg(test)]
fn simulate_projection_failure_after_authority_commit(params: &Value) -> bool {
    params
        .get("simulateMcpProjectionFailureAfterAuthorityCommit")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

#[cfg(not(test))]
fn simulate_projection_failure_after_authority_commit(_params: &Value) -> bool {
    false
}
