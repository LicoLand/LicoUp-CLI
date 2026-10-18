mod control;
mod events;
mod helpers;
pub mod session;

use self::helpers::request_id_matches;
use crate::app_server::config::ProtocolConfig;
use crate::app_server::limits::{
    ACCOUNT_RATE_LIMITS_REQUEST_ID, INITIALIZE_REQUEST_ID, THREAD_REQUEST_ID,
    THREAD_UNARCHIVE_REQUEST_ID, TURN_REQUEST_ID,
};
use crate::app_server::model::{EffectiveSettings, ProtocolEffect, ProtocolFailure, ProtocolPhase};
use licoup_agent_adapter_sdk::adapters::{AdapterContract, NativeLineParser};
use licoup_agent_adapter_sdk::{LifecycleStage, Transition, TransitionReducer};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::io;

pub(super) const CONTRACT: AdapterContract = AdapterContract::new("codex", "stdio-jsonrpc");

pub fn completed_transitions(output: &str) -> Vec<Transition> {
    terminal_transitions("codex:reply", output)
}

pub fn failure_transitions(code: &str, stage: &str, message: &str) -> Vec<Transition> {
    let mut reducer = TransitionReducer::default();
    let mut transitions = reducer.advance(LifecycleStage::Submitted);
    if let Some(failure) = reducer.fail(code, stage, message) {
        transitions.push(failure);
    }
    transitions
}

fn terminal_transitions(unit_id: &str, output: &str) -> Vec<Transition> {
    let mut reducer = TransitionReducer::default();
    let mut transitions = reducer.advance(LifecycleStage::Responding);
    if !output.is_empty() {
        transitions.push(Transition::Text {
            unit_id: unit_id.to_owned(),
            text: output.to_owned(),
        });
    }
    transitions.extend(reducer.advance(LifecycleStage::Completed));
    transitions
}

#[derive(Debug)]
pub struct CodexParser {
    config: ProtocolConfig,
    phase: ProtocolPhase,
    session_id: Option<String>,
    thread_id: Option<String>,
    turn_id: Option<String>,
    effective: EffectiveSettings,
    completed_items: Vec<Value>,
    observed_processing_items: HashSet<String>,
    unidentified_processing_items: usize,
    unarchive_attempted: bool,
    rate_limits_fallback_attempted: bool,
    turn_model_override: Option<String>,
}

pub enum CodexEffect {
    Protocol(ProtocolEffect),
    SteerResponse { request_id: String, accepted: bool },
}

/// Decode a model-catalog response at the same parser-owned wire boundary.
pub fn parse_response_line(line: &[u8], expected_id: i64) -> Result<Option<Value>, ()> {
    let message: Value = serde_json::from_slice(line).map_err(|_| ())?;
    if message.get("id").and_then(Value::as_i64) != Some(expected_id) {
        return Ok(None);
    }
    message.get("result").cloned().map(Some).ok_or(())
}

pub fn encode_message(message: &Value) -> io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(message).map_err(io::Error::other)?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub fn steer_request(thread_id: &str, turn_id: &str, text: &str) -> (String, Value) {
    let request_id = format!("lico-steer-{}", uuid::Uuid::new_v4().simple());
    let message = json!({
        "id": request_id,
        "method": "turn/steer",
        "params": {
            "threadId": thread_id,
            "expectedTurnId": turn_id,
            "input": [{"type": "text", "text": text}]
        }
    });
    (request_id, message)
}

pub fn interrupt_request(thread_id: &str, turn_id: &str) -> (String, Value) {
    let request_id = format!("lico-interrupt-{}", uuid::Uuid::new_v4().simple());
    let message = json!({
        "id": request_id,
        "method": "turn/interrupt",
        "params": {
            "threadId": thread_id,
            "turnId": turn_id
        }
    });
    (request_id, message)
}

