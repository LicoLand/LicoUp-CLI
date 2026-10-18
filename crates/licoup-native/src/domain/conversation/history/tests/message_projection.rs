use super::test_support::*;

#[test]
fn claude_code_adapter_preserves_mixed_text_and_tool_use_blocks() {
    let dir = temp_dir("claude-mixed-content-history");
    let path_canary = ["fixture", "source.rs"].join("/");
    let credential_canary = ["fixture", "credential", "canary"].join("-");
    fs::write(
        dir.join("project.jsonl"),
        [
            json!({
                "sessionId": "claude-session-1",
                "type": "user",
                "message": {
                    "role": "user",
                    "content": [{"type": "text", "text": "Inspect the current implementation"}]
                }
            })
            .to_string(),
            json!({
                "sessionId": "claude-session-1",
                "type": "assistant",
                "message": {
                    "role": "assistant",
                    "content": [
                        {"type": "text", "text": "I will inspect it."},
                        {
                            "type": "tool_use",
                            "id": "toolu_1",
                            "name": "Read",
                            "input": {
                                "file_path": path_canary.clone(),
                                "access_token": credential_canary.clone()
                            }
                        },
                        {"type": "text", "text": "Inspection complete."}
                    ]
                }
            })
            .to_string(),
        ]
        .join("\n"),
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "claude-code",
        "root": dir.to_string_lossy()
    }))
    .unwrap();

    let messages = listed["sessions"][0]["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[1]["text"], "I will inspect it.");
    assert_eq!(messages[2]["role"], "tool_call");
    assert_eq!(messages[2]["cardType"], "tool-call");
    assert_eq!(messages[2]["cardTitle"], "Read");
    assert_eq!(messages[3]["text"], "Inspection complete.");
    let serialized = serde_json::to_string(messages).unwrap();
    assert!(!serialized.contains(&credential_canary));
    assert!(!serialized.contains(&path_canary));
}

#[test]
fn claude_code_adapter_formats_tool_result_details_from_json_content() {
    let dir = temp_dir("claude-tool-result-history");
    fs::write(
        dir.join("project.jsonl"),
        [
            r#"{"sessionId":"claude-session-1","type":"user","message":{"role":"user","content":[{"type":"text","text":"为什么这个对话上下文这么长？"}]}}"#,
            r#"{"sessionId":"claude-session-1","type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"{\n  \"id\": \"client.linux.smoke\",\n  \"owner\": \"client\",\n  \"package\": \"client\",\n  \"requiredServices\": [],\n  \"profiles\": [\"external\"]\n}"}]}}"#,
            r#"{"sessionId":"claude-session-1","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"这是工具返回的配置，不应该作为正文展示。"}]}}"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "claude-code",
        "root": dir.to_string_lossy()
    }))
    .unwrap();

    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["title"], "为什么这个对话上下文这么长？");
    let messages = sessions[0]["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0]["text"], "为什么这个对话上下文这么长？");
    assert_eq!(messages[1]["role"], "tool_result");
    assert_eq!(messages[1]["cardType"], "tool-result");
    assert_eq!(messages[1]["collapsed"], true);
    assert_eq!(
        messages[1]["text"],
        "id: client.linux.smoke\nowner: client\npackage: client\nprofiles: [\"external\"]\nrequiredServices: []"
    );
    assert_eq!(messages[2]["role"], "agent");
}

#[test]
fn native_history_preserves_metadata_error_and_unknown_event_semantics() {
    let dir = temp_dir("native-structured-events");
    let path_canary = ["fixture", "project"].join("/");
    let credential_canary = ["fixture", "credential", "canary"].join("-");
    fs::write(
        dir.join("project.jsonl"),
        [
            json!({
                "sessionId": "structured-session",
                "role": "user",
                "content": "Run the native operation"
            })
            .to_string(),
            json!({
                "sessionId": "structured-session",
                "role": "metadata",
                "content": json!({
                    "cwd": path_canary.clone(),
                    "access_token": credential_canary.clone()
                })
                .to_string()
            })
            .to_string(),
            json!({
                "sessionId": "structured-session",
                "role": "error",
                "content": format!(
                    "Operation failed under {path_canary} with credential={credential_canary}"
                )
            })
            .to_string(),
            json!({
                "sessionId": "structured-session",
                "role": "lifecycle_notice",
                "content": "Native operation entered cleanup."
            })
            .to_string(),
            json!({
                "sessionId": "structured-session",
                "role": "assistant",
                "content": "Cleanup completed."
            })
            .to_string(),
        ]
        .join("\n"),
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "claude-code",
        "root": dir.to_string_lossy()
    }))
    .unwrap();

    let messages = listed["sessions"][0]["messages"].as_array().unwrap();
    let roles = messages
        .iter()
        .map(|message| message["role"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(roles, vec!["user", "metadata", "error", "event", "agent"]);
    assert_eq!(messages[1]["role"], "metadata");
    assert_eq!(messages[1]["cardType"], "metadata");
    assert_eq!(messages[1]["collapsed"], true);
    assert_eq!(
        messages[1]["text"],
        "access_token: [redacted]\ncwd: [local path hidden]"
    );
    assert_eq!(messages[2]["role"], "error");
    assert_eq!(messages[2]["cardType"], "error");
    assert_eq!(messages[2]["collapsed"], false);
    assert_eq!(messages[3]["role"], "event");
    assert_eq!(messages[3]["cardType"], "event");
    assert_eq!(messages[4]["role"], "agent");
    let serialized = serde_json::to_string(messages).unwrap();
    assert!(!serialized.contains(&credential_canary));
    assert!(!messages.iter().any(|message| {
        message["text"]
            .as_str()
            .unwrap_or_default()
            .contains(&path_canary)
    }));
}

#[test]
fn native_history_decodes_embedded_json_string_content() {
    let dir = temp_dir("decoded-embedded-history");
    fs::write(
        dir.join("project.jsonl"),
        [
            r#"{"sessionId":"claude-session-1","type":"user","message":{"role":"user","content":"{\"type\":\"text\",\"text\":\"Decoded native prompt title\"}"}}"#,
            r#"{"sessionId":"claude-session-1","type":"assistant","message":{"role":"assistant","content":"{\"type\":\"text\",\"text\":\"Decoded native answer\"}"}}"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "claude-code",
        "root": dir.to_string_lossy()
    }))
    .unwrap();

    let session = &listed["sessions"].as_array().unwrap()[0];
    assert_eq!(session["title"], "Decoded native prompt title");
    assert_eq!(
        session["messages"][0]["text"],
        "Decoded native prompt title"
    );
    assert_eq!(session["messages"][1]["text"], "Decoded native answer");
}

