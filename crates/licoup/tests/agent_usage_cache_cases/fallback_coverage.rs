use super::support::*;

fn scan_agent(home: &PathBuf, state: &PathBuf, agent: &str, force_refresh: bool) -> Value {
    agent_usage::scan(&json!({
        "agent": agent,
        "homeDir": home.to_string_lossy(),
        "stateRoot": state.to_string_lossy(),
        "forceRefresh": force_refresh,
        "historyDays": 30,
        "now": "2026-07-15T12:00:00Z"
    }))
    .unwrap()
}

fn assert_fallback(report: &Value) {
    assert_eq!(report["agents"][0]["history"]["totalTokens"], 5);
    assert_eq!(report["agents"][0]["confidence"], "low");
}

#[test]
fn native_adapters_cache_estimates_when_native_counters_are_absent() {
    let home = temp_dir("native-adapter-estimate-home");
    let state = temp_dir("native-adapter-estimate-state");

    let antigravity_session = home.join(".gemini/antigravity-cli/brain/session");
    fs::create_dir_all(&antigravity_session).unwrap();
    fs::write(
        antigravity_session.join("conversation.json"),
        json!({
            "id": "antigravity-session",
            "createdAt": "2026-07-15T09:00:00Z",
            "messages": [
                {"role": "user", "text": "abcdefgh", "createdAt": "2026-07-15T09:00:00Z"},
                {"role": "assistant", "text": "abcdefghijkl", "createdAt": "2026-07-15T09:00:01Z"}
            ]
        })
        .to_string(),
    )
    .unwrap();
    assert_fallback(&scan_agent(&home, &state, "antigravity", true));

    let copilot_session = home.join(".copilot/session-state/session");
    fs::create_dir_all(&copilot_session).unwrap();
    fs::write(
        copilot_session.join("events.jsonl"),
        [
            json!({
                "type": "user.message",
                "data": {"timestamp": "2026-07-15T10:00:00Z", "sessionId": "copilot-session", "content": "abcdefgh"}
            })
            .to_string(),
            json!({
                "type": "assistant.message",
                "data": {"timestamp": "2026-07-15T10:00:01Z", "sessionId": "copilot-session", "content": "abcdefghijkl"}
            })
            .to_string(),
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();
    assert_fallback(&scan_agent(&home, &state, "copilot", true));

    // Cursor never estimates from local conversation text: its ledger is
    // hosted, so an unavailable session fails closed instead of inventing
    // tokens from a workspace store.
    let cursor_workspace =
        home.join("Library/Application Support/Cursor/User/workspaceStorage/workspace");
    fs::create_dir_all(&cursor_workspace).unwrap();
    let connection = SqliteConnection::open(cursor_workspace.join("state.vscdb")).unwrap();
    connection
        .execute_batch("CREATE TABLE cursorDiskKV (key TEXT PRIMARY KEY, value BLOB NOT NULL);")
        .unwrap();
    connection
        .execute(
            "INSERT INTO cursorDiskKV VALUES(?1,?2)",
            rusqlite::params![
                "composerData:cursor-session",
                json!({
                    "composerId": "cursor-session",
                    "createdAt": 1_784_080_800_000i64,
                    "modelConfig": {"modelName": "cursor-test"},
                    "fullConversationHeadersOnly": [{"bubbleId": "user"}, {"bubbleId": "assistant"}]
                })
                .to_string()
            ],
        )
        .unwrap();
    for (id, kind, text) in [("user", 1, "abcdefgh"), ("assistant", 2, "abcdefghijkl")] {
        connection
            .execute(
                "INSERT INTO cursorDiskKV VALUES(?1,?2)",
                rusqlite::params![
                    format!("bubbleId:cursor-session:{id}"),
                    json!({"createdAt": 1_784_080_800_000i64, "type": kind, "text": text})
                        .to_string()
                ],
            )
            .unwrap();
    }
    drop(connection);
    let cursor = scan_agent(&home, &state, "cursor", true);
    assert_eq!(cursor["agents"][0]["history"]["totalTokens"], 0);
    assert_eq!(cursor["agents"][0]["confidence"], "unavailable");

    let pi_sessions = home.join(".pi/agent/sessions");
    fs::create_dir_all(&pi_sessions).unwrap();
    fs::write(
        pi_sessions.join("2026-07-15_session.jsonl"),
        [
            json!({"type": "session", "id": "pi-session"}).to_string(),
            json!({
                "type": "message", "timestamp": "2026-07-15T11:00:00Z",
                "message": {"role": "user", "content": "abcdefgh"}
            })
            .to_string(),
            json!({
                "type": "message", "timestamp": "2026-07-15T11:00:01Z",
                "message": {"role": "assistant", "content": "abcdefghijkl"}
            })
            .to_string(),
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();
    assert_fallback(&scan_agent(&home, &state, "pi", true));

    for agent in ["antigravity", "copilot", "pi"] {
        let warm = scan_agent(&home, &state, agent, false);
        assert_fallback(&warm);
        assert_eq!(warm["agents"][0]["history"]["scanCache"]["fresh"], true);
        assert_eq!(warm["agents"][0]["history"]["scanCache"]["parsedBytes"], 0);
    }

    fs::remove_dir_all(home).unwrap();
    fs::remove_dir_all(state).unwrap();
}
