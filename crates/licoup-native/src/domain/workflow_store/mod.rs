//! Native host re-export of the independent durable workflow store crate.
//!
//! Durable strategy state, command queues, subscriptions, admission control and
//! their routing contracts are owned by `licoup-workflow-store`. This module
//! keeps the stable `licoup_native::domain::workflow_store` path the host's own
//! consumers already use and holds no implementation of its own.
pub use licoup_workflow_store::*;
