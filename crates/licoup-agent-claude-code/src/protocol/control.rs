//! The control channel the CLI drives the client with, and the shapes it uses.
//!
//! Claude Code asks the client questions on the same stream it answers on: a
//! `control_request` carries a subtype, and the client answers with a
//! `control_response` the CLI correlates by request identifier. This module owns
//! the request shape this client understands; the parser in
//! [`crate::protocol::parser`] owns the framing and the refusals.

/// One permission request the CLI parked on the client's decision.
#[derive(Debug)]
pub struct PermissionRequest {
    /// The correlation identity the answer must carry.
    pub request_id: String,
    /// The vendor tool-call identity, when the request names one.
    pub tool_use_id: Option<String>,
    /// The tool the CLI wants to use, bounded to the tool name only.
    pub tool_name: Option<String>,
    /// The one-line summary the client shows the user.
    pub summary: String,
}
