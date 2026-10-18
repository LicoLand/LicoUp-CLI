use super::super::evaluate_approval_adapter_capability_json;
use serde_json::json;

#[test]
fn adapter_capability_reports_callback_agents_without_enabling_drivers_flag() {
    let capability = evaluate_approval_adapter_capability_json(&json!({
        "agentId": "openclaw"
    }))
    .unwrap();
    assert_eq!(capability["approvalsSupported"], true);
    assert_eq!(capability["permissionSelection"], "callback");
    assert_eq!(capability["driversRegistryApprovalsEnabled"], false);
    assert_eq!(capability["failClosedWithoutUserDecision"], true);
}
