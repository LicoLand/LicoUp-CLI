use super::test_support::*;

#[test]
fn kimi_code_history_roots_are_isolated_from_desktop_history() {
    let home = temp_dir("history-kimi-code-roots");
    let custom = home.join("custom-kimi-code");

    let default_roots = history_roots(
        HistoryAdapter::KimiCode,
        &json!({"homeDir": display_path(&home)}),
    );
    assert_eq!(default_roots[0].path, home.join(".kimi-code/sessions"));
    assert_eq!(default_roots[0].source_kind, "kimi-code-session-store");
    assert_eq!(default_roots.len(), 1);

    let custom_roots = history_roots(
        HistoryAdapter::KimiCode,
        &json!({
            "homeDir": display_path(&home),
            "kimiCodeHome": display_path(&custom),
        }),
    );
    assert_eq!(custom_roots[0].path, custom.join("sessions"));
    assert!(HistoryAdapter::KimiCode.accepts_file(
        &custom.join("sessions/wd/session/agents/main/wire.jsonl"),
        "jsonl",
    ));
    assert!(
        !HistoryAdapter::KimiCode
            .accepts_file(&custom.join("sessions/wd/session/state.json"), "json",)
    );
}

#[test]
fn kimi_code_wire_usage_records_preserve_model_and_exact_token_fields() {
    let root = temp_dir("kimi-code-wire");
    let wire = root.join("wd_project/session-1/agents/main/wire.jsonl");
    fs::create_dir_all(wire.parent().unwrap()).unwrap();
    fs::write(
        &wire,
        [
            r#"{"type":"metadata","protocol_version":1}"#,
            r#"{"type":"context.append_message","time":1780912800000,"message":{"role":"user","content":"Review the synthetic Kimi Code fixture"}}"#,
            r#"{"type":"usage.record","time":1780912801000,"model":"kimi-code/kimi-for-coding","usageScope":"turn","usage":{"inputOther":100,"inputCacheRead":20,"inputCacheCreation":5,"output":30}}"#,
            r#"{"type":"usage.record","time":1780912802000,"model":"kimi-code/kimi-for-coding","usageScope":"session","usage":{"inputOther":9999,"inputCacheRead":0,"inputCacheCreation":0,"output":9999}}"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "kimi-code",
        "root": display_path(&root),
    }))
    .unwrap();
    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["adapterId"], "kimi-code");
    assert_eq!(sessions[0]["adapterLabel"], "Kimi Code CLI");
    assert_eq!(sessions[0]["nativeSessionId"], "session-1");

    let messages = sessions[0]["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| {
        message["role"] == "user" && message["text"] == "Review the synthetic Kimi Code fixture"
    }));
    let usage = messages
        .iter()
        .find(|message| message["sourceEventType"] == "usage.record")
        .unwrap();
    assert_eq!(usage["model"], "kimi-code/kimi-for-coding");
    assert_eq!(usage["usage"]["promptTokens"], 125);
    assert_eq!(usage["usage"]["cachedInputTokens"], 20);
    assert_eq!(usage["usage"]["completionTokens"], 30);
    assert_eq!(usage["usage"]["totalTokens"], 155);
    assert_eq!(
        messages
            .iter()
            .filter(|message| message["sourceEventType"] == "usage.record")
            .count(),
        1,
    );
}

#[test]
fn kimi_code_status_update_preserves_native_token_usage() {
    let root = temp_dir("kimi-code-status-usage");
    let wire = root.join("wd_project/session-status/agents/main/wire.jsonl");
    fs::create_dir_all(wire.parent().unwrap()).unwrap();
    fs::write(
        &wire,
        [
            r#"{"type":"context.append_message","time":"2026-07-10T09:59:59Z","message":{"role":"user","content":"Check exact status usage"}}"#,
            r#"{"type":"StatusUpdate","time":"2026-07-10T10:00:00Z","model":"kimi-status-model","token_usage":{"input_other":80,"input_cache_read":20,"input_cache_creation":5,"output":15}}"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "kimi-code",
        "root": display_path(&root),
    }))
    .unwrap();
    let message = listed["sessions"][0]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["sourceEventType"] == "StatusUpdate")
        .unwrap();
    assert_eq!(message["sourceEventType"], "StatusUpdate");
    assert_eq!(message["model"], "kimi-status-model");
    assert_eq!(message["usage"]["promptTokens"], 105);
    assert_eq!(message["usage"]["cachedInputTokens"], 20);
    assert_eq!(message["usage"]["completionTokens"], 15);
    assert_eq!(message["usage"]["totalTokens"], 120);
}

