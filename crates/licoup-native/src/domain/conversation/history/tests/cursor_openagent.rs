use super::test_support::*;

#[test]
fn normalized_openagent_tokens_keep_additive_cache_and_reasoning() {
    let usage = extract_token_usage(&json!({
        "tokens": {
            "input": 60,
            "output": 5,
            "reasoning": 2,
            "cache": {"read": 30, "write": 10},
            "total": 67
        }
    }))
    .expect("normalized usage");

    assert_eq!(usage["promptTokens"], 100);
    assert_eq!(usage["cachedInputTokens"], 30);
    assert_eq!(usage["completionTokens"], 7);
    assert_eq!(usage["totalTokens"], 107);
}

#[test]
fn opencode_adapter_imports_sqlite_message_parts() {
    let dir = temp_dir("opencode-sqlite-history");
    let database = dir.join("opencode.db");
    create_openagent_fixture_database(&database, "OpenCode prompt");

    let listed = conversation_list(&json!({
        "agent": "opencode",
        "root": dir.to_string_lossy()
    }))
    .unwrap();

    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["adapterId"], "opencode");
    assert_eq!(sessions[0]["title"], "OpenCode prompt");
    assert_eq!(sessions[0]["model"], "gpt-test");
    assert_eq!(sessions[0]["workingDirectory"], "/workspace/opencode");
    let messages = sessions[0]["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["text"], "OpenCode prompt");
    assert_eq!(messages[1]["role"], "agent");
    assert_eq!(messages[1]["text"], "OpenCode answer");
}

#[test]
fn kilo_code_adapter_imports_sqlite_message_parts() {
    let dir = temp_dir("kilo-sqlite-history");
    let database = dir.join("kilo.db");
    create_openagent_fixture_database(&database, "Kilo prompt");

    let listed = conversation_list(&json!({
        "agent": "kilo-code",
        "root": dir.to_string_lossy()
    }))
    .unwrap();

    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["adapterId"], "kilo-code");
    assert_eq!(sessions[0]["title"], "Kilo prompt");
    let messages = sessions[0]["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["text"], "Kilo prompt");
    assert_eq!(messages[1]["role"], "agent");
}

#[test]
fn openagent_sqlite_scan_does_not_truncate_after_one_thousand_sessions() {
    let dir = temp_dir("openagent-sqlite-unbounded-sessions");
    let database = dir.join("opencode.db");
    create_openagent_fixture_database(&database, "OpenCode prompt 0");
    let mut connection = Connection::open(&database).unwrap();
    connection
        .execute_batch(
            "CREATE INDEX message_session_id ON message(session_id);\
             CREATE INDEX part_session_id ON part(session_id);",
        )
        .unwrap();
    let transaction = connection.transaction().unwrap();
    for index in 1..=1_000 {
        let session_id = format!("ses_{index:04}");
        let message_id = format!("msg_{index:04}");
        let prompt = format!("OpenCode prompt {index}");
        transaction
            .execute(
                "INSERT INTO session VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                (
                    &session_id,
                    &prompt,
                    "/workspace/opencode",
                    "/workspace/opencode",
                    "build",
                    "gpt-test",
                    1_787_616_000_000i64 + index,
                    1_787_616_060_000i64 + index,
                    1i64,
                    2i64,
                    0i64,
                    0i64,
                    0i64,
                ),
            )
            .unwrap();
        transaction
            .execute(
                "INSERT INTO message VALUES (?1, ?2, ?3, ?4, ?5)",
                (
                    &message_id,
                    &session_id,
                    1_787_616_000_000i64 + index,
                    1_787_616_000_000i64 + index,
                    json!({
                        "role": "user",
                        "time": {"created": 1_787_616_000_000i64 + index},
                        "tokens": {"total": 3, "input": 1, "output": 2}
                    })
                    .to_string(),
                ),
            )
            .unwrap();
        transaction
            .execute(
                "INSERT INTO part VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                (
                    format!("part_{index:04}"),
                    &message_id,
                    &session_id,
                    1_787_616_000_000i64 + index,
                    1_787_616_000_000i64 + index,
                    json!({"type": "text", "text": prompt}).to_string(),
                ),
            )
            .unwrap();
    }
    transaction.commit().unwrap();

    let listed = conversation_list(&json!({
        "agent": "opencode",
        "root": dir.to_string_lossy()
    }))
    .unwrap();

    assert_eq!(listed["page"]["totalSessions"], 1_001);
    assert_eq!(listed["sessions"].as_array().unwrap().len(), 1_001);
}

