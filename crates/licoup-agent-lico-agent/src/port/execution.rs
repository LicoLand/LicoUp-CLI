//! The agent-execution port this package needs its host to answer.
//!
//! The client owns dispatch: which conversation is admitted, which turn is
//! cancelled, when an update may replace a running package. This package owns
//! what one Lico Agent execution *is*: the packaged program it launches with
//! `--mode rpc`, the `lf-jsonl-jsonrpc` stream it reads, the frames
//! [`crate::parser`] classifies, the session [`crate::session`] resolves, and
//! the outcome it reports.
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
//! package's own protocol, session layout and replay corpus remain fully
//! exercised without a host at all.
//!
//! The process half of a Lico Agent turn is this package's [`crate::driver`]
//! now, and it reaches the platform through [`crate::port::sandbox`]. What
//! remains for this seam is the extension host that answers it when it starts
//! the package's binary; the package declares it now so that route is one
//! installation rather than a new contract.

use std::sync::OnceLock;

/// The host facilities one Lico Agent execution asks for.
///
/// Each member is one answer the host owns. They arrive as one installed value
/// so a package cannot be half-wired: either the host answered the port or the
/// package is fail-closed as a whole.
#[derive(Clone, Copy)]
pub struct ExecutionPort {
    /// Whether the host currently admits a new execution on this host. The idle
    /// admission decision belongs to the host; this package cannot bypass it.
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

/// Whether the host admits a new execution. Fail-closed when uninstalled: a
/// package that cannot ask the host's admission never claims it was admitted.
pub fn admits_execution() -> bool {
    PORT.get().is_some_and(|port| (port.admits_execution)())
}
