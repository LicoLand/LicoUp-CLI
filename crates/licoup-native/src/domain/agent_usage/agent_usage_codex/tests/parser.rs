use super::support::{
    codex_database_path, install_v12_fixture_schema, scan_params, temp_dir, token_event,
};
use crate::domain::agent_usage;
use rusqlite::Connection;
use serde_json::json;
use std::fs;
use std::io::Write;

#[test]
fn placeholder_models_use_actual_same_turn_evidence_without_leaking_next_turn() {
    let history_root = temp_dir("placeholder-history");
    let state_root = temp_dir("placeholder-state");
    let token = |index: u64, model: &str, payload_model: Option<&str>, effort: Option<&str>| {
        let mut value: serde_json::Value = serde_json::from_str(&token_event(
            &format!("2026-07-10T10:00:0{index}Z"),
            (index * 8, 0, index * 2),
            (8, 0, 2),
        ))
        .unwrap();
        value["payload"]["info"]["model"] = json!(model);
        if let Some(model) = payload_model {
            value["payload"]["model"] = json!(model);
        }
        if let Some(effort) = effort {
            value["payload"]["info"]["effort"] = json!(effort);
        }
        value
    };
    fs::write(history_root.join("rollout.jsonl"),[
        json!({"type":"session_meta","payload":{"id":"placeholder-session"}}),
        json!({"type":"turn_context","payload":{"turn_id":"one","model":"actual-context","effort":"high"}}),
        json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"one","model":"default"}}),
        token(1,"default",None,None),
        token(2,"unknown",Some("actual-direct"),Some("low")),
        json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"one"}}),
        json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"two","model":"auto","effort":"medium"}}),
        token(3,"default",None,None),
        token(4,"actual-response",None,None),
        json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"three"}}),
        token(5,"default",None,None),
    ].iter().map(serde_json::Value::to_string).collect::<Vec<_>>().join("\n")).unwrap();
    let result = agent_usage::scan(&scan_params(&history_root, &state_root)).unwrap();
    assert_eq!(result["summary"]["totalTokens"], 50);
    let models = &result["agents"][0]["history"]["dailyUsage"][0]["modelTokenUsage"];
    assert_eq!(
        models["actual-context"]["variants"]["High"]["totalTokens"],
        10
    );
    assert_eq!(
        models["actual-direct"]["variants"]["Low"]["totalTokens"],
        10
    );
    assert_eq!(
        models["actual-response"]["variants"]["Medium"]["totalTokens"],
        10
    );
    assert_eq!(models["Others"]["totalTokens"], 20);
    assert_eq!(models["Others"]["variants"]["Medium"]["totalTokens"], 10);
    assert_eq!(
        models["Others"]["unattributedVariantUsage"]["totalTokens"],
        10
    );
    fs::remove_dir_all(history_root).unwrap();
    fs::remove_dir_all(state_root).unwrap();
}

