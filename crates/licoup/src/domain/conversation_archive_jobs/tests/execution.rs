use super::super::request::display_path;
use super::super::{drain, status};
use super::support::{archive_job_fixture, create_planned};
use serde_json::json;

#[test]
fn archive_and_verify_state_machine_reaches_completed() {
    let (state, home, archive_root) = archive_job_fixture("execution-complete", "Durable complete");
    let created = create_planned(json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "agent": "codex",
        "selectionMode": "exact-keyword",
        "query": "Durable complete",
        "path": display_path(&archive_root)
    }))
    .unwrap();
    let job_id = created["jobId"].as_str().unwrap();

    let drained = drain(&json!({
        "stateRoot": display_path(&state),
        "jobId": job_id
    }))
    .unwrap();

    assert_eq!(drained["completed"], 1);
    let status = status(&json!({"stateRoot": display_path(&state), "jobId": job_id})).unwrap();
    assert_eq!(status["status"], "completed");
    assert_eq!(status["attempt"], 1);
    assert_eq!(
        status["validationResult"]["validation"]["healthStatus"],
        "ok"
    );
}
