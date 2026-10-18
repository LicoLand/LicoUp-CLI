//! Delegated-task transcripts stored beside their conversation.
//!
//! Cursor and Claude Code both write one directory per conversation and put each
//! delegated task in a `subagents/` directory inside it:
//!
//! ```text
//! ~/.cursor/projects/<mangled-project>/agent-transcripts/
//!   <conversationId>/
//!     <conversationId>.jsonl          the conversation
//!     subagents/<taskId>.jsonl        one delegated task each
//!
//! ~/.claude/projects/<mangled-project>/
//!   <sessionId>.jsonl                 the conversation
//!   <sessionId>/subagents/agent-<taskId>.jsonl
//! ```
//!
//! In both stores a task transcript reports the *conversation's* identity rather
//! than its own: Cursor records carry no session field at all, so the generic
//! JSONL reader falls back to the first UUID directory component above the file,
//! and Claude Code records carry `sessionId`, which is the conversation. Left
//! alone, a conversation and each of its tasks share one identity, so nothing can
//! relate them and every task surfaces as its own conversation. This module is
//! the single place that reads the layout, so the catalog and the parsers agree
//! on identity and lineage.

use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use super::project_workspace::bounded_project_workspace;

/// Directory holding one conversation's delegated task transcripts.
pub(crate) const DELEGATED_TASKS_DIRECTORY: &str = "subagents";
/// Cursor CLI project trees nest conversations one level deeper than Claude Code.
pub(crate) const CURSOR_TRANSCRIPTS_DIRECTORY: &str = "agent-transcripts";
const MAX_DELEGATED_TITLE_CHARS: usize = 80;
/// Shortest line accepted as a task label before falling back to a shorter one.
const MIN_DELEGATED_TITLE_CHARS: usize = 24;
const WORKSPACE_TRUST_FILE: &str = ".workspace-trusted";
const MAX_WORKSPACE_TRUST_BYTES: u64 = 64 * 1024;

/// Conversation this transcript belongs to. A delegated task transcript reports
/// the conversation that spawned it, not its own task id.
pub(crate) fn transcript_conversation_id(path: &Path) -> Option<String> {
    if let Some(owner) = delegated_conversation_directory(path) {
        return directory_name(owner).map(str::to_string);
    }
    let owner = path.parent()?;
    if directory_name(owner) == Some(CURSOR_TRANSCRIPTS_DIRECTORY) {
        // A Cursor transcript written directly under `agent-transcripts/` carries
        // its identity in the file name.
        return file_stem(path);
    }
    // Cursor nests one directory per conversation; Claude Code writes the
    // transcript straight into the project directory.
    directory_name(owner)
        .filter(|name| Some(*name) == file_stem(path).as_deref())
        .map(str::to_string)
        .or_else(|| file_stem(path))
}

/// Whether this transcript is a delegated task of its conversation.
///
/// Claude Code nests workflow-driven tasks deeper
/// (`subagents/workflows/<workflowId>/agent-<taskId>.jsonl`), so the marker is a
/// `subagents` component anywhere above the file, not only its direct parent.
pub(crate) fn transcript_is_delegated(path: &Path) -> bool {
    delegated_conversation_directory(path).is_some()
}

/// Directory of the conversation that owns this delegated transcript.
fn delegated_conversation_directory(path: &Path) -> Option<&Path> {
    let mut ancestors = path.ancestors().skip(1);
    loop {
        let ancestor = ancestors.next()?;
        if directory_name(ancestor) == Some(DELEGATED_TASKS_DIRECTORY) {
            return ancestor.parent();
        }
        if ancestor.parent().is_none() {
            return None;
        }
    }
}

/// Delegated lineage for one transcript path: the task id and the conversation
/// that spawned it. `None` for a conversation transcript.
pub(crate) fn delegated_transcript_lineage(path: &Path) -> Option<(String, String)> {
    let conversation_id = directory_name(delegated_conversation_directory(path)?)?.to_string();
    let task_id = file_stem(path)?;
    (task_id != conversation_id).then_some((task_id, conversation_id))
}

/// Whether one delegated file is a conversation transcript rather than workflow
/// bookkeeping. Claude Code writes `journal.jsonl` and `<task>.meta.json` beside
/// the task transcripts; neither is a conversation.
pub(crate) fn delegated_file_is_transcript(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(extension.as_str(), "jsonl" | "ndjson") {
        return false;
    }
    let Some(stem) = file_stem(path) else {
        return false;
    };
    stem != "journal" && !stem.ends_with(".meta")
}

