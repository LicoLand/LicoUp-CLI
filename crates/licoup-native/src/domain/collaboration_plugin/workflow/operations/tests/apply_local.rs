use serde_json::json;

use super::super::apply_local::local_deployment_apply;

#[test]
fn local_apply_requires_direct_confirmation_before_plan_claim() {
    let error = local_deployment_apply(&json!({"requestOrigin": "direct-user"})).unwrap_err();
    assert_eq!(
        error.to_string(),
        "collaboration_workflow_apply_confirmation_required"
    );
}
