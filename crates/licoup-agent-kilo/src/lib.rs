//! The Kilo Code adapter package.
//!
//! One Agent, one package, one program. This crate owns everything LicoUp knows
//! about Kilo Code's own protocol and program: the `serve` HTTP/SSE documents
//! its endpoint answers and the parser that classifies them exactly once below
//! the adapter port ([`parser`]), the endpoint contract the Agent
//! owns ([`policy`]), this Agent's own half of one turn ([`driver`]), the
//! registration composition injects into the adapter SDK ([`registration`]),
//! the recorded-transcript replay arm ([`replay`]), and the ports the host
//! answers ([`port`]).
//!
//! # The boundaries this crate keeps
//!
//! - **No client crate.** Nothing here reaches into `licoup-native` or any
//!   other composition crate. What the package needs from its host arrives
//!   through [`port`], and the host installs it with one [`host::install`].
//! - **One parse.** A raw serve document becomes this Agent's facts here and is
//!   never re-parsed above: the parser is the sole ingress, per ADR-0008.
//! - **The client owns the turn.** The parser reports a completed message, the
//!   stream's text chunks and the protocol's failures; it settles no turn,
//!   imposes no implicit timeout, and hides no content. The conversation layer
//!   remains the sole turn authority.
//! - **Fail closed without a host.** An uninstalled port emits nothing and
//!   admits nothing rather than guessing, so the protocol and the replay corpus
//!   are fully exercised with no host at all.
//!
//! # Who executes Kilo Code
//!
//! The package owns the Agent's half of a turn and performs it: [`driver`]
//! carries the launch declaration, the capability probe and the turn the host
//! composes, and every engine operation they need arrives through [`port`]. The
//! host keeps what is the host's — the serve engine that starts and supervises
//! the endpoint, the HTTP and SSE reads, the active-turn registry force stop
//! reaches, and the consumer a turn's events go to — and it answers all of them
//! in its own port module rather than in a second Kilo module of its own.
//!
//! The agent-execution port ([`port::execution`]) is what completes the separate
//! route where an extension host starts this package's own binary, so the engine
//! and the consumer travel with the program instead of with the client. Until
//! that route is driven, the client composes this package in-process: the
//! endpoint contract, the protocol and the turn are this package's either way,
//! and the client describes none of them itself.

pub mod driver;
pub mod host;
pub mod parser;
pub mod policy;
pub mod port;
pub mod registration;
pub mod replay;

#[cfg(test)]
mod tests;
