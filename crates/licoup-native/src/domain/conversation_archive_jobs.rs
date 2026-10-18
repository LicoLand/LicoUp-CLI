//! Local-only conversation archive job queue.
//!
//! Every command resolves an explicit local filesystem destination and persists queue state in
//! the selected local client-state root. No network or external-transfer implementation exists.

mod activity;
mod clock;
mod commands;
mod constants;
mod creation;
mod drain;
mod execution;
mod plan;
mod projection;
mod queries;
mod request;
mod retry;
mod store;
mod validation;

pub use commands::{cancel, create, drain, events, list, preview, status};

#[cfg(test)]
mod tests;
