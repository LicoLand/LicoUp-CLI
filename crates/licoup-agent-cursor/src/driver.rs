//! The Cursor process half: one `cursor-agent` turn.
//!
//! This is the program that runs a Cursor turn: it resolves the bounded
//! workspace, creates or resumes the native chat, launches `cursor-agent` with
//! the package's fixed arguments on the shared pty transport, reads the strict
//! NDJSON stream the package's parser classifies, watches the CLI's own
//! auto-update signals, supervises the child, and reports the outcome the
//! client's normalization reads.
//!
//! What is Cursor's, and therefore here: the launch ([`crate::model`]), the
//! stream dialect ([`crate::parser`]), the session-identity rule the active-turn
//! registry is keyed by, the local chat storage `cleanup_session` retires and
//! the update phases this driver surfaces. What is not: the conversation lane
//! that admits a turn and the consumer its events reach
//! ([`crate::port::turn_event`]). Those are the host's, and they arrive through
//! the package's port; the terminal the child is launched on is neither half's
//! private mechanism — it is the process primitive in `licoup-foundation` both
//! halves link. A package whose host answers no port reports that it could not
//! start the turn instead of inventing an effect.
//!
//! [`execute`] is the one turn entry point and [`probe`] answers whether the
//! installed executable is the lane this package declares. The result type and
//! the protocol identity are the package's wire vocabulary
//! ([`crate::model`]), read here where the process reports them.

mod control;
mod execution;
mod io;
mod probe;
mod update_watcher;

pub use crate::model::{DRIVER_ID, RUNTIME_PROTOCOL, RunResult};
pub use control::{ControlDisposition, cancel, cleanup_session};
pub use execution::execute;
pub use probe::probe;

#[cfg(test)]
mod tests;
