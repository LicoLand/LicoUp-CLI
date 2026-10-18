use super::*;
use licoup_foundation::platform::process_supervisor::run_bounded_untrusted_agent_output;
use std::time::Duration;

// Product policy: every model-catalog scan waits up to one minute.
const DEFAULT_OPENCODE_CLI_MODEL_LOOKUP_TIMEOUT_MS: u64 = 60_000;
const MIN_OPENCODE_CLI_MODEL_LOOKUP_TIMEOUT_MS: u64 = 100;
const MAX_OPENCODE_CLI_MODEL_LOOKUP_TIMEOUT_MS: u64 = 60_000;
const MAX_OPENCODE_CLI_MODEL_LOOKUP_OUTPUT_BYTES: usize = 512 * 1024;

pub(super) fn collect_opencode_model_catalog(
    config_path: Option<&Path>,
    params: &Value,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
    diagnostics: &mut Vec<Value>,
) -> bool {
    for path in opencode_config_paths(config_path, params) {
        collect_opencode_provider_catalog_from_path(&path, entries, diagnostics);
    }
    let cli_refreshed = collect_opencode_cli_model_catalog(params, entries, diagnostics);
    retain_opencode_provider_scoped_models(entries);
    cli_refreshed
}

fn opencode_config_paths(config_path: Option<&Path>, params: &Value) -> Vec<PathBuf> {
    let mut paths = Vec::<PathBuf>::new();
    if let Some(path) = config_path {
        paths.push(path.to_path_buf());
    }
    if let Some(home) = home_dir_for_model_catalog(params) {
        let config_dir = home.join(".config").join("opencode");
        paths.push(config_dir.join("opencode.jsonc"));
        paths.push(config_dir.join("opencode.json"));
    }
    paths.sort();
    paths.dedup();
    paths.into_iter().filter(|path| path.is_file()).collect()
}

fn collect_opencode_provider_catalog_from_path(
    path: &Path,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
    diagnostics: &mut Vec<Value>,
) {
    let source = "opencode-config";
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(_) => {
            diagnostics.push(json!({"source": source, "status": "not-readable"}));
            return;
        }
    };
    let Some(value) = parse_model_config_document(path, &raw) else {
        diagnostics.push(json!({"source": source, "status": "not-parseable"}));
        return;
    };
    collect_opencode_provider_catalog(&value, source, entries);
}

pub(super) fn collect_opencode_provider_catalog(
    value: &Value,
    source: &str,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
) {
    collect_opencode_provider_catalog_inner(value, source, entries, 0);
}

