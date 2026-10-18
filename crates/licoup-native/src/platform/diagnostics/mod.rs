//! Native diagnostic owners.
//!
//! This tree holds the diagnostic *ports* that leaf tasks install beside the
//! existing sinks (`log`/`env_logger` for the native host, the Dart diagnostic
//! surfaces for the client). A port added here must adapt to an existing owner
//! rather than open a second history store, log sink, or correlation carrier.

pub mod observation;
