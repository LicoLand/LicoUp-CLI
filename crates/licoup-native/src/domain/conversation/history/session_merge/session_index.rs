use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::domain::conversation::parameters::text_param;
use crate::domain::conversation::paths::home_dir;
use crate::domain::conversation::source_catalog::{HistoryAdapter, history_roots};

use super::super::message_projection::find_string;
use super::super::query_filter::title_from_text;
use super::super::session_metadata::{extract_conversation_title, meaningful_explicit_title};

pub(crate) fn apply_codex_session_index_titles(params: &Value, sessions: &mut [Value]) {
    let titles = load_codex_session_index_titles(params);
    if titles.is_empty() {
        return;
    }
    for session in sessions.iter_mut() {
        let Some(native_id) = session
            .get("nativeSessionId")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            continue;
        };
        let Some(title) = titles.get(&native_id) else {
            continue;
        };
        if !meaningful_explicit_title(title) {
            continue;
        }
        if let Some(object) = session.as_object_mut() {
            object.insert("title".to_string(), json!(title_from_text(title)));
        }
    }
}

pub(super) fn load_codex_session_index_titles(params: &Value) -> HashMap<String, String> {
    for path in codex_session_index_candidates(params) {
        let titles = read_codex_session_index_titles_file(&path);
        if !titles.is_empty() {
            return titles;
        }
    }
    HashMap::new()
}

pub(super) fn codex_session_index_candidates(params: &Value) -> Vec<PathBuf> {
    let mut candidates = Vec::<PathBuf>::new();
    for root in history_roots(HistoryAdapter::Codex, params) {
        if root.source_kind == "codex-session-index" {
            candidates.push(root.path);
        } else if root.source_kind == "override-root" {
            candidates.push(root.path.join("session_index.jsonl"));
            candidates.push(root.path.join(".codex/session_index.jsonl"));
        }
    }
    if let Some(home) = text_param(params, &["homeDir"]).filter(|value| !value.trim().is_empty()) {
        candidates.push(PathBuf::from(home).join(".codex/session_index.jsonl"));
    }
    if candidates.is_empty() && text_param(params, &["root", "historyRoot", "homeDir"]).is_none() {
        candidates.push(home_dir().join(".codex/session_index.jsonl"));
    }
    candidates
}

pub(super) fn read_codex_session_index_titles_file(path: &Path) -> HashMap<String, String> {
    let Ok(raw) = fs::read_to_string(path) else {
        return HashMap::new();
    };
    parse_codex_session_index_titles(&raw)
}

pub(super) fn parse_codex_session_index_titles(raw: &str) -> HashMap<String, String> {
    let mut stamped = HashMap::<String, (String, String)>::new();
    for line in raw.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(id) = find_string(&value, &["id", "sessionId", "session_id", "thread_id"])
            .filter(|value| !value.trim().is_empty())
        else {
            continue;
        };
        let Some(title) = extract_conversation_title(&value) else {
            continue;
        };
        let updated =
            find_string(&value, &["updated_at", "updatedAt", "timestamp"]).unwrap_or_default();
        match stamped.get(&id) {
            Some((previous, _)) if !updated.is_empty() && previous.as_str() >= updated.as_str() => {
            }
            _ => {
                stamped.insert(id, (updated, title));
            }
        }
    }
    stamped
        .into_iter()
        .map(|(id, (_, title))| (id, title))
        .collect()
}