#[test]
fn kimi_code_subagent_wires_collapse_into_main_session_cards() {
    let root = temp_dir("kimi-code-subagent-merge");
    let session_root = root.join("wd_project/session-9");
    fs::create_dir_all(session_root.join("agents/main")).unwrap();
    fs::create_dir_all(session_root.join("agents/agent-0")).unwrap();
    fs::create_dir_all(session_root.join("agents/agent-1")).unwrap();
    fs::write(
        session_root.join("state.json"),
        json!({
            "title": "Synthetic parent session",
            "agents": {
                "main": {"type": "main"},
                "agent-0": {"type": "sub", "parentAgentId": "main", "swarmItem": "Survey the first synthetic subtask"},
                "agent-1": {"type": "sub", "parentAgentId": "main"}
            }
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        session_root.join("agents/main/wire.jsonl"),
        [
            r#"{"type":"turn.prompt","turnId":"turn-1","time":"2026-07-10T00:00:00Z","input":"Parent synthetic prompt"}"#,
            r#"{"type":"context.append_loop_event","turnId":"turn-1","time":"2026-07-10T00:00:03Z","event":{"type":"content.part","step":1,"part":{"type":"text","text":"Parent answer"}}}"#,
        ]
        .join("\n"),
    )
    .unwrap();
    for (agent, reply) in [
        ("agent-0", "First subtask done"),
        ("agent-1", "Second subtask done"),
    ] {
        fs::write(
            session_root.join(format!("agents/{agent}/wire.jsonl")),
            [
                r#"{"type":"turn.prompt","turnId":"turn-1","time":"2026-07-10T00:00:01Z","input":"Subtask synthetic prompt"}"#,
                &format!(
                    r#"{{"type":"context.append_loop_event","turnId":"turn-1","time":"2026-07-10T00:00:02Z","event":{{"type":"content.part","step":1,"part":{{"type":"text","text":"{reply}"}}}}}}"#
                ),
            ]
            .join("\n"),
        )
        .unwrap();
    }

    let listed = conversation_list(&json!({
        "agent": "kimi-code",
        "root": display_path(&root),
    }))
    .unwrap();
    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["nativeSessionId"], "session-9");
    assert!(sessions[0].get("delegatedSubagent").is_none());
    let cards = sessions[0]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "subagent")
        .collect::<Vec<_>>();
    assert_eq!(cards.len(), 2);
    let titles = cards
        .iter()
        .map(|card| card["cardTitle"].as_str().unwrap_or_default())
        .collect::<BTreeSet<_>>();
    // A delegated agent with no declared swarm label is titled by its own task
    // instruction rather than a generic placeholder.
    assert_eq!(
        titles,
        BTreeSet::from([
            "Subtask synthetic prompt",
            "Survey the first synthetic subtask"
        ])
    );
    let titled = cards
        .iter()
        .find(|card| card["cardTitle"] == "Survey the first synthetic subtask")
        .unwrap();
    assert!(titled["messages"].as_array().unwrap().is_empty());
    let child = conversation_list(&json!({"agent": "kimi-code", "root": display_path(&root), "sessionId": titled["childSessionId"]})).unwrap();
    assert!(
        child["sessions"][0]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["role"] == "agent" && message["text"] == "First subtask done")
    );
}

#[test]
fn kimi_code_subagent_wire_stays_standalone_when_parent_wire_missing() {
    let root = temp_dir("kimi-code-subagent-orphan");
    let session_root = root.join("wd_project/session-orphan");
    fs::create_dir_all(session_root.join("agents/agent-0")).unwrap();
    fs::write(
        session_root.join("agents/agent-0/wire.jsonl"),
        [
            r#"{"type":"turn.prompt","turnId":"turn-1","time":"2026-07-10T00:00:01Z","input":"Orphan synthetic prompt"}"#,
            r#"{"type":"context.append_loop_event","turnId":"turn-1","time":"2026-07-10T00:00:02Z","event":{"type":"content.part","step":1,"part":{"type":"text","text":"Orphan reply"}}}"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "kimi-code",
        "root": display_path(&root),
    }))
    .unwrap();
    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["nativeSessionId"], "session-orphan:agent-0");
    assert!(sessions[0].get("delegatedSubagent").is_none());
}

