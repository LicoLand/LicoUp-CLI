use super::super::request::display_path;
use super::super::{create, list, preview};
use super::support::{create_planned, temp_dir};
use crate::platform::client_state::ClientStateStore;
use serde_json::json;
use std::fs;

#[test]
fn create_job_persists_target_scan_and_queued_state() {
    let state = temp_dir("create-state");
    let home = temp_dir("create-home");
    let history = temp_dir("create-history");
    fs::write(
        history.join("history.jsonl"),
        r#"{"sessionId":"job-create","role":"user","content":"Durable job create"}"#,
    )
    .unwrap();
    let store = ClientStateStore::new(state.clone()).unwrap();
    store
        .write_collection(
            "targets",
            json!({
                "items": [{
                    "target": "codex",
                    "manual": true,
                    "historyRoots": [display_path(&history)]
                }]
            }),
        )
        .unwrap();

    let result = create_planned(json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "agent": "codex",
        "selectionMode": "exact-keyword",
        "query": "Durable job",
        "path": display_path(&temp_dir("create-archive"))
    }))
    .unwrap();

    assert_eq!(result["status"], "queued");
    let candidates = result["targetScan"]["candidates"].as_array().unwrap();
    assert!(!candidates.is_empty());
    let target_ids = candidates
        .iter()
        .filter_map(|candidate| candidate["target"].as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(target_ids.len(), candidates.len());
    let history_path = display_path(&history);
    assert!(candidates.iter().any(|candidate| {
        candidate["target"] == "codex"
            && candidate["historyRoots"]
                .as_array()
                .unwrap()
                .iter()
                .any(|root| root.as_str() == Some(history_path.as_str()))
    }));
    assert!(
        result["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["type"] == "archive.scan.completed")
    );
    let listed = list(&json!({"stateRoot": display_path(&state)})).unwrap();
    assert_eq!(listed["jobs"].as_array().unwrap().len(), 1);
}

#[test]
fn create_requires_the_exact_preview_binding() {
    let state = temp_dir("binding-state");
    let home = temp_dir("binding-home");
    let archive = temp_dir("binding-archive");
    let params = json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "agent": "codex",
        "selectionMode": "all",
        "path": display_path(&archive)
    });
    let plan = preview(&params).unwrap();
    for binding in ["", "sha256:stale"] {
        let mut attempted = params.clone();
        attempted["planBinding"] = json!(binding);
        let error = create(&attempted).unwrap_err().to_string();
        assert!(error.contains("plan"));
    }
    assert_eq!(plan["plan"]["selectionMode"], "all");
    assert!(plan["plan"]["source"].is_object());
    assert!(plan["plan"]["destination"].is_string());
    assert!(plan["plan"]["count"].is_number());
    assert!(plan["plan"]["conflict"].is_boolean());
}
