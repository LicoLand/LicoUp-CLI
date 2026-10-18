//! Request aliases, local-filesystem boundary, and retry parameter normalization.

use anyhow::{Result, anyhow, ensure};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use super::clock::timestamp;
use crate::domain::conversation::archive_queue::{DEFAULT_MAX_ATTEMPTS, RetryPolicy};

pub(super) fn normalize_request(params: &Value) -> Result<Value> {
    let selection_mode = text_param(params, &["selectionMode", "selection"])
        .unwrap_or_else(|| "exact-keyword".to_string())
        .to_ascii_lowercase();
    ensure!(
        matches!(selection_mode.as_str(), "all" | "exact-keyword"),
        "archive selectionMode must be all or exact-keyword"
    );
    let query = if selection_mode == "all" {
        String::new()
    } else {
        text_param(params, &["query"]).unwrap_or_default()
    };
    ensure!(
        selection_mode == "all" || !query.is_empty(),
        "exact-keyword archive requires --query"
    );
    let path = text_param(
        params,
        &[
            "path",
            "archiveRoot",
            "destination",
            "destinationPath",
            "outputDir",
            "snapshotRoot",
        ],
    )
    .filter(|value| !value.trim().is_empty())
    .ok_or_else(|| anyhow!("archive jobs create requires --path"))?;
    let path = local_path_from_user_input(&path, "archive destination")?;
    let mut request = Map::new();
    for key in [
        "agent",
        "agentId",
        "target",
        "agents",
        "archiveParallelism",
        "parallelism",
        "maxAttempts",
        "retryBackoffSeconds",
    ] {
        if let Some(value) = params.get(key).filter(|value| !value.is_null()) {
            request.insert(key.to_string(), value.clone());
        }
    }
    for (key, label) in [
        ("stateRoot", "state root"),
        ("clientStateRoot", "client state root"),
        ("portableDir", "portable directory"),
        ("homeDir", "home directory"),
    ] {
        if let Some(path) = optional_local_path_param(params, &[key], label)? {
            request.insert(key.to_string(), json!(display_path(&path)));
        }
    }
    request.insert("selectionMode".to_string(), json!(selection_mode));
    request.insert("query".to_string(), json!(query));
    request.insert("path".to_string(), json!(display_path(&path)));
    request
        .entry("maxAttempts".to_string())
        .or_insert_with(|| json!(DEFAULT_MAX_ATTEMPTS));
    Ok(Value::Object(request))
}

pub(super) fn required_job_id(params: &Value) -> Result<String> {
    text_param(params, &["jobId", "id"])
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("conversation archive jobs command requires --job-id"))
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

pub(super) fn text_param(params: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        params
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

pub(super) fn optional_local_path_param(
    params: &Value,
    keys: &[&str],
    label: &str,
) -> Result<Option<PathBuf>> {
    for key in keys {
        let Some(value) = params.get(*key).filter(|value| !value.is_null()) else {
            continue;
        };
        let raw = value
            .as_str()
            .ok_or_else(|| anyhow!("{label} must be a string path"))?;
        return local_path_from_user_input(raw, label).map(Some);
    }
    Ok(None)
}

pub(super) fn number_param(params: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| {
        params.get(*key).and_then(|value| {
            value.as_u64().or_else(|| {
                value
                    .as_str()
                    .and_then(|text| text.trim().parse::<u64>().ok())
            })
        })
    })
}

pub(super) fn bool_param(params: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter().find_map(|key| {
        params.get(*key).and_then(|value| {
            value.as_bool().or_else(|| {
                value.as_str().map(|text| {
                    matches!(
                        text.trim().to_ascii_lowercase().as_str(),
                        "1" | "true" | "yes" | "on"
                    )
                })
            })
        })
    })
}

pub(super) fn retry_policy_from_request(request: &Value, max_attempts: Option<u64>) -> RetryPolicy {
    RetryPolicy::new(
        max_attempts.unwrap_or_else(|| {
            number_param(request, &["maxAttempts"]).unwrap_or(DEFAULT_MAX_ATTEMPTS)
        }),
        number_param(request, &["retryBackoffSeconds"]).unwrap_or(0),
    )
}

pub(super) fn job_id_for(request: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(request).unwrap_or_default());
    hasher.update(timestamp().as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    format!("conversation_archive_job_{}", &digest[..24])
}

pub(super) fn display_path(path: &Path) -> String {
    path.display().to_string()
}

pub(super) fn local_path_from_user_input(value: &str, label: &str) -> Result<PathBuf> {
    let value = value.trim();
    ensure!(!value.is_empty(), "{label} is required");
    ensure!(
        !value.contains('\0'),
        "{label} contains an invalid path character"
    );
    ensure!(
        !value.starts_with("//") && !value.starts_with("\\"),
        "{label} must be a local filesystem path, not a network share"
    );
    ensure!(
        !has_uri_scheme(value),
        "{label} must be a local filesystem path, not a URI"
    );
    let expanded = expand_home(value);
    if expanded.is_absolute() {
        return Ok(expanded);
    }
    Ok(std::env::current_dir()?.join(expanded))
}

fn has_uri_scheme(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
    {
        return false;
    }
    let Some(separator) = value.find(':') else {
        return false;
    };
    let scheme = &value[..separator];
    !scheme.is_empty()
        && scheme.as_bytes()[0].is_ascii_alphabetic()
        && scheme
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
}

fn expand_home(value: &str) -> PathBuf {
    if value == "~" || value.starts_with("~/") {
        if let Some(home) = licoup_foundation::platform::paths::user_home_from_env() {
            if value == "~" {
                return home;
            }
            return home.join(value.trim_start_matches("~/"));
        }
    }
    PathBuf::from(value)
}
