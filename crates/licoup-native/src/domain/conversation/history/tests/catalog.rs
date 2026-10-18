use super::test_support::*;

fn now_epoch_seconds() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

fn now_epoch_millis() -> i64 {
    now_epoch_seconds() * 1_000
}

fn iso_days_ago(days: i64) -> String {
    (OffsetDateTime::now_utc() - time::Duration::days(days))
        .format(&Rfc3339)
        .unwrap()
}

fn codex_rollout_fixture(session_id: &str, prompt: &str, reply: &str) -> String {
    [
        format!(
            r#"{{"timestamp":"2026-08-01T00:00:00Z","type":"session_meta","payload":{{"id":"{session_id}","cwd":"/workspace/catalog"}}}}"#
        ),
        format!(
            r#"{{"timestamp":"2026-08-01T00:00:01Z","type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"{prompt}"}}]}}}}"#
        ),
        format!(
            r#"{{"timestamp":"2026-08-01T00:00:02Z","type":"response_item","payload":{{"type":"message","role":"assistant","content":[{{"type":"output_text","text":"{reply}"}}]}}}}"#
        ),
    ]
    .join("\n")
}

fn create_codex_state_db(path: &Path, threads: &[(&str, &str, i64, i64, &str, i64)]) {
    let connection = Connection::open(path).unwrap();
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
                model TEXT
            )",
            [],
        )
        .unwrap();
    for (id, rollout_path, created_at, updated_at, title, archived) in threads {
        connection
            .execute(
                "INSERT INTO threads VALUES (?1, ?2, ?3, ?4, ?5, ?6, '/workspace/catalog', 'gpt-test')",
                (id, rollout_path, created_at, updated_at, title, archived),
            )
            .unwrap();
    }
}

fn session_ids(listed: &Value) -> Vec<String> {
    listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|session| session["nativeSessionId"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn codex_catalog_uses_state_threads_and_supplements_fresh_rollouts() {
    let home = temp_dir("codex-catalog-tiered");
    let sessions_dir = home.join(".codex/sessions/2026/08/01");
    fs::create_dir_all(&sessions_dir).unwrap();
    let recent_id = "019f0000-0000-7000-8000-0000000000a1";
    let fresh_id = "019f0000-0000-7000-8000-0000000000b2";
    let recent_rollout =
        sessions_dir.join(format!("rollout-2026-08-01T00-00-00-{recent_id}.jsonl"));
    fs::write(
        &recent_rollout,
        codex_rollout_fixture(recent_id, "Recent prompt", "Recent reply"),
    )
    .unwrap();
    fs::write(
        sessions_dir.join(format!("rollout-2026-08-01T00-00-01-{fresh_id}.jsonl")),
        codex_rollout_fixture(fresh_id, "Fresh prompt", "Fresh reply"),
    )
    .unwrap();
    let now = now_epoch_seconds();
    create_codex_state_db(
        &home.join(".codex/state_5.sqlite"),
        &[
            (
                recent_id,
                &*recent_rollout.to_string_lossy(),
                now - 10,
                now,
                "Recent thread",
                0,
            ),
            // Archived threads never enter the browse catalog.
            (
                "019f0000-0000-7000-8000-0000000000c3",
                "/missing/archived.jsonl",
                now - 10,
                now,
                "Archived thread",
                1,
            ),
            // Older Agent-owned threads remain pageable.
            (
                "019f0000-0000-7000-8000-0000000000d4",
                "/missing/old.jsonl",
                now - 41 * 86_400,
                now - 40 * 86_400,
                "Old thread",
                0,
            ),
        ],
    );

    let listed = conversation_list(&json!({
        "agent": "codex",
        "homeDir": display_path(&home),
        "limit": 20
    }))
    .unwrap();

    let ids = session_ids(&listed);
    assert_eq!(ids.len(), 3);
    assert!(ids.contains(&recent_id.to_string()));
    assert!(ids.contains(&fresh_id.to_string()));
    assert!(ids.contains(&"019f0000-0000-7000-8000-0000000000d4".to_string()));
    assert_eq!(listed["page"]["totalSessions"], 3);
    let sessions = listed["sessions"].as_array().unwrap();
    let recent = sessions
        .iter()
        .find(|session| session["nativeSessionId"] == recent_id)
        .unwrap();
    assert_eq!(recent["title"], "Recent thread");
    assert_eq!(recent["workingDirectory"], "/workspace/catalog");
    // Page sessions are hydrated from their rollout content.
    let messages = recent["messages"].as_array().unwrap();
    assert!(
        messages
            .iter()
            .any(|message| message["text"] == "Recent reply")
    );
    let fresh = sessions
        .iter()
        .find(|session| session["nativeSessionId"] == fresh_id)
        .unwrap();
    assert!(
        fresh["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["text"] == "Fresh prompt")
    );
}

#[test]
fn codex_catalog_rejects_unrecognized_state_schema_and_falls_back_to_rollouts() {
    let home = temp_dir("codex-catalog-fail-closed");
    let sessions_dir = home.join(".codex/sessions/2026/08/01");
    fs::create_dir_all(&sessions_dir).unwrap();
    let session_id = "019f0000-0000-7000-8000-0000000000e5";
    fs::write(
        sessions_dir.join(format!("rollout-2026-08-01T00-00-00-{session_id}.jsonl")),
        codex_rollout_fixture(session_id, "Fallback prompt", "Fallback reply"),
    )
    .unwrap();
    let connection = Connection::open(home.join(".codex/state_5.sqlite")).unwrap();
    connection
        .execute("CREATE TABLE threads (id TEXT PRIMARY KEY, note TEXT)", [])
        .unwrap();
    drop(connection);

    let listed = conversation_list(&json!({
        "agent": "codex",
        "homeDir": display_path(&home),
        "limit": 20
    }))
    .unwrap();

    assert_eq!(session_ids(&listed), vec![session_id.to_string()]);
    assert!(
        listed["sources"]["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .any(|skip| skip["reason"] == "codex_state_schema_unrecognized")
    );
    assert!(
        listed["sessions"][0]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["text"] == "Fallback reply")
    );
}

#[test]
fn codex_catalog_accepts_state_threads_without_optional_display_columns() {
    let home = temp_dir("codex-catalog-narrow-state-schema");
    let sessions_dir = home.join(".codex/sessions/2026/08/01");
    fs::create_dir_all(&sessions_dir).unwrap();
    let session_id = "019f0000-0000-7000-8000-0000000000f7";
    let rollout = sessions_dir.join(format!("rollout-2026-08-01T00-00-00-{session_id}.jsonl"));
    fs::write(
        &rollout,
        codex_rollout_fixture(session_id, "Prompt", "Reply"),
    )
    .unwrap();

    let now = now_epoch_seconds();
    let connection = Connection::open(home.join(".codex/state_5.sqlite")).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY,
                rollout_path TEXT,
                created_at INTEGER,
                updated_at INTEGER
            )",
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO threads(id, rollout_path, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4)",
            (session_id, rollout.to_string_lossy().as_ref(), now - 1, now),
        )
        .unwrap();
    drop(connection);

    let listed = conversation_list(&json!({
        "agent": "codex",
        "homeDir": display_path(&home),
        "limit": 20
    }))
    .unwrap();

    assert_eq!(session_ids(&listed), vec![session_id.to_string()]);
    assert_eq!(listed["sessions"][0]["model"], Value::Null);
    assert!(
        listed["sessions"][0]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["text"] == "Reply")
    );
}

