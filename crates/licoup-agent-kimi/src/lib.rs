//! The Kimi Code adapter package.
//!
//! One Agent, one package, one program. This crate owns everything LicoUp knows
//! about Kimi Code's own protocol and program: the ACP frame dialect Kimi
//! answers with ([`dialect`]), the byte-line parser that classifies those frames
//! exactly once below the adapter port ([`parser`]), the Kimi half of one ACP
//! execution ([`driver`]), the registration composition injects into the
//! adapter SDK ([`registration`]), the recorded-transcript replay arm
//! ([`replay`]), and the ports the host answers ([`port`]).
//!
//! # The boundaries this crate keeps
//!
//! - **No client crate.** Nothing here reaches into `licoup-native` or any other
//!   composition crate. What the package needs from its host arrives through
//!   [`port`], and the host installs it.
//! - **One parse.** A raw ACP line becomes this Agent's effects here and is
//!   never re-parsed above: the parser is the sole ingress, per ADR-0008. The
//!   shared ACP reducer reads it through
//!   `licoup_agent_drivers::acp_driver_runtime::parser_port`, keyed by the driver
//!   identity this package declares.
//! - **The engine stays shared.** The ACP transport engines — bounded process
//!   I/O, capability negotiation, session lifecycle, the two reducers, the
//!   persistent session pool and the control plane — stay in
//!   `licoup-agent-drivers`, because Kimi speaks the published ACP profile and
//!   that profile is not one Agent's protocol. What is Kimi's is the launch
//!   metadata and the frame dialect.
//! - **The client owns the turn.** The dialect reports protocol finishes, end of
//!   stream and confirmed cancellation through the shared transition vocabulary;
//!   it settles no turn, imposes no implicit timeout, and hides no content. The
//!   conversation layer remains the sole turn authority.
//! - **Fail closed without a host.** An uninstalled port admits nothing rather
//!   than guessing, so the dialect and the replay corpus are fully exercised
//!   with no host at all.
//!
//! # The ports this package declares
//!
//! [`port::execution`] is the agent-execution port the extension host answers
//! when it starts this package's binary: dispatch, admission and the Subagent
//! caller context belong to the host, and one Kimi execution belongs here.
//! Until that side is installed the execution port is fail-closed, so a package
//! running outside the client cannot claim it was admitted.
//!
//! # Who reaches this package
//!
//! The kernel reaches this Agent's driver only here: `licoup-native`'s
//! composition names [`driver`] directly and keeps no Kimi module of its own, so
//! one Kimi execution is described in exactly one place. The packaged binary
//! route is completed by the agent-execution port above.

pub mod dialect;
pub mod driver;
pub mod parser;
pub mod port;
pub mod registration;

#[cfg(any(test, feature = "test-support"))]
pub mod replay;

#[cfg(test)]
mod tests;
