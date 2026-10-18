// The shared ACP state reducer. One reducer serves every Agent that speaks the
// shared ACP dialect; the Agent's own frame policy — how a line decodes, what a
// frame means, how a failure is worded — arrives through
// [`super::parser_port`], so this file names no Agent.
use super::errors::{ProtocolFailure, failure_from_response};
use super::model::{CapabilityProbe, EffectiveSettings};
use super::params::ProtocolConfig;
use super::parser_port::{AcpParserRegistration, ProtocolClientRequest};
use super::session_plan::{AcpSessionPlan, reconcile_acp_session_id, select_acp_session_plan};
use super::settings::{
    ConfigChange, config_request, effective_settings, requested_config_changes, setting_applied,
};
use licoup_foundation::core::acp::{
    self, AcpClientCapabilities, AcpImplementation, AcpSessionOptions,
};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::Path;
use uuid::Uuid;

pub use crate::state_machines::copilot_protocol::State as ProtocolPhase;
use crate::state_machines::copilot_protocol::{self as protocol_machine, Event as ProtocolEvent};

pub const INITIALIZE_REQUEST_ID: i64 = 1;
pub const SESSION_REQUEST_ID: i64 = 2;
pub const PROMPT_REQUEST_ID: i64 = 3;
pub const FIRST_CONFIG_REQUEST_ID: i64 = 10;

#[derive(Debug)]
pub struct ProtocolOutcome {
    pub output: String,
    pub session_id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub turn_status: String,
    pub effective: EffectiveSettings,
    pub capabilities: CapabilityProbe,
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

#[derive(Debug)]
pub struct AcpProtocol {
    /// This turn's frame dialect, resolved by driver identity when the
    /// protocol is built. It is the whole of what this reducer may not name.
    pub parser: AcpParserRegistration,
    pub config: ProtocolConfig,
    pub phase: ProtocolPhase,
    pub capabilities: CapabilityProbe,
    pub session_id: Option<String>,
    pub config_options: Vec<Value>,
    pub modes: Option<Value>,
    pub pending_changes: VecDeque<ConfigChange>,
    pub current_change: Option<ConfigChange>,
    pub next_config_request_id: i64,
    pub output: String,
    pub events: Vec<Value>,
    pub interaction_failure: Option<ProtocolFailure>,
    pub turn_id: String,
}

impl AcpProtocol {
    fn advance(&mut self, event: ProtocolEvent) {
        self.phase = protocol_machine::transition(self.phase, event).unwrap_or_else(|| {
            panic!(
                "invalid ACP protocol transition: {} + {}",
                self.phase.as_str(),
                event.as_str()
            )
        });
    }

    pub fn new(config: ProtocolConfig, driver_id: &str) -> Self {
        // A build with no composition reads the shared dialect rather than
        // failing closed; see `crate::test_support`.
        #[cfg(any(test, feature = "test-support"))]
        crate::test_support::ensure();
        let parser = super::parser_port::parser_for(driver_id);
        Self::with_dialect(config, parser)
    }

    /// Build one turn's reducer over a dialect the caller already resolved.
    ///
    /// This is the constructor an Agent's own package uses: the package owns the
    /// dialect, so no port installation is needed for the reducer to read the
    /// right frames, and the dialect a caller hands over is the one this reducer
    /// reads for the whole turn. The driver identity the turn is keyed on is the
    /// dialect's own, so a reducer cannot read one Agent's frames while being
    /// keyed on another's.
    pub fn with_dialect(
        config: ProtocolConfig,
        parser: super::parser_port::AcpParserRegistration,
    ) -> Self {
        Self {
            parser,
            config,
            phase: protocol_machine::INITIAL,
            capabilities: CapabilityProbe::default(),
            session_id: None,
            config_options: Vec::new(),
            modes: None,
            pending_changes: VecDeque::new(),
            current_change: None,
            next_config_request_id: FIRST_CONFIG_REQUEST_ID,
            output: String::new(),
            events: Vec::new(),
            interaction_failure: None,
            turn_id: Uuid::new_v4().to_string(),
        }
    }

