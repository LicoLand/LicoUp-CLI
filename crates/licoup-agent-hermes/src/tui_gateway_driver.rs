//! The bounded turn one Hermes TUI Gateway connection runs.
//!
//! VENDOR-CODE-REMOVAL moved this module, its client ([`crate::tui_gateway`])
//! and the conversation-history projection over the same connection
//! ([`crate::remote_gateway_history`]) out of `licoup-native`'s platform tree:
//! the Gateway is one Agent's vendor transport, so it belongs to that Agent's
//! package. The composition above still owns the lane choice — a turn bound to
//! the Gateway speaks this protocol and a local turn speaks [`crate::driver`]'s
//! ACP transport — and hands the connection here; nothing below reaches back
//! into the client.

use crate::tui_gateway::{
    GatewayClient, GatewayFailure, event_payload, event_session_id, event_type,
};
use licoup_agent_drivers::acp_session_transport::{EffectiveSettings, ProtocolFailure, RunResult};
use licoup_agent_targets::platform::virtual_machine::{
    SshRuntimeConnection, is_valid_guest_working_directory,
};
use licoup_conversation::{TurnEvent as CanonicalTurnEvent, TurnState as CanonicalTurnState};
use licoup_foundation::platform::turn_event_emit::{
    emit_agent_message_chunk, emit_agent_message_completed, emit_turn_event,
};
use serde_json::{Map, Value, json};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const MAX_IDENTIFIER_BYTES: usize = 512;

pub fn execute(
    connection: &SshRuntimeConnection,
    params: &Value,
    prompt: &str,
    requested_session_id: &str,
    cwd: Option<&Path>,
    timeout_ms: u64,
    max_stdout: Option<usize>,
    max_stderr: usize,
) -> RunResult {
    let started_at = timestamp();
    let config = match GatewayConfig::from_params(params, prompt, requested_session_id, cwd) {
        Ok(config) => config,
        Err(failure) => return failed_result(failure, started_at, false, false),
    };
    let mut client = match GatewayClient::connect(connection, max_stdout, max_stderr) {
        Ok(client) => client,
        Err(failure) => {
            return failed_result(
                protocol_transport_failure(failure, None, Some(&config.turn_id)),
                started_at,
                failure == GatewayFailure::OutputLimit,
                false,
            );
        }
    };
    // `timeoutMs: 0` opts out of any turn deadline: the gateway is waited on
    // until its native terminal signal, however long the turn takes.
    let deadline = if timeout_ms == 0 {
        None
    } else {
        Some(Instant::now() + Duration::from_millis(timeout_ms))
    };
    let run = run_turn(&mut client, &config, deadline, max_stdout);
    let cleanup = client.finish();
    if let Err(failure) = cleanup {
        return failed_result(
            protocol_transport_failure(
                failure,
                run.as_ref().ok().map(|outcome| outcome.session_id.as_str()),
                Some(&config.turn_id),
            ),
            started_at,
            failure == GatewayFailure::OutputLimit,
            failure == GatewayFailure::OutputLimit,
        );
    }
    match run {
        Ok(outcome) => RunResult {
            ok: true,
            output: outcome.output,
            events: outcome.events,
            error: None,
            thread_id: outcome.session_id.clone(),
            session_id: outcome.session_id,
            turn_id: config.turn_id,
            turn_status: "end_turn".to_string(),
            effective: EffectiveSettings {
                cwd: Some(config.cwd),
                model: outcome.model.or(config.model),
                ..EffectiveSettings::default()
            },
            status_code: None,
            stdout_truncated: false,
            stderr_truncated: false,
            started_at,
        },
        Err(failure) => {
            let output_limit = failure.code == "hermes_gateway_protocol_output_limit";
            failed_result(failure, started_at, output_limit, false)
        }
    }
}

struct GatewayConfig {
    prompt: String,
    requested_session_id: String,
    cwd: String,
    model: Option<String>,
    turn_id: String,
}

