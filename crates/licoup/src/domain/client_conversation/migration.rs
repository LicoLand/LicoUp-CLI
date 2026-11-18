//! One-time migration from the former local projection/group generation.
//!
//! The reader is intentionally bounded and isolated. It only consumes files
//! owned by LicoUp under the portable client-state root; third-party Agent
//! history is never scanned here. A private completion marker prevents a second
//! process from reopening the old format after a successful cutover.

use super::{
    ConversationStore, EventKind, EventPartKind, MembershipAccess, NewEventPart, Principal,
    PrincipalKind, RuntimeBinding,
};
use anyhow::{Result, anyhow};
use fs2::FileExt;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

const MIGRATION_VERSION: &str = "v5";
const LEGACY_GROUP_WRITER_LOCK_FILE: &str = "lico-group-default.json.lock";
const MAX_LEGACY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SESSIONS_PER_AGENT: usize = 100;
const MAX_AGENTS: usize = 64;
const MAX_MESSAGES_PER_SESSION: usize = 100_000;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MigrationReport {
    pub migrated_conversations: usize,
    pub migrated_events: usize,
    pub skipped_sources: usize,
    pub cleaned_files: usize,
    pub already_complete: bool,
}

pub fn migrate_legacy_state(
    store: &ConversationStore,
    portable_root: &Path,
) -> Result<MigrationReport> {
    let state_root = portable_root.join("client-state");
    let canonical_root = state_root.join("conversations");
    licoup_foundation::platform::file_security::ensure_private_dir(&state_root)?;
    licoup_foundation::platform::file_security::ensure_private_dir(&canonical_root)?;
    let marker = canonical_root.join(format!("migration-{MIGRATION_VERSION}.complete"));
    if marker.is_file() {
        return Ok(MigrationReport {
            already_complete: true,
            ..MigrationReport::default()
        });
    }
    let lock_path = canonical_root.join("migration.lock");
    let lock = licoup_foundation::platform::file_security::open_private_lock_file(&lock_path)?;
    lock.lock_exclusive()
        .map_err(|_| anyhow!("migration_lock_unavailable"))?;
    if marker.is_file() {
        return Ok(MigrationReport {
            already_complete: true,
            ..MigrationReport::default()
        });
    }

    let mut report = MigrationReport::default();
    let mut migrated_ids = BTreeMap::<String, String>::new();
    let projection_path = state_root.join("agent-conversation-projections.json");
    if let Some(document) = read_json_if_present(&projection_path)? {
        migrate_projection(store, &document, &mut migrated_ids, &mut report)?;
    }

    let group_root = state_root.join("group-conversations");
    let group_path = group_root.join("lico-group-default.json");
    let legacy_group_files = validate_legacy_group_root(&group_root)?;
    if let Some(document) = read_json_if_present(&group_path)? {
        migrate_group(store, &document, &mut migrated_ids, &mut report)?;
    }

    // The retired ordinal Flywheel document has no representation in the
    // Graph strategy domain. It is removed only after all still-supported
    // Conversation sources have committed successfully.
    let flywheel_path = state_root.join("adaptive-flywheel.toml");

    // Cleanup happens only after every source has been parsed and destination
    // writes have committed. Missing files are not errors, making restart
    // cleanup idempotent.
    for path in [projection_path, flywheel_path] {
        if path.is_file() {
            fs::remove_file(&path).map_err(|_| anyhow!("migration_cleanup_failed"))?;
            report.cleaned_files += 1;
        }
    }
    if group_root.is_dir() {
        fs::remove_dir_all(&group_root).map_err(|_| anyhow!("migration_cleanup_failed"))?;
        report.cleaned_files += legacy_group_files;
    }
    // A group adopted by the legacy import (provenance for the reserved
    // default group) is normalized and renamed by the same idempotent current
    // schema routine before the completion marker makes the run final.
    store.normalize_reserved_default_group_after_legacy_import()?;
    let marker_text = format!("schema={MIGRATION_VERSION}\nstatus=complete\n");
    licoup_foundation::platform::file_security::atomic_write_private_text(&marker, &marker_text)?;
    Ok(report)
}

