//! The DeepSeek Harness adapter package.
//!
//! One Agent, one package, one program. This crate owns everything LicoUp knows
//! about the DeepSeek Harness SDK's own protocol, its own program, and its own
//! durable session log: the JSON-RPC vocabulary ([`parser`]), the byte-line
//! parser that classifies those frames exactly once below the adapter port
//! ([`parser`]), the process half that speaks that vocabulary — the `--profile
//! sdk` transport, its supervision, the turn it carries and the cleanup of one
//! session ([`driver`]) — the reader that folds the Harness's session log into
//! usage samples ([`session_store`]), the provider catalogue a default
//! installation starts from ([`model_catalog`]), the registration composition
//! injects into the adapter SDK ([`registration`]), the recorded-transcript
//! replay arm ([`replay`]), and the ports the host answers ([`port`]).
//!
//! # The boundaries this crate keeps
//!
//! - **No client crate.** Nothing here reaches into `licoup-native` or any other
//!   composition crate. What the package needs from its host arrives through
//!   [`port`], and the host installs it.
//! - **One parse.** A raw Harness line becomes this Agent's facts here and is
//!   never re-parsed above: the parser is the sole ingress, per ADR-0008, and
//!   [`driver`] builds its requests and reads its frames there rather than
//!   declaring a second copy.
//! - **The client owns the turn.** The parser reports protocol finishes, end of
//!   stream and confirmed cancellation; it settles no turn, imposes no implicit
//!   timeout, and hides no content. The conversation layer remains the sole turn
//!   authority, and [`driver`] reports one turn's outcome to it rather than
//!   deciding what the conversation does next.
//! - **Fail closed without a host.** An uninstalled port emits nothing and
//!   admits nothing rather than guessing, so the protocol and the replay corpus
//!   are fully exercised with no host at all.
//!
//! # Who executes DeepSeek Harness
//!
//! The package ships the Harness's protocol, its program and the process half
//! that runs one turn: `licoup-native`'s composition names [`driver`] directly
//! and keeps no DeepSeek Harness module of its own, so one Harness execution is
//! described in exactly one place. The extension host's binary route is what will
//! move the process behind an agent-execution port, which this package does not
//! declare yet.
//!
//! # The external dependency this package declares
//!
//! The session log this package reads, and the provider catalogue it publishes,
//! belong to the installed DeepSeek Harness and not to LicoUp. The log's
//! physical framing — concatenated Zstandard frames, one durable batch each, and
//! a versioned header record — is implemented natively by [`session_store`],
//! which declares the one generation it reads and refuses the rest;
//! [`model_catalog`] carries the vendor's advisory model rows and names the
//! generation they were transcribed from. Neither needs the vendor's own
//! libraries, and neither starts a Node runtime.

pub mod driver;
pub mod model_catalog;
pub mod parser;
pub mod port;
pub mod registration;
pub mod replay;
pub mod session_store;

#[cfg(test)]
mod tests;