#[test]
fn kimi_code_wire_readback_preserves_session_and_structured_order() {
    let root = temp_dir("kimi-code-structured-wire");
    let session_root = root.join("work-key/native-session-42");
    let wire = session_root.join("agents/main/wire.jsonl");
    fs::create_dir_all(wire.parent().unwrap()).unwrap();
    fs::write(
        session_root.join("state.json"),
        r#"{"title":"Synthetic Kimi Code session","workDir":"/workspace/kimi-project"}"#,
    )
    .unwrap();
    let reasoning_canary = "PRIVATE_REASONING_CANARY";
    let argument_canary = "api_key=PRIVATE_ARGUMENT_CANARY";
    fs::write(
        &wire,
        [
            r#"{"type":"turn.prompt","turnId":"turn-1","time":"2026-07-10T00:00:00Z","input":"Kimi Code synthetic prompt"}"#,
            &format!(r#"{{"type":"context.append_loop_event","turnId":"turn-1","time":"2026-07-10T00:00:01Z","event":{{"type":"content.part","step":1,"part":{{"type":"think","think":"{reasoning_canary} "}}}}}}"#),
            r#"{"type":"context.append_loop_event","turnId":"turn-1","time":"2026-07-10T00:00:01Z","event":{"type":"content.part","step":1,"part":{"type":"think","think":"second private chunk"}}}"#,
            &format!(r#"{{"type":"context.append_loop_event","turnId":"turn-1","time":"2026-07-10T00:00:02Z","event":{{"type":"tool.call","name":"exec","arguments":{{"command":"{argument_canary}"}}}}}}"#),
            r#"{"type":"context.append_loop_event","turnId":"turn-1","time":"2026-07-10T00:00:03Z","event":{"type":"tool.result","name":"exec","result":"completed"}}"#,
            r#"{"type":"context.append_loop_event","turnId":"turn-1","time":"2026-07-10T00:00:04Z","event":{"type":"content.part","step":2,"part":{"type":"text","text":"Final "}}}"#,
            r#"{"type":"context.append_loop_event","turnId":"turn-1","time":"2026-07-10T00:00:04Z","event":{"type":"content.part","step":2,"part":{"type":"text","text":"answer"}}}"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let listed = conversation_list(&json!({
        "agent": "kimi-code",
        "root": display_path(&root),
    }))
    .unwrap();
    let sessions = listed["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["adapterId"], "kimi-code");
    assert_eq!(sessions[0]["adapterLabel"], "Kimi Code CLI");
    assert_eq!(sessions[0]["nativeSessionId"], "native-session-42");
    assert_eq!(sessions[0]["workingDirectory"], "/workspace/kimi-project");

    let messages = sessions[0]["messages"].as_array().unwrap();
    let roles = messages
        .iter()
        .map(|message| message["role"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(
        roles,
        vec!["user", "reasoning", "tool_call", "tool_result", "agent"]
    );
    assert_eq!(messages[0]["text"], "Kimi Code synthetic prompt");
    assert_eq!(
        messages[1]["text"],
        "PRIVATE_REASONING_CANARY second private chunk"
    );
    assert!(messages[1].get("providerSummary").is_none());
    assert_eq!(messages[2]["cardType"], "tool-call");
    assert_eq!(messages[2]["text"], "command: api_key: [redacted]");
    assert_eq!(messages[3]["cardType"], "tool-result");
    assert_eq!(messages[4]["text"], "Final answer");
    assert_eq!(
        messages
            .iter()
            .filter(|message| message["role"] == "reasoning")
            .count(),
        1
    );
    assert_eq!(
        messages
            .iter()
            .filter(|message| message["role"] == "agent")
            .count(),
        1
    );
    let serialized = serde_json::to_string(messages).unwrap();
    // Reasoning detail is shown locally, but secret argument values stay
    // redacted in the projected tool call.
    assert!(!serialized.contains(argument_canary));
}

#[test]
fn kimi_code_missing_timestamps_use_stable_source_order_not_epoch() {
    let root = temp_dir("kimi-code-stable-source-order");
    let wire = root.join("work-key/native-session-order/agents/main/wire.jsonl");
    fs::create_dir_all(wire.parent().unwrap()).unwrap();
    fs::write(
        &wire,
        [
            r#"{"type":"turn.prompt","turnId":"turn-1","input":"First untimed prompt"}"#,
            r#"{"type":"context.append_loop_event","turnId":"turn-1","event":{"type":"content.part","step":1,"part":{"type":"text","text":"Untimed reply"}}}"#,
        ]
        .join("\n"),
    )
    .unwrap();

    let read = || {
        conversation_list(&json!({
            "agent": "kimi-code",
            "root": display_path(&root),
            "sessionId": "native-session-order",
            "messageLimit": 50
        }))
        .unwrap()["sessions"][0]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|message| message["createdAt"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    let first = read();
    let second = read();
    assert_eq!(first, second);
    assert!(
        first
            .iter()
            .all(|timestamp| !timestamp.starts_with("1970-01-01"))
    );
    assert!(first.windows(2).all(|pair| pair[0] <= pair[1]));
}
