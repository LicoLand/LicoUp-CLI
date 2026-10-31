use super::super::constants::{CACHE_SCHEMA_VERSION, PARSER_REVISION};
use super::support::{codex_database_path, scan_params, temp_dir, token_event};
use crate::domain::agent_usage;
use rusqlite::Connection;
use serde_json::json;
use std::fs;

#[test]
fn aggregation_keeps_unknown_models_separate_from_explicit_turn_models() {
    let history_root = temp_dir("aggregation-history");
    let state_root = temp_dir("aggregation-state");
    let mut model_event: serde_json::Value =
        serde_json::from_str(&token_event("2026-07-10T10:00:06Z", (78, 2, 4), (7, 0, 0))).unwrap();
    model_event["payload"]["info"]["modelID"] = json!("gpt-5.6");
    fs::write(
        history_root.join("rollout.jsonl"),
        [
            json!({"type":"session_meta","payload":{"id":"session"}}).to_string(),
            token_event("2026-07-10T10:00:01Z", (6, 2, 4), (6, 2, 4)),
            json!({"type":"turn_context","payload":{"turn_id":"one","model":"gpt-5.5"}}).to_string(),
            json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"one"}}).to_string(),
            token_event("2026-07-10T10:00:02Z", (26, 2, 4), (20, 0, 0)),
            json!({"type":"turn_context","payload":{"turn_id":"one"}}).to_string(),
            token_event("2026-07-10T10:00:03Z", (31, 2, 4), (5, 0, 0)),
            json!({"type":"turn_context","payload":{"turn_id":"one","model":"gpt-5.5"}}).to_string(),
            token_event("2026-07-10T10:00:04Z", (41, 2, 4), (10, 0, 0)),
            json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"two"}}).to_string(),
            token_event("2026-07-10T10:00:05Z", (71, 2, 4), (30, 0, 0)),
            model_event.to_string(),
            token_event("2026-07-10T10:00:07Z", (81, 2, 4), (3, 0, 0)),
            json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"two"}}).to_string(),
            token_event("2026-07-10T10:00:08Z", (85, 2, 4), (4, 0, 0)),
            json!({"type":"turn_context","payload":{"turn_id":"three","model":"gpt-5.5"}}).to_string(),
            json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"four"}}).to_string(),
            token_event("2026-07-10T10:00:09Z", (86, 2, 4), (1, 0, 0)),
            json!({
                "timestamp": "2026-07-10T10:00:10Z",
                "type": "response_item",
                "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "abcd"}]}
            })
            .to_string(),
        ]
        .join("\n"),
    )
    .unwrap();

    let mut params = scan_params(&history_root, &state_root);
    let result = agent_usage::scan(&params).unwrap();
    let history = &result["agents"][0]["history"];
    let expected = json!({"Others":50,"gpt-5.5":30,"gpt-5.6":10});
    assert_eq!(history["totalTokens"], 90);
    assert_eq!(history["dailyUsage"][0]["modelUsage"], expected);
    assert_eq!(history["scanCache"]["schemaVersion"], CACHE_SCHEMA_VERSION);
    assert_eq!(history["scanCache"]["parserRevision"], PARSER_REVISION);
    let serialized = result.to_string();
    assert!(!serialized.contains(&history_root.to_string_lossy().to_string()));
    assert!(!serialized.contains("abcd"));
    let connection = Connection::open(codex_database_path(&state_root)).unwrap();
    let models = connection
        .prepare("SELECT model FROM usage_rows ORDER BY event_index")
        .unwrap()
        .query_map([], |row| row.get::<_, Option<String>>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        models,
        [
            None,
            Some("gpt-5.5"),
            None,
            Some("gpt-5.5"),
            None,
            Some("gpt-5.6"),
            Some("gpt-5.6"),
            None,
            None
        ]
        .map(|model| model.map(str::to_owned))
    );
    drop(connection);
    params["now"] = json!("2026-07-11T12:00:00Z");
    let rolled = agent_usage::scan(&params).unwrap();
    assert_eq!(rolled["summary"]["totalTokens"], 90);
    assert_eq!(
        rolled["agents"][0]["history"]["dailyUsage"][0]["modelUsage"],
        expected
    );
}

fn identity_fixture() -> (std::path::PathBuf, std::path::PathBuf, serde_json::Value) {
    let history = temp_dir("identity-source");
    let state = temp_dir("identity-cache");
    let mut today: serde_json::Value =
        serde_json::from_str(&token_event("2026-07-10T10:00:00Z", (30, 6, 8), (20, 4, 5))).unwrap();
    today["payload"]["info"]["model"] = json!("Default");
    today["payload"]["model"] = json!("actual-model");
    fs::write(history.join("rollout.jsonl"), [
        json!({"type":"session_meta","payload":{"id":"synthetic-session"}}).to_string(),
        json!({"type":"turn_context","payload":{"turn_id":"synthetic-turn","model":"actual-model"}}).to_string(),
        token_event("2026-07-09T10:00:00Z", (10, 2, 3), (10, 2, 3)),
        today.to_string(),
    ].join("\n")).unwrap();
    let params = scan_params(&history, &state);
    agent_usage::scan(&params).unwrap();
    (history, state, params)
}

