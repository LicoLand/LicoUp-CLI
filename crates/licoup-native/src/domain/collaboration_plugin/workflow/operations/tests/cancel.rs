use serde_json::json;

use super::super::cancel::workflow_cancel;

#[test]
fn cancellation_requires_direct_confirmation_before_plan_claim() {
    let error = workflow_cancel(&json!({"requestOrigin": "direct-user"})).unwrap_err();
    assert_eq!(
        error.to_string(),
        "collaboration_workflow_cancel_confirmation_required"
    );
}
