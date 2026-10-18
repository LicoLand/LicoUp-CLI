//! Shared bounded parameter, filesystem, hashing, path, and time helpers.

use super::*;

pub(super) fn text_param(params: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| params.get(*key).and_then(Value::as_str))
        .map(|value| value.trim().to_string())
}

pub(super) fn usize_param(params: &Value, keys: &[&str]) -> Option<usize> {
    keys.iter().find_map(|key| {
        params.get(*key).and_then(|value| match value {
            Value::Number(number) => number
                .as_u64()
                .and_then(|value| usize::try_from(value).ok()),
            Value::String(text) => text.trim().parse::<usize>().ok(),
            _ => None,
        })
    })
}

pub(super) fn string_list_value(value: &Value, key: &str) -> Vec<String> {
    match value.get(key) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .flat_map(split_path_list)
            .collect(),
        Some(Value::String(value)) => split_path_list(value),
        _ => Vec::new(),
    }
}

pub(super) fn merge_params(base: &Value, overlay: Value) -> Value {
    let mut object = base.as_object().cloned().unwrap_or_default();
    if let Some(overlay) = overlay.as_object() {
        for (key, value) in overlay {
            object.insert(key.clone(), value.clone());
        }
    }
    Value::Object(object)
}

pub(super) fn history_roots_from_value(value: Option<&Value>) -> Vec<PathBuf> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .flat_map(split_path_list)
            .map(|path| expand_home(&path))
            .collect(),
        Some(Value::String(value)) => split_path_list(value)
            .into_iter()
            .map(|path| expand_home(&path))
            .collect(),
        _ => Vec::new(),
    }
}

pub(super) fn split_path_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

pub(super) fn normalize_agent_alias(agent: &str) -> String {
    match agent.trim().to_ascii_lowercase().as_str() {
        "claude" | "claude-code" => "claude-code",
        "vscode" | "vs-code" => "code",
        "copilot" | "github-copilot" => "copilot",
        "hermes" | "hermes-agent" => "hermes",
        "kilo" | "kilo-code" => "kilo-code",
        "kimi-code" | "kimicode" => "kimi-code",
        "pi" | "pi-agent" | "pi-coding-agent" => "pi",
        other => other,
    }
    .to_string()
}

pub(super) fn client_state_store(params: &Value) -> Result<ClientStateStore> {
    if let Some(root) = text_param(params, &["stateRoot", "clientStateRoot"]) {
        if !root.is_empty() {
            return ClientStateStore::new(expand_home(&root));
        }
    }
    if let Some(portable_dir) = text_param(params, &["portableDir"]) {
        if !portable_dir.is_empty() {
            return ClientStateStore::new(expand_home(&portable_dir).join("client-state"));
        }
    }
    Ok(ClientStateStore::new(
        portable_data_dir()?.join("client-state"),
    )?)
}

pub(super) fn read_json_or_default<F>(path: &Path, default_value: F) -> Result<Value>
where
    F: FnOnce() -> Value,
{
    if !path.exists() {
        return Ok(default_value());
    }
    let raw = fs::read_to_string(path)?;
    if raw.trim().is_empty() {
        return Ok(default_value());
    }
    Ok(serde_json::from_str(&raw)?)
}

pub(super) fn atomic_write_json(path: &Path, value: &Value) -> Result<()> {
    atomic_write_text(path, &format!("{}\n", serde_json::to_string_pretty(value)?))
}

pub(super) fn atomic_write_text(path: &Path, content: &str) -> Result<()> {
    atomic_write_private_text(path, content)
}

pub(super) fn copy_dir_all(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_dir_all(&entry.path(), &target)?;
        } else if file_type.is_symlink() {
            // Skip symlinks — never follow them to external paths.
            continue;
        } else {
            // Copy only regular file content without following symlinks.
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

pub(super) fn directory_is_empty(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(true);
    }
    Ok(fs::read_dir(path)?.next().is_none())
}

pub(super) fn equivalent_paths(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

pub(super) fn existing_created_at(path: &Path) -> Option<String> {
    read_json_or_default(path, || json!({}))
        .ok()
        .and_then(|value| text_value(&value, "createdAt"))
}

pub(super) fn expand_home(value: &str) -> PathBuf {
    expand_home_from(value, home_dir)
}

pub(super) fn expand_home_from<F>(value: &str, home: F) -> PathBuf
where
    F: Fn() -> PathBuf,
{
    if value == "~" {
        return home();
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return home().join(rest);
    }
    if let Some(rest) = value.strip_prefix("~\\") {
        return home().join(rest);
    }
    PathBuf::from(value)
}

pub(super) fn home_dir() -> PathBuf {
    home_dir_from_env(|name| std::env::var_os(name))
}

pub(super) fn home_dir_from_env<F>(var: F) -> PathBuf
where
    F: Fn(&str) -> Option<OsString>,
{
    licoup_foundation::platform::paths::env_home_from(var).unwrap_or_else(|| PathBuf::from("."))
}

pub(super) fn hash_parts(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

pub(super) fn hash_bytes(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    format!("{:x}", hasher.finalize())
}

pub(super) fn sanitize_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

pub(super) fn display_path(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

pub(super) fn timestamp_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}