    pub fn initial_request(&self) -> Result<Value, ProtocolFailure> {
        acp::initialize_request(
            INITIALIZE_REQUEST_ID,
            &AcpImplementation::new("lico-up", env!("CARGO_PKG_VERSION")),
            AcpClientCapabilities::default(),
        )
        .map_err(|error| ProtocolFailure::from_acp(error, acp::INITIALIZE_METHOD))
    }

    pub fn handle_message(&mut self, message: Value) -> Vec<ProtocolEffect> {
        if let Some(request) = self.client_request(&message) {
            return self.handle_server_request(request);
        }
        if self.is_notification(&message) {
            return self.handle_notification(&message);
        }
        match self.phase {
            ProtocolPhase::AwaitInitialize
                if self.response_id_matches(&message, INITIALIZE_REQUEST_ID) =>
            {
                self.handle_initialize_response(&message)
            }
            ProtocolPhase::AwaitSession
                if self.response_id_matches(&message, SESSION_REQUEST_ID) =>
            {
                self.handle_session_response(&message)
            }
            ProtocolPhase::AwaitConfig
                if self.current_change.as_ref().is_some_and(|_| {
                    self.response_id_matches(&message, self.next_config_request_id - 1)
                }) =>
            {
                self.handle_config_response(&message)
            }
            ProtocolPhase::AwaitPrompt if self.response_id_matches(&message, PROMPT_REQUEST_ID) => {
                self.handle_prompt_response(&message)
            }
            _ => Vec::new(),
        }
    }

