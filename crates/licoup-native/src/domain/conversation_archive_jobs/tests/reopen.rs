use super::super::request::display_path;
use super::super::{events, list, status};
use super::support::{archive_job_fixture, create_planned};
use serde_json::json;

#[test]
fn status_list_events_survive_store_reopen() {
    let (state, home, archive_root) = archive_job_fixture("restart", "Durable restart");
    let created = create_planned(json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "agent": "codex",
        "selectionMode": "exact-keyword",
        "query": "Durable restart",
        "path": display_path(&archive_root)
    }))
    .unwrap();
    let job_id = created["jobId"].as_str().unwrap().to_string();
    drop(created);

    let status = status(&json!({"stateRoot": display_path(&state), "jobId": job_id})).unwrap();
    assert_eq!(status["status"], "queued");
    let list = list(&json!({"stateRoot": display_path(&state)})).unwrap();
    assert_eq!(list["jobs"].as_array().unwrap().len(), 1);
    let events = events(&json!({"stateRoot": display_path(&state), "jobId": job_id})).unwrap();
    assert!(
        events["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["type"] == "archive.job.queued")
    );
}
