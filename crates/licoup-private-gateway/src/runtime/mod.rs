//! Gateway Runtime process: LLM Gateway (lower) + Communication Channels (upper).

mod serve;

pub use serve::{GatewayServeArgs, serve_gateway_runtime};
