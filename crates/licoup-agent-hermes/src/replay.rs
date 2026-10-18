//! The recorded-transcript replay arm for Hermes.
//!
//! The arm constructs the same persistent ACP session reducer the production
//! Hermes transport constructs, keyed by the same driver identity, so a recorded
//! transcript can only pass if Hermes' real dialect still reports the recorded
//! facts. The projection, the corpus resolution and the fail-closed properties
//! stay in the SDK's harness; this module is the one thing the harness cannot
//! know — how to build *this* Agent's arm.
//!
//! It is behind `test-support` rather than `cfg(test)` because the host's replay
//! suite drives it, and the host links this crate as a dependency: `cfg(test)`
//! is false for a dependency, so a `cfg(test)` arm is compiled out of exactly the
//! build that calls it.
//!
//! The reducer reads the Hermes dialect the package hands it, exactly as an
//! Agent's own package builds any ACP arm: the dialect is this package's own
//! registration, so the arm replays against the real parser regardless of what
//! a composing build has or has not installed into the transport's port.

use licoup_agent_adapter_sdk::replay::{FrameReplay, RecordedFrame};
use licoup_agent_drivers::acp_session_transport::{
    EffectiveSettings, ProtocolConfig, ProtocolEffect, ProtocolFailure, SessionProtocol,
};
use serde_json::{Value, json};

use crate::dialect;

/// The exact-resume request configuration. `SessionProtocol` fixes its
/// configuration for the whole transcript, and the recorded transcripts open the
/// conversation the client already knew, so the arm replays the driver's
/// exact-resume configuration: `session/load` carrying `REQUESTED_SESSION_ID`.
const REQUESTED_SESSION_ID: &str = "native-session";
const PROMPT: &str = "<REDACTED_CONTENT>";
const CWD: &str = "/workspace/synthetic-project";
/// The turn id is request identity rather than parser output, so the recorder
/// keeps it fixed; it is never projected.
const TURN_ID: &str = "synthetic-turn";

/// Build the replay arm for this package's adapter.
///
/// An adapter id this package does not carry is refused rather than defaulted,
/// so a transcript can never pass against a parser that was never constructed.
pub fn replay_arm(adapter_id: &str) -> Result<Box<dyn FrameReplay>, String> {
    if adapter_id != crate::registration::ADAPTER_ID {
        return Err(format!(
            "no replayable parser is registered for adapter {adapter_id}"
        ));
    }
    Ok(Box::new(Replay::new()))
}

struct Replay {
    protocol: SessionProtocol,
}

impl Replay {
    fn new() -> Self {
        Self {
            protocol: SessionProtocol::with_dialect(
                ProtocolConfig {
                    prompt: PROMPT.to_owned(),
                    requested_session_id: REQUESTED_SESSION_ID.to_owned(),
                    cwd: CWD.to_owned(),
                    model: None,
                    turn_id: TURN_ID.to_owned(),
                    mcp_servers: Vec::new(),
                },
                dialect::registration(),
            ),
        }
    }
}

impl FrameReplay for Replay {
    fn feed(&mut self, frame: &RecordedFrame) -> Result<Vec<Value>, String> {
        // The Hermes session state machine consumes every recorded frame: a
        // frame it rejects is reported as a `fail` effect, never as a boundary
        // error.
        Ok(self
            .protocol
            .handle_frame(frame.payload.as_bytes())
            .effects
            .into_iter()
            .map(effect_projection)
            .collect())
    }
}

/// Project one real `ProtocolEffect`. The turn id is carried from the request
/// configuration and is never parser output, so it is omitted here as it is for
/// the ACP adapters whose state machine generates one.
fn effect_projection(effect: ProtocolEffect) -> Value {
    match effect {
        ProtocolEffect::Send(message) => json!({"effect": "send", "message": message}),
        ProtocolEffect::Complete(outcome) => json!({
            "effect": "complete",
            "output": outcome.output,
            "sessionId": outcome.session_id,
            "turnStatus": outcome.turn_status,
            "effective": effective_projection(&outcome.effective),
            "events": outcome.events,
        }),
        ProtocolEffect::Fail(failure) => failure_projection(&failure),
        ProtocolEffect::AwaitExternalApproval {
            request_id,
            display_summary,
            option_id,
            requested_tools,
        } => json!({
            "effect": "await_external_approval",
            "requestId": request_id,
            "displaySummary": display_summary,
            "optionId": option_id,
            "requestedTools": requested_tools,
        }),
    }
}

fn failure_projection(failure: &ProtocolFailure) -> Value {
    let mut entry = json!({
        "error": failure.code,
        "message": failure.message,
        "stage": failure.stage,
        "userInteractionRequired": failure.user_interaction_required,
    });
    let fields = entry.as_object_mut().expect("projection object");
    if let Some(session_id) = failure.session_id.as_deref() {
        fields.insert("sessionId".into(), json!(session_id));
    }
    if let Some(method) = failure.request_method.as_deref() {
        fields.insert("requestMethod".into(), json!(method));
    }
    if let Some(status) = failure.turn_status.as_deref() {
        fields.insert("turnStatus".into(), json!(status));
    }
    entry
}

fn effective_projection(effective: &EffectiveSettings) -> Value {
    json!({
        "cwd": effective.cwd,
        "model": effective.model,
        "reasoningEffort": effective.reasoning_effort,
        "sandbox": effective.sandbox,
        "approvalPolicy": effective.approval_policy,
    })
}
