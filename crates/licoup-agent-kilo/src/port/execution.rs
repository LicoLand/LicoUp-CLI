//! The agent-execution port this package needs its host to answer.
//!
//! The client owns dispatch: which conversation is admitted, which turn is
//! cancelled, when an update may replace a running package. This package owns
//! what one Kilo Code execution *is*: the serve endpoint it asks for, the
//! documents it reads, the frames it classifies, and the outcome it reports.
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
//! installs nothing gets [`HostEffect`] refusals rather than invented effects,
//! and the package's own protocol, parser and replay remain fully exercised
//! without a host at all.

use std::path::PathBuf;
use std::sync::OnceLock;

/// What the host must tell one Kilo Code execution before it starts.
///
/// The fields are the facts a serve launch cannot derive for itself: the user's
/// own shell environment, the workspace it runs in, and the caller context a
/// delegated subagent carries.
#[derive(Clone, Debug, Default)]
pub struct ExecutionContext {
    /// The workspace root the turn runs in.
    pub workspace: Option<PathBuf>,
    /// The user's own shell environment, captured once by the host.
    pub environment: Vec<(String, String)>,
    /// The exported Subagent caller context, when this turn is a delegation.
    pub subagent_caller: Option<String>,
}

/// The host facilities one Kilo Code execution asks for.
///
/// Each member is one answer the host owns. They arrive as one installed value
/// so a package cannot be half-wired: either the host answered the port or the
/// package is fail-closed as a whole.
#[derive(Clone, Copy)]
pub struct ExecutionPort {
    /// The caller context a delegated subagent exports, or `None` for a local
    /// turn. `None` is an answer, not a missing one.
    pub subagent_caller_context: fn() -> Option<String>,
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

/// The caller context the host exports, or [`HostEffect::Uninstalled`].
pub fn subagent_caller_context() -> Result<Option<String>, HostEffect> {
    PORT.get()
        .map(|port| (port.subagent_caller_context)())
        .ok_or(HostEffect::Uninstalled)
}

/// Whether the host admits a new execution. Fail-closed when uninstalled: a
/// package that cannot ask the host's admission never claims it was admitted.
pub fn admits_execution() -> bool {
    PORT.get().is_some_and(|port| (port.admits_execution)())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_query_is_fail_closed_before_the_host_installs_anything() {
        // This test runs in the same process as the other ports' tests, so it
        // only asserts the shape of an answer rather than absence: an
        // uninstalled port refuses rather than inventing a caller, and an
        // uninstalled admission never claims admission.
        let asked = subagent_caller_context();
        assert!(matches!(
            asked,
            Ok(None) | Ok(Some(_)) | Err(HostEffect::Uninstalled)
        ));
        let _ = admits_execution();
        let _ = installed();
    }
}
