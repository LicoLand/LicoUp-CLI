//! Kimi Code's frame dialect, as the shared ACP transport reads it.
//!
//! `licoup-agent-drivers` owns the ACP transport engines and declares the port
//! they read one driver's frame dialect through
//! (`acp_driver_runtime::parser_port::AcpParserRegistration`). This module is
//! Kimi's answer to that port: every member is this package's own parser
//! function, and the driver identity is the one the transport already pools and
//! cancels Kimi sessions by.
//!
//! Two members are stated here rather than in the parser because they are
//! transport projections rather than wire facts:
//!
//! * [`client_request`] projects this package's own [`parser::ClientRequest`]
//!   onto the transport's `ProtocolClientRequest`. The two are field-for-field
//!   identical; the projection exists so the transport reads one shape rather
//!   than every Agent's.
//! * [`response_is_error`] answers this dialect's error test. The frame rule is
//!   the shared ACP envelope rule — a frame carrying `error` is the outcome the
//!   envelope rejects — so it is stated once, here, for the dialect that reports
//!   its error through `failure_from_response` rather than through a predicate.
//!
//! A composition that installs this registration is what makes Kimi's frames
//! reach Kimi's parser: the transport resolves by driver identity, and a
//! registration this package does not publish is a dialect the transport
//! refuses through its fail-closed default rather than a guess.

use licoup_agent_drivers::acp_driver_runtime::parser_port::{
    AcpParserRegistration, ProtocolClientRequest, ProtocolPermissionRequest,
};
use serde_json::Value;

use crate::parser;

/// The driver identity every Kimi ACP session is pooled, probed and cancelled
/// by. It is this package's declaration and the identity its launch spec
/// carries, so a dialect cannot answer for another Agent's sessions.
pub const DRIVER_ID: &str = "kimi-code-acp";

/// Kimi's frame dialect, as the shared transport reads it.
pub fn registration() -> AcpParserRegistration {
    AcpParserRegistration {
        driver_id: DRIVER_ID,
        decode_frame: parser::decode_frame,
        is_notification: parser::is_notification,
        response_id_matches: parser::response_id_matches,
        response_is_error,
        session_update: parser::session_update,
        prompt_stop_reason: parser::prompt_stop_reason,
        initialize_response: parser::initialize_response,
        client_request,
        permission_request,
        completed_transitions: parser::completed_transitions,
        failed_transitions: parser::failed_transitions,
    }
}

/// Project this package's client request onto the transport's shape.
pub fn client_request(message: &Value) -> Option<ProtocolClientRequest> {
    let request = parser::client_request(message)?;
    Some(ProtocolClientRequest {
        id: request.id,
        method: request.method,
        session_id: request.session_id,
        allow_once_option: request.allow_once_option,
    })
}

/// The shared ACP error rule: a frame carrying `error` is the response outcome
/// the ACP envelope rejects.
pub fn response_is_error(message: &Value) -> bool {
    message.get("error").is_some()
}

/// Kimi never asks a permission question over this profile: the ACP v1
/// interaction it uses reaches the client as an ordinary client request, so a
/// permission request is reported as absent rather than invented.
pub fn permission_request(_: &Value) -> Option<ProtocolPermissionRequest> {
    None
}
