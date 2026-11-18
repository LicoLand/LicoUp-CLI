use anyhow::Result;
use licoup_client_state::ClientStateStore;
use serde_json::Value;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn client_state_store(params: &Value) -> Result<ClientStateStore> {
    if let Some(root) = params
        .get("stateRoot")
        .or_else(|| params.get("clientStateRoot"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return ClientStateStore::new(PathBuf::from(root));
    }
    if let Some(portable_dir) = params
        .get("portableDir")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return ClientStateStore::new(PathBuf::from(portable_dir).join("client-state"));
    }
    ClientStateStore::portable()
}

pub(super) fn client_state_store_read_only(params: &Value) -> Result<ClientStateStore> {
    if let Some(root) = params
        .get("stateRoot")
        .or_else(|| params.get("clientStateRoot"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(ClientStateStore::open_read_only(PathBuf::from(root)));
    }
    if let Some(portable_dir) = params
        .get("portableDir")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(ClientStateStore::open_read_only(
            PathBuf::from(portable_dir).join("client-state"),
        ));
    }
    ClientStateStore::portable_read_only()
}

pub(super) fn timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}-{}", now.as_secs(), now.subsec_nanos())
}

pub(super) fn display_path(path: PathBuf) -> String {
    path.to_string_lossy().to_string()
}
