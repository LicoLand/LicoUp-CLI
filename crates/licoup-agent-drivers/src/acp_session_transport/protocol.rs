// The persistent-ACP semantic reducer. It serves every Agent that speaks the
// persistent ACP dialect; how a line decodes and what a frame means arrive
// through [`crate::acp_driver_runtime::parser_port`], so this file names no Agent.
use super::capabilities::EffectiveSettings;
use super::command::ProtocolConfig;
use super::errors::ProtocolFailure;
use crate::acp_driver_runtime::parser_port::{AcpParserRegistration, ProtocolPermissionRequest};
use licoup_foundation::core::acp::{
    self, AcpClientCapabilities, AcpImplementation, AcpSessionMethod, AcpSessionOptions,
};
use serde_json::{Value, json};
use std::path::Path;

pub use crate::state_machines::hermes_protocol::State as ProtocolPhase;
use crate::state_machines::hermes_protocol::{self as protocol_machine, Event as ProtocolEvent};

pub const INITIALIZE_REQUEST_ID: i64 = 1;
pub const SESSION_REQUEST_ID: i64 = 2;
pub const MODEL_REQUEST_ID: i64 = 3;
pub const PROMPT_REQUEST_ID: i64 = 4;

#[derive(Clone, Debug)]
pub struct ProtocolOutcome {
    pub output: String,
    pub events: Vec<Value>,
    pub session_id: String,
    pub turn_id: String,
    pub turn_status: String,
    pub effective: EffectiveSettings,
}

#[derive(Debug)]
pub enum ProtocolEffect {
    Send(Value),
    Complete(ProtocolOutcome),
    Fail(ProtocolFailure),
    /// Durable pause: keep the ACP process waiting until the user resolves.
    AwaitExternalApproval {
        request_id: Value,
        display_summary: String,
        option_id: Option<String>,
        requested_tools: Vec<String>,
    },
}

pub struct ParsedProtocolFrame {
    pub effects: Vec<ProtocolEffect>,
}

#[derive(Debug)]
pub struct SessionProtocol {
    /// This session's frame dialect, resolved by driver identity when the
    /// protocol is built. It is the whole of what this reducer may not name.
    pub parser: AcpParserRegistration,
    pub config: ProtocolConfig,
    pub phase: ProtocolPhase,
    pub session_id: Option<String>,
    pub output: String,
    pub events: Vec<Value>,
    pub effective: EffectiveSettings,
    pub interaction_failure: Option<ProtocolFailure>,
}

impl SessionProtocol {
    pub fn new(config: ProtocolConfig, driver_id: &str) -> Self {
        // A build with no composition reads the shared dialect rather than
        // failing closed; see `crate::test_support`.
        #[cfg(any(test, feature = "test-support"))]
        crate::test_support::ensure();
        let parser = crate::acp_driver_runtime::parser_port::parser_for(driver_id);
        Self::with_dialect(config, parser)
    }

    /// Build one session's reducer over a dialect the caller already resolved.
    ///
    /// This is the constructor an Agent's own package uses: the package owns the
    /// dialect, so no port installation is needed for the reducer to read the
    /// right frames, and the dialect a caller hands over is the one this reducer
    /// reads for the whole session.
    pub fn with_dialect(config: ProtocolConfig, parser: AcpParserRegistration) -> Self {
        let effective = EffectiveSettings {
            cwd: Some(config.cwd.clone()),
            model: config.model.clone(),
            ..EffectiveSettings::default()
        };
        Self {
            parser,
            config,
            phase: protocol_machine::INITIAL,
            session_id: None,
            output: String::new(),
            events: Vec::new(),
            effective,
            interaction_failure: None,
        }
    }

    pub fn new_ready(config: ProtocolConfig, driver_id: &str) -> Self {
        let mut protocol = Self::new(config, driver_id);
        protocol.advance(ProtocolEvent::TransportReady);
        protocol
    }

