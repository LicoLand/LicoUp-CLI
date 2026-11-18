/// Readiness of an optional local integration. This value carries no
/// orchestration ownership and never selects a Conversation role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntegrationState {
    Ready,
    Missing,
    Unavailable,
}
