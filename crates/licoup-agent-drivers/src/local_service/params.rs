use serde_json::Value;

pub fn text(params: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        params
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

pub fn u16_value(params: &Value, keys: &[&str]) -> Option<u16> {
    keys.iter().find_map(|key| {
        params
            .get(*key)
            .and_then(|value| {
                value
                    .as_u64()
                    .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
            })
            .and_then(|value| u16::try_from(value).ok())
            .filter(|value| *value != 0)
    })
}

pub fn u64_value(params: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| {
        params.get(*key).and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
        })
    })
}
