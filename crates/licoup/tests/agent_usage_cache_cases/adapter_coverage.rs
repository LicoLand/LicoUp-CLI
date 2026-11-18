use super::support::*;

fn scan_agent(home: &PathBuf, state: &PathBuf, agent: &str) -> Value {
    agent_usage::scan(&json!({
        "agent": agent,
        "homeDir": home.to_string_lossy(),
        "stateRoot": state.to_string_lossy(),
        "forceRefresh": true,
        "historyDays": 30,
        "now": "2026-07-15T12:00:00Z"
    }))
    .unwrap()
}

#[test]
fn native_adapters_prefer_exact_metadata_from_bounded_standard_stores() {
    let home = temp_dir("native-adapter-coverage-home");
    let state = temp_dir("native-adapter-coverage-state");

    let antigravity_logs =
        home.join(".gemini/antigravity-cli/brain/session/.system_generated/logs");
    fs::create_dir_all(&antigravity_logs).unwrap();
    fs::write(
        antigravity_logs.join("transcript.jsonl"),
        json!({
            "type": "step.completed",
            "timestamp": "2026-07-15T10:00:00Z",
            "sessionId": "antigravity-session",
            "model": "gemini-test",
            "usageMetadata": {
                "promptTokenCount": 100,
                "cachedContentTokenCount": 40,
                "candidatesTokenCount": 12,
                "totalTokenCount": 112
            }
        })
        .to_string()
            + "\n",
    )
    .unwrap();
    fs::write(
        home.join(".gemini/antigravity-cli/brain/session/conversation.json"),
        json!({
            "id": "antigravity-session",
            "createdAt": "2026-07-15T09:00:00Z",
            "messages": [
                {"role": "user", "text": "conversation fallback must not overlap native counters", "createdAt": "2026-07-15T09:00:00Z"},
                {"role": "assistant", "text": "native metadata wins for the complete calendar day", "createdAt": "2026-07-15T09:00:01Z"}
            ]
        })
        .to_string(),
    )
    .unwrap();
    let antigravity = scan_agent(&home, &state, "antigravity");
    assert_eq!(antigravity["agents"][0]["history"]["totalTokens"], 112);
    assert_eq!(antigravity["agents"][0]["confidence"], "high");
    assert_eq!(
        antigravity["agents"][0]["history"]["scanCache"]["discoveredSources"],
        2
    );

    let copilot_session = home.join(".copilot/session-state/session");
    fs::create_dir_all(&copilot_session).unwrap();
    fs::write(
        copilot_session.join("events.jsonl"),
        json!({
            "type": "assistant.message",
            "data": {
                "timestamp": "2026-07-15T10:15:00Z",
                "sessionId": "copilot-session",
                "model": "copilot-test",
                "usage": {"input_tokens": 20, "output_tokens": 5}
            }
        })
        .to_string()
            + "\n",
    )
    .unwrap();
    let copilot = scan_agent(&home, &state, "copilot");
    assert_eq!(copilot["agents"][0]["history"]["totalTokens"], 25);
    assert_eq!(copilot["agents"][0]["confidence"], "high");

    // Cursor's local stores are not a billing ledger: bubble counters in a
    // workspace database never become reported usage, and an unavailable
    // hosted session fails closed with a sanitized warning.
    let cursor_workspace =
        home.join("Library/Application Support/Cursor/User/workspaceStorage/workspace");
    fs::create_dir_all(&cursor_workspace).unwrap();
    let cursor_database = cursor_workspace.join("state.vscdb");
    let connection = SqliteConnection::open(&cursor_database).unwrap();
    connection
        .execute_batch("CREATE TABLE cursorDiskKV (key TEXT NOT NULL, value BLOB NOT NULL);")
        .unwrap();
    connection
        .execute(
            "INSERT INTO cursorDiskKV VALUES(?1,?2)",
            rusqlite::params![
                "bubbleId:cursor-session:bubble",
                serde_json::to_vec(&json!({
                    "createdAt": 1_784_080_800_000i64,
                    "modelInfo": {"modelName": "cursor-test"},
                    "tokenCount": {"inputTokens": 30, "outputTokens": 7}
                }))
                .unwrap()
            ],
        )
        .unwrap();
    drop(connection);
    fs::write(cursor_workspace.join("conversation.json"), b"{}").unwrap();
    let cursor = scan_agent(&home, &state, "cursor");
    assert_eq!(cursor["agents"][0]["history"]["totalTokens"], 0);
    assert_eq!(
        cursor["agents"][0]["history"]["source"],
        "cursor-hosted-usage-events"
    );
    assert!(
        cursor["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| {
                warning["code"] == "cursor_auth_token_unreadable" && warning["agentId"] == "cursor"
            })
    );

    fs::remove_dir_all(home).unwrap();
    fs::remove_dir_all(state).unwrap();
}
