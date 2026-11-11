use super::test_support::*;

#[test]
fn pure_startup_logs_are_not_native_conversations() {
    let dir = temp_dir("startup-log-history");
    fs::write(
        dir.join("opencode.log"),
        [
            r#"INFO 2026-06-20T00:00:00Z args=["mcp","list"] opencode"#,
            "INFO service=config path=<user-home>/.config/opencode/config.json",
            "INFO directory=/workspace/licomesh creating instance",
        ]
        .join("\n"),
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "opencode",
        "root": dir.to_string_lossy()
    }))
    .unwrap();

    assert_eq!(listed["sessions"].as_array().unwrap().len(), 0);
}

#[test]
fn service_logs_with_embedded_messages_are_not_native_conversations() {
    let dir = temp_dir("embedded-message-log-history");
    fs::write(
        dir.join("opencode.log"),
        [
            r#"INFO 2026-06-20T00:00:00Z service=default directory=/repo/Pact creating instance"#,
            r#"ERROR 2026-06-20T00:00:01Z service=llm requestBodyValues={"messages":[{"role":"user","content":"Pact task"},{"role":"assistant","content":"answer"}]}"#,
            r#"INFO 2026-06-20T00:00:02Z service=server status=started"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "opencode",
        "root": dir.to_string_lossy(),
        "archiveMode": true,
        "matchTerms": ["Pact"]
    }))
    .unwrap();

    assert_eq!(listed["sessions"].as_array().unwrap().len(), 0);
}

#[test]
fn text_transcripts_are_native_conversations() {
    let dir = temp_dir("text-transcript-history");
    fs::write(
        dir.join("conversation.txt"),
        ["User: archive the LicoMesh history", "Assistant: archived"].join("\n"),
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "opencode",
        "root": dir.to_string_lossy()
    }))
    .unwrap();

    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["title"], "archive the LicoMesh history");
}

