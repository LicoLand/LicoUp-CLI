//! The process half of the Codex driver: the app-server this package runs.
//!
//! The wire half above ([`super::config`], [`super::model`], [`super::limits`],
//! [`super::reserve`]) states what one Codex turn says. This half states what
//! one Codex turn *is* for the operating system: the program that is started,
//! the JSON-RPC line that is written next, the frames that are read back under a
//! bound, the turn that is supervised to its own terminal event, the control
//! requests that reach a live turn, and the read-only model directory probe.
//!
//! It lives in the package rather than in the client because it is Codex's
//! process, not the client's: the app-server is the installed vendor program,
//! its stdio framing is the vendor's, and a client that carries no Codex package
//! starts no app-server. The client reaches one turn through
//! [`execute`], and an installed package binary serves the same turn through the
//! execution verbs the extension host starts it with.
//!
//! Nothing here names a client crate: the bounded process owner, the raw-bytes
//! observer and the data-root selection arrive from `licoup-foundation`, and the
//! progressive events a turn produces go out through this package's own
//! [`crate::port::turn_event`] seam.

mod io;
mod launch;
mod supervision;
mod transport;

pub mod active_control;
pub mod model_catalog;

#[cfg(test)]
mod tests;

pub use launch::{CodexLaunchSpec, apply_launch_environment, apply_launch_environment_with_root};
pub use model_catalog::list_models;
pub use transport::execute;

// The vocabulary the transport and the supervision loop read at their former
// path, so a caller that already names this package's protocol keeps reading the
// same names.
pub use super::config::ProtocolConfig;
pub use super::model::{EffectiveSettings, ProtocolEffect, ProtocolFailure, RunResult};
