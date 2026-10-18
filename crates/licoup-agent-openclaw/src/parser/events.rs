// Privacy-safe OpenClaw update projection owned by its parser adapter.
use licoup_foundation::core::acp::{AcpSessionUpdate, AcpSessionUpdateKind};
use serde_json::{Value, json};

/// Project only fields needed by the client. OpenClaw metadata, session keys,
/// tool arguments, paths, thoughts, and user-message echoes remain private.
pub fn projected_event(update: &AcpSessionUpdate) -> Option<Value> {
    let event = match update.kind {
        AcpSessionUpdateKind::AgentMessageChunk => json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {
                "type": "text",
                "text": update.agent_message_text().unwrap_or("")
            }
        }),
        AcpSessionUpdateKind::CurrentModeUpdate => json!({
            "sessionUpdate": "current_mode_update",
            "currentModeId": update.current_mode_id().unwrap_or("")
        }),
        AcpSessionUpdateKind::UsageUpdate => json!({
            "sessionUpdate": "usage_update",
            "used": update.payload().get("used").and_then(Value::as_u64),
            "size": update.payload().get("size").and_then(Value::as_u64)
        }),
        AcpSessionUpdateKind::UserMessageChunk => {
            json!({"sessionUpdate": "user_message_chunk"})
        }
        AcpSessionUpdateKind::AgentThoughtChunk => {
            json!({"sessionUpdate": "agent_thought_chunk"})
        }
        AcpSessionUpdateKind::ToolCall => json!({"sessionUpdate": "tool_call"}),
        AcpSessionUpdateKind::ToolCallUpdate => {
            json!({"sessionUpdate": "tool_call_update"})
        }
        AcpSessionUpdateKind::Plan => json!({"sessionUpdate": "plan"}),
        AcpSessionUpdateKind::AvailableCommandsUpdate => {
            json!({"sessionUpdate": "available_commands_update"})
        }
        AcpSessionUpdateKind::ConfigOptionUpdate => {
            json!({"sessionUpdate": "config_option_update"})
        }
        AcpSessionUpdateKind::SessionInfoUpdate => return None,
    };
    Some(event)
}
