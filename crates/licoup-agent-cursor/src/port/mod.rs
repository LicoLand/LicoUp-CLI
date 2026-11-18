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
//!   and this package owns the events one Cursor turn produces, so the package
//!   calls the host rather than reimplementing the sink.
//! - [`execution`] is the agent-execution port. The client owns dispatch,
//!   cancellation and admission; this package owns what one Cursor execution
//!   is. The Subagent MCP caller context is part of this Agent's own surface —
//!   a delegated turn binds the caller a Cursor client was registered with — so
//!   the package names that seam and the host answers it.
//!
//! The turn's own machinery is not a seam: the pty it is launched on is the
//! shared primitive in `licoup-foundation`, and the launch, the stream and the
//! outcome are [`crate::driver`]'s.
//!
//! No port names a vendor fact about the client: a host that composes no Cursor
//! package installs nothing and links nothing of this crate.

pub mod execution;
pub mod turn_event;
