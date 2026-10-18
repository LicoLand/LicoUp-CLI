//! Local-only provider-quota snapshot facade.
//!
//! The per-provider quota fetch approach (local agent credentials to provider
//! quota endpoints, app-server and loopback fallback lanes, adaptive refresh
//! discipline) is reimplemented from the documented behavior of CodexBar
//! (MIT, Peter Steinberger, github.com/steipete/CodexBar), which is credited
//! here as the approach reference. No CodexBar code is copied.

mod antigravity;
mod codex;
mod command;
mod contract;
mod credentials;
mod cursor;
mod http;
mod kimi_code;
mod persistence;
mod redaction;
mod scheduler;

pub use command::snapshot;

/// Cursor session resolution is shared with the hosted Cursor usage ledger in
/// `domain::agent_usage`: one owner for the state-store read, the JWT expiry
/// check, and the WorkOS cookie derivation, so both consumers send the same
/// in-memory credential and neither persists it.
pub(crate) use cursor::{resolve_session, state_db_path};

#[cfg(test)]
mod tests;
