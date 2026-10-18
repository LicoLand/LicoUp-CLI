//! This Agent's own half of one turn.
//!
//! The client's serve engine owns the sockets, the process and the active-turn
//! registry. What the engine cannot know is what *Kilo Code* needs: the request
//! shape it accepts, the identity document it answers with, how a completed
//! message becomes this Agent's transitions, and what its endpoint must report
//! before a turn may start. That is what this module tree owns.
//!
//! - [`config`] reads one turn's request out of the host's parameters.
//! - [`turn`] performs the turn against the engine's port: bind a session, post
//!   the message, project the stream's chunks and the terminal document.
//! - [`projection`] is that projection and this Agent's capability answer.
//! - [`probe`] is the capability probe the host runs before offering the Agent.
//!
//! Nothing here reaches a socket or starts a process: every engine operation
//! arrives through [`crate::port::serve`], so a package running without its host
//! reports that it could not ask rather than performing the effect itself.
//!
//! The two bounded entry points the host composes are here as well
//! ([`execute`] and [`capability_probe`]), because what one Kilo turn *is* and
//! how its endpoint is offered are this Agent's contract; what the host owns is
//! the engine underneath them and its own result vocabulary, which these
//! translate onto.

use licoup_agent_drivers::acp_driver_runtime::AcpDriverSpec;

pub mod config;
pub mod probe;
pub mod projection;
pub mod turn;

#[cfg(test)]
mod tests;

pub use config::{ServeTurnConfig, timestamp};
pub use probe::{EndpointProbe, capability_probe, probe_endpoint};
// The Agent's own material, re-exported at the names its client used to read it
// by, so a composition that moves in slices names one path throughout. A moved
// spelling is not a second copy: these are the same functions.
pub use licoup_agent_adapter_sdk::{LifecycleStage, Transition};
pub use probe as protocol;
pub use projection::{CapabilityProbe, EffectiveSettings, ProtocolOutcome, serve_capabilities};
pub use turn::{build_message_body, execute, execute_via_serve};

/// This Agent's failure type: a closed code, a fixed message and the stage it
/// was observed at.
///
/// The shape is the shared driver vocabulary — the host's normalized result
/// carries the same four facts — but the *values* are this Agent's. A package
/// that could not ask its host reports a failure here rather than a panic, and
/// the host maps the code onto its own error taxonomy where one exists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtocolFailure {
    pub code: String,
    pub message: &'static str,
    pub stage: &'static str,
    pub session_id: Option<String>,
    pub thread_id: Option<String>,
}

impl ProtocolFailure {
    /// A failure with no identity bound yet.
    pub fn new(code: &'static str, message: &'static str, stage: &'static str) -> Self {
        Self {
            code: code.to_owned(),
            message,
            stage,
            session_id: None,
            thread_id: None,
        }
    }

    /// The same failure, bound to the native session it was observed on.
    pub fn with_session(mut self, session_id: Option<&str>) -> Self {
        self.session_id = session_id.map(str::to_owned);
        self.thread_id = session_id.map(str::to_owned);
        self
    }
}

/// The name this Agent's runtime protocol reports.
pub const RUNTIME_PROTOCOL: &str = "kilo-code-serve-http-v1";

/// The driver identity this Agent's results are stamped with.
pub const DRIVER_ID: &str = "kilo-code-serve";

/// The error prefix this Agent's shared ACP-shaped codes carry.
pub const ERROR_PREFIX: &str = "kilo_code_serve";

/// This Agent's immutable launch declaration, as the shared engine reads it.
///
/// The declaration is metadata only: a Kilo turn is performed over this
/// package's own `serve` HTTP/SSE documents rather than over a shared ACP
/// stdio transport, so the launch names the subcommand that starts the
/// endpoint and nothing about a turn.
pub const DRIVER: AcpDriverSpec =
    AcpDriverSpec::new(RUNTIME_PROTOCOL, &["serve"]).with_identity(DRIVER_ID, ERROR_PREFIX);
