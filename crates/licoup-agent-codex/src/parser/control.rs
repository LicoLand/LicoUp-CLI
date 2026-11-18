use super::CodexParser;
use crate::app_server::model::ProtocolEffect;
use serde_json::{Value, json};

impl CodexParser {
    pub(super) fn reject_server_request(&self, message: &Value) -> Option<Vec<ProtocolEffect>> {
        let request_id = message.get("id")?;
        let method = message.get("method")?.as_str()?;
        if message.get("result").is_some() || message.get("error").is_some() {
            return None;
        }

        Some(vec![ProtocolEffect::Send(decline_server_request(
            request_id, method,
        ))])
    }
}

/// Unattended PersistentTurn has no approval UI. Decline the native request and
/// leave the turn running so Codex can still emit the agent message. Aborting
/// here is what surfaces as a target diagnostic (`codex_user_interaction_required`)
/// before canary text.
fn decline_server_request(request_id: &Value, method: &str) -> Value {
    match method {
        "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => json!({
            "id": request_id,
            "result": { "decision": "decline" }
        }),
        "item/permissions/requestApproval" => json!({
            "id": request_id,
            "result": { "permissions": [] }
        }),
        "mcpServer/elicitation/request" => json!({
            "id": request_id,
            "result": { "action": "decline", "content": null }
        }),
        "item/tool/requestUserInput" | "tool/requestUserInput" => json!({
            "id": request_id,
            "result": { "decision": "decline" }
        }),
        "item/tool/call" => json!({
            "id": request_id,
            "result": { "success": false, "contentItems": [] }
        }),
        _ => json!({
            "id": request_id,
            "error": {
                "code": -32001,
                "message": "User interaction is required and was not approved by this client."
            }
        }),
    }
}