fn validate_legacy_group_root(root: &Path) -> Result<usize> {
    if !root.exists() {
        return Ok(0);
    }
    if !root.is_dir() {
        return Err(anyhow!("migration_group_source_invalid"));
    }
    let mut count = 0usize;
    for entry in fs::read_dir(root).map_err(|_| anyhow!("migration_source_unavailable"))? {
        let path = entry
            .map_err(|_| anyhow!("migration_source_unavailable"))?
            .path();
        if path.file_name().and_then(|value| value.to_str()) == Some("lico-group-default.json") {
            let _ = read_json_if_present(&path)?
                .ok_or_else(|| anyhow!("migration_group_source_invalid"))?;
            count += 1;
            continue;
        }
        if path.file_name().and_then(|value| value.to_str()) == Some(LEGACY_GROUP_WRITER_LOCK_FILE)
        {
            let metadata =
                fs::symlink_metadata(&path).map_err(|_| anyhow!("migration_source_unavailable"))?;
            if !metadata.file_type().is_file() || metadata.len() != 0 {
                return Err(anyhow!("migration_group_source_invalid"));
            }
            count += 1;
            continue;
        }
        if path.file_name().and_then(|value| value.to_str()) != Some("transcripts")
            || !path.is_dir()
        {
            return Err(anyhow!("migration_group_source_invalid"));
        }
        for transcript in
            fs::read_dir(&path).map_err(|_| anyhow!("migration_source_unavailable"))?
        {
            let transcript = transcript
                .map_err(|_| anyhow!("migration_source_unavailable"))?
                .path();
            if transcript.file_name().and_then(|value| value.to_str())
                != Some("lico-group-default.jsonl")
            {
                return Err(anyhow!("migration_group_source_invalid"));
            }
            let metadata =
                fs::metadata(&transcript).map_err(|_| anyhow!("migration_source_unavailable"))?;
            if !metadata.is_file() || metadata.len() > MAX_LEGACY_BYTES {
                return Err(anyhow!("migration_group_source_invalid"));
            }
            if metadata.len() != 0 {
                return Err(anyhow!("migration_group_transcript_unsupported"));
            }
            count += 1;
        }
    }
    Ok(count)
}

fn migrate_projection(
    store: &ConversationStore,
    document: &Value,
    migrated_ids: &mut BTreeMap<String, String>,
    report: &mut MigrationReport,
) -> Result<()> {
    let sessions = document
        .get("sessionsByAgent")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("migration_projection_invalid"))?;
    if sessions.len() > MAX_AGENTS {
        return Err(anyhow!("migration_agent_limit_exceeded"));
    }
    for (agent_id, values) in sessions {
        let Some(values) = values.as_array() else {
            continue;
        };
        if values.len() > MAX_SESSIONS_PER_AGENT {
            return Err(anyhow!("migration_session_limit_exceeded"));
        }
        for session in values {
            let Some(object) = session.as_object() else {
                report.skipped_sources += 1;
                continue;
            };
            let Some(session_id) = object
                .get("id")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
            else {
                report.skipped_sources += 1;
                continue;
            };
            let source_identity = migrated_projection_source_identity(agent_id, session_id)?;
            let canonical_id = format!("conversation:projection:{source_identity}");
            if let Some(id) = store.migration_conversation("projection", &source_identity)? {
                migrated_ids.insert(source_identity, id);
                continue;
            }
            store.reset_incomplete_migration_conversation(
                "projection",
                &source_identity,
                &canonical_id,
            )?;
            let title = object
                .get("title")
                .and_then(Value::as_str)
                .filter(|v| !v.trim().is_empty())
                .unwrap_or(session_id);
            let owner = local_owner();
            let conversation = store.create_conversation_with_id(&canonical_id, title, owner)?;
            let agent_principal = Principal {
                id: format!("agent:{agent_id}"),
                kind: PrincipalKind::Agent,
                display_name: object
                    .get("agentDisplayName")
                    .and_then(Value::as_str)
                    .unwrap_or(agent_id)
                    .to_owned(),
                agent_id: Some(agent_id.clone()),
                created_at_unix_ms: 0,
            };
            let agent_membership =
                store.add_member(&conversation.id, agent_principal, MembershipAccess::Member)?;
            let owner_membership = conversation
                .memberships
                .first()
                .map(|membership| membership.id.clone())
                .ok_or_else(|| anyhow!("migration_owner_missing"))?;
            let Some(messages) = object.get("messages").and_then(Value::as_array) else {
                store.record_migration("projection", &source_identity, &conversation.id)?;
                migrated_ids.insert(source_identity, conversation.id);
                report.migrated_conversations += 1;
                continue;
            };
            if messages.len() > MAX_MESSAGES_PER_SESSION {
                return Err(anyhow!("migration_message_limit_exceeded"));
            }
            for message in messages {
                let Some(message) = message.as_object() else {
                    continue;
                };
                let parts = message_parts(message);
                if parts.is_empty() {
                    continue;
                }
                let role = message
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or("user")
                    .to_ascii_lowercase();
                let author = if matches!(role.as_str(), "assistant" | "agent" | "tool") {
                    &agent_membership.id
                } else {
                    &owner_membership
                };
                store.append_event(
                    &conversation.id,
                    Some(author),
                    EventKind::Message,
                    &parts,
                    None,
                    None,
                    true,
                )?;
                report.migrated_events += 1;
            }
            store.source_link(&conversation.id, "projection", &source_identity)?;
            store.record_migration("projection", &source_identity, &conversation.id)?;
            migrated_ids.insert(source_identity, conversation.id);
            report.migrated_conversations += 1;
        }
    }
    Ok(())
}

