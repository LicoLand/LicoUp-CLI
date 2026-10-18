use super::*;

/// Kimi's model table keys are native selectors; `model` is the upstream API
/// identifier and `provider` references its configured provider table.
pub(super) fn collect_kimi_model_config(
    path: &Path,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
    diagnostics: &mut Vec<Value>,
) -> Option<String> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(_) => {
            diagnostics.push(json!({"source": "config", "status": "not-readable"}));
            return None;
        }
    };
    let Some(document) = parse_model_config_document(path, &raw) else {
        diagnostics.push(json!({"source": "config", "status": "not-parseable"}));
        return None;
    };
    if let Some(models) = document.get("models").and_then(Value::as_object) {
        for (selector, row) in models {
            let Some(provider_id) = row.get("provider").and_then(Value::as_str) else {
                continue;
            };
            let provider = document
                .get("providers")
                .and_then(|providers| providers.get(provider_id));
            let provider_label = provider
                .and_then(|provider| provider.get("name"))
                .and_then(Value::as_str);
            let upstream = row.get("model").and_then(Value::as_str).unwrap_or(selector);
            let display_name = row
                .get("display_name")
                .and_then(Value::as_str)
                .unwrap_or(upstream);
            let efforts = row
                .get("support_efforts")
                .map(option_names_from_value)
                .unwrap_or_else(|| reasoning_efforts_from_value(row));
            add_model_catalog_entry_with_provider(
                entries,
                selector,
                Some(display_name),
                Some(provider_id),
                provider_label,
                "config",
                efforts,
            );
        }
    }
    default_model_name_from_config_document(&document)
}
