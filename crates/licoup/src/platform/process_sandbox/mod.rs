//! Shared macOS seatbelt process isolation for LicoUp-owned children.
//!
//! The primitive is generic: it answers whether this platform can sandbox at
//! all, how one absolute path becomes a profile literal, and how one runner is
//! invoked under one sealed profile. Which paths a profile binds is the Agent
//! contract that asked for it — Lico Agent's Plan profile lives in
//! `licoup-agent-lico-agent` and reaches this primitive through the client's
//! answer for its sandbox port.

mod seatbelt;
mod strategy;

pub use seatbelt::{
    CAPABILITY_COLLABORATION_LOOPBACK, SandboxError, collaboration_loopback_command,
    sandboxed_command, seatbelt_literal,
};
pub(crate) use strategy::strategy_script_command;
// The probe that decides whether a seatbelt profile can be applied at all is
// shared so the tests of one Agent's profile and the primitive's own test ask
// the same question of this machine.
#[cfg(all(test, target_os = "macos"))]
pub(crate) use seatbelt::sandbox_exec_can_apply;
