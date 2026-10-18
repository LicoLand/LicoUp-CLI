//! The Claude Code adapter package.
//!
//! One Agent, one package, one program. This crate owns everything LicoUp knows
//! about Claude Code's own protocol and program: the CLI's `stream-json`
//! dialect and the byte-line parser that classifies those frames exactly once
//! below the adapter port ([`protocol`]), the launch and effective-setting
//! shapes that dialect reports, the registration composition injects into the
//! adapter SDK ([`registration`]), the recorded-transcript replay arm
//! ([`replay`]), the process that speaks the dialect ([`driver`]), and the
//! ports the host answers ([`port`]).
//!
//! # The boundaries this crate keeps
//!
//! - **No composition crate.** Nothing here reaches into `licoup-native` or any
//!   other crate that composes Agents. What the package needs from its host
//!   arrives through [`port`], and the host installs it. The process half also
//!   reads the two shared libraries below the composition that name no Agent —
//!   the user-shell environment every launcher observes and the approval park
//!   registry every permission route parks in — exactly as its sibling packages
//!   read them.
//! - **One parse.** A raw CLI line becomes this Agent's effects here and is
//!   never re-parsed above: [`protocol::parser`] is the sole ingress, per
//!   ADR-0008.
//! - **The client owns the turn.** The parser reports protocol finishes, end of
//!   stream and confirmed cancellation; it settles no turn, imposes no implicit
//!   timeout, and hides no content. The conversation layer remains the sole turn
//!   authority.
//! - **The client owns dispatch.** Which conversation is admitted, which turn is
//!   cancelled and when an update may replace a running package belong to the
//!   conversation layer, so the package's process half answers dispatch through
//!   the bounded control entries [`driver::cancel`], [`driver::steer`],
//!   [`driver::cleanup_session`] and [`driver::history`] rather than deciding
//!   any of it. The package owns the process those entries act on: spawning the
//!   CLI, reading its frames, supervising the turn and parking an approval
//!   ([`driver`]).

pub mod driver;
pub mod port;
pub mod protocol;
pub mod registration;
pub mod replay;

#[cfg(test)]
mod tests;
