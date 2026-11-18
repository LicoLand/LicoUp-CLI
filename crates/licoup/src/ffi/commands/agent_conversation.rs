use super::{AdmittedCommand, CliExecution, admitted_params};
use crate::ffi::generated::client_error::ClientError;
use crate::platform::runtime_adapters::RuntimeAdapterError;
use anyhow::Result;
use serde_json::Value;

// The FFI envelope serializes the complete ClientError: code, stage, component,
// retryable, recovery, and presentationArgs are never reclassified here.
pub(super) fn handle_agent_conversation(command: AdmittedCommand) -> Result<CliExecution> {
    let operation = match command.path() {
        ["agent", "conversation", operation] => *operation,
        _ => {
            return Err(super::handler_error("command_failed", "use_cli_help").into());
        }
    };
    let admitted_json = command.option_json("stdin-json");
    let params = match admitted_json {
        Some(value) => value.clone(),
        None => serde_json::json!({}),
    };

    if operation == "send" && stream_events_enabled(&params) {
        let _guard = crate::platform::install_stdout_ndjson_sink();
        crate::platform::emit_turn_event(
            "dispatch.turn.started",
            params
                .get("sessionId")
                .or_else(|| params.get("nativeSessionId"))
                .and_then(Value::as_str)
                .unwrap_or(""),
            "",
            serde_json::json!({
                "agentId": params.get("agent").or_else(|| params.get("agentId")).cloned().unwrap_or(serde_json::json!("")),
                "streamTransport": "stdio_ndjson_on_send"
            }),
        );
        let result = match crate::agent_port::dispatch(operation, &params) {
            Ok(result) => result,
            Err(error) => agent_conversation_failure(&error),
        };
        observe_skill_invocations(&params, &result);
        let mut done = result;
        if let Some(object) = done.as_object_mut() {
            object.insert("event".to_string(), serde_json::json!("done"));
        }
        write_stdout_json_line(&done)?;
        return Ok(CliExecution::Streamed);
    }

    let result = match crate::agent_port::dispatch(operation, &params) {
        Ok(result) => result,
        Err(error) => agent_conversation_failure(&error),
    };
    if operation == "send" {
        observe_skill_invocations(&params, &result);
    }
    Ok(CliExecution::Json(result))
}

fn observe_skill_invocations(params: &Value, result: &Value) {
    let Some(agent_id) = params
        .get("agent")
        .or_else(|| params.get("agentId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return;
    };
    // Metering is observational and local-only. A damaged ledger or revoked
    // pairing must never change the outcome of the user's conversation.
    let _ = crate::domain::skill_hub::observe_agent_skill_invocations(agent_id, result);
}

fn agent_conversation_failure(error: &RuntimeAdapterError) -> Value {
    let client_error: ClientError =
        crate::platform::runtime_adapters::client_error::client_error(error);
    serde_json::json!({
        "ok": false,
        "error": client_error
    })
}

fn stream_events_enabled(params: &Value) -> bool {
    params
        .get("streamEvents")
        .or_else(|| params.get("stream-events"))
        .and_then(|value| {
            value.as_bool().or_else(|| match value.as_str()?.trim() {
                "true" | "1" | "yes" => Some(true),
                _ => Some(false),
            })
        })
        == Some(true)
}

fn write_stdout_json_line(value: &Value) -> Result<()> {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, value)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

pub(super) fn handle_agents_pair(command: AdmittedCommand) -> Result<CliExecution> {
    let action = match command.path() {
        ["agents", "pair", action] => *action,
        _ => {
            return Err(super::handler_error("command_failed", "use_cli_help").into());
        }
    };
    let params = admitted_params(
        &[
            ("agent", command.option_text("agent")),
            ("target", command.option_text("target")),
        ],
        &[],
        &[],
    );
    let result = match action {
        "request" => crate::domain::skill_hub::pair_request(&params)?,
        "approve" => crate::domain::skill_hub::pair_approve(&params)?,
        "revoke" => crate::domain::skill_hub::pair_revoke(&params)?,
        "list" => crate::domain::skill_hub::pair_list(&params)?,
        _ => {
            return Err(super::handler_error("command_failed", "use_cli_help").into());
        }
    };
    Ok(CliExecution::Json(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::client_state::ClientStateStore;
    use serde_json::json;
    use std::fs;
    use uuid::Uuid;

    #[test]
    fn conversation_failures_report_the_exact_failed_stage() {
        let launch = agent_conversation_failure(&RuntimeAdapterError::ExecutableUnavailable);
        assert_eq!(
            launch["error"]["code"],
            "native_agent_executable_unavailable"
        );
        assert_eq!(launch["error"]["stage"], "process/launch");

        let request = agent_conversation_failure(&RuntimeAdapterError::MessageMissing);
        assert_eq!(request["error"]["code"], "agent_message_missing");
        assert_eq!(request["error"]["stage"], "request/validation");
    }

    #[test]
    fn successful_runtime_skill_event_is_aggregated_through_the_send_observer() {
        let root =
            std::env::temp_dir().join(format!("lico-agent-skill-observer-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let previous =
            licoup_foundation::platform::paths::set_portable_data_dir_override(Some(root));
        let store = ClientStateStore::portable().unwrap();
        crate::domain::skill_hub::pair_request(&json!({"agent": "codex"})).unwrap();
        crate::domain::skill_hub::pair_approve(&json!({"agent": "codex"})).unwrap();
        observe_skill_invocations(
            &json!({"agent": "codex"}),
            &json!({
                "ok": true,
                "events": [{"event": "skill.invoked", "skillId": "review-helper"}],
                "output": "must not enter usage storage"
            }),
        );
        let report = crate::domain::skill_hub::skill_usage_report(&json!({"days": 1})).unwrap();
        assert_eq!(report["totalInvocations"], 1);
        assert!(
            !store
                .read_collection("skill-usage")
                .unwrap()
                .to_string()
                .contains("must not enter usage storage")
        );
        licoup_foundation::platform::paths::set_portable_data_dir_override(previous);
    }
}
