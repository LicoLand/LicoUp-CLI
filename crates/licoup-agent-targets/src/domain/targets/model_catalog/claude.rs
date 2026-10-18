use super::*;

const CLAUDE_CODE_MODEL_SOURCE: &str = "claude-settings";

/// Claude Code has no non-interactive catalog command. Its documented aliases
/// remain selectable alongside configured models, subject to availableModels.
/// Never invoke an unsupported `claude models` command: that sends a prompt.
pub(super) fn claude_code_model_catalog(
    port: &AgentTargetPort,
    config_path: Option<&Path>,
    params: &Value,
) -> Value {
    let mut diagnostics = Vec::<Value>::new();
    let settings = claude_code_settings_path(config_path, params)
        .and_then(|path| read_claude_code_settings(&path, &mut diagnostics))
        .unwrap_or_else(|| json!({}));
    let configured_model = claude_code_current_model_from_settings(&settings);
    let allowed = settings.get("availableModels").and_then(Value::as_array);
    let mut entries = BTreeMap::<String, ModelCatalogEntry>::new();
    let mut sources = BTreeSet::from([CLAUDE_CODE_MODEL_SOURCE.to_string()]);
    let mut selectors = allowed
        .map(|models| models.iter().map(model_name_from_value).collect::<Vec<_>>())
        .unwrap_or_else(|| {
            [
                "opus",
                "opus[1m]",
                "sonnet",
                "sonnet[1m]",
                "haiku",
                "opusplan",
            ]
            .map(str::to_string)
            .to_vec()
        });
    if allowed.is_none() {
        if let Some(model) = configured_model.as_ref() {
            selectors.push(model.clone());
        }
        // Fable's native picker is account-gated. A local explicit binding is
        // usable evidence; the global built-in table is not an entitlement.
        if settings
            .pointer("/env/ANTHROPIC_DEFAULT_FABLE_MODEL")
            .and_then(Value::as_str)
            .is_some()
        {
            selectors.push("fable".to_string());
        }
        if let Some(custom) = settings
            .pointer("/env/ANTHROPIC_CUSTOM_MODEL_OPTION")
            .and_then(Value::as_str)
        {
            selectors.push(custom.to_string());
        }
    }
    // Default is always admitted by Claude Code, even for an empty allowlist.
    selectors.push("default".to_string());
    for selector in selectors {
        let alias = selector.split('[').next().unwrap_or(&selector);
        let env_key = match alias {
            "fable" => Some("ANTHROPIC_DEFAULT_FABLE_MODEL"),
            "opus" => Some("ANTHROPIC_DEFAULT_OPUS_MODEL"),
            "sonnet" => Some("ANTHROPIC_DEFAULT_SONNET_MODEL"),
            "haiku" => Some("ANTHROPIC_DEFAULT_HAIKU_MODEL"),
            _ => None,
        };
        let pinned = env_key
            .and_then(|key| settings.get("env").and_then(|env| env.get(key)))
            .and_then(Value::as_str);
        let display = match selector.as_str() {
            "default" => "Default".to_string(),
            "opusplan" => "Claude Opus Plan".to_string(),
            "fable" | "fable[1m]" | "opus" | "sonnet" | "haiku" | "opus[1m]" | "sonnet[1m]" => {
                let base = pinned
                    .map(canonical_model_display_name)
                    .unwrap_or_else(|| format!("Claude {}", canonical_model_display_name(alias)));
                if selector.ends_with("[1m]") && !base.ends_with("[1m]") {
                    format!("{base} (1M)")
                } else {
                    base
                }
            }
            _ => canonical_model_display_name(&selector),
        };
        let provider = presentation::inferred_model_provider(pinned.unwrap_or(&selector));
        let provider = provider
            .as_deref()
            .or((selector == "default").then_some("anthropic"));
        let efforts = pinned
            .map(|model| builtin::builtin_reasoning_efforts("claude-code", model))
            .unwrap_or_default();
        add_model_catalog_entry_with_provider(
            &mut entries,
            &selector,
            Some(&display),
            provider,
            None,
            CLAUDE_CODE_MODEL_SOURCE,
            efforts.into_iter().collect(),
        );
        for entry in entries.values_mut().filter(|entry| entry.name == selector) {
            entry.provider_inferred = provider.is_some();
        }
    }
    if let Some(fixture) = model_catalog_fixture_for_target("claude-code", params) {
        merge_model_catalog_value_into(
            &fixture,
            "fixture",
            &mut entries,
            &mut sources,
            &mut diagnostics,
        );
        if let Some(allowed) = allowed {
            let allowed = allowed
                .iter()
                .map(model_name_from_value)
                .collect::<BTreeSet<_>>();
            entries.retain(|_, entry| entry.name == "default" || allowed.contains(&entry.name));
        }
    }
    apply_builtin_model_catalog_overlay("claude-code", &mut entries, &mut sources);
    let default = configured_model
        .filter(|model| entries.values().any(|entry| &entry.name == model))
        .unwrap_or_else(|| "default".to_string());
    build_model_catalog(
        port,
        "claude-code",
        entries,
        sources,
        diagnostics,
        Some(default),
    )
}

fn claude_code_settings_path(config_path: Option<&Path>, params: &Value) -> Option<PathBuf> {
    config_path.map(Path::to_path_buf).or_else(|| {
        home_dir_for_model_catalog(params).map(|home| home.join(".claude").join("settings.json"))
    })
}

fn read_claude_code_settings(path: &Path, diagnostics: &mut Vec<Value>) -> Option<Value> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(_) => {
            diagnostics.push(json!({"source": CLAUDE_CODE_MODEL_SOURCE, "status": "not-readable"}));
            return None;
        }
    };
    let settings = parse_model_config_document(path, &raw);
    if settings.is_none() {
        diagnostics.push(json!({"source": CLAUDE_CODE_MODEL_SOURCE, "status": "not-parseable"}));
    }
    settings
}

fn claude_code_current_model_from_settings(settings: &Value) -> Option<String> {
    let object = settings.as_object()?;
    object
        .get("env")
        .and_then(Value::as_object)
        .and_then(|env| env.get("ANTHROPIC_MODEL"))
        .map(model_name_from_value)
        .filter(|model| !model.is_empty())
        .or_else(|| {
            object
                .get("model")
                .map(model_name_from_value)
                .filter(|model| !model.is_empty())
        })
}
