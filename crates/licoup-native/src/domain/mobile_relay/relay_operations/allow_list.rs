use crate::domain::mobile_relay::support::json_param;
use crate::domain::targets;
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub(in crate::domain::mobile_relay) fn allowed_agent_ids(
    params: &Value,
    command_kind: &str,
) -> Result<Value> {
    let mut agent_ids = if matches!(
        command_kind,
        "agent.sessions.list" | "agent.sessions.describe"
    ) {
        crate::platform::runtime_adapters::PACKAGED_RUNTIME_ADAPTER_IDS
            .iter()
            .map(|agent| (*agent).to_string())
            .collect::<BTreeSet<_>>()
    } else {
        let scan = targets::scan_targets_with_params(
            &crate::target_port::agent_target_port(),
            &json!({}),
        )?;
        let candidates = scan.get("candidates").cloned().unwrap_or_else(|| json!([]));
        connectable_relay_targets(&candidates)
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|target| target.get("target").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default()
    };
    if let Some(explicit) =
        json_param(params, "allowedAgentIds").and_then(|value| value.as_array().cloned())
    {
        let explicit = explicit
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect::<BTreeSet<_>>();
        agent_ids.retain(|agent| explicit.contains(agent));
    }
    Ok(json!(agent_ids.into_iter().collect::<Vec<_>>()))
}

pub(in crate::domain::mobile_relay) fn connectable_relay_targets(value: &Value) -> Value {
    let items = value.as_array().cloned().unwrap_or_default();
    Value::Array(
        items
            .into_iter()
            .filter_map(|item| {
                let status = item
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let supports_runtime = item
                    .get("supportedActions")
                    .and_then(Value::as_array)
                    .map(|actions| {
                        actions.iter().any(|action| {
                            action.as_str().unwrap_or_default() == "runtime.message.send"
                        })
                    })
                    .unwrap_or(false);
                if status == "not-detected" || !supports_runtime {
                    return None;
                }
                let target = item
                    .get("target")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())?;
                let label = item
                    .get("label")
                    .or_else(|| item.get("name"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .unwrap_or(target);
                let kind = item
                    .get("kind")
                    .or_else(|| item.get("type"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .unwrap_or("cli");
                Some(json!({
                    "target": target,
                    "label": label,
                    "kind": kind,
                    "status": status
                }))
            })
            .collect(),
    )
}
