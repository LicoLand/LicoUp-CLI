//! The ports this package asks the host to answer.
//!
//! A package is a program: it carries one Agent's vendor protocol and it owns
//! nothing the client owns. Everything it needs from its host arrives here as a
//! value composition installs once per process, and every port is fail-closed
//! before it is installed — an uninstalled port answers nothing rather than
//! guessing, so a package started without its host cannot invent an effect.
//!
//! Three seams are declared, and they are deliberately different shapes:
//!
//! - [`turn_event`] is the host's progressive turn-event emission. The client
//!   owns the sink (a CLI `--stream-events` consumer, a Flutter NDJSON reader)
//!   and this package owns the events one OpenClaw turn produces, so the package
//!   calls the host rather than reimplementing the sink. It is a process-wide
//!   port because the package's transport and state machine emit from deep inside
//!   frame classification, where there is no caller to hand an answer to.
//! - [`gateway`] is the client's OpenClaw Gateway lifecycle. The client owns the
//!   process it starts and stops, the port scan, the state document and the
//!   bounded HTTP health probe; this package owns *which* endpoint pair an attach
//!   names, in [`crate::policy`] and [`crate::gateway`]. The driver asks the one
//!   question it cannot answer itself — ensure the Gateway and describe it — and
//!   the engine answers with its own stable failure code.
//! - [`execution`] is the agent-execution port. The client owns dispatch,
//!   cancellation and admission; this package owns what one OpenClaw execution
//!   is. The package publishes the vendor side of that contract, and the host
//!   supplies the other side through the extension host that starts this
//!   package's binary.
//!
//! One host fact deliberately has no port: *which* MCP servers a turn registers
//! is the client's own answer, and
//! [`crate::gateway_acp::params::ProtocolConfig::from_params`] takes it as an
//! explicit lazy supplier because that call is the one place the answer is
//! needed. A port would move that fact behind a second global for no gain, and
//! would lose the ordering the kernel has always used — the supplier runs only
//! after the request itself has been accepted. The composition that calls
//! [`crate::driver::execute_with_connection`] passes it at that call site.
//!
//! No port names a vendor fact about the client: a host that composes no
//! OpenClaw package installs nothing and links nothing of this crate.

pub mod execution;
pub mod gateway;
pub mod turn_event;