#[test]
fn codex_catalog_leaves_archived_rollouts_out_of_browse_lists() {
    let home = temp_dir("codex-catalog-archived-excluded");
    let archived_dir = home.join(".codex/archived_sessions");
    fs::create_dir_all(&archived_dir).unwrap();
    let archived_id = "019f0000-0000-7000-8000-0000000000f6";
    fs::write(
        archived_dir.join(format!("rollout-2026-08-01T00-00-00-{archived_id}.jsonl")),
        codex_rollout_fixture(archived_id, "Archived prompt", "Archived reply"),
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "codex",
        "homeDir": display_path(&home),
        "limit": 20
    }))
    .unwrap();

    assert_eq!(listed["page"]["totalSessions"], 0);
    assert!(listed["sessions"].as_array().unwrap().is_empty());
}

#[test]
fn browse_catalog_omits_lico_archived_native_sessions_but_keeps_exact_reads() {
    let home = temp_dir("catalog-lico-archived-sessions");
    let sessions_dir = home.as_ref().join(".codex/sessions/2026/08/01");
    fs::create_dir_all(&sessions_dir).unwrap();
    let archived_id = "019f0000-0000-7000-8000-0000000000aa";
    let kept_id = "019f0000-0000-7000-8000-0000000000bb";
    fs::write(
        sessions_dir.join(format!("rollout-2026-08-01T00-00-00-{archived_id}.jsonl")),
        codex_rollout_fixture(archived_id, "Archived prompt", "Archived reply"),
    )
    .unwrap();
    fs::write(
        sessions_dir.join(format!("rollout-2026-08-01T00-01-00-{kept_id}.jsonl")),
        codex_rollout_fixture(kept_id, "Kept prompt", "Kept reply"),
    )
    .unwrap();

    let store_root = home.as_ref().join("portable");
    fs::create_dir_all(&store_root).unwrap();
    struct Override(Option<PathBuf>);
    impl Drop for Override {
        fn drop(&mut self) {
            licoup_foundation::platform::paths::set_portable_data_dir_override(self.0.take());
        }
    }
    let _guard = Override(
        licoup_foundation::platform::paths::set_portable_data_dir_override(Some(
            store_root.clone(),
        )),
    );
    {
        use licoup_conversation::{ConversationStore, MembershipAccess, Principal, PrincipalKind};
        let store = ConversationStore::open(&store_root).unwrap();
        let group = store
            .create_conversation_with_members(
                "Group",
                Principal {
                    id: "human:synthetic".into(),
                    kind: PrincipalKind::Human,
                    display_name: "Human".into(),
                    agent_id: None,
                    created_at_unix_ms: 1,
                },
                &[(
                    Principal {
                        id: "agent:codex".into(),
                        kind: PrincipalKind::Agent,
                        display_name: "Codex".into(),
                        agent_id: Some("codex".into()),
                        created_at_unix_ms: 1,
                    },
                    MembershipAccess::Member,
                )],
            )
            .unwrap();
        let membership = group
            .memberships
            .iter()
            .find(|membership| membership.principal.kind == PrincipalKind::Agent)
            .unwrap();
        let scope = store
            .prepare_runtime_dispatch(
                "codex",
                archived_id,
                "Synthetic request",
                Some(&group.id),
                Some(&membership.id),
                None,
                None,
            )
            .unwrap();
        store
            .bind_runtime_session(&scope, "codex", archived_id, None, None)
            .unwrap();
        store
            .archive_conversation_tree(&group.id, true, false)
            .unwrap();
        store.checkpoint().unwrap();
    }

    let listed = conversation_list(&json!({
        "agent": "codex",
        "homeDir": display_path(home.as_ref()),
        "limit": 20
    }))
    .unwrap();
    let ids: Vec<&str> = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|session| session["nativeSessionId"].as_str())
        .collect();
    assert_eq!(ids, [kept_id]);

    // Archive is a browse-list visibility decision: an exact read of the same
    // session still resolves.
    let exact = conversation_list(&json!({
        "agent": "codex",
        "homeDir": display_path(home.as_ref()),
        "sessionId": archived_id
    }))
    .unwrap();
    assert!(
        exact["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|session| session["nativeSessionId"].as_str() == Some(archived_id))
    );
}

#[test]
fn codex_search_still_scans_archived_rollouts_without_a_window() {
    let home = temp_dir("codex-catalog-search-full-scan");
    let archived_dir = home.join(".codex/archived_sessions");
    fs::create_dir_all(&archived_dir).unwrap();
    let archived_id = "019f0000-0000-7000-8000-0000000000a7";
    fs::write(
        archived_dir.join(format!("rollout-2026-08-01T00-00-00-{archived_id}.jsonl")),
        codex_rollout_fixture(
            archived_id,
            "needleword archived prompt",
            "needleword reply",
        ),
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "codex",
        "homeDir": display_path(&home),
        "matchTerm": "needleword",
        "limit": 20
    }))
    .unwrap();

    assert!(
        !listed["sessions"].as_array().unwrap().is_empty(),
        "explicit search must cover archived history without the recency window"
    );
}

#[test]
fn codex_catalog_paginates_and_hydrates_only_the_returned_page() {
    let home = temp_dir("codex-catalog-paging");
    let sessions_dir = home.join(".codex/sessions/2026/08/01");
    fs::create_dir_all(&sessions_dir).unwrap();
    let first_id = "019f0000-0000-7000-8000-000000000011";
    let second_id = "019f0000-0000-7000-8000-000000000022";
    fs::write(
        sessions_dir.join(format!("rollout-2026-08-01T00-00-00-{first_id}.jsonl")),
        codex_rollout_fixture(first_id, "First prompt", "First reply"),
    )
    .unwrap();
    fs::write(
        sessions_dir.join(format!("rollout-2026-08-01T00-00-01-{second_id}.jsonl")),
        codex_rollout_fixture(second_id, "Second prompt", "Second reply"),
    )
    .unwrap();
    // Pin the recency order so the page assignment is deterministic.
    let now = SystemTime::now();
    fs::OpenOptions::new()
        .write(true)
        .open(sessions_dir.join(format!("rollout-2026-08-01T00-00-00-{first_id}.jsonl")))
        .unwrap()
        .set_modified(now)
        .unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(sessions_dir.join(format!("rollout-2026-08-01T00-00-01-{second_id}.jsonl")))
        .unwrap()
        .set_modified(now - std::time::Duration::from_secs(60))
        .unwrap();

    let page_one = conversation_list(&json!({
        "agent": "codex",
        "homeDir": display_path(&home),
        "limit": 1
    }))
    .unwrap();
    assert_eq!(page_one["page"]["totalSessions"], 2);
    assert_eq!(page_one["page"]["hasMore"], true);
    let sessions = page_one["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["nativeSessionId"], first_id);
    assert!(
        sessions[0]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["text"] == "First reply")
    );

    let page_two = conversation_list(&json!({
        "agent": "codex",
        "homeDir": display_path(&home),
        "limit": 1,
        "offset": 1
    }))
    .unwrap();
    let sessions = page_two["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["nativeSessionId"], second_id);
    assert!(
        sessions[0]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["text"] == "Second reply")
    );
    assert_eq!(page_two["page"]["hasMore"], false);
}

