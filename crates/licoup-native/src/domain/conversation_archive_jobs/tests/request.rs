use super::super::list;
use super::super::request::{local_path_from_user_input, normalize_request};
use serde_json::json;

#[test]
fn create_request_rejects_uri_and_network_share_destinations() {
    let windows_separator = char::from(92).to_string();
    let invalid_destinations = [
        "https://example.invalid/archive".to_owned(),
        ["file:", "", "test-data", "archive"].join("/"),
        ["", "", "host", "share"].join("/"),
        ["", "", "host", "share"].join(&windows_separator),
    ];
    for path in invalid_destinations {
        let error = normalize_request(&json!({
            "selectionMode": "exact-keyword",
            "query": "local",
            "path": path
        }))
        .unwrap_err()
        .to_string();
        assert!(error.contains("local filesystem path"));
    }
}

#[test]
fn relative_destination_is_resolved_to_an_explicit_local_path() {
    let request = normalize_request(&json!({
        "selectionMode": "exact-keyword",
        "query": "local",
        "path": "archive-output"
    }))
    .unwrap();
    assert!(std::path::Path::new(request["path"].as_str().unwrap()).is_absolute());
    assert!(
        local_path_from_user_input("state-output", "state root")
            .unwrap()
            .is_absolute()
    );
}

#[test]
fn create_request_rejects_non_local_or_untyped_state_paths() {
    let non_local_state = ["file:", "", "test-data", "state"].join("/");
    for params in [
        json!({"selectionMode": "exact-keyword", "query": "local", "path": "archive-output", "stateRoot": non_local_state}),
        json!({"selectionMode": "exact-keyword", "query": "local", "path": "archive-output", "portableDir": 7}),
    ] {
        let error = normalize_request(&params).unwrap_err().to_string();
        assert!(error.contains("path"));
    }
}

#[test]
fn query_commands_reject_non_local_state_roots_before_opening_storage() {
    for state_root in ["https://example.invalid/state", "//host/state"] {
        let error = list(&json!({"stateRoot": state_root}))
            .unwrap_err()
            .to_string();
        assert!(error.contains("local filesystem path"));
    }
}

#[test]
fn all_selection_drops_irrelevant_query_instead_of_reusing_an_agent_id() {
    let request = normalize_request(&json!({
        "selectionMode": "all",
        "query": "codex",
        "agent": "codex",
        "path": "archive-output"
    }))
    .unwrap();

    assert_eq!(request["selectionMode"], "all");
    assert_eq!(request["agent"], "codex");
    assert_eq!(request["query"], "");
}