fn collect_opencode_provider_catalog_inner(
    value: &Value,
    source: &str,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
    depth: usize,
) {
    if depth > 6 {
        return;
    }
    let Some(object) = value.as_object() else {
        if let Value::Array(items) = value {
            for item in items {
                collect_opencode_provider_catalog_inner(item, source, entries, depth + 1);
            }
        }
        return;
    };
    for key in ["provider", "providers"] {
        if let Some(providers) = object.get(key) {
            match providers {
                Value::Object(providers) => {
                    for (provider_id, provider) in providers {
                        collect_opencode_provider_entry(provider_id, provider, source, entries);
                    }
                }
                Value::Array(providers) => {
                    for provider in providers {
                        let provider_id = provider
                            .get("id")
                            .or_else(|| provider.get("providerID"))
                            .or_else(|| provider.get("providerId"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        collect_opencode_provider_entry(provider_id, provider, source, entries);
                    }
                }
                _ => {}
            }
        }
    }
    for (key, child) in object {
        if matches!(
            normalize_model_catalog_key(key).as_str(),
            "provider" | "providers"
        ) {
            continue;
        }
        collect_opencode_provider_catalog_inner(child, source, entries, depth + 1);
    }
}

fn collect_opencode_provider_entry(
    provider_id: &str,
    provider: &Value,
    source: &str,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
) {
    let Some(provider_id) = sanitize_option_name(provider_id) else {
        return;
    };
    let Some(models) = provider.get("models") else {
        return;
    };
    match models {
        Value::Object(models) => {
            for (model_id, model) in models {
                add_opencode_model(&provider_id, model_id, Some(model), source, entries);
            }
        }
        Value::Array(models) => {
            for model in models {
                let model_id = model_name_from_value(model);
                add_opencode_model(&provider_id, &model_id, Some(model), source, entries);
            }
        }
        Value::String(model_id) => {
            add_opencode_model(&provider_id, model_id, None, source, entries);
        }
        _ => {}
    }
}

fn add_opencode_model(
    provider_id: &str,
    model_id: &str,
    model: Option<&Value>,
    source: &str,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
) {
    let Some(model_id) = sanitize_model_name(model_id) else {
        return;
    };
    if model_id.eq_ignore_ascii_case(provider_id) {
        return;
    }
    let qualified = if model_id.contains('/') {
        model_id
    } else {
        format!("{provider_id}/{model_id}")
    };
    let display_name = model.and_then(|value| model_display_name_from_value(value, &qualified));
    let efforts = model.map(reasoning_efforts_from_value).unwrap_or_default();
    add_model_catalog_entry_with_provider(
        entries,
        &qualified,
        display_name.as_deref(),
        Some(provider_id),
        provider_label_from_provider_id(provider_id).as_deref(),
        source,
        efforts,
    );
}

pub(super) fn retain_opencode_provider_scoped_models(
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
) {
    collapse_opencode_unqualified_duplicates(entries);
    if !entries.values().any(|entry| entry.name.contains('/')) {
        return;
    }
    // Generic config walking can promote display names and bare ids. Once the
    // OpenCode adapter has provider-scoped selectors, keep only those.
    entries.retain(|_, entry| entry.name.contains('/'));
}

pub(super) fn collapse_opencode_unqualified_duplicates(
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
) {
    let qualified_tails = entries
        .iter()
        .filter_map(|(key, entry)| {
            let (_, model) = entry.name.split_once('/')?;
            Some((model.to_ascii_lowercase(), key.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    let duplicates = entries
        .iter()
        .filter(|(_, entry)| !entry.name.contains('/'))
        .filter_map(|(key, entry)| {
            qualified_tails
                .get(&entry.name.to_ascii_lowercase())
                .cloned()
                .map(|qualified_key| (key.clone(), qualified_key))
        })
        .collect::<Vec<_>>();
    for (duplicate_key, qualified_key) in duplicates {
        let Some(duplicate) = entries.remove(&duplicate_key) else {
            continue;
        };
        let Some(canonical) = entries.get_mut(&qualified_key) else {
            continue;
        };
        canonical.sources.extend(duplicate.sources);
        canonical.extend_reasoning_efforts(duplicate.reasoning_efforts);
    }
}

fn collect_opencode_cli_model_catalog(
    params: &Value,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
    diagnostics: &mut Vec<Value>,
) -> bool {
    let source = "opencode-cli:models";
    if !agent_cli_model_lookup_enabled(params)
        || param_bool(params, "disableOpencodeCliModelLookup").unwrap_or(false)
    {
        diagnostics.push(json!({"source": source, "status": "disabled"}));
        return false;
    }

    let program = param_string(params, "opencodeCliPath")
        .or_else(|| param_string(params, "opencodePath"))
        .map(PathBuf::from)
        .or_else(|| find_binary(&["opencode"]));
    let Some(program) = program else {
        diagnostics.push(json!({"source": source, "status": "binary-unavailable"}));
        return false;
    };
    if !crate::domain::targets::scan_paths::discovered_agent_may_execute(&program, true) {
        diagnostics.push(json!({"source": source, "status": "execution-denied"}));
        return false;
    }
    let timeout_ms = param_u64(params, "opencodeCliModelLookupTimeoutMs")
        .or_else(|| param_u64(params, "agentCliModelLookupTimeoutMs"))
        .unwrap_or(DEFAULT_OPENCODE_CLI_MODEL_LOOKUP_TIMEOUT_MS)
        .clamp(
            MIN_OPENCODE_CLI_MODEL_LOOKUP_TIMEOUT_MS,
            MAX_OPENCODE_CLI_MODEL_LOOKUP_TIMEOUT_MS,
        );
    let mut command = Command::new(program);
    command.arg("models");
    // Applied for uniformity with the other catalog lookups; this lookup runs
    // through the deliberately scrubbed untrusted-agent runner, whose
    // env_clear + whitelist is applied afterwards and still wins.
    crate::platform::user_shell_environment::apply_to_command(&mut command);
    let Ok(output) = run_bounded_untrusted_agent_output(
        &mut command,
        Duration::from_millis(timeout_ms),
        MAX_OPENCODE_CLI_MODEL_LOOKUP_OUTPUT_BYTES,
    ) else {
        diagnostics.push(json!({"source": source, "status": "command-failed"}));
        return false;
    };
    if output.timed_out {
        diagnostics.push(json!({"source": source, "status": "timeout"}));
        return false;
    }
    if output.truncated {
        diagnostics.push(json!({"source": source, "status": "output-too-large"}));
        return false;
    }
    if !output.status.is_some_and(|status| status.success()) {
        diagnostics.push(json!({
            "source": source,
            "status": "command-exited",
            "code": output.status.and_then(|status| status.code()),
        }));
        return false;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if collect_opencode_models_from_cli_output(&stdout, source, entries) == 0 {
        diagnostics.push(json!({"source": source, "status": "empty"}));
        return false;
    }
    true
}

pub(super) fn collect_opencode_models_from_cli_output(
    raw: &str,
    source: &str,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
) -> usize {
    let before = entries.len();
    let trimmed = raw.trim();
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && let Ok(value) = serde_json::from_str::<Value>(trimmed)
    {
        collect_opencode_provider_catalog(&value, source, entries);
        collect_opencode_qualified_string_values(&value, source, entries);
        if entries.len() == before {
            collect_model_catalog_entries_from_collection_value(&value, source, entries);
        }
        return entries.len().saturating_sub(before);
    }
    for line in raw.lines() {
        let trimmed = line
            .trim()
            .trim_start_matches(|ch| matches!(ch, '-' | '*') || ch == '\u{2022}')
            .trim();
        if trimmed.is_empty()
            || trimmed.starts_with("Usage")
            || trimmed.starts_with("Available")
            || trimmed.starts_with("Tip:")
        {
            continue;
        }
        let (selector, display_name) = trimmed
            .split_once(" - ")
            .map(|(id, label)| (id.trim(), Some(label.trim())))
            .unwrap_or((trimmed, None));
        let selector = selector.split_whitespace().next().unwrap_or(selector);
        let Some((provider_id, model_id)) = selector.split_once('/') else {
            continue;
        };
        let display = display_name.map(|label| json!({"displayName": label}));
        add_opencode_model(provider_id, model_id, display.as_ref(), source, entries);
    }
    entries.len().saturating_sub(before)
}

fn collect_opencode_qualified_string_values(
    value: &Value,
    source: &str,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_opencode_qualified_string_values(item, source, entries);
            }
        }
        Value::String(raw) => {
            let Some((provider_id, model_id)) = raw.split_once('/') else {
                return;
            };
            add_opencode_model(provider_id, model_id, None, source, entries);
        }
        _ => {}
    }
}
