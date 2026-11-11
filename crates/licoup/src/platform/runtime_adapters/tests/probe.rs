use super::super::probe::probe_runtime_driver;
use std::path::Path;

#[test]
fn unknown_adapter_probe_fails_closed_with_a_bounded_code() {
    super::compose();
    let result = probe_runtime_driver("unknown-adapter", Path::new("must-not-run"), Path::new("."));

    assert_eq!(result["available"], false);
    assert_eq!(result["supported"], false);
    assert_eq!(result["errorCode"], "unknown_adapter");
    assert!(result.get("output").is_none());
    assert!(result.get("path").is_none());
}

#[test]
fn codex_probe_is_static_and_does_not_launch_a_runtime() {
    super::compose();
    let result = probe_runtime_driver("codex", Path::new("codex"), Path::new("."));

    assert_eq!(result["available"], true);
    assert_eq!(result["supported"], true);
    assert_eq!(result["newSession"], true);
    assert_eq!(result["resumeSession"], true);
    assert_eq!(result["interactiveApprovalEvents"], false);
    assert!(result.get("output").is_none());
}

#[test]
fn deepseek_detection_does_not_invent_an_authenticated_sdk_handshake() {
    super::compose();
    let result = probe_runtime_driver("deepseek-harness", Path::new("dsh"), Path::new("."));

    assert_eq!(result["available"], true);
    assert_eq!(result["supported"], false);
    assert_eq!(result["newSession"], false);
    assert_eq!(result["errorCode"], "deepseek_harness_initialize_required");
}
