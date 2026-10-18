use serde_json::json;

use super::super::{
    local_deployment_apply, local_deployment_plan, mcp_install_apply, mcp_install_plan,
    workflow_cancel,
};

#[test]
fn facade_routes_each_operation_through_direct_user_guards() {
    for result in [
        local_deployment_plan(&json!({})),
        local_deployment_apply(&json!({})),
        mcp_install_plan(&json!({})),
        mcp_install_apply(&json!({})),
        workflow_cancel(&json!({})),
    ] {
        assert_eq!(
            result.unwrap_err().to_string(),
            "collaboration_workflow_direct_user_origin_required"
        );
    }
}