    pub fn handle_frame(&mut self, line: &[u8]) -> ParsedProtocolFrame {
        let decoded = (self.parser.decode_frame)(line);
        let message = match decoded {
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

    pub fn handle_initialize_response(&mut self, message: &Value) -> Vec<ProtocolEffect> {
        let response = match acp::validate_initialize_response(message, INITIALIZE_REQUEST_ID) {
            Ok(response) => response,
            Err(error) if error.is_remote_error() => {
                self.advance(ProtocolEvent::Fail);
                return vec![ProtocolEffect::Fail(failure_from_response(
                    message,
                    "acp_initialize_rejected",
                    "The ACP agent rejected protocol initialization.",
                    acp::INITIALIZE_METHOD,
                    None,
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
        self.capabilities = CapabilityProbe::from_initialize(&response);

        self.advance(ProtocolEvent::Initialized);
        let plan = match select_acp_session_plan(&self.config, &self.capabilities) {
            Ok(plan) => plan,
            Err(failure) => {
                self.advance(ProtocolEvent::Fail);
                return vec![ProtocolEffect::Fail(failure)];
            }
        };
        if self.config.is_resume() {
            self.session_id = Some(self.config.requested_session_id.clone());
        }
        let request = self.session_request(plan);
        self.request_effect(request)
    }

    pub fn session_request(&self, plan: AcpSessionPlan) -> Result<Value, ProtocolFailure> {
        acp::session_request(
            SESSION_REQUEST_ID,
            plan.method(&self.config.requested_session_id),
            AcpSessionOptions::new(Path::new(&self.config.cwd))
                .mcp_servers(&self.config.mcp_servers),
        )
        .map_err(|error| ProtocolFailure::from_acp(error, "session/setup"))
    }

    pub fn request_effect(
        &mut self,
        request: Result<Value, ProtocolFailure>,
    ) -> Vec<ProtocolEffect> {
        match request {
            Ok(request) => vec![ProtocolEffect::Send(request)],
            Err(failure) => {
                self.advance(ProtocolEvent::Fail);
                vec![ProtocolEffect::Fail(
                    failure.with_session(self.session_id.as_deref()),
                )]
            }
        }
    }

    pub fn handle_session_response(&mut self, message: &Value) -> Vec<ProtocolEffect> {
        let plan = match select_acp_session_plan(&self.config, &self.capabilities) {
            Ok(plan) => plan,
            Err(failure) => {
                self.advance(ProtocolEvent::Fail);
                return vec![ProtocolEffect::Fail(failure)];
            }
        };
        let response = match acp::validate_session_response(
            message,
            SESSION_REQUEST_ID,
            plan.method(&self.config.requested_session_id),
        ) {
            Ok(response) => response,
            Err(error) if error.is_remote_error() => {
                self.advance(ProtocolEvent::Fail);
                return vec![ProtocolEffect::Fail(failure_from_response(
                    message,
                    "acp_session_rejected",
                    "The ACP agent rejected the native conversation session.",
                    "session/setup",
                    self.session_id.as_deref(),
                ))];
            }
            Err(error) => {
                self.advance(ProtocolEvent::Fail);
                return vec![ProtocolEffect::Fail(
                    ProtocolFailure::from_acp(error, "session/setup")
                        .with_session(self.session_id.as_deref()),
                )];
            }
        };
        self.session_id = match reconcile_acp_session_id(&self.config, plan, response.session_id) {
            Ok(session_id) => Some(session_id),
            Err(failure) => {
                self.advance(ProtocolEvent::Fail);
                return vec![ProtocolEffect::Fail(failure)];
            }
        };
        self.config_options = response.config_options;
        self.modes = response.modes;
        match requested_config_changes(
            &self.config.settings,
            &self.config_options,
            self.session_id.as_deref(),
        ) {
            Ok(changes) => self.pending_changes = changes,
            Err(failure) => {
                self.advance(ProtocolEvent::Fail);
                return vec![ProtocolEffect::Fail(failure)];
            }
        }
        self.next_config_or_prompt()
    }

    pub fn next_config_or_prompt(&mut self) -> Vec<ProtocolEffect> {
        if let Some(change) = self.pending_changes.pop_front() {
            let request_id = self.next_config_request_id;
            self.next_config_request_id += 1;
            let request = config_request(request_id, self.session_id.as_deref(), &change);
            self.current_change = Some(change);
            self.advance(ProtocolEvent::RequestConfig);
            return vec![ProtocolEffect::Send(request)];
        }
        self.current_change = None;
        self.advance(ProtocolEvent::RequestPrompt);
        let request = self
            .session_id
            .as_deref()
            .ok_or_else(|| {
                ProtocolFailure::new(
                    "acp_session_id_missing",
                    "The ACP agent did not return a native conversation identifier.",
                    acp::SESSION_PROMPT_METHOD,
                )
            })
            .and_then(|session_id| {
                acp::text_prompt_request(PROMPT_REQUEST_ID, session_id, &self.config.prompt)
                    .map_err(|error| ProtocolFailure::from_acp(error, acp::SESSION_PROMPT_METHOD))
            });
        self.request_effect(request)
    }

    pub fn handle_config_response(&mut self, message: &Value) -> Vec<ProtocolEffect> {
        if message.get("error").is_some() {
            self.advance(ProtocolEvent::Fail);
            return vec![ProtocolEffect::Fail(failure_from_response(
                message,
                "acp_setting_rejected",
                "The ACP agent rejected a requested native session setting.",
                "session/set_config_option",
                self.session_id.as_deref(),
            ))];
        }
        let Some(options) = message
            .get("result")
            .and_then(|result| result.get("configOptions"))
            .and_then(Value::as_array)
        else {
            self.advance(ProtocolEvent::Fail);
            return vec![ProtocolEffect::Fail(
                ProtocolFailure::new(
                    "acp_setting_response_invalid",
                    "The ACP agent returned an invalid setting response.",
                    "session/set_config_option",
                )
                .with_session(self.session_id.as_deref()),
            )];
        };
        self.config_options = options.clone();
        if let Some(change) = self.current_change.as_ref()
            && !setting_applied(&self.config_options, change)
        {
            self.advance(ProtocolEvent::Fail);
            return vec![ProtocolEffect::Fail(
                ProtocolFailure::new(
                    "acp_setting_not_applied",
                    "The ACP agent did not apply a requested native session setting.",
                    "session/set_config_option",
                )
                .with_session(self.session_id.as_deref()),
            )];
        }
        self.current_change = None;
        self.next_config_or_prompt()
    }

    pub fn handle_notification(&mut self, message: &Value) -> Vec<ProtocolEffect> {
        if message.get("method").and_then(Value::as_str) != Some(acp::SESSION_UPDATE_METHOD) {
            return Vec::new();
        }
        let Some(expected_session_id) = self.session_id.as_deref() else {
            // ACP agents may emit session/update notifications for the
            // conversation that is still being created: real Copilot sends
            // available_commands_update before the session/new response.
            // The update necessarily concerns the pending session of this
            // single-session process, so validate its shape and drop it
            // instead of failing the turn. Output accumulation stays gated
            // to the prompt phases, where binding is strictly enforced.
            return match self.session_update(message, None) {
                Ok(_) => Vec::new(),
                Err(error) => {
                    self.advance(ProtocolEvent::Fail);
                    vec![ProtocolEffect::Fail(ProtocolFailure::from_acp(
                        error,
                        acp::SESSION_UPDATE_METHOD,
                    ))]
                }
            };
        };
        let update = match self.session_update(message, Some(expected_session_id)) {
            Ok(update) => update,
            Err(error) => {
                self.advance(ProtocolEvent::Fail);
                return vec![ProtocolEffect::Fail(
                    ProtocolFailure::from_acp(error, acp::SESSION_UPDATE_METHOD)
                        .with_session(self.session_id.as_deref()),
                )];
            }
        };
        if let Some(options) = update.config_options() {
            self.config_options = options.to_vec();
        }
        if let Some(mode) = update.current_mode_id()
            && let Some(modes) = self.modes.as_mut()
        {
            modes["currentModeId"] = json!(mode);
        }
        if !matches!(self.phase, ProtocolPhase::AwaitPrompt) {
            return Vec::new();
        }
        if let Some(evidence_kind) = update.kind().processing_evidence_kind() {
            licoup_foundation::platform::turn_event_emit::emit_agent_processing(
                self.session_id.as_deref().unwrap_or_default(),
                &self.turn_id,
                evidence_kind,
                None,
            );
        }
        if let Some(text) = update.agent_message_text() {
            self.output.push_str(text);
            licoup_foundation::platform::turn_event_emit::emit_agent_message_chunk(
                self.session_id.as_deref().unwrap_or_default(),
                &self.turn_id,
                text,
            );
        }
        let skill_events =
            licoup_agent_adapter_sdk::skill_invocation_projection::project_skill_invocations(
                update.payload(),
            );
        if skill_events.is_empty() {
            self.events.push(update.into_payload());
        } else {
            self.events.extend(skill_events);
        }
        Vec::new()
    }

    fn handle_server_request(&mut self, request: ProtocolClientRequest) -> Vec<ProtocolEffect> {
        let method = request.method.as_str();
        let id = request.id;
        if method == "session/request_permission" {
            if request.session_id.as_deref() != self.session_id.as_deref() {
                self.advance(ProtocolEvent::Fail);
                return vec![
                    ProtocolEffect::Send(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": -32602, "message": "Session does not match"}
                    })),
                    ProtocolEffect::Fail(
                        ProtocolFailure::new(
                            "acp_session_mismatch",
                            "The ACP agent requested interaction for a different session.",
                            "session/request_permission",
                        )
                        .with_session(self.session_id.as_deref()),
                    ),
                ];
            }
            if self.config.allow_all_authorized
                && let Some(option_id) = request.allow_once_option
            {
                return vec![ProtocolEffect::Send(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "outcome": {
                            "outcome": "selected",
                            "optionId": option_id
                        }
                    }
                }))];
            }
            self.interaction_failure = Some(ProtocolFailure::user_interaction(
                method,
                self.session_id.as_deref(),
            ));
            let mut effects = vec![ProtocolEffect::Send(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"outcome": {"outcome": "cancelled"}}
            }))];
            match self
                .session_id
                .as_deref()
                .ok_or(acp::AcpError::SessionIdInvalid)
                .and_then(acp::cancel_notification)
            {
                Ok(notification) => effects.push(ProtocolEffect::Send(notification)),
                Err(error) => effects.push(ProtocolEffect::Fail(
                    ProtocolFailure::from_acp(error, acp::SESSION_CANCEL_METHOD)
                        .with_session(self.session_id.as_deref()),
                )),
            }
            return effects;
        }
        self.advance(ProtocolEvent::Fail);
        vec![
            ProtocolEffect::Send(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": "Client method is not available"}
            })),
            ProtocolEffect::Fail(
                ProtocolFailure::new(
                    "acp_client_method_unsupported",
                    "The ACP agent requested a client capability that LicoUp did not advertise.",
                    "client/request",
                )
                .with_session(self.session_id.as_deref()),
            ),
        ]
    }

    pub fn handle_prompt_response(&mut self, message: &Value) -> Vec<ProtocolEffect> {
        let stop_reason = match self.prompt_stop_reason(message) {
            Ok(stop_reason) => stop_reason,
            Err(error) if error.is_remote_error() => {
                self.advance(ProtocolEvent::Fail);
                return vec![ProtocolEffect::Fail(failure_from_response(
                    message,
                    "acp_prompt_rejected",
                    "The ACP agent rejected the requested turn.",
                    acp::SESSION_PROMPT_METHOD,
                    self.session_id.as_deref(),
                ))];
            }
            Err(error) => {
                self.advance(ProtocolEvent::Fail);
                return vec![ProtocolEffect::Fail(
                    ProtocolFailure::from_acp(error, acp::SESSION_PROMPT_METHOD)
                        .with_session(self.session_id.as_deref()),
                )];
            }
        };
        if stop_reason == acp::AcpStopReason::Cancelled
            && let Some(mut failure) = self.interaction_failure.take()
        {
            self.advance(ProtocolEvent::Fail);
            failure.turn_status = Some(stop_reason.as_str().to_owned());
            failure.turn_id = Some(self.turn_id.clone());
            return vec![ProtocolEffect::Fail(failure)];
        }
        self.advance(ProtocolEvent::PromptCompleted);
        let stop_reason = stop_reason.as_str().to_owned();
        if let Some(mut failure) = self.interaction_failure.take() {
            failure.turn_status = Some(stop_reason);
            failure.turn_id = Some(self.turn_id.clone());
            return vec![ProtocolEffect::Fail(failure)];
        }
        if stop_reason != "end_turn" {
            let mut failure = ProtocolFailure::new(
                "acp_turn_not_completed",
                "The ACP agent did not complete the requested turn.",
                "session/prompt",
            )
            .with_session(self.session_id.as_deref());
            failure.turn_status = Some(stop_reason);
            failure.turn_id = Some(self.turn_id.clone());
            return vec![ProtocolEffect::Fail(failure)];
        }
        if self.output.is_empty() {
            let mut failure = ProtocolFailure::new(
                "acp_final_message_missing",
                "The ACP agent completed the turn without a final agent message.",
                "session/prompt",
            )
            .with_session(self.session_id.as_deref());
            failure.turn_status = Some(stop_reason);
            failure.turn_id = Some(self.turn_id.clone());
            return vec![ProtocolEffect::Fail(failure)];
        }
        let output = std::mem::take(&mut self.output);
        let transitions = (self.parser.completed_transitions)(&output);
        vec![ProtocolEffect::Complete(Box::new(ProtocolOutcome {
            output,
            session_id: self.session_id.clone().unwrap_or_default(),
            thread_id: self.session_id.clone().unwrap_or_default(),
            turn_id: self.turn_id.clone(),
            turn_status: stop_reason,
            effective: effective_settings(&self.config, &self.config_options, self.modes.as_ref()),
            capabilities: self.capabilities.clone(),
            transitions,
        }))]
    }

    fn client_request(&self, message: &Value) -> Option<ProtocolClientRequest> {
        (self.parser.client_request)(message)
    }

    fn is_notification(&self, message: &Value) -> bool {
        (self.parser.is_notification)(message)
    }

    fn response_id_matches(&self, message: &Value, expected: i64) -> bool {
        (self.parser.response_id_matches)(message, expected)
    }

    fn session_update(
        &self,
        message: &Value,
        expected_session_id: Option<&str>,
    ) -> Result<acp::AcpSessionUpdate, acp::AcpError> {
        (self.parser.session_update)(message, expected_session_id)
    }

    fn prompt_stop_reason(&self, message: &Value) -> Result<acp::AcpStopReason, acp::AcpError> {
        (self.parser.prompt_stop_reason)(message, PROMPT_REQUEST_ID)
    }
}