#[test]
fn catalog_pages_include_old_agent_owned_sessions() {
    let home = temp_dir("catalog-old-pages");
    fs::create_dir_all(home.join(".codex/sessions")).unwrap();
    let database = home.join(".codex/state_5.sqlite");
    let connection = Connection::open(&database).unwrap();
    connection
        .execute(
            "CREATE TABLE threads (
                id TEXT PRIMARY KEY, rollout_path TEXT, created_at INTEGER,
                updated_at INTEGER, title TEXT, archived INTEGER, cwd TEXT, model TEXT
            )",
            [],
        )
        .unwrap();
    let now = now_epoch_seconds();
    for index in 0..75 {
        let id = format!("synthetic-old-{index:03}");
        let updated = now - (index as i64 + 40) * 86_400;
        connection
            .execute(
                "INSERT INTO threads VALUES (?1, ?2, ?3, ?3, ?4, 0, '/synthetic/workspace', 'synthetic-model')",
                (
                    &id,
                    format!("/synthetic/missing/{id}.jsonl"),
                    updated,
                    format!("Old session {index}"),
                ),
            )
            .unwrap();
    }
    drop(connection);

    let mut identities = Vec::new();
    for offset in [0, 20, 40, 60] {
        let listed = conversation_list(&json!({
            "agent": "codex",
            "homeDir": display_path(&home),
            "offset": offset,
            "limit": 20
        }))
        .unwrap();
        identities.extend(session_ids(&listed));
        assert_eq!(listed["page"]["totalSessions"], 75);
        assert_eq!(listed["page"]["hasMore"], offset < 60);
    }
    assert_eq!(identities.len(), 75);
    assert_eq!(identities.iter().collect::<BTreeSet<_>>().len(), 75);
}

fn create_openagent_catalog_db(path: &Path) {
    let connection = Connection::open(path).unwrap();
    connection
        .execute(
            "CREATE TABLE session (
                id TEXT PRIMARY KEY,
                parent_id TEXT,
                title TEXT,
                directory TEXT,
                model TEXT,
                time_created INTEGER,
                time_updated INTEGER,
                time_archived INTEGER
            )",
            [],
        )
        .unwrap();
    let now = now_epoch_millis();
    for (id, parent, archived, updated) in [
        ("ses_keep", None, None, Some(now)),
        ("ses_sub", Some("ses_keep"), None, Some(now)),
        ("ses_arch", None, Some(now), Some(now)),
        ("ses_old", None, None, Some(now - 40 * 86_400_000)),
    ] {
        connection
            .execute(
                "INSERT INTO session VALUES (?1, ?2, ?3, '/workspace/openagent', 'gpt-test', ?4, ?4, ?5)",
                (
                    id,
                    parent,
                    format!("Title {id}"),
                    updated,
                    archived,
                ),
            )
            .unwrap();
    }
}

#[test]
fn opencode_catalog_filters_sub_sessions_and_archived_but_keeps_old() {
    let home = temp_dir("opencode-catalog");
    let data_dir = home.join(".local/share/opencode");
    fs::create_dir_all(&data_dir).unwrap();
    create_openagent_catalog_db(&data_dir.join("opencode.db"));

    let listed = conversation_list(&json!({
        "agent": "opencode",
        "homeDir": display_path(&home),
        "limit": 20
    }))
    .unwrap();

    let mut ids = session_ids(&listed);
    ids.sort();
    // Sub-agent sessions stay reachable through their parent's transcript,
    // matching the kilo catalog rule.
    assert_eq!(ids, vec!["ses_keep".to_string(), "ses_old".to_string()]);
    let sessions = listed["sessions"].as_array().unwrap();
    let keep = sessions
        .iter()
        .find(|session| session["nativeSessionId"] == "ses_keep")
        .unwrap();
    assert_eq!(keep["title"], "Title ses_keep");
    assert_eq!(keep["workingDirectory"], "/workspace/openagent");
    assert!(keep["messages"].as_array().unwrap().is_empty());
}

#[test]
fn kilo_catalog_filters_sub_sessions_and_archived_but_keeps_old() {
    let home = temp_dir("kilo-catalog");
    let data_dir = home.join(".local/share/kilo");
    fs::create_dir_all(&data_dir).unwrap();
    create_openagent_catalog_db(&data_dir.join("kilo.db"));

    let listed = conversation_list(&json!({
        "agent": "kilo-code",
        "homeDir": display_path(&home),
        "limit": 20
    }))
    .unwrap();

    let mut ids = session_ids(&listed);
    ids.sort();
    assert_eq!(ids, vec!["ses_keep".to_string(), "ses_old".to_string()]);
}

#[test]
fn openagent_explicit_parent_folds_identically_in_browse_and_exact_reads() {
    let home = temp_dir("openagent-parent-parity");
    let data_dir = home.join(".local/share/opencode");
    fs::create_dir_all(&data_dir).unwrap();
    let database = data_dir.join("opencode.db");
    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE session (
                id TEXT PRIMARY KEY, parent_id TEXT, title TEXT, directory TEXT,
                time_created INTEGER, time_updated INTEGER, time_archived INTEGER
             );
             CREATE TABLE message (
                id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER,
                time_updated INTEGER, data TEXT
             );
             CREATE TABLE part (
                id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT,
                time_created INTEGER, data TEXT
             );",
        )
        .unwrap();
    let now = now_epoch_millis();
    connection
        .execute(
            "INSERT INTO session VALUES ('parent', NULL, 'Parent', '/synthetic/workspace', ?1, ?1, NULL)",
            [now],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO session VALUES ('child', 'parent', 'Child task', '/synthetic/workspace', ?1, ?1, NULL)",
            [now],
        )
        .unwrap();
    for (id, session, created, data) in [
        (
            "parent-user",
            "parent",
            now,
            r#"{"role":"user","text":"Parent prompt"}"#,
        ),
        (
            "parent-agent",
            "parent",
            now + 1,
            r#"{"role":"assistant","text":"Parent reply"}"#,
        ),
        (
            "child-tool",
            "child",
            now + 2,
            r#"{"role":"tool_result","type":"tool_result","result":"Tool-only child result"}"#,
        ),
    ] {
        connection
            .execute(
                "INSERT INTO message VALUES (?1, ?2, ?3, ?3, ?4)",
                (id, session, created, data),
            )
            .unwrap();
    }
    drop(connection);

    let browse = conversation_list(&json!({
        "agent": "opencode",
        "homeDir": display_path(&home),
        "limit": 20
    }))
    .unwrap();
    let exact = conversation_list(&json!({
        "agent": "opencode",
        "homeDir": display_path(&home),
        "sessionId": "parent",
        "messageLimit": 50
    }))
    .unwrap();
    assert_eq!(browse["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(exact["sessions"].as_array().unwrap().len(), 1);
    for session in [&browse["sessions"][0], &exact["sessions"][0]] {
        assert_eq!(session["nativeSessionId"], "parent");
        let cards = session["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "subagent")
            .collect::<Vec<_>>();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0]["childSessionId"], "child");
        assert_eq!(cards[0]["childMessageCount"], 1);
        assert!(cards[0]["messages"].as_array().unwrap().is_empty());
    }
    let child = conversation_list(
        &json!({"agent": "opencode", "homeDir": display_path(&home), "sessionId": "child"}),
    )
    .unwrap();
    assert_eq!(child["sessions"][0]["nativeSessionId"], "child");
    assert!(
        child["sessions"][0]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["text"]
                .as_str()
                .is_some_and(|text| text.contains("Tool-only child result")))
    );
}

