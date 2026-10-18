//! The ports this package asks the host to answer.
//!
//! A package is a program: it carries one Agent's vendor protocol and the
//! Agent's own half of one turn, and it owns nothing the client owns. Everything
//! it needs from its host arrives here as a value composition installs once per
//! process, and every port is fail-closed before it is installed — an
//! uninstalled port answers nothing rather than guessing, so a package started
//! without its host cannot invent an effect.
//!
//! Three seams are declared, and they are deliberately different shapes:
//!
//! - [`turn_event`] is the host's progressive turn-event emission. The client
//!   owns the sink (a CLI `--stream-events` consumer, a Flutter NDJSON reader)
//!   and this package owns the events one Kilo turn produces, so the package
//!   calls the host rather than reimplementing the sink.
//! - [`serve`] is the shared local-service engine. Starting and supervising the
//!   `serve` process, reading its HTTP documents and its SSE stream, observing
//!   raw bytes for a diagnostic record and admitting an active turn for force
//!   stop are all the client's, because they are the same work for every
//!   serve-family Agent. *What* one Kilo turn asks that engine for is this
//!   package's, and it arrives through this seam.
//! - [`execution`] is the agent-execution port. The client owns dispatch,
//!   cancellation and admission; this package owns what one Kilo execution is.
//!   The package publishes the vendor side of that contract, and the host
//!   supplies the other side through the extension host that starts this
//!   package's binary.
//!
//! None of the three names a vendor fact about the client: a host that composes
//! no Kilo package installs nothing and links nothing of this crate.

pub mod execution;
pub mod serve;
pub mod turn_event;
