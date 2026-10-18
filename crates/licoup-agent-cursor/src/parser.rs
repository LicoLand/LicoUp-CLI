//! The Cursor Agent CLI turn dialect, classified exactly once below the port.
//!
//! This is the sole ingress for Cursor's `strict-lf-ndjson` turn stream, per
//! ADR-0008: one raw line becomes this Agent's effects here and is never
//! re-parsed above. The parser reports what the protocol reported — delivery
//! acknowledgement, streamed text, structured tool calls, application tool
//! errors and a terminal result — and it settles no turn, imposes no implicit
//! timeout, and hides no content. The conversation layer remains the sole turn
//! authority.
//!
//! Two Cursor protocol facts are load-bearing and stated here rather than
//! inferred by a reader:
//!
//! - **Delivery is the exact prompt echo.** A system/init frame proves only
//!   that the process started, so assistant output, tool activity and a
//!   terminal result are refused until the turn's own user frame carries the
//!   exact prompt this parser was built with.
//! - **Identity is bound, never relabeled.** The turn runs against the native
//!   chat it was launched with; a frame naming another conversation is a
//!   protocol failure rather than a silent rebind.
//!
//! The Subagent MCP caller surface Cursor speaks is part of this dialect: the
//! application error codes an installed Cursor client returns for a delegated
//! turn are classified here, where the wire frame is read, instead of by a
//! second reader above the port.

use licoup_agent_adapter_sdk::adapters::{AdapterContract, NativeLineParser};
use licoup_agent_adapter_sdk::{
    LifecycleStage, TextForm, TextReconciler, Transition, TransitionReducer,
};
use serde_json::Value;
use std::collections::BTreeSet;

use crate::errors::CursorFailureKind;
use crate::model::{EffectiveSettings, MAX_SESSION_ID_LEN, MIN_SESSION_ID_LEN};

pub const CONTRACT: AdapterContract = AdapterContract::new("cursor", "strict-lf-ndjson");

pub fn completed_transitions(output: &str) -> Vec<Transition> {
    terminal_transitions("cursor:reply", output)
}

pub fn failure_transitions(code: &str, stage: &str, message: &str) -> Vec<Transition> {
    let mut reducer = TransitionReducer::default();
    let mut transitions = reducer.advance(LifecycleStage::Submitted);
    if let Some(failure) = reducer.fail(code, stage, message) {
        transitions.push(failure);
    }
    transitions
}

pub enum CreatedSessionFailure {
    Missing,
    Invalid,
}

pub fn parse_created_session(output: &str) -> Result<String, CreatedSessionFailure> {
    let session_id = output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or(CreatedSessionFailure::Missing)?;
    if !safe_session_id(session_id) {
        return Err(CreatedSessionFailure::Invalid);
    }
    Ok(session_id.to_owned())
}