fn migrate_group(
    store: &ConversationStore,
    document: &Value,
    migrated_ids: &mut BTreeMap<String, String>,
    report: &mut MigrationReport,
) -> Result<()> {
    let object = document
        .as_object()
        .ok_or_else(|| anyhow!("migration_group_invalid"))?;
    let source_identity = object
        .get("id")
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .unwrap_or("lico-group-default");
    if let Some(id) = store.migration_conversation("group", source_identity)? {
        migrated_ids.insert(source_identity.to_owned(), id.clone());
        migrated_ids.insert("lico-group-default".to_owned(), id);
        return Ok(());
    }
    let title = object
        .get("title")
        .and_then(Value::as_str)
        .filter(|v| !v.trim().is_empty())
        .unwrap_or(source_identity);
    let participants = object
        .get("roster")
        .and_then(|v| v.get("participants"))
        .and_then(Value::as_array);
    let has_managed_membership = participants.is_some_and(|participants| {
        participants.iter().any(|participant| {
            participant
                .get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.is_empty() && id != "human:local")
        })
    });
    let has_configuration = object
        .get("agentSessions")
        .and_then(Value::as_object)
        .is_some_and(|sessions| !sessions.is_empty())
        || object
            .get("lastLocalOrchestrationSessionId")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty());
    if !has_managed_membership && !has_configuration {
        report.skipped_sources += 1;
        return Ok(());
    }
    store.reset_incomplete_migration_conversation(
        "group",
        source_identity,
        "conversation:migrated-group",
    )?;
    let conversation =
        store.create_group_with_id("conversation:migrated-group", title, local_owner())?;
    store.set_conversation_pinned(&conversation.id, true)?;
    if let Some(participants) = participants {
        for participant in participants {
            let Some(participant) = participant.as_object() else {
                continue;
            };
            let kind = participant
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("human");
            let id = participant.get("id").and_then(Value::as_str).unwrap_or("");
            if id == "human:local" || id.is_empty() {
                continue;
            }
            let principal = Principal {
                id: id.to_owned(),
                kind: if kind == "agent" {
                    PrincipalKind::Agent
                } else {
                    PrincipalKind::Human
                },
                display_name: participant
                    .get("displayName")
                    .and_then(Value::as_str)
                    .unwrap_or(id)
                    .to_owned(),
                agent_id: participant
                    .get("agentId")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                created_at_unix_ms: 0,
            };
            store.add_member(&conversation.id, principal, MembershipAccess::Member)?;
        }
    }
    if let Some(sessions) = object.get("agentSessions").and_then(Value::as_object) {
        if sessions.len() > MAX_AGENTS {
            return Err(anyhow!("migration_agent_limit_exceeded"));
        }
        for (key, value) in sessions {
            let Some(binding) = value.as_object() else {
                report.skipped_sources += 1;
                continue;
            };
            let agent_id = binding
                .get("agentId")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or(key)
                .trim();
            if agent_id.is_empty() {
                report.skipped_sources += 1;
                continue;
            }
            let membership = ensure_agent_membership(store, &conversation.id, agent_id)?;
            let native_session_id = binding
                .get("nativeSessionId")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty());
            let runtime_conversation_path = binding
                .get("sourcePath")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty());
            let working_directory = binding
                .get("workingDirectory")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty());
            store.runtime_binding_with_private_location(
                RuntimeBinding {
                    id: format!("runtime:{}", uuid::Uuid::new_v4()),
                    conversation_id: conversation.id.clone(),
                    membership_id: membership.id,
                    lane: "conversation".into(),
                    availability: "available".into(),
                    safe_reason: None,
                },
                native_session_id,
                runtime_conversation_path,
                working_directory,
            )?;
        }
    }
    if let Some(local_session_id) = object
        .get("lastLocalOrchestrationSessionId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        store.source_link(&conversation.id, "legacy-group-session", local_session_id)?;
    }
    store.source_link(&conversation.id, "group", source_identity)?;
    store.record_migration("group", source_identity, &conversation.id)?;
    migrated_ids.insert(source_identity.to_owned(), conversation.id.clone());
    migrated_ids.insert("lico-group-default".to_owned(), conversation.id);
    report.migrated_conversations += 1;
    Ok(())
}

