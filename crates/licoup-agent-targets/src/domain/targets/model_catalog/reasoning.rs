use super::*;

pub(super) fn reasoning_efforts_from_value(value: &Value) -> BTreeSet<String> {
    let mut efforts = BTreeSet::<String>::new();
    collect_reasoning_efforts_from_value(value, &mut efforts, 0);
    efforts
}

pub(super) fn collect_reasoning_efforts_from_value(
    value: &Value,
    efforts: &mut BTreeSet<String>,
    depth: usize,
) {
    if depth > 4 {
        return;
    }
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let normalized = normalize_model_catalog_key(key);
                if is_reasoning_option_key(&normalized) {
                    efforts.extend(option_names_from_value(child));
                }
                if normalized == "reasoning" || normalized == "thinking" {
                    collect_reasoning_efforts_from_value(child, efforts, depth + 1);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_reasoning_efforts_from_value(item, efforts, depth + 1);
            }
        }
        _ => {}
    }
}

pub(super) fn option_names_from_value(value: &Value) -> BTreeSet<String> {
    match value {
        Value::String(value) => sanitize_option_name(value).into_iter().collect(),
        Value::Array(items) => items
            .iter()
            .flat_map(option_names_from_value)
            .collect::<BTreeSet<_>>(),
        Value::Object(object) => {
            for key in [
                "displayName",
                "display_name",
                "label",
                "title",
                "name",
                "value",
                "effort",
                "level",
                "id",
            ] {
                if let Some(name) = object
                    .get(key)
                    .and_then(Value::as_str)
                    .and_then(sanitize_option_name)
                {
                    return [name].into_iter().collect();
                }
            }
            object
                .iter()
                .filter_map(|(key, value)| {
                    if value.as_bool() == Some(true) {
                        sanitize_option_name(key)
                    } else {
                        None
                    }
                })
                .collect()
        }
        _ => BTreeSet::new(),
    }
}
