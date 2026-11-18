use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub(super) fn usage_roots(scan_params: &Value) -> Vec<PathBuf> {
    if let Some(root) = text_param(scan_params, &["root", "historyRoot"]) {
        return vec![expand_user_path(&root)];
    }
    let Some(home) = resolve_codex_home(scan_params) else {
        return Vec::new();
    };
    vec![home.join("sessions"), home.join("archived_sessions")]
}

pub(super) fn bool_param(params: &Value, key: &str) -> Option<bool> {
    params.get(key).and_then(|value| {
        value.as_bool().or_else(|| {
            value
                .as_str()
                .and_then(|text| match text.trim().to_ascii_lowercase().as_str() {
                    "1" | "true" | "yes" | "on" => Some(true),
                    "0" | "false" | "no" | "off" => Some(false),
                    _ => None,
                })
        })
    })
}

fn text_param(params: &Value, keys: &[&str]) -> Option<String> {
    crate::domain::conversation::parameters::text_param(params, keys)
        .filter(|value| !value.is_empty())
}

fn resolve_codex_home(params: &Value) -> Option<PathBuf> {
    if let Some(path) = text_param(params, &["codexHome"]) {
        return Some(PathBuf::from(path));
    }
    if let Ok(path) = std::env::var("CODEX_HOME") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    licoup_foundation::platform::paths::user_home_from_env().map(|home| {
        licoup_foundation::platform::paths::strip_macos_data_volume(&home).join(".codex")
    })
}

pub(super) fn expand_user_path(value: &str) -> PathBuf {
    let trimmed = value.trim();
    if trimmed == "~" {
        return default_home_dir();
    }
    if let Some(rest) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("~\\"))
    {
        return default_home_dir().join(rest);
    }
    PathBuf::from(trimmed)
}

fn default_home_dir() -> PathBuf {
    licoup_foundation::platform::paths::user_home_from_env()
        .map(|home| licoup_foundation::platform::paths::strip_macos_data_volume(&home))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub(super) fn roots_fingerprint(roots: &[PathBuf], timezone_key: &str) -> String {
    let mut values = roots
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    hash_text(&format!("tz={timezone_key};roots={}", values.join("\n")))
}

pub(super) fn source_key(root_key: &str, path: &Path) -> String {
    hash_text(&format!("{root_key}\n{}", path.to_string_lossy()))
}

fn hash_text(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}