pub fn safe_session_id(session_id: &str) -> bool {
    let len = session_id.len();
    len >= MIN_SESSION_ID_LEN
        && len <= MAX_SESSION_ID_LEN
        && session_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
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

pub struct CursorParser {
    requested_session: String,
    expected_prompt: String,
    observed_session: String,
    turn_id: String,
    effective: EffectiveSettings,
    text: TextReconciler,
    accepted: bool,
    observed_tool_call_ids: BTreeSet<String>,
    observed_tool_error_ids: BTreeSet<String>,
}

pub enum CursorEffect {
    Accepted {
        session_id: String,
        turn_id: String,
    },
    Text {
        session_id: String,
        turn_id: String,
        text: String,
    },
    Tool {
        session_id: String,
        turn_id: String,
        tool_name: String,
    },
    ToolError {
        session_id: String,
        turn_id: String,
        tool_name: String,
        error_code: &'static str,
    },
    Complete(CursorOutcome),
}

pub struct CursorOutcome {
    pub output: String,
    pub session_id: String,
    pub turn_id: String,
    pub effective: EffectiveSettings,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorParseFailure {
    InvalidJson,
    IdentityMismatch,
    PromptAcknowledgementMissing,
    PromptAcknowledgementMismatch,
    TurnFailed(CursorFailureKind),
}

impl CursorParser {
    pub fn new(
        requested_session: &str,
        expected_prompt: &str,
        effective: EffectiveSettings,
    ) -> Self {
        Self {
            requested_session: requested_session.to_owned(),
            expected_prompt: expected_prompt.to_owned(),
            observed_session: requested_session.to_owned(),
            turn_id: String::new(),
            effective,
            text: TextReconciler::default(),
            accepted: false,
            observed_tool_call_ids: BTreeSet::new(),
            observed_tool_error_ids: BTreeSet::new(),
        }
    }

    pub fn session_id(&self) -> &str {
        if self.observed_session.is_empty() {
            &self.requested_session
        } else {
            &self.observed_session
        }
    }

    pub fn parse_line(&mut self, line: &[u8]) -> Result<Vec<CursorEffect>, CursorParseFailure> {
        NativeLineParser::parse_line(self, line)
    }
}

impl NativeLineParser for CursorParser {
    type Report = Vec<CursorEffect>;
    type Error = CursorParseFailure;

    /// Sole ingress for a PTY-originated strict NDJSON wire line.
    fn parse_line(&mut self, line: &[u8]) -> Result<Self::Report, Self::Error> {
        let trimmed = line
            .iter()
            .copied()
            .skip_while(|byte| byte.is_ascii_whitespace())
            .collect::<Vec<_>>();
        if trimmed.iter().all(|byte| byte.is_ascii_whitespace()) {
            return Ok(Vec::new());
        }
        let message: Value =
            serde_json::from_slice(&trimmed).map_err(|_| CursorParseFailure::InvalidJson)?;
        let mut effects = Vec::new();
        if let Some(id) = session_id(&message) {
            // The turn was launched bound to the requested (or freshly
            // created) native id. Any explicit stream identity must equal that
            // bound id before this frame contributes an accepted, chunk, or
            // terminal effect: a different or drifted conversation is never
            // silently relabeled.
            if id != self.requested_session {
                return Err(CursorParseFailure::IdentityMismatch);
            }
            self.observed_session = id.to_owned();
        }
        // The real protocol carries `request_id` on frames, never `uuid`. The
        // first frame with an explicit transport id wins; the synthetic
        // default only stands in while nothing better has arrived.
        let frame_turn_id = native_turn_id(&message);
        if self.turn_id.is_empty()
            || (self.turn_id == "cursor-turn" && frame_turn_id != "cursor-turn")
        {
            self.turn_id = frame_turn_id.to_owned();
        }
        update_effective(&message, &mut self.effective);
        if !self.accepted && is_user_frame(&message) {
            let acknowledged = user_prompt_text(&message)
                .filter(|text| text == &self.expected_prompt)
                .is_some();
            if !acknowledged {
                return Err(CursorParseFailure::PromptAcknowledgementMismatch);
            }
            effects.push(CursorEffect::Accepted {
                session_id: self.observed_session.clone(),
                turn_id: self.turn_id.clone(),
            });
            self.accepted = true;
        }
        let terminal = terminal_result(&message).map(str::to_owned);
        let terminal_failed = terminal.is_some() && is_error_result(&message);
        if terminal_failed {
            return Err(CursorParseFailure::TurnFailed(
                CursorFailureKind::from_terminal_subtype(
                    message.get("subtype").and_then(Value::as_str),
                ),
            ));
        }
        let structured_tool_calls = structured_tool_calls(&message);
        // A system/init frame proves only that the process started. Delivery
        // belongs exclusively to Cursor's exact user prompt echo. Never show
        // assistant output or success for an unacknowledged input.
        if !self.accepted
            && (is_assistant_frame(&message)
                || !structured_tool_calls.is_empty()
                || terminal.is_some())
        {
            return Err(CursorParseFailure::PromptAcknowledgementMissing);
        }
        for (call_id, tool_name) in structured_tool_calls {
            if self.observed_tool_call_ids.insert(call_id.to_owned()) {
                effects.push(CursorEffect::Tool {
                    session_id: self.observed_session.clone(),
                    turn_id: self.turn_id.clone(),
                    tool_name: tool_name.to_owned(),
                });
            }
        }
        if let Some((call_id, tool_name, error_code)) = completed_mcp_tool_error(&message)
            && self.observed_tool_error_ids.insert(call_id.to_owned())
        {
            effects.push(CursorEffect::ToolError {
                session_id: self.observed_session.clone(),
                turn_id: self.turn_id.clone(),
                tool_name: tool_name.to_owned(),
                error_code,
            });
        }
        if is_assistant_frame(&message) {
            if let Some(text) = assistant_text(&message) {
                // Current Cursor stream-json marks partial deltas with a
                // timestamp and then emits one timestamp-free cumulative
                // assistant snapshot before the result frame. A rewritten
                // snapshot is not a failed turn: keep the already-emitted
                // reply so a later result frame can still complete.
                let form = if message.get("timestamp_ms").is_some() {
                    TextForm::Delta(&text)
                } else {
                    TextForm::Cumulative(&text)
                };
                if let Ok(suffix) = self.text.observe("assistant", form)
                    && !suffix.is_empty()
                {
                    effects.push(CursorEffect::Text {
                        session_id: self.observed_session.clone(),
                        turn_id: self.turn_id.clone(),
                        text: suffix,
                    });
                }
            }
        }
        if let Some(result) = terminal {
            // Prefer an extending result suffix. When Cursor's result
            // rendering diverges from streamed assistant text, the live
            // reply stays the turn output instead of failing a finished job.
            if let Ok(suffix) = self
                .text
                .observe("assistant", TextForm::Cumulative(&result))
                && !suffix.is_empty()
            {
                effects.push(CursorEffect::Text {
                    session_id: self.observed_session.clone(),
                    turn_id: self.turn_id.clone(),
                    text: suffix,
                });
            }
            let output = self
                .text
                .observed("assistant")
                .unwrap_or_default()
                .to_owned();
            effects.push(CursorEffect::Complete(CursorOutcome {
                output,
                session_id: self.observed_session.clone(),
                turn_id: self.turn_id.clone(),
                effective: self.effective.clone(),
            }));
        }
        Ok(effects)
    }
}

fn update_effective(message: &Value, effective: &mut EffectiveSettings) {
    let is_init = message.get("subtype").and_then(Value::as_str) == Some("init")
        || message.get("type").and_then(Value::as_str) == Some("init");
    if !is_init {
        return;
    }
    if effective.model.as_deref().is_none_or(str::is_empty)
        && let Some(model) = message
            .get("model")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
    {
        // Cursor reports a display label (for example `Composer 2.5`) even
        // when the launch selector was the stable slug (`composer-2.5`). Keep
        // an explicit request authoritative so validation compares one
        // selector namespace; use the init value only for default-model turns.
        effective.model = Some(model.to_owned());
    }
    if let Some(mode) = message
        .get("permissionMode")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        effective.permission_mode = Some(mode.to_owned());
    }
}

fn native_turn_id(message: &Value) -> &str {
    message
        .get("uuid")
        .or_else(|| message.get("turn_id"))
        .or_else(|| message.get("request_id"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("cursor-turn")
}

fn session_id(message: &Value) -> Option<&str> {
    message
        .get("session_id")
        .or_else(|| message.get("sessionId"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn is_assistant_frame(message: &Value) -> bool {
    message.get("type").and_then(Value::as_str) == Some("assistant")
        && message.pointer("/message/role").and_then(Value::as_str) == Some("assistant")
}

fn is_user_frame(message: &Value) -> bool {
    message.get("type").and_then(Value::as_str) == Some("user")
        && message.pointer("/message/role").and_then(Value::as_str) == Some("user")
}

fn user_prompt_text(message: &Value) -> Option<String> {
    text_blocks(message)
}

fn assistant_text(message: &Value) -> Option<String> {
    text_blocks(message)
}

fn structured_tool_calls(message: &Value) -> Vec<(&str, &str)> {
    let mut calls = message
        .pointer("/message/content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        .filter_map(|block| {
            let call_id = block
                .get("id")
                .and_then(Value::as_str)
                .filter(|value| valid_tool_call_id(value))?;
            let tool_name = block
                .get("name")
                .and_then(Value::as_str)
                .filter(|value| valid_tool_identifier(value))?;
            Some((call_id, tool_name))
        })
        .collect::<Vec<_>>();
    if message.get("type").and_then(Value::as_str) == Some("tool_call")
        && let Some(arguments) = message
            .pointer("/tool_call/mcpToolCall/args")
            .and_then(Value::as_object)
    {
        let call_id = arguments
            .get("toolCallId")
            .or_else(|| message.pointer("/tool_call/toolCallId"))
            .or_else(|| message.get("call_id"))
            .and_then(Value::as_str)
            .filter(|value| valid_tool_call_id(value));
        let tool_name = arguments
            .get("toolName")
            .or_else(|| arguments.get("name"))
            .and_then(Value::as_str)
            .filter(|value| valid_tool_identifier(value));
        if let (Some(call_id), Some(tool_name)) = (call_id, tool_name) {
            calls.push((call_id, tool_name));
        }
    }
    calls
}

fn valid_tool_call_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 512 && !value.contains('\0')
}

fn valid_tool_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':' | b'/')
        })
}