fn identity_ledger(connection: &Connection) -> (String, String) {
    (connection.query_row("SELECT json_group_array(json_array(event_index,session_id,turn_id,day,input_tokens,cached_input_tokens,output_tokens,event_identity,effort,fast)) FROM usage_rows", [], |r| r.get(0)).unwrap(),
     connection.query_row("SELECT json_array(modified_ns,size,file_id,parsed_bytes,append_guard,session_id,forked_from_id,current_turn_id,raw_input,raw_cached,raw_output,counted_input,counted_cached,counted_output,divergent,next_event_index,token_chain_hash,current_effort,current_fast,pending_context) FROM usage_files", [], |r| r.get(0)).unwrap())
}

#[test]
fn force_refresh_updates_only_current_event_models_with_identical_counters() {
    for has_baseline in [true, false] {
        let (history, state, params) = identity_fixture();
        let connection = Connection::open(codex_database_path(&state)).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM usage_rows WHERE day='2026-07-10'",
                    [],
                    |r| r.get::<_, u64>(0)
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM usage_rows WHERE day<'2026-07-10'",
                    [],
                    |r| r.get::<_, u64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM usage_daily_models WHERE day='2026-07-09'",
                    [],
                    |r| r.get::<_, u64>(0)
                )
                .unwrap(),
            1
        );
        if !has_baseline {
            // A broader replay window may stage already sealed historical rows.
            connection.execute("DELETE FROM usage_scans", []).unwrap();
        }
        connection
            .execute(
                "UPDATE usage_rows SET model=?1",
                [json!({"id":"default","providerID":"relay"}).to_string()],
            )
            .unwrap();
        connection
            .execute("UPDATE usage_files SET last_model='Default'", [])
            .unwrap();
        connection
            .execute("UPDATE usage_daily_models SET model='Default'", [])
            .unwrap();
        let before = identity_ledger(&connection);
        let refreshed = agent_usage::scan(&params).unwrap();
        assert_eq!(
            refreshed["agents"][0]["history"]["scanCache"]["identityRefreshedFiles"],
            1
        );
        assert_eq!(identity_ledger(&connection), before);
        assert_eq!(
            connection
                .query_row("SELECT model FROM usage_rows", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "actual-model"
        );
        assert_eq!(
            connection
                .query_row("SELECT model FROM usage_daily_models", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "Default"
        );
        let again = agent_usage::scan(&params).unwrap();
        assert_eq!(
            again["summary"]["totalTokens"],
            refreshed["summary"]["totalTokens"]
        );
        assert_eq!(
            again["agents"][0]["history"]["scanCache"]["identityRefreshedFiles"],
            0
        );
        assert_eq!(identity_ledger(&connection), before);
        drop(connection);
        fs::remove_dir_all(history).unwrap();
        fs::remove_dir_all(state).unwrap();
    }
}

#[test]
fn force_refresh_preserves_rows_when_cached_components_or_event_identity_disagree() {
    for reason in ["cached", "event", "count"] {
        let (history, state, params) = identity_fixture();
        let connection = Connection::open(codex_database_path(&state)).unwrap();
        connection
            .execute("UPDATE usage_rows SET model='Default'", [])
            .unwrap();
        match reason {
            "cached" => {
                connection
                    .execute(
                        "UPDATE usage_rows SET cached_input_tokens=cached_input_tokens+1",
                        [],
                    )
                    .unwrap();
            }
            "event" => {
                connection
                    .execute(
                        "UPDATE usage_rows SET event_identity='different-synthetic-event'",
                        [],
                    )
                    .unwrap();
            }
            _ => {
                connection.execute_batch("INSERT INTO usage_rows SELECT root_key,source_key,event_index+99,session_id,turn_id,day,model,input_tokens,cached_input_tokens,output_tokens,event_identity||'-extra',effort,fast FROM usage_rows;").unwrap();
            }
        }
        let before = identity_ledger(&connection);
        let refreshed = agent_usage::scan(&params).unwrap();
        assert_eq!(
            refreshed["agents"][0]["history"]["scanCache"]["identitySkippedFiles"], 1,
            "{reason}"
        );
        assert_eq!(identity_ledger(&connection), before, "{reason}");
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM usage_rows WHERE model<>'Default'",
                    [],
                    |r| r.get::<_, u64>(0)
                )
                .unwrap(),
            0
        );
        drop(connection);
        fs::remove_dir_all(history).unwrap();
        fs::remove_dir_all(state).unwrap();
    }
}
