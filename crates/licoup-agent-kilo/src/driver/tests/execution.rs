//! This Agent's composition of one Kilo turn.

use super::super::{RUNTIME_PROTOCOL, execute};
use serde_json::json;

#[test]
fn empty_executable_fails_closed_without_session_fallback() {
    let cwd = std::env::current_dir().unwrap();
    let result = execute(
        "",
        &json!({}),
        "private-kilo-prompt",
        "existing-kilo-native",
        Some(&cwd),
        1_000,
        Some(1024),
        1024,
    );
    assert!(!result.ok);
    assert_eq!(result.driver_id, "kilo-code-serve");
    assert_eq!(result.runtime_protocol, RUNTIME_PROTOCOL);
    assert_eq!(
        result.error.as_ref().map(|failure| failure.code.as_str()),
        Some("kilo_code_serve_process_start_failed")
    );
    assert!(matches!(
        result.transitions.last(),
        Some(super::super::Transition::Failed { code, .. })
            if code == "kilo_code_serve_process_start_failed"
    ));
    // No native session is invented for a turn that never ran.
    assert_eq!(result.session_id, "");
    assert_eq!(result.thread_id, "");
}

#[test]
fn a_relative_workspace_is_refused_before_the_endpoint_is_attached() {
    let result = execute(
        "kilo",
        &json!({}),
        "prompt",
        "native",
        Some(std::path::Path::new("relative")),
        1_000,
        None,
        1024,
    );
    assert!(!result.ok);
    assert_eq!(
        result.error.as_ref().map(|failure| failure.code.as_str()),
        Some("kilo_code_serve_working_directory_invalid")
    );
}