#[test]
fn antigravity_history_decodes_protocol_wrapped_messages() {
    let dir = temp_dir("antigravity-protocol-history");
    fs::write(
        dir.join("ag-session.json"),
        serde_json::to_string_pretty(&json!({
            "sessions": [
                {
                    "sessionId": "antigravity-session",
                    "title": "<USER_REQUEST> 请找到本项目的开发规则文档入口 </USER_REQUEST>",
                    "messages": [
                        {
                            "role": "user",
                            "content": "<SYSTEM_MESSAGE>Hidden Antigravity runtime context.</SYSTEM_MESSAGE>\n<USER_REQUEST>请找到本项目的开发规则文档入口</USER_REQUEST>"
                        },
                        {
                            "role": "assistant",
                            "content": "The following is a <SYSTEM_MESSAGE> not actually sent by the user. It is provided by the system as important information to pay attention to."
                        },
                        {
                            "role": "view_file",
                            "content": "2255 │ \"coverageContribution\": false,\n2256 │ \"artifacts\": [],\n2257 │ \"command\": \"npm\"\n2258 │ \"args\": [\n2259 │   \"run\",\n2260 │   \"verify\"\n2261 │ ]"
                        },
                        {
                            "role": "run_command",
                            "content": "npm run verify\nPASS 133 tests"
                        },
                        {
                            "role": "planner_response",
                            "content": "开发规则入口在仓库根目录的 AGENTS.md。"
                        }
                    ]
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "antigravity",
        "root": dir.to_string_lossy()
    }))
    .unwrap();

    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["title"], "请找到本项目的开发规则文档入口");
    let messages = sessions[0]["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0]["text"], "请找到本项目的开发规则文档入口");
    assert_eq!(messages[1]["role"], "tool_call");
    assert_eq!(messages[1]["cardTitle"], "View file");
    // Activity details are expandable: the recorded command content and file
    // view stay available on the tool call card.
    assert!(
        messages[1]["text"]
            .as_str()
            .unwrap()
            .contains("coverageContribution")
    );
    assert_eq!(messages[2]["role"], "tool_call");
    assert_eq!(messages[2]["cardTitle"], "Run command");
    assert_eq!(messages[2]["text"], "npm run verify\nPASS 133 tests");
    assert_eq!(
        messages[3]["text"],
        "开发规则入口在仓库根目录的 AGENTS.md。"
    );
    assert!(!messages.iter().any(|message| {
        let text = message["text"].as_str().unwrap_or_default();
        text.contains("<USER_REQUEST>")
            || text.contains("<SYSTEM_MESSAGE>")
            || text.contains("not actually sent by the user")
    }));
}

#[test]
fn claude_code_adapter_extracts_nested_jsonl_messages() {
    let dir = temp_dir("claude-history");
    fs::write(
        dir.join("project.jsonl"),
        [
            r#"{"sessionId":"claude-session-1","type":"user","message":{"role":"user","content":[{"type":"text","text":"Open the LicoMesh repo"}]}}"#,
            r#"{"sessionId":"claude-session-1","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Repo opened"}]}}"#,
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
    assert_eq!(sessions[0]["adapterId"], "claude-code");
    assert_eq!(sessions[0]["nativeSessionId"], "claude-session-1");
    assert_eq!(sessions[0]["messages"][0]["text"], "Open the LicoMesh repo");
    assert_eq!(sessions[0]["messages"][1]["role"], "agent");
}

#[test]
fn exact_jsonl_reads_keep_only_the_requested_session_group() {
    let dir = temp_dir("exact-jsonl-session-filter");
    let path = dir.join("shared-history.jsonl");
    let lines = (0..80)
        .map(|index| {
            json!({
                "sessionId": format!("session-{index}"),
                "role": "user",
                "content": format!("Prompt {index}")
            })
            .to_string()
        })
        .collect::<Vec<_>>();
    fs::write(&path, lines.join("\n")).unwrap();

    let metadata = fs::metadata(&path).unwrap();
    let sessions = parse_jsonl_sessions(
        HistoryAdapter::ClaudeCode,
        &path,
        "test-history",
        &metadata,
        HistoryScanConfig::from_params(&json!({"sessionId": "session-37"})),
    );

    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["nativeSessionId"], "session-37");
    assert_eq!(sessions[0]["messages"].as_array().unwrap().len(), 1);
}

#[test]
fn exact_json_document_reads_only_the_requested_session_item() {
    let dir = temp_dir("exact-json-session-filter");
    let sessions = (0..80)
        .map(|index| {
            json!({
                "sessionId": format!("session-{index}"),
                "messages": [{"role": "user", "content": format!("Prompt {index}")}]
            })
        })
        .collect::<Vec<_>>();
    fs::write(
        dir.join("shared-history.json"),
        json!({"sessions": sessions}).to_string(),
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "opencode",
        "root": display_path(&dir),
        "sessionId": "session-37"
    }))
    .unwrap();

    assert_eq!(listed["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(listed["sessions"][0]["nativeSessionId"], "session-37");
    assert_eq!(
        listed["sessions"][0]["messages"].as_array().unwrap().len(),
        1
    );
}

#[test]
fn claude_code_adapter_projects_launch_directory_for_project_grouping() {
    let dir = temp_dir("claude-working-directory");
    fs::write(
        dir.join("project.jsonl"),
        [
            r#"{"sessionId":"claude-session-cwd","type":"system","cwd":"/workspace/projects/alpha"}"#,
            r#"{"sessionId":"claude-session-cwd","type":"user","message":{"role":"user","content":"Group this conversation"}}"#,
            r#"{"sessionId":"claude-session-cwd","type":"assistant","message":{"role":"assistant","content":"Grouped"}}"#,
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
    assert_eq!(sessions[0]["workingDirectory"], "/workspace/projects/alpha");
}

#[test]
fn claude_code_adapter_rejects_relative_launch_directory() {
    let dir = temp_dir("claude-relative-working-directory");
    fs::write(
        dir.join("project.jsonl"),
        [
            r#"{"sessionId":"claude-session-relative","type":"user","cwd":"relative/project","message":{"role":"user","content":"Do not group this conversation"}}"#,
            r#"{"sessionId":"claude-session-relative","type":"assistant","message":{"role":"assistant","content":"Ungrouped"}}"#,
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
    assert!(session.get("workingDirectory").is_none());
}

#[test]
fn claude_code_adapter_attaches_message_model() {
    let dir = temp_dir("claude-model-history");
    fs::write(
        dir.join("project.jsonl"),
        [
            r#"{"sessionId":"claude-session-2","type":"user","message":{"role":"user","content":[{"type":"text","text":"Refactor the parser"}]}}"#,
            r#"{"sessionId":"claude-session-2","type":"assistant","message":{"role":"assistant","model":"claude-opus-4-6","content":[{"type":"text","text":"Done"}],"usage":{"input_tokens":12,"output_tokens":4}}}"#,
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
    let message = &sessions[0]["messages"][1];
    assert_eq!(message["model"], "claude-opus-4-6");
    assert_eq!(message["usage"]["totalTokens"], 16);
}

#[test]
fn directory_layout_sessions_take_the_conversation_uuid_from_the_path() {
    let dir = temp_dir("antigravity-directory-identity");
    let conversation_a = dir
        .join("brain")
        .join("7bb7b109-f089-4529-a6c9-2c019a71c106")
        .join(".system_generated")
        .join("logs");
    let conversation_b = dir
        .join("brain")
        .join("2f7230ca-e675-4846-a922-1104cf0a1854")
        .join(".system_generated")
        .join("logs");
    fs::create_dir_all(&conversation_a).unwrap();
    fs::create_dir_all(&conversation_b).unwrap();
    let record = |text: &str| {
        json!({"role": "user", "content": text, "created_at": "2026-07-31T08:04:37Z"}).to_string()
    };
    for (dir, name, text) in [
        (&conversation_a, "transcript.jsonl", "first conversation"),
        (
            &conversation_a,
            "transcript_full.jsonl",
            "first conversation",
        ),
        (&conversation_b, "transcript.jsonl", "second conversation"),
    ] {
        fs::write(dir.join(name), record(text)).unwrap();
    }

    let listed = conversation_list(&json!({
        "agent": "antigravity",
        "root": dir.to_string_lossy()
    }))
    .unwrap();

    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2);
    let mut native_ids: Vec<&str> = sessions
        .iter()
        .map(|session| session["nativeSessionId"].as_str().unwrap())
        .collect();
    native_ids.sort();
    assert_eq!(
        native_ids,
        vec![
            "2f7230ca-e675-4846-a922-1104cf0a1854",
            "7bb7b109-f089-4529-a6c9-2c019a71c106",
        ],
        "transcript duplicates collapse by conversation uuid while distinct conversations stay distinct"
    );
    let projected_ids: std::collections::BTreeSet<&str> = sessions
        .iter()
        .map(|session| session["id"].as_str().unwrap())
        .collect();
    assert_eq!(projected_ids.len(), 2);
}

#[test]
fn embedded_session_id_wins_and_non_identity_jsonl_is_skipped() {
    let dir = temp_dir("antigravity-embedded-identity");
    let uuid_dir = dir
        .join("brain")
        .join("7bb7b109-f089-4529-a6c9-2c019a71c106");
    fs::create_dir_all(&uuid_dir).unwrap();
    fs::write(
        uuid_dir.join("transcript.jsonl"),
        json!({"sessionId": "embedded-id", "role": "user", "content": "hi"}).to_string(),
    )
    .unwrap();
    fs::write(
        dir.join("loose.jsonl"),
        json!({"role": "user", "content": "hi"}).to_string(),
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "antigravity",
        "root": dir.to_string_lossy()
    }))
    .unwrap();

    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["nativeSessionId"], "embedded-id");
}

#[test]
fn large_generic_sources_keep_first_and_last_messages() {
    let dir = temp_dir("large-generic-history");
    let path = dir.join("large.jsonl");
    let mut file = fs::File::create(&path).unwrap();
    use std::io::Write;
    writeln!(
        file,
        "{}",
        json!({"sessionId": "large-session", "role": "user", "content": "first"})
    )
    .unwrap();
    writeln!(
        file,
        "{}",
        json!({"sessionId": "large-session", "role": "assistant", "content": "x".repeat(32 * 1024 * 1024 + 1)})
    )
    .unwrap();
    writeln!(
        file,
        "{}",
        json!({"sessionId": "large-session", "role": "assistant", "content": "last"})
    )
    .unwrap();
    drop(file);

    let listed = conversation_list(&json!({
        "agent": "opencode",
        "root": dir.to_string_lossy()
    }))
    .unwrap();
    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    let messages = sessions[0]["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages.first().unwrap()["text"], "first");
    assert_eq!(messages.last().unwrap()["text"], "last");
}

#[test]
fn generic_bookkeeping_never_becomes_a_session() {
    let dir = temp_dir("generic-bookkeeping");
    fs::write(
        dir.join("settings.json"),
        json!({"theme": "dark", "telemetry": false}).to_string(),
    )
    .unwrap();
    fs::write(
        dir.join("state.jsonl"),
        json!({"kind": "state", "value": "ready"}).to_string(),
    )
    .unwrap();
    fs::write(
        dir.join("conversation.json"),
        json!({
            "sessionId": "real-session",
            "messages": [{"role": "user", "content": "visible"}]
        })
        .to_string(),
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "opencode",
        "root": dir.to_string_lossy()
    }))
    .unwrap();
    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["nativeSessionId"], "real-session");
}
