//! Copilot's frame dialect, as the shared ACP engine reads it.
//!
//! `licoup-agent-drivers` owns the ACP engines and declares the port they read
//! one driver's frame policy through: which byte-line decoder reads a frame,
//! whether a frame is a notification or a response, what a session update or a
//! prompt result says, and how a failure becomes the shared transition
//! vocabulary. It owns no Agent's policy. This module is Copilot's answer to
//! that port, and [`DIALECT`] is the value the composition installs.
//!
//! Three members are projections rather than parser functions, and each exists
//! for a stated reason:
//!
//! * [`client_request`] projects this Agent's own
//!   [`crate::parser::ClientRequest`] onto the transport's
//!   [`ProtocolClientRequest`]. The two are field-for-field identical, and the
//!   projection exists so the transport reads one shape rather than every
//!   Agent's.
//! * [`response_is_error`] answers this dialect's error test. Copilot's parser
//!   reports a remote error through the shared ACP envelope rather than
//!   exposing a predicate, and the rule it follows — a frame carrying `error`
//!   is an error — is stated here rather than inherited from another Agent.
//! * [`no_permission_request`] is the honest answer for a profile that never
//!   asks a permission question: Copilot answers approvals through its client
//!   requests, so a permission frame is never invented.

use licoup_agent_drivers::AcpParserRegistration;
use licoup_agent_drivers::ProtocolClientRequest;
use licoup_agent_drivers::ProtocolPermissionRequest;
use serde_json::Value;

use crate::driver::DRIVER_ID;
use crate::parser;

/// Copilot's frame dialect, keyed by the driver identity the transport pools
/// and cancels sessions by.
pub const DIALECT: AcpParserRegistration = AcpParserRegistration {
    driver_id: DRIVER_ID,
    decode_frame: parser::decode_frame,
    is_notification: parser::is_notification,
    response_id_matches: parser::response_id_matches,
    response_is_error,
    session_update: parser::session_update,
    prompt_stop_reason: parser::prompt_stop_reason,
    initialize_response: parser::initialize_response,
    client_request,
    permission_request: no_permission_request,
    completed_transitions: parser::completed_transitions,
    failed_transitions: parser::failed_transitions,
};

/// Project Copilot's client request onto the transport's shape.
pub fn client_request(message: &Value) -> Option<ProtocolClientRequest> {
    let request = parser::client_request(message)?;
    Some(ProtocolClientRequest {
        id: request.id,
        method: request.method,
        session_id: request.session_id,
        allow_once_option: request.allow_once_option,
    })
}

/// The ACP profile's error test: a frame carrying `error` is an error outcome.
pub fn response_is_error(message: &Value) -> bool {
    message.get("error").is_some()
}

/// A dialect that never asks a permission question.
pub fn no_permission_request(_: &Value) -> Option<ProtocolPermissionRequest> {
    None
}