#[test]
fn cursor_adapter_reads_sqlite_blob_chat_payloads() {
    let dir = temp_dir("cursor-history");
    let database = dir.join("state.vscdb");
    {
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "CREATE TABLE ItemTable (key TEXT NOT NULL, value BLOB NOT NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
                (
                    "composerData.session-1",
                    br#"{"messages":[{"role":"user","text":"Cursor native prompt"},{"role":"assistant","text":"Cursor native answer"}]}"#.as_slice(),
                ),
            )
            .unwrap();
    }

    let listed = conversation_list(&json!({
        "agent": "cursor",
        "root": dir.to_string_lossy()
    }))
    .unwrap();

    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["adapterId"], "cursor");
    assert_eq!(sessions[0]["nativeSessionId"], "composerData.session-1");
    assert_eq!(sessions[0]["messages"].as_array().unwrap().len(), 2);
}

#[test]
fn cursor_adapter_reads_disk_kv_composer_bubbles_with_model() {
    let dir = temp_dir("cursor-disk-kv");
    let database = dir.join("state.vscdb");
    let composer_id = "11111111-1111-1111-1111-111111111111";
    let user_bubble = "22222222-2222-2222-2222-222222222222";
    let agent_bubble = "33333333-3333-3333-3333-333333333333";
    {
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "CREATE TABLE cursorDiskKV (key TEXT NOT NULL, value BLOB NOT NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                rusqlite::params![
                    format!("composerData:{composer_id}"),
                    serde_json::to_vec(&json!({
                        "composerId": composer_id,
                        "name": "Cursor model session",
                        "createdAt": 1_773_798_000_000i64,
                        "lastUpdatedAt": 1_773_798_100_000i64,
                        "modelConfig": { "modelName": "default", "maxMode": false },
                        "fullConversationHeadersOnly": [
                            { "bubbleId": user_bubble, "type": 1 },
                            { "bubbleId": agent_bubble, "type": 2 }
                        ]
                    }))
                    .unwrap()
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                rusqlite::params![
                    format!("bubbleId:{composer_id}:{user_bubble}"),
                    serde_json::to_vec(&json!({
                        "bubbleId": user_bubble,
                        "type": 1,
                        "createdAt": 1_773_798_000_000i64,
                        "text": "Please review this Cursor usage scan.",
                        "tokenCount": { "inputTokens": 0, "outputTokens": 0 }
                    }))
                    .unwrap()
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                rusqlite::params![
                    format!("bubbleId:{composer_id}:{agent_bubble}"),
                    serde_json::to_vec(&json!({
                        "bubbleId": agent_bubble,
                        "type": 2,
                        "createdAt": 1_773_798_050_000i64,
                        "text": "Cursor attributed this reply to the selected model.",
                        "modelInfo": { "modelName": "grok-4.5" },
                        "tokenCount": { "inputTokens": 120, "outputTokens": 40 }
                    }))
                    .unwrap()
                ],
            )
            .unwrap();
    }

    let listed = conversation_list(&json!({
        "agent": "cursor",
        "root": dir.to_string_lossy()
    }))
    .unwrap();
    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["nativeSessionId"], composer_id);
    assert_eq!(sessions[0]["model"], "cursor-auto");
    assert_eq!(sessions[0]["messages"].as_array().unwrap().len(), 2);
    assert_eq!(sessions[0]["messages"][0]["role"], "user");
    assert_eq!(sessions[0]["messages"][0]["model"], "cursor-auto");
    assert_eq!(sessions[0]["messages"][1]["role"], "agent");
    assert_eq!(sessions[0]["messages"][1]["model"], "grok-4.5");
    assert_eq!(sessions[0]["messages"][1]["usage"]["promptTokens"], 120);
    assert_eq!(sessions[0]["messages"][1]["usage"]["completionTokens"], 40);

    let usage_state = temp_dir("cursor-usage-state");
    let _ = crate::domain::agent_usage::host::install();
    let usage = crate::domain::agent_usage::scan(&json!({
        "agent": "cursor",
        "root": dir.to_string_lossy(),
        "historyDays": 3650,
        "now": "2026-03-18T12:00:00Z",
        "forceRefresh": true,
        "stateRoot": usage_state.to_string_lossy()
    }))
    .unwrap();
    // Cursor usage comes from the hosted ledger only. A rerooted scan names
    // no state store, so the local bubble counters stay unprojected.
    let history = &usage["agents"][0]["history"];
    assert_eq!(history["source"], "cursor-hosted-usage-events");
    assert_eq!(history["totalTokens"], 0);
    assert_eq!(history["dailyUsage"], json!([]));
    assert!(usage["warnings"].as_array().unwrap().iter().any(|warning| {
        warning["code"] == "cursor_state_store_unavailable" && warning["agentId"] == "cursor"
    }));
}

