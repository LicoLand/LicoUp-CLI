//! The Claude Code CLI protocol, below the adapter port.
//!
//! This is the vendor half of the adapter: the `stream-json` wire the CLI
//! writes on its standard output, the launch identity and settings that wire
//! reports, the failure shape this Agent's parser reports, and the byte-line
//! parser that classifies one frame exactly once.
//!
//! Nothing above re-parses a frame. The parser is the sole ingress for this
//! Agent's bytes, which is what ADR-0008 states: one classification, below the
//! port, with the client reading the effects rather than the wire.

pub mod parser;

pub mod control;
pub mod failure;
pub mod launch;
pub mod params;
pub mod settings;

pub use control::PermissionRequest;
pub use failure::{ProtocolFailure, ProtocolFailurePayload};
pub use launch::{FIXED_STREAM_ARGS, LaunchIdentity};
pub use params::DriverConfig;
pub use settings::{CapabilityProbe, EffectiveSettings};

/// This Agent's protocol generation, as a readable format identity.
pub const PROTOCOL_FORMAT: &str = "claude-code.stream-json.v1";

/// The runtime protocol this Agent's CLI lane reports.
pub const RUNTIME_PROTOCOL: &str = "claude-code-cli-stream-json";
