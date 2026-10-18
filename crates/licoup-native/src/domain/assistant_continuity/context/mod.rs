//! Context composition leaf. M1 replaces the body without editing the parent.

mod compose;
mod store;
mod workspace;

pub use compose::UnavailableContextCompositionService;
pub use store::FrozenContextStore;
pub use workspace::ContinuityWorkspace;

pub fn context_composition_port() -> UnavailableContextCompositionService {
    UnavailableContextCompositionService::empty()
}