impl GatewayConfig {
    fn from_params(
        params: &Value,
        prompt: &str,
        session_id: &str,
        cwd: Option<&Path>,
    ) -> Result<Self, ProtocolFailure> {
        if prompt.trim().is_empty() {
            return Err(ProtocolFailure::new(
                "hermes_empty_prompt",
                "Hermes Agent requires a non-empty message.",
                "request/validate",
            ));
        }
        if text_param(params, &["reasoningEffort", "reasoning_effort"]).is_some() {
            return Err(ProtocolFailure::new(
                "hermes_gateway_reasoning_override_unsupported",
                "Hermes Gateway does not expose a per-turn reasoning-effort override in LicoUp.",
                "capability/reasoning",
            ));
        }
        if explicit_value(params, &["sandbox", "sandboxMode"]).is_some() {
            return Err(ProtocolFailure::new(
                "hermes_gateway_sandbox_override_unsupported",
                "Hermes Gateway inherits the native sandbox configuration.",
                "capability/sandbox",
            ));
        }
        if explicit_value(params, &["approvalPolicy", "approval_policy"]).is_some() {
            return Err(ProtocolFailure::new(
                "hermes_gateway_approval_override_unsupported",
                "Hermes Gateway interaction requests require an explicit client response.",
                "capability/approval",
            ));
        }
        let cwd = cwd
            .filter(|path| is_valid_guest_working_directory(path))
            .map(|path| path.to_string_lossy().to_string())
            .ok_or_else(|| {
                ProtocolFailure::new(
                    "hermes_gateway_absolute_cwd_required",
                    "Hermes Gateway requires an absolute guest working directory.",
                    "request/validate",
                )
            })?;
        Ok(Self {
            prompt: prompt.to_string(),
            requested_session_id: session_id.trim().to_string(),
            cwd,
            model: text_param(params, &["model", "modelId"]),
            turn_id: Uuid::new_v4().to_string(),
        })
    }
}

struct GatewayOutcome {
    output: String,
    events: Vec<Value>,
    session_id: String,
    model: Option<String>,
}

fn run_turn(
    client: &mut GatewayClient,
    config: &GatewayConfig,
    deadline: Option<Instant>,
    max_output_bytes: Option<usize>,
) -> Result<GatewayOutcome, ProtocolFailure> {
    client
        .wait_ready(deadline)
        .map_err(|failure| protocol_transport_failure(failure, None, Some(&config.turn_id)))?;
    let opened = open_session(client, config, deadline)?;
    let mut turn = TurnObservation::new(
        opened.live_session_id,
        opened.durable_session_id,
        config.turn_id.clone(),
        opened.model,
        max_output_bytes,
    );
    emit_turn_event(
        "dispatch.turn.bound",
        &turn.durable_session_id,
        &turn.turn_id,
        json!({"nativeSteer": false}),
    );
    let prompt_result = client
        .request(
            "prompt.submit",
            json!({
                "session_id": turn.live_session_id,
                "text": config.prompt,
            }),
            deadline,
            |message| turn.observe(message),
        )
        .map_err(|failure| {
            protocol_transport_failure(failure, Some(&turn.durable_session_id), Some(&turn.turn_id))
        })?;
    let status = prompt_result
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !matches!(status, "streaming" | "queued") {
        return Err(turn.failure(
            "hermes_gateway_prompt_rejected",
            "Hermes Gateway rejected the prompt before acceptance.",
            "prompt/submit",
        ));
    }
    turn.ensure_started().map_err(|failure| {
        protocol_transport_failure(failure, Some(&turn.durable_session_id), Some(&turn.turn_id))
    })?;
    while !turn.state.is_terminal() {
        let message = client.next_message(deadline).map_err(|failure| {
            protocol_transport_failure(failure, Some(&turn.durable_session_id), Some(&turn.turn_id))
        })?;
        turn.observe(&message).map_err(|failure| {
            protocol_transport_failure(failure, Some(&turn.durable_session_id), Some(&turn.turn_id))
        })?;
    }
    if turn.output.trim().is_empty() {
        return Err(turn.failure(
            "hermes_gateway_final_message_missing",
            "Hermes Gateway completed without a final assistant message.",
            "prompt/complete",
        ));
    }

    let close_result = client
        .request(
            "session.close",
            json!({"session_id": turn.live_session_id}),
            deadline,
            |_| Ok(()),
        )
        .map_err(|failure| {
            protocol_transport_failure(failure, Some(&turn.durable_session_id), Some(&turn.turn_id))
        })?;
    if close_result.get("closed").and_then(Value::as_bool) != Some(true) {
        return Err(turn.failure(
            "hermes_gateway_session_close_failed",
            "Hermes Gateway could not finalize the completed conversation safely.",
            "session/close",
        ));
    }
    emit_agent_message_completed(&turn.durable_session_id, &turn.turn_id, &turn.output);
    Ok(GatewayOutcome {
        output: turn.output,
        events: turn.events,
        session_id: turn.durable_session_id,
        model: turn.model,
    })
}

