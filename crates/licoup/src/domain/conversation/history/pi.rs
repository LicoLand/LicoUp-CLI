//! Pi session identity: the header record owns the native session id and
//! only the known filename suffix is the fallback when no header exists.

use super::*;

pub(crate) struct PiSessionHeader {
    pub(crate) native_session_id: String,
    pub(crate) created_at: Option<String>,
    pub(crate) working_directory: Option<String>,
}

/// One identity owner for Pi browse and exact reads: the native session header
/// wins, and only the known filename suffix is the fallback when no header id
/// exists.
pub(crate) fn pi_session_header(path: &Path) -> PiSessionHeader {
    let fallback = path
        .file_stem()
        .and_then(|value| value.to_str())
        .and_then(|stem| stem.rsplit_once('_').map(|(_, id)| id.to_string()))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "pi-session".to_string());
    let value = fs::File::open(path)
        .ok()
        .and_then(|file| BufReader::new(file).lines().next())
        .and_then(|line| line.ok())
        .and_then(|line| serde_json::from_str::<Value>(&line).ok())
        .filter(|value| value.get("type").and_then(Value::as_str) == Some("session"));
    let native_session_id = value
        .as_ref()
        .and_then(|value| value.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or(fallback);
    let created_at = value
        .as_ref()
        .and_then(|value| value.get("timestamp"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let working_directory = value
        .as_ref()
        .and_then(|value| value.get("cwd"))
        .and_then(Value::as_str)
        .and_then(super::project_workspace::bounded_project_workspace);
    PiSessionHeader {
        native_session_id,
        created_at,
        working_directory,
    }
}
