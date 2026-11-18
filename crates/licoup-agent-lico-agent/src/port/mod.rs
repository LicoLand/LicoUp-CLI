//! The ports this package asks its host to answer.
//!
//! A package is a program: it carries one Agent's protocol and it owns nothing
//! the client owns. Everything it needs from its host arrives here as a value
//! composition installs once per process, and every port is fail-closed before
//! it is installed — an uninstalled port answers nothing rather than guessing,
//! so a package started without its host cannot invent an effect.
//!
//! Two seams are declared:
//!
//! - [`execution`] carries the host's own admission decision: whether this host
//!   currently admits new work is a decision the client makes and a package may
//!   not bypass.
//! - [`sandbox`] carries the platform's sandbox primitive. The profile a Plan
//!   turn runs under is this Agent's; that the platform can enforce one at all,
//!   and how a path and a sealed profile reach its runner, is the client's.
//!
//! The Subagent MCP caller context is deliberately *not* a member here: the
//! mesh dispatches `codex`, `cursor`, `antigravity` and `claude-code`, and never
//! Lico Agent, so there is no delegation fact for this Agent to ask about. A
//! package that declared a seam it never reads would be describing a
//! conversation it does not have.

pub mod execution;
pub mod sandbox;