#[test]
fn kimi_code_catalog_reads_state_json_and_hydrates_page_from_wire() {
    let home = temp_dir("kimi-code-catalog");
    let session_dir = home.join(".kimi-code/sessions/wd_project/session-a");
    fs::create_dir_all(session_dir.join("agents/main")).unwrap();
    fs::write(
        session_dir.join("state.json"),
        json!({
            "title": "Kimi fixture title",
            "createdAt": iso_days_ago(1),
            "updatedAt": iso_days_ago(0),
            "workDir": "/workspace/kimi"
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        session_dir.join("agents/main/wire.jsonl"),
        [
            r#"{"type":"metadata","protocol_version":1}"#,
            r#"{"type":"context.append_message","time":1780912800000,"message":{"role":"user","content":"Kimi catalog prompt"}}"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "kimi-code",
        "homeDir": display_path(&home),
        "limit": 20
    }))
    .unwrap();

    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["nativeSessionId"], "session-a");
    assert_eq!(sessions[0]["title"], "Kimi fixture title");
    assert_eq!(sessions[0]["workingDirectory"], "/workspace/kimi");
    assert!(
        sessions[0]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["text"] == "Kimi catalog prompt")
    );
}

fn browse_params(home: &Path, cache_root: &Path) -> Value {
    json!({
        "agent": "codex",
        "homeDir": display_path(home),
        "limit": 20,
        "historyProjectionCacheRoot": display_path(cache_root)
    })
}

fn browse_with_counters(
    home: &Path,
    cache_root: &Path,
) -> (Value, super::super::catalog::BrowseWorkCounters) {
    let params = browse_params(home, cache_root);
    let scan_config = HistoryScanConfig::from_params(&params);
    super::super::catalog::conversation_list_from_catalog_inner(
        HistoryAdapter::Codex,
        "codex",
        &params,
        &scan_config,
    )
}

#[test]
fn browse_cache_serves_warm_pages_identically_and_counts_work() {
    let home = temp_dir("browse-cache-warm");
    let sessions_dir = home.join(".codex/sessions/2026/08/01");
    fs::create_dir_all(&sessions_dir).unwrap();
    let id = "019f0000-0000-7000-8000-0000000000e1";
    let rollout = sessions_dir.join(format!("rollout-2026-08-01T00-00-00-{id}.jsonl"));
    fs::write(
        &rollout,
        codex_rollout_fixture(id, "Cache prompt", "Cache reply"),
    )
    .unwrap();
    let now = now_epoch_seconds();
    create_codex_state_db(
        &home.join(".codex/state_5.sqlite"),
        &[(
            id,
            &*rollout.to_string_lossy(),
            now - 10,
            now,
            "Cache thread",
            0,
        )],
    );
    let cache_root = temp_dir("browse-cache-root");

    let (first, cold) = browse_with_counters(&home, &cache_root);
    assert_eq!(cold.cache_misses, 1);
    assert_eq!(cold.cache_hits, 0);
    assert_eq!(cold.cache_entries, 1);
    assert!(cold.cache_bytes > 0);
    assert_eq!(first["sessions"].as_array().unwrap().len(), 1);

    let (second, warm) = browse_with_counters(&home, &cache_root);
    assert_eq!(warm.cache_hits, 1);
    assert_eq!(warm.cache_misses, 0);
    assert_eq!(first["sessions"], second["sessions"]);
    let without_cache = conversation_list(&json!({
        "agent": "codex",
        "homeDir": display_path(&home),
        "limit": 20
    }))
    .unwrap();
    assert_eq!(first["sessions"], without_cache["sessions"]);
    assert!(
        cache_root.join("history-projections.json").is_file(),
        "the cache file is written beneath the requested root"
    );
}

#[test]
fn browse_cache_invalidates_when_the_source_changes() {
    let home = temp_dir("browse-cache-invalidate");
    let sessions_dir = home.join(".codex/sessions/2026/08/01");
    fs::create_dir_all(&sessions_dir).unwrap();
    let id = "019f0000-0000-7000-8000-0000000000e2";
    let rollout = sessions_dir.join(format!("rollout-2026-08-01T00-00-00-{id}.jsonl"));
    fs::write(
        &rollout,
        codex_rollout_fixture(id, "First prompt", "First reply"),
    )
    .unwrap();
    let now = now_epoch_seconds();
    create_codex_state_db(
        &home.join(".codex/state_5.sqlite"),
        &[(
            id,
            &*rollout.to_string_lossy(),
            now - 10,
            now,
            "Invalidate thread",
            0,
        )],
    );
    let cache_root = temp_dir("browse-cache-invalidate-root");

    let (first, _) = browse_with_counters(&home, &cache_root);
    let first_reply = first["sessions"][0]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["text"] == "First reply")
        .map(|message| message["text"].as_str().unwrap().to_string())
        .unwrap();
    assert_eq!(first_reply, "First reply");

    let updated = format!(
        "{}
{}
",
        codex_rollout_fixture(id, "First prompt", "First reply"),
        r#"{"timestamp":"2026-08-01T00:00:03Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Second reply"}]}}"#
    );
    fs::write(&rollout, updated).unwrap();
    let modified = SystemTime::now() + std::time::Duration::from_secs(5);
    let file = fs::File::open(&rollout).unwrap();
    file.set_modified(modified).unwrap();
    drop(file);

    let (second, counters) = browse_with_counters(&home, &cache_root);
    assert_eq!(
        counters.cache_misses, 1,
        "a changed source must miss and re-parse"
    );
    assert_eq!(counters.cache_hits, 0);
    let replies = second["sessions"][0]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["text"].as_str())
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert!(replies.contains(&"Second reply".to_string()));
}

#[test]
fn browse_cache_discards_whole_cache_on_schema_mismatch() {
    let home = temp_dir("browse-cache-schema");
    let sessions_dir = home.join(".codex/sessions/2026/08/01");
    fs::create_dir_all(&sessions_dir).unwrap();
    let id = "019f0000-0000-7000-8000-0000000000e3";
    let rollout = sessions_dir.join(format!("rollout-2026-08-01T00-00-00-{id}.jsonl"));
    fs::write(
        &rollout,
        codex_rollout_fixture(id, "Schema prompt", "Schema reply"),
    )
    .unwrap();
    let now = now_epoch_seconds();
    create_codex_state_db(
        &home.join(".codex/state_5.sqlite"),
        &[(
            id,
            &*rollout.to_string_lossy(),
            now - 10,
            now,
            "Schema thread",
            0,
        )],
    );
    let cache_root = temp_dir("browse-cache-schema-root");
    fs::write(
        cache_root.join("history-projections.json"),
        json!({"schema": "licoup.history-projection-cache/v0", "entries": []}).to_string(),
    )
    .unwrap();

    let (listed, counters) = browse_with_counters(&home, &cache_root);
    assert_eq!(counters.cache_discards, 1);
    assert_eq!(
        listed["sessions"].as_array().unwrap().len(),
        1,
        "a discarded cache still serves the page from sources"
    );
}

