//! The Claude Code protocol state machine.
//!
//! One instance is one turn: it binds the native conversation identity the CLI
//! reports, accumulates visible text without inventing any, remembers the
//! redacted evidence a transcript may carry, and reports one terminal result.
//! It never settles a turn and never times one out.

use super::events::{
    partial_text_delta, processing_evidence_kind, processing_tool_name, transcript_event,
};
use crate::protocol::ProtocolFailure;
use crate::protocol::launch::LaunchIdentity;
use crate::protocol::params::DriverConfig;
use crate::protocol::settings::EffectiveSettings;
use licoup_foundation::platform::turn_event_emit;
use serde_json::{Value, json};

/// The terminal report one consumed `result` frame produces.
///
/// It carries the frame's own facts and the identity the parser bound earlier,
/// so a reader never re-derives either from the raw line.
#[derive(Debug)]
pub struct ProtocolFinishReport {
    /// The terminal assistant message.
    pub output: String,
    /// The native conversation identity.
    pub session_id: String,
    /// The turn the client dispatched.
    pub turn_id: String,
    /// The settings the CLI reported for this turn.
    pub effective: EffectiveSettings,
    /// The redacted transcript evidence retained for this turn.
    pub events: Vec<Value>,
}

/// One Claude Code turn's state machine.
pub struct ClaudeCodeStateMachine<'a> {
    pub(crate) config: &'a DriverConfig,
    /// The native identity the launch expected, when it had one.
    pub expected_session_id: Option<String>,
    /// The native identity the CLI actually reported.
    pub observed_session_id: Option<String>,
    effective: EffectiveSettings,
    started_emitted: bool,
    next_message_unit: u64,
    pending_native_message_id: String,
    active_message: Option<ClaudeMessageUnit>,
    visible_text: String,
    transcript_events: Vec<Value>,
    /// A user-initiated cancel interrupt was successfully written for the
    /// current turn. The CLI answers it with an is_error terminal result; the
    /// marker distinguishes that interrupt-shaped reply from a genuine turn
    /// failure.
    cancel_requested: bool,
}

struct ClaudeMessageUnit {
    unit: String,
    native_id: String,
    snapshot: String,
    visible: String,
    from_stream: bool,
    assistant_snapshot_seen: bool,
}

impl<'a> ClaudeCodeStateMachine<'a> {
    /// A state machine for one turn, bound to the identity the launch knew.
    pub fn new(
        config: &'a DriverConfig,
        identity: &LaunchIdentity,
        known_session: Option<String>,
    ) -> Self {
        Self {
            config,
            expected_session_id: known_session,
            observed_session_id: None,
            effective: identity.effective(),
            started_emitted: false,
            next_message_unit: 0,
            pending_native_message_id: String::new(),
            active_message: None,
            visible_text: String::new(),
            transcript_events: Vec::new(),
            cancel_requested: false,
        }
    }