fn ensure_agent_membership(
    store: &ConversationStore,
    conversation_id: &str,
    agent_id: &str,
) -> Result<super::Membership> {
    let principal_id = format!("agent:{agent_id}");
    if let Some(membership) =
        store
            .get(conversation_id)?
            .memberships
            .into_iter()
            .find(|membership| {
                membership.principal.agent_id.as_deref() == Some(agent_id)
                    || membership.principal.id == principal_id
            })
    {
        return Ok(membership);
    }
    store.add_member(
        conversation_id,
        Principal {
            id: principal_id,
            kind: PrincipalKind::Agent,
            display_name: agent_id.to_owned(),
            agent_id: Some(agent_id.to_owned()),
            created_at_unix_ms: 0,
        },
        MembershipAccess::Member,
    )
}

fn local_owner() -> Principal {
    Principal {
        id: "human:local".into(),
        kind: PrincipalKind::Human,
        display_name: "human:local".into(),
        agent_id: None,
        created_at_unix_ms: 0,
    }
}

fn migrated_projection_source_identity(agent_id: &str, session_id: &str) -> Result<String> {
    let identity = format!("{}:{agent_id}:{session_id}", agent_id.len());
    if identity.len() > 136
        || identity
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(anyhow!("migration_source_identity_invalid"));
    }
    Ok(identity)
}