struct OpenedSession {
    live_session_id: String,
    durable_session_id: String,
    model: Option<String>,
}

fn open_session(
    client: &mut GatewayClient,
    config: &GatewayConfig,
    deadline: Option<Instant>,
) -> Result<OpenedSession, ProtocolFailure> {
    let result = if config.requested_session_id.is_empty() {
        let mut params = Map::from_iter([
            ("cwd".to_string(), json!(config.cwd)),
            ("source".to_string(), json!("desktop")),
            ("close_on_disconnect".to_string(), json!(true)),
        ]);
        if let Some(model) = config.model.as_deref() {
            params.insert("model".to_string(), json!(model));
        }
        client.request(
            "session.create",
            Value::Object(params),
            deadline,
            reject_interaction_event,
        )
    } else {
        client.request(
            "session.resume",
            json!({
                "session_id": config.requested_session_id,
                "source": "desktop",
                "close_on_disconnect": true,
            }),
            deadline,
            reject_interaction_event,
        )
    }
    .map_err(|failure| {
        protocol_transport_failure(
            failure,
            (!config.requested_session_id.is_empty())
                .then_some(config.requested_session_id.as_str()),
            Some(&config.turn_id),
        )
    })?;

    let live_session_id = bounded_identifier(result.get("session_id"))
        .ok_or_else(|| invalid_open_response(config, "hermes_gateway_live_session_id_missing"))?;
    let durable_session_id = if config.requested_session_id.is_empty() {
        bounded_identifier(result.get("stored_session_id")).ok_or_else(|| {
            invalid_open_response(config, "hermes_gateway_durable_session_id_missing")
        })?
    } else {
        let resumed =
            bounded_identifier(result.get("resumed").or_else(|| result.get("session_key")))
                .ok_or_else(|| {
                    invalid_open_response(config, "hermes_gateway_resume_identity_missing")
                })?;
        if resumed != config.requested_session_id {
            return Err(invalid_open_response(
                config,
                "hermes_gateway_session_identity_mismatch",
            ));
        }
        resumed
    };
    let model = result
        .pointer("/info/model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    Ok(OpenedSession {
        live_session_id,
        durable_session_id,
        model,
    })
}

fn reject_interaction_event(message: &Value) -> Result<(), GatewayFailure> {
    if event_type(message).is_some_and(is_interaction_event) {
        Err(GatewayFailure::Interaction)
    } else if event_type(message).is_some() {
        Ok(())
    } else {
        Err(GatewayFailure::InvalidMessage)
    }
}

/// Hermes protocol observation data. Lifecycle decisions are delegated to the
/// canonical Conversation state machine; this adapter reports signals only.
struct TurnObservation {
    live_session_id: String,
    durable_session_id: String,
    turn_id: String,
    output: String,
    events: Vec<Value>,
    model: Option<String>,
    max_output_bytes: Option<usize>,
    state: CanonicalTurnState,
}

impl TurnObservation {
    fn new(
        live_session_id: String,
        durable_session_id: String,
        turn_id: String,
        model: Option<String>,
        max_output_bytes: Option<usize>,
    ) -> Self {
        Self {
            live_session_id,
            durable_session_id,
            turn_id,
            output: String::new(),
            events: Vec::new(),
            model,
            max_output_bytes,
            state: CanonicalTurnState::Pending,
        }
    }

