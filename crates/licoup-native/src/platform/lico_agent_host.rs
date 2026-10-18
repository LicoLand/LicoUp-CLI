//! The client's answer for the Lico Agent adapter package's sandbox port.
//!
//! The package owns what a Lico Agent Plan turn *is*: which paths the profile
//! binds, that writes reach exactly one literal plan file, and that a turn
//! without a reliable sandbox fails. This module owns the platform half — that
//! this machine has a trustworthy sandbox runner, how one absolute path becomes
//! a profile literal, and how a runner is invoked under a sealed profile — and
//! states it in the package's vocabulary.
//!
//! Both answers are the shared [`super::process_sandbox`] primitive's, so the
//! Plan profile cannot drift from the isolation every other sandboxed LicoUp
//! child runs under, and a host that installs nothing leaves Plan mode failing
//! closed rather than unsandboxed.

use std::path::Path;
use std::process::Command;

use licoup_agent_lico_agent::port::sandbox::{SandboxPort, SandboxPrimitiveFailure};

use super::process_sandbox::{self, SandboxError};

/// The engine's sandbox failure, named in the package's own closed vocabulary.
fn primitive_failure(failure: SandboxError) -> SandboxPrimitiveFailure {
    match failure {
        SandboxError::Unavailable => SandboxPrimitiveFailure::Unavailable,
        SandboxError::PathInvalid => SandboxPrimitiveFailure::PathInvalid,
    }
}

fn profile_literal(path: &Path) -> Result<String, SandboxPrimitiveFailure> {
    process_sandbox::seatbelt_literal(path).map_err(primitive_failure)
}

fn sandboxed_command(
    profile: &str,
    runner: &Path,
    extra_args: &[String],
) -> Result<Command, SandboxPrimitiveFailure> {
    process_sandbox::sandboxed_command(profile, runner, extra_args).map_err(primitive_failure)
}

/// This client's answer for the package's sandbox port.
pub(crate) fn sandbox_port() -> SandboxPort {
    SandboxPort {
        profile_literal,
        sandboxed_command,
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests;
