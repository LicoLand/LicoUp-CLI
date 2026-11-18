use super::*;

pub(super) fn resolve_skill_root(agent_id: &str, params: &Value) -> Result<PathBuf> {
    if let Some(root) = string_param(params, &["skillRoot"], 1) {
        return absolute_lexical_path(Path::new(&root));
    }
    match agent_id {
        "codex" => {
            let codex_home = env::var("CODEX_HOME")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from)
                .or_else(|| home_dir().map(|home| home.join(".codex")))
                .ok_or_else(|| anyhow!("cannot resolve CODEX_HOME or user home for Codex"))?;
            Ok(codex_home.join("skills"))
        }
        "claude-code" => {
            let claude_home = env::var("CLAUDE_HOME")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from)
                .or_else(|| home_dir().map(|home| home.join(".claude")))
                .ok_or_else(|| {
                    anyhow!("cannot resolve CLAUDE_HOME or user home for Claude Code")
                })?;
            Ok(claude_home.join("skills"))
        }
        _ => Err(anyhow!(
            "target adapter '{agent_id}' has no built-in local skill root"
        )),
    }
}

pub(super) fn home_dir() -> Option<PathBuf> {
    licoup_foundation::platform::paths::user_home_from_env()
}

pub(super) fn get_approved_pairing(
    store: &ClientStateStore,
    agent_id: &str,
) -> Result<Option<Value>> {
    let document = store.read_collection("pairings")?;
    Ok(document
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .find(|item| {
                    item.get("agentId").and_then(Value::as_str) == Some(agent_id)
                        && item.get("status").and_then(Value::as_str) == Some(STATUS_APPROVED)
                })
                .cloned()
        })
        .unwrap_or(None))
}

pub(super) fn is_explicitly_revealed(
    store: &ClientStateStore,
    agent_id: &str,
    skill_id: &str,
) -> bool {
    let document = match store.read_collection("skills") {
        Ok(doc) => doc,
        Err(_) => return false,
    };
    document
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter().any(|item| {
                item.get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("visibility")
                    != "skill"
                    && item.get("agentId").and_then(Value::as_str) == Some(agent_id)
                    && item.get("skillId").and_then(Value::as_str) == Some(skill_id)
                    && item.get("hidden").and_then(Value::as_bool) == Some(false)
                    && item.get("visibility").and_then(Value::as_str) == Some("allowed")
            })
        })
        .unwrap_or(false)
}

pub(super) fn is_hidden(store: &ClientStateStore, agent_id: &str, skill_id: &str) -> Result<bool> {
    let document = store.read_collection("skills")?;
    Ok(document
        .get("items")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter().any(|item| {
                item.get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("visibility")
                    != "skill"
                    && item.get("agentId").and_then(Value::as_str) == Some(agent_id)
                    && item.get("skillId").and_then(Value::as_str) == Some(skill_id)
                    && item.get("hidden").and_then(Value::as_bool).unwrap_or(false)
            })
        })
        .unwrap_or(false))
}

/// Local skill management is itself the user's explicit action, so a missing
/// pairing is created already approved and kept only as an audit record. A
/// legacy requested record is approved in place; an explicit revocation is
/// still honored.
pub(super) fn is_agent_approved(store: &ClientStateStore, agent_id: &str) -> Result<bool> {
    let mut document = store.read_collection("pairings")?;
    let existing_index = document
        .get("items")
        .and_then(Value::as_array)
        .and_then(|items| {
            items
                .iter()
                .position(|item| item.get("agentId").and_then(Value::as_str) == Some(agent_id))
        });
    if let Some(index) = existing_index {
        let status = document["items"][index]
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        match status.as_str() {
            STATUS_REVOKED => return Ok(false),
            STATUS_APPROVED => return Ok(true),
            _ => {
                document["items"][index]["status"] = json!(STATUS_APPROVED);
                document["items"][index]["approvedAt"] = json!(timestamp());
                store.write_collection("pairings", document)?;
                return Ok(true);
            }
        }
    }
    let now = timestamp();
    let pairing_id = format!("pair-{}", uuid_v4());
    let record = json!({
        "pairingId": pairing_id,
        "agentId": agent_id,
        "target": agent_id,
        "targetKind": "unknown",
        "label": agent_id,
        "configPath": "",
        "binaryPath": "",
        "localIdentity": format!("local-{}", uuid_v4()),
        "status": STATUS_APPROVED,
        "requestedAt": now,
        "approvedAt": now,
        "defaultVisibilityPolicy": "allow-all",
        "scopes": [],
    });
    let items = collection_items_mut(&mut document)?;
    items.push(record);
    store.write_collection("pairings", document)?;
    append_activity(
        store,
        "pairing.approved",
        json!({
            "target": agent_id,
            "agentId": agent_id,
            "pairingId": pairing_id,
            "origin": "auto"
        }),
    )?;
    Ok(true)
}