/// Project directory of the Cursor CLI project tree holding this transcript.
///
/// The project root is the directory that owns `agent-transcripts/`, and it is
/// the only place whose `.workspace-trusted` describes this conversation. A
/// marker further up belongs to a different trust decision — Cursor writes
/// `~/.cursor/projects/.workspace-trusted` with `workspacePath: "/"` — so
/// walking past the project root would hand every conversation the filesystem
/// root.
pub(crate) fn cursor_transcript_project_workspace(path: &Path) -> Option<String> {
    let project = path
        .ancestors()
        .find(|ancestor| directory_name(ancestor) == Some(CURSOR_TRANSCRIPTS_DIRECTORY))?
        .parent()?;
    let trusted = project.join(WORKSPACE_TRUST_FILE);
    if !trusted.is_file() {
        return None;
    }
    if fs::metadata(&trusted)
        .ok()
        .is_some_and(|metadata| metadata.len() > MAX_WORKSPACE_TRUST_BYTES)
    {
        return None;
    }
    let raw = fs::read_to_string(&trusted).ok()?;
    let value = serde_json::from_str::<Value>(&raw).ok()?;
    bounded_project_workspace(value.get("workspacePath").and_then(Value::as_str)?)
}

/// Identity the generic reader could not derive from the records.
///
/// A conversation transcript whose directory name is not a UUID leaves the
/// generic reader with its `"file"` placeholder, which collapses every such
/// conversation of one agent into a single identity. The layout always knows the
/// conversation, so it fills the gap.
pub(crate) fn apply_transcript_identity(session: &mut Value, path: &Path) {
    if transcript_is_delegated(path) {
        return;
    }
    let derived = session
        .get("nativeSessionId")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if !derived.is_empty() && derived != "file" {
        return;
    }
    let Some(conversation_id) = transcript_conversation_id(path) else {
        return;
    };
    if let Some(object) = session.as_object_mut() {
        object.insert("nativeSessionId".to_string(), json!(conversation_id));
    }
}

/// Rewrite one parsed delegated transcript so the session merge can fold it into
/// its conversation as a collapsed task card.
pub(crate) fn mark_delegated_transcript_session(session: &mut Value, path: &Path) {
    let Some((task_id, conversation_id)) = delegated_transcript_lineage(path) else {
        return;
    };
    let declared = delegated_task_metadata(path);
    let title = declared
        .as_ref()
        .and_then(|declared| declared.description.clone())
        .or_else(|| delegated_task_title(session));
    let Some(object) = session.as_object_mut() else {
        return;
    };
    object.insert("nativeSessionId".to_string(), json!(task_id));
    object.insert("delegatedSubagent".to_string(), json!(true));
    object.insert("parentSessionId".to_string(), json!(conversation_id));
    if let Some(title) = title {
        object.insert("subagentTitle".to_string(), json!(title));
    }
    if let Some(declared) = declared {
        if let Some(agent_type) = declared.agent_type {
            object.insert("subagentType".to_string(), json!(agent_type));
        }
        if let Some(spawn_depth) = declared.spawn_depth {
            object.insert("subagentSpawnDepth".to_string(), json!(spawn_depth));
        }
    }
}

/// What the conversation declared when it delegated the task.
struct DelegatedTaskMetadata {
    description: Option<String>,
    agent_type: Option<String>,
    spawn_depth: Option<u64>,
}

/// Claude Code writes `<taskId>.meta.json` beside each task transcript with the
/// task description, the agent type, and the nesting depth. That description is
/// the label the user chose, so it beats guessing from the prompt text.
fn delegated_task_metadata(path: &Path) -> Option<DelegatedTaskMetadata> {
    let stem = file_stem(path)?;
    let meta = path.with_file_name(format!("{stem}.meta.json"));
    if !meta.is_file() {
        return None;
    }
    if fs::metadata(&meta)
        .ok()
        .is_some_and(|metadata| metadata.len() > MAX_WORKSPACE_TRUST_BYTES)
    {
        return None;
    }
    let raw = fs::read_to_string(&meta).ok()?;
    let value = serde_json::from_str::<Value>(&raw).ok()?;
    Some(DelegatedTaskMetadata {
        description: value
            .get("description")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.chars().take(MAX_DELEGATED_TITLE_CHARS).collect()),
        agent_type: value
            .get("agentType")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        spawn_depth: value.get("spawnDepth").and_then(Value::as_u64),
    })
}

/// Task label taken from the prompt the conversation handed the delegated agent.
fn delegated_task_title(session: &Value) -> Option<String> {
    delegated_task_label(delegated_task_prompt_text(session)?)
}

/// The instruction text of a delegated task, from its first authored message.
pub(crate) fn delegated_task_prompt_text(session: &Value) -> Option<&str> {
    session
        .get("messages")
        .and_then(Value::as_array)?
        .iter()
        .find(|message| {
            message
                .get("role")
                .and_then(Value::as_str)
                .is_some_and(|role| role == "user")
        })
        .and_then(|message| message.get("text"))
        .and_then(Value::as_str)
}

