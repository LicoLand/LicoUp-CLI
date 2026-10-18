//! Hermetic Codex protocol fixture. Method names match app-server thread/turn APIs.

use super::fixture_config;
use licoup_agent_runtime::work_context::{
    CapabilityProfile, HermeticProtocol, SessionPresence, WorkContextRuntime,
};

pub fn hermetic_codex(profile: CapabilityProfile) -> WorkContextRuntime {
    let knowledge = profile == CapabilityProfile::Low;
    WorkContextRuntime::from_hermetic(
        HermeticProtocol::codex(profile).with_knowledge_injected(knowledge),
        fixture_config(knowledge),
    )
}

pub fn hermetic_codex_high() -> WorkContextRuntime {
    hermetic_codex(CapabilityProfile::High)
}

pub fn hermetic_codex_low() -> WorkContextRuntime {
    hermetic_codex(CapabilityProfile::Low)
}

pub fn hermetic_codex_lost() -> WorkContextRuntime {
    WorkContextRuntime::from_hermetic(
        HermeticProtocol::codex(CapabilityProfile::High).with_presence(SessionPresence::Lost),
        fixture_config(false),
    )
}
