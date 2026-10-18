//! Hermetic Pi protocol fixture. Resume stays session/resume; new is session/new.

use super::fixture_config;
use licoup_agent_runtime::work_context::{
    CapabilityProfile, HermeticProtocol, SessionPresence, WorkContextRuntime,
};

pub fn hermetic_pi(profile: CapabilityProfile) -> WorkContextRuntime {
    let knowledge = profile == CapabilityProfile::Low;
    WorkContextRuntime::from_hermetic(
        HermeticProtocol::pi(profile).with_knowledge_injected(knowledge),
        fixture_config(knowledge),
    )
}

pub fn hermetic_pi_high() -> WorkContextRuntime {
    hermetic_pi(CapabilityProfile::High)
}

pub fn hermetic_pi_low() -> WorkContextRuntime {
    hermetic_pi(CapabilityProfile::Low)
}

pub fn hermetic_pi_lost() -> WorkContextRuntime {
    WorkContextRuntime::from_hermetic(
        HermeticProtocol::pi(CapabilityProfile::High).with_presence(SessionPresence::Lost),
        fixture_config(false),
    )
}
