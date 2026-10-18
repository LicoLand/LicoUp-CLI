use serde_json::json;

use super::super::plan_mcp::mcp_install_plan;

#[test]
fn mcp_plan_rejects_automatic_triggers_before_package_access() {
    let error = mcp_install_plan(&json!({
        "requestOrigin": "direct-user",
        "scheduled": true
    }))
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "collaboration_workflow_automatic_trigger_forbidden"
    );
}
