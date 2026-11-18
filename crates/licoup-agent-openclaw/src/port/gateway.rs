//! The host's OpenClaw Gateway lifecycle, as one installed port.
//!
//! An OpenClaw turn runs an ACP bridge attached to a Gateway the client's own
//! lifecycle starts, reuses or attaches to. Starting and stopping that program,
//! scanning for a free port, writing and reading its state document, health-
//! checking it over bounded HTTP and stopping a process this client owns are
//! *the client's* work: they are the reviewed local process, port, state and
//! HTTP primitives, and they stay in the client.
//!
//! What arrives through this seam is the one question the package asks that
//! engine: *which* endpoint pair an attach names, once the Gateway is reachable.
//! The package owns the answer's vocabulary — the vendor-default and preferred
//! port values and the attach-mode names in [`crate::policy`], and the paired
//! endpoint model in [`crate::gateway`] — while the socket, the state document
//! and the process remain the engine's. The port therefore carries exactly the
//! calls the driver's own leaves make, and no more: a package that asks for a
//! facility it does not use would be declaring a second owner for it.
//!
//! Until a host installs the port it is fail-closed: a package running outside
//! the client reports the same "Gateway unavailable" code the transport has
//! always reported when no Gateway could be ensured, rather than inventing an
//! endpoint. That is what keeps the parser, the replay corpus and the explicit
//! `gatewayWsUrl` attach fully exercisable with no host at all.

use std::sync::OnceLock;

use crate::gateway::GatewayEndpoint;

/// The host facilities one OpenClaw attach needs from the Gateway lifecycle.
///
/// Each member is one answer the host owns. They arrive as one installed value
/// so a package cannot be half-wired: either the host answered the port or the
/// package is fail-closed as a whole.
#[derive(Clone, Copy)]
pub struct GatewayPort {
    /// Ensure this Agent's Gateway is reachable for an attach and answer the
    /// endpoint pair an ACP bridge attaches to.
    ///
    /// `Err` carries the engine's own stable failure code, so a reader can tell
    /// a missing OpenClaw executable from an exhausted port range without
    /// parsing a message; the package maps that code onto its own typed driver
    /// failure and never re-words it.
    pub ensure_attach_endpoint: fn(executable: &str) -> Result<GatewayEndpoint, String>,
}

static PORT: OnceLock<GatewayPort> = OnceLock::new();

/// Install the host's Gateway lifecycle once per process.
pub fn install(port: GatewayPort) -> Result<(), &'static str> {
    PORT.set(port)
        .map_err(|_| "the Gateway port is already installed")
}

/// Whether the host has installed its Gateway lifecycle.
pub fn installed() -> bool {
    PORT.get().is_some()
}

/// Ensure this Agent's Gateway for an attach, or report that no host answered.
///
/// The refusal names the engine's own "unavailable" code rather than a new
/// package code: from the driver's side a Gateway nobody could ensure and a
/// Gateway no host could ask about are the same answer.
pub(crate) fn ensure_attach_endpoint(executable: &str) -> Result<GatewayEndpoint, String> {
    PORT.get()
        .ok_or_else(|| "openclaw_gateway_unavailable".to_owned())
        .and_then(|port| (port.ensure_attach_endpoint)(executable))
}
