//! The agent-execution port this package needs its host to answer.
//!
//! The client owns dispatch: which conversation is admitted, which turn is
//! cancelled, when an update may replace a running package. This package owns
//! what one Kimi Code execution *is*: the `kimi acp` launch, the frames it
//! classifies, and the outcome it reports.
//!
//! The port is declared here, on the package side, because the package is what
//! has to name the seam. Most of the profile it maps onto —
//! `licoup-extension-contracts`' agent-execution profile: execute, describe,
//! resume/observe, cancel, steer, history and models — is answered by the shared
//! ACP engine in `licoup-agent-drivers` plus this package's own launch metadata,
//! so the one fact the package genuinely cannot derive is the host's admission
//! decision. That is the member stated below, and it is the reason this seam
//! exists at all rather than being an empty declaration.
//!
//! The host supplies the other side through the extension host that starts this
//! package's binary, and the package never reaches into a client crate to find
//! it. Until that side exists the seam is *declared and fail-closed*: a host
//! that installs nothing gets a refusal rather than an invented effect, and the
//! package's dialect, parser and replay remain fully exercised without a host.

use std::sync::OnceLock;

/// The host facilities one Kimi Code execution asks for.
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
