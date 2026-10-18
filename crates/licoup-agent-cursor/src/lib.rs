//! The Cursor adapter package.
//!
//! One Agent, one package, one program. This crate owns everything LicoUp knows
//! about Cursor's own protocol and program: the strict-NDJSON turn dialect of
//! the Cursor Agent CLI ([`parser`]), classified exactly once below the adapter
//! port, the wire vocabulary that dialect reads and reports ([`model`],
//! [`errors`]), the process half that runs one turn ([`driver`]), the
//! registration composition injects into the adapter SDK ([`registration`]), the
//! recorded-transcript replay arm ([`replay`]), and the ports the host answers
//! ([`port`]).
//!
//! # The boundaries this crate keeps
//!
//! - **No client crate.** Nothing here reaches into `licoup-native` or any other
//!   composition crate. What the package needs from its host arrives through
//!   [`port`], and the host installs it.
//! - **One parse.** A raw strict-NDJSON line becomes this Agent's effects here
//!   and is never re-parsed above: the parser is the sole ingress, per ADR-0008.
//! - **The client owns the turn.** The parser reports delivery acknowledgement,
//!   streamed text, structured tool calls, application tool errors and protocol
//!   finishes; it settles no turn, imposes no implicit timeout, and hides no
//!   content. The conversation layer remains the sole turn authority.
//! - **Fail closed without a host.** An uninstalled port emits nothing, admits
//!   nothing and starts no process rather than guessing, so the protocol and the
//!   replay corpus are fully exercised with no host at all.
//!
//! # Who executes Cursor
//!
//! The package ships Cursor's declaration, its protocol and the process half
//! that runs one turn ([`driver`]): the workspace binding, the chat
//! creation/resume, the `cursor-agent` launch on the host's pty, the stream
//! classification and the turn's outcome. `licoup-native`'s composition names
//! [`driver`] directly and keeps no Cursor driver module of its own, so the
//! launch, the probe and the turn are described in exactly one place.
//!
//! What stays the client's is what the client owns: the conversation lane that
//! admits, normalizes and cancels a turn, the event consumer
//! ([`port::turn_event`]), the hosted Cursor history projection, and the local
//! Subagent MCP caller registration. The terminal the turn is launched on is
//! neither half's private mechanism: it is the shared pty primitive in
//! `licoup-foundation`. The agent-execution port ([`port::execution`]) remains
//! declared and fail-closed until the extension host starts this package's own
//! binary, which is what answers it.

pub mod driver;
pub mod errors;
pub mod model;
pub mod parser;
pub mod port;
pub mod registration;
pub mod replay;

#[cfg(test)]
mod tests;
