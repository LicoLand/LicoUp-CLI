use super::super::request::display_path;
use super::super::{cancel, drain, status};
use super::support::{archive_job_fixture, create_planned};
use serde_json::json;

#[test]
fn cancelled_job_is_terminal_and_never_drained() {
    let (state, home, archive_root) = archive_job_fixture("cancel", "Cancelled locally");
    let created = create_planned(json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "agent": "codex",
        "selectionMode": "exact-keyword",
        "query": "Cancelled locally",
        "path": display_path(&archive_root),
    }))
    .unwrap();
    let job_id = created["jobId"].as_str().unwrap();

    let cancelled = cancel(&json!({"stateRoot": display_path(&state), "jobId": job_id})).unwrap();
    assert_eq!(cancelled["status"], "cancelled");
    let drained = drain(&json!({"stateRoot": display_path(&state), "jobId": job_id})).unwrap();
    assert_eq!(drained["processed"], 0);
    assert_eq!(
        status(&json!({"stateRoot": display_path(&state), "jobId": job_id})).unwrap()["status"],
        "cancelled"
    );
}