    fn advance(&mut self, event: ProtocolEvent) {
        self.phase = protocol_machine::transition(self.phase, event).unwrap_or_else(|| {
            panic!(
                "invalid Hermes protocol transition: {} + {}",
                self.phase.as_str(),
                event.as_str()
            )
        });
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
        if let Some(request) = (self.parser.permission_request)(&message)
            && let Some(effects) = self.handle_server_request(request)
        {
            return effects;
        }
        if (self.parser.is_notification)(&message) {
            return self.handle_notification(message);
        }
        match self.phase {
            ProtocolPhase::AwaitInitialize
                if (self.parser.response_id_matches)(&message, INITIALIZE_REQUEST_ID) =>
            {
                self.handle_initialize_response(&message)
            }
            ProtocolPhase::AwaitSession
                if (self.parser.response_id_matches)(&message, SESSION_REQUEST_ID) =>
            {
                self.handle_session_response(&message)
            }
            ProtocolPhase::AwaitModel
                if (self.parser.response_id_matches)(&message, MODEL_REQUEST_ID) =>
            {
                self.handle_model_response(&message)
            }
            ProtocolPhase::AwaitPrompt
                if (self.parser.response_id_matches)(&message, PROMPT_REQUEST_ID) =>
            {
                self.handle_prompt_response(&message)
            }
            _ => Vec::new(),
        }
    }

    pub fn handle_frame(&mut self, line: &[u8]) -> ParsedProtocolFrame {
        let message = match (self.parser.decode_frame)(line) {
            Ok(message) => message,
            Err(error) => {
                self.advance(ProtocolEvent::Fail);
                return ParsedProtocolFrame {
                    effects: vec![ProtocolEffect::Fail(ProtocolFailure::from_acp(
                        error,
                        "protocol/read",
                    ))],
                };
            }
        };
        ParsedProtocolFrame {
            effects: self.handle_message(message),
        }
    }