#[test]
fn cursor_adapter_prefers_selected_models_over_composer_label() {
    let dir = temp_dir("cursor-selected-models");
    let database = dir.join("state.vscdb");
    let composer_id = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    let user_bubble = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
    let agent_bubble = "cccccccc-cccc-cccc-cccc-cccccccccccc";
    {
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "CREATE TABLE cursorDiskKV (key TEXT NOT NULL, value BLOB NOT NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                rusqlite::params![
                    format!("composerData:{composer_id}"),
                    serde_json::to_vec(&json!({
                        "composerId": composer_id,
                        "name": "Composer label hides selected model",
                        "createdAt": 1_773_798_000_000i64,
                        "lastUpdatedAt": 1_773_798_100_000i64,
                        "modelConfig": {
                            "modelName": "composer-2.5-fast",
                            "maxMode": false,
                            "selectedModels": [{ "modelId": "grok-4.5", "parameters": [] }]
                        },
                        "fullConversationHeadersOnly": [
                            { "bubbleId": user_bubble, "type": 1 },
                            { "bubbleId": agent_bubble, "type": 2 }
                        ]
                    }))
                    .unwrap()
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                rusqlite::params![
                    format!("bubbleId:{composer_id}:{user_bubble}"),
                    serde_json::to_vec(&json!({
                        "bubbleId": user_bubble,
                        "type": 1,
                        "createdAt": 1_773_798_000_000i64,
                        "text": "Attribute Cursor usage to the selected model.",
                        "tokenCount": { "inputTokens": 0, "outputTokens": 0 },
                        "modelInfo": { "modelName": "default" }
                    }))
                    .unwrap()
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                rusqlite::params![
                    format!("bubbleId:{composer_id}:{agent_bubble}"),
                    serde_json::to_vec(&json!({
                        "bubbleId": agent_bubble,
                        "type": 2,
                        "createdAt": 1_773_798_050_000i64,
                        "text": "Selected model should win over Composer product label.",
                        "tokenCount": { "inputTokens": 80, "outputTokens": 20 },
                        "modelInfo": { "modelName": "default" }
                    }))
                    .unwrap()
                ],
            )
            .unwrap();
    }

    let listed = conversation_list(&json!({
        "agent": "cursor",
        "root": dir.to_string_lossy()
    }))
    .unwrap();
    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["model"], "grok-4.5");
    assert_eq!(sessions[0]["messages"][0]["model"], "grok-4.5");
    assert_eq!(sessions[0]["messages"][1]["model"], "grok-4.5");

    let usage_state = temp_dir("cursor-selected-usage-state");
    let _ = crate::domain::agent_usage::host::install();
    let usage = crate::domain::agent_usage::scan(&json!({
        "agent": "cursor",
        "root": dir.to_string_lossy(),
        "historyDays": 3650,
        "now": "2026-03-18T12:00:00Z",
        "forceRefresh": true,
        "stateRoot": usage_state.to_string_lossy()
    }))
    .unwrap();
    let history = &usage["agents"][0]["history"];
    assert_eq!(history["source"], "cursor-hosted-usage-events");
    assert_eq!(history["totalTokens"], 0);
    assert_eq!(history["dailyUsage"], json!([]));
}

