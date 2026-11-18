//! Which reported failures invalidate the supervised transport.
//!
//! The failure shape is [`crate::protocol`]'s; the decision that a failure
//! means the live CLI process can no longer be trusted belongs here, because
//! this module's siblings own that process. A failure this predicate answers
//! `true` for releases the transport and lets the next turn launch a fresh
//! one.

use crate::protocol::ProtocolFailure;

/// Whether one reported failure leaves the live transport unusable.
pub(crate) fn requires_transport_reset(failure: &ProtocolFailure) -> bool {
    matches!(
        failure.code,
        "claude_code_write_failed"
            | "claude_code_timeout"
            | "claude_code_invalid_json"
            | "claude_code_output_limit"
            | "claude_code_read_failed"
            | "claude_code_exited"
            | "claude_code_cleanup_requested"
            | "claude_code_session_mismatch"
            | "claude_code_session_id_invalid"
            | "claude_code_authentication_required"
    )
}
