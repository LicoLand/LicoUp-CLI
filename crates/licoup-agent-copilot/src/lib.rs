//! The Copilot adapter package.
//!
//! One Agent, one package, one program. This crate owns everything LicoUp knows
//! about Copilot's own protocol and program: the ACP frames Copilot sends
//! ([`parser`]), Copilot's immutable ACP launch declaration ([`driver`]), the
//! frame dialect that projects the parser onto the shared ACP engine's port
//! ([`dialect`]), the registration composition injects into the adapter SDK
//! ([`registration`]), and the recorded-transcript replay arm that drives the
//! same parser ([`replay`]).
//!
//! # The boundaries this crate keeps
//!
//! - **No client crate.** Nothing here reaches into `licoup-native` or any other
//!   composition crate. What this package needs from the shared ACP engine
//!   arrives through `licoup-agent-drivers`' declared port, and composition
//!   installs this Agent's answer.
//! - **One parse.** A raw line becomes this Agent's effects in [`parser`] and is
//!   never re-parsed above: the parser is the sole ingress, per ADR-0008.
//! - **The shared engine stays shared.** The ACP reducer, the framing, the
//!   session semantics and the process runtime are not Copilot's protocol and do
//!   not live here. Copilot, Kimi Code and Hermes reach the same engine; what
//!   differs between them is exactly what this crate supplies.
//! - **No vendor branch above.** Composition reads [`registration::REGISTRATION`]
//!   and installs [`registration::DIALECT`]; neither names a frame rule of
//!   another Agent, and a host that composes no Copilot package links nothing of
//!   this crate.
//!
//! # Who executes Copilot
//!
//! The package ships Copilot's declaration, its protocol and the two bounded
//! entry points that run one turn: `licoup-native`'s composition names
//! [`driver`] directly and keeps no Copilot module of its own, so the launch,
//! the probe and the turn are described in exactly one place.
//!
//! The package declares no host port of its own and answers no admission
//! question, because it has no admission to ask for: the shared engine admits
//! and bounds the process, and which conversation is admitted, when an update
//! may replace a running package and where a turn's events go are the host's.
//! The extension host's binary route is what will move the process behind that
//! port.

pub mod dialect;
pub mod driver;
pub mod parser;
pub mod registration;

/// The recorded-transcript replay arm.
///
/// It drives the shared ACP reducer, so it is compiled only where that reducer's
/// replay surface is: this crate's own tests and a composing test build that
/// asks for `test-support`. A production build carries no arm.
#[cfg(any(test, feature = "test-support"))]
pub mod replay;

#[cfg(test)]
mod tests;
