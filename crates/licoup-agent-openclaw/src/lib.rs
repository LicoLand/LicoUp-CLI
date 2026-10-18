//! The OpenClaw adapter package.
//!
//! One Agent, one package, one program. This crate owns everything LicoUp knows
//! about OpenClaw's own protocol and program: the Gateway ACP state machine
//! ([`parser`]), the byte-line codec that classifies one Gateway frame exactly
//! once below the adapter port, the driver vocabulary those frames are reported
//! through ([`gateway_acp`]), the endpoint an ACP attach names ([`gateway`]) and
//! the endpoint policy behind it ([`policy`]), the whole process half of one turn
//! ([`driver`]), the registration composition injects into the adapter SDK
//! ([`registration`]), the recorded-transcript replay arm ([`replay`]), and the
//! ports the host answers ([`port`]).
//!
//! # The boundaries this crate keeps
//!
//! - **No client crate.** Nothing here reaches into `licoup-native` or any other
//!   composition crate. What the package needs from its host arrives through
//!   [`port`] or as an explicit argument to the one call that needs it, and the
//!   host installs or supplies it.
//! - **One parse.** A raw Gateway line becomes this Agent's effects here and is
//!   never re-parsed above: [`parser::protocol`] is the sole ingress, per
//!   ADR-0008.
//! - **The client owns the turn.** The state machine reports protocol finishes,
//!   end of stream and confirmed cancellation; it settles no turn, imposes no
//!   implicit timeout, and hides no content. The conversation layer remains the
//!   sole turn authority.
//! - **Fail closed without a host.** An uninstalled port emits nothing, admits
//!   nothing and ensures no Gateway rather than guessing, so the protocol, the
//!   explicit-endpoint attach and the replay corpus are fully exercised with no
//!   host at all.
//!
//! # What this crate does not own yet
//!
//! The client still drives this package in-process: composition calls
//! [`driver::execute_with_connection`] for an OpenClaw turn and answers the
//! Gateway lifecycle, the turn-event emission and the MCP registration at that
//! call site. What is not yet completed is the package's *binary* route — the
//! extension host that starts this package's program and reaches the same turn
//! through the agent-execution port, which stays declared and fail-closed until
//! it exists.

pub mod driver;
pub mod gateway;
pub mod gateway_acp;
pub mod parser;
pub mod policy;
pub mod port;
pub mod registration;
pub mod replay;

#[cfg(test)]
mod tests;
