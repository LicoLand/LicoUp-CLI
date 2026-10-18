use super::errors::ProtocolFailure;
use super::model::EffectiveSettings;
use super::params::{ProtocolConfig, RequestedSettings};
use serde_json::{Value, json};
use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub enum ConfigValue {
    Select(String),
    Boolean(bool),
}

#[derive(Clone, Debug)]
pub struct ConfigChange {
    pub id: String,
    pub value: ConfigValue,
}

pub fn requested_config_changes(
    settings: &RequestedSettings,
    options: &[Value],
    session_id: Option<&str>,
) -> Result<VecDeque<ConfigChange>, ProtocolFailure> {
    let mut changes = VecDeque::new();
    if let Some(model) = settings.model.as_deref() {
        push_select_change(&mut changes, options, "model", model, session_id)?;
    } else if let Some(router_default) = advertised_router_default(options) {
        // No explicit model: prefer the session-advertised routing entry
        // ("auto") over the agent's persisted current value. Real Copilot
        // advertises currentModelId gpt-5-mini while the account backend
        // rejects it (CAPIError 400); "auto" is the vendor's own
        // supported-model routing and is only engaged when the agent
        // advertises it. Agents without an "auto" value keep their own
        // session default untouched.
        changes.push_back(ConfigChange {
            id: "model".to_string(),
            value: ConfigValue::Select(router_default),
        });
    }
    if let Some(reasoning) = settings.reasoning_effort.as_deref() {
        let id = if option(options, "reasoning_effort").is_some() {
            "reasoning_effort"
        } else {
            "variant"
        };
        push_select_change(&mut changes, options, id, reasoning, session_id)?;
    }
    if let Some(mode) = settings.mode.as_deref() {
        push_select_change(&mut changes, options, "mode", mode, session_id)?;
    }
    if let Some(runtime_agent) = settings.runtime_agent.as_deref() {
        push_select_change(&mut changes, options, "agent", runtime_agent, session_id)?;
    }
    if let Some(allow_all) = settings.allow_all {
        push_boolean_change(&mut changes, options, "allow_all", allow_all, session_id)?;
    }
    Ok(changes)
}

pub fn config_request(request_id: i64, session_id: Option<&str>, change: &ConfigChange) -> Value {
    let params = match &change.value {
        ConfigValue::Select(value) => json!({
            "sessionId": session_id,
            "configId": change.id,
            "value": value
        }),
        ConfigValue::Boolean(value) => json!({
            "sessionId": session_id,
            "configId": change.id,
            "type": "boolean",
            "value": value
        }),
    };
    json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "method": "session/set_config_option",
        "params": params
    })
}

pub fn effective_settings(
    config: &ProtocolConfig,
    options: &[Value],
    modes: Option<&Value>,
) -> EffectiveSettings {
    EffectiveSettings {
        cwd: Some(config.cwd.clone()),
        model: current_select(options, "model"),
        reasoning_effort: current_select(options, "reasoning_effort")
            .or_else(|| current_select(options, "variant")),
        mode: current_select(options, "mode").or_else(|| {
            modes
                .and_then(|value| value.get("currentModeId"))
                .and_then(Value::as_str)
                .map(str::to_string)
        }),
        runtime_agent: current_select(options, "agent"),
        allow_all: current_boolean(options, "allow_all"),
        sandbox: None,
        approval_policy: current_boolean(options, "allow_all").map(Value::Bool),
    }
}

fn push_select_change(
    changes: &mut VecDeque<ConfigChange>,
    options: &[Value],
    id: &str,
    requested: &str,
    session_id: Option<&str>,
) -> Result<(), ProtocolFailure> {
    let Some(option) = option(options, id) else {
        return Err(unsupported_setting_failure(session_id));
    };
    if option.get("type").and_then(Value::as_str) != Some("select")
        || !select_value_supported(option, requested)
    {
        return Err(unsupported_setting_failure(session_id));
    }
    if option.get("currentValue").and_then(Value::as_str) != Some(requested) {
        changes.push_back(ConfigChange {
            id: id.to_string(),
            value: ConfigValue::Select(requested.to_string()),
        });
    }
    Ok(())
}

fn push_boolean_change(
    changes: &mut VecDeque<ConfigChange>,
    options: &[Value],
    id: &str,
    requested: bool,
    session_id: Option<&str>,
) -> Result<(), ProtocolFailure> {
    let Some(option) = option(options, id) else {
        return Err(unsupported_setting_failure(session_id));
    };
    if option.get("type").and_then(Value::as_str) != Some("boolean") {
        return Err(unsupported_setting_failure(session_id));
    }
    if option.get("currentValue").and_then(Value::as_bool) != Some(requested) {
        changes.push_back(ConfigChange {
            id: id.to_string(),
            value: ConfigValue::Boolean(requested),
        });
    }
    Ok(())
}

fn option<'a>(options: &'a [Value], id: &str) -> Option<&'a Value> {
    options
        .iter()
        .find(|value| value.get("id").and_then(Value::as_str) == Some(id))
}

/// The session-advertised model routing value ("auto"), present only when the
/// agent offers it and the session is not already routed through it.
fn advertised_router_default(options: &[Value]) -> Option<String> {
    const ROUTER_VALUE: &str = "auto";
    let model_option = option(options, "model")?;
    if model_option.get("type").and_then(Value::as_str) != Some("select")
        || !select_value_supported(model_option, ROUTER_VALUE)
    {
        return None;
    }
    if model_option.get("currentValue").and_then(Value::as_str) == Some(ROUTER_VALUE) {
        return None;
    }
    Some(ROUTER_VALUE.to_string())
}

fn unsupported_setting_failure(session_id: Option<&str>) -> ProtocolFailure {
    ProtocolFailure::new(
        "acp_setting_unsupported",
        "The ACP agent cannot preserve one of the requested native session settings.",
        "session/configure",
    )
    .with_session(session_id)
}

pub fn select_value_supported(option: &Value, requested: &str) -> bool {
    fn contains(options: &[Value], requested: &str) -> bool {
        options.iter().any(|item| {
            item.get("value").and_then(Value::as_str) == Some(requested)
                || item
                    .get("options")
                    .and_then(Value::as_array)
                    .is_some_and(|nested| contains(nested, requested))
        })
    }
    option
        .get("options")
        .and_then(Value::as_array)
        .is_some_and(|options| contains(options, requested))
}

pub fn setting_applied(options: &[Value], change: &ConfigChange) -> bool {
    options
        .iter()
        .find(|option| option.get("id").and_then(Value::as_str) == Some(change.id.as_str()))
        .is_some_and(|option| match &change.value {
            ConfigValue::Select(value) => {
                option.get("currentValue").and_then(Value::as_str) == Some(value.as_str())
            }
            ConfigValue::Boolean(value) => {
                option.get("currentValue").and_then(Value::as_bool) == Some(*value)
            }
        })
}

pub fn current_select(options: &[Value], id: &str) -> Option<String> {
    options
        .iter()
        .find(|option| option.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|option| option.get("currentValue"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub fn current_boolean(options: &[Value], id: &str) -> Option<bool> {
    options
        .iter()
        .find(|option| option.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|option| option.get("currentValue"))
        .and_then(Value::as_bool)
}
