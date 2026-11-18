// OpenClaw's Gateway-attached ACP semantic parser.
use super::codec::{
    INITIALIZE_REQUEST_ID, MODE_REQUEST_ID, PROMPT_REQUEST_ID, SESSION_REQUEST_ID,
    request_id_matches, response_is_error,
};
use super::events::projected_event;
use crate::gateway_acp::continuity::{SessionBinding, session_method, session_request};
use crate::gateway_acp::errors::ProtocolFailure;
use crate::gateway_acp::model::EffectiveSettings;
use crate::gateway_acp::params::ProtocolConfig;
use licoup_foundation::core::acp::{self, AcpClientCapabilities, AcpImplementation};
use serde_json::{Value, json};

#[derive(Clone, Debug)]
pub struct ProtocolOutcome {
    pub output: String,
    pub session_id: String,
    pub turn_id: String,
    pub turn_status: String,
    pub effective: EffectiveSettings,
    pub transitions: Vec<licoup_agent_adapter_sdk::Transition>,
}

#[derive(Debug)]
pub enum ProtocolEffect {
    Send(Value),
    Complete(Box<ProtocolOutcome>),
    Fail(ProtocolFailure),
}

pub struct ParsedProtocolFrame {
    pub effects: Vec<ProtocolEffect>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolPhase {
    AwaitInitialize,
    AwaitSession,
    AwaitMode,
    AwaitPrompt,
    Finished,
}

#[derive(Debug)]
pub struct OpenClawProtocol {
    pub config: ProtocolConfig,
    pub phase: ProtocolPhase,
    pub binding: SessionBinding,
    pub output: String,
    pub events: Vec<Value>,
    pub effective: EffectiveSettings,
}

impl OpenClawProtocol {
    pub fn new(config: ProtocolConfig) -> Self {
        let effective = EffectiveSettings {
            cwd: Some(config.cwd.clone()),
            reasoning_effort: config.reasoning_effort.clone(),
            ..EffectiveSettings::default()
        };
        let binding = SessionBinding::new(&config);
        Self {
            config,
            phase: ProtocolPhase::AwaitInitialize,
            binding,
            output: String::new(),
            events: Vec::new(),
            effective,
        }
    }

    pub fn initial_request(&self) -> Result<Value, ProtocolFailure> {
        acp::initialize_request(
            INITIALIZE_REQUEST_ID,
            &AcpImplementation::new("lico-up", env!("CARGO_PKG_VERSION")).title("LicoUp"),
            AcpClientCapabilities::default(),
        )
        .map_err(|error| ProtocolFailure::from_acp(error, acp::INITIALIZE_METHOD))
    }

    pub fn handle_message(&mut self, message: Value) -> Vec<ProtocolEffect> {
        if let Some(effects) = self.handle_server_request(&message) {
            self.phase = ProtocolPhase::Finished;
            return effects;
        }
        if message.get("method").is_some() {
            return self.handle_notification(message);
        }
        match self.phase {
            ProtocolPhase::AwaitInitialize
                if request_id_matches(&message, INITIALIZE_REQUEST_ID) =>
            {
                self.handle_initialize_response(&message)
            }
            ProtocolPhase::AwaitSession if request_id_matches(&message, SESSION_REQUEST_ID) => {
                self.handle_session_response(&message)
            }
            ProtocolPhase::AwaitMode if request_id_matches(&message, MODE_REQUEST_ID) => {
                self.handle_mode_response(&message)
            }
            ProtocolPhase::AwaitPrompt if request_id_matches(&message, PROMPT_REQUEST_ID) => {
                self.handle_prompt_response(&message)
            }
            _ => Vec::new(),
        }
    }

