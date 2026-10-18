//! Client-side facade over the managed Gateway Runtime.
//!
//! The runtime process itself lives in `licoup-private-gateway`. This module keeps the
//! kernel's managed-lifecycle commands: start, stop, status, initialize and the
//! pushed verified-readiness reload.

pub mod service;

pub use licoup_public_gateway::telegram::{self, channel_layer_status};
pub use service::{
    REPORT_SCHEMA, reload_conversation_inventory, service_initialize, service_start,
    service_status, service_stop, service_stop_managed, state_directory,
};