#[test]
fn cursor_adapter_folds_subagent_composers_into_parent_session_cards() {
    let dir = temp_dir("cursor-subagent-merge");
    let database = dir.join("state.vscdb");
    let parent_id = "0a0a0a0a-0000-4000-8000-000000000001";
    let child_id = "0a0a0a0a-0000-4000-8000-000000000002";
    let parent_user_bubble = "1b1b1b1b-0000-4000-8000-000000000001";
    let parent_agent_bubble = "1b1b1b1b-0000-4000-8000-000000000002";
    let child_user_bubble = "2c2c2c2c-0000-4000-8000-000000000001";
    let child_agent_bubble = "2c2c2c2c-0000-4000-8000-000000000002";
    {
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "CREATE TABLE cursorDiskKV (key TEXT NOT NULL, value BLOB NOT NULL)",
                [],
            )
            .unwrap();
        for (key, value) in [
            (
                format!("composerData:{parent_id}"),
                json!({
                    "composerId": parent_id,
                    "name": "Main Cursor thread",
                    "createdAt": 1_773_798_000_000i64,
                    "lastUpdatedAt": 1_773_798_400_000i64,
                    "subagentComposerIds": [child_id],
                    "fullConversationHeadersOnly": [
                        { "bubbleId": parent_user_bubble, "type": 1 },
                        { "bubbleId": parent_agent_bubble, "type": 2 }
                    ]
                }),
            ),
            (
                format!("composerData:{child_id}"),
                json!({
                    "composerId": child_id,
                    "name": "Explore the cursor parser",
                    "createdAt": 1_773_798_100_000i64,
                    "lastUpdatedAt": 1_773_798_300_000i64,
                    "subagentInfo": {
                        "subagentType": 1,
                        "parentComposerId": parent_id,
                        "subagentTypeName": "explore",
                        "toolCallId": "tool-call-1",
                        "rootParentConversationId": parent_id
                    },
                    "fullConversationHeadersOnly": [
                        { "bubbleId": child_user_bubble, "type": 1 },
                        { "bubbleId": child_agent_bubble, "type": 2 }
                    ]
                }),
            ),
            (
                format!("bubbleId:{parent_id}:{parent_user_bubble}"),
                json!({
                    "bubbleId": parent_user_bubble,
                    "type": 1,
                    "createdAt": 1_773_798_000_000i64,
                    "text": "Where does the Cursor parser live?"
                }),
            ),
            (
                format!("bubbleId:{parent_id}:{parent_agent_bubble}"),
                json!({
                    "bubbleId": parent_agent_bubble,
                    "type": 2,
                    "createdAt": 1_773_798_400_000i64,
                    "text": "The main thread wraps up."
                }),
            ),
            (
                format!("bubbleId:{child_id}:{child_user_bubble}"),
                json!({
                    "bubbleId": child_user_bubble,
                    "type": 1,
                    "createdAt": 1_773_798_100_000i64,
                    "text": "Map the cursor history parser."
                }),
            ),
            (
                format!("bubbleId:{child_id}:{child_agent_bubble}"),
                json!({
                    "bubbleId": child_agent_bubble,
                    "type": 2,
                    "createdAt": 1_773_798_300_000i64,
                    "text": "The parser lives in cursor.rs."
                }),
            ),
        ] {
            connection
                .execute(
                    "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                    rusqlite::params![key, serde_json::to_vec(&value).unwrap()],
                )
                .unwrap();
        }
    }

    let listed = conversation_list(&json!({
        "agent": "cursor",
        "root": dir.to_string_lossy()
    }))
    .unwrap();

    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(
        sessions.len(),
        1,
        "subagent composer must not stay top-level"
    );
    let session = &sessions[0];
    assert_eq!(session["nativeSessionId"], parent_id);
    let messages = session["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3, "expected user, subagent card, agent");
    assert_eq!(messages[0]["role"], "user");
    let card = &messages[1];
    assert_eq!(card["role"], "subagent");
    assert_eq!(card["cardType"], "subagent");
    assert_eq!(card["cardTitle"], "Explore the cursor parser");
    assert_eq!(card["collapsed"], true);
    assert!(card["messages"].as_array().unwrap().is_empty());
    assert_eq!(card["childMessageCount"], 2);
    let child = conversation_list(&json!({"agent": "cursor", "root": display_path(&dir), "sessionId": card["childSessionId"]})).unwrap();
    let child_messages = child["sessions"][0]["messages"].as_array().unwrap();
    assert_eq!(child_messages.len(), 2);
    assert_eq!(child_messages[0]["role"], "user");
    assert_eq!(child_messages[0]["text"], "Map the cursor history parser.");
    assert_eq!(child_messages[1]["role"], "agent");
    assert_eq!(child_messages[1]["text"], "The parser lives in cursor.rs.");
    assert_eq!(messages[2]["role"], "agent");
    assert_eq!(messages[2]["text"], "The main thread wraps up.");
}

#[test]
fn cursor_adapter_keeps_orphan_subagent_composer_as_top_level_session() {
    let dir = temp_dir("cursor-subagent-orphan");
    let database = dir.join("state.vscdb");
    let child_id = "0a0a0a0a-0000-4000-8000-000000000003";
    let child_user_bubble = "2c2c2c2c-0000-4000-8000-000000000003";
    {
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "CREATE TABLE cursorDiskKV (key TEXT NOT NULL, value BLOB NOT NULL)",
                [],
            )
            .unwrap();
        for (key, value) in [
            (
                format!("composerData:{child_id}"),
                json!({
                    "composerId": child_id,
                    "name": "Subagent without a parent session",
                    "subagentInfo": {
                        "subagentType": 1,
                        "parentComposerId": "missing-parent-composer",
                        "subagentTypeName": "explore"
                    },
                    "fullConversationHeadersOnly": [
                        { "bubbleId": child_user_bubble, "type": 1 }
                    ]
                }),
            ),
            (
                format!("bubbleId:{child_id}:{child_user_bubble}"),
                json!({
                    "bubbleId": child_user_bubble,
                    "type": 1,
                    "createdAt": 1_773_798_100_000i64,
                    "text": "Orphan subagent task prompt."
                }),
            ),
        ] {
            connection
                .execute(
                    "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                    rusqlite::params![key, serde_json::to_vec(&value).unwrap()],
                )
                .unwrap();
        }
    }

    let listed = conversation_list(&json!({
        "agent": "cursor",
        "root": dir.to_string_lossy()
    }))
    .unwrap();

    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session["nativeSessionId"], child_id);
    assert!(
        session.get("delegatedSubagent").is_none(),
        "orphan subagent must not carry delegated markers: {session:?}"
    );
    assert!(
        session["messages"]
            .as_array()
            .unwrap()
            .iter()
            .all(|message| message.get("cardType").is_none()),
        "orphan subagent messages must stay flat: {session:?}"
    );
}

