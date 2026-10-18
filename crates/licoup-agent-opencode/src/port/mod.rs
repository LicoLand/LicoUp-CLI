//! The ports this package asks its host to answer.
//!
//! A package is a program: it carries one Agent's vendor protocol and it owns
//! nothing the client owns. Everything it needs from its host arrives here as a
//! value composition installs once per process, and every port is fail-closed
//! before it is installed — an uninstalled port answers nothing rather than
//! guessing, so a package started without its host cannot invent an effect.
//!
//! Three seams are declared, and each is the reason this module exists at all:
//!
//! - [`serve`] reaches the client's shared local-service engine — starting and
//!   supervising the endpoint, reading its HTTP documents, framing its SSE
//!   stream, recording raw bytes and admitting an active turn for force stop.
//!   The engine is protocol-agnostic and stays the client's; this package states
//!   only what one OpenCode turn asks of it.
//! - [`turn_event`] carries where the events one turn produces go, because the
//!   host owns the consumer that reads them.
//! - [`execution`] carries the host's own admission decision, which is the one
//!   fact a turn may not derive for itself: a package that could skip it would be
//!   able to start work while the client is replacing installed state.

pub mod execution;
pub mod serve;
pub mod turn_event;