fn message_parts(message: &serde_json::Map<String, Value>) -> Vec<NewEventPart> {
    let structured = message
        .get("parts")
        .and_then(Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| {
                    let object = part.as_object()?;
                    let content = object
                        .get("content")
                        .or_else(|| object.get("text"))
                        .and_then(Value::as_str)?;
                    if content.is_empty() {
                        return None;
                    }
                    let kind = match object.get("kind").and_then(Value::as_str) {
                        Some("reasoning") => EventPartKind::Reasoning,
                        Some("tool-call") | Some("tool_call") => EventPartKind::ToolCall,
                        Some("tool-result") | Some("tool_result") => EventPartKind::ToolResult,
                        Some("artifact") => EventPartKind::Artifact,
                        Some("diagnostic") => EventPartKind::Diagnostic,
                        Some("metadata") => EventPartKind::Metadata,
                        _ => EventPartKind::Text,
                    };
                    Some(NewEventPart {
                        id: String::new(),
                        kind,
                        content: content.to_owned(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !structured.is_empty() {
        return structured;
    }
    message
        .get("content")
        .and_then(Value::as_str)
        .or_else(|| message.get("text").and_then(Value::as_str))
        .filter(|content| !content.is_empty())
        .map(|content| {
            vec![NewEventPart {
                id: String::new(),
                kind: EventPartKind::Text,
                content: content.to_owned(),
            }]
        })
        .unwrap_or_default()
}

fn read_json_if_present(path: &Path) -> Result<Option<Value>> {
    let Some(content) = read_text_if_present(path)? else {
        return Ok(None);
    };
    serde_json::from_str(&content)
        .map(Some)
        .map_err(|_| anyhow!("migration_json_invalid"))
}

fn read_text_if_present(path: &Path) -> Result<Option<String>> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(anyhow!("migration_source_unavailable")),
    };
    if !metadata.is_file() || metadata.len() > MAX_LEGACY_BYTES {
        return Err(anyhow!("migration_source_too_large"));
    }
    let content = fs::read_to_string(path).map_err(|_| anyhow!("migration_source_unreadable"))?;
    Ok(Some(content))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::fs;
    use uuid::Uuid;

    #[test]
    fn migrates_projection_once_and_removes_legacy_input() {
        let root = std::env::temp_dir().join(format!("lico-migration-{}", Uuid::new_v4()));
        let state = root.join("client-state");
        fs::create_dir_all(&state).unwrap();
        fs::write(state.join("agent-conversation-projections.json"), r#"{"schemaVersion":1,"sessionsByAgent":{"agent-one":[{"id":"session-1","title":"Migrated","messages":[{"role":"user","content":"hello"},{"role":"assistant","content":"world"}]}]}}"#).unwrap();
        let store = ConversationStore::open(&root).unwrap();
        let partial = store
            .create_conversation_with_id(
                "conversation:projection:9:agent-one:session-1",
                "partial",
                local_owner(),
            )
            .unwrap();
        store
            .append_event(
                &partial.id,
                Some(&partial.memberships[0].id),
                EventKind::Message,
                &[NewEventPart {
                    id: String::new(),
                    kind: EventPartKind::Text,
                    content: "partial".into(),
                }],
                None,
                None,
                true,
            )
            .unwrap();
        assert_eq!(
            store
                .migration_conversation("projection", "9:agent-one:session-1")
                .unwrap(),
            None
        );
        let report = migrate_legacy_state(&store, &root).unwrap();
        assert_eq!(report.migrated_conversations, 1);
        let conversations = store.list(false).unwrap();
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].event_count, 3);
        let events = store
            .page_events(&conversations[0].id, None, 100)
            .unwrap()
            .events;
        assert_eq!(
            events
                .iter()
                .filter(|event| event.kind == EventKind::Message)
                .count(),
            2
        );
        assert!(
            events
                .iter()
                .flat_map(|event| &event.parts)
                .all(|part| part.content != "partial")
        );
        assert!(!state.join("agent-conversation-projections.json").exists());
        assert!(
            migrate_legacy_state(&store, &root)
                .unwrap()
                .already_complete
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn migrates_group_with_writer_lock_and_private_runtime_bindings_then_removes_root() {
        let root = std::env::temp_dir().join(format!("lico-group-migration-{}", Uuid::new_v4()));
        let group_root = root.join("client-state").join("group-conversations");
        let transcripts = group_root.join("transcripts");
        fs::create_dir_all(&transcripts).unwrap();
        fs::write(
            group_root.join("lico-group-default.json"),
            r#"{
              "id":"lico-group-default",
              "title":"Migrated group",
              "roster":{"participants":[
                {"id":"human:local","kind":"human","displayName":"You"},
                {"id":"agent:codex","kind":"agent","displayName":"Codex","agentId":"codex"}
              ]},
              "agentSessions":{"codex":{
                "agentId":"codex",
                "nativeSessionId":"native-session",
                "sourcePath":"private-runtime-location",
                "workingDirectory":"private-working-directory"
              }},
              "lastLocalOrchestrationSessionId":"legacy-local-session"
            }"#,
        )
        .unwrap();
        fs::write(group_root.join(LEGACY_GROUP_WRITER_LOCK_FILE), "").unwrap();
        fs::write(transcripts.join("lico-group-default.jsonl"), "").unwrap();
        fs::write(
            root.join("client-state").join("adaptive-flywheel.toml"),
            "retired = true\n",
        )
        .unwrap();

        let store = ConversationStore::open(&root).unwrap();
        let report = migrate_legacy_state(&store, &root).unwrap();

        assert_eq!(report.migrated_conversations, 1);
        assert!(!group_root.exists());
        assert!(store.list(false).unwrap()[0].pinned);
        assert!(
            !root
                .join("client-state")
                .join("adaptive-flywheel.toml")
                .exists()
        );
        let connection = Connection::open(store.db_path()).unwrap();
        let private_binding: (String, String, String) = connection
            .query_row(
                "SELECT runtime_session_id, runtime_conversation_path, working_directory
                 FROM runtime_bindings",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(private_binding.0, "native-session");
        assert_eq!(private_binding.1, "private-runtime-location");
        assert_eq!(private_binding.2, "private-working-directory");
        let _ = fs::remove_dir_all(root);
    }
}
