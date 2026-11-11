//! Kimi Code session identity: the composite `session:agent` id and its
//! conversation-level projection used when scoping queries.

/// The browse/exact-read conversation id drops the delegated-agent suffix: the
/// primary session and its agent wires share one conversation surface.
pub(super) fn kimi_code_conversation_id(native_session_id: &str) -> &str {
    native_session_id
        .split_once(':')
        .map_or(native_session_id, |(session, _)| session)
}
