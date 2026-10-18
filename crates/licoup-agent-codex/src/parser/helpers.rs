use serde_json::Value;

pub(super) fn response_is_error(message: &Value) -> bool {
    message.get("error").is_some()
}

pub(super) fn request_id_matches(message: &Value, expected: i64) -> bool {
    message.get("id").is_some_and(|id| {
        id.as_i64() == Some(expected)
            || id.as_str().and_then(|value| value.parse::<i64>().ok()) == Some(expected)
    })
}

pub(super) fn matches_current_ids(
    params: &Value,
    thread_id: Option<&str>,
    turn_id: Option<&str>,
) -> bool {
    params.get("threadId").and_then(Value::as_str) == thread_id
        && params.get("turnId").and_then(Value::as_str) == turn_id
}

pub(super) fn final_agent_message(items: &[Value]) -> Option<String> {
    items.iter().rev().find_map(|item| {
        (item.get("type").and_then(Value::as_str) == Some("agentMessage"))
            .then(|| {
                item.get("text")
                    .and_then(Value::as_str)
                    .filter(|text| !text.trim().is_empty())
                    .map(str::to_string)
            })
            .flatten()
    })
}

pub(super) fn mcp_tool_name(item: &Value) -> Option<&str> {
    if !is_mcp_tool_call(item) {
        return None;
    }
    ["tool", "toolName"]
        .iter()
        .find_map(|key| item.get(*key).and_then(Value::as_str))
        .or_else(|| {
            item.get("name")
                .and_then(Value::as_str)
                .filter(|value| frozen_tool_suffix(value))
        })
        .filter(|value| valid_tool_identifier(value))
}

pub(super) fn mcp_tool_application_error(item: &Value) -> Option<(&str, &'static str)> {
    let tool_name = mcp_tool_name(item)?;
    let mut wire = String::new();
    if let Some(result) = item.get("result") {
        wire.push_str(&serde_json::to_string(result).ok()?);
    }
    if let Some(error) = item.get("error") {
        wire.push_str(&serde_json::to_string(error).ok()?);
    }
    APPLICATION_CODES
        .iter()
        .copied()
        .find(|code| wire.contains(code))
        .map(|code| (tool_name, code))
}

fn is_mcp_tool_call(item: &Value) -> bool {
    let item_type = item
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    item_type == "mcptoolcall" || item_type == "mcp_tool_call"
}

fn valid_tool_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':' | b'/')
        })
}

fn frozen_tool_suffix(value: &str) -> bool {
    [
        "lico_assistant_profiles",
        "lico_assistant_workflow_execute",
        "lico_assistant_workflow_inspect",
        "lico_assistant_workflow_cancel",
        "lico_subagents_list",
        "lico_subagent_probe",
        "lico_subagent_delegate",
        "lico_subagent_continue",
        "lico_subagent_cancel",
    ]
    .iter()
    .any(|tool| value.ends_with(*tool))
}

const APPLICATION_CODES: &[&str] = &[
    "caller_authentication_required",
    "caller_membership_binding_required",
    "caller_membership_not_authorized",
    "conversation_not_found",
    "conversation_state_unavailable",
    "conversation_working_directory_mismatch",
    "dispatch_reconciliation_required",
    "invalid_working_directory",
    "subagent_adapter_unavailable",
    "subagent_capability_unavailable",
    "subagent_caller_membership_inactive",
    "subagent_cross_conversation_rejected",
    "subagent_depth_exceeded",
    "subagent_dispatch_receipt_invalid",
    "subagent_dispatch_transition_invalid",
    "subagent_dispatch_uncertain",
    "subagent_duplicate_active_edge",
    "subagent_lineage_caller_mismatch",
    "subagent_lineage_cycle",
    "subagent_parent_dispatch_unavailable",
    "subagent_readiness_rejected",
    "subagent_resume_unavailable",
    "subagent_self_call_rejected",
    "subagent_target_invalid",
    "subagent_target_membership_inactive",
];