pub(super) fn upsert_policy_item(
    items: &mut Vec<Value>,
    agent_id: &str,
    skill_id: &str,
    replacement: Value,
) {
    items.retain(|item| {
        !(item
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("visibility")
            != "skill"
            && item.get("agentId").and_then(Value::as_str) == Some(agent_id)
            && item.get("skillId").and_then(Value::as_str) == Some(skill_id))
    });
    items.push(replacement);
}

pub(super) fn collection_items_mut(document: &mut Value) -> Result<&mut Vec<Value>> {
    if document.get("items").and_then(Value::as_array).is_none() {
        document["items"] = json!([]);
    }
    document
        .get_mut("items")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| anyhow!("state collection is missing items array"))
}

pub(super) fn append_activity(
    store: &ClientStateStore,
    event_type: &str,
    payload: Value,
) -> Result<Value> {
    store.activity_log().append(event_type, payload)
}

pub(super) fn pairing_required(agent_id: &str) -> Value {
    json!({
        "ok": false,
        "error": "pairing_required",
        "agentId": agent_id
    })
}

pub(super) fn agent_id(params: &Value) -> Result<String> {
    string_param(params, &["agent", "agentId", "id"], 0)
        .ok_or_else(|| anyhow!("missing --agent <agent-id>"))
}

pub(super) fn target_id(params: &Value) -> Option<String> {
    string_param(params, &["target"], 1)
}

pub(super) fn skill_id(params: &Value) -> Result<String> {
    string_param(params, &["skill", "skillId", "id"], 0).ok_or_else(|| anyhow!("missing skill id"))
}

pub(super) fn string_param(
    params: &Value,
    keys: &[&str],
    positional_index: usize,
) -> Option<String> {
    for key in keys {
        if let Some(value) = params.get(*key).and_then(Value::as_str) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    params
        .get("positionals")
        .and_then(Value::as_array)
        .and_then(|items| items.get(positional_index))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub(super) fn bool_param(params: &Value, key: &str) -> Option<bool> {
    params.get(key).and_then(|value| {
        value.as_bool().or_else(|| {
            value.as_str().map(|raw| {
                matches!(
                    raw.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on" | "hidden" | "hide"
                )
            })
        })
    })
}

pub(super) fn is_path_inside(parent_path: &Path, candidate_path: &Path) -> bool {
    let parent = normalize_lexical_path(parent_path);
    let candidate = normalize_lexical_path(candidate_path);
    candidate.starts_with(parent)
}

pub(super) fn normalize_lexical_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

pub(super) fn absolute_lexical_path(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()?.join(path)
    };
    ensure!(
        !absolute
            .components()
            .any(|component| matches!(component, Component::ParentDir)),
        "managed skill path contains a parent traversal component"
    );
    Ok(normalize_lexical_path(&absolute))
}

pub(super) fn directory_exists_no_follow(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "managed skill path is not a stable directory"
            );
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
pub(super) fn display_path(path: PathBuf) -> String {
    path.to_string_lossy().to_string()
}

pub(super) fn timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}-{}", now.as_secs(), now.subsec_nanos())
}

pub(super) fn uuid_v4() -> String {
    Uuid::new_v4().to_string()
}
