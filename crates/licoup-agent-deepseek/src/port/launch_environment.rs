//! The environment one Harness launch observes.
//!
//! Architecture invariant 4 (ADR 0007): a CLI Agent spawned by LicoUp must
//! observe exactly the environment the user gets when they start the same CLI
//! from their own terminal login shell. That snapshot is the host's — it asks the
//! user's login shell once per process, and the answer is the host's to keep —
//! while the launch is this package's. The package therefore asks its host for
//! the one fact it cannot derive instead of reading a second copy of the
//! login-shell rules.
//!
//! The port is one installed function rather than a map of variables: the
//! package applies the host's answer to a command and never reads it, so no
//! environment value crosses this seam as data this package could inspect or
//! persist. Before installation the port is fail-closed in the only sense a
//! spawn has — the child keeps the environment this process inherited, and the
//! package invents no snapshot it did not observe.

use std::process::Command;
use std::sync::OnceLock;

/// The host's answer for the environment one launch observes.
///
/// It is a function over the command rather than a snapshot because the snapshot
/// stays the host's: the package hands the command over and takes it back
/// configured.
pub type LaunchEnvironment = fn(&mut Command);

static PORT: OnceLock<LaunchEnvironment> = OnceLock::new();

/// Install the host's launch environment once per process.
///
/// A second installation is refused rather than silently replacing the first:
/// one process has one login shell, and a second answer would mean two
/// environments for one Agent.
pub fn install(port: LaunchEnvironment) -> Result<(), &'static str> {
    PORT.set(port)
        .map_err(|_| "the launch-environment port is already installed")
}

/// Whether the host has installed its launch environment.
pub fn installed() -> bool {
    PORT.get().is_some()
}

/// Apply the host's launch environment to one command.
///
/// Fail-closed before installation: the command keeps the environment this
/// process inherited, and this package fabricates no login-shell snapshot.
pub fn apply_to_command(command: &mut Command) {
    if let Some(port) = PORT.get() {
        (port)(command);
    }
}