#[test]
fn codex_large_rollout_keeps_opening_turn_exact_count_and_stable_ids() {
    let home = temp_dir("codex-complete-browse");
    let sessions_dir = home.join(".codex/sessions/2026/08/01");
    fs::create_dir_all(&sessions_dir).unwrap();
    let id = "019f0000-0000-7000-8000-0000000000e4";
    let rollout = sessions_dir.join(format!("rollout-2026-08-01T00-00-00-{id}.jsonl"));

    let header = format!(
        r#"{{"timestamp":"2026-08-01T00:00:00Z","type":"session_meta","payload":{{"id":"{id}","cwd":"/workspace/catalog"}}}}"#
    );
    let filler_line = r#"{"timestamp":"2026-08-01T00:00:01Z","type":"turn_context","payload":{}}"#;
    let filler = std::iter::repeat_n(filler_line, 20_000)
        .collect::<Vec<_>>()
        .join(
            "
",
        );
    let opening = r#"{"timestamp":"2026-08-01T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Opening prompt"}]}}"#;
    let user = r#"{"timestamp":"2026-08-01T00:00:02Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Final prompt"}]}}"#;
    let assistant = r#"{"timestamp":"2026-08-01T00:00:03Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Final reply"}]}}"#;
    fs::write(
        &rollout,
        format!(
            "{header}
{opening}
{filler}
{user}
{assistant}
"
        ),
    )
    .unwrap();
    assert!(fs::metadata(&rollout).unwrap().len() > 1024 * 1024);
    let now = now_epoch_seconds();
    create_codex_state_db(
        &home.join(".codex/state_5.sqlite"),
        &[(
            id,
            &*rollout.to_string_lossy(),
            now - 10,
            now,
            "Tail thread",
            0,
        )],
    );
    let cache_root = temp_dir("codex-complete-cache-root");

    let (listed, counters) = browse_with_counters(&home, &cache_root);
    assert_eq!(counters.cache_misses, 1);

    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["nativeSessionId"], id);
    assert_eq!(
        sessions[0]["sourceMessageCount"], 3,
        "browse count is exact for the complete logical transcript"
    );
    let messages = sessions[0]["messages"].as_array().unwrap();
    let texts = messages
        .iter()
        .map(|message| message["text"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert!(texts.contains(&"Opening prompt".to_string()));
    assert!(texts.contains(&"Final prompt".to_string()));
    assert!(texts.contains(&"Final reply".to_string()));

    // Browse and exact read serialize the same finalized identities.
    let whole = conversation_list(&json!({
        "agent": "codex",
        "homeDir": display_path(&home),
        "sessionIds": [id]
    }))
    .unwrap();
    let whole_sessions = whole["sessions"].as_array().unwrap();
    assert_eq!(whole_sessions.len(), 1);
    let whole_ids = whole_sessions[0]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| message["id"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    let browse_ids = messages
        .iter()
        .map(|message| message["id"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(browse_ids, whole_ids, "browse and exact ids agree");
    assert_eq!(whole_sessions[0]["sourceMessageCount"], 3);
}

#[test]
fn cursor_catalog_reads_chat_meta_and_skips_empty_chats() {
    let home = temp_dir("cursor-catalog");
    let chat_dir = home.join(".cursor/chats/ab12cd34/chat-one");
    fs::create_dir_all(&chat_dir).unwrap();
    fs::write(
        chat_dir.join("meta.json"),
        json!({
            "createdAtMs": now_epoch_millis() - 1_000,
            "updatedAtMs": now_epoch_millis(),
            "cwd": "/workspace/cursor",
            "hasConversation": true
        })
        .to_string(),
    )
    .unwrap();
    let empty_dir = home.join(".cursor/chats/ab12cd34/chat-empty");
    fs::create_dir_all(&empty_dir).unwrap();
    fs::write(
        empty_dir.join("meta.json"),
        json!({
            "createdAtMs": now_epoch_millis(),
            "updatedAtMs": now_epoch_millis(),
            "hasConversation": false
        })
        .to_string(),
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "cursor",
        "homeDir": display_path(&home),
        "limit": 20
    }))
    .unwrap();

    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["nativeSessionId"], "chat-one");
    assert_eq!(sessions[0]["workingDirectory"], "/workspace/cursor");
}

#[test]
fn cursor_catalog_reads_project_workspace_path_for_agent_transcripts() {
    let home = temp_dir("cursor-project-cwd");
    let project_dir = home.join(".cursor/projects/Users-fixture-LicoUp");
    let transcript_dir = project_dir.join("agent-transcripts/session-one");
    fs::create_dir_all(&transcript_dir).unwrap();
    fs::write(
        project_dir.join(".workspace-trusted"),
        json!({
            "trustedAt": "2026-08-02T02:06:46.038Z",
            "workspacePath": "/workspace/licoup",
            "trustMethod": "cli-flag"
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        transcript_dir.join("session-one.jsonl"),
        concat!(
            r#"{"role":"user","message":{"content":[{"type":"text","text":"project prompt"}]}}"#,
            "\n",
            r#"{"role":"assistant","message":{"content":[{"type":"text","text":"project reply"}]}}"#,
            "\n",
        ),
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "cursor",
        "homeDir": display_path(&home),
        "limit": 20
    }))
    .unwrap();

    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["nativeSessionId"], "session-one");
    assert_eq!(sessions[0]["workingDirectory"], "/workspace/licoup");
    assert_eq!(sessions[0]["sourceKind"], "cursor-cli-projects");
}