fn completed_mcp_tool_error(message: &Value) -> Option<(&str, &str, &'static str)> {
    if message.get("type").and_then(Value::as_str) != Some("tool_call")
        || message.get("subtype").and_then(Value::as_str) != Some("completed")
    {
        return None;
    }
    let arguments = message
        .pointer("/tool_call/mcpToolCall/args")
        .and_then(Value::as_object)?;
    let call_id = arguments
        .get("toolCallId")
        .or_else(|| message.pointer("/tool_call/toolCallId"))
        .and_then(Value::as_str)
        .filter(|value| valid_tool_call_id(value))?;
    let tool_name = arguments
        .get("toolName")
        .or_else(|| arguments.get("name"))
        .and_then(Value::as_str)
        .filter(|value| valid_tool_identifier(value))?;
    let result = message.pointer("/tool_call/mcpToolCall/result")?;
    let wire = serde_json::to_string(result).ok()?;
    const APPLICATION_CODES: &[&str] = &[
        "caller_authentication_required",
        "caller_membership_binding_required",
        "caller_membership_not_authorized",
        "conversation_not_found",
        "conversation_state_unavailable",
        "conversation_working_directory_mismatch",
        "dispatch_reconciliation_required",
        "invalid_working_directory",
        "subagent_adapter_unavailable",
        "subagent_capability_unavailable",
        "subagent_caller_membership_inactive",
        "subagent_cross_conversation_rejected",
        "subagent_depth_exceeded",
        "subagent_dispatch_receipt_invalid",
        "subagent_dispatch_transition_invalid",
        "subagent_dispatch_uncertain",
        "subagent_duplicate_active_edge",
        "subagent_lineage_caller_mismatch",
        "subagent_lineage_cycle",
        "subagent_parent_dispatch_unavailable",
        "subagent_readiness_rejected",
        "subagent_resume_unavailable",
        "subagent_self_call_rejected",
        "subagent_target_invalid",
        "subagent_target_membership_inactive",
    ];
    APPLICATION_CODES
        .iter()
        .copied()
        .find(|code| wire.contains(code))
        .map(|code| (call_id, tool_name, code))
}

fn text_blocks(message: &Value) -> Option<String> {
    let blocks = message
        .pointer("/message/content")
        .and_then(Value::as_array)?;
    let text = blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<String>();
    (!text.is_empty()).then_some(text)
}

fn terminal_result(message: &Value) -> Option<&str> {
    (message.get("type").and_then(Value::as_str) == Some("result")).then(|| {
        message
            .get("result")
            .and_then(Value::as_str)
            .unwrap_or_default()
    })
}

fn is_error_result(message: &Value) -> bool {
    // `is_error` is the protocol's explicit terminal authority. Cursor can
    // report a non-success subtype after a recoverable tool problem while
    // still returning a valid final answer with `is_error: false`; treating
    // the subtype alone as fatal discards that answer. Older frames without
    // the flag keep the conservative subtype fallback.
    message
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            message
                .get("subtype")
                .and_then(Value::as_str)
                .is_some_and(|value| value != "success")
        })
}
