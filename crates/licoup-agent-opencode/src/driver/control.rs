use super::{CapabilityProbe, OPENCODE_DRIVER};
use licoup_agent_drivers::local_service::turn_control::{self, ControlDisposition};
use licoup_foundation::core::acp::PROTOCOL_VERSION;

/// Reach the endpoint's active turn for force stop.
///
/// The active-turn registry is the shared control plane's — the same registry
/// every serve-family Agent's stop uses — and this package links it, so the
/// registry answers directly rather than through a host seam: a cancel for a
/// session it does not hold is already the fail-closed answer.
pub fn cancel(session_id: &str) -> ControlDisposition {
    turn_control::cancel(OPENCODE_DRIVER.agent_id, session_id)
}

/// The capabilities one OpenCode serve endpoint reports.
///
/// The endpoint speaks the shared ACP-shaped session lifecycle: it loads,
/// resumes, closes and lists native conversations, and it carries none of the
/// optional prompt channels.
pub fn serve_capabilities() -> CapabilityProbe {
    CapabilityProbe {
        protocol_version: Some(u64::from(PROTOCOL_VERSION)),
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
