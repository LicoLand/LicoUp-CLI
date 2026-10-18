use super::super::request::display_path;
use super::super::{drain, list};
use super::support::{archive_job_fixture, create_planned, temp_dir};
use serde_json::json;

#[test]
fn once_drain_processes_only_the_oldest_queued_job() {
    let (state, home, archive_root) = archive_job_fixture("drain-once", "Durable bounded");
    for path in [archive_root, temp_dir("drain-once-second-archive")] {
        create_planned(json!({
            "stateRoot": display_path(&state),
            "homeDir": display_path(&home),
            "agent": "codex",
            "selectionMode": "exact-keyword",
            "query": "Durable bounded",
            "path": display_path(&path)
        }))
        .unwrap();
    }

    let drained = drain(&json!({
        "stateRoot": display_path(&state),
        "once": true
    }))
    .unwrap();

    assert_eq!(drained["processed"], 1);
    let listed = list(&json!({"stateRoot": display_path(&state)})).unwrap();
    let statuses = listed["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|job| job["status"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == "queued")
            .count(),
        1
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == "verifying")
            .count(),
        1
    );
}
