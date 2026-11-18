//! Service-neutral Model Context Protocol message adapter.
//!
//! The wire codec is isolated from the exact-scope, one-shot external transfer
//! gate. Neither module discovers services, retains endpoints, nor performs I/O.

use std::time::Duration;

mod transfer;
mod wire;

pub use transfer::{McpExternalTransferGate, McpTransferDirection, McpTransferPacket};
pub use wire::{
    McpError, McpMessage, McpRequestId, McpResponse, decode_http_body, decode_stdio_line,
    encode_http_body, encode_stdio_line,
};

/// Protocol revision used only by explicit outbound MCP transfers. Independent
/// inbound services own and negotiate their own protocol contracts.
pub const OUTBOUND_TRANSFER_PROTOCOL_REVISION: &str = "2025-11-25";
pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 1024 * 1024;
pub const DEFAULT_TRANSFER_APPROVAL_TTL: Duration = Duration::from_secs(120);
pub const MAX_TRANSFER_APPROVAL_TTL: Duration = Duration::from_secs(600);

#[cfg(test)]
mod tests;
