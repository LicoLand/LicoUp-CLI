use super::super::{RuntimeLane, adapter_for_agent_public, runtime_lane_for_agent, send_message};
use crate::domain::cli_registration;
use serde_json::json;

#[test]
fn dedicated_adapter_wins_over_cli_registration() {
    assert!(adapter_for_agent_public("codex").is_some());
    assert!(cli_registration::registration_for("codex").is_none());
    assert!(matches!(
        runtime_lane_for_agent("codex"),
        Some(RuntimeLane::Dedicated(_))
    ));
}

#[test]
fn catalog_ids_without_a_point_adapter_use_the_generic_lane() {
    assert!(adapter_for_agent_public("grok").is_none());
    assert!(adapter_for_agent_public("command-code").is_none());
    assert!(matches!(
        runtime_lane_for_agent("grok"),
        Some(RuntimeLane::GenericCli(registration)) if registration.id == "grok"
    ));
    assert!(matches!(
        runtime_lane_for_agent("command-code"),
        Some(RuntimeLane::GenericCli(registration)) if registration.id == "command-code"
    ));
    assert!(runtime_lane_for_agent("unknown-cli-agent").is_none());
}

#[test]
fn unknown_id_still_rejects_as_unsupported_adapter() {
    let error = send_message(
        &crate::target_port::agent_target_port(),
        &json!({"agent": "unknown-cli-agent", "text": "hello"}),
    )
    .unwrap_err();
    assert!(error.to_string().contains("unsupported runtime adapter"));
}

#[cfg(unix)]
#[test]
fn generic_lane_executes_a_registered_stdio_command() {
    let registration = cli_registration::CliRegistration {
        id: "echo-fixture".to_owned(),
        label: "Echo".to_owned(),
        command: "/bin/echo".to_owned(),
        args: vec!["{prompt}".to_owned()],
        stream_mode: cli_registration::StreamMode::Stdio,
    };
    let result = crate::platform::generic_cli_driver::execute(
        &registration,
        "/bin/echo",
        &json!({"agentId": "echo-fixture", "model": "fixture-model"}),
        "hello-generic",
        None,
        5_000,
        None,
    )
    .expect("echo");
    assert!(result.ok);
    assert!(result.output.contains("hello-generic"));
}
