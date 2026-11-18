//! One completed Kilo turn, projected onto the shared driver result.
//!
//! The projection is a field copy, not a second derivation: the parser already
//! reported the assistant text and the native tool interactions, and the request
//! already carries the settings the turn ran with. Nothing is re-read from a
//! vendor document here, and nothing is invented that the endpoint did not say.
//!
//! The capability answer is this Agent's own declaration of what its endpoint
//! supports. It is produced in exactly one place so the probe the host runs
//! before offering the Agent and the result of a real turn cannot disagree.

use super::ProtocolFailure;
use super::config::ServeTurnConfig;
use crate::parser::{ServeMessage, Transition};

/// What one Kilo turn reported.
pub struct ProtocolOutcome {
    pub output: String,
    pub transitions: Vec<Transition>,
    pub session_id: String,
    pub thread_id: String,
    pub turn_id: String,
    pub turn_status: String,
    pub effective: EffectiveSettings,
    pub capabilities: CapabilityProbe,
}

/// The settings one turn actually ran with, as the endpoint was asked.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EffectiveSettings {
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub mode: Option<String>,
    pub runtime_agent: Option<String>,
    pub allow_all: Option<bool>,
    pub sandbox: Option<serde_json::Value>,
    pub approval_policy: Option<serde_json::Value>,
}

/// What this Agent's endpoint supports, in the shared probe vocabulary.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CapabilityProbe {
    pub protocol_version: Option<u64>,
    pub load_session: bool,
    pub resume_session: bool,
    pub close_session: bool,
    pub list_sessions: bool,
    pub delete_session: bool,
    pub additional_directories: bool,
    pub image_prompts: bool,
    pub audio_prompts: bool,
    pub embedded_context: bool,
}

/// This Agent's capability declaration.
///
/// Every field is what the `serve` endpoint really does, not what would be
/// convenient: sessions can be loaded, resumed, closed and listed, while
/// deletion, extra directories, image and audio prompts and embedded context are
/// not offered and are therefore reported as unsupported.
pub fn serve_capabilities() -> CapabilityProbe {
    CapabilityProbe {
        protocol_version: Some(1),
        load_session: true,
        resume_session: true,
        close_session: true,
        list_sessions: true,
        delete_session: false,
        additional_directories: false,
        image_prompts: false,
        audio_prompts: false,
        embedded_context: false,
    }
}

/// Project one completed message onto this Agent's turn result.
pub fn project_turn(
    response: ServeMessage,
    session_id: String,
    turn_id: String,
    config: &ServeTurnConfig,
) -> Result<ProtocolOutcome, ProtocolFailure> {
    Ok(ProtocolOutcome {
        output: response.output,
        transitions: response.transitions,
        thread_id: session_id.clone(),
        session_id,
        turn_id,
        // The endpoint reports its terminal through the message document's own
        // return; a completed serve turn is an end of turn and never a
        // cancellation, which this protocol reports out of band.
        turn_status: "end_turn".to_string(),
        effective: EffectiveSettings {
            cwd: Some(config.cwd.clone()),
            model: config.model.clone(),
            reasoning_effort: config.reasoning_effort.clone(),
            mode: config.mode.clone(),
            runtime_agent: config.runtime_agent.clone(),
            allow_all: config.allow_all,
            sandbox: None,
            approval_policy: None,
        },
        capabilities: serve_capabilities(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn the_projection_copies_the_settings_the_turn_ran_with() {
        let config = ServeTurnConfig::from_params(
            &json!({"cwd": "/workspace/project", "model": "kilo/kilo-auto/free", "mode": "code"}),
            "hello",
            "",
            None,
        )
        .unwrap();
        let outcome = project_turn(
            ServeMessage {
                output: "answer".to_owned(),
                transitions: Vec::new(),
            },
            "kilo-1".to_owned(),
            "turn-1".to_owned(),
            &config,
        )
        .unwrap();
        assert_eq!(outcome.output, "answer");
        assert_eq!(outcome.session_id, "kilo-1");
        assert_eq!(outcome.thread_id, "kilo-1");
        assert_eq!(outcome.turn_status, "end_turn");
        assert_eq!(outcome.effective.cwd.as_deref(), Some("/workspace/project"));
        assert_eq!(
            outcome.effective.model.as_deref(),
            Some("kilo/kilo-auto/free")
        );
        assert_eq!(outcome.effective.mode.as_deref(), Some("code"));
        // The projection never claims a capability the declaration denies.
        assert_eq!(outcome.capabilities, serve_capabilities());
        assert!(!outcome.capabilities.delete_session);
        assert!(!outcome.capabilities.image_prompts);
        let _ = Path::new("/workspace/project");
    }
}
