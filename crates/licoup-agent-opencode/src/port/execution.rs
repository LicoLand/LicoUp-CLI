//! The agent-execution port this package needs its host to answer.
//!
//! The client owns dispatch: which conversation is admitted, which turn is
//! cancelled, when an update may replace a running package. This package owns
//! what one OpenCode execution *is*: the `opencode serve` protocol it reads, the
//! documents it classifies, and the outcome it reports.
//!
//! The port is declared here, on the package side, because the package is what
//! has to name the seam. Most of the profile it maps onto —
//! `licoup-extension-contracts`' agent-execution profile: execute, describe,
//! resume/observe, cancel, steer, history and models — is this package's own
//! driver over the shared local-service engine, reached through
//! [`crate::port::serve`], so the one fact the package genuinely cannot derive is
//! the host's admission decision. That is the member stated below, and it is the
//! reason this seam exists rather than being an empty declaration.
//!
//! The driver asks the question through this port before it starts a turn: the
//! admission answer is the host's close-admission barrier, and a turn launched
//! while a maintenance switch holds that barrier would run an Agent the switch
//! may be replacing. A package that never installed the port is refused rather
//! than admitted, so the protocol and the replay corpus stay fully exercisable
//! with no host at all.

use std::sync::OnceLock;

/// The host facilities one OpenCode execution asks for.
///
/// The member is one answer the host owns. It arrives as one installed value so
/// a package cannot be half-wired: either the host answered the port or the
/// package is fail-closed as a whole.
#[derive(Clone, Copy)]
pub struct ExecutionPort {
    /// Whether the host currently admits a new execution on this process. The
    /// idle-admission decision belongs to the host; this package cannot bypass
    /// it, and a package that never installed the port is not admitted.
    pub admits_execution: fn() -> bool,
}

static PORT: OnceLock<ExecutionPort> = OnceLock::new();

/// Install the host's execution answer once per process.
///
/// A second installation is refused rather than silently replacing the first:
/// the admission decision belongs to one host, and a second answer would mean
/// two admission paths for one turn.
pub fn install(port: ExecutionPort) -> Result<(), &'static str> {
    PORT.set(port)
        .map_err(|_| "the agent-execution port is already installed")
}

/// Whether the host has installed its execution answer.
pub fn installed() -> bool {
    PORT.get().is_some()
}

/// Whether the host admits a new execution. Fail-closed when uninstalled: a
/// package that cannot ask the host's admission never claims it was admitted.
pub fn admits_execution() -> bool {
    PORT.get().is_some_and(|port| (port.admits_execution)())
}