#[test]
fn cursor_usage_scan_ignores_composer_context_occupancy() {
    let dir = temp_dir("cursor-composer-usage");
    let database = dir.join("state.vscdb");
    let composer_id = "dddddddd-dddd-dddd-dddd-dddddddddddd";
    let user_bubble = "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee";
    let agent_bubble = "ffffffff-ffff-ffff-ffff-ffffffffffff";
    {
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "CREATE TABLE cursorDiskKV (key TEXT NOT NULL, value BLOB NOT NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                rusqlite::params![
                    format!("composerData:{composer_id}"),
                    serde_json::to_vec(&json!({
                        "composerId": composer_id,
                        "name": "Composer context meter session",
                        "createdAt": 1_773_798_000_000i64,
                        "lastUpdatedAt": 1_773_798_100_000i64,
                        "modelConfig": {
                            "modelName": "default",
                            "selectedModels": [{"modelId": "claude-fable-5"}]
                        },
                        "promptTokenBreakdown": {
                            "totalUsedTokens": 4200,
                            "maxTokens": 256000
                        },
                        "fullConversationHeadersOnly": [
                            {"bubbleId": user_bubble, "type": 1},
                            {"bubbleId": agent_bubble, "type": 2}
                        ]
                    }))
                    .unwrap()
                ],
            )
            .unwrap();
        for (bubble_id, bubble_type, text) in [
            (user_bubble, 1, "Long prompt that would be badly estimated."),
            (agent_bubble, 2, "Short reply."),
        ] {
            connection
                .execute(
                    "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                    rusqlite::params![
                        format!("bubbleId:{composer_id}:{bubble_id}"),
                        serde_json::to_vec(&json!({
                            "bubbleId": bubble_id,
                            "type": bubble_type,
                            "createdAt": 1_773_798_050_000i64,
                            "text": text,
                            "tokenCount": {"inputTokens": 0, "outputTokens": 0}
                        }))
                        .unwrap()
                    ],
                )
                .unwrap();
        }
    }

    let usage_state = temp_dir("cursor-composer-usage-state");
    let _ = crate::domain::agent_usage::host::install();
    let usage = crate::domain::agent_usage::scan(&json!({
        "agent": "cursor",
        "root": dir.to_string_lossy(),
        "historyDays": 3650,
        "forceRefresh": true,
        "stateRoot": usage_state.to_string_lossy()
    }))
    .unwrap();
    let history = &usage["agents"][0]["history"];
    assert_eq!(history["source"], "cursor-hosted-usage-events");
    assert_eq!(history["totalTokens"], 0);
    assert_eq!(history["confidence"], "unavailable");
    assert_eq!(history["dailyUsage"], json!([]));
}

#[test]
fn cursor_usage_scan_does_not_treat_product_context_meter_as_usage() {
    let dir = temp_dir("cursor-composer-product-usage");
    let database = dir.join("state.vscdb");
    let composer_id = "99999999-9999-9999-9999-999999999999";
    let user_bubble = "88888888-8888-8888-8888-888888888888";
    let agent_bubble = "77777777-7777-7777-7777-777777777777";
    {
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "CREATE TABLE cursorDiskKV (key TEXT NOT NULL, value BLOB NOT NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                rusqlite::params![
                    format!("composerData:{composer_id}"),
                    serde_json::to_vec(&json!({
                        "composerId": composer_id,
                        "name": "Composer product context meter",
                        "createdAt": 1_773_798_000_000i64,
                        "lastUpdatedAt": 1_773_798_100_000i64,
                        "modelConfig": {
                            "modelName": "composer-2.5-fast",
                            "selectedModels": [{"modelId": "grok-4.5"}]
                        },
                        "promptTokenBreakdown": {
                            "totalUsedTokens": 6840995,
                            "maxTokens": 256000
                        },
                        "fullConversationHeadersOnly": [
                            {"bubbleId": user_bubble, "type": 1},
                            {"bubbleId": agent_bubble, "type": 2}
                        ]
                    }))
                    .unwrap()
                ],
            )
            .unwrap();
        for (bubble_id, bubble_type, text) in [
            (user_bubble, 1, "Composer session prompt."),
            (
                agent_bubble,
                2,
                "Routed through grok but metered as Composer.",
            ),
        ] {
            connection
                .execute(
                    "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                    rusqlite::params![
                        format!("bubbleId:{composer_id}:{bubble_id}"),
                        serde_json::to_vec(&json!({
                            "bubbleId": bubble_id,
                            "type": bubble_type,
                            "createdAt": 1_773_798_050_000i64,
                            "text": text,
                            "tokenCount": {"inputTokens": 0, "outputTokens": 0},
                            "modelInfo": {"modelName": "default"}
                        }))
                        .unwrap()
                    ],
                )
                .unwrap();
        }
    }

    let usage_state = temp_dir("cursor-composer-product-usage-state");
    let _ = crate::domain::agent_usage::host::install();
    let usage = crate::domain::agent_usage::scan(&json!({
        "agent": "cursor",
        "root": dir.to_string_lossy(),
        "historyDays": 3650,
        "forceRefresh": true,
        "stateRoot": usage_state.to_string_lossy()
    }))
    .unwrap();
    assert_eq!(
        usage["agents"][0]["history"]["source"],
        "cursor-hosted-usage-events"
    );
    assert_eq!(usage["agents"][0]["history"]["totalTokens"], 0);
    assert_eq!(usage["agents"][0]["history"]["dailyUsage"], json!([]));
}