#[test]
fn parser_keeps_actual_variants_separate_and_missing_context_unspecified() {
    let history_root = temp_dir("actual-variants-history");
    let state_root = temp_dir("actual-variants-state");
    let mut model_event: serde_json::Value = serde_json::from_str(&token_event(
        "2026-07-10T10:00:07Z",
        (50, 5, 10),
        (10, 1, 2),
    ))
    .unwrap();
    model_event["payload"]["info"]["model"] = json!("gpt-5.6");
    model_event["payload"]["info"]["reasoning_effort"] = json!("low");
    model_event["payload"]["info"]["fast"] = json!(true);
    fs::write(history_root.join("rollout.jsonl"),[
        json!({"type":"session_meta","payload":{"id":"variant-session"}}).to_string(),
        json!({"type":"turn_context","payload":{"turn_id":"one","model":"gpt-5.5","effort":"medium","service_tier":"fast"}}).to_string(),
        json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"one"}}).to_string(),
        token_event("2026-07-10T10:00:01Z",(10,1,2),(10,1,2)),
        json!({"type":"turn_context","payload":{"turn_id":"one","model":"gpt-5.5"}}).to_string(),
        token_event("2026-07-10T10:00:02Z",(20,2,4),(10,1,2)),
        json!({"type":"turn_context","payload":{"turn_id":"one","model":"gpt-5.5","effort":"xhigh","service_tier":"default"}}).to_string(),
        token_event("2026-07-10T10:00:03Z",(30,3,6),(10,1,2)),
        json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":"one"}}).to_string(),
        json!({"type":"turn_context","payload":{"turn_id":"two","model":"gpt-5.5"}}).to_string(),
        json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"two"}}).to_string(),
        token_event("2026-07-10T10:00:04Z",(40,4,8),(10,1,2)),
        model_event.to_string(),
    ].join("\n")).unwrap();
    let result = agent_usage::scan(&scan_params(&history_root, &state_root)).unwrap();
    assert_eq!(result["summary"]["totalTokens"], 60);
    let models = &result["agents"][0]["history"]["dailyUsage"][0]["modelTokenUsage"];
    assert_eq!(models["gpt-5.5"]["totalTokens"], 48);
    for label in ["Medium Fast", "Extra High"] {
        assert_eq!(models["gpt-5.5"]["variants"][label]["totalTokens"], 12);
    }
    assert_eq!(
        models["gpt-5.5"]["unattributedVariantUsage"]["totalTokens"],
        24
    );
    assert_eq!(models["gpt-5.6"]["variants"]["Low Fast"]["totalTokens"], 12);
    let connection = Connection::open(codex_database_path(&state_root)).unwrap();
    let variants = connection
        .prepare("SELECT effort,fast FROM usage_rows ORDER BY event_index")
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<bool>>(1)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        variants,
        vec![
            (Some("medium".to_owned()), Some(true)),
            (None, None),
            (Some("xhigh".to_owned()), Some(false)),
            (None, None),
            (Some("low".to_owned()), Some(true))
        ]
    );
}

#[test]
fn incremental_context_and_daily_rollup_preserve_variant_without_leaking_next_turn() {
    let history_root = temp_dir("variant-incremental-history");
    let state_root = temp_dir("variant-incremental-state");
    let path = history_root.join("rollout.jsonl");
    fs::write(&path,[
        json!({"type":"session_meta","payload":{"id":"incremental-session"}}).to_string(),
        json!({"type":"turn_context","payload":{"model":"gpt-5.5","collaboration_mode":{"settings":{"reasoning_effort":"high"}},"fast":false}}).to_string(),
        String::new(),
    ].join("\n")).unwrap();
    let mut params = scan_params(&history_root, &state_root);
    assert_eq!(
        agent_usage::scan(&params).unwrap()["summary"]["totalTokens"],
        0
    );
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(
            [
                json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"first"}})
                    .to_string(),
                token_event("2026-07-10T10:00:01Z", (10, 0, 0), (10, 0, 0)),
                String::new(),
            ]
            .join("\n")
            .as_bytes(),
        )
        .unwrap();
    assert_eq!(
        agent_usage::scan(&params).unwrap()["summary"]["totalTokens"],
        10
    );
    fs::OpenOptions::new().append(true).open(&path).unwrap().write_all([
        token_event("2026-07-10T10:00:02Z",(15,0,0),(5,0,0)),
        json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"second"}}).to_string(),
        token_event("2026-07-10T10:00:03Z",(25,0,0),(10,0,0)),
        json!({"type":"turn_context","payload":{"turn_id":"second","model":"gpt-5.5","effort":"low","fast":true}}).to_string(),
        token_event("2026-07-10T10:00:04Z",(30,0,0),(5,0,0)),
        String::new(),
    ].join("\n").as_bytes()).unwrap();
    let appended = agent_usage::scan(&params).unwrap();
    assert_eq!(
        appended["agents"][0]["history"]["scanCache"]["appendedFiles"],
        1
    );
    params["now"] = json!("2026-07-11T12:00:00Z");
    let rolled = agent_usage::scan(&params).unwrap();
    assert_eq!(rolled["summary"]["totalTokens"], 30);
    let variants =
        &rolled["agents"][0]["history"]["dailyUsage"][0]["modelTokenUsage"]["gpt-5.5"]["variants"];
    assert_eq!(variants["High"]["totalTokens"], 15);
    assert_eq!(
        rolled["agents"][0]["history"]["dailyUsage"][0]["modelTokenUsage"]["Others"]["unattributedVariantUsage"]
            ["totalTokens"],
        10
    );
    assert_eq!(variants["Low Fast"]["totalTokens"], 5);
    let connection = Connection::open(codex_database_path(&state_root)).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM usage_rows", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM usage_daily_models", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        3
    );
}

