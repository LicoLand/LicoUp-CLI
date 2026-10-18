//! The Claude Code driver's process half.
//!
//! The protocol vocabulary this driver speaks — the `lf-ndjson` framing, the
//! launch identity and its argv, the effective settings the CLI reports, the
//! failure shape, the control request, and the byte-line parser that classifies
//! one frame — is [`crate::protocol`], beside this module in the same package,
//! and this driver reads it at its own path rather than through a re-export.
//!
//! What this module composes is the *process* half: supervising the
//! streaming-input CLI, binding the conversation, reading its framed lines,
//! parking an approval and answering a cancel. It is one package's process
//! because it is one Agent's process: composition above names [`execute`],
//! [`probe`] and the four bounded control entries, and holds no launch field,
//! no frame dialect and no turn phase of its own.
//!
//! The Subagent MCP caller registration is the other half still named by the
//! host: it is composed from the host's own provider-config registry, which the
//! host owns for every provider at once.

// The entries the host's composition and its lane read: the runtime protocol
// identity, the probe, the turn, and the bounded control surface.
pub use control::ControlDisposition;
pub use execution::execute;
pub use model::{RUNTIME_PROTOCOL, RunResult};
pub use probe::probe;
pub use supervision::{cancel, cleanup_session, history, steer};

// The process half's own leaves. Nothing outside this module reads them: the
// package's suite lives under `tests/` below and reaches them as a descendant.
mod approval;
mod control;
mod execution;
mod failure;
mod io;
mod launch;
mod model;
mod probe;
mod reset;
mod supervision;
mod transport;

#[cfg(test)]
mod tests;
