//! The agent-execution port this package needs its host to answer.
//!
//! The client owns dispatch: which conversation is admitted, which turn is
//! cancelled, when an update may replace a running package. This package owns
//! what one OpenClaw execution *is*: the Gateway endpoint it attaches to, the
//! ACP frames it writes next, how it classifies them, and the outcome it
//! reports.
//!
//! The port is declared here, on the package side, because the package is what
//! has to name the seam. It maps onto the agent-execution profile of
//! `licoup-extension-contracts` — execute, describe, resume/observe, cancel,
//! steer, history and models — and it is a seam rather than an implementation:
//! the host supplies the other side through the extension host that starts this
//! package's binary, and the package never reaches into a client crate to find
//! it.
//!
//! Until that side exists the seam is *declared and fail-closed*: a host that
//! installs nothing gets a refusal rather than an invented admission, and the
//! package's own protocol, parser and replay remain fully exercised without a
//! host at all.
//!
//! # What this port does not yet carry
//!
//! The client drives this package in-process, so the turn itself is
//! [`crate::driver`]'s and the Gateway lifecycle, the event emission and the MCP
//! registration are answered at that call site; the port is not the entry to
//! that path yet. What completes it is the package's binary route: the extension
//! host that starts this package's program, admits one execution through this
//! answer, and reaches the same [`crate::driver`] entry points. Until then this
//! answer stays fail-closed, and the package claims no execution it does not
//! perform.

use std::sync::OnceLock;

/// The host facilities one OpenClaw execution asks for.
///
/// Each member is one answer the host owns. They arrive as one installed value
/// so a package cannot be half-wired: either the host answered the port or the
/// package is fail-closed as a whole.
#[derive(Clone, Copy)]
pub struct ExecutionPort {
    /// Whether the host currently admits a new execution on this host.
    ///
    /// The idle-update admission decision belongs to the host: it is the same
    /// barrier a package generation must hold before it may replace a running
    /// package. This package never keeps an admission of its own and cannot
    /// bypass that barrier — a package that cannot ask the host never claims it
    /// was admitted.
    pub admits_execution: fn() -> bool,
}

static PORT: OnceLock<ExecutionPort> = OnceLock::new();

/// Install the host's execution answers once per process.
pub fn install(port: ExecutionPort) -> Result<(), &'static str> {
    PORT.set(port)
        .map_err(|_| "the agent-execution port is already installed")
}

/// Whether the host has installed its execution answers.
pub fn installed() -> bool {
    PORT.get().is_some()
}

/// One effect the package asks the host to perform.
///
/// A refusal is a value rather than a panic: a package running without its host
/// reports that it could not ask, and the caller decides what that means. It
/// never invents the effect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostEffect {
    /// The host answered.
    Answered,
    /// No host installed the port.
    Uninstalled,
}

/// Whether the host admits a new execution. Fail-closed when uninstalled: a
/// package that cannot ask the host's admission never claims it was admitted.
pub fn admits_execution() -> bool {
    PORT.get().is_some_and(|port| (port.admits_execution)())
}