    /// Consume one already-decoded frame.
    ///
    /// This is the parser's real body; the byte-line ingress in [`super`]
    /// decodes into it, so replay and production drive one implementation.
    pub fn handle(
        &mut self,
        message: Value,
    ) -> Result<Option<ProtocolFinishReport>, ProtocolFailure> {
        if let Some(session_id) = message
            .get("session_id")
            .or_else(|| message.get("sessionId"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            self.record_session(session_id)?;
        }
        if let Some(event) = transcript_event(&message) {
            self.transcript_events.push(event);
        }
        if message.get("type").and_then(Value::as_str) == Some("system")
            && message.get("subtype").and_then(Value::as_str) == Some("permission_denied")
        {
            turn_event_emit::emit_turn_event(
                "permission.denied",
                self.observed_session_id
                    .as_deref()
                    .or(self.expected_session_id.as_deref())
                    .unwrap_or_default(),
                &self.config.turn_id,
                json!({"text": "Claude Code reported a permission denial."}),
            );
        }
        if message.get("type").and_then(Value::as_str) == Some("system")
            && message.get("subtype").and_then(Value::as_str) == Some("init")
        {
            self.effective.cwd = message
                .get("cwd")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or(self.effective.cwd.take());
            self.effective.model = message
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or(self.effective.model.take());
            self.effective.permission_mode = message
                .get("permissionMode")
                .or_else(|| message.get("permission_mode"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .or(self.effective.permission_mode.take());
            self.effective.approval_policy =
                self.effective.permission_mode.clone().map(Value::String);
        }
        if message.pointer("/event/type").and_then(Value::as_str) == Some("message_start") {
            self.pending_native_message_id = message
                .pointer("/event/message/id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
        }
        if let Some(text) = partial_text_delta(&message) {
            let (message_unit, suffix) = self.observe_text_delta(text);
            turn_event_emit::emit_agent_message_chunk_for_unit(
                self.observed_session_id
                    .as_deref()
                    .or(self.expected_session_id.as_deref())
                    .unwrap_or_default(),
                &self.config.turn_id,
                &message_unit,
                &suffix,
            );
        }
        // Real Claude Code 2.x print streams deliver whole assistant messages
        // instead of content_block_delta events: each tool-call round arrives
        // as one assistant message whose content text blocks carry the reply.
        // Surface every text block as a chunk so the client renders progress
        // turn by turn instead of waiting for the final result.
        if message.get("type").and_then(Value::as_str) == Some("assistant")
            && let Some(snapshot) = assistant_text_snapshot(&message)
            && let Some((message_unit, suffix)) =
                self.observe_assistant_snapshot(&message, &snapshot)
        {
            turn_event_emit::emit_agent_message_chunk_for_unit(
                self.observed_session_id
                    .as_deref()
                    .or(self.expected_session_id.as_deref())
                    .unwrap_or_default(),
                &self.config.turn_id,
                &message_unit,
                &suffix,
            );
        }
        if let Some(evidence_kind) = processing_evidence_kind(&message) {
            turn_event_emit::emit_agent_processing(
                self.observed_session_id
                    .as_deref()
                    .or(self.expected_session_id.as_deref())
                    .unwrap_or_default(),
                &self.config.turn_id,
                evidence_kind,
                processing_tool_name(&message),
            );
        }
        if message.get("type").and_then(Value::as_str) != Some("result") {
            return Ok(None);
        }
        self.finish(message).map(Some)
    }

    /// Bind the native conversation identity one frame reported.
    ///
    /// An identity that is malformed, or that contradicts the conversation this
    /// turn was launched for, is a failure rather than a silent re-bind: the
    /// client must never read another conversation's reply.
    pub(crate) fn record_session(&mut self, value: &str) -> Result<(), ProtocolFailure> {
        if value.len() > 512 || value.chars().any(char::is_control) {
            return Err(self.failure(
                "claude_code_session_id_invalid",
                "Claude Code returned an invalid native conversation identifier.",
                "session/open",
            ));
        }
        if self
            .expected_session_id
            .as_deref()
            .is_some_and(|expected| expected != value)
            || self
                .observed_session_id
                .as_deref()
                .is_some_and(|observed| observed != value)
        {
            return Err(self.failure(
                "claude_code_session_mismatch",
                "Claude Code returned a different conversation than requested.",
                "session/resume",
            ));
        }
        self.observed_session_id = Some(value.to_string());
        if !self.started_emitted {
            turn_event_emit::emit_turn_event(
                "agent.turn.accepted",
                value,
                &self.config.turn_id,
                json!({"evidenceKind": "stream-init"}),
            );
            self.started_emitted = true;
        }
        Ok(())
    }

    fn finish(&mut self, terminal: Value) -> Result<ProtocolFinishReport, ProtocolFailure> {
        let session_id = self
            .observed_session_id
            .clone()
            .or_else(|| self.expected_session_id.clone())
            .ok_or_else(|| {
                self.failure(
                    "claude_code_session_id_missing",
                    "Claude Code did not return the native conversation identifier.",
                    "session/open",
                )
            })?;
        // Permission denials are rendered honestly, never failed: the CLI
        // already answered the denial and completed the turn. Each denied
        // tool is surfaced as a permission.denied event so the client can show
        // an approval card with a retry affordance.
        if let Some(denials) = terminal
            .get("permission_denials")
            .and_then(Value::as_array)
            .filter(|values| !values.is_empty())
        {
            for denial in denials {
                let tool_name = denial
                    .get("tool_name")
                    .and_then(Value::as_str)
                    .unwrap_or("tool");
                let tool_use_id = denial
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let session_id = self
                    .observed_session_id
                    .as_deref()
                    .or(self.expected_session_id.as_deref())
                    .unwrap_or_default();
                turn_event_emit::emit_turn_event(
                    "permission.denied",
                    session_id,
                    &self.config.turn_id,
                    json!({
                        "toolName": tool_name,
                        "toolUseId": tool_use_id,
                        "text": format!("{tool_name} was denied permission."),
                    }),
                );
            }
        }
        let deferred = terminal.get("deferred_tool_use").is_some()
            || terminal
                .get("terminal_reason")
                .or_else(|| terminal.get("stop_reason"))
                .and_then(Value::as_str)
                == Some("tool_deferred");
        if deferred {
            let mut failure = self.failure(
                "claude_code_user_interaction_required",
                "Claude Code requires user interaction before this turn can continue.",
                "permission/request",
            );
            failure.user_interaction_required = true;
            failure.request_method = Some("can_use_tool".to_string());
            failure.turn_status = Some("userInteractionRequired".to_string());
            return Err(failure.with_session(Some(&session_id)));
        }
        let subtype = terminal
            .get("subtype")
            .and_then(Value::as_str)
            .unwrap_or("failed");
        // The CLI reports tool errors as `error_during_execution` even when
        // the turn completed with a real reply. Only an explicit is_error
        // (or a missing flag on a plain failure) fails the turn; the honest
        // reply stays visible.
        let is_error = terminal
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(subtype == "failed");
        if is_error {
            if matches!(subtype, "authentication_required" | "authentication_failed") {
                return Err(self
                    .failure(
                        "claude_code_authentication_required",
                        "Claude Code authentication is required before this turn can continue.",
                        "authentication/runtime",
                    )
                    .with_session(Some(&session_id)));
            }
            if self.cancel_requested {
                // A user-initiated cancel is answered by the CLI with an
                // interrupt-shaped is_error result; it is a cancellation,
                // never a turn failure.
                let mut failure = self.failure(
                    "claude_code_turn_cancelled",
                    "Claude Code turn was cancelled by the user.",
                    "turn/cancelled",
                );
                failure.turn_status = Some("cancelled".to_string());
                return Err(failure.with_session(Some(&session_id)));
            }
            let mut failure = self.failure(
                "claude_code_turn_failed",
                "Claude Code reported that the requested turn failed.",
                "turn/completed",
            );
            failure.turn_status = Some(subtype.to_string());
            return Err(failure.with_session(Some(&session_id)));
        }
        let output = terminal
            .get("result")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                self.failure(
                    "claude_code_final_message_missing",
                    "Claude Code completed without a final assistant message.",
                    "turn/completed",
                )
                .with_session(Some(&session_id))
            })?;
        let (message_unit, visible_output) = self.complete_message(output);
        turn_event_emit::emit_agent_message_completed_for_unit(
            &session_id,
            &self.config.turn_id,
            &message_unit,
            &visible_output,
        );
        turn_event_emit::emit_turn_event(
            "dispatch.turn.completed",
            &session_id,
            &self.config.turn_id,
            serde_json::json!({"output": output}),
        );
        Ok(ProtocolFinishReport {
            output: output.to_string(),
            session_id,
            turn_id: self.config.turn_id.clone(),
            effective: self.effective.clone(),
            events: std::mem::take(&mut self.transcript_events),
        })
    }

    fn observe_text_delta(&mut self, text: &str) -> (String, String) {
        let start_new = self.active_message.as_ref().is_none_or(|active| {
            !self.pending_native_message_id.is_empty()
                && active.native_id != self.pending_native_message_id
        });
        if start_new {
            let native_id = std::mem::take(&mut self.pending_native_message_id);
            let unit = self.allocate_message_unit();
            self.active_message = Some(ClaudeMessageUnit {
                unit,
                native_id,
                snapshot: String::new(),
                visible: String::new(),
                from_stream: true,
                assistant_snapshot_seen: false,
            });
        }
        let active = self.active_message.as_mut().expect("message unit exists");
        active.snapshot.push_str(text);
        active.visible.push_str(text);
        self.visible_text.push_str(text);
        (active.unit.clone(), text.to_owned())
    }

    fn observe_assistant_snapshot(
        &mut self,
        message: &Value,
        snapshot: &str,
    ) -> Option<(String, String)> {
        let native_id = assistant_message_identity(message);
        let continues_active = self.active_message.as_ref().is_some_and(|active| {
            (!native_id.is_empty() && active.native_id == native_id)
                || (active.from_stream && !active.assistant_snapshot_seen)
        });
        if continues_active {
            let active = self.active_message.as_mut().expect("message unit exists");
            active.assistant_snapshot_seen = true;
            if active.native_id.is_empty() {
                active.native_id = native_id;
            }
            if snapshot.starts_with(&active.snapshot) {
                let suffix = snapshot[active.snapshot.len()..].to_owned();
                active.snapshot.clear();
                active.snapshot.push_str(snapshot);
                active.visible.push_str(&suffix);
                self.visible_text.push_str(&suffix);
                return (!suffix.is_empty()).then(|| (active.unit.clone(), suffix));
            }
            if active.snapshot.starts_with(snapshot) {
                return None;
            }
            // A whole-message echo that disagrees with already streamed deltas
            // is not another delta. The terminal result remains authoritative.
            return None;
        }

        let suffix = novel_text_suffix(&self.visible_text, snapshot).to_owned();
        let unit = self.allocate_message_unit();
        self.active_message = Some(ClaudeMessageUnit {
            unit: unit.clone(),
            native_id,
            snapshot: snapshot.to_owned(),
            visible: suffix.clone(),
            from_stream: false,
            assistant_snapshot_seen: true,
        });
        self.visible_text.push_str(&suffix);
        (!suffix.is_empty()).then_some((unit, suffix))
    }

    fn complete_message(&mut self, output: &str) -> (String, String) {
        if self.active_message.is_none() {
            let suffix = novel_text_suffix(&self.visible_text, output).to_owned();
            let unit = self.allocate_message_unit();
            self.visible_text.push_str(&suffix);
            self.active_message = Some(ClaudeMessageUnit {
                unit,
                native_id: String::new(),
                snapshot: output.to_owned(),
                visible: suffix,
                from_stream: false,
                assistant_snapshot_seen: true,
            });
        }
        let starts_active = self
            .active_message
            .as_ref()
            .is_some_and(|active| output.starts_with(&active.snapshot));
        let stale_active = self
            .active_message
            .as_ref()
            .is_some_and(|active| active.snapshot.starts_with(output));
        if !starts_active && !stale_active {
            let suffix = novel_text_suffix(&self.visible_text, output).to_owned();
            let unit = self.allocate_message_unit();
            self.visible_text.push_str(&suffix);
            self.active_message = Some(ClaudeMessageUnit {
                unit,
                native_id: String::new(),
                snapshot: output.to_owned(),
                visible: suffix,
                from_stream: false,
                assistant_snapshot_seen: true,
            });
        }
        let active = self.active_message.as_mut().expect("message unit exists");
        if output.starts_with(&active.snapshot) {
            let suffix = &output[active.snapshot.len()..];
            active.snapshot.clear();
            active.snapshot.push_str(output);
            active.visible.push_str(suffix);
            self.visible_text.push_str(suffix);
        }
        (active.unit.clone(), active.visible.clone())
    }

    fn allocate_message_unit(&mut self) -> String {
        self.next_message_unit += 1;
        self.next_message_unit.to_string()
    }

    /// Record that the client wrote a user-initiated interrupt for this turn.
    pub fn mark_cancel_requested(&mut self) {
        self.cancel_requested = true;
    }

    /// Whether the client wrote a user-initiated interrupt for this turn.
    pub fn cancel_was_requested(&self) -> bool {
        self.cancel_requested
    }

    /// This Agent's failure, carrying the identity the parser has bound.
    pub fn failure(
        &self,
        code: &'static str,
        message: &'static str,
        stage: &'static str,
    ) -> ProtocolFailure {
        ProtocolFailure::new(code, message, stage)
            .with_session(
                self.observed_session_id
                    .as_deref()
                    .or(self.expected_session_id.as_deref()),
            )
            .with_turn(&self.config.turn_id)
    }
}

fn assistant_text_snapshot(message: &Value) -> Option<String> {
    let text = message
        .pointer("/message/content")
        .and_then(Value::as_array)?
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<String>();
    (!text.trim().is_empty()).then_some(text)
}

fn assistant_message_identity(message: &Value) -> String {
    message
        .pointer("/message/id")
        .or_else(|| message.get("uuid"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn novel_text_suffix<'a>(observed: &str, incoming: &'a str) -> &'a str {
    if incoming.starts_with(observed) {
        return &incoming[observed.len()..];
    }
    let max_overlap = observed.len().min(incoming.len());
    for overlap in (0..=max_overlap).rev() {
        if observed.is_char_boundary(observed.len() - overlap)
            && incoming.is_char_boundary(overlap)
            && incoming.starts_with(&observed[observed.len() - overlap..])
        {
            return &incoming[overlap..];
        }
    }
    incoming
}