#[test]
fn schema_migration_preserves_sealed_days_and_rebuilds_only_current_usage() {
    for version in [12, 13] {
        let history_root = temp_dir("persistent-upgrade-history");
        let state_root = temp_dir("persistent-upgrade-state");
        let archived = history_root.join("archived.jsonl");
        let live = history_root.join("live.jsonl");
        fs::write(&archived, [
            json!({"type":"turn_context","payload":{"model":"gpt-5.5","effort":"xhigh","fast":true}}).to_string(),
            token_event("2026-07-08T10:00:00Z",(10,2,3),(10,2,3)),
        ].join("\n")).unwrap();
        fs::write(
            &live,
            [
                json!({"type":"turn_context","payload":{"model":"gpt-5.6","effort":"medium"}})
                    .to_string(),
                token_event("2026-07-09T10:00:00Z", (10, 0, 0), (10, 0, 0)),
            ]
            .join("\n"),
        )
        .unwrap();
        let mut params = scan_params(&history_root, &state_root);
        params["now"] = json!("2026-07-09T12:00:00Z");
        assert_eq!(
            agent_usage::scan(&params).unwrap()["summary"]["totalTokens"],
            23
        );
        let database = codex_database_path(&state_root);
        let connection = Connection::open(&database).unwrap();
        if version == 12 {
            install_v12_fixture_schema(&connection);
        } else {
            connection.pragma_update(None, "user_version", 13).unwrap();
        }
        drop(connection);

        // A missing old source and rewritten past content must not replace
        // recorded days. New historical files cannot be mixed into them.
        fs::remove_file(archived).unwrap();
        fs::write(
            &live,
            [
                json!({"type":"turn_context","payload":{"model":"gpt-5.6","effort":"medium"}})
                    .to_string(),
                token_event("2026-07-09T10:00:00Z", (1000, 0, 0), (1000, 0, 0)),
                json!({"type":"turn_context","payload":{"model":"gpt-5.6","effort":"high"}})
                    .to_string(),
                token_event("2026-07-10T10:00:00Z", (1007, 0, 0), (7, 0, 0)),
                String::new(),
            ]
            .join("\n"),
        )
        .unwrap();
        fs::write(
            history_root.join("late-past.jsonl"),
            token_event("2026-07-07T10:00:00Z", (50, 0, 0), (50, 0, 0)),
        )
        .unwrap();
        params["now"] = json!("2026-07-10T12:00:00Z");
        params["forceRefresh"] = json!(false);
        let upgraded = agent_usage::scan(&params).unwrap();
        let days = upgraded["agents"][0]["history"]["dailyUsage"]
            .as_array()
            .unwrap();
        assert_eq!(upgraded["summary"]["totalTokens"], 30);
        assert_eq!(
            days.iter()
                .map(|day| (
                    day["date"].as_str().unwrap(),
                    day["totalTokens"].as_u64().unwrap()
                ))
                .collect::<Vec<_>>(),
            vec![("2026-07-08", 13), ("2026-07-09", 10), ("2026-07-10", 7)]
        );
        assert_eq!(
            days[2]["modelTokenUsage"]["gpt-5.6"]["variants"]["High"]["totalTokens"],
            7
        );
        if version == 12 {
            assert_eq!(
                days[0]["modelTokenUsage"]["gpt-5.5"]["unattributedVariantUsage"]["totalTokens"],
                13
            );
            assert!(
                days[0]["modelTokenUsage"]["gpt-5.5"]["variants"]
                    .as_object()
                    .unwrap()
                    .is_empty()
            );
        } else {
            assert_eq!(
                days[0]["modelTokenUsage"]["gpt-5.5"]["variants"]["Extra High Fast"]["totalTokens"],
                13
            );
        }
        params["forceRefresh"] = json!(true);
        assert_eq!(
            agent_usage::scan(&params).unwrap()["summary"]["totalTokens"],
            30
        );
        fs::OpenOptions::new()
            .append(true)
            .open(&live)
            .unwrap()
            .write_all(token_event("2026-07-10T11:00:00Z", (1012, 0, 0), (5, 0, 0)).as_bytes())
            .unwrap();
        assert_eq!(
            agent_usage::scan(&params).unwrap()["summary"]["totalTokens"],
            35
        );
        let connection = Connection::open(database).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
                .unwrap(),
            super::super::constants::CACHE_SCHEMA_VERSION
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT SUM(explicit_prompt+explicit_completion) FROM usage_daily_totals",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            23
        );
    }
}

