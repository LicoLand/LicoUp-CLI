//! The Gateway endpoint vocabulary one OpenClaw ACP attach names.
//!
//! OpenClaw serves one Gateway over a loopback *pair*: an HTTP endpoint the
//! client calls and a WebSocket endpoint an ACP bridge attaches to. Both
//! spellings are this Agent's protocol fact, and they always travel together —
//! an attach that named one without the other could not be health-checked and
//! attached as the same service. The paired model therefore lives here, beside
//! the ACP frames that use it.
//!
//! What does *not* live here is the client's own Gateway lifecycle: which port
//! range the client prefers, where it keeps the service state document, how it
//! health-checks and stops a process, and which executables it spawns. Those are
//! the client's operational choices and its reviewed process sites, and they
//! stay in `platform::openclaw_gateway`. The client projects its own state
//! document onto this model; the model never reads that document itself.

/// One OpenClaw Gateway endpoint, as its HTTP and WebSocket spellings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GatewayEndpoint {
    /// The host both spellings name. A managed Gateway binds loopback.
    pub host: String,
    /// The port both spellings name.
    pub port: u16,
    /// The HTTP endpoint the client queries.
    pub attach_url: String,
    /// The WebSocket endpoint an ACP bridge attaches to.
    pub ws_url: String,
}

impl GatewayEndpoint {
    /// The endpoint pair one host and port spell, in OpenClaw's own schemes.
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        let host = host.into();
        Self {
            attach_url: format!("http://{}:{}", host, port),
            ws_url: format!("ws://{}:{}", host, port),
            host,
            port,
        }
    }
}
