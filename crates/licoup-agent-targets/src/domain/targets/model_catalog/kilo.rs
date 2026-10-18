use super::*;
use licoup_foundation::platform::process_supervisor::run_bounded_command_output;
use std::time::Duration;

// `kilo models` resolves the complete configured-provider catalog, including
// the account-scoped Kilo gateway. Local editor state contains only recent or
// favorite selections and is retained solely as a failed-lookup fallback.
const DEFAULT_KILO_CLI_MODEL_LOOKUP_TIMEOUT_MS: u64 = 60_000;
const MIN_KILO_CLI_MODEL_LOOKUP_TIMEOUT_MS: u64 = 100;
const MAX_KILO_CLI_MODEL_LOOKUP_TIMEOUT_MS: u64 = 60_000;
const MAX_KILO_CLI_MODEL_LOOKUP_OUTPUT_BYTES: usize = 1024 * 1024;

pub(super) fn collect_kilo_code_model_catalog(
    params: &Value,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
    sources: &mut BTreeSet<String>,
    diagnostics: &mut Vec<Value>,
) -> bool {
    if let Some(home) = home_dir_for_model_catalog(params) {
        collect_kilo_local_model_catalog(params, &home, entries, sources, diagnostics);
    } else {
        diagnostics.push(json!({
            "source": "kilo-state",
            "status": "home-unavailable",
        }));
    }

    if collect_kilo_cli_model_catalog(params, entries, diagnostics) {
        sources.clear();
        sources.insert("kilo-cli".to_string());
        true
    } else {
        false
    }
}

fn collect_kilo_local_model_catalog(
    params: &Value,
    home: &Path,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
    sources: &mut BTreeSet<String>,
    diagnostics: &mut Vec<Value>,
) {
    let selected = agent_cli_model_lookup_enabled(params);

    let vscode_state_paths = {
        let explicit = param_paths(params, &["kiloVsCodeStateDbPath", "kiloVscodeStateDbPath"]);
        if explicit.is_empty() {
            kilo_vscode_state_db_paths(home, selected)
        } else {
            explicit
        }
    };
    let before_vscode = entries.len();
    for path in vscode_state_paths {
        collect_kilo_models_from_vscode_state_db(&path, entries, diagnostics);
    }
    if entries.len() > before_vscode {
        sources.insert("kilo-vscode-state".to_string());
    }

    let kilo_db_paths = {
        let explicit = param_paths(params, &["kiloDbPath", "kiloDatabasePath"]);
        if explicit.is_empty() {
            vec![
                home.join(".local")
                    .join("share")
                    .join("kilo")
                    .join("kilo.db"),
            ]
        } else {
            explicit
        }
    };
    let before_local = entries.len();
    for path in kilo_db_paths {
        collect_kilo_models_from_local_db(&path, entries, diagnostics);
    }
    if entries.len() > before_local {
        sources.insert("kilo-local-db".to_string());
    }
}

