//! Conversation-catalog invariants: 13 regression tests, one per documented
//! invariant of the native history catalog and loading contract
//! (conversation-catalog-and-loading.md).
//!
//! Each test builds a synthetic fixture (temporary home, synthetic sqlite
//! store, synthetic transcripts) that reproduces the defect class the invariant
//! was introduced for, and asserts the absence of the defect through the
//! public native facade (`licoup_native::domain::conversations::conversation_list`).
//!
//! Fixtures are synthetic only: temp directories below the platform temp root,
//! generated identifiers, and no real user data, real paths, or machine
//! identity.
//!
//! Invariant map:
//!
//! | Test                                                                  | Invariant |
//! | --------------------------------------------------------------------- | --------- |
//! | one_conversation_recorded_in_several_locations_is_one_row             | I1/I2     |
//! | lineage_outside_the_transcript_folds_delegated_work_into_the_parent   | I4        |
//! | a_drifted_schema_still_yields_its_conversations                       | I11       |
//! | root_and_home_directory_records_are_never_bindable_workspaces         | I6        |
//! | a_stale_recorded_directory_is_provenance_only                         | I6        |
//! | a_delegate_claiming_the_parent_identity_is_identified_as_the_child    | I3/I4     |
//! | the_richest_source_wins_and_metadata_carries_over_from_discarded_copies| I2       |
//! | the_catalog_walk_is_complete_and_iterative                             | I10       |
//! | folding_never_changes_the_parent_own_message_count                    | I4/I10    |
//! | a_delegated_task_whose_whole_trace_is_tool_work_still_appears         | I4        |
//! | an_unrecognized_schema_falls_back_to_file_extraction_instead_of_none  | I9/I11    |
//! | the_exact_read_is_narrowed_and_includes_delegated_files               | I8        |
//! | the_projection_cache_only_accelerates_never_changes_semantics         | cache     |

use licoup_native::domain::conversations::conversation_list;
use rusqlite::Connection;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

// ---------------------------------------------------------------------------
// Synthetic fixture helpers (temp roots, sqlite stores, transcripts).
// ---------------------------------------------------------------------------

/// Unique temporary fixture root below the platform temp directory.
fn temp_root(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "lico-catalog-invariants-{label}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn now_seconds() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

fn now_millis() -> i64 {
    now_seconds() * 1_000
}

fn iso_ago(days: i64) -> String {
    (OffsetDateTime::now_utc() - time::Duration::days(days))
        .format(&Rfc3339)
        .unwrap()
}

/// Home path the read path itself resolves (`HOME`/`USERPROFILE`/
/// `HOMEDRIVE`+`HOMEPATH` only), mirroring the crate's own resolution rule.
fn env_home_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

fn browse(home: &Path, agent: &str) -> Value {
    conversation_list(&json!({
        "agent": agent,
        "homeDir": home.to_string_lossy().to_string(),
        "limit": 20
    }))
    .unwrap()
}

fn exact_read(home: &Path, agent: &str, session_id: &str) -> Value {
    conversation_list(&json!({
        "agent": agent,
        "homeDir": home.to_string_lossy().to_string(),
        "sessionIds": [session_id]
    }))
    .unwrap()
}

fn read_lazy_child_messages(
    home: &Path,
    agent: &str,
    card: &Value,
    expected_id: &str,
    expected_count: usize,
) -> Vec<Value> {
    assert_eq!(card["childSessionId"], expected_id);
    assert!(card["messages"].as_array().unwrap().is_empty());
    assert_eq!(card["childMessageCount"], expected_count);
    let revision = card["childSourceRevision"].as_str().unwrap();
    assert!(!revision.is_empty());

    let exact = exact_read(home, agent, card["childSessionId"].as_str().unwrap());
    assert_eq!(session_ids(&exact), vec![expected_id.to_string()]);
    let child = &exact["sessions"][0];
    assert_eq!(child["sourceMessageCount"], expected_count);
    assert_eq!(child["sourceRevision"], revision);
    let messages = child["messages"].as_array().unwrap();
    assert_eq!(messages.len(), expected_count);
    assert_eq!(child["messagePage"]["total"], expected_count);
    assert_eq!(child["messagePage"]["returned"], messages.len());
    assert_eq!(child["messagePage"]["hasEarlier"], false);
    messages.clone()
}

fn session_ids(listed: &Value) -> Vec<String> {
    listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|session| session["nativeSessionId"].as_str())
        .map(str::to_string)
        .collect()
}