    fn observe(&mut self, message: &Value) -> Result<(), GatewayFailure> {
        let event = event_type(message).ok_or(GatewayFailure::InvalidMessage)?;
        if let Some(session_id) = event_session_id(message)
            && session_id != self.live_session_id
        {
            return Err(GatewayFailure::InvalidMessage);
        }
        if is_interaction_event(event) {
            self.ensure_started()?;
            self.report(CanonicalTurnEvent::WaitForHuman)?;
            return Err(GatewayFailure::Interaction);
        }
        match event {
            "message.delta" => {
                self.ensure_started()?;
                let text = event_payload(message)
                    .and_then(|payload| payload.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.append_output(text)?;
                emit_agent_message_chunk(&self.durable_session_id, &self.turn_id, text);
            }
            "message.complete" => {
                self.ensure_started()?;
                let payload = event_payload(message).ok_or(GatewayFailure::InvalidMessage)?;
                if payload.get("status").and_then(Value::as_str) == Some("error") {
                    self.report(CanonicalTurnEvent::Fail)?;
                    return Err(GatewayFailure::Rpc);
                }
                if let Some(text) = payload.get("text").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    if self
                        .max_output_bytes
                        .is_some_and(|limit| text.len() > limit)
                    {
                        return Err(GatewayFailure::OutputLimit);
                    }
                    self.output = text.to_string();
                }
                self.report(CanonicalTurnEvent::Succeed)?;
            }
            "error" => {
                self.report(CanonicalTurnEvent::Fail)?;
                return Err(GatewayFailure::Rpc);
            }
            "session.info" => {
                if let Some(model) = event_payload(message)
                    .and_then(|payload| payload.get("model"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    self.model = Some(model.to_string());
                }
            }
            "tool.start" | "tool.progress" | "tool.complete" => {
                // Tool events are part of the conversation projection and stay
                // unbounded by default: with no caller output request they run
                // until the native terminal signal. An explicit caller bound is
                // still enforced as the named protocol output-limit failure.
                let payload = event_payload(message).unwrap_or(&Value::Null);
                self.events.push(json!({
                    "type": event,
                    "toolName": payload.get("name").and_then(Value::as_str).unwrap_or(""),
                    "toolCallId": payload.get("tool_id").and_then(Value::as_str).unwrap_or(""),
                    "isError": payload.get("is_error").and_then(Value::as_bool)
                }));
            }
            _ => {}
        }
        Ok(())
    }

    fn ensure_started(&mut self) -> Result<(), GatewayFailure> {
        match self.state {
            CanonicalTurnState::Pending | CanonicalTurnState::Claimed => {
                self.report(CanonicalTurnEvent::Start)
            }
            CanonicalTurnState::Running => Ok(()),
            CanonicalTurnState::WaitingForHuman => self.report(CanonicalTurnEvent::Resume),
            CanonicalTurnState::Succeeded
            | CanonicalTurnState::Failed
            | CanonicalTurnState::Interrupted
            | CanonicalTurnState::Cancelled => Ok(()),
        }
    }

    fn report(&mut self, event: CanonicalTurnEvent) -> Result<(), GatewayFailure> {
        self.state = self
            .state
            .transition(event)
            .map_err(|_| GatewayFailure::InvalidMessage)?;
        Ok(())
    }

    fn append_output(&mut self, text: &str) -> Result<(), GatewayFailure> {
        if self
            .max_output_bytes
            .is_some_and(|limit| self.output.len().saturating_add(text.len()) > limit)
        {
            return Err(GatewayFailure::OutputLimit);
        }
        self.output.push_str(text);
        Ok(())
    }

    fn failure(
        &self,
        code: &'static str,
        message: &'static str,
        stage: &'static str,
    ) -> ProtocolFailure {
        failure_with_ids(
            code,
            message,
            stage,
            Some(&self.durable_session_id),
            Some(&self.turn_id),
        )
    }
}

fn is_interaction_event(event: &str) -> bool {
    matches!(
        event,
        "approval.request"
            | "clarify.request"
            | "sudo.request"
            | "secret.request"
            | "terminal.read.request"
    )
}

fn bounded_identifier(value: Option<&Value>) -> Option<String> {
    let value = value?.as_str()?;
    (!value.is_empty()
        && value.len() <= MAX_IDENTIFIER_BYTES
        && value.trim() == value
        && !value.bytes().any(|byte| byte.is_ascii_control()))
    .then(|| value.to_string())
}

fn invalid_open_response(config: &GatewayConfig, code: &'static str) -> ProtocolFailure {
    failure_with_ids(
        code,
        "Hermes Gateway returned an invalid conversation binding.",
        "session/open",
        (!config.requested_session_id.is_empty()).then_some(config.requested_session_id.as_str()),
        Some(&config.turn_id),
    )
}

fn protocol_transport_failure(
    failure: GatewayFailure,
    session_id: Option<&str>,
    turn_id: Option<&str>,
) -> ProtocolFailure {
    let (message, stage) = match failure {
        GatewayFailure::Start => ("Hermes Gateway could not be started.", "process/start"),
        GatewayFailure::Pipe => (
            "Hermes Gateway protocol pipes are unavailable.",
            "process/start",
        ),
        GatewayFailure::Write => (
            "Hermes Gateway stopped accepting protocol messages.",
            "protocol/write",
        ),
        GatewayFailure::Timeout => (
            "Hermes Gateway timed out before the operation completed.",
            "protocol/wait",
        ),
        GatewayFailure::InvalidJson | GatewayFailure::InvalidMessage => (
            "Hermes Gateway returned an invalid protocol message.",
            "protocol/read",
        ),
        GatewayFailure::OutputLimit => (
            "Hermes Gateway exceeded the configured protocol output limit.",
            "protocol/read",
        ),
        GatewayFailure::Read => (
            "Hermes Gateway protocol output could not be read.",
            "protocol/read",
        ),
        GatewayFailure::Exited => (
            "Hermes Gateway exited before the operation completed.",
            "process/exit",
        ),
        GatewayFailure::Rpc => (
            "Hermes Gateway could not complete the requested operation.",
            "protocol/rpc",
        ),
        GatewayFailure::Interaction => (
            "Hermes Agent requires explicit user interaction before this turn can continue.",
            "server/request",
        ),
        GatewayFailure::Cleanup => (
            "Hermes Gateway process cleanup could not be completed safely.",
            "process/cleanup",
        ),
    };
    let is_timeout = failure == GatewayFailure::Timeout;
    let mut failure = failure_with_ids(failure.code(), message, stage, session_id, turn_id);
    if is_timeout {
        failure.turn_status = Some("timeout".to_string());
    }
    if failure.code == "hermes_gateway_user_interaction_required" {
        failure.user_interaction_required = true;
        failure.request_method = Some("hermes.gateway.interaction".to_string());
    }
    failure
}

fn failure_with_ids(
    code: &'static str,
    message: &'static str,
    stage: &'static str,
    session_id: Option<&str>,
    turn_id: Option<&str>,
) -> ProtocolFailure {
    let mut failure = ProtocolFailure::new(code, message, stage);
    failure.session_id = session_id.map(str::to_string);
    failure.turn_id = turn_id.map(str::to_string);
    failure
}

fn failed_result(
    failure: ProtocolFailure,
    started_at: String,
    stdout_truncated: bool,
    stderr_truncated: bool,
) -> RunResult {
    let session_id = failure.session_id.clone().unwrap_or_default();
    RunResult {
        ok: false,
        output: String::new(),
        events: Vec::new(),
        error: Some(failure.clone()),
        thread_id: session_id.clone(),
        session_id,
        turn_id: failure.turn_id.clone().unwrap_or_default(),
        turn_status: failure.turn_status.clone().unwrap_or_default(),
        effective: EffectiveSettings::default(),
        status_code: None,
        stdout_truncated,
        stderr_truncated,
        started_at,
    }
}

fn explicit_value<'a>(params: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter()
        .find_map(|key| params.get(*key))
        .filter(|value| !value.is_null())
}