fn collect_kilo_cli_model_catalog(
    params: &Value,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
    diagnostics: &mut Vec<Value>,
) -> bool {
    let source = "kilo-cli:models";
    if !agent_cli_model_lookup_enabled(params)
        || param_bool(params, "disableKiloCliModelLookup").unwrap_or(false)
    {
        diagnostics.push(json!({"source": source, "status": "disabled"}));
        return false;
    }

    let program = param_string(params, "kiloCliPath")
        .or_else(|| param_string(params, "kiloPath"))
        .or_else(|| param_string(params, "kiloBin"))
        .map(PathBuf::from)
        .or_else(|| find_binary(&["kilo", "kilocode"]));
    let Some(program) = program else {
        diagnostics.push(json!({"source": source, "status": "binary-unavailable"}));
        return false;
    };
    if !crate::domain::targets::scan_paths::discovered_agent_may_execute(&program, true) {
        diagnostics.push(json!({"source": source, "status": "execution-denied"}));
        return false;
    }
    let timeout_ms = param_u64(params, "kiloCliModelLookupTimeoutMs")
        .or_else(|| param_u64(params, "agentCliModelLookupTimeoutMs"))
        .unwrap_or(DEFAULT_KILO_CLI_MODEL_LOOKUP_TIMEOUT_MS)
        .clamp(
            MIN_KILO_CLI_MODEL_LOOKUP_TIMEOUT_MS,
            MAX_KILO_CLI_MODEL_LOOKUP_TIMEOUT_MS,
        );
    let mut command = Command::new(program);
    command.arg("models");
    // Provider availability may depend on account and provider environment
    // variables, so the selected-agent lookup runs with the user shell
    // environment (ADR 0007).
    crate::platform::user_shell_environment::apply_to_command(&mut command);
    let Ok(output) = run_bounded_command_output(
        &mut command,
        Duration::from_millis(timeout_ms),
        MAX_KILO_CLI_MODEL_LOOKUP_OUTPUT_BYTES,
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
    let mut official_entries = BTreeMap::new();
    if collect_kilo_model_catalog_from_cli_output(&stdout, source, &mut official_entries) == 0 {
        diagnostics.push(json!({"source": source, "status": "empty"}));
        return false;
    }
    *entries = official_entries;
    true
}

pub(super) fn collect_kilo_model_catalog_from_cli_output(
    raw: &str,
    source: &str,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
) -> usize {
    let before = entries.len();
    for line in raw.lines() {
        let selector = line.trim();
        if selector.is_empty() || selector.chars().any(char::is_whitespace) {
            continue;
        }
        let Some((provider_id, model_id)) = selector.split_once('/') else {
            continue;
        };
        if provider_id.is_empty() || model_id.is_empty() {
            continue;
        }
        add_model_catalog_entry_with_provider(
            entries,
            selector,
            None,
            Some(provider_id),
            None,
            source,
            BTreeSet::new(),
        );
    }
    entries.len().saturating_sub(before)
}

pub(super) fn kilo_vscode_state_db_paths(home: &Path, selected: bool) -> Vec<PathBuf> {
    let roots = match std::env::consts::OS {
        "windows" => {
            let app_data = default_app_data_dir(home);
            vec![
                app_data.join("Code"),
                app_data.join("Code - Insiders"),
                app_data.join("Cursor"),
                app_data.join("VSCodium"),
            ]
        }
        "macos" => {
            let app_support = home.join("Library").join("Application Support");
            vec![
                app_support.join("Code"),
                app_support.join("Code - Insiders"),
                app_support.join("Cursor"),
                app_support.join("VSCodium"),
            ]
        }
        _ => vec![
            home.join(".config").join("Code"),
            home.join(".config").join("Code - Insiders"),
            home.join(".config").join("Cursor"),
            home.join(".config").join("VSCodium"),
        ],
    };
    roots
        .into_iter()
        .map(|root| root.join("User").join("globalStorage").join("state.vscdb"))
        .filter(|path| {
            crate::domain::targets::scan_paths::selected_agent_named_store_exists(
                path, home, selected,
            )
        })
        .collect()
}

pub(super) fn collect_kilo_models_from_vscode_state_db(
    path: &Path,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
    diagnostics: &mut Vec<Value>,
) {
    let Some(connection) = open_sqlite_readonly(path) else {
        if path.exists() {
            diagnostics.push(json!({
                "source": "kilo-vscode-state",
                "status": "not-readable",
            }));
        }
        return;
    };
    let mut statement = match connection.prepare("SELECT value FROM ItemTable WHERE key=?1") {
        Ok(statement) => statement,
        Err(_) => {
            diagnostics.push(json!({
                "source": "kilo-vscode-state",
                "status": "schema-mismatch",
            }));
            return;
        }
    };
    let value = statement
        .query_row(["kilocode.kilo-code"], |row| row.get::<_, String>(0))
        .ok();
    let Some(value) = value else {
        return;
    };
    let Ok(parsed) = serde_json::from_str::<Value>(&value) else {
        diagnostics.push(json!({
            "source": "kilo-vscode-state",
            "status": "not-parseable",
        }));
        return;
    };
    collect_kilo_models_from_state_value(&parsed, "kilo-vscode-state", entries);
}

pub(super) fn collect_kilo_models_from_state_value(
    value: &Value,
    source: &str,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
) {
    let Some(object) = value.as_object() else {
        return;
    };
    for key in ["recentModels", "favoriteModels"] {
        if let Some(items) = object.get(key).and_then(Value::as_array) {
            for item in items {
                collect_kilo_model_ref(item, source, entries);
            }
        }
    }
    if let Some(selections) = object.get("variantSelections").and_then(Value::as_object) {
        for (raw_key, variant) in selections {
            if kilo_is_session_identity(raw_key) {
                continue;
            }
            let Some((provider_id, model_id)) =
                kilo_provider_and_model_id_from_selection_key(raw_key)
            else {
                continue;
            };
            if kilo_is_session_identity(&model_id) {
                continue;
            }
            let efforts = variant
                .as_str()
                .and_then(sanitize_option_name)
                .into_iter()
                .collect::<BTreeSet<_>>();
            let selector = kilo_qualified_selector(&model_id, provider_id.as_deref());
            add_model_catalog_entry_with_provider(
                entries,
                &selector,
                None,
                provider_id.as_deref(),
                None,
                source,
                efforts,
            );
        }
    }
}

pub(super) fn collect_kilo_model_ref(
    value: &Value,
    source: &str,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
) {
    let Some(object) = value.as_object() else {
        return;
    };
    let Some(model_id) = object
        .get("modelID")
        .or_else(|| object.get("modelId"))
        .or_else(|| object.get("model"))
        .or_else(|| object.get("id"))
        .and_then(Value::as_str)
        .and_then(sanitize_model_name)
        .filter(|name| !kilo_is_session_identity(name))
    else {
        return;
    };
    let efforts = object
        .get("variant")
        .or_else(|| object.get("reasoningEffort"))
        .or_else(|| object.get("thinking"))
        .and_then(Value::as_str)
        .and_then(sanitize_option_name)
        .into_iter()
        .collect::<BTreeSet<_>>();
    let provider_id = object
        .get("providerID")
        .or_else(|| object.get("providerId"))
        .or_else(|| object.get("provider_id"))
        .and_then(Value::as_str)
        .and_then(sanitize_option_name);
    let provider_name = object
        .get("providerName")
        .or_else(|| object.get("providerLabel"))
        .or_else(|| object.get("provider"))
        .and_then(Value::as_str)
        .and_then(sanitize_option_name);
    let selector = kilo_qualified_selector(&model_id, provider_id.as_deref());
    add_model_catalog_entry_with_provider(
        entries,
        &selector,
        model_display_name_from_object(object, &model_id).as_deref(),
        provider_id.as_deref(),
        provider_name.as_deref(),
        source,
        efforts,
    );
}

fn kilo_qualified_selector(model_id: &str, provider_id: Option<&str>) -> String {
    let Some(provider_id) = provider_id.map(str::trim).filter(|value| !value.is_empty()) else {
        return model_id.to_string();
    };
    if model_id.starts_with(&format!("{provider_id}/")) {
        model_id.to_string()
    } else {
        format!("{provider_id}/{model_id}")
    }
}

pub(super) fn kilo_provider_and_model_id_from_selection_key(
    value: &str,
) -> Option<(Option<String>, String)> {
    let trimmed = value.trim();
    if trimmed.is_empty() || kilo_is_session_identity(trimmed) {
        return None;
    }
    let parts = trimmed.split('/').collect::<Vec<_>>();
    let (provider_id, model_id) = if parts.len() >= 4 && parts[0] == "agent" {
        (sanitize_option_name(parts[2]), parts[3..].join("/"))
    } else if parts.len() >= 2 && parts[0] == "kilo" {
        (sanitize_option_name(parts[0]), parts[1..].join("/"))
    } else {
        (None, trimmed.to_string())
    };
    sanitize_model_name(&model_id).map(|model_id| (provider_id, model_id))
}

pub(super) fn collect_kilo_models_from_local_db(
    path: &Path,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
    diagnostics: &mut Vec<Value>,
) {
    let Some(connection) = open_sqlite_readonly(path) else {
        if path.exists() {
            diagnostics.push(json!({
                "source": "kilo-local-db",
                "status": "not-readable",
            }));
        }
        return;
    };
    if sqlite_table_exists(&connection, "session_message") {
        collect_kilo_models_from_session_messages(&connection, entries, diagnostics);
    }
    if sqlite_table_exists(&connection, "session") {
        collect_kilo_models_from_session_rows(&connection, entries);
    }
}

pub(super) fn collect_kilo_models_from_session_messages(
    connection: &Connection,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
    diagnostics: &mut Vec<Value>,
) {
    let mut statement = match connection.prepare(
        "SELECT data FROM session_message WHERE type='model-switched' ORDER BY time_created DESC LIMIT 200",
    ) {
        Ok(statement) => statement,
        Err(_) => return,
    };
    let Ok(rows) = statement.query_map([], |row| row.get::<_, String>(0)) else {
        diagnostics.push(json!({
            "source": "kilo-local-db",
            "status": "query-failed",
        }));
        return;
    };
    for row in rows.flatten() {
        let Ok(value) = serde_json::from_str::<Value>(&row) else {
            continue;
        };
        if let Some(model) = value.get("model") {
            collect_kilo_model_ref(model, "kilo-local-db", entries);
        }
    }
}

pub(super) fn collect_kilo_models_from_session_rows(
    connection: &Connection,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
) {
    let mut statement = match connection.prepare(
        "SELECT model FROM session WHERE model IS NOT NULL AND trim(model) <> '' ORDER BY time_updated DESC LIMIT 200",
    ) {
        Ok(statement) => statement,
        Err(_) => return,
    };
    let Ok(rows) = statement.query_map([], |row| row.get::<_, String>(0)) else {
        return;
    };
    for row in rows.flatten() {
        if let Some(name) = sanitize_model_name(&row).filter(|name| !kilo_is_session_identity(name))
        {
            add_model_catalog_entry(entries, &name, "kilo-local-db", BTreeSet::new());
        }
    }
}

/// Kilo session store keys (`session/ses_…`) are conversation identities, not
/// callable model ids. Keep this filter inside the Kilo adapter so other
/// agents are not subject to a shared heuristic.
pub(super) fn kilo_is_session_identity(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("session/") || lower.contains("/session/") {
        return true;
    }
    let tail = lower.rsplit('/').next().unwrap_or(lower.as_str());
    tail.starts_with("ses_")
}

pub(super) fn remove_kilo_session_identities(entries: &mut BTreeMap<String, ModelCatalogEntry>) {
    entries.retain(|_, entry| !kilo_is_session_identity(&entry.name));
}

pub(super) fn open_sqlite_readonly(path: &Path) -> Option<Connection> {
    if !path.exists() {
        return None;
    }
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()
}

pub(super) fn sqlite_table_exists(connection: &Connection, table: &str) -> bool {
    connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1 LIMIT 1",
            [table],
            |_| Ok(()),
        )
        .is_ok()
}
