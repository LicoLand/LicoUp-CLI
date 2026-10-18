use super::{processing_evidence_kind, sanitized_event};
use crate::driver::errors::ProtocolFailure;
use crate::driver::model::EffectiveSettings;
use crate::driver::params::ProtocolConfig;
use serde_json::{Value, json};
use std::sync::mpsc::TryRecvError;

#[derive(Clone, Debug)]
pub struct ProtocolOutcome {
    pub output: String,
    pub session_id: String,
    pub turn_id: String,
    pub turn_status: String,
    pub effective: EffectiveSettings,
}

pub enum ProtocolEffect {
    Send(Value),
    Interact(PendingInteraction),
    Complete(Box<ProtocolOutcome>),
    Fail(ProtocolFailure),
}

/// Parser-owned parked Pi dialog. It retains the exact native request until
/// the single structured client response is available for conversion back to
/// one matching `extension_ui_response` frame.
pub struct PendingInteraction {
    request: Value,
    parked: licoup_foundation::platform::native_agent_interaction::ParkedInteraction,
}

impl PendingInteraction {
    /// The parked dialog's callback token, as the interaction registry issued
    /// it. The tests that drive a dialog from either side read it here.
    pub fn callback_token(&self) -> &str {
        &self.parked.token
    }

    /// The exact native request the dialog parked, retained for conversion back
    /// to one matching `extension_ui_response` frame.
    pub fn exact_request(&self) -> &Value {
        &self.request
    }

    /// Take the single structured client response, waiting for it. The
    /// production loop polls with [`Self::try_response`] so a parked dialog
    /// never blocks the turn it belongs to.
    pub fn response(self, protocol: &PiProtocol) -> Result<Value, ProtocolFailure> {
        let response = self.parked.response_rx.recv().map_err(|_| {
            protocol.failure_with_ids(
                "pi_interaction_transport_closed",
                "Pi Agent interaction transport closed before the response.",
                "extension-ui/response",
            )
        })?;
        self.encode_response(response, protocol)
    }

    pub fn try_response(&self, protocol: &PiProtocol) -> Result<Option<Value>, ProtocolFailure> {
        match self.parked.response_rx.try_recv() {
            Ok(response) => self.encode_response(response, protocol).map(Some),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(protocol.failure_with_ids(
                "pi_interaction_transport_closed",
                "Pi Agent interaction transport closed before the response.",
                "extension-ui/response",
            )),
        }
    }

    fn encode_response(
        &self,
        response: Value,
        protocol: &PiProtocol,
    ) -> Result<Value, ProtocolFailure> {
        let request_id = self
            .request
            .get("id")
            .or_else(|| self.request.get("requestId"))
            .cloned()
            .ok_or_else(|| {
                protocol.failure_with_ids(
                    "pi_extension_ui_request_invalid",
                    "Pi Agent returned an interaction request without an identity.",
                    "extension-ui/request",
                )
            })?;
        let method = self
            .request
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                protocol.failure_with_ids(
                    "pi_extension_ui_method_missing",
                    "Pi Agent returned an interaction request without a method.",
                    "extension-ui/request",
                )
            })?;
        let mut native = json!({
            "type": "extension_ui_response",
            "id": request_id,
        });
        match method {
            "select" => native["value"] = response["selected"].clone(),
            "confirm" => {
                native["confirmed"] = response
                    .get("confirmed")
                    .or_else(|| response.get("allow"))
                    .cloned()
                    .unwrap_or(Value::Bool(false));
            }
            "input" | "editor" => native["value"] = response["text"].clone(),
            _ => {
                return Err(protocol.failure_with_ids(
                    "pi_extension_ui_method_unsupported",
                    "Pi Agent requested an unsupported interaction method.",
                    "extension-ui/request",
                ));
            }
        }
        Ok(native)
    }
}

