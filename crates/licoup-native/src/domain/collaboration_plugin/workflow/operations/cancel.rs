use anyhow::Result;
use serde_json::{Value, json};

use super::super::super::lifecycle::{client_state_store, require_direct_confirmation};
use super::super::store::{abandon_claim, cancel_claim, claim_plan};
use super::validation::{ApplyRequest, require_direct_origin, validate_expected_digests};

pub(in crate::domain::collaboration_plugin::workflow) fn workflow_cancel(
    params: &Value,
) -> Result<Value> {
    require_direct_origin(params)?;
    require_direct_confirmation(
        params,
        "collaboration_workflow_cancel_confirmation_required",
    )?;
    let request = ApplyRequest::from_params(params)?;
    let store = client_state_store(params)?;
    let _transaction =
        super::super::super::transaction::CollaborationTransactionGuard::acquire(&store)?;
    let claim = claim_plan(&store, &request.plan_id)?;
    if let Err(error) = validate_expected_digests(&claim.record, &request) {
        abandon_claim(&store, &claim);
        return Err(error);
    }
    cancel_claim(&store, &claim)?;
    Ok(json!({
        "ok": true,
        "status": "cancelled",
        "workflowKind": claim.record.workflow_kind.as_str(),
        "planId": request.plan_id,
        "planDigestSha256": claim.record.plan_digest_sha256,
        "packageDigestSha256": claim.record.package_digest_sha256,
        "pluginId": claim.record.plugin_id,
        "planConsumed": true
    }))
}