fn subagent_cards(row: &Value) -> Vec<&Value> {
    row["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["cardType"] == "subagent")
        .collect()
}

fn text_messages(row: &Value) -> Vec<&Value> {
    row["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["cardType"] != "subagent")
        .collect()
}

// -- Codex -----------------------------------------------------------------

fn codex_rollout(home: &Path, session_id: &str, prompt: &str, reply: &str) -> PathBuf {
    let dir = home.join(".codex/sessions/2026/08/01");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("rollout-2026-08-01T00-00-00-{session_id}.jsonl"));
    let lines = [
        json!({
            "timestamp": "2026-08-01T00:00:00Z",
            "type": "session_meta",
            "payload": {"id": session_id, "cwd": "/workspace/catalog"}
        }),
        json!({
            "timestamp": "2026-08-01T00:00:01Z",
            "type": "response_item",
            "payload": {"type": "message", "role": "user",
                        "content": [{"type": "input_text", "text": prompt}]}
        }),
        json!({
            "timestamp": "2026-08-01T00:00:02Z",
            "type": "response_item",
            "payload": {"type": "message", "role": "assistant",
                        "content": [{"type": "output_text", "text": reply}]}
        }),
    ];
    fs::write(
        &path,
        lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    path
}

struct CodexStateThread<'a> {
    id: &'a str,
    rollout_path: &'a Path,
    title: &'a str,
    cwd: &'a str,
    model: &'a str,
    nickname: Option<&'a str>,
    role: Option<&'a str>,
    first_user_message: Option<&'a str>,
}

fn codex_state_db(
    home: &Path,
    threads: &[CodexStateThread<'_>],
    spawn_edges: &[(&str, &str)],
) -> PathBuf {
    let db = home.join(".codex/state_5.sqlite");
    let connection = Connection::open(&db).unwrap();
    connection
        .execute(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                rollout_path TEXT,
                created_at INTEGER,
                updated_at INTEGER,
                title TEXT,
                archived INTEGER,
                cwd TEXT,
                model TEXT,
                agent_nickname TEXT,
                agent_role TEXT,
                first_user_message TEXT
            )",
            [],
        )
        .unwrap();
    for thread in threads {
        connection
            .execute(
                "INSERT INTO threads VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    thread.id,
                    thread.rollout_path.to_string_lossy().to_string(),
                    now_seconds() - 20,
                    now_seconds() - 5,
                    thread.title,
                    thread.cwd,
                    thread.model,
                    thread.nickname,
                    thread.role,
                    thread.first_user_message,
                ],
            )
            .unwrap();
    }
    if !spawn_edges.is_empty() {
        connection
            .execute(
                "CREATE TABLE thread_spawn_edges (
                    parent_thread_id TEXT NOT NULL,
                    child_thread_id TEXT NOT NULL,
                    status TEXT NOT NULL
                )",
                [],
            )
            .unwrap();
        for (parent, child) in spawn_edges {
            connection
                .execute(
                    "INSERT INTO thread_spawn_edges VALUES (?1, ?2, 'closed')",
                    (*parent, *child),
                )
                .unwrap();
        }
    }
    db
}

fn codex_state_db_unrecognized(home: &Path) {
    let db = home.join(".codex/state_5.sqlite");
    let connection = Connection::open(db).unwrap();
    connection
        .execute("CREATE TABLE threads (id TEXT PRIMARY KEY, note TEXT)", [])
        .unwrap();
}

// -- OpenCode / Kilo (openagent shape) -------------------------------------

/// Session table only, with a wide-format id/time_updated/directory column set.
/// The catalog lists these rows; without message/part tables the rows are
/// metadata-only catalog entries.
fn openagent_metadata_db(path: &Path, sessions: &[(&str, Option<&str>)]) {
    let connection = Connection::open(path).unwrap();
    connection
        .execute(
            "CREATE TABLE session (
                id TEXT PRIMARY KEY,
                time_updated INTEGER,
                time_archived INTEGER,
                directory TEXT
            )",
            [],
        )
        .unwrap();
    for (id, directory) in sessions {
        connection
            .execute(
                "INSERT INTO session VALUES (?1, ?2, NULL, ?3)",
                (id, now_millis(), directory),
            )
            .unwrap();
    }
}

/// Cross-version drifted schema: the `session` table carries none of the
/// optional columns (no title, no directory, no model, no time_created), but
/// message/part tables still hold the conversation content.
fn openagent_drifted_db(path: &Path, session_id: &str, prompt: &str) {
    let connection = Connection::open(path).unwrap();
    connection
        .execute(
            "CREATE TABLE session (
                id TEXT PRIMARY KEY,
                time_updated INTEGER,
                time_archived INTEGER
            )",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO session VALUES (?1, ?2, NULL)",
            (session_id, now_millis()),
        )
        .unwrap();
    connection
        .execute(
            "CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER,
                time_updated INTEGER,
                data TEXT NOT NULL
            )",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO message VALUES ('m-1', ?1, ?2, ?2, ?3)",
            rusqlite::params![
                session_id,
                now_millis(),
                json!({"role": "user", "text": prompt}).to_string()
            ],
        )
        .unwrap();
    connection
        .execute(
            "CREATE TABLE part (
                id TEXT PRIMARY KEY,
                message_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                data TEXT NOT NULL
            )",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO part VALUES ('p-1', 'm-1', ?1, ?2)",
            rusqlite::params![
                session_id,
                json!({"type": "text", "text": prompt}).to_string()
            ],
        )
        .unwrap();
}

// -- Claude Code -----------------------------------------------------------

fn claude_transcript_lines(records: impl IntoIterator<Item = Value>) -> String {
    let mut lines = records
        .into_iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    lines.push('\n');
    lines
}

fn claude_conversation_record(session_id: &str, kind: &str, text: &str) -> Value {
    json!({
        "type": kind,
        "sessionId": session_id,
        "message": {"role": kind, "content": [{"type": "text", "text": text}]}
    })
}