impl Drop for PendingInteraction {
    fn drop(&mut self) {
        licoup_foundation::platform::native_agent_interaction::abandon(&self.parked.token);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolPhase {
    AwaitSwitch,
    AwaitInitialState,
    AwaitAvailableModels,
    AwaitModel,
    AwaitThinking,
    AwaitPromptAccept,
    AwaitSettled,
    AwaitAssistantText,
    AwaitState,
    Finished,
}

#[derive(Debug)]
pub struct PiProtocol {
    pub config: ProtocolConfig,
    pub phase: ProtocolPhase,
    pub session_id: Option<String>,
    pub output: String,
    pub events: Vec<Value>,
    pub effective: EffectiveSettings,
    pub pending_request: Option<&'static str>,
    /// Last provider/turn error observed during the active prompt (not echoed
    /// raw to callers; used only to classify a typed failure when text is empty).
    turn_error: Option<String>,
}

impl PiProtocol {
    pub fn new(config: ProtocolConfig) -> Self {
        let effective = EffectiveSettings {
            cwd: Some(config.cwd.clone()),
            model: config.model.clone(),
            reasoning_effort: config.thinking_level.clone(),
            ..EffectiveSettings::default()
        };
        let phase = if config.is_resume() {
            ProtocolPhase::AwaitSwitch
        } else {
            ProtocolPhase::AwaitInitialState
        };
        Self {
            config,
            phase,
            session_id: None,
            output: String::new(),
            events: Vec::new(),
            effective,
            pending_request: None,
            turn_error: None,
        }
    }

    pub fn initial_request(&mut self) -> Value {
        if let Some(path) = self.config.resume_session_path.clone() {
            self.pending_request = Some("switch_session");
            return json!({
                "id": "lico-pi-switch",
                "type": "switch_session",
                "sessionPath": path.to_string_lossy()
            });
        }
        self.state_request("lico-pi-initial-state", ProtocolPhase::AwaitInitialState)
    }

    fn prompt_request(&mut self) -> Value {
        self.pending_request = Some("prompt");
        self.phase = ProtocolPhase::AwaitPromptAccept;
        json!({
            "id": "lico-pi-prompt",
            "type": "prompt",
            "message": self.config.prompt
        })
    }

    fn thinking_request(&mut self) -> Option<Value> {
        let level = self.config.thinking_level.clone()?;
        self.pending_request = Some("set_thinking_level");
        self.phase = ProtocolPhase::AwaitThinking;
        Some(json!({
            "id": "lico-pi-thinking",
            "type": "set_thinking_level",
            "level": level
        }))
    }

    fn state_request(&mut self, id: &'static str, phase: ProtocolPhase) -> Value {
        self.pending_request = Some("get_state");
        self.phase = phase;
        json!({
            "id": id,
            "type": "get_state"
        })
    }

    fn model_request(&mut self) -> Option<Value> {
        let provider = self.config.model_provider.clone()?;
        let model_id = self.config.model_id.clone()?;
        self.pending_request = Some("set_model");
        self.phase = ProtocolPhase::AwaitModel;
        Some(json!({
            "id": "lico-pi-model",
            "type": "set_model",
            "provider": provider,
            "modelId": model_id
        }))
    }

    fn available_models_request(&mut self) -> Value {
        self.pending_request = Some("get_available_models");
        self.phase = ProtocolPhase::AwaitAvailableModels;
        json!({
            "id": "lico-pi-available-models",
            "type": "get_available_models"
        })
    }

    fn next_configuration_request(&mut self) -> Value {
        if let Some(request) = self.model_request() {
            return request;
        }
        if self.config.model_id.is_some() {
            return self.available_models_request();
        }
        if let Some(request) = self.thinking_request() {
            return request;
        }
        self.prompt_request()
    }

    fn capture_state(&mut self, message: &Value) {
        if let Some(session_id) = message.pointer("/data/sessionId").and_then(Value::as_str) {
            self.session_id = Some(session_id.to_string());
        }
        if let Some(level) = message
            .pointer("/data/thinkingLevel")
            .and_then(Value::as_str)
        {
            self.effective.reasoning_effort = Some(level.to_string());
        }
        if let Some(model) = message.pointer("/data/model") {
            let provider = model.get("provider").and_then(Value::as_str);
            let model_id = model
                .get("id")
                .or_else(|| model.get("modelId"))
                .and_then(Value::as_str);
            if let (Some(provider), Some(model_id)) = (provider, model_id) {
                self.effective.model = Some(format!("{provider}/{model_id}"));
            }
        }
    }

    pub fn failure_with_ids(
        &self,
        code: &'static str,
        message: &'static str,
        stage: &'static str,
    ) -> ProtocolFailure {
        ProtocolFailure::new(code, message, stage)
            .with_session(
                self.session_id
                    .as_deref()
                    .or(Some(self.config.requested_session_id.as_str())),
            )
            .with_turn(&self.config.turn_id)
    }

    pub fn active_turn_binding(&self) -> Option<(&str, &str)> {
        if self.phase != ProtocolPhase::AwaitSettled {
            return None;
        }
        Some((self.session_id.as_deref()?, &self.config.turn_id))
    }

    pub fn handle_message(&mut self, message: Value) -> Vec<ProtocolEffect> {
        let message_type = message.get("type").and_then(Value::as_str).unwrap_or("");
        if message_type == "extension_ui_request" {
            let Some(method) = message
                .get("method")
                .and_then(Value::as_str)
                .filter(|method| !method.trim().is_empty())
            else {
                self.phase = ProtocolPhase::Finished;
                return vec![ProtocolEffect::Fail(self.failure_with_ids(
                    "pi_extension_ui_method_missing",
                    "Pi Agent returned an interaction request without a method.",
                    "extension-ui/request",
                ))];
            };
            return match method {
                "select" | "confirm" | "input" | "editor" => match self.park_interaction(message) {
                    Ok(pending) => vec![ProtocolEffect::Interact(pending)],
                    Err(failure) => {
                        self.phase = ProtocolPhase::Finished;
                        vec![ProtocolEffect::Fail(failure)]
                    }
                },
                "notify" | "setStatus" | "setWidget" | "setTitle" | "set_editor_text" => {
                    self.events.push(json!({
                        "type": "extension_ui_event",
                        "method": method,
                    }));
                    Vec::new()
                }
                _ => {
                    self.phase = ProtocolPhase::Finished;
                    vec![ProtocolEffect::Fail(self.failure_with_ids(
                        "pi_extension_ui_method_unsupported",
                        "Pi Agent requested an unsupported interaction method.",
                        "extension-ui/request",
                    ))]
                }
            };
        }
        if message_type == "response" {
            return self.handle_response(&message);
        }
        self.handle_event(&message)
    }

    fn park_interaction(&self, request: Value) -> Result<PendingInteraction, ProtocolFailure> {
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                self.failure_with_ids(
                    "pi_extension_ui_method_missing",
                    "Pi Agent returned an interaction request without a method.",
                    "extension-ui/request",
                )
            })?;
        let request_id = request
            .get("id")
            .or_else(|| request.get("requestId"))
            .cloned()
            .ok_or_else(|| {
                self.failure_with_ids(
                    "pi_extension_ui_request_invalid",
                    "Pi Agent returned an interaction request without an identity.",
                    "extension-ui/request",
                )
            })?;
        let response_shape = match method {
            "select" => {
                licoup_foundation::platform::native_agent_interaction::ResponseShape::Select
            }
            "confirm" => {
                licoup_foundation::platform::native_agent_interaction::ResponseShape::Confirm
            }
            "input" => licoup_foundation::platform::native_agent_interaction::ResponseShape::Input,
            "editor" => {
                licoup_foundation::platform::native_agent_interaction::ResponseShape::Editor
            }
            _ => {
                return Err(self.failure_with_ids(
                    "pi_extension_ui_method_unsupported",
                    "Pi Agent requested an unsupported interaction method.",
                    "extension-ui/request",
                ));
            }
        };
        let session_id = self.session_id.as_deref().ok_or_else(|| {
            self.failure_with_ids(
                "pi_session_id_missing",
                "Pi Agent did not bind the interaction to a session.",
                "extension-ui/request",
            )
        })?;
        let summary = request
            .get("message")
            .or_else(|| request.get("title"))
            .and_then(Value::as_str)
            .unwrap_or("Pi Agent requests input.")
            .chars()
            .take(256)
            .collect::<String>();
        let options = request
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|option| {
                option
                    .as_str()
                    .or_else(|| option.get("label").and_then(Value::as_str))
            })
            .take(32)
            .map(|option| option.chars().take(128).collect())
            .collect::<Vec<String>>();
        let parked = licoup_foundation::platform::native_agent_interaction::park(
            licoup_foundation::platform::native_agent_interaction::InteractionRequest {
                adapter_id: "pi".to_string(),
                session_id: session_id.to_string(),
                turn_id: self.config.turn_id.clone(),
                request_id,
                method: method.to_string(),
                summary: summary.clone(),
                options: options.clone(),
                response_shape,
            },
        )
        .map_err(|_| {
            self.failure_with_ids(
                "pi_interaction_park_unavailable",
                "Pi Agent interaction state is unavailable.",
                "extension-ui/request",
            )
        })?;
        crate::port::turn_event::emit_turn_event(
            "agent.interaction.needed",
            session_id,
            &self.config.turn_id,
            json!({
                "agentId": "pi",
                "adapterCallbackTokenRef": parked.token,
                "requestMethod": method,
                "displaySummary": summary,
                "options": options,
                "adapterStyle": "callback",
            }),
        );
        Ok(PendingInteraction { request, parked })
    }

    fn handle_response(&mut self, message: &Value) -> Vec<ProtocolEffect> {
        let command = message
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let success = message
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        match self.phase {
            ProtocolPhase::AwaitSwitch if command == "switch_session" => {
                if !success {
                    self.phase = ProtocolPhase::Finished;
                    return vec![ProtocolEffect::Fail(self.failure_with_ids(
                        "pi_session_switch_failed",
                        "Pi Agent could not switch to the requested session.",
                        "session/switch",
                    ))];
                }
                if let Some(cancelled) = message.pointer("/data/cancelled").and_then(Value::as_bool)
                    && cancelled
                {
                    self.phase = ProtocolPhase::Finished;
                    return vec![ProtocolEffect::Fail(self.failure_with_ids(
                        "pi_session_switch_cancelled",
                        "Pi Agent cancelled the session switch.",
                        "session/switch",
                    ))];
                }
                vec![ProtocolEffect::Send(self.state_request(
                    "lico-pi-switched-state",
                    ProtocolPhase::AwaitInitialState,
                ))]
            }
            ProtocolPhase::AwaitInitialState if command == "get_state" => {
                if !success {
                    self.phase = ProtocolPhase::Finished;
                    return vec![ProtocolEffect::Fail(self.failure_with_ids(
                        "pi_session_state_failed",
                        "Pi Agent did not expose the active session state.",
                        "session/state",
                    ))];
                }
                self.capture_state(message);
                let Some(active_session_id) = self.session_id.as_deref() else {
                    self.phase = ProtocolPhase::Finished;
                    return vec![ProtocolEffect::Fail(self.failure_with_ids(
                        "pi_session_id_missing",
                        "Pi Agent did not return a session identifier.",
                        "session/state",
                    ))];
                };
                if !self.config.requested_session_id.is_empty()
                    && active_session_id != self.config.requested_session_id
                {
                    self.phase = ProtocolPhase::Finished;
                    return vec![ProtocolEffect::Fail(self.failure_with_ids(
                        "pi_session_identity_mismatch",
                        "Pi Agent switched to a different session than requested.",
                        "session/switch",
                    ))];
                }
                if self.config.prompt.trim().is_empty() {
                    let session_id = active_session_id.to_owned();
                    self.phase = ProtocolPhase::Finished;
                    return vec![ProtocolEffect::Complete(Box::new(ProtocolOutcome {
                        output: String::new(),
                        session_id,
                        turn_id: self.config.turn_id.clone(),
                        turn_status: if self.config.is_resume() {
                            "resumed".to_owned()
                        } else {
                            "started".to_owned()
                        },
                        effective: self.effective.clone(),
                    }))];
                }
                vec![ProtocolEffect::Send(self.next_configuration_request())]
            }
            ProtocolPhase::AwaitAvailableModels if command == "get_available_models" => {
                if !success {
                    self.phase = ProtocolPhase::Finished;
                    return vec![ProtocolEffect::Fail(self.failure_with_ids(
                        "pi_model_override_failed",
                        "Pi Agent could not resolve the requested model.",
                        "capability/model",
                    ))];
                }
                let requested_model = self.config.model_id.as_deref().unwrap_or_default();
                let mut matches = message
                    .pointer("/data/models")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|model| {
                        let provider = model.get("provider").and_then(Value::as_str)?.trim();
                        let model_id = model
                            .get("id")
                            .or_else(|| model.get("modelId"))
                            .and_then(Value::as_str)?
                            .trim();
                        (model_id == requested_model
                            && !provider.is_empty()
                            && !model_id.is_empty())
                        .then(|| (provider.to_string(), model_id.to_string()))
                    })
                    .collect::<Vec<_>>();
                matches.sort();
                matches.dedup();
                if matches.len() != 1 {
                    self.phase = ProtocolPhase::Finished;
                    let (code, message) = if matches.is_empty() {
                        (
                            "pi_model_override_failed",
                            "Pi Agent could not resolve the requested model.",
                        )
                    } else {
                        (
                            "pi_model_provider_required",
                            "Pi Agent found the requested model under more than one provider.",
                        )
                    };
                    return vec![ProtocolEffect::Fail(self.failure_with_ids(
                        code,
                        message,
                        "capability/model",
                    ))];
                }
                let Some((provider, model_id)) = matches.pop() else {
                    self.phase = ProtocolPhase::Finished;
                    return vec![ProtocolEffect::Fail(self.failure_with_ids(
                        "pi_model_override_failed",
                        "Pi Agent could not resolve the requested model.",
                        "capability/model",
                    ))];
                };
                self.config.model_provider = Some(provider);
                self.config.model_id = Some(model_id);
                let Some(request) = self.model_request() else {
                    self.phase = ProtocolPhase::Finished;
                    return vec![ProtocolEffect::Fail(self.failure_with_ids(
                        "pi_model_override_failed",
                        "Pi Agent could not resolve the requested model.",
                        "capability/model",
                    ))];
                };
                vec![ProtocolEffect::Send(request)]
            }
            ProtocolPhase::AwaitModel if command == "set_model" => {
                if !success {
                    self.phase = ProtocolPhase::Finished;
                    return vec![ProtocolEffect::Fail(self.failure_with_ids(
                        "pi_model_override_failed",
                        "Pi Agent could not apply the requested model.",
                        "capability/model",
                    ))];
                }
                if let Some(model) = message.get("data") {
                    let provider = model.get("provider").and_then(Value::as_str);
                    let model_id = model
                        .get("id")
                        .or_else(|| model.get("modelId"))
                        .and_then(Value::as_str);
                    if let (Some(provider), Some(model_id)) = (provider, model_id) {
                        self.effective.model = Some(format!("{provider}/{model_id}"));
                    }
                }
                if let Some(request) = self.thinking_request() {
                    return vec![ProtocolEffect::Send(request)];
                }
                vec![ProtocolEffect::Send(self.prompt_request())]
            }
            ProtocolPhase::AwaitThinking if command == "set_thinking_level" => {
                if !success {
                    self.phase = ProtocolPhase::Finished;
                    return vec![ProtocolEffect::Fail(self.failure_with_ids(
                        "pi_thinking_override_failed",
                        "Pi Agent could not apply the requested thinking level.",
                        "capability/thinking",
                    ))];
                }
                vec![ProtocolEffect::Send(self.prompt_request())]
            }
            ProtocolPhase::AwaitPromptAccept if command == "prompt" => {
                if !success {
                    self.phase = ProtocolPhase::Finished;
                    return vec![ProtocolEffect::Fail(self.failure_with_ids(
                        "pi_prompt_rejected",
                        "Pi Agent rejected the prompt before acceptance.",
                        "prompt",
                    ))];
                }
                self.phase = ProtocolPhase::AwaitSettled;
                self.pending_request = None;
                Vec::new()
            }
            ProtocolPhase::AwaitAssistantText if command == "get_last_assistant_text" => {
                if success {
                    // Pi returns {"text": null} when the last assistant turn has
                    // no text (for example a provider error). Never wipe streamed
                    // deltas with an empty/null snapshot.
                    if let Some(text) = message.pointer("/data/text").and_then(Value::as_str) {
                        if !text.trim().is_empty() {
                            self.output = text.to_string();
                        }
                    }
                }
                self.pending_request = Some("get_state");
                self.phase = ProtocolPhase::AwaitState;
                vec![ProtocolEffect::Send(json!({
                    "id": "lico-pi-state",
                    "type": "get_state"
                }))]
            }
            ProtocolPhase::AwaitState if command == "get_state" => {
                if success {
                    self.capture_state(message);
                }
                if self.output.trim().is_empty() {
                    self.phase = ProtocolPhase::Finished;
                    return vec![ProtocolEffect::Fail(self.empty_final_failure())];
                }
                let session_id = self
                    .session_id
                    .clone()
                    .filter(|value| !value.is_empty())
                    .or_else(|| {
                        (!self.config.requested_session_id.is_empty())
                            .then(|| self.config.requested_session_id.clone())
                    })
                    .unwrap_or_default();
                if session_id.is_empty() {
                    self.phase = ProtocolPhase::Finished;
                    return vec![ProtocolEffect::Fail(self.failure_with_ids(
                        "pi_session_id_missing",
                        "Pi Agent did not return a session identifier.",
                        "session/state",
                    ))];
                }
                self.session_id = Some(session_id.clone());
                crate::port::turn_event::emit_agent_message_completed(
                    &session_id,
                    &self.config.turn_id,
                    &self.output,
                );
                self.phase = ProtocolPhase::Finished;
                vec![ProtocolEffect::Complete(Box::new(ProtocolOutcome {
                    output: self.output.clone(),
                    session_id,
                    turn_id: self.config.turn_id.clone(),
                    turn_status: "end_turn".to_string(),
                    effective: self.effective.clone(),
                }))]
            }
            _ => Vec::new(),
        }
    }

    fn handle_event(&mut self, message: &Value) -> Vec<ProtocolEffect> {
        let message_type = message.get("type").and_then(Value::as_str).unwrap_or("");
        if self.phase == ProtocolPhase::AwaitSettled {
            if let Some(evidence_kind) = processing_evidence_kind(message)
                && let Some(session_id) = self.session_id.as_deref()
            {
                crate::port::turn_event::emit_agent_processing(
                    session_id,
                    &self.config.turn_id,
                    evidence_kind,
                    None,
                );
            }
            self.events.extend(
                licoup_agent_adapter_sdk::skill_invocation_projection::project_skill_invocations(
                    message,
                ),
            );
            if let Some(event) = sanitized_event(message) {
                self.events.push(event);
            }
            self.capture_turn_error(message);
            self.capture_assistant_text_event(message);
            if message_type == "agent_settled" {
                self.pending_request = Some("get_last_assistant_text");
                self.phase = ProtocolPhase::AwaitAssistantText;
                return vec![ProtocolEffect::Send(json!({
                    "id": "lico-pi-assistant",
                    "type": "get_last_assistant_text"
                }))];
            }
        }
        Vec::new()
    }

    fn capture_assistant_text_event(&mut self, message: &Value) {
        let Some(event) = message.get("assistantMessageEvent") else {
            return;
        };
        let event_type = event.get("type").and_then(Value::as_str).unwrap_or("");
        match event_type {
            "text_delta" => {
                if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                    self.output.push_str(delta);
                    if let Some(session_id) = self.session_id.as_deref() {
                        crate::port::turn_event::emit_agent_message_chunk(
                            session_id,
                            &self.config.turn_id,
                            delta,
                        );
                    }
                }
            }
            "text_end" => {
                // Authoritative block text when deltas were missed.
                if self.output.trim().is_empty() {
                    if let Some(content) = event.get("content").and_then(Value::as_str) {
                        if !content.trim().is_empty() {
                            self.output = content.to_owned();
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn capture_turn_error(&mut self, message: &Value) {
        let message_type = message.get("type").and_then(Value::as_str).unwrap_or("");
        match message_type {
            "message_end" => {
                let Some(assistant) = message.get("message") else {
                    return;
                };
                if assistant.get("role").and_then(Value::as_str) != Some("assistant") {
                    return;
                }
                let stop = assistant
                    .get("stopReason")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if let Some(error) = assistant
                    .get("errorMessage")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    self.turn_error = Some(error.to_owned());
                } else if stop == "error" {
                    self.turn_error =
                        Some("assistant stopReason=error without errorMessage".to_owned());
                }
            }
            "auto_retry_end" => {
                if message.get("success").and_then(Value::as_bool) == Some(false) {
                    if let Some(error) = message
                        .get("finalError")
                        .or_else(|| message.get("errorMessage"))
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        self.turn_error = Some(error.to_owned());
                    }
                }
            }
            "extension_error" => {
                if let Some(error) = message
                    .get("errorMessage")
                    .or_else(|| message.get("message"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    self.turn_error = Some(error.to_owned());
                }
            }
            _ => {}
        }
    }

    fn empty_final_failure(&self) -> ProtocolFailure {
        let detail = self.turn_error.as_deref().unwrap_or("");
        if detail.contains("gateway_credential_unavailable") {
            return self.failure_with_ids(
                "pi_gateway_credentials_unavailable",
                "Pi Agent could not call the model because the LLM Gateway has no authorized API keys. Open LicoUp → Keys and authorize credentials so they are hot-applied to the running Gateway.",
                "prompt/complete",
            );
        }
        if !detail.is_empty() {
            return self.failure_with_ids(
                "pi_provider_turn_failed",
                "Pi Agent finished the turn without an assistant reply after a model or provider error.",
                "prompt/complete",
            );
        }
        self.failure_with_ids(
            "pi_final_message_missing",
            "Pi Agent completed without a final assistant message.",
            "prompt/complete",
        )
    }
}
