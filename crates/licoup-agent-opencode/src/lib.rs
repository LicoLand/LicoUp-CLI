//! The OpenCode adapter package.
//!
//! One Agent, one package, one program. This crate owns everything LicoUp knows
//! about OpenCode's own `serve` protocol: the HTTP documents and SSE event
//! frames that classify one OpenCode line exactly once below the adapter port
//! ([`parser`]), the Agent's own half of one turn ([`driver`]) and the endpoint
//! contract that turn runs on ([`policy`]), the registration composition injects
//! into the adapter SDK ([`registration`]), the recorded-transcript replay arm
//! ([`replay`]), and the ports the host answers ([`port`]) installed by the one
//! call that states them ([`host`]).
//!
//! # The boundaries this crate keeps
//!
//! - **No client crate.** Nothing here reaches into `licoup-native` or any other
//!   composition crate. What the package needs from its host arrives through
//!   [`port`], and the host installs it.
//! - **One parse.** A raw `serve` document becomes this Agent's facts here and is
//!   never re-parsed above: the parser is the sole ingress, per ADR-0008.
//! - **The client owns the turn.** The parser reports a completed message, the
//!   stream's assistant text and the protocol's framing failure; it settles no
//!   turn, imposes no implicit timeout, and hides no content. The conversation
//!   layer remains the sole turn authority.
//! - **Fail closed without a host.** An uninstalled port admits nothing rather
//!   than guessing, so the protocol and the replay corpus are fully exercised
//!   with no host at all.
//!
//! # Who owns what around one turn
//!
//! The `serve` *process* half — starting and supervising the endpoint, reading
//! its HTTP documents and its SSE stream, recording raw bytes for a diagnostic
//! record and admitting an active turn so force stop can reach it — is the
//! client's shared local-service engine. The package asks for each of those
//! operations through [`port::serve`], which the client answers from that engine
//! in `licoup-native`'s `opencode_host`, and it reaches the shared active-turn
//! registry's control answer directly because that answer is already this
//! package's vocabulary and needs no translation.
//!
//! [`driver`] therefore owns the whole of an OpenCode turn — the launch
//! declaration, the session-open protocol, the request shaping, the stream
//! classification, the capability probe and the result the shared engine reports
//! — and the client composes it without keeping a module or a tree of its own.
//! What remains client-side is the engine the ports describe and the endpoint
//! facade that configures it (`opencode_serve`), which reads this package's
//! [`policy`] and [`parser`] rather than restating either.

pub mod driver;
pub mod host;
pub mod parser;
pub mod policy;
pub mod port;
pub mod registration;
pub mod replay;

#[cfg(test)]
mod tests;
