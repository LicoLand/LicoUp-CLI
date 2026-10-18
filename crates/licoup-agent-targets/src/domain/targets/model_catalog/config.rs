use super::*;

pub(super) fn extra_model_config_paths(target: &str, params: &Value) -> Vec<PathBuf> {
    let Some(home) = home_dir_for_model_catalog(params) else {
        return Vec::new();
    };
    let paths = match target {
        "antigravity" => vec![
            home.join(".gemini").join("settings.json"),
            home.join(".gemini")
                .join("antigravity-ide")
                .join("settings.json"),
            home.join(".gemini")
                .join("antigravity-cli")
                .join("settings.json"),
        ],
        "opencode" => vec![
            home.join(".config").join("opencode").join("opencode.jsonc"),
            home.join(".config").join("opencode").join("opencode.json"),
        ],
        _ => Vec::new(),
    };
    paths
        .into_iter()
        .filter(|path| crate::domain::targets::scan_paths::probe_exists_under_home(path, &home))
        .collect()
}

pub(super) fn extra_model_collection_paths(target: &str, params: &Value) -> Vec<PathBuf> {
    let Some(home) = home_dir_for_model_catalog(params) else {
        return Vec::new();
    };
    let mut paths = Vec::<PathBuf>::new();
    if target == "codex" {
        // Codex Desktop/CLI persist the full picker directory here; App Server
        // model/list alone is a shorter live projection and must not be the
        // only source when this cache is present.
        let models_cache = home.join(".codex").join("models_cache.json");
        if models_cache.is_file() {
            paths.push(models_cache);
        }
        collect_json_model_catalog_files(&home.join(".codex").join("model-catalogs"), &mut paths);
    }
    if target == "copilot" {
        let root = home
            .join("Library")
            .join("Application Support")
            .join("Code")
            .join("User")
            .join("workspaceStorage");
        let selected = agent_cli_model_lookup_enabled(params);
        if crate::domain::targets::scan_paths::selected_agent_named_store_exists(
            &root, &home, selected,
        ) {
            collect_named_model_cache_files(&root, "GitHub.copilot-chat", &mut paths, 0);
        }
    }
    paths.sort_by(|left, right| {
        file_modified_at(right)
            .cmp(&file_modified_at(left))
            .then_with(|| left.cmp(right))
    });
    paths.truncate(8);
    paths
}

pub(super) fn collect_json_model_catalog_files(root: &Path, paths: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) == Some("json") {
            paths.push(path);
        }
    }
}

pub(super) fn home_dir_for_model_catalog(params: &Value) -> Option<PathBuf> {
    param_string(params, "homeDir")
        .map(PathBuf::from)
        .or_else(licoup_foundation::platform::paths::user_home_from_env)
}

pub(super) fn collect_named_model_cache_files(
    root: &Path,
    required_component: &str,
    paths: &mut Vec<PathBuf>,
    depth: usize,
) {
    if depth > 5 || paths.len() >= 32 {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_named_model_cache_files(&path, required_component, paths, depth + 1);
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        if path.file_name().and_then(|value| value.to_str()) != Some("models.json") {
            continue;
        }
        if path
            .components()
            .any(|component| component.as_os_str() == required_component)
        {
            paths.push(path);
        }
    }
}

pub(super) fn file_modified_at(path: &Path) -> SystemTime {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(UNIX_EPOCH)
}

pub(super) fn collect_model_catalog_from_config_path(
    path: &Path,
    source: &str,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
    global_efforts: &mut BTreeSet<String>,
    diagnostics: &mut Vec<Value>,
) -> Option<String> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(_) => {
            diagnostics.push(json!({
                "source": source,
                "status": "not-readable",
            }));
            return None;
        }
    };
    let parsed = parse_model_config_document(path, &raw);
    let Some(value) = parsed else {
        diagnostics.push(json!({
            "source": source,
            "status": "not-parseable",
        }));
        return None;
    };
    let default_model = default_model_name_from_config_document(&value);
    collect_model_catalog_from_value(&value, source, entries, global_efforts);
    default_model
}

pub(super) fn collect_model_catalog_from_model_collection_path(
    path: &Path,
    source: &str,
    entries: &mut BTreeMap<String, ModelCatalogEntry>,
    diagnostics: &mut Vec<Value>,
) {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(_) => {
            diagnostics.push(json!({
                "source": source,
                "status": "not-readable",
            }));
            return;
        }
    };
    let Some(value) = parse_model_config_document(path, &raw) else {
        diagnostics.push(json!({
            "source": source,
            "status": "not-parseable",
        }));
        return;
    };
    collect_model_catalog_entries_from_collection_value(&value, source, entries);
}

pub(super) fn parse_model_config_document(path: &Path, raw: &str) -> Option<Value> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if extension == "toml" {
        return raw
            .parse::<toml::Value>()
            .ok()
            .and_then(|value| serde_json::to_value(value).ok());
    }
    serde_json::from_str::<Value>(&strip_json_comments(raw)).ok()
}

pub(super) fn strip_json_comments(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if in_string {
            escaped = ch == '\\' && !escaped;
            if ch == '"' && !escaped {
                in_string = false;
            }
            output.push(ch);
            if ch != '\\' {
                escaped = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            output.push(ch);
            continue;
        }
        if ch == '/' {
            match chars.peek() {
                Some('/') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if next == '\n' {
                            output.push('\n');
                            break;
                        }
                    }
                    continue;
                }
                Some('*') => {
                    chars.next();
                    let mut previous = '\0';
                    for next in chars.by_ref() {
                        if previous == '*' && next == '/' {
                            break;
                        }
                        previous = next;
                    }
                    continue;
                }
                _ => {}
            }
        }
        output.push(ch);
    }
    output
}