#[test]
fn parser_reconciles_explicit_deltas_and_model_context() {
    let history_root = temp_dir("parser-history");
    let state_root = temp_dir("parser-state");
    fs::write(
        history_root.join("rollout.jsonl"),
        [
            r#"{"timestamp":"2026-07-08T10:00:00Z","type":"session_meta","payload":{"id":"session"}}"#.to_string(),
            r#"{"timestamp":"2026-07-08T10:00:01Z","type":"turn_context","payload":{"model":"gpt-test"}}"#.to_string(),
            token_event("2026-07-08T10:00:02Z", (6, 2, 4), (6, 2, 4)),
            token_event("2026-07-08T10:00:03Z", (8, 2, 5), (2, 0, 1)),
        ]
        .join("\n"),
    )
    .unwrap();

    let result = agent_usage::scan(&scan_params(&history_root, &state_root)).unwrap();
    let history = &result["agents"][0]["history"];
    assert_eq!(history["totalTokens"], 13);
    assert_eq!(history["dailyUsage"][0]["modelUsage"]["gpt-test"], 13);
}

#[test]
fn parser_keeps_incomplete_jsonl_suffix_for_the_next_append() {
    let history_root = temp_dir("parser-partial-history");
    let state_root = temp_dir("parser-partial-state");
    fs::write(
        history_root.join("rollout.jsonl"),
        r#"{"timestamp":"2026-07-08T10:00:00Z","type":"event_msg""#,
    )
    .unwrap();
    let result = agent_usage::scan(&scan_params(&history_root, &state_root)).unwrap();
    assert_eq!(result["summary"]["totalTokens"], 0);
}

#[test]
fn parser_rolls_up_all_history_without_losing_windowed_token_deltas() {
    let history_root = temp_dir("parser-retention-history");
    let state_root = temp_dir("parser-retention-state");
    fs::write(
        history_root.join("rollout.jsonl"),
        [
            token_event("2026-01-01T10:00:00Z", (100, 20, 10), (100, 20, 10)),
            token_event("2026-07-08T10:00:00Z", (106, 20, 14), (6, 0, 4)),
        ]
        .join("\n"),
    )
    .unwrap();

    let result = agent_usage::scan(&scan_params(&history_root, &state_root)).unwrap();
    let history = &result["agents"][0]["history"];
    assert_eq!(history["totalTokens"], 10);
    assert_eq!(history["tokenSourceBreakdown"]["explicitRecords"], 1);
    let database_path = codex_database_path(&state_root);
    let connection = Connection::open(database_path).unwrap();
    let retained_rows = connection
        .query_row("SELECT COUNT(*) FROM usage_daily_totals", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    assert_eq!(retained_rows, 2);
}

#[test]
fn current_day_details_are_compacted_after_the_calendar_rolls_over() {
    let history_root = temp_dir("parser-rollover-history");
    let state_root = temp_dir("parser-rollover-state");
    fs::write(
        history_root.join("rollout.jsonl"),
        token_event("2026-07-10T10:00:00Z", (6, 2, 4), (6, 2, 4)),
    )
    .unwrap();
    let mut params = scan_params(&history_root, &state_root);
    let first = agent_usage::scan(&params).unwrap();
    assert_eq!(first["summary"]["totalTokens"], 10);
    let database_path = codex_database_path(&state_root);
    let connection = Connection::open(&database_path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM usage_rows", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM usage_daily_totals", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
    drop(connection);

    params["now"] = serde_json::json!("2026-07-11T12:00:00Z");
    let next_day = agent_usage::scan(&params).unwrap();
    assert_eq!(next_day["summary"]["totalTokens"], 10);
    let connection = Connection::open(database_path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM usage_rows", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM usage_daily_totals", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        1
    );
}
