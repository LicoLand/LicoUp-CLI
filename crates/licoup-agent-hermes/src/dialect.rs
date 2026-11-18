//! Hermes' persistent-ACP frame dialect, as the shared ACP transport reads it.
//!
//! `licoup-agent-drivers` owns the ACP transport engines and declares the port
//! they read one driver's frame dialect through
//! (`acp_driver_runtime::parser_port::AcpParserRegistration`). This module is
//! Hermes' answer to that port: every member is this package's own parser
//! function, and the driver identity is the one the transport already pools and
//! cancels Hermes sessions by.
//!
//! Hermes is the one Agent on the persistent ACP profile, and its entry borrows
//! nothing from another Agent's dialect. Two members are stated here rather than
//! in the parser because they are transport projections rather than wire facts:
//!
//! * [`permission_request`] projects this package's own
//!   [`parser::PermissionRequest`] onto the transport's
//!   `ProtocolPermissionRequest`. The projection exists so the transport reads
//!   one shape rather than every Agent's, and Hermes' is the richer of the two:
//!   it carries the display summary and the requested-tool list.
//! * [`no_client_request`] is the honest answer for a dialect that never asks a
//!   client request over this profile.
//!
//! A composition that installs this registration is what makes Hermes' frames
//! reach Hermes' parser: the transport resolves by driver identity, and a
//! registration this package does not publish is a dialect the transport
//! refuses through its fail-closed default rather than a guess.

use licoup_agent_drivers::acp_driver_runtime::parser_port::{
    AcpParserRegistration, ProtocolClientRequest, ProtocolPermissionRequest,
};
use serde_json::Value;

use crate::parser;

/// The driver identity every Hermes ACP session is pooled, probed and cancelled
/// by. It is this package's declaration and the identity its launch spec
/// carries, so a dialect cannot answer for another Agent's sessions.
pub const DRIVER_ID: &str = "hermes-acp";

/// The vendor wire format this package's frames are recorded in, as the release
/// declaration names it. It is the persistent ACP profile Hermes speaks over
/// stdio JSON-RPC lines.
pub const PROTOCOL_FORMAT: &str = "hermes.acp.v1";

/// Hermes' frame dialect, as the shared transport reads it.
pub fn registration() -> AcpParserRegistration {
    AcpParserRegistration {
        driver_id: DRIVER_ID,
        decode_frame: parser::decode_frame,
        is_notification: parser::is_notification,
        response_id_matches: parser::response_id_matches,
        response_is_error: parser::response_is_error,
        session_update: parser::session_update,
        prompt_stop_reason: parser::prompt_stop_reason,
        initialize_response: parser::initialize_response,
        client_request: no_client_request,
        permission_request,
        completed_transitions: parser::completed_transitions,
        failed_transitions: parser::failed_transitions,
    }
}

/// Project this package's permission request onto the transport's shape.
pub fn permission_request(message: &Value) -> Option<ProtocolPermissionRequest> {
    let request = parser::permission_request(message)?;
    Some(ProtocolPermissionRequest {
        id: request.id,
        method: request.method,
        session_id: request.session_id,
        display_summary: request.display_summary,
        option_id: request.option_id,
        requested_tools: request.requested_tools,
    })
}

/// Hermes never asks a client request over this profile: the persistent ACP
/// dialect reaches the client only to ask permission, so a client request is
/// reported as absent rather than invented.
pub fn no_client_request(_: &Value) -> Option<ProtocolClientRequest> {
    None
}
