use serde_json::json;

use super::super::plan_local::local_deployment_plan;

#[test]
fn local_plan_requires_explicit_destination_confirmation_before_io() {
    let error = local_deployment_plan(&json!({
        "requestOrigin": "direct-user",
        "destinationConfirmed": false
    }))
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "collaboration_workflow_destination_confirmation_required"
    );
}