#[test]
fn claude_catalog_probes_head_titles_and_keeps_old_sessions() {
    let home = temp_dir("claude-catalog");
    let project_dir = home.join(".claude/projects/-workspace-project");
    fs::create_dir_all(&project_dir).unwrap();
    let recent = project_dir.join("cd2442dd-a04c-4503-8ce3-1d114047ce63.jsonl");
    fs::write(
        &recent,
        [
            r#"{"type":"queue-operation","operation":"enqueue","timestamp":"2026-08-01T00:00:00Z","sessionId":"cd2442dd-a04c-4503-8ce3-1d114047ce63"}"#,
            r#"{"type":"user","timestamp":"2026-08-01T00:00:01Z","sessionId":"cd2442dd-a04c-4503-8ce3-1d114047ce63","message":{"role":"user","content":[{"type":"text","text":"Claude catalog prompt"}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-08-01T00:00:02Z","sessionId":"cd2442dd-a04c-4503-8ce3-1d114047ce63","message":{"role":"assistant","content":[{"type":"text","text":"Claude catalog reply"}]}}"#,
        ]
        .join("\n"),
    )
    .unwrap();
    let stale = project_dir.join("00000000-0000-4000-8000-000000000000.jsonl");
    fs::write(
        &stale,
        r#"{"type":"user","message":{"role":"user","content":"stale prompt"}}"#,
    )
    .unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&stale)
        .unwrap()
        .set_modified(SystemTime::now() - std::time::Duration::from_secs(40 * 86_400))
        .unwrap();

    let listed = conversation_list(&json!({
        "agent": "claude-code",
        "homeDir": display_path(&home),
        "limit": 20
    }))
    .unwrap();

    let ids = session_ids(&listed);
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&"cd2442dd-a04c-4503-8ce3-1d114047ce63".to_string()));
    assert!(ids.contains(&"00000000-0000-4000-8000-000000000000".to_string()));
    let session = listed["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|session| session["nativeSessionId"] == "cd2442dd-a04c-4503-8ce3-1d114047ce63")
        .unwrap();
    assert_eq!(session["title"], "Claude catalog prompt");
    assert!(
        session["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["text"] == "Claude catalog reply")
    );
}

#[test]
fn pi_catalog_reads_the_session_header_line() {
    let home = temp_dir("pi-catalog");
    let session_dir = home.join(".pi/agent/sessions/--workspace-project");
    fs::create_dir_all(&session_dir).unwrap();
    fs::write(
        session_dir.join("2026-08-01T00-00-00-000Z_019f9a4b-afeb-7c91-bb4b-fd4fa934e367.jsonl"),
        [
            r#"{"type":"session","version":3,"id":"019f9a4b-afeb-7c91-bb4b-fd4fa934e367","timestamp":"2026-08-01T00:00:00.000Z","cwd":"/workspace/pi"}"#,
            r#"{"type":"message","timestamp":"2026-08-01T00:00:01.000Z","message":{"role":"user","content":[{"type":"text","text":"Pi catalog prompt"}]}}"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "pi",
        "homeDir": display_path(&home),
        "limit": 20
    }))
    .unwrap();

    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        sessions[0]["nativeSessionId"],
        "019f9a4b-afeb-7c91-bb4b-fd4fa934e367"
    );
    assert_eq!(sessions[0]["workingDirectory"], "/workspace/pi");
}
#[test]
fn claude_code_sidechain_transcripts_fold_into_the_conversation() {
    let home = temp_dir("claude-sidechain-fold");
    let project_dir = home.join(".claude/projects/-workspace-project");
    let session_id = "cd2442dd-a04c-4503-8ce3-1d114047ce63";
    fs::create_dir_all(project_dir.join(session_id).join("subagents")).unwrap();
    fs::write(
        project_dir.join(format!("{session_id}.jsonl")),
        [
            json!({"type":"user","sessionId":session_id,"cwd":"/workspace/project","message":{"role":"user","content":[{"type":"text","text":"Audit the parser"}]}}),
            json!({"type":"assistant","sessionId":session_id,"message":{"role":"assistant","content":[{"type":"text","text":"Audit finished"}]}}),
        ]
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n"),
    )
    .unwrap();
    fs::write(
        project_dir
            .join(session_id)
            .join("subagents")
            .join("agent-a7975e289d9a63743.jsonl"),
        [
            json!({"type":"user","isSidechain":true,"sessionId":session_id,"agentId":"a7975e289d9a63743","message":{"role":"user","content":[{"type":"text","text":"Map the scan pipeline"}]}}),
            json!({"type":"assistant","isSidechain":true,"sessionId":session_id,"message":{"role":"assistant","content":[{"type":"text","text":"Pipeline mapped"}]}}),
        ]
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n"),
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "claude-code",
        "homeDir": display_path(&home),
        "sessionId": session_id
    }))
    .unwrap();

    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(
        sessions.len(),
        1,
        "sidechain must not be its own conversation"
    );
    let cards = sessions[0]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["cardType"] == "subagent")
        .collect::<Vec<_>>();
    assert_eq!(
        cards.len(),
        1,
        "expected one delegated task card: {:?}",
        sessions[0]["messages"]
    );
    assert_eq!(cards[0]["cardTitle"], "Map the scan pipeline");
}

#[test]
fn codex_delegated_threads_fold_into_the_conversation_that_spawned_them() {
    let home = temp_dir("codex-spawn-edges");
    let sessions_dir = home.join(".codex/sessions/2026/08/01");
    fs::create_dir_all(&sessions_dir).unwrap();
    let parent_id = "019f0000-0000-7000-8000-00000000e001";
    let child_id = "019f0000-0000-7000-8000-00000000e002";
    let parent_rollout =
        sessions_dir.join(format!("rollout-2026-08-01T00-00-00-{parent_id}.jsonl"));
    let child_rollout = sessions_dir.join(format!("rollout-2026-08-01T00-00-01-{child_id}.jsonl"));
    fs::write(
        &parent_rollout,
        codex_rollout_fixture(parent_id, "Plan the migration", "Delegating the survey"),
    )
    .unwrap();
    fs::write(
        &child_rollout,
        codex_rollout_fixture(child_id, "Survey the adapter modules", "Survey complete"),
    )
    .unwrap();
    let now = now_epoch_seconds();
    let state_db = home.join(".codex/state_5.sqlite");
    create_codex_state_db(
        &state_db,
        &[
            (
                parent_id,
                &*parent_rollout.to_string_lossy(),
                now - 20,
                now,
                "Migration plan",
                0,
            ),
            (
                child_id,
                &*child_rollout.to_string_lossy(),
                now - 10,
                now - 5,
                "Adapter survey",
                0,
            ),
        ],
    );
    {
        let connection = Connection::open(&state_db).unwrap();
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
        connection
            .execute(
                "INSERT INTO thread_spawn_edges VALUES (?1, ?2, 'closed')",
                (parent_id, child_id),
            )
            .unwrap();
    }

    let listed = conversation_list(&json!({
        "agent": "codex",
        "homeDir": display_path(&home),
        "limit": 20
    }))
    .unwrap();

    assert_eq!(
        session_ids(&listed),
        vec![parent_id.to_string()],
        "a delegated thread must not occupy its own browse row"
    );
    let cards = listed["sessions"][0]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["cardType"] == "subagent")
        .collect::<Vec<_>>();
    assert_eq!(cards.len(), 1, "expected one delegated task card");
    assert_eq!(cards[0]["cardTitle"], "Survey the adapter modules");
    assert_eq!(
        listed["sessions"][0]["workingDirectory"], "/workspace/catalog",
        "the thread record keeps the project directory"
    );
}

#[test]
fn codex_exact_lineage_reaches_every_explicit_child_without_a_total_cap() {
    let home = temp_dir("codex-all-explicit-children");
    fs::create_dir_all(home.join(".codex/sessions")).unwrap();
    let database = home.join(".codex/state_5.sqlite");
    let connection = Connection::open(&database).unwrap();
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
    for index in 0..65 {
        connection
            .execute(
                "INSERT INTO thread_spawn_edges VALUES ('parent-thread', ?1, 'closed')",
                [format!("child-thread-{index:03}")],
            )
            .unwrap();
    }
    drop(connection);

    let children = super::super::catalog::codex_delegated_thread_ids(
        &super::super::catalog::codex_spawn_lineage(&json!({"homeDir": display_path(&home)})),
        &["parent-thread".to_string()],
    );
    assert_eq!(children.len(), 65);
    assert_eq!(children.iter().collect::<BTreeSet<_>>().len(), 65);
}

#[test]
fn codex_rollouts_outside_the_thread_index_still_carry_their_project_directory() {
    let home = temp_dir("codex-rollout-cwd");
    let sessions_dir = home.join(".codex/sessions/2026/08/01");
    fs::create_dir_all(&sessions_dir).unwrap();
    let session_id = "019f0000-0000-7000-8000-00000000f001";
    fs::write(
        sessions_dir.join(format!("rollout-2026-08-01T00-00-00-{session_id}.jsonl")),
        codex_rollout_fixture(session_id, "Unindexed prompt", "Unindexed reply"),
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "codex",
        "homeDir": display_path(&home),
        "limit": 20
    }))
    .unwrap();

    assert_eq!(session_ids(&listed), vec![session_id.to_string()]);
    assert_eq!(
        listed["sessions"][0]["workingDirectory"], "/workspace/catalog",
        "the rollout header is the project directory of an unindexed rollout"
    );
}

