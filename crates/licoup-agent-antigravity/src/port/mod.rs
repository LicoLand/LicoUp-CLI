//! The ports this package asks the host to answer.
//!
//! A package is a program: it carries one Agent's vendor protocol and it owns
//! nothing the client owns. Everything it needs from its host arrives here as a
//! value composition installs once per process, and every port is fail-closed
//! before it is installed — an uninstalled port answers nothing rather than
//! guessing, so a package started without its host cannot invent an effect.
//!
//! Two seams are declared, and they are deliberately different shapes:
//!
//! - [`turn_event`] is the host's progressive turn-event emission. The client
//!   owns the sink (a CLI `--stream-events` consumer, a Flutter NDJSON reader)
//!   and this package owns the events one Antigravity turn produces, so the
//!   package calls the host rather than reimplementing the sink.
//! - [`execution`] is the agent-execution port. The client owns dispatch,
//!   cancellation, idle-update admission and the Subagent caller context; this
//!   package owns what one Antigravity execution is. The package publishes the
//!   vendor side of that contract, and the host supplies the other side through
//!   the extension host that starts this package's binary.
//!
//! Neither port names a vendor fact about the client: a host that composes no
//! Antigravity package installs nothing and links nothing of this crate.

pub mod execution;
pub mod turn_event;
