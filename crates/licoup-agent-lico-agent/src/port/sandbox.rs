//! The platform's OS sandbox primitive, as one installed port.
//!
//! Lico Agent's Plan profile runs the packaged program under a platform sandbox
//! that limits filesystem writes to one literal plan file and network egress to
//! the loopback Gateway port. *Which* paths that profile binds, how the runner
//! is invoked and that it must fail closed are this Agent's facts and live in
//! [`crate::driver::sandbox`].
//!
//! What is not this Agent's is the platform's own primitive: whether this
//! machine has a trustworthy sandbox runner at all, how one absolute path is
//! escaped into a profile literal, and how the runner is invoked with a sealed
//! profile. Those are the same questions every sandboxed LicoUp child asks, so
//! they belong to the client's shared `process_sandbox` and arrive here.
//!
//! Until a host installs the port every member is fail-closed: no literal, no
//! command and therefore no Plan turn, because Plan mode without a reliable
//! sandbox is not a Lico Agent turn at all.

use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

/// Why the platform's sandbox primitive refused one request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SandboxPrimitiveFailure {
    /// This platform has no trustworthy sandbox runner.
    Unavailable,
    /// The path cannot be bound by a sandbox profile literal.
    PathInvalid,
}

/// The host facilities one sandboxed Lico Agent Plan turn needs.
#[derive(Clone, Copy)]
pub struct SandboxPort {
    /// Escape one absolute path for this platform's sandbox profile literal.
    pub profile_literal: fn(&Path) -> Result<String, SandboxPrimitiveFailure>,
    /// The platform's sandboxed invocation of one runner under one sealed
    /// profile, or the reason this platform cannot sandbox at all.
    ///
    /// The profile is the package's; the runner, its own path and the way a
    /// profile is handed to it are the platform's.
    pub sandboxed_command: fn(
        profile: &str,
        runner: &Path,
        extra_args: &[String],
    ) -> Result<Command, SandboxPrimitiveFailure>,
}

static PORT: OnceLock<SandboxPort> = OnceLock::new();

/// Install the host's sandbox primitive once per process.
pub fn install(port: SandboxPort) -> Result<(), &'static str> {
    PORT.set(port)
        .map_err(|_| "the sandbox port is already installed")
}

/// Whether the host has installed its sandbox primitive.
pub fn installed() -> bool {
    PORT.get().is_some()
}

/// Escape one absolute path, or report that no host can.
pub(crate) fn profile_literal(path: &Path) -> Result<String, SandboxPrimitiveFailure> {
    match PORT.get() {
        Some(port) => (port.profile_literal)(path),
        // Fail-closed: a package that cannot ask for a literal cannot build a
        // profile, so Plan mode reports the sandbox unavailable.
        None => Err(SandboxPrimitiveFailure::Unavailable),
    }
}

/// The platform's sandboxed invocation, or the reason there is none.
pub(crate) fn sandboxed_command(
    profile: &str,
    runner: &Path,
    extra_args: &[String],
) -> Result<Command, SandboxPrimitiveFailure> {
    match PORT.get() {
        Some(port) => (port.sandboxed_command)(profile, runner, extra_args),
        None => Err(SandboxPrimitiveFailure::Unavailable),
    }
}
