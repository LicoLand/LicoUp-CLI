//! The Antigravity driver: the vendor CLI launch, its supervision and the
//! outcome one turn produced.
//!
//! Ownership boundary: this module owns the send/resume/cancel/cleanup lane and
//! the Lico-namespaced Gemini hook bridge. Unrelated product features must not
//! hardcode Antigravity transport details; swap or detach by replacing this
//! module plus its inventory/manifest/gate config entries.
//!
//! Everything the Antigravity CLI *is* — the runtime protocol identity, the argv
//! its one print-mode entry point takes, the effective settings it reports, the
//! receipt its Stop hook writes, the authorization gate and the terminal
//! classification that decides what a turn produced — is declared here, beside
//! the parser that reads the same protocol, so one vendor fact has one owner.
//!
//! What this module asks its host for is exactly two things, and both arrive as
//! values the composition installs rather than as a client crate this package
//! could reach: the progressive turn-event sink ([`crate::port::turn_event`],
//! because the host owns the consumer) and the execution-admission answer
//! ([`crate::port::execution`], because the host owns whether a new execution is
//! admitted at all). Everything else a launch applies is a rule below every
//! Agent and is read from its owner rather than retyped here: the bounded process
//! owner, the PTY transport, the raw execution observer and the workspace rule
//! from `licoup-foundation`, the login-shell snapshot from the Agent inventory,
//! and the Subagent caller context from the mesh that binds it.

mod control;
mod errors;
mod execution;
mod hooks;
mod model;
mod probe;

mod auth;
pub use auth::authorize;
pub use control::{ControlDisposition, cancel, cleanup_session};
pub use errors::{ProtocolFailure, ProtocolFailurePayload};
pub use execution::execute;
// The hook bridge's installation entries belong to the adapter's lifecycle
// commands; `ensure_hook_bridge` and `uninstall_hook_bridge` are the driver's own
// two steps and the package suite's, so they stay crate-visible.
#[cfg(test)]
pub(crate) use hooks::{ensure_hook_bridge, uninstall_hook_bridge};
pub use hooks::{hook_bridge_status, install_hook_bridge, uninstall_hook_bridge_report};
pub use model::{CapabilityProbe, DRIVER_ID, EffectiveSettings, RUNTIME_PROTOCOL, RunResult};
pub use probe::probe;

#[cfg(test)]
mod tests;
