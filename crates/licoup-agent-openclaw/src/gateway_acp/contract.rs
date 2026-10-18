//! The published wire identity of this package's protocol.
//!
//! The release declaration names the converter's source format from here, so the
//! declaration and the protocol the package's entry actually speaks are one fact
//! rather than two strings that can drift. The runtime protocol string the
//! driver reports stays beside the run vocabulary it describes, in
//! [`crate::gateway_acp::model`].

/// The published format identity of the wire this package's entry reads.
pub const PROTOCOL_FORMAT: &str = "openclaw.gateway-acp.v1";
