//! Service-neutral Agent Client Protocol wire primitives.
//!
//! The facade keeps ACP v1 callers independent from request construction,
//! response validation, JSON-line framing, and protocol data types.

mod codec;
mod error;
mod requests;
mod responses;
mod types;
mod validation;

pub use codec::{decode_json_line, encode_json_line};
pub use error::AcpError;
pub use requests::{
    cancel_notification, close_session_request, initialize_request, session_list_request,
    session_request, text_prompt_request,
};
pub use responses::{
    validate_close_session_response, validate_initialize_response, validate_prompt_response,
    validate_session_list_response, validate_session_response, validate_session_update,
};
pub use types::{
    AcpAgentCapabilities, AcpClientCapabilities, AcpImplementation, AcpInitializeResponse,
    AcpPromptResponse, AcpRequestId, AcpSessionInfo, AcpSessionListResponse, AcpSessionMethod,
    AcpSessionOptions, AcpSessionResponse, AcpSessionUpdate, AcpSessionUpdateKind, AcpStopReason,
};

pub const JSON_RPC_VERSION: &str = "2.0";
pub const PROTOCOL_VERSION: u16 = 1;
pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 1024 * 1024;
pub const MAX_JSON_LINE_BYTES: usize = DEFAULT_MAX_MESSAGE_BYTES + 2;
pub const MAX_ADDITIONAL_DIRECTORIES: usize = 64;
pub const MAX_MCP_SERVERS: usize = 32;
pub const MAX_SESSION_LIST_ITEMS: usize = 1_000;

pub const INITIALIZE_METHOD: &str = "initialize";
pub const SESSION_NEW_METHOD: &str = "session/new";
pub const SESSION_LOAD_METHOD: &str = "session/load";
pub const SESSION_RESUME_METHOD: &str = "session/resume";
pub const SESSION_LIST_METHOD: &str = "session/list";
pub const SESSION_CLOSE_METHOD: &str = "session/close";
pub const SESSION_PROMPT_METHOD: &str = "session/prompt";
pub const SESSION_CANCEL_METHOD: &str = "session/cancel";
pub const SESSION_UPDATE_METHOD: &str = "session/update";

/// The wire protocol name the Hermes TUI Gateway lane reports.
///
/// This names a protocol, not a driver: the Agent inventory's virtual-machine
/// projection, the Hermes TUI Gateway adapter and the packaged readiness
/// documents all have to agree on the same string, and every one of them sits
/// above this crate. It is therefore vocabulary and lives here rather than in
/// whichever of them happens to be extracted first.
pub const HERMES_TUI_GATEWAY_PROTOCOL: &str = "hermes-tui-gateway-stdio-jsonrpc";

#[cfg(test)]
mod tests;
