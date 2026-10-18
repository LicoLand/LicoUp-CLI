//! The adapter-facing ingress: this Agent's parser behind the SDK's byte-line
//! contract.
//!
//! [`ClaudeCodeParser`] is the type composition names. It holds the protocol
//! state machine and implements [`NativeLineParser`] over the CLI's
//! `lf-ndjson` framing, so the SDK's registry, host queries and replay harness
//! reach one parser rather than a second implementation of the same dialect.

use licoup_agent_adapter_sdk::adapters::NativeLineParser;
use serde_json::Value;

use super::state::ClaudeCodeStateMachine;
use crate::protocol::ProtocolFailure;
use crate::protocol::launch::LaunchIdentity;
use crate::protocol::params::DriverConfig;

use super::{ClaudeEffect, permission_request_details};

/// The Claude Code adapter parser: this Agent's dialect behind the shared
/// byte-line ingress.
pub struct ClaudeCodeParser<'a> {
    state: ClaudeCodeStateMachine<'a>,
}

impl<'a> ClaudeCodeParser<'a> {
    /// The parser for one turn, bound to the identity the launch already knew.
    pub fn new(
        config: &'a DriverConfig,
        identity: &LaunchIdentity,
        known_session: Option<String>,
    ) -> Self {
        Self {
            state: ClaudeCodeStateMachine::new(config, identity, known_session),
        }
    }

    /// The protocol state machine this parser drives.
    ///
    /// The process half reads it for the failures it reports and the identity
    /// the turn bound; the parser owns the classification, so both halves see
    /// one state rather than two.
    pub fn state(&self) -> &ClaudeCodeStateMachine<'a> {
        &self.state
    }

    /// The protocol state machine this parser drives, mutably.
    pub fn state_mut(&mut self) -> &mut ClaudeCodeStateMachine<'a> {
        &mut self.state
    }

    /// Consume one already-decoded frame, as the replay arm and the process
    /// half both do.
    pub fn handle(
        &mut self,
        message: Value,
    ) -> Result<Option<super::ProtocolFinishReport>, ProtocolFailure> {
        self.state.handle(message)
    }

    /// This Agent's failure, carrying the identity the parser has bound.
    pub fn failure(
        &self,
        code: &'static str,
        message: &'static str,
        stage: &'static str,
    ) -> ProtocolFailure {
        self.state.failure(code, message, stage)
    }

    /// Record that the client wrote a user-initiated interrupt for this turn.
    pub fn mark_cancel_requested(&mut self) {
        self.state.mark_cancel_requested();
    }

    /// Whether the client wrote a user-initiated interrupt for this turn.
    pub fn cancel_was_requested(&self) -> bool {
        self.state.cancel_was_requested()
    }

    /// The native conversation identity this turn bound, once a frame reported
    /// one.
    pub fn observed_session_id(&self) -> Option<&str> {
        self.state.observed_session_id.as_deref()
    }
}

/// Sole ingress for one Claude Code stream-json wire line.
impl NativeLineParser for ClaudeCodeParser<'_> {
    type Report = Option<ClaudeEffect>;
    type Error = ProtocolFailure;

    fn parse_line(&mut self, line: &[u8]) -> Result<Self::Report, Self::Error> {
        let trimmed = line
            .iter()
            .copied()
            .skip_while(|byte| byte.is_ascii_whitespace())
            .collect::<Vec<_>>();
        if trimmed.iter().all(|byte| byte.is_ascii_whitespace()) {
            return Ok(None);
        }
        let message: Value = serde_json::from_slice(&trimmed).map_err(|_| {
            self.state.failure(
                "claude_code_invalid_json",
                "Claude Code returned an invalid stream event.",
                "protocol/read",
            )
        })?;
        if message.get("type").and_then(Value::as_str) == Some("control_request") {
            if let Some(session_id) = message
                .get("session_id")
                .or_else(|| message.get("sessionId"))
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                self.state.record_session(session_id)?;
            }
            if let Some(permission) = permission_request_details(&message) {
                return Ok(Some(ClaudeEffect::Permission(permission)));
            }
            let request_method = message
                .pointer("/request/subtype")
                .or_else(|| message.pointer("/request/type"))
                .and_then(Value::as_str)
                .unwrap_or("control_request")
                .chars()
                .take(64)
                .collect::<String>();
            licoup_foundation::platform::turn_event_emit::emit_turn_event(
                "agent.interaction.unsupported",
                self.state
                    .observed_session_id
                    .as_deref()
                    .or(self.state.expected_session_id.as_deref())
                    .unwrap_or_default(),
                &self.state.config.turn_id,
                serde_json::json!({"requestMethod": request_method}),
            );
            let response = message
                .get("request_id")
                .and_then(Value::as_str)
                .map(super::denied_control_response);
            return Ok(Some(ClaudeEffect::Control { response }));
        }
        Ok(Some(match self.state.handle(message)? {
            Some(report) => ClaudeEffect::ProtocolFinished(report),
            None => ClaudeEffect::Progress {
                session_id: self.state.observed_session_id.clone(),
            },
        }))
    }
}