#[test]
fn antigravity_catalog_lists_brain_conversations_and_skips_cli_logs() {
    let home = temp_dir("antigravity-catalog");
    let bridge = home.join(".gemini/antigravity");
    let conversation_id = "2e6e527a-4c4d-48ef-a512-59d8fc55e85a";
    let logs = bridge
        .join("brain")
        .join(conversation_id)
        .join(".system_generated/logs");
    fs::create_dir_all(&logs).unwrap();
    fs::write(
        logs.join("transcript.jsonl"),
        [
            r#"{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","created_at":"2026-08-01T00:00:00Z","content":"Audit the transport profile"}"#,
            r#"{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-08-01T00:00:02Z","content":"Audit finished"}"#,
        ]
        .join("\n"),
    )
    .unwrap();
    // Rotating CLI logs and crash reports share the tree and are not conversations.
    let cli_logs = home.join(".gemini/antigravity-cli/log");
    fs::create_dir_all(&cli_logs).unwrap();
    fs::write(cli_logs.join("cli-20260801_000000.log"), "starting cli\n").unwrap();
    fs::write(
        home.join(".gemini/antigravity-cli/cli.log"),
        "cli lifecycle\n",
    )
    .unwrap();

    // Antigravity records the conversation workspace in the trajectory metadata
    // record as a `file://` URI inside an opaque protobuf payload.
    let trajectories = bridge.join("conversations");
    fs::create_dir_all(&trajectories).unwrap();
    let trajectory_db = trajectories.join(format!("{conversation_id}.db"));
    {
        let connection = Connection::open(&trajectory_db).unwrap();
        connection
            .execute(
                "CREATE TABLE trajectory_metadata_blob (id TEXT PRIMARY KEY, data BLOB)",
                [],
            )
            .unwrap();
        let mut payload = vec![0x0a, 0x2c];
        payload.extend_from_slice(b"file:///workspace/antigravity-project");
        payload.extend_from_slice(&[0x12, 0x04]);
        payload.extend_from_slice(b"name");
        connection
            .execute(
                "INSERT INTO trajectory_metadata_blob VALUES ('main', ?1)",
                [payload],
            )
            .unwrap();
    }

    let listed = conversation_list(&json!({
        "agent": "antigravity",
        "homeDir": display_path(&home),
        "limit": 20
    }))
    .unwrap();

    assert_eq!(
        session_ids(&listed),
        vec![conversation_id.to_string()],
        "only brain conversations are conversations"
    );
    assert_eq!(
        listed["sessions"][0]["workingDirectory"], "/workspace/antigravity-project",
        "the trajectory record is the conversation project directory"
    );
    assert!(
        listed["sessions"][0]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["text"]
                .as_str()
                .unwrap_or_default()
                .contains("Audit the transport profile")),
        "the transcript content must reach the browse row"
    );

    // Flutter loads history through `conversations stream`, not `list`.
    let mut stream_output = Vec::<u8>::new();
    crate::domain::conversation::streaming::stream_to_writer(
        &json!({
            "agent": "antigravity",
            "homeDir": display_path(&home),
            "limit": 20
        }),
        &mut stream_output,
    )
    .unwrap();
    let streamed = String::from_utf8(stream_output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|frame| frame["event"] == "session")
        .collect::<Vec<_>>();
    assert_eq!(streamed.len(), 1, "stream must use the catalog browse path");
    assert_eq!(
        streamed[0]["session"]["workingDirectory"], "/workspace/antigravity-project",
        "stream must carry the same project directory as list"
    );
}

#[test]
fn codex_lazy_nested_cards_keep_browse_exact_and_older_pages_complete() {
    let home = temp_dir("codex-lazy-nested-pages");
    let store = home.join(".codex/sessions/2026/08/01");
    fs::create_dir_all(&store).unwrap();
    let parent = "019f0000-0000-7000-8000-00000000a001";
    let child = "019f0000-0000-7000-8000-00000000a002";
    let grandchild = "019f0000-0000-7000-8000-00000000a003";
    let write_rollout = |id: &str, owner: Option<&str>, count: usize, start: i64| {
        let mut header =
            json!({"type": "session_meta", "payload": {"id": id, "cwd": "/synthetic/project"}});
        if let Some(owner) = owner {
            header["payload"]["source"] = json!({"subagent": {"thread_spawn": {"parent_thread_id": owner, "agent_nickname": id}}});
        }
        let mut lines = vec![header.to_string()];
        for index in 0..count {
            let timestamp =
                OffsetDateTime::from_unix_timestamp(1_786_147_200 + start + index as i64)
                    .unwrap()
                    .format(&Rfc3339)
                    .unwrap();
            lines.push(json!({"timestamp": timestamp, "type": "response_item", "payload": {"type": "message", "role": if index == 0 { "user" } else { "assistant" }, "content": [{"type": "output_text", "text": format!("{id}-message-{index}")}]}}).to_string());
        }
        let path = store.join(format!("rollout-2026-08-01T00-00-00-{id}.jsonl"));
        fs::write(&path, lines.join("\n")).unwrap();
        path
    };
    write_rollout(parent, None, 45, 0);
    let child_path = write_rollout(child, Some(parent), 65, 100);
    let grandchild_path = write_rollout(grandchild, Some(child), 2, 200);
    let second = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let set_source_time = |path: &Path, nanos| {
        fs::File::open(path)
            .unwrap()
            .set_times(
                fs::FileTimes::new()
                    .set_modified(UNIX_EPOCH + std::time::Duration::new(second, nanos)),
            )
            .unwrap();
    };
    set_source_time(&child_path, 100_000_000);
    set_source_time(&grandchild_path, 100_000_000);
    let cache = temp_dir("codex-lazy-nested-cache");
    let started = std::time::Instant::now();
    let (browse, cold) = browse_with_counters(&home, &cache);
    let cold_elapsed = started.elapsed();
    let started = std::time::Instant::now();
    let (warm, counters) = browse_with_counters(&home, &cache);
    let warm_elapsed = started.elapsed();
    assert_eq!(browse["page"]["totalSessions"], 1);
    assert_eq!(browse["sessions"], warm["sessions"]);
    assert_eq!(cold.cache_misses, 1);
    assert_eq!(counters.cache_hits, 1);
    let root = &browse["sessions"][0];
    assert_eq!(root["messagePage"]["returned"], 20);
    assert_eq!(root["messagePage"]["total"], 46);
    let card = root["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["childSessionId"] == child)
        .unwrap();
    assert_eq!(card["childMessageCount"], 66);
    assert_eq!(card["subagentDepth"], 2);
    assert!(card["messages"].as_array().unwrap().is_empty());
    assert!(card.get("childMessagePage").is_none());
    let exact = conversation_list(
        &json!({"agent": "codex", "homeDir": display_path(&home), "sessionId": parent}),
    )
    .unwrap();
    assert_eq!(root["messages"], exact["sessions"][0]["messages"]);
    assert_eq!(root["messagePage"], exact["sessions"][0]["messagePage"]);
    for (id, expected_total) in [(parent, 46), (child, 66), (grandchild, 2)] {
        let mut params = json!({"agent": "codex", "homeDir": display_path(&home), "sessionId": id});
        let mut ids = BTreeSet::new();
        loop {
            let page = conversation_list(&params).unwrap();
            let session = &page["sessions"][0];
            assert_eq!(session["nativeSessionId"], id);
            assert_eq!(session["sourceMessageCount"], expected_total);
            if id == child {
                assert_eq!(session["sourceRevision"], card["childSourceRevision"]);
            }
            assert!(session["messages"].as_array().unwrap().len() <= 20);
            for message in session["messages"].as_array().unwrap() {
                assert!(ids.insert(message["id"].as_str().unwrap().to_string()));
                if message["role"] == "subagent" {
                    assert!(message["messages"].as_array().unwrap().is_empty());
                    if id == child {
                        assert_eq!(message["childSessionId"], grandchild);
                    }
                }
            }
            let next = &session["messagePage"]["nextBefore"];
            if next.is_null() {
                break;
            }
            params["messageBefore"] = next.clone();
        }
        assert_eq!(ids.len(), expected_total);
    }
    let mut previous_revision = card["childSourceRevision"].clone();
    // Same byte length, same message count and same display second. A nested
    // source update must also propagate through the collapsed ancestor card.
    for (id, path, index) in [(child, &child_path, 64), (grandchild, &grandchild_path, 1)] {
        let original = fs::read_to_string(path).unwrap();
        let changed = original.replace(
            &format!("{id}-message-{index}"),
            &format!("{id}-revised-{index}"),
        );
        assert_eq!(original.len(), changed.len());
        assert_ne!(original, changed);
        fs::write(path, changed).unwrap();
        set_source_time(path, 200_000_000);
        let (updated, counters) = browse_with_counters(&home, &cache);
        assert_eq!(counters.cache_misses, 1);
        let updated_card = updated["sessions"][0]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["childSessionId"] == child)
            .unwrap();
        assert_eq!(updated_card["id"], card["id"]);
        assert_eq!(updated_card["createdAt"], card["createdAt"]);
        assert_eq!(updated_card["childMessageCount"], 66);
        assert_ne!(updated_card["childSourceRevision"], previous_revision);
        let exact_child = conversation_list(
            &json!({"agent": "codex", "homeDir": display_path(&home), "sessionId": child}),
        )
        .unwrap();
        assert_eq!(
            exact_child["sessions"][0]["sourceRevision"],
            updated_card["childSourceRevision"]
        );
        previous_revision = updated_card["childSourceRevision"].clone();
    }
    write_rollout(child, Some(parent), 70, 100);
    let updated = conversation_list(
        &json!({"agent": "codex", "homeDir": display_path(&home), "sessionId": parent}),
    )
    .unwrap();
    let updated_card = updated["sessions"][0]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["childSessionId"] == child)
        .unwrap();
    assert_eq!(updated_card["id"], card["id"]);
    assert_eq!(updated_card["createdAt"], card["createdAt"]);
    assert_eq!(updated_card["childMessageCount"], 71);
    eprintln!(
        "synthetic lazy history: source_sessions=3 browse_rows=1 root_messages=46 returned=20 cold_micros={} warm_micros={} cache_hits={}",
        cold_elapsed.as_micros(),
        warm_elapsed.as_micros(),
        counters.cache_hits
    );
}

