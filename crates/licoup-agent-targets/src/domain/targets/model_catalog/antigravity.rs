use super::*;
use licoup_foundation::platform::process_supervisor::run_bounded_command_output;
use std::time::Duration;

// The native account catalog is network-backed. Cold start, auth refresh, or a
// busy machine can take tens of seconds; on timeout the catalog collapses to
// the one-or-two models named in local settings.json. Product policy: every
// model-catalog scan waits up to one minute.
const DEFAULT_AGENT_CLI_MODEL_LOOKUP_TIMEOUT_MS: u64 = 60_000;
const MIN_AGENT_CLI_MODEL_LOOKUP_TIMEOUT_MS: u64 = 100;
const MAX_AGENT_CLI_MODEL_LOOKUP_TIMEOUT_MS: u64 = 60_000;
const MAX_AGENT_CLI_MODEL_LOOKUP_OUTPUT_BYTES: usize = 256 * 1024;

pub(super) fn remove_unsupported_antigravity_reasoning_efforts(
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
) {
    for entry in entries.values_mut() {
        entry.reasoning_efforts.clear();
    }
}

pub(super) fn collect_antigravity_cli_model_catalog(
    params: &Value,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
    diagnostics: &mut Vec<Value>,
    replace: bool,
) -> bool {
    let source = "antigravity-cli:models";
    if !agent_cli_model_lookup_enabled(params)
        || param_bool(params, "disableAntigravityCliModelLookup").unwrap_or(false)
    {
        diagnostics.push(json!({
            "source": source,
            "status": "disabled",
        }));
        return false;
    }

    let program = param_string(params, "antigravityCliPath")
        .or_else(|| param_string(params, "agyPath"))
        .or_else(|| param_string(params, "agyBin"))
        .map(PathBuf::from)
        .or_else(|| find_binary(&["agy", "antigravity"]));
    let Some(program) = program else {
        diagnostics.push(json!({
            "source": source,
            "status": "binary-unavailable",
        }));
        return false;
    };
    let timeout_ms = param_u64(params, "antigravityCliModelLookupTimeoutMs")
        .or_else(|| param_u64(params, "agentCliModelLookupTimeoutMs"))
        .unwrap_or(DEFAULT_AGENT_CLI_MODEL_LOOKUP_TIMEOUT_MS)
        .clamp(
            MIN_AGENT_CLI_MODEL_LOOKUP_TIMEOUT_MS,
            MAX_AGENT_CLI_MODEL_LOOKUP_TIMEOUT_MS,
        );
    if !crate::domain::targets::scan_paths::discovered_agent_may_execute(&program, true) {
        diagnostics.push(json!({
            "source": source,
            "status": "execution-denied",
        }));
        return false;
    }
    let mut command = Command::new(program);
    command.arg("models");
    // This lookup runs only after the user selects Antigravity. The child
    // observes the user shell environment (ADR 0007): `agy models` reads the
    // same account/proxy environment it has in the user's terminal.
    crate::platform::user_shell_environment::apply_to_command(&mut command);
    let output = run_bounded_command_output(
        &mut command,
        Duration::from_millis(timeout_ms),
        MAX_AGENT_CLI_MODEL_LOOKUP_OUTPUT_BYTES,
    );
    let Ok(output) = output else {
        diagnostics.push(json!({
            "source": source,
            "status": "command-failed",
        }));
        return false;
    };
    if output.timed_out {
        diagnostics.push(json!({
            "source": source,
            "status": "timeout",
        }));
        return false;
    }
    if output.truncated {
        diagnostics.push(json!({
            "source": source,
            "status": "output-too-large",
        }));
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
    let mut official_entries = BTreeMap::new();
    let added = collect_model_catalog_from_cli_lines(&stdout, source, &mut official_entries);
    if added == 0 {
        diagnostics.push(json!({
            "source": source,
            "status": "empty",
        }));
        return false;
    }
    if replace {
        *entries = official_entries;
    } else {
        for (key, entry) in official_entries {
            entries.entry(key).or_insert(entry);
        }
    }
    true
}

pub(super) fn collect_antigravity_available_models_param(
    params: &Value,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
    diagnostics: &mut Vec<Value>,
) {
    const MAX_AVAILABLE_MODELS_BYTES: usize = 1024 * 1024;
    let Some(raw) = param_string(params, "antigravityAvailableModelsJson") else {
        return;
    };
    let source = "antigravity-local:available-models";
    if raw.len() > MAX_AVAILABLE_MODELS_BYTES {
        diagnostics.push(json!({"source": source, "status": "too-large"}));
        return;
    }
    let Ok(document) = serde_json::from_str::<Value>(&raw) else {
        diagnostics.push(json!({"source": source, "status": "not-parseable"}));
        return;
    };
    let Some(models) = document.get("models") else {
        diagnostics.push(json!({"source": source, "status": "models-missing"}));
        return;
    };
    match models {
        Value::Object(models) => {
            for (model_id, model) in models {
                let Some(name) = model_display_name_from_value(model, model_id) else {
                    continue;
                };
                add_model_catalog_entry_with_provider(
                    entries,
                    &name,
                    Some(&name),
                    provider_id_from_model_value(model).as_deref(),
                    provider_name_from_model_value(model).as_deref(),
                    source,
                    reasoning_efforts_from_value(model),
                );
            }
        }
        Value::Array(_) => {
            collect_model_catalog_entries_from_collection_value(models, source, entries);
        }
        _ => diagnostics.push(json!({"source": source, "status": "models-invalid"})),
    }
}

pub(super) fn collect_antigravity_available_models_from_disk(
    params: &Value,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
    diagnostics: &mut Vec<Value>,
) -> bool {
    let before = entries.len();
    for path in antigravity_available_models_paths(params) {
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(parsed) = serde_json::from_str::<Value>(&raw) else {
            diagnostics.push(json!({
                "source": "antigravity-local:available-models",
                "status": "not-parseable",
            }));
            continue;
        };
        let document = if parsed.get("models").is_some() {
            parsed
        } else {
            json!({ "models": parsed })
        };
        collect_antigravity_available_models_param(
            &json!({
                "antigravityAvailableModelsJson": document.to_string(),
            }),
            entries,
            diagnostics,
        );
    }
    entries.len() > before
}

fn antigravity_available_models_paths(params: &Value) -> Vec<PathBuf> {
    let explicit = param_paths(
        params,
        &[
            "antigravityAvailableModelsPath",
            "antigravityAvailableModelsPaths",
        ],
    );
    if !explicit.is_empty() {
        return explicit.into_iter().filter(|path| path.is_file()).collect();
    }
    let Some(home) = home_dir_for_model_catalog(params) else {
        return Vec::new();
    };
    let gemini = home.join(".gemini");
    [
        gemini.join("antigravity").join("available-models.json"),
        gemini.join("antigravity-cli").join("available-models.json"),
        gemini.join("antigravity-ide").join("available-models.json"),
        gemini.join("antigravity").join("models.json"),
        gemini.join("antigravity-cli").join("models.json"),
        gemini.join("antigravity-ide").join("models.json"),
    ]
    .into_iter()
    .filter(|path| path.is_file())
    .collect()
}

pub(super) fn collect_model_catalog_from_cli_lines(
    raw: &str,
    source: &str,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
) -> usize {
    let before = entries.len();
    let trimmed = raw.trim();
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && let Ok(value) = serde_json::from_str::<Value>(trimmed)
    {
        collect_model_catalog_entries_from_collection_value(&value, source, entries);
        if entries.len() > before {
            return entries.len() - before;
        }
    }
    for line in raw.lines() {
        let trimmed = line
            .trim()
            .trim_start_matches(|ch| matches!(ch, '-' | '*') || ch == '\u{2022}')
            .trim();
        if trimmed.is_empty()
            || trimmed.starts_with("Usage")
            || trimmed.starts_with("Available")
            || is_antigravity_cli_header_line(trimmed)
        {
            continue;
        }
        let (selector, display_name) = split_cli_model_line(trimmed);
        add_model_catalog_entry_with_provider(
            entries,
            selector,
            display_name,
            None,
            None,
            source,
            BTreeSet::new(),
        );
    }
    entries.len().saturating_sub(before)
}

/// Splits one `agy models` line into the model id and its display name.
/// Lines arrive as `id - Name`, `id Name`, a bare id, or a bare name.
fn split_cli_model_line(line: &str) -> (&str, Option<&str>) {
    if let Some((id, name)) = line.split_once(" - ") {
        let name = name.trim();
        return (id.trim(), if name.is_empty() { None } else { Some(name) });
    }
    if let Some((head, tail)) = line.split_once(char::is_whitespace) {
        let tail = tail.trim();
        if is_cli_model_id_token(head) && !tail.is_empty() {
            return (head, Some(tail));
        }
    }
    (line, None)
}

/// Model ids are lowercase slug tokens (`gemini-3.7-flash-high`); a display
/// name's first word is capitalized (`Gemini`, `Claude`), so a capital letter
/// or a slug-free token means the line carries no id column.
fn is_cli_model_id_token(token: &str) -> bool {
    !token.is_empty()
        && token.chars().all(|ch| {
            ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '_' | '.' | '/')
        })
        && token.chars().any(|ch| ch == '-' || ch.is_ascii_digit())
}

fn is_antigravity_cli_header_line(trimmed: &str) -> bool {
    let lower = trimmed.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "models" | "model" | "model id" | "model id name"
    ) {
        return true;
    }
    let tokens = lower.split_whitespace().collect::<Vec<_>>();
    !tokens.is_empty()
        && tokens.iter().all(|token| {
            matches!(
                *token,
                "model" | "models" | "id" | "ids" | "name" | "names" | "display" | "provider"
            )
        })
}
