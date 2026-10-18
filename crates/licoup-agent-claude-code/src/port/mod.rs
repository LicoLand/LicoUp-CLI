//! The ports this package asks the host to answer.
//!
//! A package is a program: it carries one Agent's vendor protocol and it owns
//! nothing the client owns. Everything it needs from its host arrives here as a
//! value composition installs once per process, and the port is fail-closed
//! before it is installed — an uninstalled port answers nothing rather than
//! guessing, so a package started without its host cannot invent an effect.
//!
//! [`execution`] is the agent-execution port. The client owns dispatch,
//! cancellation and admission; this package owns what one Claude Code execution
//! is. The package publishes the vendor side of that contract, and the host
//! supplies the other side through the extension host that starts this
//! package's binary.
//!
//! # The turn-event seam is not a port here yet
//!
//! One Claude Code turn emits progressive events, and *where* they go belongs to
//! the host because the host owns the consumer. This package writes them to
//! `licoup-foundation`'s turn-event bus — the same bus every other adapter in
//! the client writes to, and the same one the host's CLI and Flutter consumers
//! already read. That is deliberate rather than unfinished: the bus is a
//! thread-local sink a thread's dispatch installs, so a package that routed its
//! emitters through an injected port instead would lose every progressive event
//! on any host path that does not install the port. The client's port
//! composition runs on the CLI entry point, not on the Flutter one, so an
//! injected port here would be a behaviour regression today rather than a
//! boundary.
//!
//! Moving that seam is a boundary step, not a move of code: the process half
//! is already this package's ([`crate::driver`]), and the bus it writes to is
//! still the only one every consumer reads. Once one host installs the port on
//! every path that can start Claude Code, the emitters can route through it.
//! Until then this package states the arrangement instead of claiming a port it
//! does not use.

pub mod execution;
