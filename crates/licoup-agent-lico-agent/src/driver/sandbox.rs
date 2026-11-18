//! The sandboxed invocation one Lico Agent Plan turn runs under.
//!
//! Plan mode is not "base mode with a promise": the packaged program runs under
//! the platform's sandbox with a profile this module owns, so the Agent's own
//! plan isolation is a fact of the invocation rather than a claim about it. The
//! profile binds exactly three paths — the runner, the one literal plan file and
//! the workspace — allows writes to the plan file only, and allows outbound
//! network to the loopback Gateway port only.
//!
//! *Which* paths those are, that the plan path must be absolute and that a turn
//! without a reliable sandbox must fail are this Agent's facts. How one path
//! becomes a profile literal, and how the platform's runner is invoked with a
//! sealed profile, arrive through [`crate::port::sandbox`]; a host that
//! installed nothing leaves Plan mode failing closed rather than unsandboxed.

use crate::port::sandbox::{self, SandboxPrimitiveFailure};
use std::path::Path;
use std::process::Command;

/// The capability token this profile publishes.
///
/// It names the isolation a Plan turn actually runs under, so a reader can tell
/// a sandboxed Plan turn from a refused one without inspecting the profile.
pub const PLAN_SANDBOX_CAPABILITY: &str = "platform-lico-agent-plan-isolated-v1";

/// Why one Plan turn could not be sandboxed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanSandboxFailure {
    /// This platform cannot enforce the profile at all.
    Unavailable,
    /// One of the paths the profile binds is not an absolute path.
    PathInvalid,
}

fn failure(reason: SandboxPrimitiveFailure) -> PlanSandboxFailure {
    match reason {
        SandboxPrimitiveFailure::Unavailable => PlanSandboxFailure::Unavailable,
        SandboxPrimitiveFailure::PathInvalid => PlanSandboxFailure::PathInvalid,
    }
}

/// The sealed invocation of one Plan turn.
///
/// `runner` is the packaged program, `plan_file` the one file it may write and
/// `workspace` the one tree it may read; `extra_args` is the Agent's own argv.
pub fn plan_command(
    runner: &Path,
    plan_file: &Path,
    workspace: &Path,
    gateway_port: u16,
    extra_args: &[String],
) -> Result<Command, PlanSandboxFailure> {
    let runner_literal = sandbox::profile_literal(runner).map_err(failure)?;
    let plan_literal = sandbox::profile_literal(plan_file).map_err(failure)?;
    let workspace_literal = sandbox::profile_literal(workspace).map_err(failure)?;
    if !(plan_file.is_absolute() && workspace.is_absolute()) {
        return Err(PlanSandboxFailure::PathInvalid);
    }
    let profile = format!(
        concat!(
            "(version 1)",
            "(deny default)",
            "(import \"system.sb\")",
            "(allow process-exec (literal \"{runner}\"))",
            "(allow signal (target self))",
            "(allow file-read* file-test-existence ",
            "(literal \"{runner}\") (literal \"{plan}\") (subpath \"{workspace}\"))",
            "(allow file-write* (literal \"{plan}\"))",
            "(allow network-outbound (remote tcp \"localhost:{port}\"))"
        ),
        runner = runner_literal,
        plan = plan_literal,
        workspace = workspace_literal,
        port = gateway_port,
    );
    sandbox::sandboxed_command(&profile, runner, extra_args).map_err(failure)
}