fn claude_sidechain_record(session_id: &str, agent_id: &str, kind: &str, text: &str) -> Value {
    json!({
        "type": kind,
        "isSidechain": true,
        "sessionId": session_id,
        "agentId": agent_id,
        "message": {"role": kind, "content": [{"type": "text", "text": text}]}
    })
}

fn claude_tool_record(session_id: &str, agent_id: &str, kind: &str, tool_name: &str) -> Value {
    let content_block = if kind == "assistant" {
        json!({"type": "tool_use", "id": "toolu-1", "name": tool_name, "input": {}})
    } else {
        json!({"type": "tool_result", "tool_use_id": "toolu-1", "content": "tool output"})
    };
    json!({
        "type": kind,
        "isSidechain": true,
        "sessionId": session_id,
        "agentId": agent_id,
        "message": {"role": kind, "content": [content_block]}
    })
}

fn claude_project(home: &Path) -> PathBuf {
    let project = home.join(".claude/projects/-workspace-fixture");
    fs::create_dir_all(&project).unwrap();
    project
}

fn claude_conversation(project: &Path, session_id: &str, prompt: &str, reply: &str) {
    fs::write(
        project.join(format!("{session_id}.jsonl")),
        claude_transcript_lines([
            claude_conversation_record(session_id, "user", prompt),
            claude_conversation_record(session_id, "assistant", reply),
        ]),
    )
    .unwrap();
}

fn claude_delegated(project: &Path, session_id: &str, task_id: &str, records: Vec<Value>) {
    let dir = project.join(session_id).join("subagents");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(format!("{task_id}.jsonl")),
        claude_transcript_lines(records),
    )
    .unwrap();
}

// -- Cursor ----------------------------------------------------------------

fn cursor_chat_meta(home: &Path, conversation_id: &str, cwd: Option<&str>) {
    let chat_dir = home.join(".cursor/chats/ab12cd34").join(conversation_id);
    fs::create_dir_all(&chat_dir).unwrap();
    let mut meta = json!({
        "createdAtMs": now_millis() - 60_000,
        "updatedAtMs": now_millis(),
        "hasConversation": true
    });
    if let Some(cwd) = cwd {
        meta["cwd"] = json!(cwd);
    }
    fs::write(chat_dir.join("meta.json"), meta.to_string()).unwrap();
}

fn cursor_ide_store(home: &Path, conversation_id: &str, title: &str, workspace: &str) {
    let store = home
        .join("Library/Application Support/Cursor/User/workspaceStorage")
        .join("store-a");
    fs::create_dir_all(&store).unwrap();
    let db = store.join("state.vscdb");
    let connection = Connection::open(&db).unwrap();
    connection
        .execute(
            "CREATE TABLE cursorDiskKV (key TEXT NOT NULL, value BLOB NOT NULL)",
            [],
        )
        .unwrap();
    let composer = json!({
        "composerId": conversation_id,
        "name": title,
        "createdAt": now_millis() - 120_000,
        "lastUpdatedAt": now_millis(),
        "fullConversationHeadersOnly": [{"bubbleId": "bubble-1", "type": 1}],
        "workspaceIdentifier": {"uri": {"path": workspace}}
    });
    connection
        .execute(
            "INSERT INTO cursorDiskKV VALUES (?1, ?2)",
            rusqlite::params![
                format!("composerData:{conversation_id}"),
                serde_json::to_vec(&composer).unwrap()
            ],
        )
        .unwrap();
}

fn cursor_project(home: &Path, project_label: &str) -> PathBuf {
    let project = home.join(".cursor/projects").join(project_label);
    fs::create_dir_all(&project).unwrap();
    project
}

fn cursor_transcript(project: &Path, conversation_id: &str, prompt: &str, reply: &str) {
    let dir = project.join("agent-transcripts").join(conversation_id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join(format!("{conversation_id}.jsonl")),
        [
            json!({"role": "user", "message": {"content": [{"type": "text", "text": prompt}]}}),
            json!({"role": "assistant", "message": {"content": [{"type": "text", "text": reply}]}}),
        ]
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n"),
    )
    .unwrap();
}

fn cursor_delegated(
    project: &Path,
    conversation_id: &str,
    task_id: &str,
    prompt: &str,
    reply: &str,
) {
    let dir = project
        .join("agent-transcripts")
        .join(conversation_id)
        .join("subagents");
    fs::create_dir_all(&dir).unwrap();
    // Cursor records carry no session field at all, so the only information
    // the parser has is the record shape and the layout position.
    fs::write(
        dir.join(format!("{task_id}.jsonl")),
        [
            json!({"role": "user", "message": {"content": [{"type": "text", "text": prompt}]}}),
            json!({"role": "assistant", "message": {"content": [{"type": "text", "text": reply}]}}),
        ]
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n"),
    )
    .unwrap();
}

// -- Kimi Code -------------------------------------------------------------

fn kimi_state(session_dir: &Path, title: &str) {
    fs::create_dir_all(session_dir).unwrap();
    fs::write(
        session_dir.join("state.json"),
        json!({
            "title": title,
            "createdAt": iso_ago(2),
            "updatedAt": iso_ago(1),
            "workDir": "/workspace/kimi"
        })
        .to_string(),
    )
    .unwrap();
}

