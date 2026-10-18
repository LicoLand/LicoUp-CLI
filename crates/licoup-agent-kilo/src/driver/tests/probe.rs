//! This Agent's capability probe, in the host's shared driver vocabulary.

use super::super::capability_probe;
use std::path::Path;

#[test]
fn probe_rejects_relative_workspace_before_process_or_http_work() {
    let failure = capability_probe("unused", Path::new("relative"), 10, Some(16), 16).unwrap_err();
    assert_eq!(failure.code, "kilo_code_serve_working_directory_invalid");
    assert_eq!(failure.stage, "initialize");
}

#[test]
fn probe_rejects_an_empty_executable_with_the_process_start_failure() {
    let failure = capability_probe("   ", Path::new("/workspace"), 10, Some(16), 16).unwrap_err();
    assert_eq!(failure.code, "kilo_code_serve_process_start_failed");
    assert_eq!(failure.stage, "serve/ensure");
}
