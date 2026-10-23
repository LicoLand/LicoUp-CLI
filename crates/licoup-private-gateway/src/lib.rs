//! LicoUp Private Gateway: the loopback LLM model gateway — model, control and
//! state contracts plus the runtime process that serves them.
//!
//! The runtime reaches conversation readiness only through the port in
//! [`ports::readiness`] that the composing host installs once per process; the
//! LLM credential domain lives in [`credential`], specialized over the
//! `credential-lease` crate's generalized lease machinery.

pub mod control;
pub mod credential;
pub mod default_config;
pub mod http;
pub mod model;
pub mod ports;
pub mod runtime;
pub mod state;
pub use licoup_statistics::gateway as usage;

pub use default_config::default_config;
pub use runtime::{GatewayServeArgs, serve_gateway_runtime};
pub use state::{DEFAULT_PORT, GatewayStatePaths, STATE_DIRECTORY};
