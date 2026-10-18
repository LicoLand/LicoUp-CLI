use super::super::request::display_path;
use super::super::{drain, status};
use super::support::{archive_job_fixture, corrupt_first_raw_content, create_planned};
use serde_json::json;

#[test]
fn verify_failure_schedules_retry_using_same_target_scan() {
    let (state, home, archive_root) =
        archive_job_fixture("verify-retry", "Durable verification retry");
    let created = create_planned(json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "agent": "codex",
        "selectionMode": "exact-keyword",
        "query": "Durable verification retry",
        "path": display_path(&archive_root),
        "maxAttempts": 2
    }))
    .unwrap();
    let job_id = created["jobId"].as_str().unwrap();
    drain(&json!({
        "stateRoot": display_path(&state),
        "jobId": job_id,
        "once": "true"
    }))
    .unwrap();
    corrupt_first_raw_content(&archive_root, "durable-verification-retry");

    let verify = drain(&json!({
        "stateRoot": display_path(&state),
        "jobId": job_id,
        "once": "true"
    }))
    .unwrap();
    assert_eq!(verify["jobs"][0]["outcome"]["status"], "retry_scheduled");
    let first_status =
        status(&json!({"stateRoot": display_path(&state), "jobId": job_id})).unwrap();
    assert_eq!(first_status["attempt"], 1);
    assert_eq!(first_status["targetScan"], created["targetScan"]);
    assert!(
        first_status["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["type"] == "archive.retry.scheduled")
    );

    let completed = drain(&json!({
        "stateRoot": display_path(&state),
        "jobId": job_id
    }))
    .unwrap();
    assert_eq!(completed["completed"], 1);
    let completed_status =
        status(&json!({"stateRoot": display_path(&state), "jobId": job_id})).unwrap();
    assert_eq!(completed_status["status"], "completed");
    assert_eq!(completed_status["attempt"], 2);
    assert_eq!(completed_status["targetScan"], created["targetScan"]);
}

#[test]
fn max_attempts_exhausted_fails_dead_letter_style() {
    let (state, home, archive_root) =
        archive_job_fixture("verify-failed", "Durable permanent failure");
    let created = create_planned(json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "agent": "codex",
        "selectionMode": "exact-keyword",
        "query": "Durable permanent failure",
        "path": display_path(&archive_root),
        "maxAttempts": 1
    }))
    .unwrap();
    let job_id = created["jobId"].as_str().unwrap();
    drain(&json!({
        "stateRoot": display_path(&state),
        "jobId": job_id,
        "once": "true"
    }))
    .unwrap();
    corrupt_first_raw_content(&archive_root, "durable-permanent-failure");

    let drained = drain(&json!({
        "stateRoot": display_path(&state),
        "jobId": job_id,
        "once": "true"
    }))
    .unwrap();

    assert_eq!(drained["failed"], 1);
    let status = status(&json!({"stateRoot": display_path(&state), "jobId": job_id})).unwrap();
    assert_eq!(status["status"], "failed");
    assert!(
        status["events"].as_array().unwrap().iter().any(
            |event| event["type"] == "archive.failed" && event["payload"]["deadLetter"] == true
        )
    );
}
