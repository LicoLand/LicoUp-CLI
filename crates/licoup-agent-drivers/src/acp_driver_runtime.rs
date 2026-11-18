//! Service-neutral ACP process runtime.
//!
//! Vendor adapters contribute only immutable launch metadata. Protocol
//! framing, capability negotiation, session lifecycle, bounded process I/O,
//! failure projection, and result types are owned here so no adapter depends
//! on another vendor implementation.

mod control;
mod errors;
mod events;
mod io;
mod model;
mod params;
pub mod parser_port;
mod probe;
mod protocol;
mod session_plan;
mod settings;
mod stdio_transport;
mod supervision;

pub use control::{ActiveAcpControl, ControlDisposition, cancel_active_turn};
pub use errors::ProtocolFailure;
pub use model::{AcpDriverSpec, CapabilityProbe, EffectiveSettings, RunResult};
pub use params::{ProtocolConfig, timestamp};
pub use probe::probe_acp;
pub use stdio_transport::execute_acp;

/// The recorded-transcript replay arms for the two ACP dialects this crate's
/// reducer serves.
///
/// It is behind `test-support` rather than `cfg(test)` because the replay
/// harness is driven from the host's test build, which links this crate as a
/// dependency — and `cfg(test)` is false for a dependency, so a `cfg(test)` arm
/// is compiled out of exactly the build that calls it. The host enables
/// `licoup-agent-drivers/test-support` from its own `test-support` feature.
#[cfg(any(test, feature = "test-support"))]
pub mod replay;
#[cfg(test)]
mod tests;