#[test]
fn sqlite_history_preserves_user_record_rows() {
    let dir = temp_dir("sqlite-history");
    let database = dir.join("state.vscdb");
    {
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "CREATE TABLE ItemTable (key TEXT NOT NULL, value TEXT NOT NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
                ["chat.first", "user message: First native conversation turn"],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
                [
                    "chat.second",
                    "assistant message: Second native conversation turn",
                ],
            )
            .unwrap();
    }

    let listed = conversation_list(&json!({
        "agent": "cursor",
        "root": dir.to_string_lossy()
    }))
    .unwrap();

    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    let total_messages = sessions
        .iter()
        .map(|session| session["messages"].as_array().unwrap().len())
        .sum::<usize>();
    assert_eq!(total_messages, 1);
    assert!(
        sessions
            .iter()
            .any(|session| session["nativeSessionId"] == "chat.first")
    );
}

#[test]
fn cursor_adapter_reads_the_project_directory_from_the_composer_record() {
    let dir = temp_dir("cursor-composer-workspace");
    let database = dir.join("state.vscdb");
    let composer_id = "0c0c0c0c-0000-4000-8000-000000000001";
    let bubble_id = "3d3d3d3d-0000-4000-8000-000000000001";
    let project = dir.join("project-alpha");
    fs::create_dir_all(&project).unwrap();
    {
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "CREATE TABLE cursorDiskKV (key TEXT NOT NULL, value BLOB NOT NULL)",
                [],
            )
            .unwrap();
        for (key, value) in [
            (
                format!("composerData:{composer_id}"),
                json!({
                    "composerId": composer_id,
                    "name": "Composer with a workspace",
                    "createdAt": 1_773_798_000_000i64,
                    "lastUpdatedAt": 1_773_798_400_000i64,
                    "workspaceIdentifier": {
                        "id": "workspace-1",
                        "uri": {
                            "scheme": "file",
                            "path": project.to_string_lossy(),
                            "fsPath": project.to_string_lossy(),
                            "external": format!("file://{}", project.to_string_lossy())
                        }
                    },
                    "fullConversationHeadersOnly": [
                        { "bubbleId": bubble_id, "type": 1 }
                    ]
                }),
            ),
            (
                format!("bubbleId:{composer_id}:{bubble_id}"),
                json!({
                    "bubbleId": bubble_id,
                    "type": 1,
                    "createdAt": 1_773_798_000_000i64,
                    "text": "Which directory am I bound to?"
                }),
            ),
        ] {
            connection
                .execute(
                    "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                    rusqlite::params![key, serde_json::to_vec(&value).unwrap()],
                )
                .unwrap();
        }
    }

    let listed = conversation_list(&json!({
        "agent": "cursor",
        "root": dir.to_string_lossy()
    }))
    .unwrap();

    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        sessions[0]["workingDirectory"].as_str(),
        Some(project.to_string_lossy().as_ref()),
        "the composer record is the authoritative project directory"
    );
}