fn kimi_wire(session_dir: &Path, prompt: &str) {
    let agent_dir = session_dir.join("agents/main");
    fs::create_dir_all(&agent_dir).unwrap();
    fs::write(
        agent_dir.join("wire.jsonl"),
        [
            r#"{"type":"metadata","protocol_version":1}"#.to_string(),
            json!({
                "type": "context.append_message",
                "time": now_millis(),
                "message": {"role": "user", "content": prompt}
            })
            .to_string(),
        ]
        .join("\n"),
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// Invariant: one conversation recorded in several locations is one row.
// ---------------------------------------------------------------------------

#[test]
fn one_conversation_recorded_in_several_locations_is_one_row() {
    // Cursor writes the same conversation into the CLI chat store
    // (metadata-only), the IDE composer store (`state.vscdb`), and the CLI
    // project transcript tree. Dedupe must key on the native identity alone,
    // never identity plus source path; a source-path key leaves the same
    // conversation in two or three rows.
    let home = temp_root("multi-store-one-row");
    let conversation_id = "11111111-1111-4111-8111-111111111111";

    cursor_chat_meta(&home, conversation_id, None);
    cursor_ide_store(
        &home,
        conversation_id,
        "Cursor IDE title",
        "/workspace/from-ide",
    );
    let project = cursor_project(&home, "Users-fixture-LicoUp");
    cursor_transcript(
        &project,
        conversation_id,
        "Multi-store prompt",
        "Multi-store reply",
    );

    let listed = browse(&home, "cursor");
    assert_eq!(listed["page"]["totalSessions"], 1);
    assert_eq!(session_ids(&listed), vec![conversation_id.to_string()]);
    let row = &listed["sessions"][0];
    assert_eq!(
        row["workingDirectory"], "/workspace/from-ide",
        "metadata carried by any recorded copy reaches the kept row"
    );
    assert_eq!(row["title"], "Cursor IDE title");
}

// ---------------------------------------------------------------------------
// Invariant: lineage comes from the store's explicit record, which lives
// outside the transcript.
// ---------------------------------------------------------------------------

#[test]
fn lineage_outside_the_transcript_folds_delegated_work_into_the_parent() {
    // Codex records the parent/child edge only in `thread_spawn_edges`; the
    // child rollout mentions no parent anywhere. Reading only transcripts
    // leaves every delegated task as its own top-level conversation.
    let home = temp_root("codex-lineage-not-in-transcript");
    let parent_id = "019f0000-0000-7000-8000-00000000e001";
    let child_id = "019f0000-0000-7000-8000-00000000e002";
    let parent_rollout = codex_rollout(
        &home,
        parent_id,
        "Plan the migration",
        "Delegating the survey",
    );
    let child_rollout = codex_rollout(
        &home,
        child_id,
        "Survey the adapter modules",
        "Survey complete",
    );
    codex_state_db(
        &home,
        &[
            CodexStateThread {
                id: parent_id,
                rollout_path: &parent_rollout,
                title: "Migration plan",
                cwd: "/workspace/catalog",
                model: "gpt-test",
                nickname: None,
                role: None,
                first_user_message: None,
            },
            CodexStateThread {
                id: child_id,
                rollout_path: &child_rollout,
                title: "Adapter survey",
                cwd: "/workspace/catalog",
                model: "gpt-test",
                nickname: Some("survey-agent"),
                role: Some("general-purpose"),
                first_user_message: Some("Survey the adapter modules"),
            },
        ],
        &[(parent_id, child_id)],
    );

    let listed = browse(&home, "codex");
    assert_eq!(
        session_ids(&listed),
        vec![parent_id.to_string()],
        "a delegated thread must not occupy its own browse row"
    );
    let row = &listed["sessions"][0];
    let cards = subagent_cards(row);
    assert_eq!(cards.len(), 1, "expected exactly one delegated task card");
    assert_eq!(cards[0]["cardTitle"], "Survey the adapter modules");
    assert_eq!(
        cards[0]["cardSubtitle"], "general-purpose",
        "the declared agent role comes from the store record, not the transcript"
    );
    let child_messages = read_lazy_child_messages(&home, "codex", cards[0], child_id, 2);
    assert_eq!(
        child_messages
            .iter()
            .map(|message| message["text"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["Survey the adapter modules", "Survey complete"],
        "expanding the delegated task reads its complete trace"
    );
}

// ---------------------------------------------------------------------------
// Invariant: tolerate third-party schema drift (missing optional columns).
// ---------------------------------------------------------------------------

#[test]
fn a_drifted_schema_still_yields_its_conversations() {
    // A fixed column list against a session table that dropped title,
    // directory, model, and time_created used to make `prepare` fail and
    // degrade into "this agent has no conversations". Missing columns must be
    // projected as NULL and conversations still listed and hydrated.
    let home = temp_root("schema-drift");
    let data_dir = home.join(".local/share/opencode");
    fs::create_dir_all(&data_dir).unwrap();
    openagent_drifted_db(
        &data_dir.join("opencode.db"),
        "ses_drift",
        "Drift-tolerant prompt",
    );

    let listed = browse(&home, "opencode");
    let rows = listed["sessions"].as_array().unwrap();
    assert_eq!(
        rows.len(),
        1,
        "a drifted schema must not yield zero conversations"
    );
    assert_eq!(rows[0]["nativeSessionId"], "ses_drift");
    assert!(
        rows[0]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["text"] == "Drift-tolerant prompt"),
        "the conversation content still hydrates from the message/part tables"
    );
    assert!(rows[0].get("workingDirectory").is_none());
}

// ---------------------------------------------------------------------------
// Invariant: the filesystem root and the home directory are never bindable.
// ---------------------------------------------------------------------------

#[test]
fn root_and_home_directory_records_are_never_bindable_workspaces() {
    // A store that recorded "/" or the user's home as the working directory
    // of a turn must not hand that value to the client. The bounded admission
    // rule rejects both, on the single shared rule the read path applies.
    let home = temp_root("root-home-not-bindable");
    let data_dir = home.join(".local/share/opencode");
    fs::create_dir_all(&data_dir).unwrap();
    let mut dirs = vec![
        ("ses_root", Some("/".to_string())),
        ("ses_relative", Some("leaky/project".to_string())),
        (
            "ses_media",
            Some("/synthetic/Personal.photoslibrary".to_string()),
        ),
        ("ses_ok", Some("/synthetic/projects/beta".to_string())),
    ];
    if let Some(env_home) = env_home_path() {
        dirs.push(("ses_home", Some(env_home.to_string_lossy().to_string())));
        if let Some(parent) = env_home.parent() {
            dirs.push((
                "ses_home_ancestor",
                Some(parent.to_string_lossy().to_string()),
            ));
        }
    }
    let sessions = dirs
        .iter()
        .map(|(id, dir)| (*id, dir.as_deref()))
        .collect::<Vec<(&str, Option<&str>)>>();
    openagent_metadata_db(&data_dir.join("opencode.db"), &sessions);

    let listed = browse(&home, "opencode");
    let listed_rows = listed["sessions"].as_array().unwrap();
    // Every row is still listed; only the unbounded value is stripped.
    assert_eq!(listed_rows.len(), dirs.len());
    for row in listed_rows {
        let id = row["nativeSessionId"].as_str().unwrap();
        match id {
            "ses_root" | "ses_relative" | "ses_media" | "ses_home" | "ses_home_ancestor" => {
                assert!(
                    row.get("workingDirectory").is_none(),
                    "the recorded directory {id} must never be reported as a bindable workspace"
                );
            }
            "ses_ok" => {
                assert_eq!(row["workingDirectory"], "/synthetic/projects/beta");
            }
            _ => unreachable!("unexpected fixture row"),
        }
    }
}

// ---------------------------------------------------------------------------
// Invariant: a stale recorded directory stays provenance, never a bind.
// ---------------------------------------------------------------------------

#[test]
fn a_stale_recorded_directory_is_provenance_only() {
    // Recorded directories go stale in bulk (deleted checkouts, renamed
    // projects). The read layer may still report the recorded value as
    // provenance, but it must never leak to another conversation's row, and
    // the conversation must stay readable.
    let home = temp_root("stale-directory-provenance");
    let data_dir = home.join(".local/share/kilo");
    fs::create_dir_all(&data_dir).unwrap();
    openagent_metadata_db(
        &data_dir.join("kilo.db"),
        &[
            ("ses_stale", Some("/synthetic/missing/renamed-project")),
            ("ses_scoped", Some("/synthetic/existing/project-alpha")),
        ],
    );

    let listed = browse(&home, "kilo-code");
    let rows = listed["sessions"].as_array().unwrap();
    assert_eq!(
        rows.len(),
        2,
        "a stale directory must not drop the conversation"
    );
    let stale = rows
        .iter()
        .find(|row| row["nativeSessionId"] == "ses_stale")
        .unwrap();
    let scoped = rows
        .iter()
        .find(|row| row["nativeSessionId"] == "ses_scoped")
        .unwrap();
    // Each row keeps exactly its own recorded directory: stale provenance is
    // never adopted by another conversation as a historical fallback.
    assert_eq!(
        stale["workingDirectory"],
        "/synthetic/missing/renamed-project"
    );
    assert_eq!(
        scoped["workingDirectory"],
        "/synthetic/existing/project-alpha"
    );
}

// ---------------------------------------------------------------------------
// Invariant: a delegate that claims the parent identity is the child.
// ---------------------------------------------------------------------------

#[test]
fn a_delegate_claiming_the_parent_identity_is_identified_as_the_child() {
    // Cursor CLI transcripts carry no session field at all, so a delegated
    // task's records resolve to the *conversation's* directory identity. Any
    // "nearest identifier-shaped component" heuristic collapses parent and
    // child into one identity. The layout (delegated task directory) must win
    // and name the task as the child of its conversation.
    let home = temp_root("delegate-claims-parent-identity");
    let conversation_id = "11111111-1111-4111-8111-111111111111";
    let task_id = "22222222-2222-4222-8222-222222222222";
    let project = cursor_project(&home, "Users-fixture-LicoUp");
    cursor_transcript(
        &project,
        conversation_id,
        "Audit the parser",
        "Audit finished",
    );
    cursor_delegated(
        &project,
        conversation_id,
        task_id,
        "Map the scan pipeline for the sales dashboard",
        "Pipeline mapping complete",
    );

    let listed = browse(&home, "cursor");
    assert_eq!(
        session_ids(&listed),
        vec![conversation_id.to_string()],
        "the delegate must not claim a top-level row"
    );
    let row = &listed["sessions"][0];
    assert_eq!(row["nativeSessionId"], conversation_id);
    let cards = subagent_cards(row);
    assert_eq!(
        cards.len(),
        1,
        "the delegate must be identified as a child card"
    );
    assert_eq!(
        cards[0]["cardTitle"],
        "Map the scan pipeline for the sales dashboard"
    );
    let child_messages = read_lazy_child_messages(&home, "cursor", cards[0], task_id, 2);
    assert_eq!(
        child_messages
            .iter()
            .map(|message| message["text"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "Map the scan pipeline for the sales dashboard",
            "Pipeline mapping complete"
        ],
        "the card opens the child's identity and complete trace"
    );
    let thread_messages = text_messages(row);
    assert!(
        thread_messages
            .iter()
            .all(|message| message["text"] != "Map the scan pipeline for the sales dashboard"),
        "the child prompt must never be spliced into the parent's own thread"
    );
    assert!(
        thread_messages
            .iter()
            .any(|message| message["text"] == "Audit the parser"),
        "the parent's own messages stay intact"
    );
}

// ---------------------------------------------------------------------------
// Invariant: the richest source wins; metadata carries over from copies.
// ---------------------------------------------------------------------------

#[test]
fn the_richest_source_wins_and_metadata_carries_over_from_discarded_copies() {
    // Cursor records the same conversation in the metadata-only chat store
    // (which knows the project directory) and the CLI project transcript tree
    // (which holds the content). The rich transcript must win the row while
    // the metadata only that copy had is absorbed. Letting recency alone pick
    // the winner keeps the smallest fragment.
    let home = temp_root("rich-source-wins");
    let conversation_id = "11111111-1111-4111-8111-111111111111";
    cursor_chat_meta(&home, conversation_id, Some("/workspace/from-chat-meta"));
    let project = cursor_project(&home, "Users-fixture-LicoUp");
    cursor_transcript(
        &project,
        conversation_id,
        "Rich source prompt",
        "Rich source reply",
    );

    let listed = browse(&home, "cursor");
    let rows = listed["sessions"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row["nativeSessionId"], conversation_id);
    assert_eq!(
        row["sourceKind"], "cursor-cli-projects",
        "the entry with content wins over the metadata-only copy"
    );
    assert!(
        row["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["text"] == "Rich source reply"),
        "the richest source's content reaches the row"
    );
    assert_eq!(
        row["workingDirectory"], "/workspace/from-chat-meta",
        "the metadata the discarded copy knew is carried over"
    );
}

// ---------------------------------------------------------------------------
// Invariant: the catalog walk is complete and iterative.
// ---------------------------------------------------------------------------

#[test]
fn the_catalog_walk_is_complete_and_iterative() {
    // Kimi Code keeps a `state.json` per session directory; the catalog walks
    // the complete session tree iteratively. A session nested beyond the
    // retired depth terminal is still Agent-owned history: it is catalogued
    // with its recorded identity, hydrates its own content, and the scan
    // reports every file and directory entry the full tree actually has.
    let home = temp_root("complete-walk");
    let sessions = home.join(".kimi-code/sessions");
    let shallow = sessions.join("wd-0/proj-a");
    kimi_state(&shallow, "Shallow fixture");
    kimi_wire(&shallow, "Kimi shallow prompt");
    // Eight nested directories: past every retired depth bound, the walk must
    // still descend, catalogue the recorded identity, and stream its content.
    let deep = sessions.join("deep-0/d1/d2/d3/d4/d5/d6/d7");
    kimi_state(&deep, "Deep fixture");
    kimi_wire(&deep, "Kimi deep prompt");

    let listed = browse(&home, "kimi-code");
    let mut ids = session_ids(&listed);
    ids.sort();
    assert_eq!(
        ids,
        vec!["d7".to_string(), "proj-a".to_string()],
        "every session directory in the complete tree is catalogued by its recorded identity"
    );
    let rows = listed["sessions"].as_array().unwrap();
    let shallow_row = rows
        .iter()
        .find(|row| row["nativeSessionId"] == "proj-a")
        .unwrap();
    assert_eq!(shallow_row["title"], "Shallow fixture");
    assert!(
        shallow_row["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["text"] == "Kimi shallow prompt"),
        "the shallow session hydrates its wire content"
    );
    let deep_row = rows
        .iter()
        .find(|row| row["nativeSessionId"] == "d7")
        .unwrap();
    assert_eq!(deep_row["title"], "Deep fixture");
    assert_eq!(deep_row["workingDirectory"], "/workspace/kimi");
    assert!(
        deep_row["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["text"] == "Kimi deep prompt"),
        "a session beyond the retired depth terminal still hydrates its content"
    );
    assert_eq!(
        listed["sources"]["filesSeen"], 2,
        "both session state files are seen"
    );
    let entries_seen = listed["sources"]["directoryEntriesSeen"].as_u64().unwrap();
    assert_eq!(
        entries_seen,
        count_tree_entries(&sessions),
        "the walk covers the complete tree with no depth terminal"
    );
}

/// Independent count of every directory entry under `root`, used to prove the
/// catalog walk observed the complete tree rather than stopping at a bound.
fn count_tree_entries(root: &Path) -> u64 {
    let mut count = 0_u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).unwrap().flatten() {
            count += 1;
            if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                pending.push(entry.path());
            }
        }
    }
    count
}

// ---------------------------------------------------------------------------
// Invariant: folding delegated work never changes the parent's own messages.
// ---------------------------------------------------------------------------

#[test]
fn folding_never_changes_the_parent_own_message_count() {
    // Claude Code writes the child task transcripts with the *conversation's*
    // identity. Folding them in must attach a card, never splice child
    // messages into the parent's thread or drop the parent's own messages.
    // The parent's own conversation count is exactly what its transcript
    // recorded.
    let home = temp_root("fold-preserves-parent-count");
    let session_id = "cd2442dd-a04c-4503-8ce3-1d114047ce63";
    let project = claude_project(&home);
    claude_conversation(
        &project,
        session_id,
        "Deploy the release checklist",
        "Release deployed",
    );
    claude_delegated(
        &project,
        session_id,
        "agent-a7975e289d9a63743",
        vec![
            claude_sidechain_record(
                session_id,
                "agent-a7975e289d9a63743",
                "user",
                "Index the documentation",
            ),
            claude_sidechain_record(
                session_id,
                "agent-a7975e289d9a63743",
                "assistant",
                "Documentation indexed",
            ),
        ],
    );

    let listed = browse(&home, "claude-code");
    assert_eq!(listed["page"]["totalSessions"], 1);
    let row = &listed["sessions"][0];
    let thread = text_messages(row);
    assert_eq!(
        thread.len(),
        2,
        "the parent's own message count must be exactly the transcript's: {:?}",
        thread
    );
    assert_eq!(thread[0]["text"], "Deploy the release checklist");
    assert_eq!(thread[1]["text"], "Release deployed");
    let cards = subagent_cards(row);
    assert_eq!(cards.len(), 1);
    let child_messages =
        read_lazy_child_messages(&home, "claude-code", cards[0], "agent-a7975e289d9a63743", 2);
    assert_eq!(
        child_messages
            .iter()
            .map(|message| message["text"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["Index the documentation", "Documentation indexed"],
        "expanding the card reads the child without changing the parent's thread"
    );
    assert!(
        thread
            .iter()
            .all(|message| message["text"] != "Index the documentation"),
        "folding must not splice child messages into the parent thread"
    );
}

// ---------------------------------------------------------------------------
// Invariant: a delegated task whose whole trace is tool work still appears.
// ---------------------------------------------------------------------------

#[test]
fn a_delegated_task_whose_whole_trace_is_tool_work_still_appears() {
    // An explore/verify subagent often produces nothing but tool steps.
    // The lazy card must survive without embedded prose, and its exact child
    // read must retain the recorded tool invocation and result.
    let home = temp_root("pure-tool-delegate-valid");
    let session_id = "cd2442dd-a04c-4503-8ce3-1d114047ce63";
    let project = claude_project(&home);
    claude_conversation(
        &project,
        session_id,
        "Verify the transport profile",
        "Profile verified",
    );
    claude_delegated(
        &project,
        session_id,
        "agent-tool-task",
        vec![
            claude_tool_record(session_id, "agent-tool-task", "assistant", "Read"),
            claude_tool_record(session_id, "agent-tool-task", "user", "grep"),
        ],
    );

    let listed = browse(&home, "claude-code");
    assert_eq!(listed["page"]["totalSessions"], 1);
    let row = &listed["sessions"][0];
    let cards = subagent_cards(row);
    assert_eq!(
        cards.len(),
        1,
        "a tool-work-only delegated task must still appear as a card"
    );
    assert_eq!(
        cards[0]["subagentToolCallCount"], 2,
        "the card reports the tool steps it recorded"
    );
    let child_messages =
        read_lazy_child_messages(&home, "claude-code", cards[0], "agent-tool-task", 2);
    assert_eq!(child_messages[0]["role"], "tool_call");
    assert_eq!(child_messages[1]["role"], "tool_result");
    assert_eq!(child_messages[1]["text"], "tool output");
}

// ---------------------------------------------------------------------------
// Invariant: schema-less extraction is bounded and falls back; never "none".
// ---------------------------------------------------------------------------

#[test]
fn an_unrecognized_schema_falls_back_to_file_extraction_instead_of_none() {
    // A Codex state database whose schema the client does not recognize must
    // not degrade into "this agent has no conversations": the catalog records
    // the skip reason and the conversation still reaches the list through
    // its rollout file.
    let home = temp_root("schema-fallback");
    let session_id = "019f0000-0000-7000-8000-0000000000e5";
    codex_rollout(&home, session_id, "Fallback prompt", "Fallback reply");
    codex_state_db_unrecognized(&home);

    let listed = browse(&home, "codex");
    assert_eq!(session_ids(&listed), vec![session_id.to_string()]);
    assert!(
        listed["sources"]["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .any(|skip| skip["reason"] == "codex_state_schema_unrecognized"),
        "the rejected database is reported, not silently swallowed"
    );
    assert!(
        listed["sessions"][0]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["text"] == "Fallback reply"),
        "the conversation still hydrates from the rollout file"
    );
}

// ---------------------------------------------------------------------------
// Invariant: the exact single-conversation read is narrowed, and the
// narrowing includes delegated files.
// ---------------------------------------------------------------------------

#[test]
fn the_exact_read_is_narrowed_and_includes_delegated_files() {
    // Opening one conversation must not parse every conversation of every
    // project. The exact read narrows discovery to the requested identity and
    // still pulls the delegated transcripts of that conversation into scope.
    let home = temp_root("exact-read-narrowed");
    let wanted = "11111111-1111-4111-8111-111111111111";
    let other = "33333333-3333-4333-8333-333333333333";
    let project = cursor_project(&home, "Users-fixture-LicoUp");
    cursor_transcript(&project, wanted, "Wanted prompt", "Wanted reply");
    cursor_delegated(
        &project,
        wanted,
        "22222222-2222-4222-8222-222222222222",
        "Map the scan pipeline for the sales dashboard",
        "Pipeline mapping complete",
    );
    let other_project = cursor_project(&home, "Users-fixture-Other");
    cursor_transcript(&other_project, other, "Other prompt", "Other reply");

    let browse_listing = browse(&home, "cursor");
    let browse_files = browse_listing["sources"]["filesSeen"].as_u64().unwrap();
    assert_eq!(browse_listing["page"]["totalSessions"], 2);

    let exact = exact_read(&home, "cursor", wanted);
    let exact_files = exact["sources"]["filesSeen"].as_u64().unwrap();
    assert!(
        exact_files < browse_files,
        "the exact read must not scan the whole store: {exact_files} vs {browse_files}"
    );
    assert_eq!(exact["page"]["totalSessions"], 1);
    let row = &exact["sessions"][0];
    assert_eq!(row["nativeSessionId"], wanted);
    let cards = subagent_cards(row);
    assert_eq!(
        cards.len(),
        1,
        "delegated files of the requested conversation stay in scope"
    );
    assert_eq!(
        cards[0]["cardTitle"],
        "Map the scan pipeline for the sales dashboard"
    );
    assert!(
        exact["sources"]["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .any(|skip| skip["reason"] == "exact_session_miss"),
        "unrelated conversation directories are pruned, not parsed"
    );
}

// ---------------------------------------------------------------------------
// Invariant: the projection cache only accelerates; it never changes
// semantics.
// ---------------------------------------------------------------------------

#[test]
fn the_projection_cache_only_accelerates_never_changes_semantics() {
    let home = temp_root("cache-semantics");
    let session_id = "019f0000-0000-7000-8000-0000000000e1";
    let rollout = codex_rollout(&home, session_id, "Cache prompt", "Cache reply");
    codex_state_db(
        &home,
        &[CodexStateThread {
            id: session_id,
            rollout_path: &rollout,
            title: "Cache thread",
            cwd: "/workspace/catalog",
            model: "gpt-test",
            nickname: None,
            role: None,
            first_user_message: None,
        }],
        &[],
    );
    let cache_root = temp_root("cache-root");
    let params = || {
        json!({
            "agent": "codex",
            "homeDir": home.to_string_lossy().to_string(),
            "limit": 20,
            "historyProjectionCacheRoot": cache_root.to_string_lossy().to_string()
        })
    };

    let first = conversation_list(&params()).unwrap();
    let second = conversation_list(&params()).unwrap();
    assert_eq!(
        first["sessions"], second["sessions"],
        "a warm cache serves the same page, byte for byte"
    );
    assert_eq!(first["page"], second["page"]);
    assert!(
        cache_root.join("history-projections.json").is_file(),
        "the cache is written beneath the requested root"
    );

    // Modified sources must miss and re-parse: a cache that freezes old
    // content changes semantics.
    let updated = format!(
        "{}\n{}\n",
        json!({
            "timestamp": "2026-08-01T00:00:00Z",
            "type": "session_meta",
            "payload": {"id": session_id, "cwd": "/workspace/catalog"}
        }),
        json!({
            "timestamp": "2026-08-01T00:00:01Z",
            "type": "response_item",
            "payload": {"type": "message", "role": "user",
                        "content": [{"type": "input_text", "text": "Cache prompt"}]}
        })
    );
    fs::write(&rollout, updated).unwrap();
    let modified = SystemTime::now() + Duration::from_secs(10);
    let file = fs::File::open(&rollout).unwrap();
    file.set_modified(modified).unwrap();
    drop(file);

    let third = conversation_list(&params()).unwrap();
    let third_messages = third["sessions"][0]["messages"].as_array().unwrap();
    assert!(
        !third_messages
            .iter()
            .any(|message| message["text"] == "Cache reply"),
        "changed sources are re-parsed, never served from a stale cache"
    );
    assert_ne!(
        third["sessions"], first["sessions"],
        "a cache must never freeze old content after the source changed"
    );
}
