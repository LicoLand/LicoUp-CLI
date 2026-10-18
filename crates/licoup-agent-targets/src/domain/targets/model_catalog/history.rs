use super::*;

pub(super) fn collect_model_catalog_from_history(
    port: &AgentTargetPort,
    target: &str,
    params: &Value,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
    diagnostics: &mut Vec<Value>,
) {
    let history_params = history_model_catalog_params(target, params);
    match (port.conversation_model_catalog)(&history_params) {
        Ok(payload) => {
            let mut sources = BTreeSet::<String>::new();
            merge_model_catalog_value_into(&payload, "history", entries, &mut sources, diagnostics);
        }
        Err(error) => diagnostics.push(json!({
            "source": "history",
            "status": "failed",
            "message": error.to_string(),
        })),
    }
}

pub(super) fn history_model_catalog_params(target: &str, params: &Value) -> Value {
    let mut history_params = json!({
        "agent": target,
        "limit": param_u64(params, "historyModelCatalogLimit").unwrap_or(80),
        "historyModelCatalogFileLimit": param_u64(params, "historyModelCatalogFileLimit").unwrap_or(80),
    });
    if let Some(object) = history_params.as_object_mut() {
        for key in [
            "homeDir",
            "stateRoot",
            "historyRoot",
            "root",
            "historyRootKind",
        ] {
            if let Some(value) = params.get(key) {
                object.insert(key.to_string(), value.clone());
            }
        }
    }
    history_params
}
