use super::input::json_text;
use anyhow::Result;
use serde_json::{Value, json};

/// Adapter capability projection for remote-approval bridges.
pub fn evaluate_approval_adapter_capability_json(params: &Value) -> Result<Value> {
    let agent_id =
        json_text(params, &["agentId", "agent_id", "requesterAgentId"]).unwrap_or_default();
    let (style, supported) = match agent_id.as_str() {
        "openclaw" | "hermes" | "copilot" | "cursor" | "kimi-code" => ("callback", true),
        "codex" | "claude-code" | "pi" => ("callback", true),
        "opencode" | "kilo-code" => ("polling", true),
        "antigravity" => ("unavailable", false),
        "" => ("", false),
        _ => ("unavailable", false),
    };
    Ok(json!({
        "ok": true,
        "agentId": agent_id,
        "approvalsSupported": supported,
        "permissionSelection": if style.is_empty() { Value::Null } else { Value::String(style.to_string()) },
        "remoteApprovalBridge": supported,
        "failClosedWithoutUserDecision": true,
        "localMachinePermissionIsNotUserApproval": true,
        "driversRegistryApprovalsEnabled": false,
        "note": "Adapter bridges may serialize and resume only after an explicit user decision; drivers.json approvals remain false until live evidence exists.",
    }))
}
