//! Hermes' ACP launch contract and the process half the shared engine runs.
//!
//! Hermes contributes its fixed launch and probe contract — the `hermes acp`
//! entry point, the capability commands, the runtime identity its frames are
//! recorded under — and nothing about how a process is owned: persistent ACP
//! process ownership, session routing, cancellation and the shared result types
//! live in the service-neutral session transport
//! ([`licoup_agent_drivers::acp_session_transport`]), which names no Agent.
//!
//! What this module adds to that engine is exactly what is Hermes':
//!
//! - [`RUNTIME_PROTOCOL`] and [`HERMES_SESSION_DRIVER`], the declaration the
//!   transport resolves this Agent's frame dialect by;
//! - [`execute_with_connection`], one bounded turn, and [`cancel`] and
//!   [`cleanup_session`], the control entries the host's lane calls;
//! - [`probe`], the two fixed `acp --check` / `acp --version` commands, which are
//!   capability questions rather than a turn.
//!
//! A turn bound to a Hermes TUI gateway is *not* here: that transport is the
//! host's, so the host picks the lane where the runtime connection is in view
//! and hands a local turn to this module.
//!
//! The probe launches through the host's own login-shell snapshot, which this
//! package reads from `licoup-agent-targets` — the same facility every other CLI
//! Agent launch stands on — rather than from a second copy of the login-shell
//! rules.

mod execution;
mod probe;

use licoup_agent_drivers::acp_session_transport::AcpSessionDriverSpec;

/// The runtime protocol identity Hermes' frames are recorded and reported under.
pub const RUNTIME_PROTOCOL: &str = "hermes-acp-stdio-jsonrpc";
/// Hermes' immutable session declaration, as the shared ACP transport reads it.
pub(crate) const HERMES_SESSION_DRIVER: AcpSessionDriverSpec =
    AcpSessionDriverSpec::new("hermes-acp", &["acp"]).with_runtime_id("hermes");

#[cfg(test)]
pub(crate) use execution::execute;
pub use execution::{cancel, cleanup_session, execute_with_connection};
pub use probe::probe;

#[cfg(test)]
mod tests;