fn text_param(params: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| params.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::process::Command;

    fn turn() -> TurnObservation {
        TurnObservation::new(
            "live-1".to_string(),
            "durable-1".to_string(),
            "turn-1".to_string(),
            None,
            Some(1024),
        )
    }

    #[test]
    fn turn_uses_durable_identity_and_canonical_completion_text() {
        let mut turn = turn();
        turn.observe(&json!({
            "jsonrpc": "2.0",
            "method": "event",
            "params": {
                "type": "message.delta",
                "session_id": "live-1",
                "payload": {"text": "hel"}
            }
        }))
        .unwrap();
        turn.observe(&json!({
            "jsonrpc": "2.0",
            "method": "event",
            "params": {
                "type": "message.delta",
                "session_id": "live-1",
                "payload": {"text": "lo"}
            }
        }))
        .unwrap();
        turn.observe(&json!({
            "jsonrpc": "2.0",
            "method": "event",
            "params": {
                "type": "message.complete",
                "session_id": "live-1",
                "payload": {"text": "hello", "status": "complete"}
            }
        }))
        .unwrap();
        assert_eq!(turn.state, CanonicalTurnState::Succeeded);
        assert_eq!(turn.output, "hello");
        assert_eq!(turn.durable_session_id, "durable-1");
    }

    #[test]
    fn cross_session_and_interaction_events_fail_closed() {
        let mut turn = turn();
        assert_eq!(
            turn.observe(&json!({
                "jsonrpc": "2.0",
                "method": "event",
                "params": {
                    "type": "message.delta",
                    "session_id": "live-2",
                    "payload": {"text": "wrong"}
                }
            })),
            Err(GatewayFailure::InvalidMessage)
        );
        assert_eq!(
            turn.observe(&json!({
                "jsonrpc": "2.0",
                "method": "event",
                "params": {
                    "type": "approval.request",
                    "session_id": "live-1",
                    "payload": {"request_id": "approval-1"}
                }
            })),
            Err(GatewayFailure::Interaction)
        );
    }

    #[test]
    fn turn_output_and_event_storage_are_bounded() {
        let mut turn = TurnObservation::new(
            "live".to_string(),
            "durable".to_string(),
            "turn".to_string(),
            None,
            Some(3),
        );
        assert_eq!(turn.append_output("four"), Err(GatewayFailure::OutputLimit));
    }

    #[cfg(unix)]
    #[test]
    fn silent_gateway_after_binding_fails_with_a_bounded_timeout() {
        let script = r#"
printf '%s\n' '{"jsonrpc":"2.0","method":"event","params":{"type":"gateway.ready","payload":{}}}'
IFS= read -r request
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"session_id":"live-1","stored_session_id":"durable-1","messages":[],"info":{"cwd":"/workspace"}}}'
IFS= read -r request
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"status":"streaming"}}'
sleep 30
"#;
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        let mut client =
            GatewayClient::connect_test_command(command, Some(64 * 1024), 64 * 1024).unwrap();
        let config = GatewayConfig {
            prompt: "test prompt".to_string(),
            requested_session_id: String::new(),
            cwd: "/workspace".to_string(),
            model: None,
            turn_id: "turn-1".to_string(),
        };
        let started = Instant::now();
        let failure = match run_turn(
            &mut client,
            &config,
            Some(started + Duration::from_millis(600)),
            Some(64 * 1024),
        ) {
            Ok(_) => panic!("a silent gateway must not complete the turn"),
            Err(failure) => failure,
        };
        assert_eq!(failure.code, "hermes_gateway_protocol_timeout");
        assert_eq!(failure.turn_status.as_deref(), Some("timeout"));
        assert_eq!(failure.session_id.as_deref(), Some("durable-1"));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a silent gateway must fail at the bounded deadline, not stall"
        );
        client.finish().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn bounded_gateway_round_trip_preserves_new_and_resumed_durable_ids() {
        for requested_session_id in ["", "durable-1"] {
            let open_result = if requested_session_id.is_empty() {
                r#"{"session_id":"live-1","stored_session_id":"durable-1","messages":[],"info":{"cwd":"/workspace","model":"test-model"}}"#
            } else {
                r#"{"session_id":"live-1","resumed":"durable-1","session_key":"durable-1","messages":[],"info":{"cwd":"/workspace","model":"test-model"}}"#
            };
            let script = format!(
                r#"
printf '%s\n' '{{"jsonrpc":"2.0","method":"event","params":{{"type":"gateway.ready","payload":{{}}}}}}'
IFS= read -r request
printf '%s\n' '{{"jsonrpc":"2.0","id":1,"result":{open_result}}}'
IFS= read -r request
printf '%s\n' '{{"jsonrpc":"2.0","id":2,"result":{{"status":"streaming"}}}}'
printf '%s\n' '{{"jsonrpc":"2.0","method":"event","params":{{"type":"message.delta","session_id":"live-1","payload":{{"text":"hel"}}}}}}'
printf '%s\n' '{{"jsonrpc":"2.0","method":"event","params":{{"type":"message.complete","session_id":"live-1","payload":{{"text":"hello","status":"complete"}}}}}}'
IFS= read -r request
printf '%s\n' '{{"jsonrpc":"2.0","id":3,"result":{{"closed":true}}}}'
"#
            );
            let mut command = Command::new("sh");
            command.args(["-c", &script]);
            let mut client =
                GatewayClient::connect_test_command(command, Some(64 * 1024), 64 * 1024).unwrap();
            let config = GatewayConfig {
                prompt: "test prompt".to_string(),
                requested_session_id: requested_session_id.to_string(),
                cwd: "/workspace".to_string(),
                model: None,
                turn_id: "turn-1".to_string(),
            };
            let outcome = run_turn(
                &mut client,
                &config,
                Some(Instant::now() + Duration::from_secs(2)),
                Some(64 * 1024),
            )
            .unwrap();
            client.finish().unwrap();
            assert_eq!(outcome.session_id, "durable-1");
            assert_eq!(outcome.output, "hello");
            assert_eq!(outcome.model.as_deref(), Some("test-model"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn hermes_zero_timeout_and_default_event_projection_are_unbounded() {
        let script = r#"
printf '%s\n' '{"jsonrpc":"2.0","method":"event","params":{"type":"gateway.ready","payload":{}}}'
IFS= read -r request
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"session_id":"live-1","stored_session_id":"durable-1","messages":[],"info":{"cwd":"/workspace"}}}'
IFS= read -r request
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"status":"streaming"}}'
sleep 0.3
i=0
while [ "$i" -lt 4200 ]; do
  printf '%s\n' '{"jsonrpc":"2.0","method":"event","params":{"type":"tool.start","session_id":"live-1","payload":{"name":"shell","tool_id":"opaque-tool-id"}}}'
  i=$((i + 1))
done
printf '%s\n' '{"jsonrpc":"2.0","method":"event","params":{"type":"message.delta","session_id":"live-1","payload":{"text":"done"}}}'
printf '%s\n' '{"jsonrpc":"2.0","method":"event","params":{"type":"message.complete","session_id":"live-1","payload":{"text":"done","status":"complete"}}}'
IFS= read -r request
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"closed":true}}'
"#;
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        let mut client = GatewayClient::connect_test_command(command, None, 64 * 1024).unwrap();
        let config = GatewayConfig {
            prompt: "test prompt".to_string(),
            requested_session_id: String::new(),
            cwd: "/workspace".to_string(),
            model: None,
            turn_id: "turn-1".to_string(),
        };
        // None deadline means no turn deadline: the delayed gate, 4,200 tool
        // events, and complete message must all survive the default projection.
        let outcome = run_turn(&mut client, &config, None, None).unwrap();
        client.finish().unwrap();
        assert_eq!(outcome.session_id, "durable-1");
        assert_eq!(outcome.output, "done");
        assert_eq!(
            outcome.events.len(),
            4_200,
            "every tool event must survive the default projection"
        );
        assert!(
            outcome
                .events
                .iter()
                .all(|event| event["type"] == "tool.start"),
            "only tool events are projected: {}",
            outcome.events.len()
        );
    }

    #[cfg(unix)]
    #[test]
    fn explicit_output_bound_keeps_the_named_failure() {
        let script = r#"
printf '%s\n' '{"jsonrpc":"2.0","method":"event","params":{"type":"gateway.ready","payload":{}}}'
IFS= read -r request
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"session_id":"live-1","stored_session_id":"durable-1","messages":[],"info":{"cwd":"/workspace"}}}'
IFS= read -r request
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"status":"streaming"}}'
payload=$(python3 -c 'print("x" * 70000)')
printf '%s\n' "{\"jsonrpc\":\"2.0\",\"method\":\"event\",\"params\":{\"type\":\"message.delta\",\"session_id\":\"live-1\",\"payload\":{\"text\":\"$payload\"}}}"
printf '%s\n' '{"jsonrpc":"2.0","method":"event","params":{"type":"message.complete","session_id":"live-1","payload":{"text":"done","status":"complete"}}}'
IFS= read -r request
"#;
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        let mut client =
            GatewayClient::connect_test_command(command, Some(64 * 1024), 64 * 1024).unwrap();
        let config = GatewayConfig {
            prompt: "test prompt".to_string(),
            requested_session_id: String::new(),
            cwd: "/workspace".to_string(),
            model: None,
            turn_id: "turn-1".to_string(),
        };
        let failure = match run_turn(
            &mut client,
            &config,
            Some(Instant::now() + Duration::from_secs(5)),
            Some(64 * 1024),
        ) {
            Ok(_) => panic!("an explicit output bound must stay a visible failure"),
            Err(failure) => failure,
        };
        assert_eq!(failure.code, "hermes_gateway_protocol_output_limit");
        client.finish().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn resume_identity_missing_or_drifted_fails_before_prompt() {
        let log_path = std::env::temp_dir().join(format!(
            "lico-hermes-tui-received-{}-{}",
            std::process::id(),
            Uuid::new_v4()
        ));
        for (requested, open_result, expected_code) in [
            (
                "",
                r#"{"session_id":"live-1","messages":[],"info":{"cwd":"/workspace"}}"#,
                "hermes_gateway_durable_session_id_missing",
            ),
            (
                "durable-1",
                r#"{"session_id":"live-1","resumed":"durable-2","messages":[],"info":{"cwd":"/workspace"}}"#,
                "hermes_gateway_session_identity_mismatch",
            ),
        ] {
            let script = format!(
                r#"
printf '%s\n' '{{"jsonrpc":"2.0","method":"event","params":{{"type":"gateway.ready","payload":{{}}}}}}'
IFS= read -r request
printf '%s' "$request" > "$RECEIPT_LOG"
printf '%s\n' '{{"jsonrpc":"2.0","id":1,"result":{open_result}}}'
IFS= read -r request
printf '%s' "$request" >> "$RECEIPT_LOG"
"#
            );
            let mut command = Command::new("sh");
            command.args(["-c", &script]).env("RECEIPT_LOG", &log_path);
            let mut client =
                GatewayClient::connect_test_command(command, Some(64 * 1024), 64 * 1024).unwrap();
            let config = GatewayConfig {
                prompt: "test prompt".to_string(),
                requested_session_id: requested.to_string(),
                cwd: "/workspace".to_string(),
                model: None,
                turn_id: "turn-1".to_string(),
            };
            let failure = match run_turn(
                &mut client,
                &config,
                Some(Instant::now() + Duration::from_secs(2)),
                Some(64 * 1024),
            ) {
                Ok(_) => panic!("an invalid binding must fail before prompt"),
                Err(failure) => failure,
            };
            assert_eq!(failure.code, expected_code);
            client.finish().unwrap();
            let received = std::fs::read_to_string(&log_path).unwrap();
            assert!(
                !received.contains("prompt.submit"),
                "no prompt may be submitted after an invalid binding: {received}"
            );
            let _ = std::fs::remove_file(&log_path);
        }
    }
}
