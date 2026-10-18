use super::*;
pub(super) use licoup_foundation::core::model_naming::model_display_name as canonical_model_display_name;

pub(super) fn collect_model_catalog_from_value(
    value: &Value,
    source: &str,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
    global_efforts: &mut BTreeSet<String>,
) {
    collect_model_catalog_from_value_inner(value, source, entries, global_efforts, 0);
}

pub(super) fn collect_model_catalog_from_value_inner(
    value: &Value,
    source: &str,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
    global_efforts: &mut BTreeSet<String>,
    depth: usize,
) {
    if depth > 8 {
        return;
    }
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let normalized = normalize_model_catalog_key(key);
                if is_reasoning_option_key(&normalized) {
                    global_efforts.extend(option_names_from_value(child));
                }
                if is_model_scalar_key(&normalized) {
                    for name in model_names_from_value(child, false) {
                        add_model_catalog_entry(
                            entries,
                            &name,
                            source,
                            reasoning_efforts_from_value(child),
                        );
                    }
                } else if is_model_collection_key(&normalized) {
                    for name in model_names_from_value(child, true) {
                        add_model_catalog_entry(
                            entries,
                            &name,
                            source,
                            reasoning_efforts_from_value(child),
                        );
                    }
                }
                collect_model_catalog_from_value_inner(
                    child,
                    source,
                    entries,
                    global_efforts,
                    depth + 1,
                );
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_model_catalog_from_value_inner(
                    item,
                    source,
                    entries,
                    global_efforts,
                    depth + 1,
                );
            }
        }
        _ => {}
    }
}

pub(super) fn normalize_model_catalog_key(key: &str) -> String {
    key.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

/// Extracts the configured default model from a parsed agent config document.
/// Only top-level scalar keys that mean "the model in use" count, in priority
/// order. Native settings may also expose an explicit default/current model in
/// nested settings or environment objects; profile and workspace branches are
/// excluded so an optional profile never becomes the agent-wide default.
pub(super) fn default_model_name_from_config_document(document: &Value) -> Option<String> {
    let object = document.as_object()?;
    if let Some(name) = model_name_for_normalized_keys(
        object,
        &[
            "model",
            "defaultmodel",
            "currentmodel",
            "selectedmodel",
            "activemodel",
        ],
    ) {
        return Some(name);
    }
    explicit_nested_default_model(document, 0).or_else(|| flagged_default_model(document, 0, false))
}

fn model_name_for_normalized_keys(
    object: &Map<String, Value>,
    wanted_keys: &[&str],
) -> Option<String> {
    for wanted in wanted_keys {
        for (key, child) in object {
            if normalize_model_catalog_key(key) != *wanted {
                continue;
            }
            let name = model_name_from_value(child);
            if !name.trim().is_empty() {
                return Some(name);
            }
        }
    }
    None
}

fn explicit_nested_default_model(value: &Value, depth: usize) -> Option<String> {
    if depth > 8 {
        return None;
    }
    match value {
        Value::Object(object) => {
            if depth > 0 {
                if let Some(name) = model_name_for_normalized_keys(
                    object,
                    &[
                        "defaultmodel",
                        "currentmodel",
                        "selectedmodel",
                        "activemodel",
                        "anthropicmodel",
                        "anthropicdefaultmodel",
                        "claudecodemodel",
                        "openaimodel",
                        "googlemodel",
                        "geminimodel",
                        "kimimodel",
                    ],
                ) {
                    return Some(name);
                }
            }
            for (key, child) in object {
                if nested_default_branch_is_scoped(&normalize_model_catalog_key(key)) {
                    continue;
                }
                if let Some(name) = explicit_nested_default_model(child, depth + 1) {
                    return Some(name);
                }
            }
            None
        }
        Value::Array(items) => items
            .iter()
            .find_map(|item| explicit_nested_default_model(item, depth + 1)),
        _ => None,
    }
}

fn flagged_default_model(value: &Value, depth: usize, model_context: bool) -> Option<String> {
    if depth > 8 {
        return None;
    }
    match value {
        Value::Object(object) => {
            let flagged = [
                "default",
                "isdefault",
                "current",
                "iscurrent",
                "selected",
                "isselected",
                "active",
                "isactive",
            ]
            .iter()
            .any(|wanted| {
                object.iter().any(|(key, child)| {
                    normalize_model_catalog_key(key) == *wanted && child.as_bool() == Some(true)
                })
            });
            if flagged && model_context {
                if let Some(name) = model_name_from_object(object) {
                    return Some(name);
                }
            }
            for (key, child) in object {
                let normalized_key = normalize_model_catalog_key(key);
                if nested_default_branch_is_scoped(&normalized_key) {
                    continue;
                }
                if let Some(name) = flagged_default_model(
                    child,
                    depth + 1,
                    model_context || is_model_collection_key(&normalized_key),
                ) {
                    return Some(name);
                }
            }
            None
        }
        Value::Array(items) => items
            .iter()
            .find_map(|item| flagged_default_model(item, depth + 1, model_context)),
        _ => None,
    }
}

fn nested_default_branch_is_scoped(key: &str) -> bool {
    matches!(
        key,
        "profile"
            | "profiles"
            | "workspace"
            | "workspaces"
            | "project"
            | "projects"
            | "subagent"
            | "subagents"
    )
}

pub(super) fn is_model_scalar_key(key: &str) -> bool {
    matches!(
        key,
        "model"
            | "modelid"
            | "modelname"
            | "modellabel"
            | "defaultmodel"
            | "currentmodel"
            | "selectedmodel"
            | "activemodel"
            | "anthropicmodel"
            | "anthropicdefaultmodel"
            | "anthropicdefaulthaikumodel"
            | "anthropicdefaultopusmodel"
            | "anthropicdefaultsonnetmodel"
            | "claudecodemodel"
            | "claudecodesubagentmodel"
    )
}

pub(super) fn is_model_collection_key(key: &str) -> bool {
    matches!(
        key,
        "models"
            | "supportedmodels"
            | "availablemodels"
            | "modeloptions"
            | "modelprofiles"
            | "modellist"
            | "modelcatalog"
    )
}

pub(super) fn is_reasoning_option_key(key: &str) -> bool {
    matches!(
        key,
        "reasoningeffort"
            | "reasoningefforts"
            | "reasoninglevel"
            | "reasoninglevels"
            | "reasoningleveloptions"
            | "supportedreasoningefforts"
            | "supportedreasoninglevels"
            | "defaultreasoninglevel"
            | "reasoningeffortoptions"
            | "thinkinglevel"
            | "thinkinglevels"
            | "thinkingleveloptions"
            | "thinkingtype"
            | "thinkingtypes"
            | "thinkingtypeoptions"
            | "thinkingoptions"
            | "effort"
            | "efforts"
            | "effortlevel"
            | "effortlevels"
            | "effortoptions"
            | "modelreasoningeffort"
            | "claudecodeeffortlevel"
    )
}

pub(super) fn model_names_from_value(value: &Value, include_object_keys: bool) -> Vec<String> {
    match value {
        Value::String(value) => {
            let trimmed = value.trim();
            if trimmed.starts_with('{') || trimmed.starts_with('[') {
                if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
                    return model_names_from_value(&parsed, include_object_keys);
                }
            }
            sanitize_model_name(value).into_iter().collect()
        }
        Value::Array(items) => items
            .iter()
            .flat_map(|item| model_names_from_value(item, include_object_keys))
            .collect(),
        Value::Object(object) => {
            if let Some(name) = model_name_from_object(object) {
                return vec![name];
            }
            let mut names = Vec::<String>::new();
            if include_object_keys {
                for (key, child) in object {
                    if looks_like_model_name(key) {
                        names.push(key.trim().to_string());
                    }
                    names.extend(model_names_from_value(child, true));
                }
            }
            names
        }
        _ => Vec::new(),
    }
}