#[test]
fn cursor_adapter_rejects_an_unbounded_recorded_project_directory() {
    let dir = temp_dir("cursor-composer-unbounded-workspace");
    let database = dir.join("state.vscdb");
    let composer_id = "0c0c0c0c-0000-4000-8000-000000000002";
    let bubble_id = "3d3d3d3d-0000-4000-8000-000000000002";
    {
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "CREATE TABLE cursorDiskKV (key TEXT NOT NULL, value BLOB NOT NULL)",
                [],
            )
            .unwrap();
        for (key, value) in [
            (
                format!("composerData:{composer_id}"),
                json!({
                    "composerId": composer_id,
                    "name": "Composer bound to the filesystem root",
                    "createdAt": 1_773_798_000_000i64,
                    "lastUpdatedAt": 1_773_798_400_000i64,
                    "workspaceIdentifier": {
                        "uri": { "scheme": "file", "path": "/" }
                    },
                    "fullConversationHeadersOnly": [
                        { "bubbleId": bubble_id, "type": 1 }
                    ]
                }),
            ),
            (
                format!("bubbleId:{composer_id}:{bubble_id}"),
                json!({
                    "bubbleId": bubble_id,
                    "type": 1,
                    "createdAt": 1_773_798_000_000i64,
                    "text": "Residual workspace record."
                }),
            ),
        ] {
            connection
                .execute(
                    "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                    rusqlite::params![key, serde_json::to_vec(&value).unwrap()],
                )
                .unwrap();
        }
    }

    let listed = conversation_list(&json!({
        "agent": "cursor",
        "root": dir.to_string_lossy()
    }))
    .unwrap();

    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert!(
        sessions[0].get("workingDirectory").is_none(),
        "the filesystem root must never become a bindable project directory"
    );
}

#[test]
fn cursor_cli_transcripts_fold_delegated_tasks_and_inherit_the_project_directory() {
    let dir = temp_dir("cursor-cli-transcript-lineage");
    let project_root = dir.join("project-beta");
    let project = dir.join("workspace-beta");
    fs::create_dir_all(&project).unwrap();
    let conversation_id = "4e4e4e4e-0000-4000-8000-000000000001";
    let task_id = "5f5f5f5f-0000-4000-8000-000000000001";
    let conversation_dir = project_root.join("agent-transcripts").join(conversation_id);
    fs::create_dir_all(conversation_dir.join("subagents")).unwrap();
    // A trust marker above the project root must not be inherited: Cursor writes
    // one with the filesystem root as its workspace.
    fs::write(
        dir.join(".workspace-trusted"),
        json!({ "workspacePath": "/" }).to_string(),
    )
    .unwrap();
    fs::write(
        project_root.join(".workspace-trusted"),
        json!({ "workspacePath": project.to_string_lossy() }).to_string(),
    )
    .unwrap();
    fs::write(
        conversation_dir.join(format!("{conversation_id}.jsonl")),
        [
            json!({"role": "user", "message": {"content": [{"type": "text", "text": "Audit the parser"}]}}),
            json!({"role": "assistant", "message": {"content": [{"type": "text", "text": "Audit finished"}]}}),
        ]
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n"),
    )
    .unwrap();
    fs::write(
        conversation_dir
            .join("subagents")
            .join(format!("{task_id}.jsonl")),
        [
            json!({"role": "user", "message": {"content": [{"type": "text", "text": "Map the scan pipeline"}]}}),
            json!({"role": "assistant", "message": {"content": [{"type": "text", "text": "Pipeline mapped"}]}}),
        ]
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n"),
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "cursor",
        "root": dir.to_string_lossy(),
        "historyRootKind": "cursor-cli-projects"
    }))
    .unwrap();

    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(
        sessions.len(),
        1,
        "a delegated task transcript must not become its own conversation"
    );
    let session = &sessions[0];
    assert_eq!(session["nativeSessionId"], conversation_id);
    assert_eq!(
        session["workingDirectory"].as_str(),
        Some(project.to_string_lossy().as_ref()),
        "only the project root trust marker describes the conversation"
    );
    let cards = session["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["cardType"] == "subagent")
        .collect::<Vec<_>>();
    assert_eq!(cards.len(), 1, "expected one delegated task card");
    assert_eq!(cards[0]["cardTitle"], "Map the scan pipeline");
}

