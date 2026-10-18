//! LicoUp Public Gateway: the public-facing optional services.
//!
//! Two independently packaged capabilities live here:
//!
//! * the MCP surface — `wire` and `server` are the service-neutral MCP engine
//!   (JSON-RPC framing, initialization, tool listing, cancellation and calls);
//!   the optional `service` feature adds the `org.licoland.feature.mcp`
//!   package payload: the five-tool subagent catalog, the CLI client behind
//!   it, the authenticated loopback endpoint, the service process control and
//!   the `connector` module mapping one stdio frame to one authenticated
//!   request;
//! * `telegram` — the optional `org.licoland.channel.telegram` communication
//!   channel: Bot API transport, update loop, lane bridge, binding store and
//!   bot-token custody. It reaches local Agent work only through [`lane`],
//!   which the composing host installs once per process.
//!
//! Only the public CLI process contract crosses into LicoUp; no scheduler,
//! history store or kernel source is linked.

pub mod lane;
mod server;
pub mod telegram;
pub mod wire;
pub use server::*;
pub use wire::*;

#[cfg(feature = "service")]
pub mod application;
#[cfg(feature = "service")]
pub mod lifecycle;
#[cfg(feature = "service")]
pub mod private_state;
#[cfg(feature = "service")]
pub mod transport;
#[cfg(all(feature = "service", any(windows, test)))]
mod windows_private_state;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Map, Value};
    mod server;
    fn object(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }
}