pub(super) fn model_name_from_value(value: &Value) -> String {
    match value {
        Value::String(value) => sanitize_model_name(value).unwrap_or_default(),
        Value::Object(object) => model_name_from_object(object).unwrap_or_default(),
        _ => String::new(),
    }
}

pub(super) fn model_name_from_object(object: &Map<String, Value>) -> Option<String> {
    model_identifier_from_object(object).or_else(|| raw_model_display_name_from_object(object))
}

pub(super) fn model_identifier_from_object(object: &Map<String, Value>) -> Option<String> {
    for key in [
        "slug",
        "model",
        "modelName",
        "model_name",
        "name",
        "id",
        "modelId",
        "model_id",
    ] {
        let name = object
            .get(key)
            .and_then(Value::as_str)
            .and_then(sanitize_model_name);
        if name.is_some() {
            return name;
        }
    }
    None
}

pub(super) fn model_display_name_from_value(value: &Value, fallback: &str) -> Option<String> {
    match value {
        Value::String(value) => {
            sanitize_model_name(value).map(|name| canonical_model_display_name(&name))
        }
        Value::Object(object) => model_display_name_from_object(object, fallback),
        _ => sanitize_model_name(fallback).map(|name| canonical_model_display_name(&name)),
    }
}

pub(super) fn model_display_name_from_object(
    object: &Map<String, Value>,
    fallback: &str,
) -> Option<String> {
    raw_model_display_name_from_object(object)
        .or_else(|| sanitize_model_name(fallback))
        .map(|name| canonical_model_display_name(&name))
}

