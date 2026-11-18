use serde_json::json;

use super::super::apply_mcp::mcp_install_apply;

#[test]
fn mcp_apply_requires_direct_confirmation_before_plan_claim() {
    let error = mcp_install_apply(&json!({"requestOrigin": "direct-user"})).unwrap_err();
    assert_eq!(
        error.to_string(),
        "collaboration_workflow_apply_confirmation_required"
    );
}
