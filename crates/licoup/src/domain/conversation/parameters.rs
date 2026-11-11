use anyhow::{Result, anyhow};
use serde_json::Value;

pub(crate) fn agent_param(params: &Value) -> Result<String> {
    text_param(params, &["agent", "agentId", "target"])
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("conversation command requires --agent"))
}

pub(crate) fn text_param(params: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| params.get(*key).and_then(Value::as_str))
        .map(|value| value.trim().to_string())
}

pub(crate) fn number_param(params: &Value, key: &str) -> Option<u64> {
    params.get(key).and_then(|value| {
        value.as_u64().or_else(|| {
            value
                .as_str()
                .and_then(|text| text.trim().parse::<u64>().ok())
        })
    })
}

pub(crate) fn string_list_param(params: &Value, keys: &[&str]) -> Vec<String> {
    keys.iter()
        .find_map(|key| params.get(*key))
        .map(|value| match value {
            Value::Array(items) => items
                .iter()
                .filter_map(Value::as_str)
                .flat_map(split_string_list)
                .collect(),
            Value::String(text) => split_string_list(text).collect(),
            _ => Vec::new(),
        })
        .unwrap_or_default()
}

pub(crate) fn param_bool(params: &Value, key: &str) -> Option<bool> {
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

fn split_string_list(value: &str) -> impl Iterator<Item = String> + '_ {
    value
        .split([',', '\n'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decodes_aliases_lists_numbers_and_flags_without_call_site_branching() {
        let params = json!({
            "agentId": " codex ",
            "sessionIds": ["one,two", "three"],
            "limit": "25",
            "archiveMode": "yes"
        });

        assert_eq!(agent_param(&params).unwrap(), "codex");
        assert_eq!(
            string_list_param(&params, &["sessionIds"]),
            vec!["one", "two", "three"]
        );
        assert_eq!(number_param(&params, "limit"), Some(25));
        assert_eq!(param_bool(&params, "archiveMode"), Some(true));
    }
}