#[test]
fn claude_orphan_tasks_keep_each_native_identity_and_exact_read() {
    let home = temp_dir("claude-orphan-catalog");
    let tasks = home.join(".claude/projects/project/missing-parent/subagents");
    fs::create_dir_all(&tasks).unwrap();
    for id in ["agent-one", "agent-two"] {
        fs::write(tasks.join(format!("{id}.jsonl")), json!({"sessionId": "missing-parent", "type": "user", "message": {"role": "user", "content": format!("Task {id}")}}).to_string()).unwrap();
    }
    let browse = conversation_list(
        &json!({"agent": "claude-code", "homeDir": display_path(&home), "limit": 20}),
    )
    .unwrap();
    assert_eq!(browse["page"]["totalSessions"], 2);
    let ids = session_ids(&browse).into_iter().collect::<BTreeSet<_>>();
    assert_eq!(
        ids,
        BTreeSet::from(["agent-one".to_string(), "agent-two".to_string()])
    );
    for id in ids {
        let exact = conversation_list(
            &json!({"agent": "claude-code", "homeDir": display_path(&home), "sessionId": id}),
        )
        .unwrap();
        assert_eq!(exact["sessions"][0]["nativeSessionId"], id);
        assert_eq!(exact["sessions"][0]["messageCount"], 1);
    }
}

#[test]
fn sqlite_wal_commit_invalidates_browse_projection_without_database_stat_change() {
    let home = temp_dir("openagent-wal-history");
    let data = home.join(".local/share/opencode");
    fs::create_dir_all(&data).unwrap();
    let database = data.join("opencode.db");
    let connection = Connection::open(&database).unwrap();
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .unwrap();
    connection.execute_batch(
        "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, time_updated INTEGER);
         CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
         CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, data TEXT);
         INSERT INTO session VALUES ('wal-session', NULL, 1786147200000);
         INSERT INTO message VALUES ('message', 'wal-session', 1786147200000, '{\"role\":\"user\",\"text\":\"First value\"}');"
    ).unwrap();
    let cache = temp_dir("openagent-wal-cache");
    let params = json!({"agent": "opencode", "homeDir": display_path(&home), "historyProjectionCacheRoot": display_path(&cache), "limit": 20});
    let first = conversation_list(&params).unwrap();
    assert_eq!(first["sessions"][0]["messages"][0]["text"], "First value");
    let before = super::super::projection_cache::SourceFingerprint::from_path(&database).unwrap();
    connection.execute("UPDATE message SET data = '{\"role\":\"user\",\"text\":\"Later value\"}' WHERE id = 'message'", []).unwrap();
    let after = super::super::projection_cache::SourceFingerprint::from_path(&database).unwrap();
    assert_eq!(before, after);
    let second = conversation_list(&params).unwrap();
    assert_eq!(second["sessions"][0]["messages"][0]["text"], "Later value");
}

#[test]
fn openagent_orphans_and_cycles_remain_accessible_under_their_own_identities() {
    for (agent, directory, file) in [
        ("opencode", ".local/share/opencode", "opencode.db"),
        ("kilo-code", ".local/share/kilo", "kilo.db"),
    ] {
        let home = temp_dir("openagent-orphan-cycle");
        let data = home.join(directory);
        fs::create_dir_all(&data).unwrap();
        let connection = Connection::open(data.join(file)).unwrap();
        connection.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, time_updated INTEGER);
             CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
             CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, data TEXT);"
        ).unwrap();
        for (id, parent) in [("orphan", "absent"), ("left", "right"), ("right", "left")] {
            connection
                .execute(
                    "INSERT INTO session VALUES (?1, ?2, 1786147200000)",
                    (id, parent),
                )
                .unwrap();
            connection.execute("INSERT INTO message VALUES (?1, ?1, 1786147200000, '{\"role\":\"tool_result\",\"type\":\"tool_result\",\"result\":\"Synthetic result\"}')", [id]).unwrap();
        }
        drop(connection);
        let browse = conversation_list(
            &json!({"agent": agent, "homeDir": display_path(&home), "limit": 20}),
        )
        .unwrap();
        assert_eq!(browse["page"]["totalSessions"], 3);
        assert_eq!(
            session_ids(&browse).into_iter().collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "orphan".to_string(),
                "left".to_string(),
                "right".to_string()
            ])
        );
        let exact = conversation_list(
            &json!({"agent": agent, "homeDir": display_path(&home), "sessionId": "orphan"}),
        )
        .unwrap();
        assert_eq!(exact["sessions"][0]["nativeSessionId"], "orphan");
        assert_eq!(exact["sessions"][0]["parentSessionId"], "absent");
        assert_eq!(exact["sessions"][0]["messageCount"], 1);
    }
}