    pub fn handle_frame(&mut self, line: &[u8]) -> ParsedProtocolFrame {
        let message = match super::codec::decode_message(line) {
            Ok(message) => message,
            Err(super::codec::DecodeFailure::TooLarge) => {
                self.phase = ProtocolPhase::Finished;
                return ParsedProtocolFrame {
                    effects: vec![ProtocolEffect::Fail(self.failure_with_ids(
                        "openclaw_acp_output_limit",
                        "OpenClaw ACP exceeded the configured protocol output limit.",
                        "protocol/read",
                    ))],
                };
            }
            Err(super::codec::DecodeFailure::Invalid) => {
                self.phase = ProtocolPhase::Finished;
                return ParsedProtocolFrame {
                    effects: vec![ProtocolEffect::Fail(self.failure_with_ids(
                        "openclaw_acp_invalid_json",
                        "OpenClaw ACP returned an invalid protocol message.",
                        "protocol/read",
                    ))],
                };
            }
        };
        ParsedProtocolFrame {
            effects: self.handle_message(message),
        }
    }

    fn handle_server_request(&self, message: &Value) -> Option<Vec<ProtocolEffect>> {
        let request_id = message.get("id")?;
        let method = message.get("method")?.as_str()?;
        if message.get("result").is_some() || message.get("error").is_some() {
            return None;
        }
        let response = if method == "session/request_permission" {
            json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {"outcome": {"outcome": "cancelled"}}
            })
        } else {
            json!({
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {"code": -32601, "message": "Client method is not available."}
            })
        };
        Some(vec![
            ProtocolEffect::Send(response),
            ProtocolEffect::Fail(ProtocolFailure::user_interaction(
                method,
                self.binding.native_id(),
                Some(&self.config.turn_id),
            )),
        ])
    }

    fn handle_initialize_response(&mut self, message: &Value) -> Vec<ProtocolEffect> {
        let response = match acp::validate_initialize_response(message, INITIALIZE_REQUEST_ID) {
            Ok(response) => response,
            Err(error) if error.is_remote_error() => {
                self.phase = ProtocolPhase::Finished;
                return vec![ProtocolEffect::Fail(ProtocolFailure::new(
                    "openclaw_acp_initialize_failed",
                    "OpenClaw ACP initialization failed.",
                    acp::INITIALIZE_METHOD,
                ))];
            }
            Err(error) => {
                self.phase = ProtocolPhase::Finished;
                return vec![ProtocolEffect::Fail(ProtocolFailure::from_acp(
                    error,
                    acp::INITIALIZE_METHOD,
                ))];
            }
        };
        if !response.capabilities.load_session {
            self.phase = ProtocolPhase::Finished;
            return vec![ProtocolEffect::Fail(ProtocolFailure::new(
                "openclaw_acp_capability_mismatch",
                "OpenClaw ACP does not expose the required conversation lifecycle.",
                "initialize/capabilities",
            ))];
        }
        self.phase = ProtocolPhase::AwaitSession;
        let request = session_request(&self.config);
        self.request_effect(request)
    }

    fn request_effect(&mut self, request: Result<Value, ProtocolFailure>) -> Vec<ProtocolEffect> {
        match request {
            Ok(request) => vec![ProtocolEffect::Send(request)],
            Err(mut failure) => {
                self.phase = ProtocolPhase::Finished;
                failure.session_id = self.binding.failure_session_id(&self.config);
                vec![ProtocolEffect::Fail(failure)]
            }
        }
    }

    fn handle_session_response(&mut self, message: &Value) -> Vec<ProtocolEffect> {
        let method = session_method(&self.config);
        let response = match acp::validate_session_response(message, SESSION_REQUEST_ID, method) {
            Ok(response) => response,
            Err(error) if error.is_remote_error() => {
                self.phase = ProtocolPhase::Finished;
                return vec![ProtocolEffect::Fail(self.failure_with_ids(
                    "openclaw_acp_session_open_failed",
                    "OpenClaw ACP could not open the requested conversation.",
                    method.method_name(),
                ))];
            }
            Err(error) => {
                self.phase = ProtocolPhase::Finished;
                let mut failure = ProtocolFailure::from_acp(error, method.method_name());
                failure.session_id = self.binding.failure_session_id(&self.config);
                return vec![ProtocolEffect::Fail(failure)];
            }
        };
        if let Err(mut failure) = self.binding.reconcile_open_response(
            &self.config,
            response.session_id,
            method.method_name(),
        ) {
            self.phase = ProtocolPhase::Finished;
            failure.session_id = self.binding.failure_session_id(&self.config);
            failure.turn_id = Some(self.config.turn_id.clone());
            return vec![ProtocolEffect::Fail(failure)];
        }
        self.capture_effective_controls(message.get("result"));
        if self.config.reasoning_effort.is_some() {
            self.phase = ProtocolPhase::AwaitMode;
            vec![ProtocolEffect::Send(self.mode_request())]
        } else {
            self.phase = ProtocolPhase::AwaitPrompt;
            let request = self.prompt_request();
            self.request_effect(request)
        }
    }

    fn mode_request(&self) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": MODE_REQUEST_ID,
            "method": "session/set_mode",
            "params": {
                "sessionId": self.binding.protocol_id(),
                "modeId": self.config.reasoning_effort
            }
        })
    }

    fn handle_mode_response(&mut self, message: &Value) -> Vec<ProtocolEffect> {
        if response_is_error(message) {
            self.phase = ProtocolPhase::Finished;
            return vec![ProtocolEffect::Fail(self.failure_with_ids(
                "openclaw_acp_thought_level_failed",
                "OpenClaw ACP could not apply the requested thought level.",
                "session/set_mode",
            ))];
        }
        self.phase = ProtocolPhase::AwaitPrompt;
        let request = self.prompt_request();
        self.request_effect(request)
    }

    fn prompt_request(&self) -> Result<Value, ProtocolFailure> {
        let session_id = self.binding.protocol_id().ok_or_else(|| {
            ProtocolFailure::new(
                "openclaw_acp_session_id_missing",
                "OpenClaw ACP did not return a native conversation identifier.",
                acp::SESSION_PROMPT_METHOD,
            )
        })?;
        acp::text_prompt_request(PROMPT_REQUEST_ID, session_id, &self.config.prompt)
            .map_err(|error| ProtocolFailure::from_acp(error, acp::SESSION_PROMPT_METHOD))
    }

    fn handle_prompt_response(&mut self, message: &Value) -> Vec<ProtocolEffect> {
        let response = match acp::validate_prompt_response(message, PROMPT_REQUEST_ID) {
            Ok(response) => response,
            Err(error) if error.is_remote_error() => {
                self.phase = ProtocolPhase::Finished;
                return vec![ProtocolEffect::Fail(self.failure_with_ids(
                    "openclaw_acp_prompt_failed",
                    "OpenClaw ACP could not complete the requested turn.",
                    acp::SESSION_PROMPT_METHOD,
                ))];
            }
            Err(error) => {
                self.phase = ProtocolPhase::Finished;
                let mut failure = ProtocolFailure::from_acp(error, acp::SESSION_PROMPT_METHOD);
                failure.session_id = self.binding.failure_session_id(&self.config);
                return vec![ProtocolEffect::Fail(failure)];
            }
        };
        let stop_reason = response.stop_reason.as_str().to_owned();
        self.phase = ProtocolPhase::Finished;
        if !matches!(
            stop_reason.as_str(),
            "end_turn" | "max_tokens" | "max_turn_requests"
        ) {
            let mut failure = self.failure_with_ids(
                "openclaw_acp_turn_not_completed",
                "OpenClaw ACP did not complete the requested turn.",
                "session/prompt",
            );
            failure.turn_status = Some(stop_reason);
            return vec![ProtocolEffect::Fail(failure)];
        }
        if self.output.is_empty() {
            let mut failure = self.failure_with_ids(
                "openclaw_acp_final_message_missing",
                "OpenClaw ACP completed the turn without a final agent message.",
                "session/prompt",
            );
            failure.turn_status = Some(stop_reason);
            return vec![ProtocolEffect::Fail(failure)];
        }
        vec![ProtocolEffect::Complete(Box::new(ProtocolOutcome {
            output: self.output.clone(),
            session_id: self.binding.native_id().unwrap_or_default().to_string(),
            turn_id: self.config.turn_id.clone(),
            turn_status: stop_reason,
            effective: self.effective.clone(),
            transitions: crate::parser::completed_transitions(&self.output),
        }))]
    }

    fn handle_notification(&mut self, message: Value) -> Vec<ProtocolEffect> {
        if message.get("method").and_then(Value::as_str) != Some(acp::SESSION_UPDATE_METHOD) {
            return Vec::new();
        }
        let expected_session_id = self
            .binding
            .expected_protocol_id(&self.config)
            .map(str::to_string);
        if expected_session_id.is_none() && self.phase != ProtocolPhase::AwaitSession {
            self.phase = ProtocolPhase::Finished;
            return vec![ProtocolEffect::Fail(self.failure_with_ids(
                acp::AcpError::SessionMismatch.code(),
                "OpenClaw ACP sent an update before establishing its conversation.",
                acp::SESSION_UPDATE_METHOD,
            ))];
        }
        let update = match acp::validate_session_update(&message, expected_session_id.as_deref()) {
            Ok(update) => update,
            Err(error) => {
                self.phase = ProtocolPhase::Finished;
                let mut failure = ProtocolFailure::from_acp(error, acp::SESSION_UPDATE_METHOD);
                failure.session_id = self.binding.failure_session_id(&self.config);
                failure.turn_id = Some(self.config.turn_id.clone());
                return vec![ProtocolEffect::Fail(failure)];
            }
        };
        if self.phase == ProtocolPhase::AwaitSession {
            if expected_session_id.is_none()
                && update.kind != acp::AcpSessionUpdateKind::SessionInfoUpdate
            {
                self.phase = ProtocolPhase::Finished;
                return vec![ProtocolEffect::Fail(self.failure_with_ids(
                    acp::AcpError::SessionMismatch.code(),
                    "OpenClaw ACP sent output before identifying its conversation.",
                    acp::SESSION_UPDATE_METHOD,
                ))];
            }
            self.binding.capture_opening_update(&update);
            return Vec::new();
        }
        let text = update.agent_message_text().map(str::to_owned);
        let current_mode = update.current_mode_id().map(str::to_owned);
        if self.phase == ProtocolPhase::AwaitPrompt {
            if let Some(evidence_kind) = update.kind().processing_evidence_kind() {
                let session_for_emit = self
                    .binding
                    .native_id()
                    .or(self.binding.protocol_id())
                    .unwrap_or_default();
                crate::port::turn_event::emit_agent_processing(
                    session_for_emit,
                    &self.config.turn_id,
                    evidence_kind,
                    None,
                );
            }
            let skill_events =
                licoup_agent_adapter_sdk::skill_invocation_projection::project_skill_invocations(
                    update.payload(),
                );
            if !skill_events.is_empty() {
                self.events.extend(skill_events);
            } else if let Some(event) = projected_event(&update) {
                self.events.push(event);
            }
            if let Some(text) = text {
                self.output.push_str(&text);
                let session_for_emit = self
                    .binding
                    .native_id()
                    .or(self.binding.protocol_id())
                    .unwrap_or_default();
                crate::port::turn_event::emit_agent_message_chunk(
                    session_for_emit,
                    &self.config.turn_id,
                    &text,
                );
            }
        }
        if let Some(level) = current_mode {
            self.effective.reasoning_effort = Some(level);
        }
        Vec::new()
    }

    fn capture_effective_controls(&mut self, result: Option<&Value>) {
        let Some(result) = result else {
            return;
        };
        if let Some(level) = result
            .pointer("/modes/currentModeId")
            .and_then(Value::as_str)
        {
            self.effective.reasoning_effort = Some(level.to_string());
        }
    }

    pub fn failure_with_ids(
        &self,
        code: &'static str,
        message: &'static str,
        stage: &'static str,
    ) -> ProtocolFailure {
        ProtocolFailure::new(code, message, stage).with_ids(
            self.binding.failure_session_id(&self.config),
            &self.config.turn_id,
        )
    }
}