/// One readable label for a delegated task instruction.
///
/// Every agent wraps environment context in tag blocks (`<git-context>`,
/// `<timestamp>`, `<ADDITIONAL_METADATA>`) and prefixes the instruction with
/// short settings lines such as `Thoroughness: medium.` or an agent nickname.
/// None of those describes the task, so the first substantive line wins and a
/// short line is only used when nothing longer exists.
pub(crate) fn delegated_task_label(text: &str) -> Option<String> {
    let mut fallback: Option<&str> = None;
    let mut open_blocks = 0usize;
    for line in text.lines().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        if line.starts_with("</") {
            open_blocks = open_blocks.saturating_sub(1);
            continue;
        }
        if line.starts_with('<') {
            if !line.ends_with("/>") && !line.contains("</") {
                open_blocks += 1;
            }
            continue;
        }
        if open_blocks > 0 || line.starts_with('#') {
            continue;
        }
        if line.chars().count() >= MIN_DELEGATED_TITLE_CHARS {
            return Some(truncated_label(line));
        }
        if fallback.is_none() {
            fallback = Some(line);
        }
    }
    fallback.map(truncated_label)
}

fn truncated_label(line: &str) -> String {
    line.chars()
        .take(MAX_DELEGATED_TITLE_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

fn directory_name(path: &Path) -> Option<&str> {
    path.file_name().and_then(|name| name.to_str())
}

fn file_stem(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn cursor_conversation_transcript() -> PathBuf {
        PathBuf::from("/store/projects/project-alpha")
            .join(CURSOR_TRANSCRIPTS_DIRECTORY)
            .join("11111111-1111-4111-8111-111111111111")
            .join("11111111-1111-4111-8111-111111111111.jsonl")
    }

    fn cursor_delegated_transcript() -> PathBuf {
        PathBuf::from("/store/projects/project-alpha")
            .join(CURSOR_TRANSCRIPTS_DIRECTORY)
            .join("11111111-1111-4111-8111-111111111111")
            .join(DELEGATED_TASKS_DIRECTORY)
            .join("22222222-2222-4222-8222-222222222222.jsonl")
    }

    fn claude_delegated_transcript() -> PathBuf {
        PathBuf::from("/store/claude/-Users-resident-project")
            .join("33333333-3333-4333-8333-333333333333")
            .join(DELEGATED_TASKS_DIRECTORY)
            .join("agent-a7975e289d9a63743.jsonl")
    }

    #[test]
    fn a_conversation_transcript_has_no_delegated_lineage() {
        let path = cursor_conversation_transcript();
        assert!(!transcript_is_delegated(&path));
        assert_eq!(delegated_transcript_lineage(&path), None);
        assert_eq!(
            transcript_conversation_id(&path).as_deref(),
            Some("11111111-1111-4111-8111-111111111111")
        );
    }

    #[test]
    fn a_delegated_transcript_reports_its_own_identity_and_its_conversation() {
        let path = cursor_delegated_transcript();
        assert!(transcript_is_delegated(&path));
        assert_eq!(
            delegated_transcript_lineage(&path),
            Some((
                "22222222-2222-4222-8222-222222222222".to_string(),
                "11111111-1111-4111-8111-111111111111".to_string()
            ))
        );
        assert_eq!(
            transcript_conversation_id(&path).as_deref(),
            Some("11111111-1111-4111-8111-111111111111")
        );
    }

    #[test]
    fn the_claude_code_layout_uses_the_same_delegated_directory() {
        let path = claude_delegated_transcript();
        assert!(transcript_is_delegated(&path));
        assert_eq!(
            delegated_transcript_lineage(&path),
            Some((
                "agent-a7975e289d9a63743".to_string(),
                "33333333-3333-4333-8333-333333333333".to_string()
            ))
        );
        assert_eq!(
            transcript_conversation_id(&path).as_deref(),
            Some("33333333-3333-4333-8333-333333333333")
        );
    }

    #[test]
    fn marking_a_delegated_session_sets_identity_lineage_and_label() {
        let mut session = json!({
            "nativeSessionId": "11111111-1111-4111-8111-111111111111",
            "messages": [
                {"role": "user", "text": "<timestamp>ignored</timestamp>\nMap the scan pipeline"},
                {"role": "agent", "text": "done"}
            ]
        });
        mark_delegated_transcript_session(&mut session, &cursor_delegated_transcript());
        assert_eq!(
            session.get("nativeSessionId").and_then(Value::as_str),
            Some("22222222-2222-4222-8222-222222222222")
        );
        assert_eq!(
            session.get("parentSessionId").and_then(Value::as_str),
            Some("11111111-1111-4111-8111-111111111111")
        );
        assert_eq!(
            session.get("delegatedSubagent").and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            session.get("subagentTitle").and_then(Value::as_str),
            Some("Map the scan pipeline")
        );
    }

    #[test]
    fn a_transcript_outside_the_conversation_layout_is_left_alone() {
        let mut session = json!({"nativeSessionId": "keep", "messages": []});
        mark_delegated_transcript_session(&mut session, Path::new("/store/loose/notes.jsonl"));
        assert_eq!(
            session.get("nativeSessionId").and_then(Value::as_str),
            Some("keep")
        );
        assert!(session.get("parentSessionId").is_none());
    }
}
