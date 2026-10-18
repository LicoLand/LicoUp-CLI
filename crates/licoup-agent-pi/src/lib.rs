//! The Pi adapter package.
//!
//! One Agent, one package, one program. This crate owns everything LicoUp knows
//! about Pi's own protocol and program: the Pi RPC JSONL vocabulary and the
//! process half that runs one turn ([`driver`]), the byte-line parser that
//! classifies those frames exactly once below the adapter port ([`parser`]), the
//! registration composition injects into the adapter SDK ([`registration`]), the
//! recorded-transcript replay arm ([`replay`]), and the ports the host answers
//! ([`port`]).
//!
//! # The boundaries this crate keeps
//!
//! - **No client crate.** Nothing here reaches into `licoup-native` or any other
//!   composition crate. What the package needs from its host arrives through
//!   [`port`], and the host installs it.
//! - **One parse.** A raw Pi RPC line becomes this Agent's effects here and is
//!   never re-parsed above: the parser is the sole ingress, per ADR-0008.
//! - **The client owns the turn.** The parser reports protocol finishes, end of
//!   stream and confirmed cancellation; it settles no turn, imposes no implicit
//!   timeout, and hides no content. The conversation layer remains the sole turn
//!   authority.
//! - **Fail closed without a host.** An uninstalled port emits nothing and
//!   admits nothing rather than guessing, so the protocol and the replay corpus
//!   are fully exercised with no host at all.
//!
//! # Who reaches this package
//!
//! The kernel reaches this Agent's driver only here: `licoup-native`'s
//! composition names [`driver`] directly and declares no Pi driver module or
//! tree of its own, so one Pi execution is described in exactly one place. The
//! host keeps its own projection of a Pi result, because that vocabulary is the
//! host's, and it keeps *where* a Pi turn's events go, because the host owns the
//! consumer.
//!
//! # The ports this package declares
//!
//! [`port::turn_event`] is the host's progressive turn-event emission, which the
//! client owns because the client owns the consumer. [`port::execution`] is the
//! agent-execution port the extension host answers when it starts this package's
//! binary: dispatch, admission and the Subagent caller context belong to the
//! host. Until that side is installed the execution port is fail-closed.

pub mod driver;
pub mod parser;
pub mod port;
pub mod registration;
pub mod replay;

#[cfg(test)]
mod tests;
