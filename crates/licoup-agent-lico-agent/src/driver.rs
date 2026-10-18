//! This Agent's own half of one turn.
//!
//! What LicoUp knows about running Lico Agent is the packaged program's stdio
//! RPC protocol: the `--mode rpc` launch, the `lf-jsonl-jsonrpc` frames
//! [`crate::parser`] classifies, the session [`crate::session`] resolves, the
//! harnessed stdio exchange that carries a turn, the persisted-transcript resume
//! rule and the failure vocabulary it reports.
//!
//! - [`execution`] performs one turn: spawn the packaged program, supervise it,
//!   hand it the readiness handshake and the prompt, and read its effects.
//! - [`sandbox`] is the sealed invocation a Plan turn runs under.
//! - [`probe`] is the capability probe the host runs before offering the Agent.
//! - [`model`] and [`errors`] are this Agent's own result and failure shapes.
//!
//! What the client owns is reached through the crates that own it rather than
//! reimplemented here: the process supervision, the workspace bound and the raw
//! byte record from `licoup-foundation`, the login-shell environment and the
//! Agent core's profile and transcript vocabulary from `licoup-agent-targets`,
//! and the platform sandbox primitive through [`crate::port::sandbox`].

mod errors;
mod execution;
mod model;
mod probe;
mod sandbox;

pub use errors::{ProtocolFailure, ProtocolFailurePayload};
pub use execution::execute;
pub use model::{CapabilityProbe, EffectiveSettings, RUNTIME_PROTOCOL, RunResult};
pub use probe::probe;
pub use sandbox::{PLAN_SANDBOX_CAPABILITY, PlanSandboxFailure, plan_command};

#[cfg(test)]
mod tests;
