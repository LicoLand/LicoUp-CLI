//! Replay arm for the openclaw adapter.
//!
//! The Gateway ACP protocol state machine is this package's own ingress, so the
//! arm lives with the package rather than in the composition: a host that
//! composes no OpenClaw package answers no OpenClaw transcript, and one that
//! composes it gets the arm built exactly as the package's own driver builds it.

use licoup_agent_adapter_sdk::Transition;
use licoup_agent_adapter_sdk::replay::{FrameReplay, RecordedFrame};
use serde_json::{Value, json};

use crate::gateway_acp::errors::ProtocolFailure;
use crate::gateway_acp::model::EffectiveSettings;
use crate::gateway_acp::params::ProtocolConfig;
use crate::parser::protocol::{OpenClawProtocol, ProtocolEffect};
use crate::registration::ADAPTER_ID;

/// The Gateway conversation key the recorded transcripts resume. OpenClaw
/// derives its resumable identity from `sessionKey`, and a resume request may
/// not name a key that differs from the requested conversation, so the key and
/// the requested session id are the same identity here exactly as they are in
/// the driver.
const GATEWAY_SESSION_KEY: &str = "agent:main:acp:native-session";
const PROMPT: &str = "<REDACTED_CONTENT>";
const CWD: &str = "/workspace/synthetic-project";
/// The turn id is request identity rather than parser output, so the recorder
/// keeps it fixed; it is never projected.
const TURN_ID: &str = "synthetic-turn";

/// Build this adapter's replay arm for the composed parser set.
///
/// Another adapter is refused rather than defaulted: this program carries one
/// Agent, and a corpus that names a different channel must not replay here.
pub fn replay_arm(adapter_id: &str) -> Result<Box<dyn FrameReplay>, String> {
    if adapter_id != ADAPTER_ID {
        return Err(format!(
            "no replayable parser is registered for adapter {adapter_id}"
        ));
    }
    Ok(Box::new(Replay::new()?))
}

pub struct Replay {
    protocol: OpenClawProtocol,
}

impl Replay {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            protocol: OpenClawProtocol::new(ProtocolConfig {
                prompt: PROMPT.to_owned(),
                requested_session_id: GATEWAY_SESSION_KEY.to_owned(),
                native_session_key: Some(GATEWAY_SESSION_KEY.to_owned()),
                cwd: CWD.to_owned(),
                reasoning_effort: None,
                turn_id: TURN_ID.to_owned(),
                mcp_servers: Vec::new(),
            }),
        })
    }
}

impl FrameReplay for Replay {
    fn feed(&mut self, frame: &RecordedFrame) -> Result<Vec<Value>, String> {
        // The Gateway state machine consumes every recorded frame: a frame it
        // rejects is reported as a `fail` effect, never as a boundary error.
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
        ProtocolEffect::Complete(outcome) => {
            let outcome = *outcome;
            json!({
                "effect": "complete",
                "output": outcome.output,
                "sessionId": outcome.session_id,
                "turnStatus": outcome.turn_status,
                "effective": effective_projection(&outcome.effective),
                "transitions": outcome
                    .transitions
                    .iter()
                    .map(Transition::to_json)
                    .collect::<Vec<Value>>(),
            })
        }
        ProtocolEffect::Fail(failure) => failure_projection(&failure),
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
