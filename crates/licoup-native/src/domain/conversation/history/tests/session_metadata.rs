use super::test_support::*;

#[test]
fn source_revision_tracks_wal_only_changes_without_exposing_paths() {
    let dir = temp_dir("source-revision-wal");
    let path = dir.join("source.sqlite");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL;
         PRAGMA wal_autocheckpoint=0;
         CREATE TABLE retained (body TEXT);
         INSERT INTO retained VALUES ('before');
         PRAGMA wal_checkpoint(TRUNCATE);",
        )
        .unwrap();
    let before_metadata = fs::metadata(&path).unwrap();
    let project = |metadata: &fs::Metadata| {
        session_from_messages(
            HistoryAdapter::Codex,
            &path,
            metadata,
            "synthetic",
            "one".into(),
            vec![],
        )
    };
    let before = project(&before_metadata);
    connection
        .execute("UPDATE retained SET body='after!'", [])
        .unwrap();
    let after_metadata = fs::metadata(&path).unwrap();
    assert_eq!(before_metadata.len(), after_metadata.len());
    assert_eq!(
        before_metadata.modified().unwrap(),
        after_metadata.modified().unwrap()
    );
    let after = project(&after_metadata);
    assert_eq!(before["updatedAt"], after["updatedAt"]);
    assert_ne!(before["sourceRevision"], after["sourceRevision"]);
    assert!(
        after["sourceRevision"]
            .as_str()
            .unwrap()
            .chars()
            .all(|ch| ch.is_ascii_digit() || matches!(ch, ':' | ';'))
    );
}

#[test]
fn codex_adapter_skips_local_command_caveats_for_titles() {
    let dir = temp_dir("codex-readable-title");
    let sessions = dir.join("sessions");
    fs::create_dir_all(&sessions).unwrap();
    let rollout =
        sessions.join("rollout-2026-06-03T18-53-32-019e8d1d-fb25-7d82-b849-80a87fbe407d.jsonl");
    fs::write(
        &rollout,
        [
            r#"{"timestamp":"2026-06-03T10:53:36.044Z","type":"session_meta","payload":{"id":"019e8d1d-fb25-7d82-b849-80a87fbe407d","cwd":"/workspace/projects/pact"}}"#,
            r#"{"timestamp":"2026-06-03T10:53:43.745Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<local-command-caveat>Caveat: generated command context. DO NOT respond to these messages.</local-command-caveat>"}]}}"#,
            r#"{"timestamp":"2026-06-03T10:53:44.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Explain readable Codex history titles"}]}}"#,
            r#"{"timestamp":"2026-06-03T10:53:50.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Readable title answer"}]}}"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "codex",
        "root": dir.to_string_lossy()
    }))
    .unwrap();

    let session = &listed["sessions"].as_array().unwrap()[0];
    assert_eq!(session["title"], "Explain readable Codex history titles");
    let messages = session["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| {
        message["text"] == "Explain readable Codex history titles" && message["role"] == "user"
    }));
    assert!(!messages.iter().any(|message| {
        message["text"]
            .as_str()
            .unwrap_or_default()
            .contains("<local-command-caveat>")
    }));
}

#[test]
fn codex_session_index_thread_name_wins_over_message_noise() {
    let dir = temp_dir("codex-session-index-title");
    let sessions = dir.join("sessions");
    fs::create_dir_all(&sessions).unwrap();
    let rollout =
        sessions.join("rollout-2026-07-12T00-00-00-019e8d1d-fb25-7d82-b849-80a87fbe407d.jsonl");
    fs::write(
        &rollout,
        [
            r#"{"timestamp":"2026-07-12T00:00:00.000Z","type":"session_meta","payload":{"id":"019e8d1d-fb25-7d82-b849-80a87fbe407d","cwd":"/workspace/projects/lico"}}"#,
            r#"{"timestamp":"2026-07-12T00:00:01.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<recommended_plugins> Here is a list of plugins that are available...</recommended_plugins>"}]}}"#,
            r#"{"timestamp":"2026-07-12T00:00:02.000Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Check the release base"}]}}"#,
            r#"{"timestamp":"2026-07-12T00:00:03.000Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok"}]}}"#,
        ]
        .join("\n"),
    )
    .unwrap();
    fs::write(
        dir.join("session_index.jsonl"),
        r#"{"id":"019e8d1d-fb25-7d82-b849-80a87fbe407d","thread_name":"检查发布基座","updated_at":"2026-07-12T00:04:00.000Z"}
"#,
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "codex",
        "root": dir.to_string_lossy()
    }))
    .unwrap();

    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(
        sessions.len(),
        1,
        "listed={}",
        serde_json::to_string_pretty(&listed).unwrap()
    );
    assert_eq!(sessions[0]["title"], "检查发布基座");
    assert_eq!(
        sessions[0]["nativeSessionId"],
        "019e8d1d-fb25-7d82-b849-80a87fbe407d"
    );
}

