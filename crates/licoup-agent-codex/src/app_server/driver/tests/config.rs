use super::support::config;
use crate::app_server::config::{ProtocolConfig, spark_default_reasoning_effort};
use serde_json::json;
use std::fs;

#[test]
fn invalid_resume_path_fails_closed_without_echoing_the_path() {
    let failure = ProtocolConfig::from_params(
        &json!({"sessionPath": "/sessions/not-a-thread.jsonl"}),
        "prompt",
        "",
        None,
    )
    .unwrap_err();
    assert_eq!(failure.code, "codex_invalid_resume_target");
    assert_eq!(failure.stage, "thread/resume");
    assert!(!failure.message.contains("sessions"));
}

#[test]
fn rollout_record_identity_is_authoritative_over_filename_and_request() {
    let dir = std::env::temp_dir().join(format!("codex-config-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rollout-01234567-89ab-cdef-0123-456789abcdef.jsonl");
    fs::write(
        &path,
        r#"{"type":"session_meta","payload":{"id":"record-thread"}}
"#,
    )
    .unwrap();

    let from_path =
        ProtocolConfig::from_params(&json!({"sessionPath": path}), "prompt", "", None).unwrap();
    assert_eq!(from_path.requested_session_id, "record-thread");

    let failure = ProtocolConfig::from_params(
        &json!({"sessionPath": path}),
        "prompt",
        "different-thread",
        None,
    )
    .unwrap_err();
    assert_eq!(failure.code, "codex_resume_source_identity_mismatch");
    assert_eq!(failure.stage, "thread/resume");
    assert_eq!(failure.session_id.as_deref(), Some("different-thread"));
    assert_eq!(failure.thread_id.as_deref(), Some("different-thread"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn unsupported_sandbox_shape_fails_closed() {
    let failure = ProtocolConfig::from_params(
        &json!({"sandbox": {"type": "workspaceWrite"}}),
        "prompt",
        "",
        None,
    )
    .unwrap_err();
    assert_eq!(failure.code, "codex_invalid_sandbox");
    assert_eq!(failure.stage, "thread/configure");
}

#[test]
fn spark_defaults_to_supported_low_effort_only_when_unspecified() {
    assert_eq!(
        spark_default_reasoning_effort(Some("gpt-spark")),
        Some("low".to_string())
    );
    assert_eq!(spark_default_reasoning_effort(Some("gpt-standard")), None);
    assert_eq!(
        config(
            json!({"model": "gpt-spark", "reasoningEffort": "high"}),
            "p",
            ""
        )
        .reasoning_effort
        .as_deref(),
        Some("high")
    );
}
