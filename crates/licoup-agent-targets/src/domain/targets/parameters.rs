use super::catalog::normalize_target;
use anyhow::{Result, anyhow};
use serde_json::Value;
use std::path::PathBuf;

pub(super) fn optional_path(item: &Value, key: &str) -> Option<PathBuf> {
    item.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub(super) fn optional_paths(item: &Value, key: &str) -> Vec<PathBuf> {
    match item.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .flat_map(split_path_list)
            .map(PathBuf::from)
            .collect(),
        Some(Value::String(value)) => split_path_list(value)
            .into_iter()
            .map(PathBuf::from)
            .collect(),
        _ => Vec::new(),
    }
}

pub(super) fn param_string(params: &Value, key: &str) -> Option<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub(super) fn param_u64(params: &Value, key: &str) -> Option<u64> {
    params.get(key).and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_str()?.parse::<u64>().ok())
    })
}

pub(super) fn param_bool(params: &Value, key: &str) -> Option<bool> {
    params.get(key).and_then(|value| match value {
        Value::Bool(value) => Some(*value),
        Value::String(value) => match value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Some(true),
            "false" | "0" | "no" => Some(false),
            _ => None,
        },
        _ => None,
    })
}

/// Third-party Agent CLIs and other-app Agent stores run only when the
/// caller opts in. Cold-start scan stays off so unused-agent discovery cannot
/// inherit TCC prompts. Opening that Agent's conversation interface sets this flag.
pub(super) fn agent_cli_model_lookup_enabled(params: &Value) -> bool {
    if param_bool(params, "disableAgentCliModelLookup").unwrap_or(false) {
        return false;
    }
    param_bool(params, "enableAgentCliModelLookup").unwrap_or(false)
}

/// Capability probes and model-catalog CLI lookups share this opt-in.
pub(super) fn discovered_agent_execution_requested(params: &Value) -> bool {
    agent_cli_model_lookup_enabled(params)
        || param_bool(params, "probeConversationRuntime").unwrap_or(false)
}

pub(super) fn param_paths(params: &Value, keys: &[&str]) -> Vec<PathBuf> {
    keys.iter()
        .filter_map(|key| params.get(*key))
        .flat_map(|value| match value {
            Value::Array(items) => items
                .iter()
                .filter_map(Value::as_str)
                .flat_map(split_path_list)
                .map(PathBuf::from)
                .collect::<Vec<_>>(),
            Value::String(value) => split_path_list(value)
                .into_iter()
                .map(PathBuf::from)
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect()
}

pub(super) fn target_param(params: &Value) -> Result<String> {
    params
        .get("target")
        .and_then(Value::as_str)
        .or_else(|| {
            params
                .get("positionals")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(Value::as_str)
        })
        .map(normalize_target)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("Missing --target <target>"))
}

fn split_path_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cli_parameters_decode_aliases_flags_numbers_and_paths() {
        let params = json!({
            "positionals": ["vscode"],
            "timeoutMs": "2500",
            "enabled": "yes",
            "historyRoots": ["one,two", "three"]
        });

        assert_eq!(target_param(&params).unwrap(), "code");
        assert_eq!(param_u64(&params, "timeoutMs"), Some(2500));
        assert_eq!(param_bool(&params, "enabled"), Some(true));
        assert_eq!(param_paths(&params, &["historyRoots"]).len(), 3);
        assert!(!agent_cli_model_lookup_enabled(&json!({})));
        assert!(!discovered_agent_execution_requested(&json!({})));
        assert!(agent_cli_model_lookup_enabled(&json!({
            "enableAgentCliModelLookup": true
        })));
        assert!(!agent_cli_model_lookup_enabled(&json!({
            "enableAgentCliModelLookup": true,
            "disableAgentCliModelLookup": true
        })));
        assert!(discovered_agent_execution_requested(&json!({
            "probeConversationRuntime": true
        })));
    }
}