#[test]
fn native_history_ignores_command_tags_and_status_titles() {
    let dir = temp_dir("native-title-noise");
    fs::write(
        dir.join("project.json"),
        r#"{
          "title": "Updated 1 path from the index",
          "sessions": [
            {
              "sessionId": "clear-command",
              "messages": [
                {"role": "user", "content": "<command-name>/clear</command-name><command-message>The conversation has been cleared.</command-message>"},
                {"role": "assistant", "content": "The conversation has been cleared. What would you like to do next?"}
              ]
            },
            {
              "sessionId": "real-request",
              "title": "Updated 1 path from the index",
              "messages": [
                {"role": "user", "content": "Fix readable conversation titles"},
                {"role": "assistant", "content": "Readable title answer"}
              ]
            }
          ]
        }"#,
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "claude-code",
        "root": dir.to_string_lossy()
    }))
    .unwrap();

    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["nativeSessionId"], "real-request");
    assert_eq!(sessions[0]["title"], "Fix readable conversation titles");
    assert!(
        !sessions[0]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| {
                message["text"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("<command-name>")
            })
    );
}

#[test]
fn generated_notifications_are_collapsed_metadata_and_never_session_titles() {
    let dir = temp_dir("native-generated-metadata");
    fs::write(
        dir.join("metadata-session.json"),
        r#"{
          "sessions": [{
            "sessionId": "metadata-session",
            "messages": [
              {"role": "user", "content": "<task-notification><status>failed</status><summary>Synthetic task failed</summary></task-notification>"},
              {"role": "user", "content": "Keep this real request"},
              {"role": "assistant", "content": "Done"}
            ]
          }]
        }"#,
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "claude-code",
        "root": dir.to_string_lossy()
    }))
    .unwrap();
    let exact = conversation_list(&json!({
        "agent": "claude-code",
        "root": dir.to_string_lossy(),
        "sessionIds": ["metadata-session"]
    }))
    .unwrap();

    for result in [&listed, &exact] {
        let session = &result["sessions"][0];
        assert_eq!(session["title"], "Keep this real request");
        let messages = session["messages"].as_array().unwrap();
        assert_eq!(
            messages
                .iter()
                .filter(|message| message["role"] == "user")
                .count(),
            1
        );
        let metadata = messages
            .iter()
            .find(|message| message["role"] == "metadata")
            .unwrap();
        assert_eq!(metadata["layer"], "execution");
        assert_eq!(metadata["cardType"], "metadata");
        assert_eq!(metadata["collapsed"], true);
    }
}

#[test]
fn vscode_hosted_copilot_files_keep_copilot_as_source_client() {
    let dir = temp_dir("vscode-hosted-copilot");
    let transcript_dir = dir.join(
        "Library/Application Support/Code/User/workspaceStorage/ws/GitHub.copilot-chat/transcripts",
    );
    fs::create_dir_all(&transcript_dir).unwrap();
    fs::write(
        transcript_dir.join("copilot-session.jsonl"),
        r#"{"sessionId":"copilot-session","role":"user","content":"Ask Copilot about Pact"}"#,
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "code",
        "root": dir.to_string_lossy()
    }))
    .unwrap();

    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["adapterId"], "code");
    assert_eq!(sessions[0]["sourceTool"], "copilot");
    assert_eq!(sessions[0]["sourceClient"], "copilot");
    assert_eq!(sessions[0]["hostApp"], "vscode");
    assert_eq!(sessions[0]["sourceLabel"], "vscode: copilot");
}

#[test]
fn cli_provenance_labels_do_not_claim_desktop_or_ide_ownership() {
    let cursor_source = super::super::session_metadata::source_client_for_session(
        HistoryAdapter::Cursor,
        std::path::Path::new(".cursor/projects/project/agent-transcripts/session/store.db"),
        "cursor-cli-projects",
        &[],
    );
    assert_eq!(cursor_source, "cursor-agent");
    assert_eq!(
        super::super::session_metadata::source_label("cursor", &cursor_source),
        "cursor: cursor agent cli"
    );
    assert_eq!(HistoryAdapter::Codex.label(), "Codex CLI");
}

#[test]
fn openclaw_gateway_session_key_is_the_native_continuity_id() {
    let dir = temp_dir("openclaw-session-key");
    fs::write(
        dir.join("session.json"),
        r#"{
          "sessions": [{
            "sessionKey": "agent:main:fixture-thread",
            "messages": [
              {"role": "user", "text": "OpenClaw native prompt"},
              {"role": "assistant", "text": "OpenClaw native answer"}
            ]
          }]
        }"#,
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "openclaw",
        "root": dir.to_string_lossy()
    }))
    .unwrap();

    assert_eq!(
        listed["sessions"][0]["nativeSessionId"],
        "agent:main:fixture-thread"
    );
}
