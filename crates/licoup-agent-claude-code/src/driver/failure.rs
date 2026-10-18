//! The process half's own reported failures.
//!
//! The failure shape is [`crate::protocol`]'s — a closed code, a static
//! message, a lifecycle stage and the identities the parser bound — and it is
//! re-exported here so the process leaves name one path. What is added below is
//! what that shape cannot state: the failures only supervising a live CLI
//! process can observe.

pub(crate) use crate::protocol::ProtocolFailure;

/// The supervisor's own state is unavailable.
pub(crate) fn supervisor_failure() -> ProtocolFailure {
    ProtocolFailure::new(
        "claude_code_supervisor_unavailable",
        "Claude Code supervisor state is unavailable.",
        "process/supervisor",
    )
}

/// The supervised process's standard I/O is unavailable.
pub(crate) fn pipe_failure() -> ProtocolFailure {
    ProtocolFailure::new(
        "claude_code_pipe_failed",
        "Claude Code standard I/O is unavailable.",
        "process/start",
    )
}
