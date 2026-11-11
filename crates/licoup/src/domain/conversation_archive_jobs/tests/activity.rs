use super::super::request::display_path;
use super::support::{archive_job_fixture, create_planned};
use crate::platform::client_state::ClientStateStore;
use serde_json::json;

#[test]
fn activity_record_stays_in_selected_local_state_root() {
    let (state, home, archive_root) = archive_job_fixture("activity-state", "Local activity only");
    create_planned(json!({
        "stateRoot": display_path(&state),
        "homeDir": display_path(&home),
        "agent": "codex",
        "selectionMode": "exact-keyword",
        "query": "Local activity only",
        "path": display_path(&archive_root),
    }))
    .unwrap();

    let store = ClientStateStore::new(state.clone()).unwrap();
    let log = store.activity_log();
    let listed = log.list(&json!({})).unwrap();
    assert_eq!(listed["events"].as_array().unwrap().len(), 1);
    assert_eq!(
        listed["events"][0]["type"],
        "conversation_archive_jobs.created"
    );
    assert!(state.join("activity/activity.jsonl").is_file());
    assert_eq!(listed["path"], "activity/activity.jsonl");
    assert!(!listed.to_string().contains(state.to_str().unwrap()));
}