    pub fn handle_server_request(
        &mut self,
        request: ProtocolPermissionRequest,
    ) -> Option<Vec<ProtocolEffect>> {
        let request_id = request.id;
        let method = request.method.as_str();
        if method == "session/request_permission" {
            // Fail closed when the lane lacks a durable pause handle (no session).
            let Some(session_id) = self.session_id.as_deref() else {
                return Some(self.fail_closed_permission_denial(&request_id, method));
            };
            if request
                .session_id
                .as_deref()
                .is_some_and(|id| id != session_id)
            {
                return Some(self.fail_closed_permission_denial(&request_id, method));
            }
            return Some(vec![ProtocolEffect::AwaitExternalApproval {
                request_id,
                display_summary: request.display_summary,
                option_id: request.option_id,
                requested_tools: request.requested_tools,
            }]);
        }
        self.interaction_failure = Some(ProtocolFailure::user_interaction(
            method,
            self.session_id.as_deref(),
            Some(&self.config.turn_id),
        ));
        let mut effects = vec![ProtocolEffect::Send(json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "error": {"code": -32601, "message": "Client method is not available."}
        }))];
        if let Some(session_id) = self.session_id.as_deref() {
            match acp::cancel_notification(session_id) {
                Ok(notification) => effects.push(ProtocolEffect::Send(notification)),
                Err(error) => {
                    let mut failure = ProtocolFailure::from_acp(error, acp::SESSION_CANCEL_METHOD);
                    failure.session_id = self.session_id.clone();
                    effects.push(ProtocolEffect::Fail(failure));
                }
            }
        }
        Some(effects)
    }

    pub fn fail_closed_permission_denial(
        &mut self,
        request_id: &Value,
        method: &str,
    ) -> Vec<ProtocolEffect> {
        self.interaction_failure = Some(ProtocolFailure::user_interaction(
            method,
            self.session_id.as_deref(),
            Some(&self.config.turn_id),
        ));
        let mut effects = vec![ProtocolEffect::Send(json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {"outcome": {"outcome": "cancelled"}}
        }))];
        if let Some(session_id) = self.session_id.as_deref() {
            match acp::cancel_notification(session_id) {
                Ok(notification) => effects.push(ProtocolEffect::Send(notification)),
                Err(error) => {
                    let mut failure = ProtocolFailure::from_acp(error, acp::SESSION_CANCEL_METHOD);
                    failure.session_id = self.session_id.clone();
                    effects.push(ProtocolEffect::Fail(failure));
                }
            }
        }
        effects
    }

    pub fn handle_initialize_response(&mut self, message: &Value) -> Vec<ProtocolEffect> {
        let response = match acp::validate_initialize_response(message, INITIALIZE_REQUEST_ID) {
            Ok(response) => response,
            Err(error) if error.is_remote_error() => {
                self.advance(ProtocolEvent::Fail);
                return vec![ProtocolEffect::Fail(ProtocolFailure::new(
                    "hermes_acp_initialize_failed",
                    "Hermes ACP initialization failed.",
                    acp::INITIALIZE_METHOD,
                ))];
            }
            Err(error) => {
                self.advance(ProtocolEvent::Fail);
                return vec![ProtocolEffect::Fail(ProtocolFailure::from_acp(
                    error,
                    acp::INITIALIZE_METHOD,
                ))];
            }
        };
        if !response.capabilities.load_session {
            self.advance(ProtocolEvent::Fail);
            return vec![ProtocolEffect::Fail(ProtocolFailure::new(
                "hermes_acp_capability_mismatch",
                "Hermes ACP does not expose the required conversation lifecycle.",
                "initialize/capabilities",
            ))];
        }
        self.advance(ProtocolEvent::Initialized);
        let request = self.session_request();
        self.request_effect(request)
    }

    pub fn session_request(&self) -> Result<Value, ProtocolFailure> {
        let method = if self.config.is_resume() {
            AcpSessionMethod::Load(&self.config.requested_session_id)
        } else {
            AcpSessionMethod::New
        };
        let stage = method.method_name();
        acp::session_request(
            SESSION_REQUEST_ID,
            method,
            AcpSessionOptions::new(Path::new(&self.config.cwd))
                .mcp_servers(&self.config.mcp_servers),
        )
        .map_err(|error| ProtocolFailure::from_acp(error, stage))
    }

    pub fn request_effect(
        &mut self,
        request: Result<Value, ProtocolFailure>,
    ) -> Vec<ProtocolEffect> {
        match request {
            Ok(request) => vec![ProtocolEffect::Send(request)],
            Err(mut failure) => {
                self.advance(ProtocolEvent::Fail);
                failure.session_id = self.session_id.clone();
                vec![ProtocolEffect::Fail(failure)]
            }
        }
    }

    pub fn handle_session_response(&mut self, message: &Value) -> Vec<ProtocolEffect> {
        let method = if self.config.is_resume() {
            AcpSessionMethod::Load(&self.config.requested_session_id)
        } else {
            AcpSessionMethod::New
        };
        let method_name = method.method_name();
        let response = match acp::validate_session_response(message, SESSION_REQUEST_ID, method) {
            Ok(response) => response,
            Err(error) if error.is_remote_error() => {
                self.advance(ProtocolEvent::Fail);
                return vec![ProtocolEffect::Fail(self.failure_with_ids(
                    "hermes_acp_session_open_failed",
                    "Hermes ACP could not open the requested conversation.",
                    method_name,
                ))];
            }
            Err(error) => {
                self.advance(ProtocolEvent::Fail);
                let mut failure = ProtocolFailure::from_acp(error, method_name);
                failure.session_id = self
                    .config
                    .is_resume()
                    .then(|| self.config.requested_session_id.clone());
                return vec![ProtocolEffect::Fail(failure)];
            }
        };
        let session_id = if self.config.is_resume() {
            if response
                .session_id
                .as_deref()
                .is_some_and(|returned| returned != self.config.requested_session_id)
            {
                self.advance(ProtocolEvent::Fail);
                return vec![ProtocolEffect::Fail(self.failure_with_ids(
                    "hermes_acp_session_mismatch",
                    "Hermes ACP returned a different conversation than the one requested.",
                    acp::SESSION_LOAD_METHOD,
                ))];
            }
            self.config.requested_session_id.clone()
        } else {
            response.session_id.unwrap_or_default()
        };
        if session_id.is_empty() {
            self.advance(ProtocolEvent::Fail);
            return vec![ProtocolEffect::Fail(ProtocolFailure::new(
                "hermes_acp_session_id_missing",
                "Hermes ACP did not return a native conversation identifier.",
                "session/open",
            ))];
        }
        self.session_id = Some(session_id);
        self.capture_effective_controls(message.get("result"));
        if self.config.model.is_some() {
            self.advance(ProtocolEvent::SessionModel);
            vec![ProtocolEffect::Send(self.model_request())]
        } else {
            self.advance(ProtocolEvent::SessionPrompt);
            let request = self.prompt_request();
            self.request_effect(request)
        }
    }

    pub fn model_request(&self) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": MODEL_REQUEST_ID,
            "method": "session/set_model",
            "params": {
                "sessionId": self.session_id,
                "modelId": self.config.model
            }
        })
    }

    pub fn handle_model_response(&mut self, message: &Value) -> Vec<ProtocolEffect> {
        if (self.parser.response_is_error)(message) {
            self.advance(ProtocolEvent::Fail);
            return vec![ProtocolEffect::Fail(self.failure_with_ids(
                "hermes_acp_model_override_failed",
                "Hermes ACP could not apply the requested model.",
                "session/set_model",
            ))];
        }
        self.effective.model = self.config.model.clone();
        self.advance(ProtocolEvent::ModelReady);
        let request = self.prompt_request();
        self.request_effect(request)
    }

    pub fn prompt_request(&self) -> Result<Value, ProtocolFailure> {
        let session_id = self.session_id.as_deref().ok_or_else(|| {
            ProtocolFailure::new(
                "hermes_acp_session_id_missing",
                "Hermes ACP did not return a native conversation identifier.",
                acp::SESSION_PROMPT_METHOD,
            )
        })?;
        acp::text_prompt_request(PROMPT_REQUEST_ID, session_id, &self.config.prompt)
            .map_err(|error| ProtocolFailure::from_acp(error, acp::SESSION_PROMPT_METHOD))
    }

    pub fn handle_prompt_response(&mut self, message: &Value) -> Vec<ProtocolEffect> {
        let stop_reason = match (self.parser.prompt_stop_reason)(message, PROMPT_REQUEST_ID) {
            Ok(stop_reason) => stop_reason,
            Err(error) if error.is_remote_error() => {
                self.advance(ProtocolEvent::Fail);
                return vec![ProtocolEffect::Fail(self.failure_with_ids(
                    "hermes_acp_prompt_failed",
                    "Hermes ACP could not complete the requested turn.",
                    acp::SESSION_PROMPT_METHOD,
                ))];
            }
            Err(error) => {
                self.advance(ProtocolEvent::Fail);
                let mut failure = ProtocolFailure::from_acp(error, acp::SESSION_PROMPT_METHOD);
                failure.session_id = self.session_id.clone();
                return vec![ProtocolEffect::Fail(failure)];
            }
        };
        let stop_reason = stop_reason.as_str().to_owned();
        self.advance(ProtocolEvent::PromptCompleted);
        if let Some(mut failure) = self.interaction_failure.take() {
            failure.turn_status = Some(stop_reason);
            return vec![ProtocolEffect::Fail(failure)];
        }
        // Cancelled / interrupted turns still expose the native session id on the
        // failure so a later boundary send can exact-resume that same conversation.
        if !matches!(
            stop_reason.as_str(),
            "end_turn" | "max_tokens" | "max_turn_requests"
        ) {
            let mut failure = self.failure_with_ids(
                "hermes_acp_turn_not_completed",
                "Hermes ACP did not complete the requested turn.",
                "session/prompt",
            );
            failure.turn_status = Some(stop_reason);
            return vec![ProtocolEffect::Fail(failure)];
        }
        if self.output.is_empty() {
            let mut failure = self.failure_with_ids(
                "hermes_acp_final_message_missing",
                "Hermes ACP completed the turn without a final agent message.",
                "session/prompt",
            );
            failure.turn_status = Some(stop_reason);
            return vec![ProtocolEffect::Fail(failure)];
        }
        let session_id = self.session_id.clone().unwrap_or_default();
        licoup_foundation::platform::turn_event_emit::emit_agent_message_completed(
            &session_id,
            &self.config.turn_id,
            &self.output,
        );
        vec![ProtocolEffect::Complete(ProtocolOutcome {
            output: self.output.clone(),
            events: self.events.clone(),
            session_id,
            turn_id: self.config.turn_id.clone(),
            turn_status: stop_reason,
            effective: self.effective.clone(),
        })]
    }

    pub fn handle_notification(&mut self, message: Value) -> Vec<ProtocolEffect> {
        if message.get("method").and_then(Value::as_str) != Some(acp::SESSION_UPDATE_METHOD) {
            return Vec::new();
        }
        let Some(expected_session_id) = self.session_id.as_deref() else {
            self.advance(ProtocolEvent::Fail);
            return vec![ProtocolEffect::Fail(self.failure_with_ids(
                acp::AcpError::SessionMismatch.code(),
                "Hermes ACP sent an update before establishing its conversation.",
                acp::SESSION_UPDATE_METHOD,
            ))];
        };
        let update = match (self.parser.session_update)(&message, Some(expected_session_id)) {
            Ok(update) => update,
            Err(error) => {
                self.advance(ProtocolEvent::Fail);
                let mut failure = ProtocolFailure::from_acp(error, acp::SESSION_UPDATE_METHOD);
                failure.session_id = self.session_id.clone();
                failure.turn_id = Some(self.config.turn_id.clone());
                return vec![ProtocolEffect::Fail(failure)];
            }
        };
        // Both facts are taken before the match below consumes `update` through
        // `payload()`, so neither borrows the frame after it is moved.
        let payload = update.payload().clone();
        let text = update.agent_message_text().map(str::to_owned);
        let current_mode = update.current_mode_id().map(str::to_owned);
        if self.phase == ProtocolPhase::AwaitPrompt {
            if let Some(evidence_kind) = update.kind().processing_evidence_kind() {
                licoup_foundation::platform::turn_event_emit::emit_agent_processing(
                    self.session_id.as_deref().unwrap_or_default(),
                    &self.config.turn_id,
                    evidence_kind,
                    None,
                );
            }
            let skill_events =
                licoup_agent_adapter_sdk::skill_invocation_projection::project_skill_invocations(
                    &payload,
                );
            if skill_events.is_empty() {
                self.events.push(payload.clone());
            } else {
                self.events.extend(skill_events);
            }
            if let Some(text) = text {
                self.output.push_str(&text);
                licoup_foundation::platform::turn_event_emit::emit_agent_message_chunk(
                    self.session_id.as_deref().unwrap_or_default(),
                    &self.config.turn_id,
                    &text,
                );
            }
        }
        if let Some(mode) = current_mode {
            self.effective.approval_policy = Some(json!(mode));
        }
        Vec::new()
    }

    pub fn capture_effective_controls(&mut self, result: Option<&Value>) {
        let Some(result) = result else {
            return;
        };
        if self.config.model.is_none()
            && let Some(model) = result
                .pointer("/models/currentModelId")
                .and_then(Value::as_str)
        {
            self.effective.model = Some(model.to_string());
        }
        if let Some(mode) = result
            .pointer("/modes/currentModeId")
            .and_then(Value::as_str)
        {
            self.effective.approval_policy = Some(json!(mode));
        }
    }

    pub fn failure_with_ids(
        &self,
        code: &'static str,
        message: &'static str,
        stage: &'static str,
    ) -> ProtocolFailure {
        let mut failure = ProtocolFailure::new(code, message, stage);
        failure.session_id = self.session_id.clone().or_else(|| {
            (!self.config.requested_session_id.is_empty())
                .then(|| self.config.requested_session_id.clone())
        });
        failure.turn_id = Some(self.config.turn_id.clone());
        failure
    }
}