#[test]
fn adapters_emit_semantic_layers_with_raw_evidence_refs() {
    let root = temp_dir("semantic-adapters");

    let codex_dir = root.join("codex");
    fs::create_dir_all(&codex_dir).unwrap();
    fs::write(
        codex_dir.join("session.jsonl"),
        [
            r#"{"sessionId":"codex-semantic","role":"user","content":"Codex semantic prompt"}"#,
            r#"{"sessionId":"codex-semantic","role":"assistant","content":"Codex semantic reply"}"#,
            r#"{"sessionId":"codex-semantic","type":"tool_use","name":"shell","input":{"command":"echo hi"}}"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let claude_dir = root.join("claude");
    fs::create_dir_all(&claude_dir).unwrap();
    fs::write(
        claude_dir.join("session.jsonl"),
        [
            r#"{"sessionId":"claude-semantic","type":"user","message":{"role":"user","content":[{"type":"text","text":"Claude semantic prompt"}]}}"#,
            r#"{"sessionId":"claude-semantic","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Claude semantic reply"},{"type":"tool_use","id":"1","name":"Read","input":{"path":"AGENTS.md"}}]}}"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let antigravity_dir = root.join("antigravity");
    fs::create_dir_all(&antigravity_dir).unwrap();
    fs::write(
        antigravity_dir.join("session.json"),
        serde_json::to_string_pretty(&json!({
            "sessions": [{
                "sessionId": "antigravity-semantic",
                "messages": [
                    {"role":"user","content":"<USER_REQUEST>Antigravity semantic prompt</USER_REQUEST>"},
                    {"role":"view_file","content":"file contents"},
                    {"role":"planner_response","content":"Antigravity semantic reply"}
                ]
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    let cursor_dir = root.join("cursor");
    fs::create_dir_all(&cursor_dir).unwrap();
    let cursor_db = cursor_dir.join("state.vscdb");
    {
        let connection = Connection::open(&cursor_db).unwrap();
        connection
            .execute(
                "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value BLOB)",
                [],
            )
            .unwrap();
        let payload = serde_json::to_vec(&json!({
            "messages": [
                {"role":"user","text":"Cursor semantic prompt"},
                {"role":"assistant","text":"Cursor semantic reply"}
            ]
        }))
        .unwrap();
        connection
            .execute(
                "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
                rusqlite::params!["composerData:chat", payload],
            )
            .unwrap();
    }

    for (agent, dir) in [
        ("codex", &codex_dir),
        ("claude-code", &claude_dir),
        ("antigravity", &antigravity_dir),
        ("cursor", &cursor_dir),
    ] {
        let listed = conversation_list(&json!({
            "agent": agent,
            "root": display_path(dir)
        }))
        .unwrap();
        let sessions = listed["sessions"].as_array().unwrap();
        assert!(
            !sessions.is_empty(),
            "{agent} should produce at least one semantic session"
        );
        let session = &sessions[0];
        assert_eq!(session["readOnly"], true);
        let semantic = session.get("semantic").expect("semantic document required");
        crate::domain::conversation_semantic::validate_semantic_conversation(semantic)
            .unwrap_or_else(|error| panic!("{agent} semantic invalid: {error}"));
        assert_eq!(semantic["kind"], "semantic-conversation");
        assert_eq!(semantic["privacyDefaults"]["defaultView"], "thread");
        assert!(
            semantic["thread"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| event["role"] == "user"),
            "{agent} thread should include a user message"
        );
        let evidence = &semantic["raw"]["evidenceRefs"][0];
        assert!(!evidence["pathRef"].as_str().unwrap_or_default().is_empty());
        assert!(
            !evidence["contentHash"]
                .as_str()
                .unwrap_or_default()
                .is_empty()
        );
        assert!(
            !session["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message["layer"] == "raw"),
            "{agent} default messages must not include raw layer dumps"
        );
    }
}
