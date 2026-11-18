//! Replay arm for the `claude-code` adapter.
//!
//! One real [`ClaudeCodeParser`] — the same parser a live turn drives — consumes
//! the recorded `lf-ndjson` frames, so every projection is the parser's own
//! report and never a re-derivation of the payload.
//!
//! A replay boundary has no launch, so the parser is built hermetically with
//! this package's own launch vocabulary: the config carries a synthetic prompt
//! and turn id, and the effective settings start empty (an init frame supplies
//! the model and permission mode it reports, exactly as it does live). The known
//! native session is taken from the transcript's own handshake — the identity
//! the CLI reports — so every later frame must carry that same conversation or
//! the parser rejects it.
//!
//! The cancel ledger is not part of any frame: the process half marks it when
//! the client writes an interrupt, which is process-local state an
//! agent-to-client transcript cannot carry. The arm therefore does not invent
//! it, and the interrupted terminal below is reported exactly as the parser
//! classifies it without the ledger — a turn failure, not a cancellation.

use licoup_agent_adapter_sdk::adapters::NativeLineParser;
use licoup_agent_adapter_sdk::replay::{FrameReplay, RecordedFrame};
use serde_json::{Map, Value, json};

use crate::protocol::failure::ProtocolFailure;
use crate::protocol::launch::LaunchIdentity;
use crate::protocol::params::DriverConfig;
use crate::protocol::parser::{ClaudeCodeParser, ClaudeEffect, ProtocolFinishReport};
use crate::protocol::settings::EffectiveSettings;

/// Synthetic turn identity for the replayed turn.
const REPLAY_TURN: &str = "synthetic-turn";

/// Synthetic prompt for the replayed turn. It never reaches a projection.
const REPLAY_PROMPT: &str = "synthetic-user-prompt";

/// Synthetic executable name for the replayed launch.
const REPLAY_EXECUTABLE: &str = "claude";

struct Replay {
    /// The parser borrows its config for the lifetime of the arm. The config is
    /// one small synthetic value per replayed transcript, so it is deliberately
    /// leaked rather than making the arm self-referential.
    config: &'static DriverConfig,
    identity: LaunchIdentity,
    parser: Option<ClaudeCodeParser<'static>>,
}

impl Replay {
    fn new() -> Result<Self, String> {
        Ok(Self {
            config: Box::leak(Box::new(DriverConfig {
                prompt: REPLAY_PROMPT.to_owned(),
                requested_session_id: String::new(),
                model: None,
                reasoning_effort: None,
                permission_mode: None,
                allowed_tools: None,
                private_instructions: None,
                turn_id: REPLAY_TURN.to_owned(),
            })),
            identity: LaunchIdentity {
                executable: REPLAY_EXECUTABLE.to_owned(),
                cwd: None,
                model: None,
                reasoning_effort: None,
                permission_mode: None,
                allowed_tools: None,
                private_instructions: None,
                resume_session_id: None,
            },
            parser: None,
        })
    }
}

/// The replay arm this package publishes: one real parser per recorded
/// transcript, and a refusal for any adapter this package does not carry.
pub fn replay_arm(adapter_id: &str) -> Result<Box<dyn FrameReplay>, String> {
    if adapter_id != crate::registration::ADAPTER_ID {
        return Err(format!(
            "the Claude Code package carries no replayable parser for adapter {adapter_id}"
        ));
    }
    Ok(Box::new(Replay::new()?))
}

impl FrameReplay for Replay {
    fn feed(&mut self, frame: &RecordedFrame) -> Result<Vec<Value>, String> {
        // The corpus records the CLI's side of the stream. A frame the process
        // half wrote cannot be replayed as CLI output, so it fails here instead
        // of being mis-parsed as a vendor frame.
        if frame.direction != "agent-to-client" {
            return Err(format!(
                "a claude-code transcript records the CLI's agent-to-client frames; a {:?} frame \
                 is not consumed by this boundary",
                frame.direction
            ));
        }
        let parser = match self.parser.as_mut() {
            Some(parser) => parser,
            None => {
                // Launch context only: bind the parser to the conversation the
                // handshake reports. This never contributes to a projection.
                let known_session = known_session(&frame.payload);
                self.parser = Some(ClaudeCodeParser::new(
                    self.config,
                    &self.identity,
                    known_session,
                ));
                self.parser
                    .as_mut()
                    .expect("the parser was just constructed")
            }
        };
        match parser.parse_line(frame.payload.as_bytes()) {
            Ok(Some(effect)) => Ok(vec![effect_json(effect)]),
            Ok(None) => Ok(Vec::new()),
            Err(failure) => Ok(vec![failure_json(&failure)]),
        }
    }
}

/// Launch context only: the native conversation the handshake binds this turn
/// to. This never contributes to a projection.
fn known_session(payload: &str) -> Option<String> {
    let message: Value = serde_json::from_str(payload).ok()?;
    message
        .get("session_id")
        .or_else(|| message.get("sessionId"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn effect_json(effect: ClaudeEffect) -> Value {
    match effect {
        ClaudeEffect::Permission(request) => json!({
            "effect": "permission",
            "requestId": request.request_id,
            "toolUseId": request.tool_use_id,
            "toolName": request.tool_name,
            "summary": request.summary,
        }),
        ClaudeEffect::Control { response } => json!({
            "effect": "control",
            "response": response,
        }),
        ClaudeEffect::Progress { session_id } => json!({
            "effect": "progress",
            "sessionId": session_id,
        }),
        ClaudeEffect::ProtocolFinished(report) => finished_json(&report),
    }
}

fn finished_json(report: &ProtocolFinishReport) -> Value {
    json!({
        "effect": "protocol_finished",
        "output": report.output,
        "sessionId": report.session_id,
        "turnId": report.turn_id,
        "effective": effective_json(&report.effective),
        "events": report.events,
    })
}

fn effective_json(effective: &EffectiveSettings) -> Value {
    json!({
        "cwd": effective.cwd,
        "model": effective.model,
        "reasoningEffort": effective.reasoning_effort,
        "permissionMode": effective.permission_mode,
        "sandbox": effective.sandbox,
        "approvalPolicy": effective.approval_policy,
    })
}

/// A rejected frame is reported as the parser's own failure: its closed code,
/// its static classification, and the identity the parser had bound.
fn failure_json(failure: &ProtocolFailure) -> Value {
    let mut entry = Map::new();
    entry.insert("error".to_owned(), json!(failure.code));
    entry.insert("message".to_owned(), json!(failure.message));
    entry.insert("stage".to_owned(), json!(failure.stage));
    entry.insert(
        "userInteractionRequired".to_owned(),
        json!(failure.user_interaction_required),
    );
    for (key, value) in [
        ("requestMethod", &failure.request_method),
        ("sessionId", &failure.session_id),
        ("threadId", &failure.thread_id),
        ("turnId", &failure.turn_id),
        ("turnStatus", &failure.turn_status),
    ] {
        if let Some(value) = value {
            entry.insert(key.to_owned(), json!(value));
        }
    }
    Value::Object(entry)
}