fn raw_model_display_name_from_object(object: &Map<String, Value>) -> Option<String> {
    for key in [
        "displayName",
        "display_name",
        "label",
        "title",
        "modelLabel",
        "model_label",
        "name",
    ] {
        let name = object
            .get(key)
            .and_then(Value::as_str)
            .and_then(sanitize_model_name);
        if let Some(name) = name {
            return Some(name);
        }
    }
    None
}

pub(super) fn collect_model_catalog_entries_from_collection_value(
    value: &Value,
    source: &str,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_model_catalog_entries_from_collection_value(item, source, entries);
            }
        }
        Value::Object(object) => {
            if !model_catalog_object_is_selectable(object) {
                return;
            }
            // Codex `models_cache.json` (and similar) wrap the directory in a
            // `models` array beside metadata scalars such as `fetched_at` /
            // `etag` / `client_version`. Prefer that collection and do not
            // treat the metadata strings as model ids.
            if let Some((_, models)) = object.iter().find(|(key, child)| {
                is_model_collection_key(&key.to_ascii_lowercase())
                    && (child.is_array() || child.is_object())
            }) {
                collect_model_catalog_entries_from_collection_value(models, source, entries);
                return;
            }
            if let Some(name) = model_name_from_object(object) {
                let display_name = model_display_name_from_object(object, &name);
                add_model_catalog_entry_with_provider(
                    entries,
                    &name,
                    display_name.as_deref(),
                    provider_id_from_model_object(object).as_deref(),
                    provider_name_from_model_object(object).as_deref(),
                    source,
                    reasoning_efforts_from_value(value),
                );
                return;
            }
            for (key, child) in object {
                if looks_like_model_name(key) {
                    add_model_catalog_entry(
                        entries,
                        key,
                        source,
                        reasoning_efforts_from_value(child),
                    );
                }
                collect_model_catalog_entries_from_collection_value(child, source, entries);
            }
        }
        _ => {
            for name in model_names_from_value(value, true) {
                add_model_catalog_entry(entries, &name, source, BTreeSet::new());
            }
        }
    }
}

pub(super) fn model_catalog_object_is_selectable(object: &Map<String, Value>) -> bool {
    if object
        .get("enabled")
        .or_else(|| object.get("isEnabled"))
        .or_else(|| object.get("selectable"))
        .and_then(Value::as_bool)
        == Some(false)
    {
        return false;
    }
    let visibility = object
        .get("visibility")
        .or_else(|| object.get("display"))
        .and_then(Value::as_str)
        .map(|value| value.trim().to_ascii_lowercase());
    !matches!(visibility.as_deref(), Some("hide" | "hidden" | "disabled"))
}

pub(super) fn sanitize_model_name(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.len() > 160
        || trimmed.contains('\n')
        || trimmed.contains('\r')
        || trimmed.starts_with('{')
        || trimmed.starts_with('[')
        || trimmed.starts_with('$')
        || trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("W/\"")
        || trimmed.to_ascii_lowercase().contains("api_key")
    {
        return None;
    }
    // Reject cache metadata that previously leaked in as model ids when a
    // models_cache document was walked as a generic object tree.
    if trimmed.contains('T')
        && trimmed.contains(':')
        && trimmed
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | ':' | '.' | 'T' | 'Z' | '+'))
    {
        return None;
    }
    if trimmed.chars().all(|ch| ch.is_ascii_digit() || ch == '.') && trimmed.contains('.') {
        return None;
    }
    Some(trimmed.to_string())
}

pub(super) fn prefer_model_display_name(name: &str, current: &str, candidate: &str) -> bool {
    if candidate.trim().is_empty() || current == candidate {
        return false;
    }
    current.trim().is_empty()
        || current == name
        || current == canonical_model_display_name(name)
        || (candidate.chars().any(|ch| ch.is_ascii_uppercase())
            && !current.chars().any(|ch| ch.is_ascii_uppercase()))
}

pub(super) fn sanitize_option_name(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.len() > 80
        || trimmed.contains('\n')
        || trimmed.contains('\r')
        || trimmed.starts_with('$')
    {
        return None;
    }
    Some(trimmed.to_string())
}

pub(super) fn looks_like_model_name(value: &str) -> bool {
    let trimmed = value.trim();
    if sanitize_model_name(trimmed).is_none() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "default" | "provider" | "providers" | "metadata" | "settings" | "options"
    ) {
        return false;
    }
    lower.contains("gpt")
        || lower.contains("claude")
        || lower.contains("gemini")
        || lower.contains("deepseek")
        || lower.contains("kimi")
        || lower.contains("llama")
        || lower.contains("qwen")
        || lower.contains("mistral")
        || lower.contains("sonnet")
        || lower.contains("opus")
        || lower.contains("haiku")
        || lower.contains("flash")
        || lower.contains("pro")
        || lower.contains("oss")
        || lower.contains('-')
        || lower.chars().any(|ch| ch.is_ascii_digit())
}