#[test]
fn cursor_conversation_recorded_in_several_stores_collapses_to_one_session() {
    let dir = temp_dir("cursor-multi-store-identity");
    let conversation_id = "6a6a6a6a-0000-4000-8000-000000000001";
    let project = dir.join("workspace-gamma");
    fs::create_dir_all(&project).unwrap();

    // The IDE store knows the project directory but keeps fewer messages.
    let database = dir.join("state.vscdb");
    let bubble_id = "7b7b7b7b-0000-4000-8000-000000000001";
    {
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "CREATE TABLE cursorDiskKV (key TEXT NOT NULL, value BLOB NOT NULL)",
                [],
            )
            .unwrap();
        for (key, value) in [
            (
                format!("composerData:{conversation_id}"),
                json!({
                    "composerId": conversation_id,
                    "name": "Recorded twice",
                    "createdAt": 1_773_798_000_000i64,
                    "lastUpdatedAt": 1_773_798_100_000i64,
                    "workspaceIdentifier": {
                        "uri": { "scheme": "file", "path": project.to_string_lossy() }
                    },
                    "fullConversationHeadersOnly": [
                        { "bubbleId": bubble_id, "type": 1 }
                    ]
                }),
            ),
            (
                format!("bubbleId:{conversation_id}:{bubble_id}"),
                json!({
                    "bubbleId": bubble_id,
                    "type": 1,
                    "createdAt": 1_773_798_000_000i64,
                    "text": "Only the opening turn."
                }),
            ),
        ] {
            connection
                .execute(
                    "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                    rusqlite::params![key, serde_json::to_vec(&value).unwrap()],
                )
                .unwrap();
        }
    }

    // The CLI project tree keeps the full transcript but no project record.
    let conversation_dir = dir
        .join("projects-tree")
        .join("agent-transcripts")
        .join(conversation_id);
    fs::create_dir_all(&conversation_dir).unwrap();
    fs::write(
        conversation_dir.join(format!("{conversation_id}.jsonl")),
        [
            json!({"role": "user", "message": {"content": [{"type": "text", "text": "Only the opening turn."}]}}),
            json!({"role": "assistant", "message": {"content": [{"type": "text", "text": "And the reply."}]}}),
            json!({"role": "user", "message": {"content": [{"type": "text", "text": "And a follow-up."}]}}),
        ]
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n"),
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "cursor",
        "root": dir.to_string_lossy()
    }))
    .unwrap();

    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(
        sessions.len(),
        1,
        "one conversation recorded in two stores is one conversation"
    );
    let session = &sessions[0];
    assert_eq!(session["nativeSessionId"], conversation_id);
    assert_eq!(
        session["messages"].as_array().unwrap().len(),
        3,
        "the richest recorded copy wins"
    );
    assert_eq!(
        session["workingDirectory"].as_str(),
        Some(project.to_string_lossy().as_ref()),
        "the project directory is carried over from the copy that knows it"
    );
}

#[test]
fn openagent_store_without_agent_or_token_columns_still_yields_its_conversations() {
    let dir = temp_dir("openagent-narrow-schema");
    let database = dir.join("kilo.db");
    {
        let connection = Connection::open(&database).unwrap();
        // The shipped Kilo Code and OpenCode schema has no `agent`, `model`, or
        // token columns at all.
        connection
            .execute(
                "CREATE TABLE session (
                    id TEXT PRIMARY KEY,
                    project_id TEXT NOT NULL,
                    parent_id TEXT,
                    slug TEXT,
                    directory TEXT NOT NULL,
                    title TEXT NOT NULL,
                    time_created INTEGER NOT NULL,
                    time_updated INTEGER NOT NULL,
                    time_archived INTEGER
                )",
                [],
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
                "CREATE TABLE part (
                    id TEXT PRIMARY KEY,
                    message_id TEXT NOT NULL,
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
                "INSERT INTO session VALUES ('ses_narrow', 'prj_1', NULL, 'slug', \
                 '/workspace/narrow', 'Narrow schema prompt', 1787616000000, 1787616060000, NULL)",
                [],
            )
            .unwrap();
        for (message_id, part_id, role, text) in [
            ("msg_1", "prt_1", "user", "Narrow schema prompt"),
            ("msg_2", "prt_2", "assistant", "Narrow schema reply"),
        ] {
            connection
                .execute(
                    "INSERT INTO message VALUES (?1, 'ses_narrow', 1787616000000, 1787616000000, ?2)",
                    (message_id, json!({"role": role}).to_string()),
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO part VALUES (?1, ?2, 'ses_narrow', 1787616000000, 1787616000000, ?3)",
                    (
                        part_id,
                        message_id,
                        json!({"type": "text", "text": text}).to_string(),
                    ),
                )
                .unwrap();
        }
    }

    let listed = conversation_list(&json!({
        "agent": "kilo-code",
        "root": dir.to_string_lossy()
    }))
    .unwrap();

    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["nativeSessionId"], "ses_narrow");
    assert_eq!(
        sessions[0]["workingDirectory"], "/workspace/narrow",
        "the narrow schema still records the project directory"
    );
    let messages = sessions[0]["messages"].as_array().unwrap();
    assert_eq!(
        messages.len(),
        2,
        "a narrower schema must not silently drop the conversation content"
    );
    assert_eq!(messages[0]["text"], "Narrow schema prompt");
}

#[test]
fn openagent_store_never_reports_an_unbounded_project_directory() {
    let dir = temp_dir("openagent-unbounded-directory");
    let database = dir.join("kilo.db");
    create_openagent_fixture_database(&database, "Unbounded prompt");
    {
        let connection = Connection::open(&database).unwrap();
        connection
            .execute("UPDATE session SET directory = '/'", [])
            .unwrap();
    }

    let listed = conversation_list(&json!({
        "agent": "kilo-code",
        "root": dir.to_string_lossy()
    }))
    .unwrap();

    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert!(
        sessions[0].get("workingDirectory").is_none(),
        "the filesystem root must never become a bindable project directory"
    );
}