impl CodexParser {
    pub fn new(config: ProtocolConfig) -> Self {
        Self {
            config,
            phase: ProtocolPhase::AwaitInitialize,
            session_id: None,
            thread_id: None,
            turn_id: None,
            effective: EffectiveSettings::default(),
            completed_items: Vec::new(),
            observed_processing_items: HashSet::new(),
            unidentified_processing_items: 0,
            unarchive_attempted: false,
            rate_limits_fallback_attempted: false,
            turn_model_override: None,
        }
    }

    pub fn initial_request(&self) -> Value {
        json!({
            "id": INITIALIZE_REQUEST_ID,
            "method": "initialize",
            "params": {
                "clientInfo": {"name": "lico-up", "title": "LicoUp", "version": env!("CARGO_PKG_VERSION")},
                "capabilities": {"experimentalApi": self.config.session_path.is_some()}
            }
        })
    }

    pub fn handle_message(&mut self, message: Value) -> Vec<ProtocolEffect> {
        if let Some(effects) = self.reject_server_request(&message) {
            return effects;
        }
        if message.get("method").is_some() {
            return self.handle_notification(&message);
        }
        match self.phase {
            ProtocolPhase::AwaitInitialize
                if request_id_matches(&message, INITIALIZE_REQUEST_ID) =>
            {
                self.handle_initialize_response(&message)
            }
            ProtocolPhase::AwaitRateLimits
                if request_id_matches(&message, ACCOUNT_RATE_LIMITS_REQUEST_ID) =>
            {
                self.handle_rate_limits_response(&message)
            }
            ProtocolPhase::AwaitThread if request_id_matches(&message, THREAD_REQUEST_ID) => {
                self.handle_thread_response(&message)
            }
            ProtocolPhase::AwaitThreadUnarchive
                if request_id_matches(&message, THREAD_UNARCHIVE_REQUEST_ID) =>
            {
                self.handle_thread_unarchive_response(&message)
            }
            ProtocolPhase::AwaitTurnStart if request_id_matches(&message, TURN_REQUEST_ID) => {
                self.handle_turn_start_response(&message)
            }
            _ => Vec::new(),
        }
    }

    pub fn contextualize(&self, mut failure: ProtocolFailure) -> ProtocolFailure {
        if failure.session_id.is_none() {
            failure.session_id = self.session_id.clone();
        }
        if failure.thread_id.is_none() {
            failure.thread_id = self.thread_id.clone();
        }
        if failure.turn_id.is_none() {
            failure.turn_id = self.turn_id.clone();
        }
        failure
    }

    pub fn active_turn_binding(&self) -> Option<(&str, &str)> {
        (self.phase == ProtocolPhase::AwaitTurnCompleted)
            .then_some((self.thread_id.as_deref()?, self.turn_id.as_deref()?))
    }

    pub fn parse_line(&mut self, line: &[u8]) -> Result<Vec<CodexEffect>, ProtocolFailure> {
        NativeLineParser::parse_line(self, line)
    }
}

impl NativeLineParser for CodexParser {
    type Report = Vec<CodexEffect>;
    type Error = ProtocolFailure;

    /// Sole ingress for one app-server JSON-RPC wire line.
    fn parse_line(&mut self, line: &[u8]) -> Result<Self::Report, Self::Error> {
        let message: Value = serde_json::from_slice(line).map_err(|_| {
            self.contextualize(ProtocolFailure::new(
                "codex_app_server_invalid_json",
                "Codex app-server returned an invalid protocol message.",
                "protocol/read",
            ))
        })?;
        if let Some(request_id) = message
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| id.starts_with("lico-steer-") || id.starts_with("lico-interrupt-"))
        {
            return Ok(vec![CodexEffect::SteerResponse {
                request_id: request_id.to_owned(),
                accepted: message.get("error").is_none() && message.get("result").is_some(),
            }]);
        }
        Ok(self
            .handle_message(message)
            .into_iter()
            .map(CodexEffect::Protocol)
            .collect())
    }
}
